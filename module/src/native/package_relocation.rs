use spacetimedb::{
    reducer, sats::bsatn, table, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{
    directory::{self, game_character_directory},
    instance_shard, package_actor,
};
use crate::{
    auth::game_account,
    package_account::game_package_account,
    peer::{is_peer, realm_shard, world_shard},
    remote_bindings::peer as remote,
    transfer::{self, game_transfer_in, game_transfer_out, CtxShard, Destination},
};

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum Relocation {
    Pending,
    Arrived(Identity),
    Refused(String),
}

#[table(accessor = game_package_relocation)]
pub struct PackageRelocation {
    #[primary_key]
    pub id: u64,
    #[index(btree)]
    pub package_name: String,
    #[index(btree)]
    pub guid: u64,
    pub trigger: u32,
    pub authority: u64,
    destination: Destination,
    pub result: Relocation,
}

#[derive(SpacetimeType, Clone, PartialEq, Eq)]
struct Crossing {
    id: u64,
    guid: u64,
    account_id: u64,
    package: String,
    realm: Identity,
    target: Identity,
    revision: u64,
}

#[table(accessor = game_package_arrival)]
pub struct PackageArrival {
    #[primary_key]
    pub id: u64,
    crossing: Crossing,
    source: Identity,
}

#[derive(SpacetimeType)]
struct Import {
    crossing: Crossing,
    account: Vec<u8>,
    blob: Vec<u8>,
}

#[derive(SpacetimeType)]
enum Message {
    Import(Import),
    Moved(Crossing),
    Release(Crossing),
}

#[table(accessor = game_package_relocation_delivery, outbox(remote::idc_package_relocation))]
pub struct RelocationDelivery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub message: Vec<u8>,
}

#[table(accessor = game_package_relocation_retry, scheduled(retry_package_relocation))]
pub struct RelocationRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: RelocationDelivery,
}
crate::virtual_clock_exempt!(game_package_relocation_retry);

fn send(ctx: &ReducerContext, target: Identity, message: Message) {
    ctx.db
        .game_package_relocation_delivery()
        .insert(RelocationDelivery {
            id: 0,
            database: remote::Identity(target),
            message: bsatn::to_vec(&message).expect("Package relocation message encodes"),
        });
}

fn route(
    ctx: &ReducerContext,
    package: &str,
    guid: u64,
    trigger: u32,
) -> Result<Destination, String> {
    if world_shard(ctx).is_some() || !crate::package_teardown::runs(ctx, &package.replace('-', "_"))
    {
        return Err("Package relocation requires an active Package on its World".into());
    }
    crate::sessionless::transfer_authority_gate(ctx, guid).map_err(|e| e.to_string())?;
    crate::sessionless::movement_gate(ctx, guid).map_err(|e| e.to_string())?;
    let character =
        crate::helpers::character_by_guid(ctx, guid).ok_or("Character is unavailable")?;
    if ctx
        .db
        .game_package_account()
        .account_id()
        .find(character.account_id)
        .is_none_or(|owner| owner.package_name != package)
    {
        return Err("Character belongs to another Package".into());
    }
    if crate::taxi::is_in_flight(ctx, guid)
        || super::realm_petition::pending(ctx, guid)
        || super::realm_auction::pending(ctx, guid)
    {
        return Err("Character transaction is settling".into());
    }
    let route = crate::areatrigger::area_trigger_route(ctx, trigger)
        .ok_or("AreaTrigger route is missing")?;
    let entity = crate::helpers::live_entity(ctx, guid).map_err(|e| e.to_string())?;
    if route.source_map != entity.map_id || !route.contains(entity.x, entity.y, entity.z) {
        return Err("Character is outside the AreaTrigger".into());
    }
    crate::areatrigger::area_trigger_destination(ctx, trigger)
        .ok_or("AreaTrigger route is missing".into())
}

// Propagate an error to abort the calling reducer; success names a retained asynchronous result.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_relocation(
    ctx: &ReducerContext,
    package: &str,
    guid: u64,
    trigger: u32,
) -> Result<u64, String> {
    let destination = route(ctx, package, guid, trigger)?;
    let requests = ctx.db.game_package_relocation();
    if requests.package_name().filter(package).count() >= 64 {
        return Err("consume completed Package relocations before requesting more".into());
    }
    if requests
        .guid()
        .filter(guid)
        .any(|row| row.result == Relocation::Pending)
    {
        return Err("Package relocation is already pending".into());
    }
    let actor = realm_shard(ctx)
        .map(|_| package_actor::local(ctx, guid))
        .transpose()?;
    let authority = actor
        .as_ref()
        .map_or(0, |(_, authority)| authority.revision);
    let id = *crate::auth::reserve_guids(ctx, 1, u64::MAX)?.start();
    let request = requests.insert(PackageRelocation {
        id,
        package_name: package.into(),
        guid,
        trigger,
        authority,
        destination,
        result: Relocation::Pending,
    });
    if crate::instance::is_dungeon_map(destination.map_id) {
        if let Some((actor, _)) = actor {
            instance_shard::enter_package(ctx, id, actor, trigger, destination)?;
        } else {
            crate::world::teleport_gate(ctx, guid, destination.map_id, 0)
                .map_err(|e| e.to_string())?;
            let instance =
                crate::instance::resolve_or_create_instance(ctx, guid, destination.map_id)?;
            begin(ctx, request, ctx.database_identity(), instance)?;
        }
    } else {
        let target = directory::transfer_destination(ctx, guid, destination.map_id, 0)
            .unwrap_or(ctx.database_identity());
        begin(ctx, request, target, 0)?;
    }
    Ok(id)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn take_relocation_result(
    ctx: &ReducerContext,
    package: &str,
    id: u64,
) -> Result<Relocation, String> {
    let requests = ctx.db.game_package_relocation();
    let request = requests.id().find(id).ok_or("unknown Package relocation")?;
    if request.package_name != package {
        return Err("relocation belongs to another Package".into());
    }
    if request.result != Relocation::Pending {
        requests.id().delete(id);
    }
    Ok(request.result)
}

fn finish(ctx: &ReducerContext, id: u64, result: Relocation) {
    let requests = ctx.db.game_package_relocation();
    if let Some(mut request) = requests.id().find(id) {
        request.result = result;
        requests.id().update(request);
    }
}

pub(super) fn validate_admission(ctx: &ReducerContext, id: u64) -> Result<(), String> {
    let request = ctx
        .db
        .game_package_relocation()
        .id()
        .find(id)
        .filter(|r| r.result == Relocation::Pending)
        .ok_or("Package relocation is no longer pending")?;
    let destination = route(ctx, &request.package_name, request.guid, request.trigger)?;
    if destination != request.destination
        || package_actor::local(ctx, request.guid)?.1.revision != request.authority
    {
        return Err("Package relocation authority or AreaTrigger changed".into());
    }
    Ok(())
}

pub(super) fn admitted(
    ctx: &ReducerContext,
    id: u64,
    world: Option<Identity>,
    instance: u64,
    error: &str,
) -> Result<(), String> {
    let Some(request) = ctx
        .db
        .game_package_relocation()
        .id()
        .find(id)
        .filter(|r| r.result == Relocation::Pending)
    else {
        return Ok(());
    };
    let validation = (|| {
        if !error.is_empty() {
            return Err(error.to_string());
        }
        validate_admission(ctx, id)?;
        world
            .filter(|_| instance != 0)
            .ok_or("Instance admission is missing".into())
    })();
    match validation {
        Ok(world) => begin(ctx, request, world, instance),
        Err(error) => {
            finish(ctx, id, Relocation::Refused(error));
            Ok(())
        }
    }
}

pub(super) fn expired(ctx: &ReducerContext, id: u64) {
    finish(
        ctx,
        id,
        Relocation::Refused("Instance admission expired".into()),
    );
}

fn begin(
    ctx: &ReducerContext,
    request: PackageRelocation,
    target: Identity,
    instance: u64,
) -> Result<(), String> {
    let destination = Destination {
        instance_id: instance,
        ..request.destination
    };
    if target == ctx.database_identity() {
        if instance != 0 {
            crate::instance::bind_character(
                ctx,
                request.guid,
                instance,
                destination.map_id,
                target,
            );
        }
        crate::world::teleport(
            ctx,
            request.guid,
            destination.map_id,
            instance,
            [destination.x, destination.y, destination.z, destination.o],
        )
        .map_err(|e| e.to_string())?;
        instance_shard::arrived(ctx, request.guid, destination.map_id, instance);
        finish(ctx, request.id, Relocation::Arrived(target));
        return Ok(());
    }
    let realm = realm_shard(ctx).ok_or("Transfer requires a Realm Shard")?;
    if target == realm || !is_peer(ctx, target) {
        return Err("destination is not a Peer World Shard".into());
    }
    let character =
        crate::helpers::character_by_guid(ctx, request.guid).ok_or("Character is unavailable")?;
    let account = ctx
        .db
        .game_account()
        .id()
        .find(character.account_id)
        .ok_or("Package Account is missing")?;
    crate::trade::cancel_trade_for(ctx, request.guid);
    crate::duel::interrupt_duel_for(ctx, request.guid);
    if let Some(cast) = crate::spell::pending_cast(ctx, request.guid) {
        crate::spell::cancel_cast_attempt(ctx, request.guid, cast.scheduled_id);
    }
    crate::combat::disengage(ctx, request.guid);
    crate::actor::stop_movement(ctx, request.guid);
    transfer::apply_begin(
        &mut CtxShard { ctx },
        request.id,
        request.guid,
        destination,
        true,
    )?;
    let escrow = ctx
        .db
        .game_transfer_out()
        .transfer_id()
        .find(request.id)
        .expect("Transfer created its source Escrow");
    let crossing = Crossing {
        id: request.id,
        guid: request.guid,
        account_id: account.id,
        package: request.package_name,
        realm,
        target,
        revision: directory::revision(ctx),
    };
    send(
        ctx,
        target,
        Message::Import(Import {
            crossing,
            account: bsatn::to_vec(&account).expect("Package Account encodes"),
            blob: escrow.blob,
        }),
    );
    Ok(())
}

fn require_source(ctx: &ReducerContext, crossing: &Crossing) -> Result<(), String> {
    if realm_shard(ctx) != Some(crossing.realm)
        || ctx.sender() == crossing.realm
        || crossing.target != ctx.database_identity()
    {
        return Err("Package Transfer source or destination does not match".into());
    }
    Ok(())
}

#[reducer]
pub fn idc_package_relocation(ctx: &ReducerContext, message: Vec<u8>) -> Result<(), String> {
    if !is_peer(ctx, ctx.sender()) {
        return Err("caller is not a Peer Shard".into());
    }
    let message: Message = bsatn::from_slice(&message).map_err(|e| e.to_string())?;
    match message {
        Message::Import(import) => {
            let crossing = import.crossing;
            require_source(ctx, &crossing)?;
            if !crate::package_teardown::runs(ctx, &crossing.package.replace('-', "_")) {
                return Err("Package is stopped on destination".into());
            }
            let decoded = transfer::decode_blob(crossing.id, &import.blob)?;
            let character = transfer::decode_character_row(crossing.id, &decoded.character_row)?;
            if decoded.transfer_id != crossing.id
                || decoded.character_guid != crossing.guid
                || character.guid != crossing.guid
                || character.account_id != crossing.account_id
                || character.online
                || ![
                    decoded.dest_x,
                    decoded.dest_y,
                    decoded.dest_z,
                    decoded.dest_o,
                ]
                .iter()
                .all(|v| v.is_finite())
                || !super::instance_allocation::hosts(ctx, decoded.dest_map_id, crossing.target)
                || (!crate::instance::is_dungeon_map(decoded.dest_map_id)
                    && decoded.dest_instance_id != 0)
            {
                return Err("Package Transfer destination or Character does not match".into());
            }
            if transfer::is_in_transit(ctx, crossing.guid)
                || crate::helpers::character_by_guid(ctx, crossing.guid).is_some()
            {
                return Err("destination already holds this Character".into());
            }
            crate::account_ownership::require_actor(
                ctx,
                crate::account_ownership::SessionActor {
                    guid: crossing.guid,
                    ownership: None,
                },
            )?;
            if ctx
                .db
                .game_account()
                .id()
                .find(crossing.account_id)
                .is_some_and(|account| account.banned)
            {
                return Err("Package Account is banned on destination".into());
            }
            let account =
                super::package_creation::store_account(ctx, &crossing.package, &import.account)?;
            if account.id != crossing.account_id || account.banned {
                return Err("Package Account cannot Transfer".into());
            }
            transfer::apply_import_blob(&mut CtxShard { ctx }, crossing.id, import.blob)?;
            instance_shard::imported(
                ctx,
                crossing.guid,
                0,
                decoded.dest_map_id,
                decoded.dest_instance_id,
                Some(crossing.id),
            )?;
            directory::observe_revision(ctx, crossing.revision);
            ctx.db.game_package_arrival().insert(PackageArrival {
                id: crossing.id,
                crossing,
                source: ctx.sender(),
            });
        }
        Message::Moved(crossing) => {
            directory::require_world(ctx)?;
            if crossing.realm != ctx.database_identity()
                || ctx
                    .db
                    .game_package_account()
                    .account_id()
                    .find(crossing.account_id)
                    .is_none_or(|owner| owner.package_name != crossing.package)
            {
                return Err("Realm does not own this Package Account".into());
            }
            super::transfer::idc_native_transfer_moved(
                ctx,
                crossing.target,
                crossing.id,
                crossing.guid,
                crossing.account_id,
                0,
                crossing.revision,
            )?;
            if ctx
                .db
                .game_character_directory()
                .guid()
                .find(crossing.guid)
                .is_none_or(|entry| {
                    entry.world != crossing.target || entry.account_id != crossing.account_id
                })
            {
                return Err("Realm has not accepted the Package destination".into());
            }
        }
        Message::Release(crossing) => {
            require_source(ctx, &crossing)?;
            let arrival = ctx
                .db
                .game_package_arrival()
                .id()
                .find(crossing.id)
                .filter(|a| a.source == ctx.sender() && a.crossing == crossing)
                .ok_or("Package arrival does not match release")?;
            let receipt = ctx
                .db
                .game_transfer_in()
                .transfer_id()
                .find(crossing.id)
                .ok_or("arrival fence is missing")?;
            if receipt.character_guid != crossing.guid {
                return Err("arrival fence belongs to another Character".into());
            }
            transfer::apply_release(&mut CtxShard { ctx }, crossing.id)?;
            super::realm_party::restore(ctx, crossing.guid);
            crate::actor::restore_sessionless_body(ctx, crossing.guid)
                .ok_or("arriving Package Character is missing")?;
            directory::changed(ctx, crossing.account_id, crossing.guid, true);
            ctx.db.game_package_arrival().id().delete(arrival.id);
        }
    }
    Ok(())
}

#[reducer(on_result(game_package_relocation_delivery))]
pub fn package_relocation_answered(
    ctx: &ReducerContext,
    call: RelocationDelivery,
    result: Result<(), String>,
) -> Result<(), String> {
    let message: Message =
        bsatn::from_slice(&call.message).expect("stored Package relocation decodes");
    match (message, result) {
        (Message::Import(import), Err(error)) => {
            transfer::apply_abort(&mut CtxShard { ctx }, import.crossing.id)?;
            crate::actor::restore_sessionless_body(ctx, import.crossing.guid)
                .ok_or("refused Package Character is missing")?;
            finish(ctx, import.crossing.id, Relocation::Refused(error));
        }
        (Message::Import(import), Ok(())) => {
            transfer::apply_confirm(&mut CtxShard { ctx }, import.crossing.id)?;
            transfer::apply_finish_step(&mut CtxShard { ctx }, import.crossing.id)?;
            send(ctx, import.crossing.realm, Message::Moved(import.crossing));
        }
        (Message::Moved(crossing), Ok(())) => {
            send(ctx, crossing.target, Message::Release(crossing))
        }
        (Message::Release(crossing), Ok(())) => {
            finish(ctx, crossing.id, Relocation::Arrived(crossing.target))
        }
        (_, Err(error)) => {
            spacetimedb::log::warn!(
                "Package relocation settlement to {} waits: {error}",
                call.database.0
            );
            ctx.db
                .game_package_relocation_retry()
                .insert(RelocationRetry {
                    id: 0,
                    scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
                    call,
                });
        }
    }
    Ok(())
}

#[reducer]
pub fn retry_package_relocation(ctx: &ReducerContext, retry: RelocationRetry) {
    if crate::time::scheduler_authorized(ctx) {
        ctx.db
            .game_package_relocation_delivery()
            .insert(RelocationDelivery {
                id: 0,
                ..retry.call
            });
    }
}

pub(crate) fn require_settled(ctx: &ReducerContext, package: &str) -> Result<(), String> {
    if ctx.db.game_package_relocation().iter().any(|r| {
        r.package_name.replace('-', "_") == package.replace('-', "_")
            && r.result == Relocation::Pending
    }) {
        return Err("Package relocation is pending; wait before teardown".into());
    }
    Ok(())
}

pub(crate) fn clear_results(ctx: &ReducerContext, package: &str) {
    let rows = ctx.db.game_package_relocation();
    let ids: Vec<_> = rows
        .iter()
        .filter(|r| r.package_name.replace('-', "_") == package.replace('-', "_"))
        .map(|r| r.id)
        .collect();
    for id in ids {
        rows.id().delete(id);
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_request_package_relocation(
    ctx: &ReducerContext,
    package: String,
    guid: u64,
    trigger: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::package::request_relocation(ctx, &package, guid, trigger).map(|_| ())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_take_package_relocation(
    ctx: &ReducerContext,
    package: String,
    id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let result = crate::package::take_relocation_result(ctx, &package, id)?;
    spacetimedb::log::info!("Package relocation {id}: {result:?}");
    Ok(())
}
