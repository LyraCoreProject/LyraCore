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

pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
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

fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_gossip_id(row.row_id())
}

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

    // Clear greeting slots before the bodies they reference.
    let texts = ctx.db.game_npc_text();
    for text_id in package_keys(texts.iter().map(|row| u64::from(row.text_id))) {
        texts.text_id().delete(text_id as u32);
    }
}

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

fn apply_fields(
    row: &TracedRow,
    mut set: impl FnMut(&str, &FieldValue) -> Result<(), String>,
) -> Result<(), String> {
    for (field, claimed) in row.fields() {
        set(field, &claimed.value)?;
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, gossip_option_claim, npc_text_claim, plan,
        PACKAGE_GOSSIP, REAL_NPC_TEXT, WHOLE_GOSSIP_OPTION_ROW, WHOLE_NPC_TEXT_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

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

    #[test]
    fn every_claimable_gossip_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::GossipMenu,
            || crate::GossipMenu {
                entry: 6,
                text_id: 0,
            },
            apply_gossip_menu_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::GossipMenuProfile,
            || blank_menu_profile(PACKAGE_GOSSIP as u32),
            apply_menu_profile_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::GossipMenuProfileOption,
            || blank_menu_profile_option(PACKAGE_GOSSIP as u32),
            apply_menu_profile_option_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::GossipOption,
            || blank_gossip_option(PACKAGE_GOSSIP as u32),
            apply_gossip_option_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::NpcText,
            || blank_npc_text(PACKAGE_GOSSIP as u32),
            apply_npc_text_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::NpcTextSlot,
            || blank_npc_text_slot(PACKAGE_GOSSIP),
            apply_npc_text_slot_field,
        );
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut slot = blank_npc_text_slot(PACKAGE_GOSSIP);

        let refusal = apply_npc_text_slot_field(&mut slot, "text_id", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("text_id"), "{refusal}");
    }
}
