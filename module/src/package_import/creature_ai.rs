use std::collections::BTreeMap;

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_creature_ai_id, FieldValue, Operation, Table as ClaimTable, TracedRow,
};

use crate::creatures::{CreatureAiBroadcastText, CreatureAiSummon, CreatureInstruction};
use crate::encounter::EncounterBinding;
use crate::quest::{game_quest_event_requirement, QuestEventRequirement};
use crate::{
    game_creature_ai_broadcast_text, game_creature_ai_definition, game_creature_ai_summon,
    game_quest_template, CreatureAiDefinition,
};

use super::{as_f32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    check_quests_exist(ctx, rows)?;
    check_encounter_ownership(ctx, rows)
}

fn check_quests_exist(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        if row.table() != ClaimTable::QuestEventRequirement {
            continue;
        }
        let quest_entry = final_quest_entry(ctx, row)?;
        if quest_entry == 0
            || ctx
                .db
                .game_quest_template()
                .entry()
                .find(quest_entry)
                .is_none()
        {
            return Err(format!(
                "`{}` row {} references missing quest_entry {quest_entry}",
                row.table(),
                row.key()
            ));
        }
    }
    Ok(())
}

fn final_quest_entry(ctx: &ReducerContext, row: &TracedRow) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row
        .fields()
        .get("quest_entry")
        .map(|claimed| &claimed.value)
    {
        return Ok(*value);
    }

    ctx.db
        .game_quest_event_requirement()
        .id()
        .find(row.row_id())
        .map(|requirement| requirement.quest_entry)
        .ok_or_else(|| {
            format!(
                "`{}` row {} vanished during preflight",
                row.table(),
                row.key()
            )
        })
}

fn check_encounter_ownership(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    let claims_catalogue_row = rows.iter().any(|row| {
        matches!(
            row.table(),
            ClaimTable::CreatureAiBroadcastText | ClaimTable::CreatureAiSummon
        )
    });
    if !claims_catalogue_row {
        return Ok(());
    }

    let owned = encounter_owned_rows(ctx);
    for row in rows {
        let key = match row.table() {
            ClaimTable::CreatureAiBroadcastText => OwnedRow::BroadcastText(row.row_id() as u32),
            ClaimTable::CreatureAiSummon => OwnedRow::Summon(row.row_id() as u32),
            ClaimTable::QuestEventRequirement => continue,
            other => unreachable!(
                "`check_claims_belong_to` refuses a non-creature-ai row before this family's \
                 dispatch runs, found {other}"
            ),
        };
        if let Some(owner) = owned.get(&key) {
            return Err(format!(
                "`{}` row {} is part of {}'s EventAI, which the Encounter Binding {:?} owns; \
                 that encounter's Package decides its fight",
                row.table(),
                row.key(),
                owner.creature,
                owner.binding
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OwnedRow {
    BroadcastText(u32),
    Summon(u32),
}

#[derive(Debug, Clone)]
struct EncounterOwner {
    creature: String,
    binding: EncounterBinding,
}

fn encounter_owned_rows(ctx: &ReducerContext) -> BTreeMap<OwnedRow, EncounterOwner> {
    let mut owned = BTreeMap::new();
    for definition in ctx.db.game_creature_ai_definition().iter() {
        let Some(binding) = notified_binding(&definition) else {
            continue;
        };
        let owner = EncounterOwner {
            creature: subject_of(&definition),
            binding,
        };
        for instruction in definition.rules.iter().flat_map(|rule| &rule.instructions) {
            match instruction {
                CreatureInstruction::Speak(speak) => {
                    for id in &speak.broadcast_ids {
                        owned.insert(OwnedRow::BroadcastText(*id), owner.clone());
                    }
                }
                CreatureInstruction::Summon(summon) => {
                    owned.insert(OwnedRow::Summon(summon.summon_location_id), owner.clone());
                }
                _ => {}
            }
        }
    }
    owned
}

fn notified_binding(definition: &CreatureAiDefinition) -> Option<EncounterBinding> {
    definition
        .rules
        .iter()
        .flat_map(|rule| &rule.instructions)
        .find_map(|instruction| match instruction {
            CreatureInstruction::NotifyEncounter(notify) => Some(notify.binding),
            _ => None,
        })
}

fn subject_of(definition: &CreatureAiDefinition) -> String {
    if definition.creature_entry == 0 {
        format!("creature guid {}", definition.creature_guid)
    } else {
        format!("creature {}", definition.creature_entry)
    }
}

pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if row.operation() == Operation::Update && is_package_creature_ai_id(row.row_id()) {
        return UpdateTarget::Uninvented;
    }
    if row_is_present(ctx, row) {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

fn row_is_present(ctx: &ReducerContext, row: &TracedRow) -> bool {
    match row.table() {
        ClaimTable::CreatureAiBroadcastText => ctx
            .db
            .game_creature_ai_broadcast_text()
            .id()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::CreatureAiSummon => ctx
            .db
            .game_creature_ai_summon()
            .id()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::QuestEventRequirement => ctx
            .db
            .game_quest_event_requirement()
            .id()
            .find(row.row_id())
            .is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-creature-ai row before this family's dispatch \
             runs, found {other}"
        ),
    }
}

pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let texts = ctx.db.game_creature_ai_broadcast_text();
    let stale: Vec<u32> = texts
        .iter()
        .map(|text| text.id)
        .filter(|id| is_package_creature_ai_id(u64::from(*id)))
        .collect();
    for id in stale {
        texts.id().delete(id);
    }

    let summons = ctx.db.game_creature_ai_summon();
    let stale: Vec<u32> = summons
        .iter()
        .map(|summon| summon.id)
        .filter(|id| is_package_creature_ai_id(u64::from(*id)))
        .collect();
    for id in stale {
        summons.id().delete(id);
    }

    let requirements = ctx.db.game_quest_event_requirement();
    let stale: Vec<u64> = requirements
        .iter()
        .map(|requirement| requirement.id)
        .filter(|id| is_package_creature_ai_id(*id))
        .collect();
    for id in stale {
        requirements.id().delete(id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::CreatureAiBroadcastText, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut text = blank_broadcast_text(row.row_id() as u32);
            apply_fields(row, |field, value| {
                apply_broadcast_text_field(&mut text, field, value)
            })?;
            ctx.db
                .game_creature_ai_broadcast_text()
                .try_insert(text)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::CreatureAiBroadcastText, Operation::Update) => {
            let texts = ctx.db.game_creature_ai_broadcast_text();
            let mut text = texts
                .id()
                .find(row.row_id() as u32)
                .ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_broadcast_text_field(&mut text, field, value)
            })?;
            texts.id().update(text);
        }
        (ClaimTable::CreatureAiSummon, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut summon = blank_summon(row.row_id() as u32);
            apply_fields(row, |field, value| {
                apply_summon_field(&mut summon, field, value)
            })?;
            ctx.db
                .game_creature_ai_summon()
                .try_insert(summon)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::CreatureAiSummon, Operation::Update) => {
            let summons = ctx.db.game_creature_ai_summon();
            let mut summon = summons
                .id()
                .find(row.row_id() as u32)
                .ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_summon_field(&mut summon, field, value)
            })?;
            summons.id().update(summon);
        }
        (ClaimTable::QuestEventRequirement, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut requirement = blank_quest_event_requirement(row.row_id());
            apply_fields(row, |field, value| {
                apply_quest_event_requirement_field(&mut requirement, field, value)
            })?;
            ctx.db
                .game_quest_event_requirement()
                .try_insert(requirement)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::QuestEventRequirement, Operation::Update) => {
            let requirements = ctx.db.game_quest_event_requirement();
            let mut requirement = requirements
                .id()
                .find(row.row_id())
                .ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_quest_event_requirement_field(&mut requirement, field, value)
            })?;
            requirements.id().update(requirement);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-creature-ai row before this family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

fn insert_failed(row: &TracedRow, e: &dyn std::fmt::Display) -> String {
    format!("`{}` row {} did not insert: {e}", row.table(), row.key())
}

fn gone(row: &TracedRow) -> String {
    format!("`{}` row {} vanished mid-apply", row.table(), row.key())
}

fn apply_fields(
    row: &TracedRow,
    mut set: impl FnMut(&str, &FieldValue) -> Result<(), String>,
) -> Result<(), String> {
    for (field, claimed) in row.fields() {
        set(field, &claimed.value)?;
    }
    Ok(())
}

fn blank_broadcast_text(id: u32) -> CreatureAiBroadcastText {
    CreatureAiBroadcastText {
        id,
        male_text: String::new(),
        female_text: String::new(),
        chat_type: 0,
        language_id: 0,
        emote_delay_1_ms: 0,
        emote_id_1: 0,
        emote_delay_2_ms: 0,
        emote_id_2: 0,
        emote_delay_3_ms: 0,
        emote_id_3: 0,
    }
}

fn apply_broadcast_text_field(
    text: &mut CreatureAiBroadcastText,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "male_text" => text.male_text = as_str(field, value)?,
        "female_text" => text.female_text = as_str(field, value)?,
        "chat_type" => text.chat_type = as_u8(field, value)?,
        "language_id" => text.language_id = as_u8(field, value)?,
        "emote_delay_1_ms" => text.emote_delay_1_ms = as_u32(field, value)?,
        "emote_id_1" => text.emote_id_1 = as_u32(field, value)?,
        "emote_delay_2_ms" => text.emote_delay_2_ms = as_u32(field, value)?,
        "emote_id_2" => text.emote_id_2 = as_u32(field, value)?,
        "emote_delay_3_ms" => text.emote_delay_3_ms = as_u32(field, value)?,
        "emote_id_3" => text.emote_id_3 = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_creature_ai_broadcast_text` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn blank_summon(id: u32) -> CreatureAiSummon {
    CreatureAiSummon {
        id,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        orientation: 0.0,
        lifetime_ms: 0,
    }
}

fn apply_summon_field(
    summon: &mut CreatureAiSummon,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "x" => summon.x = as_f32(field, value)?,
        "y" => summon.y = as_f32(field, value)?,
        "z" => summon.z = as_f32(field, value)?,
        "orientation" => summon.orientation = as_f32(field, value)?,
        "lifetime_ms" => summon.lifetime_ms = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_creature_ai_summon` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn blank_quest_event_requirement(id: u64) -> QuestEventRequirement {
    QuestEventRequirement { id, quest_entry: 0 }
}

fn apply_quest_event_requirement_field(
    requirement: &mut QuestEventRequirement,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "quest_entry" => requirement.quest_entry = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_quest_event_requirement` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, broadcast_text_claim, plan,
        quest_event_requirement_claim, summon_claim, PACKAGE_BROADCAST_TEXT,
        PACKAGE_QUEST_EVENT_REQUIREMENT, PACKAGE_SUMMON, REAL_BROADCAST_TEXT,
        WHOLE_BROADCAST_TEXT_ROW, WHOLE_QUEST_EVENT_REQUIREMENT_ROW, WHOLE_SUMMON_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    #[test]
    fn tuning_a_package_broadcast_text_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &broadcast_text_claim(
                PACKAGE_BROADCAST_TEXT,
                "update",
                r#"{"chat_type":{"type":"u8","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(is_package_creature_ai_id(plan.rows[0].row_id()));
    }

    #[test]
    fn tuning_an_imported_broadcast_text_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &broadcast_text_claim(
                REAL_BROADCAST_TEXT,
                "update",
                r#"{"chat_type":{"type":"u8","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!is_package_creature_ai_id(plan.rows[0].row_id()));
    }

    #[test]
    fn an_inserted_broadcast_text_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.voice",
            &broadcast_text_claim(PACKAGE_BROADCAST_TEXT, "insert", WHOLE_BROADCAST_TEXT_ROW),
        )])
        .expect("plan builds");

        let mut text = blank_broadcast_text(PACKAGE_BROADCAST_TEXT);
        for (field, claimed) in plan.rows[0].fields() {
            apply_broadcast_text_field(&mut text, field, &claimed.value).expect("the setter runs");
        }

        assert_eq!(text.id, PACKAGE_BROADCAST_TEXT);
        assert_eq!(text.male_text, "The forge remembers.");
        assert_eq!(text.chat_type, 1);
        assert_eq!(text.emote_id_1, 5);
    }

    #[test]
    fn an_inserted_summon_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.summon",
            &summon_claim(PACKAGE_SUMMON, "insert", WHOLE_SUMMON_ROW),
        )])
        .expect("plan builds");

        let mut summon = blank_summon(PACKAGE_SUMMON);
        for (field, claimed) in plan.rows[0].fields() {
            apply_summon_field(&mut summon, field, &claimed.value).expect("the setter runs");
        }

        assert_eq!(summon.id, PACKAGE_SUMMON);
        assert_eq!(summon.lifetime_ms, 30_000);
    }

    #[test]
    fn an_inserted_quest_event_requirement_carries_its_quest_onto_the_row() {
        let plan = plan(&[artifact(
            "example.quest.event",
            &quest_event_requirement_claim(
                PACKAGE_QUEST_EVENT_REQUIREMENT,
                "insert",
                WHOLE_QUEST_EVENT_REQUIREMENT_ROW,
            ),
        )])
        .expect("plan builds");

        let mut requirement = blank_quest_event_requirement(PACKAGE_QUEST_EVENT_REQUIREMENT);
        for (field, claimed) in plan.rows[0].fields() {
            apply_quest_event_requirement_field(&mut requirement, field, &claimed.value)
                .expect("the setter runs");
        }

        assert_eq!(requirement.quest_entry, 8_000_001);
    }

    #[test]
    fn every_claimable_creature_ai_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::CreatureAiBroadcastText,
            || blank_broadcast_text(PACKAGE_BROADCAST_TEXT),
            apply_broadcast_text_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::CreatureAiSummon,
            || blank_summon(PACKAGE_SUMMON),
            apply_summon_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::QuestEventRequirement,
            || blank_quest_event_requirement(PACKAGE_QUEST_EVENT_REQUIREMENT),
            apply_quest_event_requirement_field,
        );
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut text = blank_broadcast_text(PACKAGE_BROADCAST_TEXT);

        let refusal = apply_broadcast_text_field(&mut text, "chat_type", &FieldValue::U32(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("chat_type"), "{refusal}");
    }
}
