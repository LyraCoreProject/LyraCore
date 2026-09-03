//! The gameobjects Import Family's half of a Package Delta apply: the setters for the gameobject
//! catalogue, its trap metadata and its spawns, and how to find one row of each.
//!
//! The shared shell in the parent module decides everything a family does not. What is here is only
//! what is true of gameobjects.
//!
//! Three tables, one band. `game_gameobject_template` and `game_gameobject_trap` share one
//! identifier space on purpose: a trap row describes the template of the same entry, so a Package
//! trap is exactly as Package-owned as its template. `game_gameobject` is SPATIAL, and unlike a
//! creature its row IS the live prop — nothing rebuilds it from a spawn record, so a write here
//! also derives the AOI grid cell the row is addressed through.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_gameobject_id, FieldValue, Operation, PrimaryKey, Table as ClaimTable, TracedRow,
};

use crate::{
    game_gameobject, game_gameobject_template, game_gameobject_trap, GameObject,
    GameObjectTemplate, GameObjectTrap,
};

use super::{as_f32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Bits 0..47 of a gameobject guid: the spawn identifier the importer packs there.
const GAMEOBJECT_GUID_SPAWN_MASK: u64 = 0xFFFF_FFFF_FFFF;

/// Refuses a gameobjects claim whose final row would point at no row after this family lands.
///
/// A spawn and a trap both name a template: the row each describes has to exist once the plan
/// lands. And two spawn claims may derive ONE durable guid, because the map routes a claim but
/// never reaches the guid — the same invalid map-ownership statement the creatures family refuses.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    check_one_guid_per_spawn(rows)?;
    for row in rows {
        let entry = match row.key() {
            PrimaryKey::GameobjectTrap { entry } => entry,
            PrimaryKey::GameobjectSpawn { .. } => final_template_entry(ctx, row)?,
            PrimaryKey::GameobjectTemplate { .. } => continue,
            other => unreachable!("gameobjects reference check received {other}"),
        };
        if !template_exists_after_apply(ctx, rows, entry) {
            return Err(format!(
                "`{}` row {} references missing template entry {entry}",
                row.table(),
                row.key()
            ));
        }
    }
    Ok(())
}

/// Refuses a plan whose spawn claims collide on one derived guid.
fn check_one_guid_per_spawn(rows: &[TracedRow]) -> Result<(), String> {
    let claimed: Vec<&TracedRow> = rows
        .iter()
        .filter(|row| row.table() == ClaimTable::GameobjectSpawn)
        .collect();
    for (index, row) in claimed.iter().enumerate() {
        if let Some(twin) = claimed[index + 1..]
            .iter()
            .find(|other| other.row_id() == row.row_id())
        {
            return Err(format!(
                "`{}` rows {} and {} are one durable gameobject; a spawn belongs to one map",
                row.table(),
                row.key(),
                twin.key()
            ));
        }
    }
    Ok(())
}

/// The template a spawn will name once its claim lands: the claimed value if the claim sets it,
/// otherwise what the Shard already holds.
fn final_template_entry(ctx: &ReducerContext, row: &TracedRow) -> Result<u32, String> {
    if let Some(FieldValue::U32(entry)) = row
        .fields()
        .get("template_entry")
        .map(|claimed| &claimed.value)
    {
        return Ok(*entry);
    }
    ctx.db
        .game_gameobject()
        .guid()
        .find(row.row_id())
        .map(|spawn| spawn.template_entry)
        .ok_or_else(|| {
            format!(
                "`{}` row {} vanished during preflight",
                row.table(),
                row.key()
            )
        })
}

/// True when `entry` will name a `game_gameobject_template` row once this plan lands.
fn template_exists_after_apply(ctx: &ReducerContext, rows: &[TracedRow], entry: u32) -> bool {
    if entry == 0 {
        return false;
    }
    if is_package_gameobject_id(entry) {
        return rows.iter().any(|row| {
            row.table() == ClaimTable::GameobjectTemplate
                && row.operation() == Operation::Insert
                && row.row_id() == u64::from(entry)
        });
    }
    ctx.db
        .game_gameobject_template()
        .entry()
        .find(entry)
        .is_some()
}

/// What this Shard holds for the row an `update` claim names.
pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if row.operation() == Operation::Update && updates_an_uninvented_package_row(row) {
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
        ClaimTable::GameobjectTemplate => ctx
            .db
            .game_gameobject_template()
            .entry()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::GameobjectTrap => ctx
            .db
            .game_gameobject_trap()
            .entry()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::GameobjectSpawn => ctx.db.game_gameobject().guid().find(row.row_id()).is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-gameobjects row before the gameobjects \
             family's dispatch runs, found {other}"
        ),
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    match row.key() {
        PrimaryKey::GameobjectTemplate { entry } | PrimaryKey::GameobjectTrap { entry } => {
            is_package_gameobject_id(entry)
        }
        PrimaryKey::GameobjectSpawn { spawn_id, .. } => is_package_gameobject_id(spawn_id),
        _ => false,
    }
}

/// Removes every row a Package invented: the spawns and the trap metadata first, then the
/// templates they name.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let spawns = ctx.db.game_gameobject();
    let package_guids: Vec<u64> = spawns
        .iter()
        .map(|spawn| spawn.guid)
        .filter(|guid| is_package_spawn_guid(*guid))
        .collect();
    for guid in package_guids {
        spawns.guid().delete(guid);
    }

    let traps = ctx.db.game_gameobject_trap();
    let package_traps: Vec<u32> = traps
        .iter()
        .map(|trap| trap.entry)
        .filter(|entry| is_package_gameobject_id(*entry))
        .collect();
    for entry in package_traps {
        traps.entry().delete(entry);
    }

    let templates = ctx.db.game_gameobject_template();
    let package_entries: Vec<u32> = templates
        .iter()
        .map(|template| template.entry)
        .filter(|entry| is_package_gameobject_id(*entry))
        .collect();
    for entry in package_entries {
        templates.entry().delete(entry);
    }
}

/// True when a durable gameobject guid carries a Package-invented spawn identifier.
fn is_package_spawn_guid(guid: u64) -> bool {
    u32::try_from(guid & GAMEOBJECT_GUID_SPAWN_MASK).is_ok_and(is_package_gameobject_id)
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::GameobjectTemplate, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut template = blank_template(row.row_id() as u32);
            apply_fields(row, |field, value| {
                apply_template_field(&mut template, field, value)
            })?;
            ctx.db
                .game_gameobject_template()
                .try_insert(template)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::GameobjectTemplate, Operation::Update) => {
            let templates = ctx.db.game_gameobject_template();
            let mut template = templates
                .entry()
                .find(row.row_id() as u32)
                .ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_template_field(&mut template, field, value)
            })?;
            templates.entry().update(template);
        }
        (ClaimTable::GameobjectTrap, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut trap = blank_trap(row.row_id() as u32);
            apply_fields(row, |field, value| {
                apply_trap_field(&mut trap, field, value)
            })?;
            ctx.db
                .game_gameobject_trap()
                .try_insert(trap)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::GameobjectTrap, Operation::Update) => {
            let traps = ctx.db.game_gameobject_trap();
            let mut trap = traps
                .entry()
                .find(row.row_id() as u32)
                .ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_trap_field(&mut trap, field, value)
            })?;
            traps.entry().update(trap);
        }
        (ClaimTable::GameobjectSpawn, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut spawn = blank_spawn(ctx, row.key());
            apply_fields(row, |field, value| {
                apply_spawn_field(&mut spawn, field, value)
            })?;
            stamp_cell(&mut spawn);
            ctx.db
                .game_gameobject()
                .try_insert(spawn)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::GameobjectSpawn, Operation::Update) => {
            let spawns = ctx.db.game_gameobject();
            let mut spawn = spawns.guid().find(row.row_id()).ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_spawn_field(&mut spawn, field, value)
            })?;
            stamp_cell(&mut spawn);
            spawns.guid().update(spawn);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-gameobjects row before the gameobjects \
             family's dispatch runs, found {other}"
        ),
    }
    Ok(())
}

/// Re-derives the AOI grid columns from the coordinates in the same write that set them. They are
/// not claimable, and a stale value here does not slow a query down — it shows players the wrong
/// world (`module/src/tripwires.rs`'s grid-cell tripwire).
fn stamp_cell(spawn: &mut GameObject) {
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(spawn.x, spawn.y);
    spawn.grid_x = grid_x;
    spawn.grid_y = grid_y;
    spawn.cell = lyracore_shared::spatial::cell_id_at(spawn.x, spawn.y);
}

fn insert_failed(row: &TracedRow, e: &dyn std::fmt::Display) -> String {
    format!("`{}` row {} did not insert: {e}", row.table(), row.key())
}

fn gone(row: &TracedRow) -> String {
    format!("`{}` row {} vanished mid-apply", row.table(), row.key())
}

/// Runs one table's setter over every claimed column.
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

fn blank_template(entry: u32) -> GameObjectTemplate {
    GameObjectTemplate {
        entry,
        type_id: 0,
        display_id: 0,
        name: String::new(),
        data0: 0,
        data1: 0,
        gather_skill_line: 0,
        respawn_secs: 0,
        gather_gray: 0,
        lock_id: 0,
        size: 0.0,
    }
}

fn apply_template_field(
    template: &mut GameObjectTemplate,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "type_id" => template.type_id = as_u8(field, value)?,
        "display_id" => template.display_id = as_u32(field, value)?,
        "name" => template.name = as_str(field, value)?,
        "data0" => template.data0 = as_u32(field, value)?,
        "data1" => template.data1 = as_u32(field, value)?,
        "gather_skill_line" => template.gather_skill_line = as_u32(field, value)?,
        "respawn_secs" => template.respawn_secs = as_u32(field, value)?,
        "gather_gray" => template.gather_gray = as_u32(field, value)?,
        "lock_id" => template.lock_id = as_u32(field, value)?,
        "size" => template.size = as_f32(field, value)?,
        other => return Err(no_such_column("game_gameobject_template", other)),
    }
    Ok(())
}

fn blank_trap(entry: u32) -> GameObjectTrap {
    GameObjectTrap {
        entry,
        spell_id: 0,
        cooldown_secs: 0,
    }
}

fn apply_trap_field(
    trap: &mut GameObjectTrap,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "spell_id" => trap.spell_id = as_u32(field, value)?,
        "cooldown_secs" => trap.cooldown_secs = as_u32(field, value)?,
        other => return Err(no_such_column("game_gameobject_trap", other)),
    }
    Ok(())
}

/// A spawn row with everything the key already decided, and the live state a fresh import stamps: a
/// ready prop, no pending respawn, open world. The grid columns are re-derived by [`stamp_cell`]
/// once the claimed coordinates are on the row.
fn blank_spawn(ctx: &ReducerContext, key: PrimaryKey) -> GameObject {
    let map_id = match key {
        PrimaryKey::GameobjectSpawn { map_id, .. } => map_id,
        other => {
            unreachable!("a gameobject spawn row carries a gameobject spawn key, found {other}")
        }
    };
    GameObject {
        guid: key.row_id(),
        template_entry: 0,
        map_id,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        orientation: 0.0,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
}

fn apply_spawn_field(
    spawn: &mut GameObject,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "template_entry" => spawn.template_entry = as_u32(field, value)?,
        "x" => spawn.x = as_f32(field, value)?,
        "y" => spawn.y = as_f32(field, value)?,
        "z" => spawn.z = as_f32(field, value)?,
        "orientation" => spawn.orientation = as_f32(field, value)?,
        "state" => spawn.state = as_u8(field, value)?,
        "rotation_0" => spawn.rotation_0 = as_f32(field, value)?,
        "rotation_1" => spawn.rotation_1 = as_f32(field, value)?,
        "rotation_2" => spawn.rotation_2 = as_f32(field, value)?,
        "rotation_3" => spawn.rotation_3 = as_f32(field, value)?,
        other => return Err(no_such_column("game_gameobject", other)),
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
        artifact, gameobject_spawn_claim, gameobject_template_claim, plan, some_value,
        PACKAGE_GAMEOBJECT, REAL_GAMEOBJECT_SPAWN, REAL_MAP, WHOLE_GAMEOBJECT_TEMPLATE_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    const A_NAME: &str = r#"{"name":{"type":"string","value":"Kindled Cache"}}"#;
    const A_POSITION: &str = r#"{"x":{"type":"f32","value":1.5}}"#;

    #[test]
    fn tuning_a_package_template_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &gameobject_template_claim(PACKAGE_GAMEOBJECT, "update", A_NAME),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn one_spawn_claimed_on_two_maps_is_refused() {
        let plan = plan(&[
            artifact(
                "example.here",
                &gameobject_spawn_claim(0, REAL_GAMEOBJECT_SPAWN, "update", A_POSITION),
            ),
            artifact(
                "example.there",
                &gameobject_spawn_claim(
                    1,
                    REAL_GAMEOBJECT_SPAWN,
                    "update",
                    r#"{"y":{"type":"f32","value":2.5}}"#,
                ),
            ),
        ])
        .expect("plan builds");

        let refusal = check_one_guid_per_spawn(&plan.rows).expect_err("the plan is refused");

        assert!(refusal.contains("one durable gameobject"), "{refusal}");
    }

    #[test]
    fn an_inserted_template_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &gameobject_template_claim(PACKAGE_GAMEOBJECT, "insert", WHOLE_GAMEOBJECT_TEMPLATE_ROW),
        )])
        .expect("plan builds");

        let mut template = blank_template(PACKAGE_GAMEOBJECT);
        apply_fields(&plan.rows[0], |field, value| {
            apply_template_field(&mut template, field, value)
        })
        .expect("row builds");

        assert_eq!(template.name, "Kindled Cache");
        assert_eq!(template.type_id, 3);
        assert_eq!(template.data0, 25);
    }

    /// The grid columns address the row for every viewer's AOI box, and they are derived rather
    /// than claimed, so a written position has to re-derive them.
    #[test]
    fn a_written_position_restamps_the_grid_cell() {
        let mut spawn = blank_spawn_for_test();
        spawn.x = 2_133.0;
        spawn.y = -4_477.0;

        stamp_cell(&mut spawn);

        let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(spawn.x, spawn.y);
        assert_eq!((spawn.grid_x, spawn.grid_y), (grid_x, grid_y));
        assert_eq!(
            spawn.cell,
            lyracore_shared::spatial::cell_id_at(2_133.0, -4_477.0)
        );
        assert_ne!(spawn.cell, 0, "a real position is not cell (0, 0)");
    }

    /// The Claim schema and the setters above are one contract.
    #[test]
    fn every_claimable_gameobject_column_has_a_setter() {
        let mut template = blank_template(PACKAGE_GAMEOBJECT);
        for column in ClaimTable::GameobjectTemplate.columns() {
            apply_template_field(&mut template, column.name, &some_value(*column)).expect("setter");
        }

        let mut trap = blank_trap(PACKAGE_GAMEOBJECT);
        for column in ClaimTable::GameobjectTrap.columns() {
            apply_trap_field(&mut trap, column.name, &some_value(*column)).expect("setter");
        }

        let mut spawn = blank_spawn_for_test();
        for column in ClaimTable::GameobjectSpawn.columns() {
            apply_spawn_field(&mut spawn, column.name, &some_value(*column)).expect("setter");
        }
    }

    #[test]
    fn a_package_spawn_guid_is_recognised_by_its_spawn_identifier() {
        let package = lyracore_package_delta::packed_gameobject_spawn_guid(PACKAGE_GAMEOBJECT);
        let imported = lyracore_package_delta::packed_gameobject_spawn_guid(REAL_GAMEOBJECT_SPAWN);

        assert!(is_package_spawn_guid(package));
        assert!(!is_package_spawn_guid(imported));
    }

    /// `blank_spawn` needs a live `ReducerContext` for its timestamp, which a native test cannot
    /// build, so the pure cases construct the row directly.
    fn blank_spawn_for_test() -> GameObject {
        GameObject {
            guid: lyracore_package_delta::packed_gameobject_spawn_guid(PACKAGE_GAMEOBJECT),
            template_entry: 0,
            map_id: REAL_MAP,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            state: 0,
            created_at: spacetimedb::Timestamp::UNIX_EPOCH,
            respawn_at_micros: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            rotation_0: 0.0,
            rotation_1: 0.0,
            rotation_2: 0.0,
            rotation_3: 0.0,
        }
    }
}
