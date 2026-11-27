//! Before changing Instance leases, population or removal, read docs/instance-lifecycle.md.

use std::collections::{HashMap, HashSet};

use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{
    game_creature_move_schedule, game_creature_spawn, game_creature_template,
    game_creature_waypoint, game_encounter_spawn, game_gameobject, game_gameobject_template,
    game_group, game_group_member, game_map, game_world_entity,
};

pub(crate) use lyracore_shared::instance::is_dungeon_map;

pub(crate) struct DungeonMap {
    pub(crate) map_id: u32,
    pub(crate) entrance: (u32, f32, f32, f32, f32),
    pub(crate) release_zone: u32,
    // Never resolve dungeon graveyards by outside-map coordinates.
    pub(crate) release_fallback: (u32, f32, f32, f32, f32),
}

pub(crate) const BOOTSTRAP_DUNGEONS: &[DungeonMap] = &[DungeonMap {
    map_id: 36,
    entrance: (0, -11080.0, 1520.0, 46.0, 0.0),
    release_zone: 1581,
    release_fallback: (0, -10650.0, 1180.0, 34.0, 0.0),
}];

pub(crate) fn dungeon(map_id: u32) -> Option<&'static DungeonMap> {
    BOOTSTRAP_DUNGEONS.iter().find(|d| d.map_id == map_id)
}

pub(crate) fn entrance_fallback(map_id: u32) -> Option<(u32, f32, f32, f32, f32)> {
    dungeon(map_id).map(|d| d.entrance)
}

pub(crate) fn imported_entrance_fallback(
    ctx: &ReducerContext,
    map_id: u32,
) -> Option<(u32, f32, f32, f32, f32)> {
    use crate::{game_area_trigger, game_areatrigger_teleport};
    let triggers = ctx.db.game_area_trigger();
    let portals = ctx.db.game_areatrigger_teleport();
    let exit = portals
        .iter()
        .filter(|portal| !lyracore_shared::instance::has_instance_population(portal.target_map))
        .filter(|portal| {
            triggers
                .id()
                .find(portal.trigger_id)
                .is_some_and(|trigger| trigger.map_id == map_id)
        })
        .min_by_key(|portal| portal.trigger_id)
        .map(|portal| (portal.target_map, portal.x, portal.y, portal.z, portal.o));
    exit.or_else(|| {
        portals
            .iter()
            .filter(|portal| portal.target_map == map_id)
            .filter_map(|portal| triggers.id().find(portal.trigger_id))
            .filter(|trigger| !lyracore_shared::instance::has_instance_population(trigger.map_id))
            .min_by_key(|trigger| trigger.id)
            .map(|trigger| (trigger.map_id, trigger.x, trigger.y, trigger.z, 0.0))
    })
    .or_else(|| entrance_fallback(map_id))
}

pub(crate) fn copy_spawn_waypoints(
    ctx: &ReducerContext,
    source_guid: u64,
    copied_guid: u64,
    next_id: &mut Option<u64>,
) -> Result<(), String> {
    let waypoints = ctx.db.game_creature_waypoint();
    let mut route: Vec<_> = waypoints.by_creature().filter(source_guid).collect();
    if route.is_empty() {
        return Ok(());
    }
    route.sort_by_key(|waypoint| waypoint.id);
    let next = next_id.get_or_insert_with(|| {
        waypoints
            .iter()
            .map(|waypoint| waypoint.id)
            .max()
            .unwrap_or(0)
    });
    for mut waypoint in route {
        *next = next.checked_add(1).ok_or("waypoint id exhausted")?;
        waypoint.id = *next;
        waypoint.creature_guid = copied_guid;
        waypoints.insert(waypoint);
    }
    Ok(())
}

pub(crate) fn remove_spawn_waypoints(ctx: &ReducerContext, guid: u64) {
    let waypoints = ctx.db.game_creature_waypoint();
    for waypoint in waypoints.by_creature().filter(guid).collect::<Vec<_>>() {
        waypoints.id().delete(waypoint.id);
    }
}

pub(crate) fn remove_spawn_copies(ctx: &ReducerContext, source_guid: u64) {
    let spawns = ctx.db.game_creature_spawn();
    for spawn in spawns
        .by_source_guid()
        .filter(source_guid)
        .collect::<Vec<_>>()
    {
        crate::creatures::despawn_creature_entity(ctx, spawn.guid);
        remove_spawn_waypoints(ctx, spawn.guid);
        crate::items::vendor_stock::forget(ctx, spawn.guid);
        spawns.guid().delete(spawn.guid);
    }
}

pub(crate) const INSTANCE_EMPTY_REAP_MICROS: i64 = 30 * 60 * 1_000_000;

pub(crate) const INSTANCE_REAPER_INTERVAL_MICROS: i64 = 60 * 1_000_000;

// A lease reads empty at run start, so its expiry must exceed the whole dungeon run.
pub(crate) const INSTANCE_LEASE_REAP_MICROS: i64 = 12 * 60 * 60 * 1_000_000;

pub(crate) fn empty_reap_micros(hosts_populations: bool) -> i64 {
    if hosts_populations {
        INSTANCE_EMPTY_REAP_MICROS
    } else {
        INSTANCE_LEASE_REAP_MICROS
    }
}

const REAP_MAX_PER_FIRING: usize = 4;

use crate::creatures::population::INSTANCE_POP_LOW_BAND;
use crate::gameobject::GO_COPY_BAND;

fn next_population_low(slot: &mut u64, mut taken: impl FnMut(u64) -> bool) -> Option<u64> {
    loop {
        *slot += 1;
        if *slot >= INSTANCE_POP_LOW_BAND {
            return None;
        }
        let low = INSTANCE_POP_LOW_BAND | *slot;
        if !taken(low) {
            return Some(low);
        }
    }
}

const GO_COPY_TYPES: [u8; 4] = [
    lyracore_shared::constants::go_type::DOOR,
    lyracore_shared::constants::go_type::BUTTON,
    lyracore_shared::constants::go_type::CHEST,
    lyracore_shared::constants::go_type::GOOBER,
];

#[table(
    accessor = game_instance,
    index(accessor = by_party, btree(columns = [party_id])),
    index(accessor = by_map, btree(columns = [map_id]))
)]
pub struct GameInstance {
    #[primary_key]
    #[auto_inc]
    pub instance_id: u64,
    pub map_id: u32,
    pub party_id: u64,
    pub created_at: Timestamp,
    pub last_empty_at_micros: i64,
    pub reset_requested: bool,
    #[default(0u64)]
    pub holder_guid: u64,
}

#[table(
    accessor = game_instance_binding,
    index(accessor = by_character, btree(columns = [character_guid])),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct GameInstanceBinding {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub instance_id: u64,
    pub map_id: u32,
    #[default(None::<spacetimedb::Identity>)]
    pub world: Option<spacetimedb::Identity>,
}

crate::character_owned!(delete, fn sweep_delete_game_instance_binding(ctx, character_guid) {
    let t = ctx.db.game_instance_binding();
    for b in t.by_character().filter(&character_guid).collect::<Vec<_>>() {
        t.id().delete(b.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_instance_binding(ctx, character_guid, io) {
    table = game_instance_binding,
    by = by_character,
    remint = id,
});

#[table(accessor = game_instance_reaper_schedule, scheduled(reap_instances))]
pub struct InstanceReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) const INSTANCE_REMOVAL_MICROS: i64 =
    lyracore_shared::instance::INSTANCE_REMOVAL_MS as i64 * 1_000;

#[table(accessor = game_instance_removal, scheduled(expire_instance_removal))]
pub struct InstanceRemoval {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub character_guid: u64,
    pub instance_id: u64,
    pub group_id: u64,
}

impl InstanceRemoval {
    pub(crate) fn insert(self, ctx: &ReducerContext) {
        let row = ctx.db.game_instance_removal().insert(self);
        crate::native::instance::removal_changed(ctx, row.character_guid, Some(&row));
    }

    pub(crate) fn delete(ctx: &ReducerContext, character_guid: u64) {
        if ctx
            .db
            .game_instance_removal()
            .character_guid()
            .delete(character_guid)
        {
            crate::native::instance::removal_changed(ctx, character_guid, None);
        }
    }
}

crate::character_owned!(delete, fn sweep_delete_game_instance_removal(ctx, character_guid) {
    InstanceRemoval::delete(ctx, character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_instance_removal());

#[derive(Debug, PartialEq)]
pub(crate) enum InstanceRoute {
    Own(u64),
    Party(u64),
    Create,
}

/// The live Instance a Character's binding names, with the Group that owns it (0 for a solo run).
#[derive(Clone, Copy)]
pub(crate) struct BoundRun {
    pub(crate) instance_id: u64,
    pub(crate) party_id: u64,
}

/// A binding to a Group's run never routes: vmangos keeps that run with the Group, not its members.
pub(crate) fn route_instance(bound: Option<BoundRun>, party_live: Option<u64>) -> InstanceRoute {
    match (bound, party_live) {
        (_, Some(id)) => InstanceRoute::Party(id),
        (
            Some(BoundRun {
                instance_id,
                party_id: 0,
            }),
            None,
        ) => InstanceRoute::Own(instance_id),
        _ => InstanceRoute::Create,
    }
}

fn instance_capacity(map_id: u32) -> usize {
    match map_id {
        249 | 409 | 469 | 531 | 533 => lyracore_shared::group::RAID_MAX_MEMBERS,
        309 | 509 => 20,
        229 => 10,
        _ => crate::group::GROUP_MAX_MEMBERS,
    }
}

pub(crate) fn party_size_allows_entry(map_id: u32, member_count: usize) -> bool {
    member_count <= instance_capacity(map_id)
}

#[derive(Debug, PartialEq)]
pub(crate) enum OccupancyAction {
    ClearStamp,
    Stamp,
    Wait,
    Reap,
}

pub(crate) fn occupancy_action(
    occupied: bool,
    last_empty_at_micros: i64,
    reset_requested: bool,
    now_micros: i64,
    empty_reap_micros: i64,
) -> OccupancyAction {
    if occupied {
        return if last_empty_at_micros != 0 {
            OccupancyAction::ClearStamp
        } else {
            OccupancyAction::Wait
        };
    }
    if reset_requested {
        return OccupancyAction::Reap;
    }
    if last_empty_at_micros == 0 {
        return OccupancyAction::Stamp;
    }
    if now_micros - last_empty_at_micros >= empty_reap_micros {
        OccupancyAction::Reap
    } else {
        OccupancyAction::Wait
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum StrandingFallback {
    Entrance(u32, f32, f32, f32, f32),
    InPlaceOpenWorld,
    HearthstoneHome,
}

pub(crate) fn stranding_fallback(
    entrance: Option<(u32, f32, f32, f32, f32)>,
    is_dungeon: bool,
) -> StrandingFallback {
    match (entrance, is_dungeon) {
        (Some((m, x, y, z, o)), _) => StrandingFallback::Entrance(m, x, y, z, o),
        (None, false) => StrandingFallback::InPlaceOpenWorld,
        (None, true) => StrandingFallback::HearthstoneHome,
    }
}

pub(crate) fn is_non_raid_dungeon_map(map: u32) -> bool {
    is_dungeon_map(map) && !matches!(map, 249 | 309 | 409 | 469 | 509 | 531 | 533)
}

pub(crate) fn resettable_map(map: u32) -> bool {
    is_non_raid_dungeon_map(map)
}

pub(crate) fn instance_removal_due(
    map_id: u32,
    instance_id: u64,
    theirs: bool,
    gm_level: u8,
) -> bool {
    instance_id != 0 && is_dungeon_map(map_id) && !theirs && gm_level == 0
}

/// `holder` pairs a solo run's holder with its Group; read docs/instance-lifecycle.md before
/// changing who may stay.
pub(crate) fn run_is_theirs(
    run_party: u64,
    holder: Option<(u64, Option<u64>)>,
    character_guid: u64,
    own_group: Option<u64>,
) -> bool {
    if run_party != 0 {
        return own_group == Some(run_party);
    }
    holder.is_none_or(|(holder, holder_group)| {
        holder == character_guid || (own_group.is_some() && holder_group == own_group)
    })
}

const MAP_TYPE_CONTINENT: u32 = 0;
const MAP_TYPE_RAID: u32 = 2;

pub(crate) fn is_instanceable(ctx: &ReducerContext, map_id: u32) -> bool {
    ctx.db
        .game_map()
        .id()
        .find(map_id)
        .is_some_and(|map| map.map_type != MAP_TYPE_CONTINENT)
}

fn raid_refuses(inside: bool, in_raid: bool, gm_level: u8) -> bool {
    !inside && !in_raid && gm_level == 0
}

pub(crate) fn raid_entry_refused(ctx: &ReducerContext, character_guid: u64, map_id: u32) -> bool {
    if !ctx
        .db
        .game_map()
        .id()
        .find(map_id)
        .is_some_and(|map| map.map_type == MAP_TYPE_RAID)
    {
        return false;
    }
    let inside = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_some_and(|entity| entity.map_id == map_id);
    let gm_level = crate::helpers::character_by_guid(ctx, character_guid).map_or(0, |character| {
        crate::auth::gm_level(ctx, character.account_id)
    });
    raid_refuses(inside, crate::group::in_raid(ctx, character_guid), gm_level)
}

fn binding_is_local(ctx: &ReducerContext, map: u32, world: Option<spacetimedb::Identity>) -> bool {
    use crate::native::directory::game_map_owner;
    world
        .or_else(|| {
            ctx.db
                .game_map_owner()
                .map_id()
                .find(map)
                .map(|owner| owner.world)
        })
        .is_none_or(|world| world == ctx.database_identity())
}

fn live_instance_for(ctx: &ReducerContext, instance_id: u64, map_id: u32) -> Option<GameInstance> {
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .filter(|i| i.map_id == map_id && !i.reset_requested)
}

fn held_by_another(run: &GameInstance, character_guid: u64) -> bool {
    run.party_id == 0 && run.holder_guid != 0 && run.holder_guid != character_guid
}

fn solo_runs_of(ctx: &ReducerContext, character_guid: u64) -> Vec<GameInstance> {
    ctx.db
        .game_instance_binding()
        .by_character()
        .filter(&character_guid)
        .filter(|b| binding_is_local(ctx, b.map_id, b.world))
        .filter_map(|b| live_instance_for(ctx, b.instance_id, b.map_id))
        .filter(|run| run.party_id == 0 && !held_by_another(run, character_guid))
        .collect()
}

fn solo_run_of(ctx: &ReducerContext, character_guid: u64, map_id: u32) -> Option<GameInstance> {
    solo_runs_of(ctx, character_guid)
        .into_iter()
        .find(|run| run.map_id == map_id)
}

/// Records `holder` on the run; with no holder the run is reset so that no binding routes there.
fn release_run(ctx: &ReducerContext, mut run: GameInstance, holder: Option<u64>) {
    let bindings = ctx.db.game_instance_binding();
    for binding in bindings
        .by_instance()
        .filter(&run.instance_id)
        .filter(|b| {
            b.map_id == run.map_id
                && Some(b.character_guid) != holder
                && binding_is_local(ctx, b.map_id, b.world)
        })
        .collect::<Vec<_>>()
    {
        bindings.id().delete(binding.id);
    }
    if let Some(holder) = holder {
        bind_character(
            ctx,
            holder,
            run.instance_id,
            run.map_id,
            ctx.database_identity(),
        );
    }
    log::info!(
        "instance {} released by party {} to {holder:?}",
        run.instance_id,
        run.party_id
    );
    run.party_id = 0;
    run.holder_guid = holder.unwrap_or(0);
    run.reset_requested |= holder.is_none();
    ctx.db.game_instance().instance_id().update(run);
}

/// `previous_leader` is `None` when the Group forms or a Shard first sees it; read
/// docs/instance-lifecycle.md before changing a handover.
pub(crate) fn hand_runs_to_leader(
    ctx: &ReducerContext,
    group_id: u64,
    previous_leader: Option<u64>,
) {
    let Some(leader) = ctx
        .db
        .game_group()
        .group_id()
        .find(group_id)
        .map(|group| group.leader_guid)
    else {
        return;
    };
    let mut changed = false;
    if let Some(previous) = previous_leader.filter(|previous| *previous != leader) {
        let runs: Vec<_> = ctx
            .db
            .game_instance()
            .by_party()
            .filter(&group_id)
            .filter(|run| !run.reset_requested && solo_run_of(ctx, leader, run.map_id).is_some())
            .collect();
        for run in runs {
            let keeps_it = crate::helpers::character_by_guid(ctx, previous).is_some()
                && solo_run_of(ctx, previous, run.map_id).is_none();
            release_run(ctx, run, keeps_it.then_some(previous));
            changed = true;
        }
    }
    for run in solo_runs_of(ctx, leader) {
        changed |= adopt_instance_for_party(ctx, run.instance_id, group_id);
    }
    if changed {
        reconcile_group_members(ctx, group_id);
    }
}

pub(crate) fn hand_run_to_survivor(ctx: &ReducerContext, group_id: u64, survivor: u64) {
    let Some(entity) = ctx.db.game_world_entity().guid().find(survivor) else {
        return;
    };
    let Some(run) = live_instance_for(ctx, entity.instance_id, entity.map_id)
        .filter(|run| run.party_id == group_id && group_id != 0)
    else {
        return;
    };
    if solo_run_of(ctx, survivor, run.map_id).is_none() {
        release_run(ctx, run, Some(survivor));
    }
}

fn party_run(ctx: &ReducerContext, party_id: u64, map_id: u32) -> Option<u64> {
    ctx.db
        .game_instance()
        .by_party()
        .filter(&party_id)
        .find(|i| i.map_id == map_id && !i.reset_requested)
        .map(|i| i.instance_id)
}

pub(crate) fn bind_character(
    ctx: &ReducerContext,
    character_guid: u64,
    instance_id: u64,
    map_id: u32,
    world: spacetimedb::Identity,
) {
    let t = ctx.db.game_instance_binding();
    let stale: Vec<u64> = t
        .by_character()
        .filter(&character_guid)
        .filter(|b| b.map_id == map_id)
        .map(|b| b.id)
        .collect();
    for id in stale {
        t.id().delete(id);
    }
    t.insert(GameInstanceBinding {
        id: 0,
        character_guid,
        instance_id,
        map_id,
        world: Some(world),
    });
}

pub(crate) fn resolve_or_create_instance(
    ctx: &ReducerContext,
    character_guid: u64,
    target_map: u32,
) -> Result<u64, String> {
    let group = crate::group::group_of(ctx, character_guid);
    let member_count = group
        .as_ref()
        .map(|g| crate::group::members_of(ctx, g.group_id).len())
        .unwrap_or(1);
    if !party_size_allows_entry(target_map, member_count) {
        return Err(format!(
            "party of {member_count} exceeds the {}-player dungeon cap",
            instance_capacity(target_map)
        ));
    }

    let bindings = ctx.db.game_instance_binding();
    let own_binding = bindings
        .by_character()
        .filter(&character_guid)
        .find(|b| b.map_id == target_map);
    let bound = own_binding.and_then(|b| {
        let live = if binding_is_local(ctx, b.map_id, b.world) {
            live_instance_for(ctx, b.instance_id, target_map)
                .filter(|run| !held_by_another(run, character_guid))
        } else {
            None
        };
        if live.is_none() {
            bindings.id().delete(b.id);
        }
        live.map(|instance| BoundRun {
            instance_id: instance.instance_id,
            party_id: instance.party_id,
        })
    });

    let party_live = group
        .as_ref()
        .and_then(|g| party_run(ctx, g.group_id, target_map));

    let party_id = group.as_ref().map(|g| g.group_id).unwrap_or(0);
    match route_instance(bound, party_live) {
        InstanceRoute::Own(id) => {
            adopt_instance_for_party(ctx, id, party_id);
            Ok(id)
        }
        InstanceRoute::Party(id) => {
            bind_character(ctx, character_guid, id, target_map, ctx.database_identity());
            Ok(id)
        }
        InstanceRoute::Create => {
            let id = create_instance(ctx, 0, target_map, party_id)?;
            bind_character(ctx, character_guid, id, target_map, ctx.database_identity());
            Ok(id)
        }
    }
}

pub(crate) fn admit_existing_party_instance(
    ctx: &ReducerContext,
    character_guid: u64,
    target_map: u32,
    expected_instance: u64,
) -> Result<u64, String> {
    let group = crate::group::group_of(ctx, character_guid)
        .ok_or("Character has no party for the expected instance")?;
    let member_count = ctx
        .db
        .game_group_member()
        .by_group()
        .filter(&group.group_id)
        .take(instance_capacity(target_map) + 1)
        .count();
    if !party_size_allows_entry(target_map, member_count) {
        return Err(format!(
            "party of {member_count} exceeds the {}-player dungeon cap",
            instance_capacity(target_map)
        ));
    }
    live_instance_for(ctx, expected_instance, target_map)
        .ok_or("expected party instance is unavailable")?;
    if !adopt_instance_for_party(ctx, expected_instance, group.group_id) {
        return Err("expected instance cannot be the party's run".to_string());
    }
    bind_character(
        ctx,
        character_guid,
        expected_instance,
        target_map,
        ctx.database_identity(),
    );
    Ok(expected_instance)
}

fn instance_is_ownerless(ctx: &ReducerContext, party_id: u64) -> bool {
    party_id == 0 || ctx.db.game_group().group_id().find(party_id).is_none()
}

/// True when the Group owns the run afterwards; a Group never takes a second run of one map.
fn adopt_instance_for_party(ctx: &ReducerContext, instance_id: u64, party_id: u64) -> bool {
    if party_id == 0 {
        return false;
    }
    let instances = ctx.db.game_instance();
    let Some(mut inst) = instances.instance_id().find(instance_id) else {
        return false;
    };
    if inst.party_id == party_id {
        return true;
    }
    if !instance_is_ownerless(ctx, inst.party_id) || party_run(ctx, party_id, inst.map_id).is_some()
    {
        return false;
    }
    inst.party_id = party_id;
    inst.holder_guid = 0;
    instances.instance_id().update(inst);
    log::info!("instance {instance_id} adopted by party {party_id}");
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoinReset {
    Done,
    /// The joining member is on another Shard; its import owes the reset.
    AwaitsArrival,
}

/// vmangos `Group::AddMember` resets a joining member's own runs, except on the map it stands on.
pub(crate) fn unbind_joining_member(ctx: &ReducerContext, character_guid: u64) -> JoinReset {
    let Some(member) = crate::group::group_of(ctx, character_guid) else {
        return JoinReset::Done;
    };
    if ctx
        .db
        .game_group()
        .group_id()
        .find(member.group_id)
        .is_none_or(|group| group.leader_guid == character_guid)
    {
        return JoinReset::Done;
    }
    let Some(character) = crate::helpers::character_by_guid(ctx, character_guid) else {
        return JoinReset::AwaitsArrival;
    };
    unbind_outside_standing_map(ctx, &character);
    JoinReset::Done
}

/// The reset `unbind_joining_member` owed a joining member that has since arrived on this Shard.
pub(crate) fn unbind_arrived_joiner(ctx: &ReducerContext, character_guid: u64) {
    if let Some(character) = crate::helpers::character_by_guid(ctx, character_guid) {
        unbind_outside_standing_map(ctx, &character);
    }
}

fn unbind_outside_standing_map(ctx: &ReducerContext, character: &crate::character::Character) {
    let standing_map = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character.guid)
        .map(|entity| entity.map_id);
    let bindings = ctx.db.game_instance_binding();
    for binding in bindings
        .by_character()
        .filter(&character.guid)
        .collect::<Vec<_>>()
    {
        if binding.map_id != character.map_id && Some(binding.map_id) != standing_map {
            bindings.id().delete(binding.id);
        }
    }
}

pub(crate) fn hosts_instance_populations(ctx: &ReducerContext) -> bool {
    crate::config::server_config(ctx).hosts_instances
}

/// Creates the Instance and, on a database that hosts populations, copies its population;
/// `instance_id` 0 allocates a fresh id.
pub(crate) fn create_instance(
    ctx: &ReducerContext,
    instance_id: u64,
    map_id: u32,
    party_id: u64,
) -> Result<u64, String> {
    let inst = ctx.db.game_instance().insert(GameInstance {
        instance_id,
        map_id,
        party_id,
        created_at: crate::time::now(ctx),
        last_empty_at_micros: 0,
        reset_requested: false,
        holder_guid: 0,
    });
    let instance_id = inst.instance_id;

    if !hosts_instance_populations(ctx) {
        log::info!(
            "create_instance: instance {instance_id} map {map_id} party {party_id} — LEASE only \
             (game_config.hosts_instances is off: this database does not host instance populations)"
        );
        return Ok(instance_id);
    }

    let entities = ctx.db.game_world_entity();
    let mut max_band_low: HashMap<u32, u64> = HashMap::new();
    for e in entities.iter() {
        if e.is_player() || e.entry == 0 {
            continue;
        }
        let low = e.guid & 0xFF_FFFF;
        if low & INSTANCE_POP_LOW_BAND != 0 {
            let seq = low & (INSTANCE_POP_LOW_BAND - 1);
            let slot = max_band_low.entry(e.entry).or_insert(0);
            if seq > *slot {
                *slot = seq;
            }
        }
    }

    for spawn in ctx.db.game_creature_spawn().by_instance().filter(1u64..) {
        let low = spawn.guid & 0xFF_FFFF;
        if low & INSTANCE_POP_LOW_BAND != 0 {
            let slot = max_band_low.entry(spawn.entry).or_insert(0);
            *slot = (*slot).max(low & (INSTANCE_POP_LOW_BAND - 1));
        }
    }
    let mut next_waypoint_id = None;

    let tracked_wave_guids: std::collections::HashSet<u64> = ctx
        .db
        .game_encounter_spawn()
        .iter()
        .map(|t| t.guid)
        .collect();
    let spawns: Vec<crate::CreatureSpawn> = ctx
        .db
        .game_creature_spawn()
        .by_map_instance()
        .filter((map_id, 0u64))
        .filter(|s| !tracked_wave_guids.contains(&s.guid))
        .collect();
    let templates = ctx.db.game_creature_template();
    let mut spawned = 0u32;
    for spawn in &spawns {
        let mut copy = spawn.clone();
        copy.instance_id = instance_id;
        copy.source_guid = spawn.guid;
        let Some(entry) = crate::creatures::population::entry_for_spawn(ctx, &copy) else {
            continue;
        };
        copy.entry = entry;
        let Some(tmpl) = templates.entry().find(entry) else {
            continue;
        };
        let slot = max_band_low.entry(entry).or_insert(0);
        // Dormant instance-zero rows in the Operator and Package spawn ranges share this band.
        let low = next_population_low(slot, |low| {
            let guid = lyracore_package_delta::packed_creature_spawn_guid(entry, low as u32);
            ctx.db.game_creature_spawn().guid().find(guid).is_some()
                || entities.guid().find(guid).is_some()
        })
        .ok_or_else(|| format!("instance population low exhausted for entry {entry}"))?;
        copy.guid = lyracore_package_delta::packed_creature_spawn_guid(entry, low as u32);
        copy.life_seq = 1;
        copy.respawn_at = crate::time::timer_never(ctx);
        copy.despawn_at = crate::time::timer_never(ctx);
        // A copy whose Game Events hide it waits with its respawn off until they show it.
        let entity = crate::game_events::spawn_shows(ctx, spawn.guid).then(|| {
            crate::creatures::build_creature_entity(ctx, &copy, &tmpl, ctx.random(), instance_id)
        });
        let copied_guid = copy.guid;
        ctx.db.game_creature_spawn().insert(copy);
        copy_spawn_waypoints(ctx, spawn.guid, copied_guid, &mut next_waypoint_id)?;
        if let Some(entity) = entity {
            crate::creatures::insert_creature_entity(ctx, entity);
            spawned += 1;
        }
    }

    let gos = ctx.db.game_gameobject();
    let first_go_seq = gos
        .iter()
        .filter(|g| {
            crate::gameobject::is_gameobject_guid(g.guid)
                && g.guid & GO_COPY_BAND != 0
                && g.guid & crate::gameobject::POOL_TAG == 0
        })
        .map(|g| g.guid & (GO_COPY_BAND - 1))
        .max()
        .unwrap_or(0)
        + 1;
    let sources: Vec<crate::gameobject::GameObject> = gos
        .iter()
        .filter(|g| {
            g.map_id == map_id
                && g.instance_id == 0
                && g.guid & crate::gameobject::POOL_TAG == 0
                && g.guid & GO_COPY_BAND == 0
                && ctx
                    .db
                    .game_gameobject_template()
                    .entry()
                    .find(g.template_entry)
                    .map(|t| GO_COPY_TYPES.contains(&t.type_id))
                    .unwrap_or(false)
                && crate::game_events::spawn_shows(ctx, g.guid)
        })
        .collect();
    let mut copied = 0u32;
    for (next_go_seq, src) in (first_go_seq..).zip(&sources) {
        crate::gameobject::GameObject {
            guid: crate::gameobject::gameobject_guid(GO_COPY_BAND | next_go_seq),
            template_entry: src.template_entry,
            map_id: src.map_id,
            x: src.x,
            y: src.y,
            z: src.z,
            orientation: src.orientation,
            state: src.state,
            created_at: crate::time::now(ctx),
            respawn_at_micros: 0,
            instance_id,
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            rotation_0: src.rotation_0,
            rotation_1: src.rotation_1,
            rotation_2: src.rotation_2,
            rotation_3: src.rotation_3,
            respawn_min_secs: src.respawn_min_secs,
            respawn_max_secs: src.respawn_max_secs,
            despawned: false,
        }
        .insert(ctx);
        copied += 1;
    }
    crate::gameobject::arm_instance_pools(ctx, map_id, instance_id);

    log::info!(
        "create_instance: instance {instance_id} map {map_id} party {party_id} — {spawned} creatures, {copied} GO copies"
    );
    Ok(instance_id)
}

#[reducer]
pub fn ensure_instance(
    ctx: &ReducerContext,
    instance_id: u64,
    map_id: u32,
    party_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor(ctx, request_actor)?;
    if instance_id == 0 {
        return Err("instance 0 is the open world — it is never mirrored".to_string());
    }
    if let Some(mut existing) = ctx.db.game_instance().instance_id().find(instance_id) {
        if existing.map_id != map_id {
            return Err(format!(
                "instance {instance_id} already exists here on map {} — refusing to mirror it as \
                 map {map_id} (two shards allocated the same id, which the shared realm index is \
                 meant to prevent)",
                existing.map_id
            ));
        }
        if existing.party_id == party_id {
            return Ok(());
        }
        if party_id != 0 && instance_is_ownerless(ctx, existing.party_id) {
            existing.party_id = party_id;
            existing.holder_guid = 0;
            ctx.db.game_instance().instance_id().update(existing);
            return Ok(());
        }
        return Err(format!(
            "instance {instance_id} belongs to party {}, not party {party_id}",
            existing.party_id
        ));
    }
    let id = create_instance(ctx, instance_id, map_id, party_id)?;
    log::info!("ensure_instance: mirrored instance {id} (map {map_id}, party {party_id})");
    Ok(())
}

// Keep the lease for later party arrivals and refuse eviction while a Character remains inside.
#[reducer]
pub fn evict_instance_population(
    ctx: &ReducerContext,
    instance_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor(ctx, request_actor)?;
    if instance_id == 0 {
        return Err("instance 0 is the open world — it is never evicted".to_string());
    }
    if ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_none()
    {
        return Ok(());
    }
    teardown_instance_inner(ctx, instance_id, false);
    Ok(())
}

#[reducer]
pub fn reap_instances(ctx: &ReducerContext, _schedule: InstanceReaperSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    let instances: Vec<GameInstance> = ctx.db.game_instance().iter().collect();
    if instances.is_empty() {
        return;
    }
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
    let reap_after = empty_reap_micros(hosts_instance_populations(ctx));
    let occupied = occupied_instances(ctx);
    let mut reaped = 0usize;
    for mut inst in instances {
        match occupancy_action(
            occupied.contains(&inst.instance_id),
            inst.last_empty_at_micros,
            inst.reset_requested,
            now,
            reap_after,
        ) {
            OccupancyAction::ClearStamp => {
                inst.last_empty_at_micros = 0;
                ctx.db.game_instance().instance_id().update(inst);
            }
            OccupancyAction::Stamp => {
                inst.last_empty_at_micros = now;
                ctx.db.game_instance().instance_id().update(inst);
            }
            OccupancyAction::Reap => {
                if reaped < REAP_MAX_PER_FIRING {
                    teardown_instance(ctx, inst.instance_id);
                    reaped += 1;
                }
            }
            OccupancyAction::Wait => {}
        }
    }
}

pub(crate) fn occupied_instances(ctx: &ReducerContext) -> HashSet<u64> {
    let mut occupied: HashSet<u64> = ctx
        .db
        .game_world_entity()
        .iter()
        .filter(|e| e.is_player() && e.instance_id != 0)
        .map(|e| e.instance_id)
        .collect();
    occupied.extend(crate::native::instance_shard::occupied(ctx));
    match crate::transfer::in_transit_instances(ctx) {
        Ok(claimed) => occupied.extend(claimed),
        Err(crate::transfer::TransferClaimReadLimit) => {
            occupied.extend(
                ctx.db
                    .game_instance()
                    .iter()
                    .map(|instance| instance.instance_id),
            );
        }
    }
    occupied
}

pub(crate) fn teardown_instance(ctx: &ReducerContext, instance_id: u64) {
    teardown_instance_inner(ctx, instance_id, true)
}

pub(crate) fn teardown_instance_inner(ctx: &ReducerContext, instance_id: u64, delete_row: bool) {
    if instance_id == 0 {
        return;
    }
    let entities = ctx.db.game_world_entity();
    if entities
        .iter()
        .any(|e| e.instance_id == instance_id && e.is_player())
    {
        log::warn!("teardown_instance: instance {instance_id} has live players — refused");
        return;
    }

    let doomed: Vec<u64> = entities
        .iter()
        .filter(|e| e.instance_id == instance_id && !e.is_player())
        .map(|e| e.guid)
        .collect();
    for guid in &doomed {
        crate::creatures::despawn_creature_entity(ctx, *guid);
    }

    let spawns = ctx.db.game_creature_spawn();
    let copies: Vec<_> = spawns.by_instance().filter(instance_id).collect();
    for spawn in copies {
        remove_spawn_waypoints(ctx, spawn.guid);
        crate::items::vendor_stock::forget(ctx, spawn.guid);
        spawns.guid().delete(spawn.guid);
    }
    crate::creatures::population::clear_instance(ctx, instance_id);

    // Player corpses outlive the Instance; see docs/instance-lifecycle.md before deleting them here.
    let gos = ctx.db.game_gameobject();
    let copies: Vec<u64> = gos
        .iter()
        .filter(|g| g.instance_id == instance_id)
        .map(|g| g.guid)
        .collect();
    for guid in &copies {
        crate::loot::reap_corpse_loot_family(ctx, *guid);
        crate::GameObject::delete(ctx, guid);
    }
    crate::gameobject::forget_instance_pools(ctx, instance_id);

    crate::creatures::cancel_relay_runs_for_instance(ctx, instance_id);
    crate::encounter::sweep_encounter_state(ctx, instance_id);

    let sched = ctx.db.game_creature_move_schedule();
    let ticks: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == instance_id)
        .map(|r| r.scheduled_id)
        .collect();
    for id in ticks {
        sched.scheduled_id().delete(id);
    }

    // Remove bindings and the Instance last; population must never outlive its parent row.
    if !delete_row {
        log::info!(
            "teardown_instance: instance {instance_id} population EVICTED ({} entities, {} GO \
             copies) — row + bindings kept for later party arrivals",
            doomed.len(),
            copies.len()
        );
        return;
    }
    let bindings = ctx.db.game_instance_binding();
    let stale: Vec<u64> = bindings
        .by_instance()
        .filter(&instance_id)
        .filter(|binding| binding_is_local(ctx, binding.map_id, binding.world))
        .map(|b| b.id)
        .collect();
    for id in stale {
        bindings.id().delete(id);
    }
    ctx.db.game_instance().instance_id().delete(instance_id);
    log::info!(
        "teardown_instance: instance {instance_id} reaped ({} entities, {} GO copies)",
        doomed.len(),
        copies.len()
    );
}

#[derive(spacetimedb::SpacetimeType, Clone)]
pub(crate) struct ResetBinding {
    pub map: u32,
    pub instance: u64,
    pub world: Option<spacetimedb::Identity>,
}

pub(crate) fn reset_bindings(ctx: &ReducerContext, guid: u64) -> Vec<ResetBinding> {
    ctx.db
        .game_instance_binding()
        .by_character()
        .filter(guid)
        .map(|binding| ResetBinding {
            map: binding.map_id,
            instance: binding.instance_id,
            world: binding.world,
        })
        .collect()
}

#[derive(spacetimedb::SpacetimeType)]
pub(crate) enum ResetFailure {
    Occupied,
    Offline,
}

#[derive(spacetimedb::SpacetimeType)]
pub(crate) struct ResetOutcome {
    pub map: u32,
    pub failure: Option<ResetFailure>,
}

pub(crate) fn reset_instances(
    ctx: &ReducerContext,
    guid: u64,
    bindings: &[ResetBinding],
    offline: bool,
) -> Vec<ResetOutcome> {
    let group = crate::group::group_of(ctx, guid)
        .and_then(|member| ctx.db.game_group().group_id().find(member.group_id));
    if group
        .as_ref()
        .is_some_and(|group| group.leader_guid != guid)
    {
        return Vec::new();
    }
    let candidates: Vec<_> = if let Some(group) = group {
        ctx.db
            .game_instance()
            .by_party()
            .filter(group.group_id)
            .collect()
    } else {
        bindings
            .iter()
            .filter(|binding| binding_is_local(ctx, binding.map, binding.world))
            .filter_map(|binding| {
                ctx.db
                    .game_instance()
                    .instance_id()
                    .find(binding.instance)
                    .filter(|instance| {
                        instance.map_id == binding.map
                            && instance_is_ownerless(ctx, instance.party_id)
                            && !held_by_another(instance, guid)
                    })
            })
            .collect()
    };
    let occupied = occupied_instances(ctx);
    let mut results = Vec::new();
    for mut instance in candidates {
        if instance.reset_requested
            || !resettable_map(instance.map_id)
            || !crate::native::directory::owns_map(ctx, instance.map_id)
        {
            continue;
        }
        let failure = if offline {
            Some(ResetFailure::Offline)
        } else if occupied.contains(&instance.instance_id) {
            Some(ResetFailure::Occupied)
        } else {
            None
        };
        let accepted = failure.is_none();
        results.push(ResetOutcome {
            map: instance.map_id,
            failure,
        });
        if accepted {
            instance.reset_requested = true;
            ctx.db.game_instance().instance_id().update(instance);
        }
    }
    results
}

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn apply_reset_instances(ctx: &ReducerContext, guid: u64) -> Result<u32, String> {
    let results = reset_instances(ctx, guid, &reset_bindings(ctx, guid), false);
    let flagged = results
        .iter()
        .filter(|result| result.failure.is_none())
        .count() as u32;
    if flagged == 0 {
        Err("no unoccupied instance eligible for reset".to_string())
    } else {
        Ok(flagged)
    }
}

#[derive(Clone, Copy)]
struct RemovalStanding {
    instance_id: u64,
    group_id: u64,
    in_world: bool,
}

fn instance_removal_standing(ctx: &ReducerContext, character_guid: u64) -> Option<RemovalStanding> {
    let character = crate::helpers::character_by_guid(ctx, character_guid)?;
    let entity = ctx.db.game_world_entity().guid().find(character_guid);
    let (map_id, instance_id) = match &entity {
        Some(_) if !character.online => return None,
        Some(entity) => (entity.map_id, entity.instance_id),
        None => (character.map_id, character.pending_instance_id),
    };
    let run = ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .filter(|instance| instance.map_id == map_id);
    let own_group = crate::group::group_of(ctx, character_guid).map(|member| member.group_id);
    let theirs = run.as_ref().is_none_or(|run| {
        let holder = (run.holder_guid != 0).then(|| {
            let group = crate::group::group_of(ctx, run.holder_guid).map(|member| member.group_id);
            (run.holder_guid, group)
        });
        run_is_theirs(run.party_id, holder, character_guid, own_group)
    });
    instance_removal_due(
        map_id,
        instance_id,
        theirs,
        crate::auth::gm_level(ctx, character.account_id),
    )
    .then_some(RemovalStanding {
        instance_id,
        group_id: run.map_or(0, |run| run.party_id),
        in_world: entity.is_some(),
    })
}

fn adopt_for_regrouped_member(ctx: &ReducerContext, character_guid: u64) -> Option<u64> {
    let standing =
        instance_removal_standing(ctx, character_guid).filter(|standing| standing.group_id != 0)?;
    let own_group = crate::group::group_of(ctx, character_guid)?.group_id;
    adopt_instance_for_party(ctx, standing.instance_id, own_group).then_some(own_group)
}

pub(crate) fn reconcile_instance_removal(ctx: &ReducerContext, character_guid: u64) {
    if let Some(group_id) = adopt_for_regrouped_member(ctx, character_guid) {
        reconcile_group_members(ctx, group_id);
        return;
    }
    let removals = ctx.db.game_instance_removal();
    let running = removals.character_guid().find(character_guid);
    let standing = instance_removal_standing(ctx, character_guid);
    if let (Some(row), Some(standing)) = (&running, standing) {
        if row.instance_id == standing.instance_id {
            return;
        }
    }
    if let Some(row) = running {
        InstanceRemoval::delete(ctx, row.character_guid);
    }
    if let Some(standing) = standing.filter(|standing| standing.in_world) {
        let due = crate::time::now(ctx).to_micros_since_unix_epoch() + INSTANCE_REMOVAL_MICROS;
        InstanceRemoval {
            scheduled_id: 0,
            scheduled_at: crate::time::schedule_time(
                ctx,
                Timestamp::from_micros_since_unix_epoch(due),
            ),
            character_guid,
            instance_id: standing.instance_id,
            group_id: standing.group_id,
        }
        .insert(ctx);
    }
}

fn reconcile_group_members(ctx: &ReducerContext, group_id: u64) {
    for member in crate::group::members_of(ctx, group_id) {
        reconcile_instance_removal(ctx, member.character_guid);
    }
}

#[reducer]
pub fn expire_instance_removal(
    ctx: &ReducerContext,
    removal: InstanceRemoval,
) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".to_string());
    }
    fire_instance_removal(ctx, &removal);
    Ok(())
}

pub(crate) fn fire_instance_removal(ctx: &ReducerContext, removal: &InstanceRemoval) {
    let removals = ctx.db.game_instance_removal();
    if removals
        .character_guid()
        .find(removal.character_guid)
        .is_none_or(|running| running.scheduled_id != removal.scheduled_id)
    {
        return;
    }
    InstanceRemoval::delete(ctx, removal.character_guid);
    if let Some(group_id) = adopt_for_regrouped_member(ctx, removal.character_guid) {
        reconcile_group_members(ctx, group_id);
        return;
    }
    match instance_removal_standing(ctx, removal.character_guid) {
        Some(standing) if standing.instance_id == removal.instance_id => {
            if let Err(refusal) = crate::character::recall_to_home(ctx, removal.character_guid) {
                log::warn!(
                    "Instance Removal of {} refused, rearmed: {refusal}",
                    removal.character_guid
                );
                reconcile_instance_removal(ctx, removal.character_guid);
            }
        }
        _ => reconcile_instance_removal(ctx, removal.character_guid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_entrances_are_outside_their_dungeon() {
        for dungeon in BOOTSTRAP_DUNGEONS {
            assert!(is_dungeon_map(dungeon.map_id));
            assert!(!is_dungeon_map(dungeon.entrance.0));
        }
    }

    fn bound(instance_id: u64, party_id: u64) -> Option<BoundRun> {
        Some(BoundRun {
            instance_id,
            party_id,
        })
    }

    #[test]
    fn a_character_without_a_run_starts_a_fresh_one() {
        assert_eq!(route_instance(None, None), InstanceRoute::Create);
    }

    #[test]
    fn a_solo_run_takes_its_holder_back() {
        assert_eq!(route_instance(bound(7, 0), None), InstanceRoute::Own(7));
    }

    #[test]
    fn the_groups_run_comes_before_a_members_solo_run() {
        assert_eq!(
            route_instance(bound(7, 0), Some(9)),
            InstanceRoute::Party(9)
        );
        assert_eq!(route_instance(None, Some(9)), InstanceRoute::Party(9));
    }

    #[test]
    fn a_binding_to_a_groups_run_starts_a_fresh_run_outside_that_group() {
        assert_eq!(route_instance(bound(7, 42), None), InstanceRoute::Create);
    }

    #[test]
    fn a_binding_to_a_former_groups_run_gives_way_to_the_new_groups_run() {
        assert_eq!(
            route_instance(bound(7, 42), Some(9)),
            InstanceRoute::Party(9)
        );
    }

    #[test]
    fn raid_admission_uses_its_map_capacity() {
        assert!(party_size_allows_entry(533, 40));
        assert!(!party_size_allows_entry(533, 41));
        assert!(party_size_allows_entry(309, 20));
        assert!(!party_size_allows_entry(309, 21));
        assert!(party_size_allows_entry(229, 10));
        assert!(!party_size_allows_entry(229, 11));
    }

    #[test]
    fn party_size_cap_admits_up_to_five_and_refuses_six() {
        for n in 1..=5 {
            assert!(
                party_size_allows_entry(36, n),
                "a party of {n} fits a 5-man"
            );
        }
        assert!(
            !party_size_allows_entry(36, 6),
            "a 6th player must be refused at the portal"
        );
    }

    #[test]
    fn occupancy_action_covers_stamp_clear_wait_and_both_reap_paths() {
        let now = 1_800_000_000_000_000i64;
        let t = empty_reap_micros(true);
        assert_eq!(t, INSTANCE_EMPTY_REAP_MICROS);
        assert_eq!(
            occupancy_action(true, now - 1, false, now, t),
            OccupancyAction::ClearStamp
        );
        assert_eq!(
            occupancy_action(true, 0, false, now, t),
            OccupancyAction::Wait
        );
        assert_eq!(
            occupancy_action(true, 0, true, now, t),
            OccupancyAction::Wait
        );
        assert_eq!(
            occupancy_action(false, 0, false, now, t),
            OccupancyAction::Stamp
        );
        let stamped = now - INSTANCE_EMPTY_REAP_MICROS;
        assert_eq!(
            occupancy_action(false, stamped + 1, false, now, t),
            OccupancyAction::Wait
        );
        assert_eq!(
            occupancy_action(false, stamped, false, now, t),
            OccupancyAction::Reap
        );
        assert_eq!(
            occupancy_action(false, 0, true, now, t),
            OccupancyAction::Reap
        );
        assert_eq!(
            occupancy_action(false, now - 1, true, now, t),
            OccupancyAction::Reap
        );
    }

    #[test]
    fn a_lease_only_database_holds_its_stub_rows_far_past_the_thirty_minute_run_timer() {
        let now = 1_800_000_000_000_000i64;
        let hosting = empty_reap_micros(true);
        let lease = empty_reap_micros(false);

        let entered = now - 40 * 60 * 1_000_000;
        assert_eq!(
            occupancy_action(false, entered, false, now, hosting),
            OccupancyAction::Reap
        );
        assert_eq!(
            occupancy_action(false, entered, false, now, lease),
            OccupancyAction::Wait,
            "a lease outlives a 30-minute run"
        );

        let longest_supported_run = 8 * 60 * 60 * 1_000_000i64;
        assert_eq!(
            occupancy_action(false, now - longest_supported_run, false, now, lease),
            OccupancyAction::Wait,
            "a lease outlives an 8-hour raid night"
        );

        let ancient = now - INSTANCE_LEASE_REAP_MICROS;
        assert_eq!(
            occupancy_action(false, ancient, false, now, lease),
            OccupancyAction::Reap
        );
        for t in [hosting, lease] {
            assert_eq!(
                occupancy_action(true, ancient, true, now, t),
                OccupancyAction::ClearStamp
            );
            assert_eq!(
                occupancy_action(true, 0, true, now, t),
                OccupancyAction::Wait
            );
        }
    }

    #[test]
    fn stranding_fallback_prefers_entrance_then_in_place_then_home() {
        assert_eq!(
            stranding_fallback(entrance_fallback(36), true),
            StrandingFallback::Entrance(0, -11080.0, 1520.0, 46.0, 0.0)
        );
        assert_eq!(
            stranding_fallback(None, false),
            StrandingFallback::InPlaceOpenWorld
        );
        assert_eq!(
            stranding_fallback(None, true),
            StrandingFallback::HearthstoneHome
        );
    }

    #[test]
    fn instance_removal_is_due_only_for_a_character_in_a_run_that_is_not_theirs() {
        assert!(instance_removal_due(36, 7, false, 0), "another's run");
        assert!(!instance_removal_due(36, 7, true, 0), "its own run");
        assert!(!instance_removal_due(36, 0, false, 0), "the open world");
        assert!(
            !instance_removal_due(0, 7, false, 0),
            "an instance of an open-world map is not a dungeon"
        );
        assert!(!instance_removal_due(36, 7, false, 1), "a GM");
    }

    fn run(party_id: u64, holder_guid: u64) -> GameInstance {
        GameInstance {
            instance_id: 7,
            map_id: 36,
            party_id,
            created_at: Timestamp::from_micros_since_unix_epoch(0),
            last_empty_at_micros: 0,
            reset_requested: false,
            holder_guid,
        }
    }

    #[test]
    fn a_binding_routes_into_or_resets_a_solo_run_only_for_its_holder() {
        assert!(!held_by_another(&run(0, 1), 1), "the holder");
        assert!(
            held_by_another(&run(0, 2), 1),
            "a binding the leaver carried to the survivor's run"
        );
        assert!(
            !held_by_another(&run(0, 0), 1),
            "a solo run without a holder routes whoever is bound to it"
        );
        assert!(
            !held_by_another(&run(42, 2), 1),
            "a Group's run has no holder"
        );
    }

    #[test]
    fn a_groups_run_is_its_members_and_a_solo_run_is_its_holders_and_their_groups() {
        assert!(!run_is_theirs(42, None, 1, None), "left the Group");
        assert!(
            !run_is_theirs(42, None, 1, Some(43)),
            "joining another Group does not cancel"
        );
        assert!(run_is_theirs(42, None, 1, Some(42)), "a member");
        assert!(run_is_theirs(0, Some((1, None)), 1, None), "the holder");
        assert!(
            !run_is_theirs(0, Some((2, None)), 1, None),
            "the member who left a disbanded Group's run, now the survivor's"
        );
        assert!(
            run_is_theirs(0, Some((2, Some(42))), 1, Some(42)),
            "a member of the holder's Group"
        );
        assert!(
            !run_is_theirs(0, Some((2, Some(42))), 1, Some(43)),
            "a member of another Group"
        );
        assert!(
            run_is_theirs(0, None, 1, None),
            "a solo run nobody holds turns nobody out"
        );
    }

    #[test]
    fn only_a_raid_member_or_a_gm_enters_a_raid_map() {
        assert!(raid_refuses(false, false, 0), "solo or party");
        assert!(!raid_refuses(false, true, 0), "raid member");
        assert!(!raid_refuses(false, false, 1), "a GM");
        assert!(!raid_refuses(true, false, 0), "already inside");
    }

    #[test]
    fn manual_reset_excludes_raids_but_includes_upper_blackrock_spire() {
        for map in [36, 229, 429] {
            assert!(resettable_map(map));
        }
        for map in [0, 1, 30, 249, 309, 409, 469, 509, 531, 533] {
            assert!(!resettable_map(map));
        }
    }

    #[test]
    fn instance_population_and_wave_allocators_never_share_a_low() {
        let mut lows = vec![1u64, 3, 150_000, 0x80_0001];
        let mut slot = 0;
        let population = next_population_low(&mut slot, |low| lows.contains(&low));
        assert_eq!(population, Some(0x80_0002));
        lows.push(0x80_0002);
        assert_eq!(
            crate::creatures::population::next_spawn_low(lows.into_iter()),
            150_001
        );
    }

    #[test]
    fn instance_population_lows_run_out_at_the_top_of_the_band() {
        let mut slot = 0x7F_FFFE;
        assert_eq!(next_population_low(&mut slot, |_| false), Some(0xFF_FFFF));
        assert_eq!(next_population_low(&mut slot, |_| false), None);
    }
}
