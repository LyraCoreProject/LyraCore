//! The quest Import Family's half of a Package Delta apply: the setters for the six quest tables a
//! Package may claim, and how to find one row of each.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of quests.
//!
//! One Package identifier band covers the whole family (`is_package_quest_id`, checked against
//! `quest_entry`), the same way the Package spell range covers both `game_spell` and
//! `game_spell_effect`. `game_quest_text` is 1:1 with its header by `quest_entry`; the other four
//! tables carry a derived packed key (`quest_entry` plus a natural per-row index), the same shape
//! `packed_spell_effect_id` documents. `game_creature_quest`/`game_gameobject_quest` (which
//! creature/gameobject starts or ends a quest) are out of this family's claimable catalogue. See
//! `lyracore_package_delta::Table::columns`'s doc for why.
//!
//! The Claim schema and the setters below are one contract; the tests at the bottom fail if a
//! claimable column has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_quest_id, packed_quest_objective_id, packed_quest_reward_choice_id,
    packed_quest_reward_item_id, FieldValue, Operation, PrimaryKey, Table as ClaimTable, TracedRow,
};

use crate::quest::{
    game_quest_cast_objective, game_quest_objective, game_quest_reward_choice,
    game_quest_reward_item, game_quest_template, game_quest_text, QuestCastObjective,
    QuestObjective, QuestRewardChoice, QuestRewardItem, QuestTemplate, QuestText,
};
use crate::{game_creature_template, game_gameobject_template, game_item_template, game_spell};

use super::{as_bool, as_i32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Refuses a quest claim whose final row would point at data this Shard does not hold.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        let quest_entry = row.key().quest_entry();
        if row.table() != ClaimTable::Quest && !quest_exists_after_apply(ctx, rows, quest_entry) {
            return Err(missing_reference(row, "quest_entry", quest_entry));
        }

        match row.table() {
            ClaimTable::Quest => {
                for field in ["prev_quest_id", "next_quest_id"] {
                    if let Some(entry) = claimed_u32(row, field) {
                        if entry != 0 && !quest_exists_after_apply(ctx, rows, entry) {
                            return Err(missing_reference(row, field, entry));
                        }
                    }
                }
                if let Some(entry) = claimed_u32(row, "src_item") {
                    if entry != 0 && !item_exists(ctx, entry) {
                        return Err(missing_reference(row, "src_item", entry));
                    }
                }
            }
            ClaimTable::QuestText => {}
            ClaimTable::QuestObjective => check_objective_reference(ctx, row)?,
            ClaimTable::QuestCastObjective => {
                if let Some(spell_id) = claimed_u32(row, "spell_id") {
                    if spell_id == 0 || ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                        return Err(missing_reference(row, "spell_id", spell_id));
                    }
                }
            }
            ClaimTable::QuestRewardItem => {
                let PrimaryKey::QuestRewardItem { item_entry, .. } = row.key() else {
                    unreachable!("quest reward rows have quest reward keys")
                };
                if !item_exists(ctx, item_entry) {
                    return Err(missing_reference(row, "item_entry", item_entry));
                }
            }
            ClaimTable::QuestRewardChoice => {
                if let Some(item_entry) = claimed_u32(row, "item_entry") {
                    if !item_exists(ctx, item_entry) {
                        return Err(missing_reference(row, "item_entry", item_entry));
                    }
                }
            }
            other => unreachable!("quest reference check received {other}"),
        }
    }
    Ok(())
}

fn check_objective_reference(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    let current = if row.operation() == Operation::Update {
        ctx.db.game_quest_objective().id().find(row.row_id())
    } else {
        None
    };
    let kind = claimed_u8(row, "kind")
        .or_else(|| current.as_ref().map(|objective| objective.kind))
        .ok_or_else(|| format!("`{}` row {} has no objective kind", row.table(), row.key()))?;
    let target = claimed_u32(row, "target_entry")
        .or_else(|| current.as_ref().map(|objective| objective.target_entry))
        .ok_or_else(|| {
            format!(
                "`{}` row {} has no objective target",
                row.table(),
                row.key()
            )
        })?;

    let present = match kind {
        crate::quest::objective_kind::KILL_CREATURE => ctx
            .db
            .game_creature_template()
            .entry()
            .find(target)
            .is_some(),
        crate::quest::objective_kind::COLLECT_ITEM => item_exists(ctx, target),
        crate::quest::objective_kind::USE_GAMEOBJECT => ctx
            .db
            .game_gameobject_template()
            .entry()
            .find(target)
            .is_some(),
        crate::quest::objective_kind::EXPLORE_AREATRIGGER => target != 0,
        _ => {
            return Err(format!(
                "`{}` row {} has unsupported objective kind {kind}",
                row.table(),
                row.key()
            ))
        }
    };
    if present {
        Ok(())
    } else {
        Err(missing_reference(row, "target_entry", target))
    }
}

fn quest_exists_after_apply(ctx: &ReducerContext, rows: &[TracedRow], entry: u32) -> bool {
    if is_package_quest_id(entry) {
        rows.iter().any(|row| {
            row.table() == ClaimTable::Quest
                && row.operation() == Operation::Insert
                && row.key().quest_entry() == entry
        })
    } else {
        ctx.db.game_quest_template().entry().find(entry).is_some()
    }
}

fn item_exists(ctx: &ReducerContext, entry: u32) -> bool {
    entry != 0 && ctx.db.game_item_template().entry().find(entry).is_some()
}

fn claimed_u32(row: &TracedRow, field: &str) -> Option<u32> {
    match row.fields().get(field).map(|claimed| &claimed.value) {
        Some(FieldValue::U32(value)) => Some(*value),
        _ => None,
    }
}

fn claimed_u8(row: &TracedRow, field: &str) -> Option<u8> {
    match row.fields().get(field).map(|claimed| &claimed.value) {
        Some(FieldValue::U8(value)) => Some(*value),
        _ => None,
    }
}

fn missing_reference(row: &TracedRow, field: &str, value: u32) -> String {
    format!(
        "`{}` row {} references missing {field} {value}",
        row.table(),
        row.key()
    )
}

/// What this shard holds for the row an `update` claim names.
pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = match row.table() {
        ClaimTable::Quest => ctx
            .db
            .game_quest_template()
            .entry()
            .find(row.key().quest_entry())
            .is_some(),
        ClaimTable::QuestText => ctx
            .db
            .game_quest_text()
            .quest_entry()
            .find(row.key().quest_entry())
            .is_some(),
        ClaimTable::QuestObjective => ctx
            .db
            .game_quest_objective()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::QuestCastObjective => ctx
            .db
            .game_quest_cast_objective()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::QuestRewardItem => ctx
            .db
            .game_quest_reward_item()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::QuestRewardChoice => ctx
            .db
            .game_quest_reward_choice()
            .id()
            .find(row.row_id())
            .is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-quest row before the quest family's dispatch \
             runs, found {other}"
        ),
    };
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package quest range is cleared on every apply, so such a row is gone by the time the write
/// pass runs: the Package that owns the quest is not enabled, and the one tuning it is claiming a
/// row that does not exist. A base quest is different because the base import puts it there. Every quest
/// table checks the same `quest_entry`, because a child row is only ever as Package-owned as the
/// quest it belongs to.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_quest_id(row.key().quest_entry())
}

/// Removes every row a Package invented, across all six tables. Once per import, right after the
/// base import rewrote them anyway.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let templates = ctx.db.game_quest_template();
    let stale_templates: Vec<u32> = templates
        .iter()
        .filter(|q| is_package_quest_id(q.entry))
        .map(|q| q.entry)
        .collect();
    for entry in stale_templates {
        templates.entry().delete(entry);
    }

    let texts = ctx.db.game_quest_text();
    let stale_texts: Vec<u32> = texts
        .iter()
        .filter(|t| is_package_quest_id(t.quest_entry))
        .map(|t| t.quest_entry)
        .collect();
    for quest_entry in stale_texts {
        texts.quest_entry().delete(quest_entry);
    }

    let objectives = ctx.db.game_quest_objective();
    let stale_objectives: Vec<u64> = objectives
        .iter()
        .filter(|o| is_package_quest_id(o.quest_entry))
        .map(|o| o.id)
        .collect();
    for id in stale_objectives {
        objectives.id().delete(id);
    }

    let cast_objectives = ctx.db.game_quest_cast_objective();
    let stale_cast_objectives: Vec<u64> = cast_objectives
        .iter()
        .filter(|o| is_package_quest_id(o.quest_entry))
        .map(|o| o.id)
        .collect();
    for id in stale_cast_objectives {
        cast_objectives.id().delete(id);
    }

    let reward_items = ctx.db.game_quest_reward_item();
    let stale_reward_items: Vec<u64> = reward_items
        .iter()
        .filter(|r| is_package_quest_id(r.quest_entry))
        .map(|r| r.id)
        .collect();
    for id in stale_reward_items {
        reward_items.id().delete(id);
    }

    let reward_choices = ctx.db.game_quest_reward_choice();
    let stale_reward_choices: Vec<u64> = reward_choices
        .iter()
        .filter(|r| is_package_quest_id(r.quest_entry))
        .map(|r| r.id)
        .collect();
    for id in stale_reward_choices {
        reward_choices.id().delete(id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::Quest, Operation::Insert) => {
            ctx.db
                .game_quest_template()
                .try_insert(built_quest(row)?)
                .map_err(|e| {
                    format!(
                        "`game_quest_template` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::Quest, Operation::Update) => {
            let templates = ctx.db.game_quest_template();
            let mut quest = templates
                .entry()
                .find(row.key().quest_entry())
                .ok_or_else(|| {
                    format!("`game_quest_template` row {} vanished mid-apply", row.key())
                })?;
            for (field, claimed) in row.fields() {
                apply_quest_field(&mut quest, field, &claimed.value)?;
            }
            templates.entry().update(quest);
        }
        (ClaimTable::QuestText, Operation::Insert) => {
            ctx.db
                .game_quest_text()
                .try_insert(built_quest_text(row)?)
                .map_err(|e| format!("`game_quest_text` row {} did not insert: {e}", row.key()))?;
        }
        (ClaimTable::QuestText, Operation::Update) => {
            let texts = ctx.db.game_quest_text();
            let mut text = texts
                .quest_entry()
                .find(row.key().quest_entry())
                .ok_or_else(|| format!("`game_quest_text` row {} vanished mid-apply", row.key()))?;
            for (field, claimed) in row.fields() {
                apply_quest_text_field(&mut text, field, &claimed.value)?;
            }
            texts.quest_entry().update(text);
        }
        (ClaimTable::QuestObjective, Operation::Insert) => {
            ctx.db
                .game_quest_objective()
                .try_insert(built_quest_objective(row)?)
                .map_err(|e| {
                    format!(
                        "`game_quest_objective` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::QuestObjective, Operation::Update) => {
            let objectives = ctx.db.game_quest_objective();
            let mut objective = objectives.id().find(row.row_id()).ok_or_else(|| {
                format!(
                    "`game_quest_objective` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_quest_objective_field(&mut objective, field, &claimed.value)?;
            }
            objectives.id().update(objective);
        }
        (ClaimTable::QuestCastObjective, Operation::Insert) => {
            ctx.db
                .game_quest_cast_objective()
                .try_insert(built_quest_cast_objective(row)?)
                .map_err(|e| {
                    format!(
                        "`game_quest_cast_objective` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::QuestCastObjective, Operation::Update) => {
            let cast_objectives = ctx.db.game_quest_cast_objective();
            let mut cast_objective = cast_objectives.id().find(row.row_id()).ok_or_else(|| {
                format!(
                    "`game_quest_cast_objective` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_quest_cast_objective_field(&mut cast_objective, field, &claimed.value)?;
            }
            cast_objectives.id().update(cast_objective);
        }
        (ClaimTable::QuestRewardItem, Operation::Insert) => {
            ctx.db
                .game_quest_reward_item()
                .try_insert(built_quest_reward_item(row)?)
                .map_err(|e| {
                    format!(
                        "`game_quest_reward_item` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::QuestRewardItem, Operation::Update) => {
            let reward_items = ctx.db.game_quest_reward_item();
            let mut reward_item = reward_items.id().find(row.row_id()).ok_or_else(|| {
                format!(
                    "`game_quest_reward_item` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_quest_reward_item_field(&mut reward_item, field, &claimed.value)?;
            }
            reward_items.id().update(reward_item);
        }
        (ClaimTable::QuestRewardChoice, Operation::Insert) => {
            ctx.db
                .game_quest_reward_choice()
                .try_insert(built_quest_reward_choice(row)?)
                .map_err(|e| {
                    format!(
                        "`game_quest_reward_choice` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::QuestRewardChoice, Operation::Update) => {
            let reward_choices = ctx.db.game_quest_reward_choice();
            let mut reward_choice = reward_choices.id().find(row.row_id()).ok_or_else(|| {
                format!(
                    "`game_quest_reward_choice` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_quest_reward_choice_field(&mut reward_choice, field, &claimed.value)?;
            }
            reward_choices.id().update(reward_choice);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-quest row before the quest family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

// ===========================================================================================
//  Pure row building.
// ===========================================================================================

fn blank_quest(entry: u32) -> QuestTemplate {
    QuestTemplate {
        entry,
        min_level: 0,
        quest_level: 0,
        title: String::new(),
        reward_money: 0,
        reward_xp: 0,
        prev_quest_id: 0,
        required_races: 0,
        required_classes: 0,
        zone_or_sort: 0,
        rew_rep_faction_1: 0,
        rew_rep_value_1: 0,
        rew_rep_faction_2: 0,
        rew_rep_value_2: 0,
        src_item: 0,
        src_item_count: 0,
        repeatable: false,
        next_quest_id: 0,
        limit_time: 0,
        reward_money_max_level: 0,
    }
}

fn apply_quest_field(
    quest: &mut QuestTemplate,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "min_level" => quest.min_level = as_u32(field, value)?,
        "quest_level" => quest.quest_level = as_u32(field, value)?,
        "title" => quest.title = as_str(field, value)?,
        "reward_money" => quest.reward_money = as_u32(field, value)?,
        "reward_xp" => quest.reward_xp = as_u32(field, value)?,
        "prev_quest_id" => quest.prev_quest_id = as_u32(field, value)?,
        "required_races" => quest.required_races = as_u32(field, value)?,
        "required_classes" => quest.required_classes = as_u32(field, value)?,
        "zone_or_sort" => quest.zone_or_sort = as_i32(field, value)?,
        "rew_rep_faction_1" => quest.rew_rep_faction_1 = as_u32(field, value)?,
        "rew_rep_value_1" => quest.rew_rep_value_1 = as_i32(field, value)?,
        "rew_rep_faction_2" => quest.rew_rep_faction_2 = as_u32(field, value)?,
        "rew_rep_value_2" => quest.rew_rep_value_2 = as_i32(field, value)?,
        "src_item" => quest.src_item = as_u32(field, value)?,
        "src_item_count" => quest.src_item_count = as_u32(field, value)?,
        "repeatable" => quest.repeatable = as_bool(field, value)?,
        "next_quest_id" => quest.next_quest_id = as_u32(field, value)?,
        "limit_time" => quest.limit_time = as_u32(field, value)?,
        "reward_money_max_level" => quest.reward_money_max_level = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_template` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_quest(row: &TracedRow) -> Result<QuestTemplate, String> {
    check_insert_is_whole(row)?;
    let mut quest = blank_quest(row.key().quest_entry());
    for (field, claimed) in row.fields() {
        apply_quest_field(&mut quest, field, &claimed.value)?;
    }
    Ok(quest)
}

fn blank_quest_text(quest_entry: u32) -> QuestText {
    QuestText {
        quest_entry,
        details: String::new(),
        objectives: String::new(),
        offer_reward_text: String::new(),
        request_items_text: String::new(),
    }
}

fn apply_quest_text_field(
    text: &mut QuestText,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "details" => text.details = as_str(field, value)?,
        "objectives" => text.objectives = as_str(field, value)?,
        "offer_reward_text" => text.offer_reward_text = as_str(field, value)?,
        "request_items_text" => text.request_items_text = as_str(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_text` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_quest_text(row: &TracedRow) -> Result<QuestText, String> {
    check_insert_is_whole(row)?;
    let mut text = blank_quest_text(row.key().quest_entry());
    for (field, claimed) in row.fields() {
        apply_quest_text_field(&mut text, field, &claimed.value)?;
    }
    Ok(text)
}

fn blank_quest_objective(quest_entry: u32, obj_index: u8) -> QuestObjective {
    QuestObjective {
        id: packed_quest_objective_id(quest_entry, obj_index),
        quest_entry,
        obj_index,
        kind: 0,
        target_entry: 0,
        required_count: 0,
    }
}

fn apply_quest_objective_field(
    objective: &mut QuestObjective,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "kind" => objective.kind = as_u8(field, value)?,
        "target_entry" => objective.target_entry = as_u32(field, value)?,
        "required_count" => objective.required_count = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_objective` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_quest_objective(row: &TracedRow) -> Result<QuestObjective, String> {
    check_insert_is_whole(row)?;
    let PrimaryKey::QuestObjective {
        quest_entry,
        obj_index,
    } = row.key()
    else {
        return Err(format!(
            "`game_quest_objective` row {} has no objective index",
            row.key()
        ));
    };
    let mut objective = blank_quest_objective(quest_entry, obj_index);
    for (field, claimed) in row.fields() {
        apply_quest_objective_field(&mut objective, field, &claimed.value)?;
    }
    Ok(objective)
}

fn blank_quest_cast_objective(quest_entry: u32, obj_index: u8) -> QuestCastObjective {
    QuestCastObjective {
        id: packed_quest_objective_id(quest_entry, obj_index),
        quest_entry,
        obj_index,
        spell_id: 0,
    }
}

fn apply_quest_cast_objective_field(
    cast_objective: &mut QuestCastObjective,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "spell_id" => cast_objective.spell_id = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_cast_objective` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_quest_cast_objective(row: &TracedRow) -> Result<QuestCastObjective, String> {
    check_insert_is_whole(row)?;
    let PrimaryKey::QuestCastObjective {
        quest_entry,
        obj_index,
    } = row.key()
    else {
        return Err(format!(
            "`game_quest_cast_objective` row {} has no objective index",
            row.key()
        ));
    };
    let mut cast_objective = blank_quest_cast_objective(quest_entry, obj_index);
    for (field, claimed) in row.fields() {
        apply_quest_cast_objective_field(&mut cast_objective, field, &claimed.value)?;
    }
    Ok(cast_objective)
}

fn blank_quest_reward_item(quest_entry: u32, item_entry: u32) -> QuestRewardItem {
    QuestRewardItem {
        id: packed_quest_reward_item_id(quest_entry, item_entry),
        quest_entry,
        item_entry,
        count: 0,
    }
}

fn apply_quest_reward_item_field(
    reward_item: &mut QuestRewardItem,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "count" => reward_item.count = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_reward_item` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_quest_reward_item(row: &TracedRow) -> Result<QuestRewardItem, String> {
    check_insert_is_whole(row)?;
    let PrimaryKey::QuestRewardItem {
        quest_entry,
        item_entry,
    } = row.key()
    else {
        return Err(format!(
            "`game_quest_reward_item` row {} has no item entry",
            row.key()
        ));
    };
    let mut reward_item = blank_quest_reward_item(quest_entry, item_entry);
    for (field, claimed) in row.fields() {
        apply_quest_reward_item_field(&mut reward_item, field, &claimed.value)?;
    }
    Ok(reward_item)
}

fn blank_quest_reward_choice(quest_entry: u32, choice_index: u8) -> QuestRewardChoice {
    QuestRewardChoice {
        id: packed_quest_reward_choice_id(quest_entry, choice_index),
        quest_entry,
        choice_index,
        item_entry: 0,
        count: 0,
    }
}

fn apply_quest_reward_choice_field(
    reward_choice: &mut QuestRewardChoice,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "item_entry" => reward_choice.item_entry = as_u32(field, value)?,
        "count" => reward_choice.count = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_reward_choice` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_quest_reward_choice(row: &TracedRow) -> Result<QuestRewardChoice, String> {
    check_insert_is_whole(row)?;
    let PrimaryKey::QuestRewardChoice {
        quest_entry,
        choice_index,
    } = row.key()
    else {
        return Err(format!(
            "`game_quest_reward_choice` row {} has no choice index",
            row.key()
        ));
    };
    let mut reward_choice = blank_quest_reward_choice(quest_entry, choice_index);
    for (field, claimed) in row.fields() {
        apply_quest_reward_choice_field(&mut reward_choice, field, &claimed.value)?;
    }
    Ok(reward_choice)
}

// ===========================================================================================
//  Pure tests. Row WRITING needs a live ReducerContext, which a native test has no way
//  to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, plan, quest_claim, quest_objective_claim, quest_reward_item_claim, some_value,
        PACKAGE_QUEST, REAL_QUEST, WHOLE_QUEST_ROW,
    };
    use super::*;

    /// The Package quest range is cleared on every apply, so tuning a Package quest nobody enables
    /// is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_quest_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &quest_claim(
                PACKAGE_QUEST,
                "update",
                r#"{"reward_money":{"type":"u32","value":100}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_base_quest_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &quest_claim(
                REAL_QUEST,
                "update",
                r#"{"reward_money":{"type":"u32","value":100}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_quest_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &quest_claim(PACKAGE_QUEST, "insert", WHOLE_QUEST_ROW),
        )])
        .expect("plan builds");

        let quest = built_quest(&plan.rows[0]).expect("row builds");

        assert_eq!(quest.entry, PACKAGE_QUEST);
        assert_eq!(quest.title, "A Kindled Errand");
        assert_eq!(quest.reward_money, 100);
    }

    /// The packed key is derived from the quest and the slot, never authored, so the built row
    /// must carry the same value the key packs.
    #[test]
    fn an_inserted_objective_derives_its_packed_key_from_the_quest_and_the_slot() {
        let plan = plan(&[artifact(
            "example.bolt",
            &quest_objective_claim(
                PACKAGE_QUEST,
                2,
                "insert",
                r#"{"kind":{"type":"u8","value":0},"target_entry":{"type":"u32","value":6},"required_count":{"type":"u32","value":5}}"#,
            ),
        )])
        .expect("plan builds");

        let objective = built_quest_objective(&plan.rows[0]).expect("row builds");

        assert_eq!(objective.id, (u64::from(PACKAGE_QUEST) << 8) | 2);
        assert_eq!(objective.quest_entry, PACKAGE_QUEST);
        assert_eq!(objective.obj_index, 2);
        assert_eq!(objective.target_entry, 6);
        assert_eq!(objective.required_count, 5);
    }

    #[test]
    fn an_inserted_reward_item_derives_its_packed_key_from_the_quest_and_the_item() {
        let plan = plan(&[artifact(
            "example.bolt",
            &quest_reward_item_claim(
                PACKAGE_QUEST,
                25,
                "insert",
                r#"{"count":{"type":"u32","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        let reward_item = built_quest_reward_item(&plan.rows[0]).expect("row builds");

        assert_eq!(reward_item.id, (u64::from(PACKAGE_QUEST) << 32) | 25);
        assert_eq!(reward_item.quest_entry, PACKAGE_QUEST);
        assert_eq!(reward_item.item_entry, 25);
        assert_eq!(reward_item.count, 1);
    }

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_quest_column_has_a_setter() {
        let mut quest = blank_quest(PACKAGE_QUEST);
        for column in ClaimTable::Quest.columns() {
            apply_quest_field(&mut quest, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_quest_template` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_quest_text_column_has_a_setter() {
        let mut text = blank_quest_text(PACKAGE_QUEST);
        for column in ClaimTable::QuestText.columns() {
            apply_quest_text_field(&mut text, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_quest_text` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_objective_column_has_a_setter() {
        let mut objective = blank_quest_objective(PACKAGE_QUEST, 0);
        for column in ClaimTable::QuestObjective.columns() {
            apply_quest_objective_field(&mut objective, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_quest_objective` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_cast_objective_column_has_a_setter() {
        let mut cast_objective = blank_quest_cast_objective(PACKAGE_QUEST, 0);
        for column in ClaimTable::QuestCastObjective.columns() {
            apply_quest_cast_objective_field(
                &mut cast_objective,
                column.name,
                &some_value(*column),
            )
            .unwrap_or_else(|e| {
                panic!("`game_quest_cast_objective` column `{}`: {e}", column.name)
            });
        }
    }

    #[test]
    fn every_claimable_reward_item_column_has_a_setter() {
        let mut reward_item = blank_quest_reward_item(PACKAGE_QUEST, 25);
        for column in ClaimTable::QuestRewardItem.columns() {
            apply_quest_reward_item_field(&mut reward_item, column.name, &some_value(*column))
                .unwrap_or_else(|e| {
                    panic!("`game_quest_reward_item` column `{}`: {e}", column.name)
                });
        }
    }

    #[test]
    fn every_claimable_reward_choice_column_has_a_setter() {
        let mut reward_choice = blank_quest_reward_choice(PACKAGE_QUEST, 0);
        for column in ClaimTable::QuestRewardChoice.columns() {
            apply_quest_reward_choice_field(&mut reward_choice, column.name, &some_value(*column))
                .unwrap_or_else(|e| {
                    panic!("`game_quest_reward_choice` column `{}`: {e}", column.name)
                });
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut quest = blank_quest(PACKAGE_QUEST);

        let refusal = apply_quest_field(&mut quest, "reward_money", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("reward_money"), "{refusal}");
    }
}
