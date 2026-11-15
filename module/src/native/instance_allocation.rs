use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

use super::directory::{self, game_map_owner};
use crate::peer::{is_peer, role, ShardRole};

#[table(accessor = game_instance_host, index(accessor = by_map, btree(columns = [map_id])))]
pub struct InstanceHost {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub map_id: u32,
    pub world: Identity,
    pub accepting: bool,
    pub last_selected: u64,
}

#[table(accessor = game_instance_placement, index(accessor = by_owner, btree(columns = [owner])))]
pub struct InstancePlacement {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub owner: u64,
    pub grouped: bool,
    pub map_id: u32,
    pub world: Identity,
}

crate::outside_table_catalog!(game_instance_host, game_instance_placement);

#[reducer]
pub fn operator_set_instance_host(
    ctx: &ReducerContext,
    map_id: u32,
    world: Identity,
    accepting: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !crate::instance::is_dungeon_map(map_id)
        || ctx.db.game_map_owner().map_id().find(map_id).is_none()
    {
        return Err("Instance hosting requires a dungeon with a default map owner".into());
    }
    match role(ctx) {
        Some(ShardRole::Realm { .. }) if is_peer(ctx, world) => {}
        Some(ShardRole::World { .. })
            if world == ctx.database_identity() || is_peer(ctx, world) => {}
        _ => return Err("Instance host must be a World Shard of this Realm".into()),
    }
    let hosts = ctx.db.game_instance_host();
    if let Some(mut host) = hosts
        .by_map()
        .filter(map_id)
        .find(|host| host.world == world)
    {
        host.accepting = accepting;
        hosts.id().update(host);
    } else {
        hosts.insert(InstanceHost {
            id: 0,
            map_id,
            world,
            accepting,
            last_selected: 0,
        });
    }
    if crate::peer::realm_shard(ctx).is_some() {
        directory::report_characters(ctx);
    }
    Ok(())
}

#[reducer]
pub fn operator_adopt_instance_placement(
    ctx: &ReducerContext,
    group_id: u64,
    map_id: u32,
    world: Identity,
) -> Result<(), String> {
    use crate::group::game_group;
    crate::helpers::require_operator(ctx)?;
    if !crate::instance::is_dungeon_map(map_id)
        || crate::peer::world_shard(ctx).is_none()
        || !hosts(ctx, map_id, world)
        || ctx.db.game_group().group_id().find(group_id).is_none()
    {
        return Err(
            "Instance placement requires a Realm Group and a configured dungeon host".into(),
        );
    }
    let placements = ctx.db.game_instance_placement();
    if let Some(old) = placements
        .by_owner()
        .filter(group_id)
        .find(|placement| placement.grouped && placement.map_id == map_id)
    {
        return if old.world == world {
            Ok(())
        } else {
            Err("Instance placement cannot move an existing run".into())
        };
    }
    placements.insert(InstancePlacement {
        id: 0,
        owner: group_id,
        grouped: true,
        map_id,
        world,
    });
    Ok(())
}

pub(crate) fn configured(ctx: &ReducerContext, map: u32) -> bool {
    ctx.db
        .game_instance_host()
        .by_map()
        .filter(map)
        .next()
        .is_some()
}

pub(crate) fn hosts(ctx: &ReducerContext, map: u32, world: Identity) -> bool {
    ctx.db
        .game_map_owner()
        .map_id()
        .find(map)
        .is_some_and(|owner| owner.world == world)
        || ctx
            .db
            .game_instance_host()
            .by_map()
            .filter(map)
            .any(|host| host.world == world)
}

pub(super) fn worlds(ctx: &ReducerContext) -> impl Iterator<Item = Identity> {
    ctx.db
        .game_instance_host()
        .iter()
        .map(|host| host.world)
        .collect::<Vec<_>>()
        .into_iter()
}

pub(super) fn select(
    ctx: &ReducerContext,
    guid: u64,
    map: u32,
    binding_world: Option<Identity>,
) -> Result<Identity, String> {
    if !configured(ctx, map) {
        return ctx
            .db
            .game_map_owner()
            .map_id()
            .find(map)
            .map(|owner| owner.world)
            .filter(|world| is_peer(ctx, *world))
            .ok_or("dungeon has no World Shard".into());
    }
    let group = crate::group::group_of(ctx, guid);
    let grouped = group.is_some();
    let owner = group.map_or(guid, |member| member.group_id);
    let placements = ctx.db.game_instance_placement();
    if let Some(placement) = placements
        .by_owner()
        .filter(owner)
        .find(|placement| placement.map_id == map && placement.grouped == grouped)
    {
        let world = if grouped {
            placement.world
        } else {
            binding_world.unwrap_or(placement.world)
        };
        if !hosts(ctx, map, world) || !is_peer(ctx, world) {
            return Err("assigned Instance Shard is not configured as a Peer Shard".into());
        }
        if world != placement.world {
            placements
                .id()
                .update(InstancePlacement { world, ..placement });
        }
        return Ok(world);
    }
    let world = if let Some(world) = binding_world {
        if !hosts(ctx, map, world) || !is_peer(ctx, world) {
            return Err("bound Instance Shard is not configured".into());
        }
        world
    } else {
        let mut host = ctx
            .db
            .game_instance_host()
            .by_map()
            .filter(map)
            .filter(|host| host.accepting && is_peer(ctx, host.world))
            .min_by_key(|host| (host.last_selected, host.id))
            .ok_or("no Instance Shard accepts new placements")?;
        host.last_selected = directory::revision(ctx);
        let world = host.world;
        ctx.db.game_instance_host().id().update(host);
        world
    };
    placements.insert(InstancePlacement {
        id: 0,
        owner,
        grouped,
        map_id: map,
        world,
    });
    Ok(world)
}

pub(super) fn forget(ctx: &ReducerContext, owner: u64, grouped: bool) {
    let placements = ctx.db.game_instance_placement();
    let ids: Vec<_> = placements
        .by_owner()
        .filter(owner)
        .filter(|placement| placement.grouped == grouped)
        .map(|placement| placement.id)
        .collect();
    for id in ids {
        placements.id().delete(id);
    }
}
