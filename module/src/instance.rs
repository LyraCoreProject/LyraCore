//! Before changing Instance allocation or pool routing, read docs/instance-lifecycle.md.

use std::collections::{HashMap, HashSet};

use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{
    game_config, game_corpse, game_creature_move_schedule, game_creature_spawn,
    game_creature_template, game_creature_waypoint, game_encounter_spawn, game_gameobject,
    game_gameobject_template, game_group, game_group_member, game_world_entity,
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

// Bit 23 separates Instance copies; respawning copies still reserve their sequence.
pub(crate) const INSTANCE_POP_LOW_BAND: u64 = 0x80_0000;

// Bit 46 reserves gameobject Instance copies below pool bit 47.
pub(crate) const GO_COPY_BAND: u64 = 1 << 46;

const GO_COPY_TYPES: [u8; 4] = [
    crate::gameobject::go_type::DOOR,
    crate::gameobject::go_type::BUTTON,
    crate::gameobject::go_type::CHEST,
    crate::gameobject::go_type::GOOBER,
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
}

#[table(accessor = game_package_instance)]
pub struct PackageInstance {
    #[primary_key]
    pub instance_id: u64,
    pub package_name: String,
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

crate::character_owned!(delete, fn sweep_delete_game_instance_removal(ctx, character_guid) {
    ctx.db.game_instance_removal().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_instance_removal());

#[derive(Debug, PartialEq)]
pub(crate) enum InstanceRoute {
    Own(u64),
    Party(u64),
    Create,
}

pub(crate) fn route_instance(
    own_binding_live: Option<u64>,
    party_live: Option<u64>,
) -> InstanceRoute {
    match (own_binding_live, party_live) {
        (_, Some(id)) => InstanceRoute::Party(id),
        (Some(id), None) => InstanceRoute::Own(id),
        (None, None) => InstanceRoute::Create,
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

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn reset_eligible(party_id: u64, caller_is_leader: bool, occupied: bool) -> bool {
    !occupied && (party_id == 0 || caller_is_leader)
}

pub(crate) fn instance_removal_due(
    map_id: u32,
    instance_id: u64,
    instance_party: u64,
    own_group: Option<u64>,
    gm_level: u8,
) -> bool {
    instance_id != 0
        && is_dungeon_map(map_id)
        && instance_party != 0
        && own_group != Some(instance_party)
        && gm_level == 0
}

fn live_instance_for(ctx: &ReducerContext, instance_id: u64, map_id: u32) -> bool {
    if is_package_instance(ctx, instance_id) {
        return false;
    }
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .map(|i| i.map_id == map_id && !i.reset_requested)
        .unwrap_or(false)
}

fn bind_character(ctx: &ReducerContext, character_guid: u64, instance_id: u64, map_id: u32) {
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
    let own_live = match own_binding {
        Some(b) if live_instance_for(ctx, b.instance_id, target_map) => Some(b.instance_id),
        Some(b) => {
            bindings.id().delete(b.id);
            None
        }
        None => None,
    };

    let party_live = group.as_ref().and_then(|g| {
        ctx.db
            .game_instance()
            .by_party()
            .filter(&g.group_id)
            .find(|i| {
                i.map_id == target_map
                    && !i.reset_requested
                    && !is_package_instance(ctx, i.instance_id)
            })
            .map(|i| i.instance_id)
    });

    let party_id = group.as_ref().map(|g| g.group_id).unwrap_or(0);
    match route_instance(own_live, party_live) {
        InstanceRoute::Own(id) => {
            adopt_instance_for_party(ctx, id, party_id);
            Ok(id)
        }
        InstanceRoute::Party(id) => {
            bind_character(ctx, character_guid, id, target_map);
            Ok(id)
        }
        InstanceRoute::Create => {
            let id = create_instance(ctx, target_map, party_id)?;
            bind_character(ctx, character_guid, id, target_map);
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
    if is_package_instance(ctx, expected_instance) {
        return Err("Package instance requires Package admission".to_string());
    }
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
    let instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(expected_instance)
        .filter(|instance| instance.map_id == target_map && !instance.reset_requested)
        .ok_or("expected party instance is unavailable")?;
    if instance.party_id != group.group_id && !instance_is_ownerless(ctx, instance.party_id) {
        return Err("expected instance belongs to another party".to_string());
    }

    if instance.party_id != group.group_id {
        adopt_instance_for_party(ctx, expected_instance, group.group_id);
    }
    bind_character(ctx, character_guid, expected_instance, target_map);
    Ok(expected_instance)
}

fn instance_is_ownerless(ctx: &ReducerContext, party_id: u64) -> bool {
    party_id == 0 || ctx.db.game_group().group_id().find(party_id).is_none()
}

fn is_package_instance(ctx: &ReducerContext, instance_id: u64) -> bool {
    ctx.db
        .game_package_instance()
        .instance_id()
        .find(instance_id)
        .is_some()
}

const PACKAGE_LOBBY_CAPACITY: usize = 200;

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn create_lobby(
    ctx: &ReducerContext,
    package_name: &str,
    map_id: u32,
) -> Result<u64, String> {
    if package_name.is_empty() || !is_dungeon_map(map_id) || !hosts_instance_populations(ctx) {
        return Err("Package lobby requires a Package name and a hosted dungeon map".into());
    }
    if let Some(instance) = ctx
        .db
        .game_instance()
        .by_map()
        .filter(map_id)
        .find(|instance| {
            instance.party_id == 0
                && !instance.reset_requested
                && ctx
                    .db
                    .game_package_instance()
                    .instance_id()
                    .find(instance.instance_id)
                    .is_some_and(|owner| owner.package_name == package_name)
        })
    {
        return Ok(instance.instance_id);
    }
    let instance_id = insert_instance(ctx, 0, map_id, 0, false)?;
    ctx.db.game_package_instance().insert(PackageInstance {
        instance_id,
        package_name: package_name.to_string(),
    });
    Ok(instance_id)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn create_empty(
    ctx: &ReducerContext,
    package_name: &str,
    map_id: u32,
    party_id: u64,
) -> Result<u64, String> {
    if package_name.is_empty() || !is_dungeon_map(map_id) {
        return Err("Package instance needs a Package name and a dungeon map".to_string());
    }
    if !hosts_instance_populations(ctx) {
        return Err("Package instances require their Instance Pool".to_string());
    }
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(party_id)
        .ok_or("Package instance needs an existing Party")?;
    let members = crate::group::members_of(ctx, party_id);
    if group.group_type != 0
        || members.is_empty()
        || members.len() > crate::group::GROUP_MAX_MEMBERS
    {
        return Err("Package instance needs a Party of at most five Characters".to_string());
    }
    let instance_id = insert_instance(ctx, 0, map_id, party_id, false)?;
    ctx.db.game_package_instance().insert(PackageInstance {
        instance_id,
        package_name: package_name.to_string(),
    });
    Ok(instance_id)
}

fn owned_instance(
    ctx: &ReducerContext,
    package_name: &str,
    instance_id: u64,
) -> Result<GameInstance, String> {
    let owner = ctx
        .db
        .game_package_instance()
        .instance_id()
        .find(instance_id)
        .ok_or("Instance is not owned by a Package")?;
    if owner.package_name != package_name {
        return Err("Instance belongs to another Package".to_string());
    }
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .ok_or_else(|| "Package instance lease is missing".to_string())
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn enter_package_instance(
    ctx: &ReducerContext,
    package_name: &str,
    character_guid: u64,
    instance_id: u64,
    position: (f32, f32, f32, f32),
) -> Result<(), String> {
    let instance = owned_instance(ctx, package_name, instance_id)?;
    if !crate::package_combat::relocation_allowed(ctx, character_guid, instance.map_id, instance_id)
    {
        return Err("Leave the Package match before entering another instance".into());
    }
    if instance.reset_requested {
        return Err("Package instance is closing".to_string());
    }
    if instance.party_id != 0 {
        let group = crate::group::group_of(ctx, character_guid)
            .ok_or("Character has no Party for the Package instance")?;
        if group.group_id != instance.party_id {
            return Err("Package instance belongs to another Party".to_string());
        }
        let party = ctx
            .db
            .game_group()
            .group_id()
            .find(group.group_id)
            .ok_or("Package instance Party is unavailable")?;
        if party.group_type != 0
            || !party_size_allows_entry(
                instance.map_id,
                crate::group::members_of(ctx, group.group_id).len(),
            )
        {
            return Err("Package instance requires a Party within map capacity".to_string());
        }
    }
    let character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or("Character is unavailable or in Transfer")?;
    if !character.online {
        use crate::package_account::game_package_account;
        if ctx
            .db
            .game_package_account()
            .account_id()
            .find(character.account_id)
            .is_none()
        {
            return Err("Session-less admission requires a Package-owned Account".to_string());
        }
        crate::sessionless::transfer_authority_gate(ctx, character_guid)
            .map_err(|refusal| refusal.to_string())?;
    }
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    if !hosts_instance_populations(ctx) || entity.map_id != instance.map_id {
        return Err(
            "Package instance admission requires the same map on its Instance Pool".to_string(),
        );
    }
    if instance.party_id == 0 && entity.instance_id != instance_id {
        let occupants = ctx
            .db
            .game_world_entity()
            .by_cell()
            .filter((instance.map_id, instance_id))
            .filter(|entity| entity.is_player())
            .take(PACKAGE_LOBBY_CAPACITY)
            .count();
        if occupants >= PACKAGE_LOBBY_CAPACITY {
            return Err("Package lobby is at its 200-Character capacity".into());
        }
    }
    if entity.dead || entity.health == 0 || !entity.is_player() {
        return Err("Package instance admission requires a living Character".to_string());
    }
    if crate::taxi::movement_is_suppressed(ctx, character_guid)
        || crate::spell::is_action_blocked(ctx, character_guid)
        || crate::spell::pending_cast(ctx, character_guid).is_some()
    {
        return Err("Character cannot enter the Package instance now".to_string());
    }
    let (x, y, z, o) = position;
    if ![x, y, z, o].into_iter().all(f32::is_finite) {
        return Err("Package instance position must be finite".to_string());
    }
    crate::combat::disengage(ctx, character_guid);
    crate::creatures::despawn_pets(ctx, character_guid);
    crate::world::teleport_player(
        ctx,
        character_guid,
        instance.map_id,
        instance_id,
        x,
        y,
        z,
        o,
    );
    Ok(())
}

// Closing blocks admission immediately; live Characters and Transfer claims delay teardown.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn close_package_instance(
    ctx: &ReducerContext,
    package_name: &str,
    instance_id: u64,
) -> Result<bool, String> {
    if ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_none()
        && !is_package_instance(ctx, instance_id)
    {
        return Ok(true);
    }
    let mut instance = owned_instance(ctx, package_name, instance_id)?;
    instance.reset_requested = true;
    ctx.db.game_instance().instance_id().update(instance);
    teardown_instance(ctx, instance_id);
    Ok(ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_none())
}

fn adopt_instance_for_party(ctx: &ReducerContext, instance_id: u64, party_id: u64) {
    if party_id == 0 || is_package_instance(ctx, instance_id) {
        return;
    }
    let instances = ctx.db.game_instance();
    if let Some(mut inst) = instances.instance_id().find(instance_id) {
        if instance_is_ownerless(ctx, inst.party_id) {
            inst.party_id = party_id;
            instances.instance_id().update(inst);
            log::info!("instance {instance_id} adopted by party {party_id}");
        }
    }
}

pub(crate) fn hosts_instance_populations(ctx: &ReducerContext) -> bool {
    ctx.db
        .game_config()
        .id()
        .find(0)
        .map(|c| c.hosts_instances)
        .unwrap_or(true)
}

pub(crate) fn create_instance(
    ctx: &ReducerContext,
    map_id: u32,
    party_id: u64,
) -> Result<u64, String> {
    create_instance_with_id(ctx, 0, map_id, party_id)
}

pub(crate) fn create_instance_with_id(
    ctx: &ReducerContext,
    instance_id: u64,
    map_id: u32,
    party_id: u64,
) -> Result<u64, String> {
    insert_instance(ctx, instance_id, map_id, party_id, true)
}

fn insert_instance(
    ctx: &ReducerContext,
    instance_id: u64,
    map_id: u32,
    party_id: u64,
    copy_population: bool,
) -> Result<u64, String> {
    let inst = ctx.db.game_instance().insert(GameInstance {
        instance_id,
        map_id,
        party_id,
        created_at: ctx.timestamp,
        last_empty_at_micros: 0,
        reset_requested: false,
    });
    let instance_id = inst.instance_id;

    if !copy_population {
        return Ok(instance_id);
    }
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
    let spawns: Vec<crate::creatures::CreatureSpawn> = ctx
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
        *slot += 1;
        if *slot >= INSTANCE_POP_LOW_BAND {
            return Err(format!(
                "instance population low exhausted for entry {entry}"
            ));
        }
        copy.guid = crate::encounter::wave_guid(entry, INSTANCE_POP_LOW_BAND | *slot);
        copy.life_seq = 1;
        copy.respawn_at = crate::creatures::timer_never(ctx);
        copy.despawn_at = crate::creatures::timer_never(ctx);
        let entity =
            crate::creatures::build_creature_entity(&copy, &tmpl, ctx.random(), instance_id);
        let copied_guid = copy.guid;
        ctx.db.game_creature_spawn().insert(copy);
        copy_spawn_waypoints(ctx, spawn.guid, copied_guid, &mut next_waypoint_id)?;
        crate::creatures::insert_creature_entity(ctx, entity);
        spawned += 1;
    }

    let gos = ctx.db.game_gameobject();
    let mut next_go_seq = gos
        .iter()
        .filter(|g| {
            g.guid >> 48 == 0xF110
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
        })
        .collect();
    let mut copied = 0u32;
    for src in &sources {
        let copy = gos.insert(crate::gameobject::GameObject {
            guid: (0xF110u64 << 48) | GO_COPY_BAND | next_go_seq,
            template_entry: src.template_entry,
            map_id: src.map_id,
            x: src.x,
            y: src.y,
            z: src.z,
            orientation: src.orientation,
            state: src.state,
            created_at: ctx.timestamp,
            respawn_at_micros: 0,
            instance_id,
            grid_x: lyracore_shared::spatial::grid_cell(src.x, src.y).0,
            grid_y: lyracore_shared::spatial::grid_cell(src.x, src.y).1,
            cell: lyracore_shared::spatial::cell_id_at(src.x, src.y),
            rotation_0: src.rotation_0,
            rotation_1: src.rotation_1,
            rotation_2: src.rotation_2,
            rotation_3: src.rotation_3,
        });
        crate::go_collider::register(ctx, &copy);
        next_go_seq += 1;
        copied += 1;
    }

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
        if is_package_instance(ctx, instance_id) {
            return Err("Package instance requires Package mirroring".to_string());
        }
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
            ctx.db.game_instance().instance_id().update(existing);
            return Ok(());
        }
        return Err(format!(
            "instance {instance_id} belongs to party {}, not party {party_id}",
            existing.party_id
        ));
    }
    let id = create_instance_with_id(ctx, instance_id, map_id, party_id)?;
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
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let instances: Vec<GameInstance> = ctx.db.game_instance().iter().collect();
    if instances.is_empty() {
        return;
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let reap_after = empty_reap_micros(hosts_instance_populations(ctx));
    let occupied = occupied_instances(ctx);
    let mut reaped = 0usize;
    for mut inst in instances {
        if inst.party_id == 0 && !inst.reset_requested && is_package_instance(ctx, inst.instance_id)
        {
            continue;
        }
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
    if crate::transfer::in_transit_instances(ctx)
        .map_or(true, |claimed| claimed.contains(&instance_id))
    {
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
    clear_instance_spell_state(ctx, instance_id, &doomed);
    for guid in &doomed {
        crate::creatures::despawn_creature_entity(ctx, *guid);
    }

    let spawns = ctx.db.game_creature_spawn();
    let copies: Vec<_> = spawns.by_instance().filter(instance_id).collect();
    for spawn in copies {
        remove_spawn_waypoints(ctx, spawn.guid);
        spawns.guid().delete(spawn.guid);
    }
    crate::creatures::population::clear_instance(ctx, instance_id);

    let corpses = ctx.db.game_corpse();
    let dead_bodies: Vec<u64> = corpses
        .iter()
        .filter(|c| c.instance_id == instance_id)
        .map(|c| c.guid)
        .collect();
    for guid in dead_bodies {
        crate::loot::reap_corpse_loot_family(ctx, guid);
        corpses.guid().delete(guid);
    }

    let gos = ctx.db.game_gameobject();
    let copies: Vec<u64> = gos
        .iter()
        .filter(|g| g.instance_id == instance_id)
        .map(|g| g.guid)
        .collect();
    for guid in &copies {
        use crate::gameobject::{game_gameobject_trap_cooldown, game_gameobject_unlocked};
        crate::loot::reap_corpse_loot_family(ctx, *guid);
        ctx.db
            .game_gameobject_trap_cooldown()
            .go_guid()
            .delete(*guid);
        ctx.db.game_gameobject_unlocked().go_guid().delete(*guid);
        crate::go_collider::remove(ctx, *guid);
        gos.guid().delete(guid);
    }

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
             copies) — row + bindings kept as a cross-shard lease",
            doomed.len(),
            copies.len()
        );
        return;
    }
    let bindings = ctx.db.game_instance_binding();
    let stale: Vec<u64> = bindings
        .by_instance()
        .filter(&instance_id)
        .map(|b| b.id)
        .collect();
    for id in stale {
        bindings.id().delete(id);
    }
    crate::package_combat::clear_instance(ctx, instance_id);
    ctx.db
        .game_package_instance()
        .instance_id()
        .delete(instance_id);
    ctx.db.game_instance().instance_id().delete(instance_id);
    log::info!(
        "teardown_instance: instance {instance_id} reaped ({} entities, {} GO copies)",
        doomed.len(),
        copies.len()
    );
}

fn clear_instance_spell_state(ctx: &ReducerContext, instance_id: u64, doomed: &[u64]) {
    use crate::game_ranged_impact_schedule;
    use crate::spell::{
        game_aura, game_dynamic_object, game_ground_area, game_pending_cast,
        game_pending_spell_impact, game_school_lockout, game_spell_cd, game_spell_cooldown,
    };

    let doomed: HashSet<u64> = doomed.iter().copied().collect();
    let mut shed: HashMap<u64, HashMap<u64, crate::spell::Aura>> = HashMap::new();
    let auras = ctx.db.game_aura();
    for &guid in &doomed {
        ctx.db.game_spell_cooldown().caster_guid().delete(guid);
        for aura in auras
            .by_target()
            .filter(guid)
            .chain(auras.by_caster().filter(guid))
        {
            shed.entry(aura.target_guid)
                .or_default()
                .insert(aura.id, aura);
        }
        for cast in ctx
            .db
            .game_pending_cast()
            .by_caster()
            .filter(guid)
            .collect::<Vec<_>>()
        {
            crate::spell::cancel_cast_attempt(ctx, guid, cast.scheduled_id);
        }
        for cooldown in ctx
            .db
            .game_spell_cd()
            .by_caster()
            .filter(guid)
            .collect::<Vec<_>>()
        {
            ctx.db.game_spell_cd().id().delete(cooldown.id);
        }
        for lockout in ctx
            .db
            .game_school_lockout()
            .by_caster()
            .filter(guid)
            .collect::<Vec<_>>()
        {
            ctx.db.game_school_lockout().id().delete(lockout.id);
        }
    }
    for (guid, auras) in shed {
        crate::spell::shed_auras(ctx, guid, auras.into_values().collect());
    }
    let impacts = ctx.db.game_pending_spell_impact();
    for impact in impacts
        .iter()
        .filter(|impact| {
            doomed.contains(&impact.caster_guid) || doomed.contains(&impact.target_guid)
        })
        .collect::<Vec<_>>()
    {
        impacts.scheduled_id().delete(impact.scheduled_id);
    }
    let ranged = ctx.db.game_ranged_impact_schedule();
    for impact in ranged
        .iter()
        .filter(|impact| {
            doomed.contains(&impact.attacker_guid) || doomed.contains(&impact.target_guid)
        })
        .collect::<Vec<_>>()
    {
        ranged.scheduled_id().delete(impact.scheduled_id);
    }
    let areas = ctx.db.game_ground_area();
    for area in areas
        .iter()
        .filter(|area| area.instance_id == instance_id)
        .collect::<Vec<_>>()
    {
        ctx.db
            .game_dynamic_object()
            .guid()
            .delete(crate::spell::cast::dynamic_object_guid(area.id));
        areas.id().delete(area.id);
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_defense_create_instance(
    ctx: &ReducerContext,
    package_name: String,
    map_id: u32,
    party_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    create_empty(ctx, &package_name, map_id, party_id).map(|_| ())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_defense_create_lobby(
    ctx: &ReducerContext,
    package_name: String,
    map_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    create_lobby(ctx, &package_name, map_id).map(|_| ())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_defense_fill_lobby(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let instance = owned_instance(ctx, "fixture.defense", instance_id)?;
    if instance.party_id != 0 {
        return Err("Fixture needs a shared lobby".into());
    }
    for index in 0..PACKAGE_LOBBY_CAPACITY {
        let mut character =
            crate::helpers::character_by_guid(ctx, 1).ok_or("Fixture Character is unavailable")?;
        character.guid = 509_929_000 + index as u64;
        character.map_id = instance.map_id;
        character.pending_instance_id = instance_id;
        let body =
            crate::creatures::build_player_entity(ctx, &character, spacetimedb::Identity::ZERO);
        ctx.db.game_world_entity().guid().delete(character.guid);
        body.insert(ctx);
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_defense_enter_instance(
    ctx: &ReducerContext,
    package_name: String,
    character_guid: u64,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    enter_package_instance(
        ctx,
        &package_name,
        character_guid,
        instance_id,
        (10.0, 20.0, 30.0, 0.0),
    )
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_defense_close_instance(
    ctx: &ReducerContext,
    package_name: String,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    close_package_instance(ctx, &package_name, instance_id).map(|_| ())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_defense_spawn_wave(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let instance = owned_instance(ctx, "fixture.defense", instance_id)?;
    crate::encounter::spawn_wave(
        ctx,
        instance_id,
        1,
        instance.map_id,
        &[51000],
        10.0,
        20.0,
        30.0,
        0.0,
    );
    Ok(())
}

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn apply_reset_instances(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<u32, String> {
    let occupied = occupied_instances(ctx);
    let bindings: Vec<GameInstanceBinding> = ctx
        .db
        .game_instance_binding()
        .by_character()
        .filter(&character_guid)
        .collect();
    let mut flagged = 0u32;
    for b in bindings {
        let Some(mut inst) = ctx.db.game_instance().instance_id().find(b.instance_id) else {
            continue;
        };
        let caller_is_leader = ctx
            .db
            .game_group()
            .group_id()
            .find(inst.party_id)
            .map(|g| g.leader_guid == character_guid)
            .unwrap_or(false);
        if !reset_eligible(
            inst.party_id,
            caller_is_leader,
            occupied.contains(&inst.instance_id),
        ) {
            continue;
        }
        if !inst.reset_requested {
            inst.reset_requested = true;
            ctx.db.game_instance().instance_id().update(inst);
            flagged += 1;
        }
    }
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
    let instance_party = ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .map_or(0, |instance| instance.party_id);
    let own_group = crate::group::group_of(ctx, character_guid).map(|member| member.group_id);
    instance_removal_due(
        map_id,
        instance_id,
        instance_party,
        own_group,
        character.gm_level,
    )
    .then_some(RemovalStanding {
        instance_id,
        group_id: instance_party,
        in_world: entity.is_some(),
    })
}

fn adopt_for_regrouped_member(ctx: &ReducerContext, character_guid: u64) -> Option<u64> {
    let standing = instance_removal_standing(ctx, character_guid)?;
    if is_package_instance(ctx, standing.instance_id) {
        return None;
    }
    let own_group = crate::group::group_of(ctx, character_guid)?.group_id;
    if !instance_is_ownerless(ctx, standing.group_id) {
        return None;
    }
    adopt_instance_for_party(ctx, standing.instance_id, own_group);
    Some(own_group)
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
        removals.scheduled_id().delete(row.scheduled_id);
    }
    if let Some(standing) = standing.filter(|standing| standing.in_world) {
        let due = ctx.timestamp.to_micros_since_unix_epoch() + INSTANCE_REMOVAL_MICROS;
        removals.insert(InstanceRemoval {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(due)),
            character_guid,
            instance_id: standing.instance_id,
            group_id: standing.group_id,
        });
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
    if ctx.sender() != ctx.database_identity() {
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
    removals.scheduled_id().delete(removal.scheduled_id);
    if let Some(group_id) = adopt_for_regrouped_member(ctx, removal.character_guid) {
        reconcile_group_members(ctx, group_id);
        return;
    }
    match instance_removal_standing(ctx, removal.character_guid) {
        Some(standing) if standing.instance_id == removal.instance_id => {
            crate::world::recall_to_home(ctx, removal.character_guid);
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

    #[test]
    fn route_prefers_the_party_then_the_personal_binding_then_create() {
        assert_eq!(route_instance(Some(7), Some(9)), InstanceRoute::Party(9));
        assert_eq!(route_instance(Some(7), None), InstanceRoute::Own(7));
        assert_eq!(route_instance(None, Some(9)), InstanceRoute::Party(9));
        assert_eq!(route_instance(None, None), InstanceRoute::Create);
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
        assert_eq!(
            hosting, INSTANCE_EMPTY_REAP_MICROS,
            "hosting databases are UNCHANGED"
        );
        assert_eq!(lease, INSTANCE_LEASE_REAP_MICROS);
        assert!(
            lease > hosting,
            "a lease must outlive the run it is a receipt for, or the world shard forgets which \
             instance the party is in while they are still in it"
        );

        let entered = now - 40 * 60 * 1_000_000;
        assert_eq!(
            occupancy_action(false, entered, false, now, hosting),
            OccupancyAction::Reap,
            "on the ordinary 30-minute timer the world shard reaps the lease MID-RUN — the \
             latecomer then resolves Create and lands in a fresh, empty dungeon"
        );
        assert_eq!(
            occupancy_action(false, entered, false, now, lease),
            OccupancyAction::Wait,
            "with the lease timer the world shard still knows which instance that party is in"
        );

        let longest_supported_run = 8 * 60 * 60 * 1_000_000i64;
        assert!(
            lease > longest_supported_run,
            "the lease timer ({lease}µs) must outlast the LONGEST run it is a receipt for, not \
             merely the 30-minute hosting timer: it starts counting when the party leaves the world \
             shard, i.e. at minute ~0 of the run. Below this bound a long dungeon or raid forks its \
             own party exactly as the 30-minute timer did."
        );
        assert_eq!(
            occupancy_action(false, now - longest_supported_run, false, now, lease),
            OccupancyAction::Wait,
            "an 8-hour raid night must still find its lease on the world shard"
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
    fn instance_removal_is_due_only_for_a_non_member_in_a_group_instance() {
        assert!(instance_removal_due(36, 7, 42, None, 0), "left the Group");
        assert!(
            instance_removal_due(36, 7, 42, Some(43), 0),
            "joining another Group does not cancel"
        );
        assert!(!instance_removal_due(36, 7, 42, Some(42), 0), "a member");
        assert!(
            !instance_removal_due(36, 7, 0, None, 0),
            "a solo instance has no Group to leave"
        );
        assert!(!instance_removal_due(36, 0, 42, None, 0), "the open world");
        assert!(
            !instance_removal_due(0, 7, 42, None, 0),
            "an instance of an open-world map is not a dungeon"
        );
        assert!(!instance_removal_due(36, 7, 42, None, 1), "a GM");
    }

    #[test]
    fn reset_eligibility_requires_unoccupied_and_leadership_for_party_instances() {
        assert!(reset_eligible(0, false, false));
        assert!(!reset_eligible(0, false, true));
        assert!(reset_eligible(42, true, false));
        assert!(
            !reset_eligible(42, false, false),
            "a non-leader member cannot reset"
        );
        assert!(
            !reset_eligible(42, true, true),
            "even the leader cannot reset an occupied instance"
        );
    }

    #[test]
    fn instance_population_low_band_is_disjoint_from_spawn_row_allocators() {
        let realistic_spawn_lows = [1u64, 3, 40_000, 150_000];
        let wave_next = realistic_spawn_lows.iter().max().unwrap() + 1;
        assert!(
            wave_next & INSTANCE_POP_LOW_BAND == 0,
            "wave allocation stays below the band"
        );
        const { assert!(INSTANCE_POP_LOW_BAND == 0x80_0000) };
        let pop_low = INSTANCE_POP_LOW_BAND | 1;
        assert!(pop_low & INSTANCE_POP_LOW_BAND != 0);
        assert_ne!(pop_low, wave_next);
        let g = crate::encounter::wave_guid(636, pop_low);
        assert_eq!(g >> 48, 0xF130);
        assert_eq!((g >> 24) & 0xFF_FFFF, 636);
        assert_eq!(g & 0xFF_FFFF, pop_low);
    }

    #[test]
    fn go_copy_band_is_disjoint_from_pool_static_and_debug_namespaces() {
        let copy_guid = (0xF110u64 << 48) | GO_COPY_BAND | 1;
        const { assert!(GO_COPY_BAND < crate::gameobject::POOL_TAG) };
        assert_eq!(crate::gameobject::pool_point_id_of(copy_guid), None);
        const { assert!(GO_COPY_BAND > 0xFF_FFFF) };
        assert_ne!(copy_guid, crate::gameobject::pool_point_guid(1));
    }
}
