//! The gossip Import Family's half of a Package Delta apply: the setters for the six gossip
//! tables, and how to find one row of each.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of gossip.
//!
//! Five of the six tables follow the loot shape: their key is a free identifier space, so one
//! Package band (`is_package_gossip_id`) covers them all. `game_gossip_menu` is the sixth and is
//! update-only ([`lyracore_package_delta::DeltaError::InsertNotSupported`]): its key is a creature
//! template entry, which no Package may invent. It carries no band, so [`clear_package_range`]
//! never touches it.
//!
//! The Claim schema and the setters below are one contract; the tests at the bottom fail if a
//! claimable column has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_gossip_id, FieldValue, Operation, Table as ClaimTable, TracedRow,
};

use crate::game_creature_template;
use crate::{
    game_gossip_menu, game_gossip_menu_profile, game_gossip_menu_profile_option,
    game_gossip_option, game_npc_text, game_npc_text_slot, GossipMenuProfile,
    GossipMenuProfileOption, GossipOption, NpcText, NpcTextSlot,
};

use super::{as_f32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Refuses a gossip claim whose final row would point at no row after this family lands.
///
/// Every reference stays inside the family except `game_gossip_option.entry`, which names a
/// creature template the creatures family owns.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        match row.table() {
            ClaimTable::GossipMenu | ClaimTable::GossipMenuProfile | ClaimTable::NpcTextSlot => {
                let text_id = final_u32(ctx, row, "text_id")?;
                if !npc_text_exists_after_apply(ctx, rows, text_id) {
                    return Err(missing_reference(row, "text_id", text_id));
                }
            }
            ClaimTable::GossipMenuProfileOption => {
                let menu_id = final_u32(ctx, row, "menu_id")?;
                if !menu_profile_exists_after_apply(ctx, rows, menu_id) {
                    return Err(missing_reference(row, "menu_id", menu_id));
                }
            }
            ClaimTable::GossipOption => {
                let entry = final_u32(ctx, row, "entry")?;
                if entry == 0
                    || ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(entry)
                        .is_none()
                {
                    return Err(missing_reference(row, "entry", entry));
                }
            }
            ClaimTable::NpcText => {}
            other => unreachable!("gossip reference check received {other}"),
        }
    }
    Ok(())
}

/// True when `text_id` will name a `game_npc_text` row once this plan lands.
///
/// A Package-band greeting is satisfied by an insert in the same plan; anything else has to be on
/// the shard already. The `quest_exists_after_apply` shape.
fn npc_text_exists_after_apply(ctx: &ReducerContext, rows: &[TracedRow], text_id: u32) -> bool {
    if text_id == 0 {
        return false;
    }
    if is_package_gossip_id(u64::from(text_id)) {
        return rows.iter().any(|row| {
            row.table() == ClaimTable::NpcText
                && row.operation() == Operation::Insert
                && row.row_id() == u64::from(text_id)
        });
    }
    ctx.db.game_npc_text().text_id().find(text_id).is_some()
}

/// True when `menu_id` will name a `game_gossip_menu_profile` row once this plan lands.
fn menu_profile_exists_after_apply(ctx: &ReducerContext, rows: &[TracedRow], menu_id: u32) -> bool {
    if menu_id == 0 {
        return false;
    }
    if is_package_gossip_id(u64::from(menu_id)) {
        return rows.iter().any(|row| {
            row.table() == ClaimTable::GossipMenuProfile
                && row.operation() == Operation::Insert
                && row.row_id() == u64::from(menu_id)
        });
    }
    ctx.db
        .game_gossip_menu_profile()
        .menu_id()
        .find(menu_id)
        .is_some()
}

/// The value `field` will hold once this row's claim lands: the claimed value if the claim sets it,
/// otherwise what the Shard already holds. An update that changes only one column is judged on what
/// the row will hold after the apply, not on the column alone.
fn final_u32(ctx: &ReducerContext, row: &TracedRow, field: &str) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row.fields().get(field).map(|claimed| &claimed.value) {
        return Ok(*value);
    }

    let id = row.row_id();
    let value = match row.table() {
        ClaimTable::GossipMenu => ctx
            .db
            .game_gossip_menu()
            .entry()
            .find(id as u32)
            .map(|menu| menu.text_id),
        ClaimTable::GossipMenuProfile => ctx
            .db
            .game_gossip_menu_profile()
            .menu_id()
            .find(id as u32)
            .map(|profile| profile.text_id),
        ClaimTable::GossipMenuProfileOption => ctx
            .db
            .game_gossip_menu_profile_option()
            .row_id()
            .find(id as u32)
            .map(|option| option.menu_id),
        ClaimTable::GossipOption => ctx
            .db
            .game_gossip_option()
            .row_id()
            .find(id as u32)
            .map(|option| option.entry),
        ClaimTable::NpcTextSlot => ctx
            .db
            .game_npc_text_slot()
            .id()
            .find(id)
            .map(|slot| slot.text_id),
        other => unreachable!("gossip value lookup received {other}"),
    };

    value.ok_or_else(|| {
        format!(
            "`{}` row {} vanished during preflight",
            row.table(),
            row.key()
        )
    })
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
    // `game_gossip_menu` permits no insert at all, so no row of it is ever "Package-invented, but
    // no enabled Package inserts it".
    if row.table() != ClaimTable::GossipMenu && updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    if row_is_present(ctx, row) {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

fn row_is_present(ctx: &ReducerContext, row: &TracedRow) -> bool {
    let id = row.row_id();
    match row.table() {
        ClaimTable::GossipMenu => ctx.db.game_gossip_menu().entry().find(id as u32).is_some(),
        ClaimTable::GossipMenuProfile => ctx
            .db
            .game_gossip_menu_profile()
            .menu_id()
            .find(id as u32)
            .is_some(),
        ClaimTable::GossipMenuProfileOption => ctx
            .db
            .game_gossip_menu_profile_option()
            .row_id()
            .find(id as u32)
            .is_some(),
        ClaimTable::GossipOption => ctx
            .db
            .game_gossip_option()
            .row_id()
            .find(id as u32)
            .is_some(),
        ClaimTable::NpcText => ctx.db.game_npc_text().text_id().find(id as u32).is_some(),
        ClaimTable::NpcTextSlot => ctx.db.game_npc_text_slot().id().find(id).is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-gossip row before the gossip family's dispatch \
             runs, found {other}"
        ),
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package gossip range is cleared on every apply, so such a row is gone by the time the write
/// pass runs.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_gossip_id(row.row_id())
}

/// Removes every row a Package invented, from each of the five banded tables. `game_gossip_menu`
/// has no band — nothing to clear there.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let profiles = ctx.db.game_gossip_menu_profile();
    for menu_id in package_keys(profiles.iter().map(|row| u64::from(row.menu_id))) {
        profiles.menu_id().delete(menu_id as u32);
    }

    let profile_options = ctx.db.game_gossip_menu_profile_option();
    for row_id in package_keys(profile_options.iter().map(|row| u64::from(row.row_id))) {
        profile_options.row_id().delete(row_id as u32);
    }

    let options = ctx.db.game_gossip_option();
    for row_id in package_keys(options.iter().map(|row| u64::from(row.row_id))) {
        options.row_id().delete(row_id as u32);
    }

    let slots = ctx.db.game_npc_text_slot();
    for id in package_keys(slots.iter().map(|row| row.id)) {
        slots.id().delete(id);
    }

    // The greeting bodies go LAST: the slots above point at them, so clearing a body first would
    // leave a slot naming a row that is already gone for the rest of this transaction.
    let texts = ctx.db.game_npc_text();
    for text_id in package_keys(texts.iter().map(|row| u64::from(row.text_id))) {
        texts.text_id().delete(text_id as u32);
    }
}

/// The Package-band keys of one table, collected before any delete so the iteration does not run
/// against a table it is mutating.
fn package_keys(keys: impl Iterator<Item = u64>) -> Vec<u64> {
    keys.filter(|key| is_package_gossip_id(*key)).collect()
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match row.operation() {
        Operation::Insert => insert_row(ctx, row),
        Operation::Update => update_row(ctx, row),
    }
}

fn insert_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    check_insert_is_whole(row)?;
    let id = row.row_id();
    let failed = |e: &dyn std::fmt::Display| {
        format!("`{}` row {} did not insert: {e}", row.table(), row.key())
    };
    match row.table() {
        ClaimTable::GossipMenuProfile => {
            let mut profile = blank_menu_profile(id as u32);
            apply_fields(row, |field, value| {
                apply_menu_profile_field(&mut profile, field, value)
            })?;
            ctx.db
                .game_gossip_menu_profile()
                .try_insert(profile)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::GossipMenuProfileOption => {
            let mut option = blank_menu_profile_option(id as u32);
            apply_fields(row, |field, value| {
                apply_menu_profile_option_field(&mut option, field, value)
            })?;
            ctx.db
                .game_gossip_menu_profile_option()
                .try_insert(option)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::GossipOption => {
            let mut option = blank_gossip_option(id as u32);
            apply_fields(row, |field, value| {
                apply_gossip_option_field(&mut option, field, value)
            })?;
            ctx.db
                .game_gossip_option()
                .try_insert(option)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::NpcText => {
            let mut text = blank_npc_text(id as u32);
            apply_fields(row, |field, value| {
                apply_npc_text_field(&mut text, field, value)
            })?;
            ctx.db
                .game_npc_text()
                .try_insert(text)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::NpcTextSlot => {
            let mut slot = blank_npc_text_slot(id);
            apply_fields(row, |field, value| {
                apply_npc_text_slot_field(&mut slot, field, value)
            })?;
            ctx.db
                .game_npc_text_slot()
                .try_insert(slot)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::GossipMenu => unreachable!(
            "`check_inventable` refuses every insert on `game_gossip_menu` before a `Claim` can \
             exist; see `DeltaError::InsertNotSupported`"
        ),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-gossip row before the gossip family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

fn update_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    let id = row.row_id();
    let gone = || format!("`{}` row {} vanished mid-apply", row.table(), row.key());
    match row.table() {
        ClaimTable::GossipMenu => {
            let menus = ctx.db.game_gossip_menu();
            let mut menu = menus.entry().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_gossip_menu_field(&mut menu, field, value)
            })?;
            menus.entry().update(menu);
        }
        ClaimTable::GossipMenuProfile => {
            let profiles = ctx.db.game_gossip_menu_profile();
            let mut profile = profiles.menu_id().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_menu_profile_field(&mut profile, field, value)
            })?;
            profiles.menu_id().update(profile);
        }
        ClaimTable::GossipMenuProfileOption => {
            let options = ctx.db.game_gossip_menu_profile_option();
            let mut option = options.row_id().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_menu_profile_option_field(&mut option, field, value)
            })?;
            options.row_id().update(option);
        }
        ClaimTable::GossipOption => {
            let options = ctx.db.game_gossip_option();
            let mut option = options.row_id().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_gossip_option_field(&mut option, field, value)
            })?;
            options.row_id().update(option);
        }
        ClaimTable::NpcText => {
            let texts = ctx.db.game_npc_text();
            let mut text = texts.text_id().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_npc_text_field(&mut text, field, value)
            })?;
            texts.text_id().update(text);
        }
        ClaimTable::NpcTextSlot => {
            let slots = ctx.db.game_npc_text_slot();
            let mut slot = slots.id().find(id).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_npc_text_slot_field(&mut slot, field, value)
            })?;
            slots.id().update(slot);
        }
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-gossip row before the gossip family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

/// Runs one table's setter over every claimed column. Six tables share the loop, so it lives here
/// rather than six times over.
fn apply_fields(
    row: &TracedRow,
    mut set: impl FnMut(&str, &FieldValue) -> Result<(), String>,
) -> Result<(), String> {
    for (field, claimed) in row.fields() {
        set(field, &claimed.value)?;
    }
    Ok(())
}

// ===========================================================================================
//  Pure row building.
// ===========================================================================================

fn apply_gossip_menu_field(
    menu: &mut crate::GossipMenu,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "text_id" => menu.text_id = as_u32(field, value)?,
        other => return Err(no_such_column("game_gossip_menu", other)),
    }
    Ok(())
}

fn blank_menu_profile(menu_id: u32) -> GossipMenuProfile {
    GossipMenuProfile {
        menu_id,
        text_id: 0,
    }
}

fn apply_menu_profile_field(
    profile: &mut GossipMenuProfile,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "text_id" => profile.text_id = as_u32(field, value)?,
        other => return Err(no_such_column("game_gossip_menu_profile", other)),
    }
    Ok(())
}

fn blank_menu_profile_option(row_id: u32) -> GossipMenuProfileOption {
    GossipMenuProfileOption {
        row_id,
        menu_id: 0,
        option_index: 0,
        icon: 0,
        text: String::new(),
        action: 0,
        action_menu_id: 0,
        cond_type: 0,
        cond_value1: 0,
        cond_value2: 0,
    }
}

fn apply_menu_profile_option_field(
    option: &mut GossipMenuProfileOption,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "menu_id" => option.menu_id = as_u32(field, value)?,
        "option_index" => option.option_index = as_u32(field, value)?,
        "icon" => option.icon = as_u32(field, value)?,
        "text" => option.text = as_str(field, value)?,
        "action" => option.action = as_u32(field, value)?,
        "action_menu_id" => option.action_menu_id = as_u32(field, value)?,
        "cond_type" => option.cond_type = as_u32(field, value)?,
        "cond_value1" => option.cond_value1 = as_u32(field, value)?,
        "cond_value2" => option.cond_value2 = as_u32(field, value)?,
        other => return Err(no_such_column("game_gossip_menu_profile_option", other)),
    }
    Ok(())
}

fn blank_gossip_option(row_id: u32) -> GossipOption {
    GossipOption {
        row_id,
        entry: 0,
        option_index: 0,
        icon: 0,
        text: String::new(),
        action: 0,
        action_menu_id: 0,
        cond_type: 0,
        cond_value1: 0,
        cond_value2: 0,
    }
}

fn apply_gossip_option_field(
    option: &mut GossipOption,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "entry" => option.entry = as_u32(field, value)?,
        "option_index" => option.option_index = as_u32(field, value)?,
        "icon" => option.icon = as_u32(field, value)?,
        "text" => option.text = as_str(field, value)?,
        "action" => option.action = as_u32(field, value)?,
        "action_menu_id" => option.action_menu_id = as_u32(field, value)?,
        "cond_type" => option.cond_type = as_u32(field, value)?,
        "cond_value1" => option.cond_value1 = as_u32(field, value)?,
        "cond_value2" => option.cond_value2 = as_u32(field, value)?,
        other => return Err(no_such_column("game_gossip_option", other)),
    }
    Ok(())
}

fn blank_npc_text(text_id: u32) -> NpcText {
    NpcText {
        text_id,
        text: String::new(),
    }
}

fn apply_npc_text_field(text: &mut NpcText, field: &str, value: &FieldValue) -> Result<(), String> {
    match field {
        "text" => text.text = as_str(field, value)?,
        other => return Err(no_such_column("game_npc_text", other)),
    }
    Ok(())
}

fn blank_npc_text_slot(id: u64) -> NpcTextSlot {
    NpcTextSlot {
        id,
        text_id: 0,
        slot_index: 0,
        text_male: String::new(),
        text_female: String::new(),
        probability: 0.0,
    }
}

fn apply_npc_text_slot_field(
    slot: &mut NpcTextSlot,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "text_id" => slot.text_id = as_u32(field, value)?,
        "slot_index" => slot.slot_index = as_u8(field, value)?,
        "text_male" => slot.text_male = as_str(field, value)?,
        "text_female" => slot.text_female = as_str(field, value)?,
        "probability" => slot.probability = as_f32(field, value)?,
        other => return Err(no_such_column("game_npc_text_slot", other)),
    }
    Ok(())
}

fn no_such_column(table: &str, column: &str) -> String {
    format!("`{table}` has no claimable column `{column}`")
}

// ===========================================================================================
//  Pure tests. Row WRITING needs a live ReducerContext, which a native test has no way
//  to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, gossip_option_claim, npc_text_claim, plan, some_value, PACKAGE_GOSSIP,
        REAL_NPC_TEXT, WHOLE_GOSSIP_OPTION_ROW, WHOLE_NPC_TEXT_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    /// The Package gossip range is cleared on every apply, so tuning a Package greeting nobody
    /// enables is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_npc_text_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &npc_text_claim(
                PACKAGE_GOSSIP as u32,
                "update",
                r#"{"text":{"type":"string","value":"Hail."}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_npc_text_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &npc_text_claim(
                REAL_NPC_TEXT,
                "update",
                r#"{"text":{"type":"string","value":"Hail."}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_gossip_option_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &gossip_option_claim(PACKAGE_GOSSIP as u32, "insert", WHOLE_GOSSIP_OPTION_ROW),
        )])
        .expect("plan builds");

        let mut option = blank_gossip_option(PACKAGE_GOSSIP as u32);
        apply_fields(&plan.rows[0], |field, value| {
            apply_gossip_option_field(&mut option, field, value)
        })
        .expect("row builds");

        assert_eq!(option.row_id, PACKAGE_GOSSIP as u32);
        assert_eq!(option.entry, 6);
        assert_eq!(option.text, "Tell me of the forge.");
        assert_eq!(option.action, 1);
    }

    #[test]
    fn an_inserted_npc_text_carries_its_body_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &npc_text_claim(PACKAGE_GOSSIP as u32, "insert", WHOLE_NPC_TEXT_ROW),
        )])
        .expect("plan builds");

        let mut text = blank_npc_text(PACKAGE_GOSSIP as u32);
        apply_fields(&plan.rows[0], |field, value| {
            apply_npc_text_field(&mut text, field, value)
        })
        .expect("row builds");

        assert_eq!(text.text, "The forge is cold, friend.");
    }

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_gossip_column_has_a_setter() {
        let mut menu = crate::GossipMenu {
            entry: 6,
            text_id: 0,
        };
        let mut profile = blank_menu_profile(PACKAGE_GOSSIP as u32);
        let mut profile_option = blank_menu_profile_option(PACKAGE_GOSSIP as u32);
        let mut option = blank_gossip_option(PACKAGE_GOSSIP as u32);
        let mut text = blank_npc_text(PACKAGE_GOSSIP as u32);
        let mut slot = blank_npc_text_slot(PACKAGE_GOSSIP);

        for column in ClaimTable::GossipMenu.columns() {
            apply_gossip_menu_field(&mut menu, column.name, &some_value(*column)).expect("setter");
        }
        for column in ClaimTable::GossipMenuProfile.columns() {
            apply_menu_profile_field(&mut profile, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::GossipMenuProfileOption.columns() {
            apply_menu_profile_option_field(&mut profile_option, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::GossipOption.columns() {
            apply_gossip_option_field(&mut option, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::NpcText.columns() {
            apply_npc_text_field(&mut text, column.name, &some_value(*column)).expect("setter");
        }
        for column in ClaimTable::NpcTextSlot.columns() {
            apply_npc_text_slot_field(&mut slot, column.name, &some_value(*column))
                .expect("setter");
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut slot = blank_npc_text_slot(PACKAGE_GOSSIP);

        let refusal = apply_npc_text_slot_field(&mut slot, "text_id", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("text_id"), "{refusal}");
    }
}
