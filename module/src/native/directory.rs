use spacetimedb::sats::bsatn;
use spacetimedb::{
    reducer, table, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration,
};

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
        Some(ShardRole::World { .. })
            if world == ctx.database_identity() || is_peer(ctx, world) => {}
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
            || super::instance_allocation::worlds(ctx).any(|world| world == sender)
            || ctx
                .db
                .game_map_owner()
                .iter()
                .any(|owner| owner.world == sender)
    });
    crate::peer::require_sender(ctx, configured.then_some(sender))
}

pub(crate) fn owns_map(ctx: &ReducerContext, map_id: u32) -> bool {
    super::instance_allocation::hosts(ctx, map_id, ctx.database_identity())
        || realm_shard(ctx).is_none()
        || ctx.db.game_map_owner().iter().next().is_none()
}

pub(crate) fn transfer_target(ctx: &ReducerContext, map_id: u32) -> Option<Identity> {
    let realm = realm_shard(ctx)?;
    ctx.db
        .game_map_owner()
        .map_id()
        .find(map_id)
        .map(|owner| owner.world)
        .filter(|world| {
            *world != ctx.database_identity() && *world != realm && is_peer(ctx, *world)
        })
}

pub(crate) fn transfer_destination(
    ctx: &ReducerContext,
    guid: u64,
    map: u32,
    instance: u64,
) -> Option<Identity> {
    use crate::instance::game_instance_binding;
    if crate::instance::is_dungeon_map(map) && instance != 0 {
        if let Some(world) = ctx
            .db
            .game_instance_binding()
            .by_character()
            .filter(guid)
            .find(|binding| binding.map_id == map && binding.instance_id == instance)
            .and_then(|binding| binding.world)
        {
            return realm_shard(ctx)
                .filter(|realm| world != *realm && world != ctx.database_identity())
                .map(|_| world);
        }
    }
    let realm = realm_shard(ctx)?;
    ctx.db
        .game_map_owner()
        .map_id()
        .find(map)
        .map(|owner| owner.world)
        .filter(|world| *world != ctx.database_identity() && *world != realm)
}

pub(super) fn worlds(ctx: &ReducerContext) -> std::collections::BTreeSet<Identity> {
    world_shard(ctx)
        .into_iter()
        .chain(ctx.db.game_map_owner().iter().map(|owner| owner.world))
        .chain(super::instance_allocation::worlds(ctx))
        .collect()
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

#[table(accessor = game_directory_clock)]
pub struct DirectoryClock {
    #[primary_key]
    pub id: u8,
    pub revision: u64,
}

pub(super) fn observe_revision(ctx: &ReducerContext, revision: u64) {
    let clock = ctx.db.game_directory_clock();
    let revision = revision.max(clock.id().find(0).map_or(0, |row| row.revision));
    clock.id().delete(0);
    clock.insert(DirectoryClock { id: 0, revision });
}

pub(super) fn revision(ctx: &ReducerContext) -> u64 {
    let revision = (ctx.timestamp.to_micros_since_unix_epoch() as u64).max(
        ctx.db
            .game_directory_clock()
            .id()
            .find(0)
            .map_or(0, |row| row.revision + 1),
    );
    observe_revision(ctx, revision);
    revision
}

pub(crate) fn changed(ctx: &ReducerContext, account_id: u64, guid: u64, present: bool) {
    send_changes(ctx, account_id, vec![(guid, present)]);
}

fn send_changes(ctx: &ReducerContext, account_id: u64, changes: Vec<(u64, bool)>) {
    let Some(realm) = realm_shard(ctx) else {
        return;
    };
    let revision = revision(ctx);
    let profiles: Vec<_> = changes
        .into_iter()
        .filter_map(|(guid, present)| {
            let character = if present {
                Some(crate::helpers::character_by_guid(ctx, guid)?)
            } else {
                None
            };
            Some(CharacterProfile {
                guid,
                account_id,
                world: ctx.database_identity(),
                revision,
                name: character
                    .as_ref()
                    .map_or_else(String::new, |character| character.name.clone()),
                race: character.as_ref().map_or(0, |character| character.race),
                class: character.as_ref().map_or(0, |character| character.class),
                gender: character.as_ref().map_or(0, |character| character.gender),
                deleted: !present,
            })
        })
        .collect();
    if profiles.is_empty() {
        return;
    }
    ctx.db.game_character_profiles().insert(CharacterProfiles {
        id: 0,
        database: remote::Identity(realm),
        profiles: bsatn::to_vec(&profiles).expect("Character profiles encode"),
    });
}

pub(crate) fn report_characters(ctx: &ReducerContext) {
    let Some(realm) = realm_shard(ctx) else {
        return;
    };
    let revision = revision(ctx);
    let complete = !ctx
        .db
        .game_character()
        .iter()
        .any(|character| crate::transfer::is_in_transit(ctx, character.guid));
    let profiles: Vec<_> = ctx
        .db
        .game_character()
        .iter()
        .filter(|character| !crate::transfer::is_in_transit(ctx, character.guid))
        .map(|character| CharacterProfile {
            guid: character.guid,
            account_id: character.account_id,
            world: ctx.database_identity(),
            revision,
            name: character.name,
            race: character.race,
            class: character.class,
            gender: character.gender,
            deleted: false,
        })
        .collect();
    let total = profiles.len().div_ceil(128).max(1);
    for index in 0..total {
        let start = index * 128;
        let report = ProfileReport {
            profiles: profiles[start..profiles.len().min(start + 128)].to_vec(),
            snapshot: complete.then_some(super::character_names::NameReport {
                revision,
                index: index as u32,
                total: total as u32,
            }),
        };
        ctx.db.game_character_profiles().insert(CharacterProfiles {
            id: 0,
            database: remote::Identity(realm),
            profiles: bsatn::to_vec(&report).expect("Character profile report encodes"),
        });
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
            if version.revision >= revision {
                continue;
            }
            if version.world != ctx.sender() {
                return Err("Character directory owner cannot change without a Transfer".into());
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
        if !present {
            super::character_names::deleted(ctx, guid);
            super::instance_allocation::forget(ctx, guid, false);
            super::realm_party::deleted(ctx, guid)?;
            super::realm_guild::deleted(ctx, guid);
        }
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

#[table(accessor = game_character_profile)]
#[derive(Clone)]
pub struct CharacterProfile {
    #[primary_key]
    pub guid: u64,
    pub account_id: u64,
    pub world: Identity,
    pub revision: u64,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub gender: u8,
    pub deleted: bool,
}

#[derive(SpacetimeType)]
struct ProfileReport {
    profiles: Vec<CharacterProfile>,
    snapshot: Option<super::character_names::NameReport>,
}

#[table(accessor = game_character_profiles, outbox(remote::idc_character_profiles))]
pub struct CharacterProfiles {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub profiles: Vec<u8>,
}

#[table(accessor = game_character_profiles_retry, scheduled(retry_character_profiles))]
pub struct CharacterProfilesRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: CharacterProfiles,
}

#[reducer]
pub fn idc_character_profiles(ctx: &ReducerContext, profiles: Vec<u8>) -> Result<(), String> {
    let realm = world_shard(ctx).is_some();
    if realm {
        require_world(ctx)?;
    } else {
        crate::peer::require_sender(ctx, realm_shard(ctx))?;
    }
    let report: ProfileReport = bsatn::from_slice(&profiles)
        .or_else(|_| {
            bsatn::from_slice::<Vec<CharacterProfile>>(&profiles).map(|profiles| ProfileReport {
                profiles,
                snapshot: None,
            })
        })
        .map_err(|error| error.to_string())?;
    let mut accepted = Vec::new();
    for profile in report.profiles {
        if realm {
            if profile.world != ctx.sender() {
                return Err("Character profile source differs from its World Shard".into());
            }
            if let Some(version) = ctx
                .db
                .game_character_directory_version()
                .guid()
                .find(profile.guid)
                .filter(|version| version.revision >= profile.revision)
            {
                let current = ctx.db.game_character_directory().guid().find(profile.guid);
                let same_owner = version.world == ctx.sender()
                    && match current {
                        Some(entry) => {
                            !profile.deleted
                                && entry.world == ctx.sender()
                                && entry.account_id == profile.account_id
                        }
                        None => profile.deleted,
                    };
                if !same_owner {
                    continue;
                }
            } else {
                let changes = bsatn::to_vec(&vec![(profile.guid, !profile.deleted)])
                    .expect("directory change encodes");
                apply(ctx, profile.account_id, &changes, profile.revision)?;
            }
        }
        let table = ctx.db.game_character_profile();
        if table
            .guid()
            .find(profile.guid)
            .is_some_and(|old| old.revision >= profile.revision)
        {
            continue;
        }
        observe_revision(ctx, profile.revision);
        if realm {
            super::character_names::observe(ctx, &profile)?;
        }
        if !realm && profile.deleted {
            super::social::deleted(ctx, profile.guid);
        }
        table.guid().delete(profile.guid);
        table.insert(profile.clone());
        accepted.push(profile);
    }
    if realm {
        publish_profiles(ctx, &accepted);
        if let Some(snapshot) = report.snapshot {
            super::character_names::reported(ctx, snapshot)?;
        }
    }
    Ok(())
}

fn publish_profiles(ctx: &ReducerContext, profiles: &[CharacterProfile]) {
    if profiles.is_empty() {
        return;
    }
    let worlds = worlds(ctx);
    let profiles = bsatn::to_vec(profiles).expect("Character profiles encode");
    for world in worlds {
        ctx.db.game_character_profiles().insert(CharacterProfiles {
            id: 0,
            database: remote::Identity(world),
            profiles: profiles.clone(),
        });
    }
}

#[reducer(on_result(game_character_profiles))]
pub fn character_profiles_answered(
    ctx: &ReducerContext,
    call: CharacterProfiles,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        spacetimedb::log::warn!("Character profiles to {} wait: {error}", call.database.0);
        ctx.db
            .game_character_profiles_retry()
            .insert(CharacterProfilesRetry {
                id: 0,
                scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
                call,
            });
    }
}

#[reducer]
pub fn retry_character_profiles(ctx: &ReducerContext, retry: CharacterProfilesRetry) {
    if crate::time::scheduler_authorized(ctx) && is_peer(ctx, retry.call.database.0) {
        ctx.db.game_character_profiles().insert(CharacterProfiles {
            id: 0,
            ..retry.call
        });
    }
}

#[reducer]
pub fn operator_report_character_profiles(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if world_shard(ctx).is_some() {
        let profiles = ctx.db.game_character_profile().iter().collect::<Vec<_>>();
        for batch in profiles.chunks(128) {
            publish_profiles(ctx, batch);
        }
    } else if realm_shard(ctx).is_some() {
        report_characters(ctx);
    } else {
        return Err("Character profiles require a Realm Shard or World Shard".into());
    }
    Ok(())
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
