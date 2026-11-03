use std::collections::BTreeSet;

use lyracore_protocol::messages::vanilla::{
    opcodes::ServerOpcodeMessage, InstanceResetFailedReason, Map, SMSG_INSTANCE_RESET,
    SMSG_INSTANCE_RESET_FAILED, SMSG_NOTIFICATION,
};
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, ReducerContext, SpacetimeType, Table,
};

use super::{directory, game_native_player, realm_party, realm_party::PartyCharacter, Rollback};
use crate::{
    group::{self, game_group},
    instance::{self, ResetBinding, ResetFailure, ResetOutcome},
    peer::{realm_shard, require_sender, world_shard},
    remote_bindings::peer as remote,
};

const RESET_MICROS: i64 = 60_000_000;

#[derive(SpacetimeType)]
struct Reset {
    actor: PartyCharacter,
    bindings: Vec<ResetBinding>,
    group_id: u64,
    offline: bool,
    expires: i64,
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
) -> Result<(), Rollback> {
    super::actor(ctx, guid)?;
    let bindings = instance::reset_bindings(ctx, guid);
    if realm_shard(ctx).is_some() {
        let expires = ctx.timestamp.to_micros_since_unix_epoch() + RESET_MICROS;
        let body = bsatn::to_vec(&(bindings, expires)).expect("Instance reset bindings encode");
        realm_party::submit_admitted(ctx, connection, guid, 0x31d, body)?;
    } else {
        let offline = group::group_of(ctx, guid).is_some_and(|member| {
            group::members_of(ctx, member.group_id)
                .iter()
                .any(|member| !super::is_player(ctx, member.character_guid))
        });
        reply(
            ctx,
            connection,
            instance::reset_instances(ctx, guid, &bindings, offline),
        )?;
    }
    Ok(())
}

pub(super) fn dispatch(
    ctx: &ReducerContext,
    actor: PartyCharacter,
    body: &[u8],
) -> Result<(), String> {
    use directory::game_map_owner;
    let (bindings, expires): (Vec<ResetBinding>, i64) =
        bsatn::from_slice(body).map_err(|error| error.to_string())?;
    if ctx.timestamp.to_micros_since_unix_epoch() >= expires {
        return Err("Instance reset request expired".into());
    }
    let group = group::group_of(ctx, actor.guid)
        .and_then(|member| ctx.db.game_group().group_id().find(member.group_id));
    if group
        .as_ref()
        .is_some_and(|group| group.leader_guid != actor.guid)
    {
        return Ok(());
    }
    let group_id = group.map_or(0, |group| group.group_id);
    let offline = group::members_of(ctx, group_id)
        .iter()
        .any(|member| !realm_party::online(ctx, member.character_guid));
    let targets: BTreeSet<_> = if group_id != 0 {
        ctx.db
            .game_map_owner()
            .iter()
            .filter(|owner| instance::resettable_map(owner.map_id))
            .map(|owner| owner.world)
            .chain(super::instance_allocation::worlds(ctx))
            .chain(
                ctx.db
                    .game_map_owner()
                    .iter()
                    .next()
                    .is_none()
                    .then(|| world_shard(ctx))
                    .flatten(),
            )
            .collect()
    } else {
        bindings
            .iter()
            .filter(|binding| instance::resettable_map(binding.map))
            .filter_map(|binding| {
                binding.world.or_else(|| {
                    ctx.db
                        .game_map_owner()
                        .map_id()
                        .find(binding.map)
                        .map(|owner| owner.world)
                        .or_else(|| world_shard(ctx))
                })
            })
            .collect()
    };
    let party = realm_party::snapshot(ctx, actor.guid);
    let request = bsatn::to_vec(&Reset {
        actor,
        bindings,
        group_id,
        offline,
        expires,
    })
    .expect("Instance reset encodes");
    for target in targets {
        ctx.db
            .game_world_instance_reset()
            .insert(WorldInstanceReset {
                id: 0,
                database: remote::Identity(target),
                request: request.clone(),
                party: party.clone(),
            });
    }
    Ok(())
}

#[table(accessor = game_world_instance_reset, outbox(remote::idc_world_instance_reset))]
pub struct WorldInstanceReset {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub request: Vec<u8>,
    pub party: Vec<u8>,
}

#[table(accessor = game_instance_reset_result, outbox(remote::idc_instance_reset_result))]
pub struct InstanceResetResult {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub request: Vec<u8>,
    pub result: Vec<u8>,
}

#[reducer]
pub fn idc_world_instance_reset(
    ctx: &ReducerContext,
    request: Vec<u8>,
    party: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let reset: Reset = bsatn::from_slice(&request).map_err(|error| error.to_string())?;
    if ctx.timestamp.to_micros_since_unix_epoch() >= reset.expires {
        return Err("Instance reset request expired".into());
    }
    if !crate::peer::is_peer(ctx, reset.actor.world) && reset.actor.world != ctx.database_identity()
    {
        return Err("Instance reset source is not a World Shard".into());
    }
    realm_party::apply_snapshot(ctx, &party)?;
    if group::group_of(ctx, reset.actor.guid).map_or(0, |member| member.group_id) != reset.group_id
        || (reset.group_id != 0
            && !ctx
                .db
                .game_group()
                .group_id()
                .find(reset.group_id)
                .is_some_and(|group| group.leader_guid == reset.actor.guid))
    {
        return Err("Group changed during Instance reset".into());
    }
    let results = instance::reset_instances(ctx, reset.actor.guid, &reset.bindings, reset.offline);
    result_to_source(ctx, &reset, request, Some(results))
}

fn result_to_source(
    ctx: &ReducerContext,
    reset: &Reset,
    request: Vec<u8>,
    result: Option<Vec<ResetOutcome>>,
) -> Result<(), String> {
    if reset.actor.world == ctx.database_identity() {
        return finish(ctx, reset, result);
    }
    ctx.db
        .game_instance_reset_result()
        .insert(InstanceResetResult {
            id: 0,
            database: remote::Identity(reset.actor.world),
            request,
            result: bsatn::to_vec(&result).expect("Instance reset result encodes"),
        });
    Ok(())
}

#[reducer(on_result(game_world_instance_reset))]
pub fn world_instance_reset_answered(
    ctx: &ReducerContext,
    call: WorldInstanceReset,
    result: Result<(), String>,
) -> Result<(), String> {
    if result.is_err() {
        if let Ok(reset) = bsatn::from_slice::<Reset>(&call.request) {
            result_to_source(ctx, &reset, call.request, None)?;
        }
    }
    Ok(())
}

#[reducer]
pub fn idc_instance_reset_result(
    ctx: &ReducerContext,
    request: Vec<u8>,
    result: Vec<u8>,
) -> Result<(), String> {
    if realm_shard(ctx).is_none() || !crate::peer::is_peer(ctx, ctx.sender()) {
        return Err("caller is not a Peer Shard".into());
    }
    let reset: Reset = bsatn::from_slice(&request).map_err(|error| error.to_string())?;
    let result: Option<Vec<ResetOutcome>> =
        bsatn::from_slice(&result).map_err(|error| error.to_string())?;
    if reset.actor.world != ctx.database_identity() {
        return Err("Instance reset reply belongs to another World Shard".into());
    }
    if let Some(results) = &result {
        for result in results {
            if !super::instance_allocation::hosts(ctx, result.map, ctx.sender()) {
                return Err("Instance reset reply is from another map's Shard".into());
            }
        }
    } else {
        require_sender(ctx, realm_shard(ctx))?;
    }
    finish(ctx, &reset, result)
}

fn finish(
    ctx: &ReducerContext,
    reset: &Reset,
    result: Option<Vec<ResetOutcome>>,
) -> Result<(), String> {
    let connection = ConnectionId::from_u128(reset.actor.connection);
    if ctx.timestamp.to_micros_since_unix_epoch() >= reset.expires
        || !ctx
            .db
            .game_native_player()
            .connection_id()
            .find(connection)
            .is_some_and(|player| player.character_guid == reset.actor.guid)
    {
        return Ok(());
    }
    match result {
        Some(results) => reply(ctx, connection, results),
        None => super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
                notification: "Instance reset request unavailable. Please try again.".into(),
            })),
            false,
        ),
    }
}

fn reply(
    ctx: &ReducerContext,
    connection: ConnectionId,
    results: Vec<ResetOutcome>,
) -> Result<(), String> {
    for result in results {
        let map = Map::try_from(result.map).map_err(|error| error.to_string())?;
        let message = match result.failure {
            Some(failure) => {
                ServerOpcodeMessage::SMSG_INSTANCE_RESET_FAILED(SMSG_INSTANCE_RESET_FAILED {
                    reason: match failure {
                        ResetFailure::Occupied => InstanceResetFailedReason::General,
                        ResetFailure::Offline => InstanceResetFailedReason::Offline,
                    },
                    map,
                })
            }
            None => ServerOpcodeMessage::SMSG_INSTANCE_RESET(SMSG_INSTANCE_RESET { map }),
        };
        super::send(ctx, connection, message, false)?;
    }
    Ok(())
}
