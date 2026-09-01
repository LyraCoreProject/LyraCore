//! The gossip Import Family's half of a Package Delta apply: the setters for the six gossip
//! tables a Package may claim, and how to find one row of each.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of gossip.
//!
//! `game_gossip_menu` is update-only ([`lyracore_package_delta::DeltaError::InsertNotSupported`]):
//! its key is the creature template entry, out of this family's scope to invent. It carries no
//! Package band, so [`update_target`] and `write_row` never treat it as `Uninvented`, and
//! [`clear_package_range`] never touches it. The way a Package gives an NPC new words is to insert
//! a `game_npc_text` row in the band and then update this row's `text_id` to point at it. The other
//! five tables follow the loot shape instead: each one's own surrogate key is the band, checked by
//! `is_package_gossip_id`.
//!
//! The Claim schema and the setters below are one contract; the tests at the bottom fail if a
//! claimable column has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_gossip_id, FieldValue, Operation, PrimaryKey, Table as ClaimTable, TracedRow,
};

use crate::creatures::{
    game_gossip_menu, game_gossip_menu_profile, game_gossip_menu_profile_option,
    game_gossip_option, game_npc_text, game_npc_text_slot, GossipMenu, GossipMenuProfile,
    GossipMenuProfileOption, GossipOption, NpcText, NpcTextSlot,
};

use super::{as_f32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Refuses a gossip claim whose final row would point at data this Shard does not hold.
///
/// `action_menu_id` on `game_gossip_option`/`game_gossip_menu_profile_option` is NOT checked: it
/// only names a menu for some values of `action`, and guessing which would refuse valid claims.
/// That is a known gap in v1, not an oversight.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        match row.table() {
            ClaimTable::GossipMenu => {
                let text_id = final_u32(ctx, row, "text_id")?;
                if text_id == 0 || !npc_text_exists_after_apply(ctx, rows, text_id) {
                    return Err(missing_reference(row, "text_id", text_id));
                }
            }
            ClaimTable::NpcText => {}
            ClaimTable::NpcTextSlot => {
                let text_id = final_u32(ctx, row, "text_id")?;
                if text_id == 0 || !npc_text_exists_after_apply(ctx, rows, text_id) {
                    return Err(missing_reference(row, "text_id", text_id));
                }
            }
            ClaimTable::GossipOption => {
                let entry = final_u32(ctx, row, "entry")?;
                if entry == 0 || ctx.db.game_gossip_menu().entry().find(entry).is_none() {
                    return Err(missing_reference(row, "entry", entry));
                }
            }
            ClaimTable::GossipMenuProfile => {
                let text_id = final_u32(ctx, row, "text_id")?;
                if text_id == 0 || !npc_text_exists_after_apply(ctx, rows, text_id) {
                    return Err(missing_reference(row, "text_id", text_id));
                }
            }
            ClaimTable::GossipMenuProfileOption => {
                let menu_id = final_u32(ctx, row, "menu_id")?;
                if menu_id == 0 || !gossip_menu_profile_exists_after_apply(ctx, rows, menu_id) {
                    return Err(missing_reference(row, "menu_id", menu_id));
                }
            }
            other => unreachable!("gossip reference check received {other}"),
        }
    }
    Ok(())
}

/// True when `text_id` will be a `game_npc_text` row once this plan lands: already in the Shard, or
/// inserted by an enabled Package in the SAME plan. The Package gossip range is cleared on every
/// apply, so a Package-range `text_id` can only exist because this plan inserts it.
fn npc_text_exists_after_apply(ctx: &ReducerContext, rows: &[TracedRow], text_id: u32) -> bool {
    if is_package_gossip_id(u64::from(text_id)) {
        rows.iter().any(|row| {
            row.table() == ClaimTable::NpcText
                && row.operation() == Operation::Insert
                && matches!(row.key(), PrimaryKey::NpcText { text_id: t } if t == text_id)
        })
    } else {
        ctx.db.game_npc_text().text_id().find(text_id).is_some()
    }
}

/// True when `menu_id` will be a `game_gossip_menu_profile` row once this plan lands. Same shape as
/// [`npc_text_exists_after_apply`].
fn gossip_menu_profile_exists_after_apply(
    ctx: &ReducerContext,
    rows: &[TracedRow],
    menu_id: u32,
) -> bool {
    if is_package_gossip_id(u64::from(menu_id)) {
        rows.iter().any(|row| {
            row.table() == ClaimTable::GossipMenuProfile
                && row.operation() == Operation::Insert
                && matches!(row.key(), PrimaryKey::GossipMenuProfile { menu_id: m } if m == menu_id)
        })
    } else {
        ctx.db
            .game_gossip_menu_profile()
            .menu_id()
            .find(menu_id)
            .is_some()
    }
}

/// The value `field` will hold once this row's claim lands: the claimed value if the claim sets
/// it, otherwise what the Shard already holds. An update that changes only one column is judged on
/// what the row will hold after the apply, not on the column alone.
fn final_u32(ctx: &ReducerContext, row: &TracedRow, field: &str) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row.fields().get(field).map(|claimed| &claimed.value) {
        return Ok(*value);
    }

    let value = match row.table() {
        ClaimTable::GossipMenu => ctx
            .db
            .game_gossip_menu()
            .entry()
            .find(row.row_id() as u32)
            .map(|menu| match field {
                "text_id" => menu.text_id,
                _ => 0,
            }),
        ClaimTable::NpcTextSlot => {
            ctx.db
                .game_npc_text_slot()
                .id()
                .find(row.row_id())
                .map(|slot| match field {
                    "text_id" => slot.text_id,
                    _ => 0,
                })
        }
        ClaimTable::GossipOption => ctx
            .db
            .game_gossip_option()
            .row_id()
            .find(row.row_id() as u32)
            .map(|option| match field {
                "entry" => option.entry,
                _ => 0,
            }),
        ClaimTable::GossipMenuProfile => ctx
            .db
            .game_gossip_menu_profile()
            .menu_id()
            .find(row.row_id() as u32)
            .map(|profile| match field {
                "text_id" => profile.text_id,
                _ => 0,
            }),
        ClaimTable::GossipMenuProfileOption => ctx
            .db
            .game_gossip_menu_profile_option()
            .row_id()
            .find(row.row_id() as u32)
            .map(|option| match field {
                "menu_id" => option.menu_id,
                _ => 0,
            }),
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
    // Only the five inventable tables have a Package band to reconcile against —
    // `game_gossip_menu` permits no insert at all, so no row of it is ever "Package-invented, but
    // no enabled Package inserts it".
    if row.table() != ClaimTable::GossipMenu && updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = match row.table() {
        ClaimTable::GossipMenu => ctx
            .db
            .game_gossip_menu()
            .entry()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::NpcText => ctx
            .db
            .game_npc_text()
            .text_id()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::NpcTextSlot => ctx
            .db
            .game_npc_text_slot()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::GossipOption => ctx
            .db
            .game_gossip_option()
            .row_id()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::GossipMenuProfile => ctx
            .db
            .game_gossip_menu_profile()
            .menu_id()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::GossipMenuProfileOption => ctx
            .db
            .game_gossip_menu_profile_option()
            .row_id()
            .find(row.row_id() as u32)
            .is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-gossip row before the gossip family's \
             dispatch runs, found {other}"
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
/// The Package gossip range is cleared on every apply, so such a row is gone by the time the write
/// pass runs.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_gossip_id(row.row_id())
}

/// Removes every row a Package invented, across the five inventable tables. `game_gossip_menu` has
/// no band — nothing to clear there. Once per import, right after the base import rewrote them
/// anyway.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let texts = ctx.db.game_npc_text();
    let stale_texts: Vec<u32> = texts
        .iter()
        .filter(|t| is_package_gossip_id(u64::from(t.text_id)))
        .map(|t| t.text_id)
        .collect();
    for text_id in stale_texts {
        texts.text_id().delete(text_id);
    }

    let slots = ctx.db.game_npc_text_slot();
    let stale_slots: Vec<u64> = slots
        .iter()
        .filter(|s| is_package_gossip_id(s.id))
        .map(|s| s.id)
        .collect();
    for id in stale_slots {
        slots.id().delete(id);
    }

    let options = ctx.db.game_gossip_option();
    let stale_options: Vec<u32> = options
        .iter()
        .filter(|o| is_package_gossip_id(u64::from(o.row_id)))
        .map(|o| o.row_id)
        .collect();
    for row_id in stale_options {
        options.row_id().delete(row_id);
    }

    let profiles = ctx.db.game_gossip_menu_profile();
    let stale_profiles: Vec<u32> = profiles
        .iter()
        .filter(|p| is_package_gossip_id(u64::from(p.menu_id)))
        .map(|p| p.menu_id)
        .collect();
    for menu_id in stale_profiles {
        profiles.menu_id().delete(menu_id);
    }

    let profile_options = ctx.db.game_gossip_menu_profile_option();
    let stale_profile_options: Vec<u32> = profile_options
        .iter()
        .filter(|o| is_package_gossip_id(u64::from(o.row_id)))
        .map(|o| o.row_id)
        .collect();
    for row_id in stale_profile_options {
        profile_options.row_id().delete(row_id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::GossipMenu, Operation::Update) => {
            let rows = ctx.db.game_gossip_menu();
            let mut menu = rows.entry().find(row.row_id() as u32).ok_or_else(|| {
                format!("`game_gossip_menu` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_gossip_menu_field(&mut menu, field, &claimed.value)?;
            }
            rows.entry().update(menu);
        }
        (ClaimTable::GossipMenu, Operation::Insert) => unreachable!(
            "`check_inventable` refuses every insert on `game_gossip_menu` before a `Claim` can \
             exist; see `DeltaError::InsertNotSupported`"
        ),
        (ClaimTable::NpcText, Operation::Insert) => {
            ctx.db
                .game_npc_text()
                .try_insert(built_npc_text(row)?)
                .map_err(|e| format!("`game_npc_text` row {} did not insert: {e}", row.key()))?;
        }
        (ClaimTable::NpcText, Operation::Update) => {
            let rows = ctx.db.game_npc_text();
            let mut text = rows
                .text_id()
                .find(row.row_id() as u32)
                .ok_or_else(|| format!("`game_npc_text` row {} vanished mid-apply", row.key()))?;
            for (field, claimed) in row.fields() {
                apply_npc_text_field(&mut text, field, &claimed.value)?;
            }
            rows.text_id().update(text);
        }
        (ClaimTable::NpcTextSlot, Operation::Insert) => {
            ctx.db
                .game_npc_text_slot()
                .try_insert(built_npc_text_slot(row)?)
                .map_err(|e| {
                    format!("`game_npc_text_slot` row {} did not insert: {e}", row.key())
                })?;
        }
        (ClaimTable::NpcTextSlot, Operation::Update) => {
            let rows = ctx.db.game_npc_text_slot();
            let mut slot = rows.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_npc_text_slot` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_npc_text_slot_field(&mut slot, field, &claimed.value)?;
            }
            rows.id().update(slot);
        }
        (ClaimTable::GossipOption, Operation::Insert) => {
            ctx.db
                .game_gossip_option()
                .try_insert(built_gossip_option(row)?)
                .map_err(|e| {
                    format!("`game_gossip_option` row {} did not insert: {e}", row.key())
                })?;
        }
        (ClaimTable::GossipOption, Operation::Update) => {
            let rows = ctx.db.game_gossip_option();
            let mut option = rows.row_id().find(row.row_id() as u32).ok_or_else(|| {
                format!("`game_gossip_option` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_gossip_option_field(&mut option, field, &claimed.value)?;
            }
            rows.row_id().update(option);
        }
        (ClaimTable::GossipMenuProfile, Operation::Insert) => {
            ctx.db
                .game_gossip_menu_profile()
                .try_insert(built_gossip_menu_profile(row)?)
                .map_err(|e| {
                    format!(
                        "`game_gossip_menu_profile` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::GossipMenuProfile, Operation::Update) => {
            let rows = ctx.db.game_gossip_menu_profile();
            let mut profile = rows.menu_id().find(row.row_id() as u32).ok_or_else(|| {
                format!(
                    "`game_gossip_menu_profile` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_gossip_menu_profile_field(&mut profile, field, &claimed.value)?;
            }
            rows.menu_id().update(profile);
        }
        (ClaimTable::GossipMenuProfileOption, Operation::Insert) => {
            ctx.db
                .game_gossip_menu_profile_option()
                .try_insert(built_gossip_menu_profile_option(row)?)
                .map_err(|e| {
                    format!(
                        "`game_gossip_menu_profile_option` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::GossipMenuProfileOption, Operation::Update) => {
            let rows = ctx.db.game_gossip_menu_profile_option();
            let mut option = rows.row_id().find(row.row_id() as u32).ok_or_else(|| {
                format!(
                    "`game_gossip_menu_profile_option` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_gossip_menu_profile_option_field(&mut option, field, &claimed.value)?;
            }
            rows.row_id().update(option);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-gossip row before the gossip family's \
             dispatch runs, found {other}"
        ),
    }
    Ok(())
}

// ===========================================================================================
//  Pure row building.
// ===========================================================================================

fn apply_gossip_menu_field(
    menu: &mut GossipMenu,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "text_id" => menu.text_id = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_gossip_menu` has no claimable column `{other}`"
            ))
        }
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
        other => return Err(format!("`game_npc_text` has no claimable column `{other}`")),
    }
    Ok(())
}

fn built_npc_text(row: &TracedRow) -> Result<NpcText, String> {
    check_insert_is_whole(row)?;
    let mut text = blank_npc_text(row.row_id() as u32);
    for (field, claimed) in row.fields() {
        apply_npc_text_field(&mut text, field, &claimed.value)?;
    }
    Ok(text)
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
        other => {
            return Err(format!(
                "`game_npc_text_slot` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_npc_text_slot(row: &TracedRow) -> Result<NpcTextSlot, String> {
    check_insert_is_whole(row)?;
    let mut slot = blank_npc_text_slot(row.row_id());
    for (field, claimed) in row.fields() {
        apply_npc_text_slot_field(&mut slot, field, &claimed.value)?;
    }
    Ok(slot)
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
        other => {
            return Err(format!(
                "`game_gossip_option` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_gossip_option(row: &TracedRow) -> Result<GossipOption, String> {
    check_insert_is_whole(row)?;
    let mut option = blank_gossip_option(row.row_id() as u32);
    for (field, claimed) in row.fields() {
        apply_gossip_option_field(&mut option, field, &claimed.value)?;
    }
    Ok(option)
}

fn blank_gossip_menu_profile(menu_id: u32) -> GossipMenuProfile {
    GossipMenuProfile {
        menu_id,
        text_id: 0,
    }
}

fn apply_gossip_menu_profile_field(
    profile: &mut GossipMenuProfile,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "text_id" => profile.text_id = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_gossip_menu_profile` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_gossip_menu_profile(row: &TracedRow) -> Result<GossipMenuProfile, String> {
    check_insert_is_whole(row)?;
    let mut profile = blank_gossip_menu_profile(row.row_id() as u32);
    for (field, claimed) in row.fields() {
        apply_gossip_menu_profile_field(&mut profile, field, &claimed.value)?;
    }
    Ok(profile)
}

fn blank_gossip_menu_profile_option(row_id: u32) -> GossipMenuProfileOption {
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

fn apply_gossip_menu_profile_option_field(
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
        other => {
            return Err(format!(
                "`game_gossip_menu_profile_option` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_gossip_menu_profile_option(row: &TracedRow) -> Result<GossipMenuProfileOption, String> {
    check_insert_is_whole(row)?;
    let mut option = blank_gossip_menu_profile_option(row.row_id() as u32);
    for (field, claimed) in row.fields() {
        apply_gossip_menu_profile_option_field(&mut option, field, &claimed.value)?;
    }
    Ok(option)
}

// ===========================================================================================
//  Pure tests. Row WRITING needs a live ReducerContext, which a native test has no way
//  to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, gossip_menu_claim, gossip_menu_profile_claim, gossip_menu_profile_option_claim,
        gossip_option_claim, npc_text_claim, npc_text_slot_claim, plan, some_value,
        PACKAGE_GOSSIP_MENU_PROFILE, PACKAGE_GOSSIP_MENU_PROFILE_OPTION, PACKAGE_GOSSIP_OPTION,
        PACKAGE_NPC_TEXT, PACKAGE_NPC_TEXT_SLOT, REAL_GOSSIP_MENU, REAL_GOSSIP_MENU_PROFILE,
        REAL_GOSSIP_MENU_PROFILE_OPTION, REAL_GOSSIP_OPTION, REAL_NPC_TEXT, REAL_NPC_TEXT_SLOT,
        WHOLE_GOSSIP_MENU_PROFILE_OPTION_ROW, WHOLE_GOSSIP_MENU_PROFILE_ROW,
        WHOLE_GOSSIP_OPTION_ROW, WHOLE_NPC_TEXT_ROW, WHOLE_NPC_TEXT_SLOT_ROW,
    };
    use super::*;

    /// The Package gossip range is cleared on every apply, so tuning a Package `game_npc_text` row
    /// nobody enables is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_npc_text_row_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &npc_text_claim(
                PACKAGE_NPC_TEXT,
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
    fn tuning_a_real_gossip_menu_row_is_never_an_uninvented_package_row() {
        // `game_gossip_menu` carries no Package band at all, so an update on it is never treated
        // as reconciling against an uninvented insert.
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_menu_claim(
                REAL_GOSSIP_MENU,
                "update",
                r#"{"text_id":{"type":"u32","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_package_npc_text_slot_row_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &npc_text_slot_claim(
                PACKAGE_NPC_TEXT_SLOT,
                "update",
                r#"{"probability":{"type":"f32","value":0.5}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_npc_text_slot_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &npc_text_slot_claim(
                REAL_NPC_TEXT_SLOT,
                "update",
                r#"{"probability":{"type":"f32","value":0.5}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_package_gossip_option_row_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_option_claim(
                PACKAGE_GOSSIP_OPTION,
                "update",
                r#"{"icon":{"type":"u32","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_gossip_option_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_option_claim(
                REAL_GOSSIP_OPTION,
                "update",
                r#"{"icon":{"type":"u32","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_package_gossip_menu_profile_row_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_menu_profile_claim(
                PACKAGE_GOSSIP_MENU_PROFILE,
                "update",
                r#"{"text_id":{"type":"u32","value":2}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_gossip_menu_profile_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_menu_profile_claim(
                REAL_GOSSIP_MENU_PROFILE,
                "update",
                r#"{"text_id":{"type":"u32","value":2}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_package_gossip_menu_profile_option_row_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_menu_profile_option_claim(
                PACKAGE_GOSSIP_MENU_PROFILE_OPTION,
                "update",
                r#"{"icon":{"type":"u32","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_gossip_menu_profile_option_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gossip_menu_profile_option_claim(
                REAL_GOSSIP_MENU_PROFILE_OPTION,
                "update",
                r#"{"icon":{"type":"u32","value":1}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_npc_text_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &npc_text_claim(PACKAGE_NPC_TEXT, "insert", WHOLE_NPC_TEXT_ROW),
        )])
        .expect("plan builds");

        let text = built_npc_text(&plan.rows[0]).expect("row builds");

        assert_eq!(text.text_id, PACKAGE_NPC_TEXT);
        assert_eq!(text.text, "The wilds do not forgive.");
    }

    #[test]
    fn an_inserted_npc_text_slot_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &npc_text_slot_claim(PACKAGE_NPC_TEXT_SLOT, "insert", WHOLE_NPC_TEXT_SLOT_ROW),
        )])
        .expect("plan builds");

        let slot = built_npc_text_slot(&plan.rows[0]).expect("row builds");

        assert_eq!(slot.id, PACKAGE_NPC_TEXT_SLOT);
        assert_eq!(slot.text_id, 1);
        assert_eq!(slot.probability, 1.0);
    }

    #[test]
    fn an_inserted_gossip_option_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &gossip_option_claim(PACKAGE_GOSSIP_OPTION, "insert", WHOLE_GOSSIP_OPTION_ROW),
        )])
        .expect("plan builds");

        let option = built_gossip_option(&plan.rows[0]).expect("row builds");

        assert_eq!(option.row_id, PACKAGE_GOSSIP_OPTION);
        assert_eq!(option.entry, 6);
        assert_eq!(option.action, 5);
    }

    #[test]
    fn an_inserted_gossip_menu_profile_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &gossip_menu_profile_claim(
                PACKAGE_GOSSIP_MENU_PROFILE,
                "insert",
                WHOLE_GOSSIP_MENU_PROFILE_ROW,
            ),
        )])
        .expect("plan builds");

        let profile = built_gossip_menu_profile(&plan.rows[0]).expect("row builds");

        assert_eq!(profile.menu_id, PACKAGE_GOSSIP_MENU_PROFILE);
        assert_eq!(profile.text_id, 1);
    }

    #[test]
    fn an_inserted_gossip_menu_profile_option_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &gossip_menu_profile_option_claim(
                PACKAGE_GOSSIP_MENU_PROFILE_OPTION,
                "insert",
                WHOLE_GOSSIP_MENU_PROFILE_OPTION_ROW,
            ),
        )])
        .expect("plan builds");

        let option = built_gossip_menu_profile_option(&plan.rows[0]).expect("row builds");

        assert_eq!(option.row_id, PACKAGE_GOSSIP_MENU_PROFILE_OPTION);
        assert_eq!(option.menu_id, 1);
        assert_eq!(option.action, 5);
    }

    /// `game_gossip_menu` is update-only: the row a claim names always already exists, so there is
    /// no `built_gossip_menu` counterpart to `built_npc_text` above. The setter is still exercised
    /// directly, on a real creature's row.
    #[test]
    fn an_update_on_gossip_menu_carries_the_claimed_text_id_onto_the_row() {
        let plan = plan(&[artifact(
            "example.repoint",
            &gossip_menu_claim(
                REAL_GOSSIP_MENU,
                "update",
                r#"{"text_id":{"type":"u32","value":200}}"#,
            ),
        )])
        .expect("plan builds");

        let mut menu = GossipMenu {
            entry: REAL_GOSSIP_MENU,
            text_id: 0,
        };
        for (field, claimed) in plan.rows[0].fields() {
            apply_gossip_menu_field(&mut menu, field, &claimed.value).expect("setter applies");
        }

        assert_eq!(menu.text_id, 200);
    }

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_gossip_menu_column_has_a_setter() {
        let mut menu = GossipMenu {
            entry: REAL_GOSSIP_MENU,
            text_id: 0,
        };
        for column in ClaimTable::GossipMenu.columns() {
            apply_gossip_menu_field(&mut menu, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_gossip_menu` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_npc_text_column_has_a_setter() {
        let mut text = blank_npc_text(PACKAGE_NPC_TEXT);
        for column in ClaimTable::NpcText.columns() {
            apply_npc_text_field(&mut text, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_npc_text` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_npc_text_slot_column_has_a_setter() {
        let mut slot = blank_npc_text_slot(PACKAGE_NPC_TEXT_SLOT);
        for column in ClaimTable::NpcTextSlot.columns() {
            apply_npc_text_slot_field(&mut slot, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_npc_text_slot` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_gossip_option_column_has_a_setter() {
        let mut option = blank_gossip_option(PACKAGE_GOSSIP_OPTION);
        for column in ClaimTable::GossipOption.columns() {
            apply_gossip_option_field(&mut option, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_gossip_option` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_gossip_menu_profile_column_has_a_setter() {
        let mut profile = blank_gossip_menu_profile(PACKAGE_GOSSIP_MENU_PROFILE);
        for column in ClaimTable::GossipMenuProfile.columns() {
            apply_gossip_menu_profile_field(&mut profile, column.name, &some_value(*column))
                .unwrap_or_else(|e| {
                    panic!("`game_gossip_menu_profile` column `{}`: {e}", column.name)
                });
        }
    }

    #[test]
    fn every_claimable_gossip_menu_profile_option_column_has_a_setter() {
        let mut option = blank_gossip_menu_profile_option(PACKAGE_GOSSIP_MENU_PROFILE_OPTION);
        for column in ClaimTable::GossipMenuProfileOption.columns() {
            apply_gossip_menu_profile_option_field(&mut option, column.name, &some_value(*column))
                .unwrap_or_else(|e| {
                    panic!(
                        "`game_gossip_menu_profile_option` column `{}`: {e}",
                        column.name
                    )
                });
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut text = blank_npc_text(PACKAGE_NPC_TEXT);

        let refusal = apply_npc_text_field(&mut text, "text", &FieldValue::U32(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("text"), "{refusal}");
    }
}
