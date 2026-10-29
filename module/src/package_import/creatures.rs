use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_creature_id, FieldValue, Operation, PrimaryKey, Table as ClaimTable, TracedRow,
    MAX_CREATURE_GUID_COMPONENT,
};

use crate::creatures::timer_never;
use crate::{game_creature_spawn, game_creature_template, CreatureSpawn, CreatureTemplate};

use super::{as_f32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    check_one_guid_per_spawn(rows)?;
    for row in rows {
        match row.key() {
            PrimaryKey::CreatureSpawn { entry, .. } => {
                if !template_exists_after_apply(ctx, rows, entry) {
                    return Err(format!(
                        "`{}` row {} references missing entry {entry}",
                        row.table(),
                        row.key()
                    ));
                }
            }
            PrimaryKey::CreatureTemplate { .. } => {}
            other => unreachable!("creatures reference check received {other}"),
        }
    }
    Ok(())
}

fn check_one_guid_per_spawn(rows: &[TracedRow]) -> Result<(), String> {
    let claimed: Vec<&TracedRow> = rows
        .iter()
        .filter(|row| row.table() == ClaimTable::CreatureSpawn)
        .collect();
    for (index, row) in claimed.iter().enumerate() {
        if let Some(twin) = claimed[index + 1..]
            .iter()
            .find(|other| other.row_id() == row.row_id())
        {
            return Err(format!(
                "`{}` rows {} and {} are one durable creature; a spawn belongs to one map",
                row.table(),
                row.key(),
                twin.key()
            ));
        }
    }
    Ok(())
}

fn template_exists_after_apply(ctx: &ReducerContext, rows: &[TracedRow], entry: u32) -> bool {
    if entry == 0 {
        return false;
    }
    if is_package_creature_id(entry) {
        return rows.iter().any(|row| {
            row.table() == ClaimTable::CreatureTemplate
                && row.operation() == Operation::Insert
                && row.row_id() == u64::from(entry)
        });
    }
    ctx.db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_some()
}

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
        ClaimTable::CreatureTemplate => ctx
            .db
            .game_creature_template()
            .entry()
            .find(row.row_id() as u32)
            .is_some(),
        ClaimTable::CreatureSpawn => ctx
            .db
            .game_creature_spawn()
            .guid()
            .find(row.row_id())
            .is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-creatures row before the creatures family's \
             dispatch runs, found {other}"
        ),
    }
}

fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    match row.key() {
        PrimaryKey::CreatureTemplate { entry } => is_package_creature_id(entry),
        PrimaryKey::CreatureSpawn { spawn_id, .. } => is_package_creature_id(spawn_id),
        _ => false,
    }
}

pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let spawns = ctx.db.game_creature_spawn();
    let package_guids: Vec<u64> = spawns
        .iter()
        .map(|spawn| spawn.guid)
        .filter(|guid| is_package_spawn_guid(*guid))
        .collect();
    for guid in package_guids {
        crate::instance::remove_spawn_copies(ctx, guid);
        crate::creatures::despawn_creature_entity(ctx, guid);
        spawns.guid().delete(guid);
    }

    let templates = ctx.db.game_creature_template();
    let package_entries: Vec<u32> = templates
        .iter()
        .map(|template| template.entry)
        .filter(|entry| is_package_creature_id(*entry))
        .collect();
    for entry in package_entries {
        templates.entry().delete(entry);
    }
}

fn is_package_spawn_guid(guid: u64) -> bool {
    is_package_creature_id((guid & u64::from(MAX_CREATURE_GUID_COMPONENT)) as u32)
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::CreatureTemplate, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut template = blank_template(row.row_id() as u32);
            apply_fields(row, |field, value| {
                apply_template_field(&mut template, field, value)
            })?;
            ctx.db
                .game_creature_template()
                .try_insert(template)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::CreatureTemplate, Operation::Update) => {
            let templates = ctx.db.game_creature_template();
            let mut template = templates
                .entry()
                .find(row.row_id() as u32)
                .ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_template_field(&mut template, field, value)
            })?;
            templates.entry().update(template);
        }
        (ClaimTable::CreatureSpawn, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut spawn = blank_spawn(ctx, row.key());
            apply_fields(row, |field, value| {
                apply_spawn_field(&mut spawn, field, value)
            })?;
            ctx.db
                .game_creature_spawn()
                .try_insert(spawn)
                .map_err(|e| insert_failed(row, &e))?;
        }
        (ClaimTable::CreatureSpawn, Operation::Update) => {
            let spawns = ctx.db.game_creature_spawn();
            let mut spawn = spawns.guid().find(row.row_id()).ok_or_else(|| gone(row))?;
            apply_fields(row, |field, value| {
                apply_spawn_field(&mut spawn, field, value)
            })?;
            spawn.respawn_at = crate::time::now(ctx);
            spawns.guid().update(spawn);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-creatures row before the creatures family's \
             dispatch runs, found {other}"
        ),
    }
    if row.table() == ClaimTable::CreatureSpawn {
        crate::instance::remove_spawn_copies(ctx, row.row_id());
        crate::creatures::despawn_creature_entity(ctx, row.row_id());
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

fn blank_template(entry: u32) -> CreatureTemplate {
    CreatureTemplate {
        entry,
        name: String::new(),
        subname: String::new(),
        display_id: 0,
        level: 0,
        health: 0,
        faction_template: 0,
        npc_flags: 0,
        unit_flags: 0,
        creature_type: 0,
        creature_family: 0,
        type_flags: 0,
        rank: 0,
        scale: 0.0,
        base_attack_time_ms: 0,
        money_min: 0,
        money_max: 0,
        max_level: 0,
        max_level_health: 0,
        aggro_range: 0,
        damage_min: 0,
        damage_max: 0,
        armor: 0,
        pickpocket_loot_id: 0,
        skin_loot_id: 0,
        trainer_type: 0,
        trainer_class: 0,
    }
}

fn apply_template_field(
    template: &mut CreatureTemplate,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "name" => template.name = as_str(field, value)?,
        "subname" => template.subname = as_str(field, value)?,
        "display_id" => template.display_id = as_u32(field, value)?,
        "level" => template.level = as_u32(field, value)?,
        "health" => template.health = as_u32(field, value)?,
        "faction_template" => template.faction_template = as_u32(field, value)?,
        "npc_flags" => template.npc_flags = as_u32(field, value)?,
        "unit_flags" => template.unit_flags = as_u32(field, value)?,
        "creature_type" => template.creature_type = as_u8(field, value)?,
        "creature_family" => template.creature_family = as_u8(field, value)?,
        "type_flags" => template.type_flags = as_u32(field, value)?,
        "rank" => template.rank = as_u8(field, value)?,
        "scale" => template.scale = as_f32(field, value)?,
        "base_attack_time_ms" => template.base_attack_time_ms = as_u32(field, value)?,
        "money_min" => template.money_min = as_u32(field, value)?,
        "money_max" => template.money_max = as_u32(field, value)?,
        "max_level" => template.max_level = as_u32(field, value)?,
        "max_level_health" => template.max_level_health = as_u32(field, value)?,
        "aggro_range" => template.aggro_range = as_u32(field, value)?,
        "damage_min" => template.damage_min = as_u32(field, value)?,
        "damage_max" => template.damage_max = as_u32(field, value)?,
        "armor" => template.armor = as_u32(field, value)?,
        "pickpocket_loot_id" => template.pickpocket_loot_id = as_u32(field, value)?,
        "skin_loot_id" => template.skin_loot_id = as_u32(field, value)?,
        "trainer_type" => template.trainer_type = as_u8(field, value)?,
        "trainer_class" => template.trainer_class = as_u8(field, value)?,
        other => return Err(no_such_column("game_creature_template", other)),
    }
    Ok(())
}

fn blank_spawn(ctx: &ReducerContext, key: PrimaryKey) -> CreatureSpawn {
    let (map_id, entry) = match key {
        PrimaryKey::CreatureSpawn { map_id, entry, .. } => (map_id, entry),
        other => unreachable!("a creature spawn row carries a creature spawn key, found {other}"),
    };
    CreatureSpawn {
        guid: key.row_id(),
        entry,
        map_id,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        orientation: 0.0,
        respawn_at: crate::time::now(ctx),
        despawn_at: timer_never(ctx),
        movement_type: 0,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: 0,
    }
}

fn apply_spawn_field(
    spawn: &mut CreatureSpawn,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "x" => spawn.x = as_f32(field, value)?,
        "y" => spawn.y = as_f32(field, value)?,
        "z" => spawn.z = as_f32(field, value)?,
        "orientation" => spawn.orientation = as_f32(field, value)?,
        "movement_type" => spawn.movement_type = as_u8(field, value)?,
        "respawn_secs" => spawn.respawn_secs = as_u32(field, value)?,
        other => return Err(no_such_column("game_creature_spawn", other)),
    }
    Ok(())
}

fn no_such_column(table: &str, column: &str) -> String {
    format!("`{table}` has no claimable column `{column}`")
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, creature_spawn_claim,
        creature_template_claim, plan, PACKAGE_CREATURE, REAL_CREATURE, REAL_CREATURE_SPAWN,
        REAL_MAP, WHOLE_CREATURE_TEMPLATE_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    const A_LEVEL: &str = r#"{"level":{"type":"u32","value":12}}"#;
    const A_POSITION: &str = r#"{"x":{"type":"f32","value":1.5}}"#;

    #[test]
    fn tuning_a_package_template_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &creature_template_claim(PACKAGE_CREATURE, "update", A_LEVEL),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn a_package_spawn_of_a_real_creature_is_judged_on_its_spawn_identifier() {
        let package_spawn = plan(&[artifact(
            "example.placer",
            &creature_spawn_claim(
                REAL_MAP,
                REAL_CREATURE,
                PACKAGE_CREATURE,
                "update",
                A_POSITION,
            ),
        )])
        .expect("plan builds");
        let real_spawn = plan(&[artifact(
            "example.placer",
            &creature_spawn_claim(
                REAL_MAP,
                REAL_CREATURE,
                REAL_CREATURE_SPAWN,
                "update",
                A_POSITION,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&package_spawn.rows[0]));
        assert!(!updates_an_uninvented_package_row(&real_spawn.rows[0]));
    }

    #[test]
    fn one_spawn_claimed_on_two_maps_is_refused() {
        let plan = plan(&[
            artifact(
                "example.here",
                &creature_spawn_claim(0, REAL_CREATURE, REAL_CREATURE_SPAWN, "update", A_POSITION),
            ),
            artifact(
                "example.there",
                &creature_spawn_claim(
                    1,
                    REAL_CREATURE,
                    REAL_CREATURE_SPAWN,
                    "update",
                    r#"{"y":{"type":"f32","value":2.5}}"#,
                ),
            ),
        ])
        .expect("plan builds");

        let refusal = check_one_guid_per_spawn(&plan.rows).expect_err("the plan is refused");

        assert!(refusal.contains("one durable creature"), "{refusal}");
    }

    #[test]
    fn two_spawns_of_one_template_on_one_map_are_two_rows() {
        let plan = plan(&[artifact(
            "example.pair",
            &format!(
                "{},{}",
                creature_spawn_claim(REAL_MAP, REAL_CREATURE, 15_000_001, "update", A_POSITION),
                creature_spawn_claim(REAL_MAP, REAL_CREATURE, 15_000_002, "update", A_POSITION),
            ),
        )])
        .expect("plan builds");

        check_one_guid_per_spawn(&plan.rows).expect("two spawn points are two rows");
    }

    #[test]
    fn an_inserted_template_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &creature_template_claim(PACKAGE_CREATURE, "insert", WHOLE_CREATURE_TEMPLATE_ROW),
        )])
        .expect("plan builds");

        let mut template = blank_template(PACKAGE_CREATURE);
        apply_fields(&plan.rows[0], |field, value| {
            apply_template_field(&mut template, field, value)
        })
        .expect("row builds");

        assert_eq!(template.name, "Kindled Sentinel");
        assert_eq!(template.level, 12);
        assert_eq!(template.armor, 120);
    }

    #[test]
    fn every_claimable_creature_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::CreatureTemplate,
            || blank_template(PACKAGE_CREATURE),
            apply_template_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::CreatureSpawn,
            || CreatureSpawn {
                guid: 1,
                entry: REAL_CREATURE,
                map_id: REAL_MAP,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                orientation: 0.0,
                respawn_at: spacetimedb::Timestamp::UNIX_EPOCH,
                despawn_at: spacetimedb::Timestamp::UNIX_EPOCH,
                movement_type: 0,
                respawn_secs: 0,
                life_seq: 0,
                source_guid: 0,
                instance_id: 0,
            },
            apply_spawn_field,
        );
    }

    #[test]
    fn a_package_spawn_guid_is_recognised_by_its_spawn_identifier() {
        let package =
            lyracore_package_delta::packed_creature_spawn_guid(REAL_CREATURE, PACKAGE_CREATURE);
        let imported =
            lyracore_package_delta::packed_creature_spawn_guid(REAL_CREATURE, REAL_CREATURE_SPAWN);

        assert!(is_package_spawn_guid(package));
        assert!(!is_package_spawn_guid(imported));
    }
}
