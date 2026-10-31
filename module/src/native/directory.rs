use spacetimedb::sats::bsatn;
use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

use crate::game_character;
use crate::peer::{is_peer, realm_shard, role, world_shard, ShardRole};
use crate::remote_bindings::peer as remote;

#[table(accessor = game_map_owner)]
pub struct MapOwner {
    #[primary_key]
    pub map_id: u32,
    pub world: Identity,
}

#[table(accessor = game_character_start_map)]
pub struct CharacterStartMap {
    #[primary_key]
    pub race_class: u16,
    pub map_id: u32,
}

#[table(accessor = game_character_directory, index(accessor = by_account, btree(columns = [account_id])))]
pub struct CharacterDirectory {
    #[primary_key]
    pub guid: u64,
    pub account_id: u64,
    pub world: Identity,
}

#[table(accessor = game_character_directory_version)]
pub struct CharacterDirectoryVersion {
    #[primary_key]
    pub guid: u64,
    pub revision: u64,
    pub world: Identity,
}

#[reducer]
pub fn operator_set_map_owner(
    ctx: &ReducerContext,
    map_id: u32,
    world: Identity,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    match role(ctx) {
        Some(ShardRole::Realm { .. }) if is_peer(ctx, world) => {}
        Some(ShardRole::World { .. }) if world == ctx.database_identity() => {}
        _ => return Err("map owner must be a World Shard of this Realm".into()),
    }
    let owners = ctx.db.game_map_owner();
    if owners
        .map_id()
        .find(map_id)
        .is_some_and(|owner| owner.world != world)
    {
        return Err("moving a map requires a Character Transfer".into());
    }
    owners.map_id().delete(map_id);
    owners.insert(MapOwner { map_id, world });
    if realm_shard(ctx).is_some() {
        report_characters(ctx);
    }
    Ok(())
}

#[reducer]
pub fn operator_set_character_start_map(
    ctx: &ReducerContext,
    race: u8,
    class: u8,
    map_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if world_shard(ctx).is_none() || ctx.db.game_map_owner().map_id().find(map_id).is_none() {
        return Err("start map requires a Realm Shard map owner".into());
    }
    let race_class = lyracore_package_delta::packed_race_class_id(race, class) as u16;
    ctx.db
        .game_character_start_map()
        .race_class()
        .delete(race_class);
    ctx.db
        .game_character_start_map()
        .insert(CharacterStartMap { race_class, map_id });
    Ok(())
}

pub(super) fn require_world(ctx: &ReducerContext) -> Result<(), String> {
    let sender = ctx.sender();
    let configured = world_shard(ctx).is_some_and(|default| {
        default == sender
            || ctx
                .db
                .game_map_owner()
                .iter()
                .any(|owner| owner.world == sender)
    });
    crate::peer::require_sender(ctx, configured.then_some(sender))
}

pub(crate) fn owns_map(ctx: &ReducerContext, map_id: u32) -> bool {
    realm_shard(ctx).is_none()
        || ctx.db.game_map_owner().iter().next().is_none()
        || ctx
            .db
            .game_map_owner()
            .map_id()
            .find(map_id)
            .is_some_and(|owner| owner.world == ctx.database_identity())
}

pub(super) fn character_world(
    ctx: &ReducerContext,
    account_id: u64,
    guid: u64,
) -> Option<Identity> {
    let explicit = ctx.db.game_map_owner().iter().next().is_some();
    route_character(
        ctx.db
            .game_character_directory()
            .guid()
            .find(guid)
            .map(|entry| (entry.account_id, entry.world)),
        account_id,
        world_shard(ctx),
        explicit,
    )
}

fn route_character(
    entry: Option<(u64, Identity)>,
    account_id: u64,
    default: Option<Identity>,
    explicit: bool,
) -> Option<Identity> {
    match entry {
        Some((owner, world)) => (owner == account_id).then_some(world),
        None => default.filter(|_| !explicit),
    }
}

pub(super) fn create_world(ctx: &ReducerContext, race: u8, class: u8) -> Option<Identity> {
    if ctx.db.game_map_owner().iter().next().is_none() {
        return world_shard(ctx);
    }
    let key = lyracore_package_delta::packed_race_class_id(race, class) as u16;
    let start = ctx.db.game_character_start_map().race_class().find(key)?;
    ctx.db
        .game_map_owner()
        .map_id()
        .find(start.map_id)
        .map(|owner| owner.world)
}

#[table(accessor = game_character_directory_update, outbox(remote::idc_character_directory))]
pub struct CharacterDirectoryUpdate {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub account_id: u64,
    pub changes: Vec<u8>,
    pub revision: u64,
}

pub(super) fn revision(ctx: &ReducerContext) -> u64 {
    ctx.timestamp.to_micros_since_unix_epoch() as u64
}

pub(crate) fn changed(ctx: &ReducerContext, account_id: u64, guid: u64, present: bool) {
    send_changes(ctx, account_id, vec![(guid, present)]);
}

fn send_changes(ctx: &ReducerContext, account_id: u64, changes: Vec<(u64, bool)>) {
    let Some(realm) = realm_shard(ctx) else {
        return;
    };
    ctx.db
        .game_character_directory_update()
        .insert(CharacterDirectoryUpdate {
            id: 0,
            database: remote::Identity(realm),
            account_id,
            changes: bsatn::to_vec(&changes).expect("directory changes encode"),
            revision: revision(ctx),
        });
}

// One message per Account: each call between databases costs a round trip and disk writes.
pub(crate) fn report_characters(ctx: &ReducerContext) {
    let mut by_account = std::collections::BTreeMap::<u64, Vec<(u64, bool)>>::new();
    for character in ctx.db.game_character().iter() {
        if !crate::transfer::is_in_transit(ctx, character.guid) {
            by_account
                .entry(character.account_id)
                .or_default()
                .push((character.guid, true));
        }
    }
    for (account_id, changes) in by_account {
        send_changes(ctx, account_id, changes);
    }
}

pub(super) fn apply(
    ctx: &ReducerContext,
    account_id: u64,
    changes: &[u8],
    revision: u64,
) -> Result<(), String> {
    let changes: Vec<(u64, bool)> =
        bsatn::from_slice(changes).map_err(|error| error.to_string())?;
    for (guid, present) in changes {
        let versions = ctx.db.game_character_directory_version();
        if let Some(version) = versions.guid().find(guid) {
            if version.world != ctx.sender() {
                return Err("Character directory owner cannot change without a Transfer".into());
            }
            if version.revision >= revision {
                continue;
            }
        }
        let directory = ctx.db.game_character_directory();
        if directory
            .guid()
            .find(guid)
            .is_some_and(|entry| entry.account_id != account_id)
        {
            return Err("Character directory Account cannot change".into());
        }
        versions.guid().delete(guid);
        versions.insert(CharacterDirectoryVersion {
            guid,
            revision,
            world: ctx.sender(),
        });
        directory.guid().delete(guid);
        if present {
            directory.insert(CharacterDirectory {
                guid,
                account_id,
                world: ctx.sender(),
            });
        }
    }
    Ok(())
}

#[reducer]
pub fn idc_character_directory(
    ctx: &ReducerContext,
    account_id: u64,
    changes: Vec<u8>,
    revision: u64,
) -> Result<(), String> {
    require_world(ctx)?;
    apply(ctx, account_id, &changes, revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_routing_requires_the_account_and_uses_default_only_without_maps() {
        let world = Identity::ZERO;
        assert_eq!(
            route_character(Some((7, world)), 7, None, true),
            Some(world)
        );
        assert_eq!(
            route_character(Some((8, world)), 7, Some(world), false),
            None
        );
        assert_eq!(route_character(None, 7, Some(world), true), None);
        assert_eq!(route_character(None, 7, Some(world), false), Some(world));
    }
}
