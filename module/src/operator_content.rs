// The Operator Content API; read docs/operator-content-api.md before changing a contract.
use lyracore_package_delta::{
    is_operator_spawn_id, is_package_creature_id, is_package_gameobject_id,
    packed_creature_spawn_guid, packed_gameobject_spawn_guid, MAX_CREATURE_GUID_COMPONENT,
    OPERATOR_SPAWN_ID_CEIL, OPERATOR_SPAWN_ID_FLOOR,
};
use lyracore_shared::terrain::{cell_index, cell_key};
use spacetimedb::{log, reducer, ReducerContext, SpacetimeType, Table};

use crate::creatures::population::game_creature_population_member;
use crate::creatures::{
    game_creature_spawn, game_creature_spawn_map, game_creature_template, game_creature_waypoint,
    CreatureSpawn, CreatureSpawnMap, CreatureWaypoint, MOVEMENT_IDLE, MOVEMENT_RANDOM,
};
use crate::game_world_entity;
use crate::gameobject::{game_gameobject, game_gameobject_template, GameObject, POOL_TAG};
use crate::nav::game_nav_chunk;
use crate::terrain::game_terrain_chunk;

const HIGHGUID_UNIT: u64 = 0xF130;
const HIGHGUID_GAMEOBJECT: u64 = 0xF110;
const CREATURE_SPAWN_ID_MASK: u64 = MAX_CREATURE_GUID_COMPONENT as u64;
const GAMEOBJECT_SPAWN_ID_MASK: u64 = 0xFFFF_FFFF_FFFF;
// The importer keeps the source's waypoint movement type; only MOVEMENT_RANDOM wanders.
const MOVEMENT_WAYPOINT: u8 = 2;

const MAX_SPAWN_EDITS: usize = 256;
const MAX_WAYPOINTS_PER_SPAWN: usize = 256;
const MAX_BATCH_WAYPOINTS: usize = 4096;

#[derive(SpacetimeType, Clone, Copy)]
pub struct CreatureSpawnPlacement {
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub movement_type: u8,
    pub respawn_secs: u32,
}

#[derive(SpacetimeType, Clone, Copy)]
pub struct CreatureSpawnCreate {
    pub entry: u32,
    pub placement: CreatureSpawnPlacement,
}

#[derive(SpacetimeType, Clone, Copy)]
pub struct CreatureSpawnUpdate {
    pub guid: u64,
    pub placement: CreatureSpawnPlacement,
}

#[derive(SpacetimeType, Clone, Copy)]
pub struct WaypointPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureWaypoints {
    pub guid: u64,
    pub points: Vec<WaypointPoint>,
}

#[derive(SpacetimeType, Clone, Copy)]
pub struct GameObjectSpawnFields {
    pub template_entry: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub state: u8,
    pub rotation_0: f32,
    pub rotation_1: f32,
    pub rotation_2: f32,
    pub rotation_3: f32,
}

#[derive(SpacetimeType, Clone, Copy)]
pub struct GameObjectSpawnUpdate {
    pub guid: u64,
    pub fields: GameObjectSpawnFields,
}

#[derive(SpacetimeType, Clone)]
pub enum SpawnEdit {
    CreateCreature(CreatureSpawnCreate),
    UpdateCreature(CreatureSpawnUpdate),
    DeleteCreature(u64),
    SetCreatureWaypoints(CreatureWaypoints),
    CreateGameObject(GameObjectSpawnFields),
    UpdateGameObject(GameObjectSpawnUpdate),
    DeleteGameObject(u64),
}

#[reducer]
pub fn operator_create_creature_spawn(
    ctx: &ReducerContext,
    entry: u32,
    placement: CreatureSpawnPlacement,
) -> Result<(), String> {
    apply_one(
        ctx,
        SpawnEdit::CreateCreature(CreatureSpawnCreate { entry, placement }),
    )
}

#[reducer]
pub fn operator_update_creature_spawn(
    ctx: &ReducerContext,
    guid: u64,
    placement: CreatureSpawnPlacement,
) -> Result<(), String> {
    apply_one(
        ctx,
        SpawnEdit::UpdateCreature(CreatureSpawnUpdate { guid, placement }),
    )
}

#[reducer]
pub fn operator_delete_creature_spawn(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    apply_one(ctx, SpawnEdit::DeleteCreature(guid))
}

#[reducer]
pub fn operator_set_creature_waypoints(
    ctx: &ReducerContext,
    guid: u64,
    points: Vec<WaypointPoint>,
) -> Result<(), String> {
    apply_one(
        ctx,
        SpawnEdit::SetCreatureWaypoints(CreatureWaypoints { guid, points }),
    )
}

#[reducer]
pub fn operator_create_gameobject(
    ctx: &ReducerContext,
    fields: GameObjectSpawnFields,
) -> Result<(), String> {
    apply_one(ctx, SpawnEdit::CreateGameObject(fields))
}

#[reducer]
pub fn operator_update_gameobject(
    ctx: &ReducerContext,
    guid: u64,
    fields: GameObjectSpawnFields,
) -> Result<(), String> {
    apply_one(
        ctx,
        SpawnEdit::UpdateGameObject(GameObjectSpawnUpdate { guid, fields }),
    )
}

#[reducer]
pub fn operator_delete_gameobject(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    apply_one(ctx, SpawnEdit::DeleteGameObject(guid))
}

/// Applies every edit in order in one transaction; any Refusal rolls back the whole batch.
#[reducer]
pub fn operator_apply_spawn_edits(
    ctx: &ReducerContext,
    edits: Vec<SpawnEdit>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    check_batch_size(&edits)?;
    let mut ids = SpawnIds::default();
    let mut changes = Vec::with_capacity(edits.len());
    for (index, edit) in edits.into_iter().enumerate() {
        changes.push(
            apply(ctx, &mut ids, edit).map_err(|reason| format!("spawn edit {index}: {reason}"))?,
        );
    }
    for change in changes {
        log_change(ctx, &change);
    }
    Ok(())
}

fn apply_one(ctx: &ReducerContext, edit: SpawnEdit) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let change = apply(ctx, &mut SpawnIds::default(), edit)?;
    log_change(ctx, &change);
    Ok(())
}

// Log only after every edit applied: a refused batch rolls back, but a log line stays.
fn log_change(ctx: &ReducerContext, change: &str) {
    log::info!("operator {} {change}", ctx.sender());
}

fn check_batch_size(edits: &[SpawnEdit]) -> Result<(), String> {
    if edits.len() > MAX_SPAWN_EDITS {
        return Err(format!(
            "a batch holds at most {MAX_SPAWN_EDITS} spawn edits, got {}",
            edits.len()
        ));
    }
    let points: usize = edits
        .iter()
        .map(|edit| match edit {
            SpawnEdit::SetCreatureWaypoints(route) => route.points.len(),
            _ => 0,
        })
        .sum();
    if points > MAX_BATCH_WAYPOINTS {
        return Err(format!(
            "a batch holds at most {MAX_BATCH_WAYPOINTS} waypoints, got {points}"
        ));
    }
    Ok(())
}

fn apply(ctx: &ReducerContext, ids: &mut SpawnIds, edit: SpawnEdit) -> Result<String, String> {
    match edit {
        SpawnEdit::CreateCreature(create) => create_creature(ctx, ids, create),
        SpawnEdit::UpdateCreature(update) => update_creature(ctx, update),
        SpawnEdit::DeleteCreature(guid) => delete_creature(ctx, guid),
        SpawnEdit::SetCreatureWaypoints(route) => set_waypoints(ctx, ids, route),
        SpawnEdit::CreateGameObject(fields) => create_gameobject(ctx, ids, fields),
        SpawnEdit::UpdateGameObject(update) => update_gameobject(ctx, update),
        SpawnEdit::DeleteGameObject(guid) => delete_gameobject(ctx, guid),
    }
}

// Each cursor holds the last identifier this transaction used, or None before the first scan.
#[derive(Default)]
struct SpawnIds {
    creature: Option<u32>,
    gameobject: Option<u32>,
    waypoint: Option<u64>,
}

impl SpawnIds {
    fn next_creature_spawn_id(&mut self, ctx: &ReducerContext) -> Result<u32, String> {
        let last = *self.creature.get_or_insert_with(|| {
            ctx.db
                .game_creature_spawn()
                .iter()
                .filter(|spawn| spawn.guid >> 48 == HIGHGUID_UNIT)
                .map(|spawn| (spawn.guid & CREATURE_SPAWN_ID_MASK) as u32)
                .filter(|&id| is_operator_spawn_id(id))
                .max()
                .unwrap_or(OPERATOR_SPAWN_ID_FLOOR - 1)
        });
        let next = next_operator_spawn_id(last)?;
        self.creature = Some(next);
        Ok(next)
    }

    fn next_gameobject_spawn_id(&mut self, ctx: &ReducerContext) -> Result<u32, String> {
        let last = *self.gameobject.get_or_insert_with(|| {
            ctx.db
                .game_gameobject()
                .iter()
                .filter(|go| go.guid >> 48 == HIGHGUID_GAMEOBJECT)
                .filter_map(|go| u32::try_from(go.guid & GAMEOBJECT_SPAWN_ID_MASK).ok())
                .filter(|&id| is_operator_spawn_id(id))
                .max()
                .unwrap_or(OPERATOR_SPAWN_ID_FLOOR - 1)
        });
        let next = next_operator_spawn_id(last)?;
        self.gameobject = Some(next);
        Ok(next)
    }

    fn next_waypoint_id(&mut self, ctx: &ReducerContext) -> Result<u64, String> {
        let last = *self.waypoint.get_or_insert_with(|| {
            ctx.db
                .game_creature_waypoint()
                .iter()
                .map(|waypoint| waypoint.id)
                .max()
                .unwrap_or(0)
        });
        let next = last.checked_add(1).ok_or("waypoint id exhausted")?;
        self.waypoint = Some(next);
        Ok(next)
    }
}

fn next_operator_spawn_id(last: u32) -> Result<u32, String> {
    let next = last.saturating_add(1);
    if next > OPERATOR_SPAWN_ID_CEIL {
        return Err(format!(
            "the Operator Spawn Range ({OPERATOR_SPAWN_ID_FLOOR} to {OPERATOR_SPAWN_ID_CEIL}) is exhausted"
        ));
    }
    Ok(next)
}

fn create_creature(
    ctx: &ReducerContext,
    ids: &mut SpawnIds,
    create: CreatureSpawnCreate,
) -> Result<String, String> {
    let CreatureSpawnCreate { entry, placement } = create;
    if entry > MAX_CREATURE_GUID_COMPONENT {
        return Err(format!(
            "creature entry {entry} does not fit a creature guid"
        ));
    }
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_none()
    {
        return Err(format!("creature template {entry} does not exist"));
    }
    check_creature_placement(ctx, &placement)?;
    let guid = packed_creature_spawn_guid(entry, ids.next_creature_spawn_id(ctx)?);
    let mut spawn = CreatureSpawn {
        guid,
        entry,
        map_id: placement.map_id,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        orientation: 0.0,
        respawn_at: ctx.timestamp,
        despawn_at: ctx.timestamp,
        movement_type: MOVEMENT_IDLE,
        respawn_secs: 0,
        life_seq: 0,
        instance_id: 0,
        source_guid: 0,
    };
    place_creature(ctx, &mut spawn, &placement);
    let created = describe_creature(&spawn);
    ctx.db
        .game_creature_spawn()
        .try_insert(spawn)
        .map_err(|e| format!("creature spawn {guid} cannot be inserted: {e}"))?;
    Ok(format!("created creature spawn {guid}: {created}"))
}

fn update_creature(ctx: &ReducerContext, update: CreatureSpawnUpdate) -> Result<String, String> {
    let CreatureSpawnUpdate { guid, placement } = update;
    let old = authored_creature_spawn(ctx, guid)?;
    check_creature_placement(ctx, &placement)?;
    retire_live_creature(ctx, guid);
    let mut spawn = old.clone();
    place_creature(ctx, &mut spawn, &placement);
    let updated = describe_creature(&spawn);
    ctx.db.game_creature_spawn().guid().update(spawn);
    Ok(format!(
        "updated creature spawn {guid}: {} -> {updated}",
        describe_creature(&old)
    ))
}

fn delete_creature(ctx: &ReducerContext, guid: u64) -> Result<String, String> {
    let old = authored_creature_spawn(ctx, guid)?;
    if let Some(member) = ctx
        .db
        .game_creature_population_member()
        .source_guid()
        .find(guid)
    {
        return Err(format!(
            "creature spawn {guid} is a member of population {}; the import owns the group",
            member.population_id
        ));
    }
    let route = ctx
        .db
        .game_creature_waypoint()
        .by_creature()
        .filter(guid)
        .count();
    retire_live_creature(ctx, guid);
    crate::instance::remove_spawn_copies(ctx, guid);
    crate::instance::remove_spawn_waypoints(ctx, guid);
    ctx.db.game_creature_spawn().guid().delete(guid);
    Ok(format!(
        "deleted creature spawn {guid}: {} with {route} waypoint(s)",
        describe_creature(&old)
    ))
}

fn set_waypoints(
    ctx: &ReducerContext,
    ids: &mut SpawnIds,
    route: CreatureWaypoints,
) -> Result<String, String> {
    let CreatureWaypoints { guid, points } = route;
    let spawn = authored_creature_spawn(ctx, guid)?;
    if points.len() > MAX_WAYPOINTS_PER_SPAWN {
        return Err(format!(
            "a patrol holds at most {MAX_WAYPOINTS_PER_SPAWN} waypoints, got {}",
            points.len()
        ));
    }
    for point in &points {
        check_position(point.x, point.y, point.z, 0.0)?;
    }
    let waypoints = ctx.db.game_creature_waypoint();
    let mut old: Vec<CreatureWaypoint> = waypoints.by_creature().filter(guid).collect();
    old.sort_by_key(|waypoint| waypoint.id);
    retire_live_creature(ctx, guid);
    crate::instance::remove_spawn_waypoints(ctx, guid);
    for point in &points {
        waypoints.insert(CreatureWaypoint {
            id: ids.next_waypoint_id(ctx)?,
            creature_guid: guid,
            x: point.x,
            y: point.y,
            z: point.z,
        });
    }
    let mut respawned = spawn;
    rearm_respawn(ctx, &mut respawned);
    ctx.db.game_creature_spawn().guid().update(respawned);
    Ok(format!(
        "set the patrol of creature spawn {guid}: [{}] -> [{}]",
        describe_route(old.iter().map(|w| (w.x, w.y, w.z))),
        describe_route(points.iter().map(|p| (p.x, p.y, p.z)))
    ))
}

fn create_gameobject(
    ctx: &ReducerContext,
    ids: &mut SpawnIds,
    fields: GameObjectSpawnFields,
) -> Result<String, String> {
    check_gameobject_fields(ctx, &fields)?;
    let guid = packed_gameobject_spawn_guid(ids.next_gameobject_spawn_id(ctx)?);
    let go = insert_gameobject(ctx, guid, &fields)?;
    Ok(format!(
        "created gameobject spawn {guid}: {}",
        describe_gameobject(&go)
    ))
}

fn update_gameobject(
    ctx: &ReducerContext,
    update: GameObjectSpawnUpdate,
) -> Result<String, String> {
    let GameObjectSpawnUpdate { guid, fields } = update;
    let old = authored_gameobject(ctx, guid)?;
    check_gameobject_fields(ctx, &fields)?;
    crate::gameobject::despawn_from_relay(ctx, guid)?;
    let go = insert_gameobject(ctx, guid, &fields)?;
    Ok(format!(
        "updated gameobject spawn {guid}: {} -> {}",
        describe_gameobject(&old),
        describe_gameobject(&go)
    ))
}

fn delete_gameobject(ctx: &ReducerContext, guid: u64) -> Result<String, String> {
    let old = authored_gameobject(ctx, guid)?;
    crate::gameobject::despawn_from_relay(ctx, guid)?;
    Ok(format!(
        "deleted gameobject spawn {guid}: {}",
        describe_gameobject(&old)
    ))
}

/// Only an instance-zero spawn row outside the Package band that is not itself a copy.
fn authored_creature_spawn(ctx: &ReducerContext, guid: u64) -> Result<CreatureSpawn, String> {
    if guid >> 48 != HIGHGUID_UNIT {
        return Err(format!("{guid} is not a creature spawn guid"));
    }
    if is_package_creature_id((guid & CREATURE_SPAWN_ID_MASK) as u32) {
        return Err(format!(
            "creature spawn {guid} is in the Package Creature Range"
        ));
    }
    let spawn = ctx
        .db
        .game_creature_spawn()
        .guid()
        .find(guid)
        .ok_or_else(|| format!("creature spawn {guid} does not exist"))?;
    if spawn.instance_id != 0 || spawn.source_guid != 0 {
        return Err(format!("{guid} is not an authored creature spawn"));
    }
    Ok(spawn)
}

/// Only an instance-zero row outside the pool and instance-copy bands and the Package band.
fn authored_gameobject(ctx: &ReducerContext, guid: u64) -> Result<GameObject, String> {
    if guid >> 48 != HIGHGUID_GAMEOBJECT || guid & (POOL_TAG | crate::instance::GO_COPY_BAND) != 0 {
        return Err(format!("{guid} is not a gameobject spawn guid"));
    }
    if u32::try_from(guid & GAMEOBJECT_SPAWN_ID_MASK).is_ok_and(is_package_gameobject_id) {
        return Err(format!(
            "gameobject spawn {guid} is in the Package Gameobject Range"
        ));
    }
    let go = ctx
        .db
        .game_gameobject()
        .guid()
        .find(guid)
        .ok_or_else(|| format!("gameobject spawn {guid} does not exist"))?;
    if go.instance_id != 0 {
        return Err(format!("{guid} is not an authored gameobject spawn"));
    }
    Ok(go)
}

fn check_creature_placement(
    ctx: &ReducerContext,
    placement: &CreatureSpawnPlacement,
) -> Result<(), String> {
    check_position(placement.x, placement.y, placement.z, placement.orientation)?;
    check_map(ctx, placement.map_id, placement.x, placement.y)?;
    if ![MOVEMENT_IDLE, MOVEMENT_RANDOM, MOVEMENT_WAYPOINT].contains(&placement.movement_type) {
        return Err(format!(
            "movement type {} is not idle ({MOVEMENT_IDLE}), random ({MOVEMENT_RANDOM}) or \
             waypoint ({MOVEMENT_WAYPOINT})",
            placement.movement_type
        ));
    }
    Ok(())
}

fn check_gameobject_fields(
    ctx: &ReducerContext,
    fields: &GameObjectSpawnFields,
) -> Result<(), String> {
    check_position(fields.x, fields.y, fields.z, fields.orientation)?;
    let rotation = [
        fields.rotation_0,
        fields.rotation_1,
        fields.rotation_2,
        fields.rotation_3,
    ];
    if !rotation.iter().all(|value| value.is_finite()) {
        return Err("the rotation must be finite".to_string());
    }
    check_map(ctx, fields.map_id, fields.x, fields.y)?;
    if ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(fields.template_entry)
        .is_none()
    {
        return Err(format!(
            "gameobject template {} does not exist",
            fields.template_entry
        ));
    }
    // A door or button toggles between 0 and 1.
    if fields.state > 1 {
        return Err(format!("gameobject state {} is not 0 or 1", fields.state));
    }
    Ok(())
}

fn check_position(x: f32, y: f32, z: f32, orientation: f32) -> Result<(), String> {
    if ![x, y, z, orientation].iter().all(|value| value.is_finite()) {
        return Err("the position and orientation must be finite".to_string());
    }
    let limit = lyracore_shared::spatial::MAP_COORD_MAX;
    if cell_index(x).is_none() || cell_index(y).is_none() || z.abs() > limit {
        return Err(format!("({x}, {y}, {z}) is outside the map bounds"));
    }
    Ok(())
}

/// A map is known when it holds instance-zero spawns, or terrain or navigation covers the position.
fn check_map(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Result<(), String> {
    let has_spawns = ctx
        .db
        .game_creature_spawn_map()
        .map_id()
        .find(map_id)
        .is_some()
        || ctx
            .db
            .game_creature_spawn()
            .by_map_instance()
            .filter((map_id, 0u64))
            .next()
            .is_some()
        || ctx
            .db
            .game_gameobject()
            .by_map()
            .filter(map_id)
            .any(|go| go.instance_id == 0);
    let has_ground = match (cell_index(x), cell_index(y)) {
        (Some(cell_x), Some(cell_y)) => {
            let key = cell_key(map_id, cell_x, cell_y);
            ctx.db.game_terrain_chunk().key().find(key).is_some()
                || ctx.db.game_nav_chunk().key().find(key).is_some()
        }
        _ => false,
    };
    if has_spawns || has_ground {
        Ok(())
    } else {
        Err(format!("map {map_id} is not loaded at ({x}, {y})"))
    }
}

fn place_creature(
    ctx: &ReducerContext,
    spawn: &mut CreatureSpawn,
    placement: &CreatureSpawnPlacement,
) {
    spawn.map_id = placement.map_id;
    spawn.x = placement.x;
    spawn.y = placement.y;
    spawn.z = placement.z;
    spawn.orientation = placement.orientation;
    spawn.movement_type = placement.movement_type;
    spawn.respawn_secs = placement.respawn_secs;
    rearm_respawn(ctx, spawn);
    let maps = ctx.db.game_creature_spawn_map();
    if maps.map_id().find(placement.map_id).is_none() {
        maps.insert(CreatureSpawnMap {
            map_id: placement.map_id,
        });
    }
}

// An instance map's source rows stay dormant; each Instance copies them when it is created.
fn rearm_respawn(ctx: &ReducerContext, spawn: &mut CreatureSpawn) {
    spawn.respawn_at = if lyracore_shared::instance::has_instance_population(spawn.map_id) {
        crate::creatures::timer_never(ctx)
    } else {
        ctx.timestamp
    };
    spawn.despawn_at = crate::creatures::timer_never(ctx);
}

// Running Instances keep their copies until they reset.
fn retire_live_creature(ctx: &ReducerContext, guid: u64) {
    if ctx.db.game_world_entity().guid().find(guid).is_some() {
        crate::creatures::despawn_creature_entity(ctx, guid);
    }
    let spawns = ctx.db.game_creature_spawn();
    let copies: Vec<u64> = spawns
        .by_source_guid()
        .filter(guid)
        .filter(|copy| copy.instance_id == 0)
        .map(|copy| copy.guid)
        .collect();
    for copy in copies {
        crate::creatures::despawn_creature_entity(ctx, copy);
        crate::instance::remove_spawn_waypoints(ctx, copy);
        spawns.guid().delete(copy);
    }
}

fn insert_gameobject(
    ctx: &ReducerContext,
    guid: u64,
    fields: &GameObjectSpawnFields,
) -> Result<GameObject, String> {
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(fields.x, fields.y);
    let go = GameObject {
        guid,
        template_entry: fields.template_entry,
        map_id: fields.map_id,
        x: fields.x,
        y: fields.y,
        z: fields.z,
        orientation: fields.orientation,
        state: fields.state,
        created_at: ctx.timestamp,
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
        rotation_0: fields.rotation_0,
        rotation_1: fields.rotation_1,
        rotation_2: fields.rotation_2,
        rotation_3: fields.rotation_3,
    }
    .try_insert(ctx)
    .map_err(|e| format!("gameobject spawn {guid} cannot be inserted: {e}"))?;
    crate::go_collider::register(ctx, &go);
    Ok(go)
}

fn describe_creature(spawn: &CreatureSpawn) -> String {
    format!(
        "entry {} map {} at ({}, {}, {}) facing {} movement {} respawn {}s",
        spawn.entry,
        spawn.map_id,
        spawn.x,
        spawn.y,
        spawn.z,
        spawn.orientation,
        spawn.movement_type,
        spawn.respawn_secs
    )
}

fn describe_gameobject(go: &GameObject) -> String {
    format!(
        "template {} map {} at ({}, {}, {}) facing {} state {} rotation ({}, {}, {}, {})",
        go.template_entry,
        go.map_id,
        go.x,
        go.y,
        go.z,
        go.orientation,
        go.state,
        go.rotation_0,
        go.rotation_1,
        go.rotation_2,
        go.rotation_3
    )
}

fn describe_route(points: impl Iterator<Item = (f32, f32, f32)>) -> String {
    points
        .map(|(x, y, z)| format!("({x}, {y}, {z})"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_spawn_id_starts_at_the_floor_and_refuses_past_the_ceiling() {
        assert_eq!(
            next_operator_spawn_id(OPERATOR_SPAWN_ID_FLOOR - 1),
            Ok(14_000_000)
        );
        assert_eq!(next_operator_spawn_id(14_000_041), Ok(14_000_042));
        assert!(next_operator_spawn_id(14_999_999).is_err());
    }

    #[test]
    fn a_position_must_be_finite_and_inside_the_map() {
        assert!(check_position(-8_949.95, -132.493, 83.5312, 0.0).is_ok());
        assert!(check_position(f32::NAN, 0.0, 0.0, 0.0).is_err());
        assert!(check_position(0.0, 0.0, 0.0, f32::INFINITY).is_err());
        assert!(check_position(17_100.0, 0.0, 0.0, 0.0).is_err());
        assert!(check_position(0.0, -17_100.0, 0.0, 0.0).is_err());
        assert!(check_position(0.0, 0.0, 20_000.0, 0.0).is_err());
    }
}
