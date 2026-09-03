//! The globals Import Family's half of a Package Delta apply: the setters for the seven world-wide
//! reference tables, and how to find one row of each.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of globals.
//!
//! Three tables follow the loot shape: `game_graveyard_zone`, `game_createinfo_spell` and
//! `game_createinfo_action` key on a free surrogate, so one Package band
//! (`is_package_globals_id`) covers them. The other four are update-only
//! ([`lyracore_package_delta::DeltaError::InsertNotSupported`]): the two stat curves and
//! `game_start_position` key on a race, class and level the client fixes, and
//! `game_areatrigger_teleport` keys on an `AreaTrigger.dbc` trigger id. None of the four carries a
//! band, so [`clear_package_range`] never touches them.
//!
//! Two references are checked, and only two. `game_graveyard_zone.safe_loc_id` names a
//! `game_graveyard` row and `game_createinfo_spell.spell_id` names a `game_spell` row, so both
//! resolve against a catalogue this shard holds. `game_areatrigger_teleport.trigger_id` is NOT
//! checked against `game_area_trigger`: that table comes from the importer's `--dbc` pass, which a
//! `--dump`-only shard has never run, so the check would refuse every legitimate claim there.
//! `game_createinfo_action.action` is not checked either — it is polymorphic on `action_type`
//! (spell, item, macro), so no single catalogue answers it.
//!
//! The Claim schema and the setters below are one contract; the tests at the bottom fail if a
//! claimable column has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_globals_id, FieldValue, Operation, Table as ClaimTable, TracedRow,
};

use crate::{
    game_areatrigger_teleport, game_class_level_stats, game_createinfo_action,
    game_createinfo_spell, game_graveyard, game_graveyard_zone, game_level_stats, game_spell,
    game_start_position, AreatriggerTeleport, ClassLevelStats, CreateinfoAction, CreateinfoSpell,
    GraveyardZone, LevelStats, StartPosition,
};

use super::{as_f32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Refuses a globals claim whose final row would point at data this Shard does not hold.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        match row.table() {
            ClaimTable::GraveyardZone => {
                let safe_loc_id = final_u32(ctx, row, "safe_loc_id")?;
                if safe_loc_id == 0 || ctx.db.game_graveyard().id().find(safe_loc_id).is_none() {
                    return Err(missing_reference(row, "safe_loc_id", safe_loc_id));
                }
            }
            ClaimTable::CreateinfoSpell => {
                let spell_id = final_u32(ctx, row, "spell_id")?;
                if spell_id == 0 || ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                    return Err(missing_reference(row, "spell_id", spell_id));
                }
            }
            ClaimTable::ClassLevelStats
            | ClaimTable::LevelStats
            | ClaimTable::StartPosition
            | ClaimTable::AreatriggerTeleport
            | ClaimTable::CreateinfoAction => {}
            other => unreachable!("globals reference check received {other}"),
        }
    }
    Ok(())
}

/// The value `field` will hold once this row's claim lands: the claimed value if the claim sets it,
/// otherwise what the Shard already holds.
fn final_u32(ctx: &ReducerContext, row: &TracedRow, field: &str) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row.fields().get(field).map(|claimed| &claimed.value) {
        return Ok(*value);
    }

    let id = row.row_id();
    let value = match row.table() {
        ClaimTable::GraveyardZone => ctx
            .db
            .game_graveyard_zone()
            .row_id()
            .find(id)
            .map(|zone| zone.safe_loc_id),
        ClaimTable::CreateinfoSpell => ctx
            .db
            .game_createinfo_spell()
            .id()
            .find(id)
            .map(|grant| grant.spell_id),
        other => unreachable!("globals value lookup received {other}"),
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
    if has_package_band(row.table()) && updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    if row_is_present(ctx, row) {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

/// True for the three tables whose key is a free surrogate. The other four permit no insert, so no
/// row of them is ever "Package-invented, but no enabled Package inserts it".
const fn has_package_band(table: ClaimTable) -> bool {
    matches!(
        table,
        ClaimTable::GraveyardZone | ClaimTable::CreateinfoSpell | ClaimTable::CreateinfoAction
    )
}

fn row_is_present(ctx: &ReducerContext, row: &TracedRow) -> bool {
    let id = row.row_id();
    match row.table() {
        ClaimTable::ClassLevelStats => ctx
            .db
            .game_class_level_stats()
            .class_level()
            .find(id as u32)
            .is_some(),
        ClaimTable::LevelStats => ctx
            .db
            .game_level_stats()
            .race_class_level()
            .find(id as u32)
            .is_some(),
        ClaimTable::StartPosition => ctx
            .db
            .game_start_position()
            .race_class()
            .find(id as u16)
            .is_some(),
        ClaimTable::GraveyardZone => ctx.db.game_graveyard_zone().row_id().find(id).is_some(),
        ClaimTable::AreatriggerTeleport => ctx
            .db
            .game_areatrigger_teleport()
            .trigger_id()
            .find(id as u32)
            .is_some(),
        ClaimTable::CreateinfoSpell => ctx.db.game_createinfo_spell().id().find(id).is_some(),
        ClaimTable::CreateinfoAction => ctx.db.game_createinfo_action().row_id().find(id).is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-globals row before the globals family's \
             dispatch runs, found {other}"
        ),
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package globals range is cleared on every apply, so such a row is gone by the time the write
/// pass runs.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_globals_id(row.row_id())
}

/// Removes every row a Package invented, from each of the three banded tables.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let zones = ctx.db.game_graveyard_zone();
    let stale: Vec<u64> = zones
        .iter()
        .map(|row| row.row_id)
        .filter(|id| is_package_globals_id(*id))
        .collect();
    for row_id in stale {
        zones.row_id().delete(row_id);
    }

    let grants = ctx.db.game_createinfo_spell();
    let stale: Vec<u64> = grants
        .iter()
        .map(|row| row.id)
        .filter(|id| is_package_globals_id(*id))
        .collect();
    for id in stale {
        grants.id().delete(id);
    }

    let buttons = ctx.db.game_createinfo_action();
    let stale: Vec<u64> = buttons
        .iter()
        .map(|row| row.row_id)
        .filter(|id| is_package_globals_id(*id))
        .collect();
    for row_id in stale {
        buttons.row_id().delete(row_id);
    }
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
        ClaimTable::GraveyardZone => {
            let mut zone = blank_graveyard_zone(id);
            apply_fields(row, |field, value| {
                apply_graveyard_zone_field(&mut zone, field, value)
            })?;
            ctx.db
                .game_graveyard_zone()
                .try_insert(zone)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::CreateinfoSpell => {
            let mut grant = blank_createinfo_spell(id);
            apply_fields(row, |field, value| {
                apply_createinfo_spell_field(&mut grant, field, value)
            })?;
            ctx.db
                .game_createinfo_spell()
                .try_insert(grant)
                .map_err(|e| failed(&e))?;
        }
        ClaimTable::CreateinfoAction => {
            let mut button = blank_createinfo_action(id);
            apply_fields(row, |field, value| {
                apply_createinfo_action_field(&mut button, field, value)
            })?;
            ctx.db
                .game_createinfo_action()
                .try_insert(button)
                .map_err(|e| failed(&e))?;
        }
        other => unreachable!(
            "`check_inventable` refuses every insert on `{other}` before a `Claim` can exist; see \
             `DeltaError::InsertNotSupported`"
        ),
    }
    Ok(())
}

fn update_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    let id = row.row_id();
    let gone = || format!("`{}` row {} vanished mid-apply", row.table(), row.key());
    match row.table() {
        ClaimTable::ClassLevelStats => {
            let curve = ctx.db.game_class_level_stats();
            let mut stats = curve.class_level().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_class_level_stats_field(&mut stats, field, value)
            })?;
            curve.class_level().update(stats);
        }
        ClaimTable::LevelStats => {
            let curve = ctx.db.game_level_stats();
            let mut stats = curve.race_class_level().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_level_stats_field(&mut stats, field, value)
            })?;
            curve.race_class_level().update(stats);
        }
        ClaimTable::StartPosition => {
            let positions = ctx.db.game_start_position();
            let mut position = positions.race_class().find(id as u16).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_start_position_field(&mut position, field, value)
            })?;
            positions.race_class().update(position);
        }
        ClaimTable::GraveyardZone => {
            let zones = ctx.db.game_graveyard_zone();
            let mut zone = zones.row_id().find(id).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_graveyard_zone_field(&mut zone, field, value)
            })?;
            zones.row_id().update(zone);
        }
        ClaimTable::AreatriggerTeleport => {
            let portals = ctx.db.game_areatrigger_teleport();
            let mut portal = portals.trigger_id().find(id as u32).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_areatrigger_teleport_field(&mut portal, field, value)
            })?;
            portals.trigger_id().update(portal);
        }
        ClaimTable::CreateinfoSpell => {
            let grants = ctx.db.game_createinfo_spell();
            let mut grant = grants.id().find(id).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_createinfo_spell_field(&mut grant, field, value)
            })?;
            grants.id().update(grant);
        }
        ClaimTable::CreateinfoAction => {
            let buttons = ctx.db.game_createinfo_action();
            let mut button = buttons.row_id().find(id).ok_or_else(gone)?;
            apply_fields(row, |field, value| {
                apply_createinfo_action_field(&mut button, field, value)
            })?;
            buttons.row_id().update(button);
        }
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-globals row before the globals family's \
             dispatch runs, found {other}"
        ),
    }
    Ok(())
}

/// Runs one table's setter over every claimed column. Seven tables share the loop.
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

fn apply_class_level_stats_field(
    stats: &mut ClassLevelStats,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "base_health" => stats.base_health = as_u32(field, value)?,
        "base_mana" => stats.base_mana = as_u32(field, value)?,
        other => return Err(no_such_column("game_class_level_stats", other)),
    }
    Ok(())
}

fn apply_level_stats_field(
    stats: &mut LevelStats,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "strength" => stats.strength = as_u32(field, value)?,
        "agility" => stats.agility = as_u32(field, value)?,
        "stamina" => stats.stamina = as_u32(field, value)?,
        "intellect" => stats.intellect = as_u32(field, value)?,
        "spirit" => stats.spirit = as_u32(field, value)?,
        other => return Err(no_such_column("game_level_stats", other)),
    }
    Ok(())
}

fn apply_start_position_field(
    position: &mut StartPosition,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "map_id" => position.map_id = as_u32(field, value)?,
        "zone_id" => position.zone_id = as_u32(field, value)?,
        "x" => position.x = as_f32(field, value)?,
        "y" => position.y = as_f32(field, value)?,
        "z" => position.z = as_f32(field, value)?,
        "orientation" => position.orientation = as_f32(field, value)?,
        "display_id" => position.display_id = as_u32(field, value)?,
        other => return Err(no_such_column("game_start_position", other)),
    }
    Ok(())
}

fn blank_graveyard_zone(row_id: u64) -> GraveyardZone {
    GraveyardZone {
        row_id,
        safe_loc_id: 0,
        zone_id: 0,
        faction: 0,
    }
}

fn apply_graveyard_zone_field(
    zone: &mut GraveyardZone,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "safe_loc_id" => zone.safe_loc_id = as_u32(field, value)?,
        "zone_id" => zone.zone_id = as_u32(field, value)?,
        "faction" => zone.faction = as_u32(field, value)?,
        other => return Err(no_such_column("game_graveyard_zone", other)),
    }
    Ok(())
}

fn apply_areatrigger_teleport_field(
    portal: &mut AreatriggerTeleport,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "target_map" => portal.target_map = as_u32(field, value)?,
        "x" => portal.x = as_f32(field, value)?,
        "y" => portal.y = as_f32(field, value)?,
        "z" => portal.z = as_f32(field, value)?,
        "o" => portal.o = as_f32(field, value)?,
        "name" => portal.name = as_str(field, value)?,
        other => return Err(no_such_column("game_areatrigger_teleport", other)),
    }
    Ok(())
}

fn blank_createinfo_spell(id: u64) -> CreateinfoSpell {
    CreateinfoSpell {
        id,
        race: 0,
        class: 0,
        spell_id: 0,
    }
}

fn apply_createinfo_spell_field(
    grant: &mut CreateinfoSpell,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "race" => grant.race = as_u8(field, value)?,
        "class" => grant.class = as_u8(field, value)?,
        "spell_id" => grant.spell_id = as_u32(field, value)?,
        other => return Err(no_such_column("game_createinfo_spell", other)),
    }
    Ok(())
}

fn blank_createinfo_action(row_id: u64) -> CreateinfoAction {
    CreateinfoAction {
        row_id,
        race: 0,
        class: 0,
        button: 0,
        action: 0,
        action_type: 0,
    }
}

fn apply_createinfo_action_field(
    button: &mut CreateinfoAction,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "race" => button.race = as_u8(field, value)?,
        "class" => button.class = as_u8(field, value)?,
        "button" => button.button = as_u8(field, value)?,
        "action" => button.action = as_u32(field, value)?,
        "action_type" => button.action_type = as_u8(field, value)?,
        other => return Err(no_such_column("game_createinfo_action", other)),
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
        artifact, class_level_stats_claim, graveyard_zone_claim, plan, some_value, PACKAGE_GLOBALS,
        REAL_GRAVEYARD_ZONE, WHOLE_GRAVEYARD_ZONE_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    /// The Package globals range is cleared on every apply, so tuning a Package graveyard link
    /// nobody enables is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_graveyard_zone_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &graveyard_zone_claim(
                PACKAGE_GLOBALS,
                "update",
                r#"{"zone_id":{"type":"u32","value":40}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_graveyard_zone_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &graveyard_zone_claim(
                REAL_GRAVEYARD_ZONE,
                "update",
                r#"{"zone_id":{"type":"u32","value":40}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    /// A stat curve carries no band, so a Package-range identifier on it is not "uninvented" — it
    /// is simply a class and a level no client has.
    #[test]
    fn a_stat_curve_row_is_never_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &class_level_stats_claim(
                1,
                10,
                "update",
                r#"{"base_health":{"type":"u32","value":120}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!has_package_band(plan.rows[0].table()));
    }

    #[test]
    fn an_inserted_graveyard_zone_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &graveyard_zone_claim(PACKAGE_GLOBALS, "insert", WHOLE_GRAVEYARD_ZONE_ROW),
        )])
        .expect("plan builds");

        let mut zone = blank_graveyard_zone(PACKAGE_GLOBALS);
        apply_fields(&plan.rows[0], |field, value| {
            apply_graveyard_zone_field(&mut zone, field, value)
        })
        .expect("row builds");

        assert_eq!(zone.row_id, PACKAGE_GLOBALS);
        assert_eq!(zone.safe_loc_id, 105);
        assert_eq!(zone.zone_id, 12);
    }

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_globals_column_has_a_setter() {
        let mut class_stats = ClassLevelStats {
            class_level: 0,
            class: 1,
            level: 10,
            base_health: 0,
            base_mana: 0,
        };
        let mut level_stats = LevelStats {
            race_class_level: 0,
            race: 1,
            class: 1,
            level: 10,
            strength: 0,
            agility: 0,
            stamina: 0,
            intellect: 0,
            spirit: 0,
        };
        let mut position = StartPosition {
            race_class: 0,
            race: 1,
            class: 1,
            map_id: 0,
            zone_id: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            display_id: 0,
        };
        let mut portal = AreatriggerTeleport {
            trigger_id: 1447,
            target_map: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            o: 0.0,
            name: String::new(),
        };
        let mut zone = blank_graveyard_zone(PACKAGE_GLOBALS);
        let mut grant = blank_createinfo_spell(PACKAGE_GLOBALS);
        let mut button = blank_createinfo_action(PACKAGE_GLOBALS);

        for column in ClaimTable::ClassLevelStats.columns() {
            apply_class_level_stats_field(&mut class_stats, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::LevelStats.columns() {
            apply_level_stats_field(&mut level_stats, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::StartPosition.columns() {
            apply_start_position_field(&mut position, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::AreatriggerTeleport.columns() {
            apply_areatrigger_teleport_field(&mut portal, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::GraveyardZone.columns() {
            apply_graveyard_zone_field(&mut zone, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::CreateinfoSpell.columns() {
            apply_createinfo_spell_field(&mut grant, column.name, &some_value(*column))
                .expect("setter");
        }
        for column in ClaimTable::CreateinfoAction.columns() {
            apply_createinfo_action_field(&mut button, column.name, &some_value(*column))
                .expect("setter");
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut zone = blank_graveyard_zone(PACKAGE_GLOBALS);

        let refusal = apply_graveyard_zone_field(&mut zone, "zone_id", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("zone_id"), "{refusal}");
    }
}
