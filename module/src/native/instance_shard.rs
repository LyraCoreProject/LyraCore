use lyracore_protocol::messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_NOTIFICATION};
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, Identity, ReducerContext, ScheduleAt, SpacetimeType,
    Table, TimeDuration,
};

use super::{
    directory::{self, game_map_owner},
    game_native_player,
    realm_party::{self, PartyCharacter},
};
use crate::{
    game_area_trigger,
    instance::{self, game_instance, game_instance_binding},
    peer::{realm_shard, require_sender},
    remote_bindings::peer as remote,
};

const ADMISSION_MICROS: i64 = 60_000_000;

#[derive(SpacetimeType, Clone)]
pub struct Request {
    id: u64,
    actor: PartyCharacter,
    map: u32,
    binding: u64,
    trigger: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    expires: i64,
}

#[derive(SpacetimeType)]
struct Admission {
    request: Request,
    binding_world: Option<Identity>,
}

#[derive(SpacetimeType)]
struct AdmissionParty {
    group: u64,
    snapshot: Vec<u8>,
}

fn admission(bytes: &[u8]) -> Result<Admission, String> {
    bsatn::from_slice(bytes)
        .or_else(|_| {
            bsatn::from_slice::<Request>(bytes).map(|request| Admission {
                request,
                binding_world: None,
            })
        })
        .map_err(|error| error.to_string())
}

#[table(accessor = game_instance_entry, scheduled(expire_instance_entry))]
pub struct InstanceEntry {
    #[primary_key]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub guid: u64,
    pub request: Vec<u8>,
}
crate::virtual_clock_exempt!(game_instance_entry);

crate::character_owned!(delete, fn sweep_delete_game_instance_entry(ctx, character_guid) {
    ctx.db.game_instance_entry().guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_instance_entry());

#[table(accessor = game_realm_instance_entry, outbox(remote::idc_realm_instance_entry))]
pub struct RealmInstanceEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub request: Vec<u8>,
}

pub(crate) fn enter(
    ctx: &ReducerContext,
    guid: u64,
    trigger: u32,
    map: u32,
    pose: [f32; 4],
) -> bool {
    let Some(realm) = realm_shard(ctx).filter(|_| {
        super::instance_allocation::configured(ctx, map)
            || directory::transfer_destination(ctx, guid, map, 0).is_some()
    }) else {
        return false;
    };
    let Some(actor) = realm_party::facts(ctx, guid, true) else {
        return true;
    };
    if ctx.db.game_instance_entry().guid().find(guid).is_some() {
        return true;
    }
    let id = directory::revision(ctx);
    queue_entry(ctx, realm, id, actor, trigger, map, pose)
        .expect("native admission has no pending entry");
    true
}

pub(super) fn enter_package(
    ctx: &ReducerContext,
    id: u64,
    actor: PartyCharacter,
    trigger: u32,
    destination: crate::transfer::Destination,
) -> Result<(), String> {
    let realm = realm_shard(ctx).ok_or("Instance admission requires a Realm Shard")?;
    queue_entry(
        ctx,
        realm,
        id,
        actor,
        trigger,
        destination.map_id,
        [destination.x, destination.y, destination.z, destination.o],
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "Native and Package admission share the same request"
)]
fn queue_entry(
    ctx: &ReducerContext,
    realm: Identity,
    id: u64,
    actor: PartyCharacter,
    trigger: u32,
    map: u32,
    pose: [f32; 4],
) -> Result<(), String> {
    let guid = actor.guid;
    if ctx.db.game_instance_entry().guid().find(guid).is_some() {
        return Err("Instance admission is already pending".into());
    }
    let binding = ctx
        .db
        .game_instance_binding()
        .by_character()
        .filter(guid)
        .find(|binding| binding.map_id == map)
        .map(|binding| (binding.instance_id, binding.world));
    let expires = ctx.timestamp.to_micros_since_unix_epoch() + ADMISSION_MICROS;
    let request = bsatn::to_vec(&Admission {
        request: Request {
            id,
            actor,
            map,
            binding: binding.map_or(0, |(instance, _)| instance),
            trigger,
            x: pose[0],
            y: pose[1],
            z: pose[2],
            o: pose[3],
            expires,
        },
        binding_world: binding.and_then(|(_, world)| world),
    })
    .expect("Instance entry encodes");
    ctx.db.game_instance_entry().insert(InstanceEntry {
        id,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(ADMISSION_MICROS)).into(),
        guid,
        request: request.clone(),
    });
    ctx.db
        .game_realm_instance_entry()
        .insert(RealmInstanceEntry {
            id: 0,
            database: remote::Identity(realm),
            request,
        });
    Ok(())
}

fn decode(ctx: &ReducerContext, bytes: &[u8]) -> Result<Admission, String> {
    let admission = admission(bytes)?;
    let request = &admission.request;
    if ctx.timestamp.to_micros_since_unix_epoch() >= request.expires {
        return Err("Instance admission expired".into());
    }
    if !instance::is_dungeon_map(request.map)
        || ![request.x, request.y, request.z, request.o]
            .iter()
            .all(|value| value.is_finite())
    {
        return Err("invalid dungeon destination".into());
    }
    Ok(admission)
}

#[table(accessor = game_world_instance_entry, outbox(remote::idc_world_instance_entry))]
pub struct WorldInstanceEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub source: remote::Identity,
    pub request: Vec<u8>,
    pub party: Vec<u8>,
}

#[reducer]
pub fn idc_realm_instance_entry(ctx: &ReducerContext, request: Vec<u8>) -> Result<(), String> {
    directory::require_world(ctx)?;
    let admission = decode(ctx, &request)?;
    let decoded = admission.request;
    if realm_party::current(ctx, &decoded.actor) != Some(ctx.sender()) {
        return Err("World Session changed".into());
    }
    let bound_world = (decoded.binding != 0)
        .then(|| {
            admission.binding_world.or_else(|| {
                ctx.db
                    .game_map_owner()
                    .map_id()
                    .find(decoded.map)
                    .map(|owner| owner.world)
            })
        })
        .flatten();
    let world =
        super::instance_allocation::select(ctx, decoded.actor.guid, decoded.map, bound_world)?;
    let party = bsatn::to_vec(&AdmissionParty {
        group: crate::group::group_of(ctx, decoded.actor.guid).map_or(0, |member| member.group_id),
        snapshot: realm_party::snapshot(ctx, decoded.actor.guid),
    })
    .expect("Instance admission Group encodes");
    ctx.db
        .game_world_instance_entry()
        .insert(WorldInstanceEntry {
            id: 0,
            database: remote::Identity(world),
            source: remote::Identity(ctx.sender()),
            request,
            party,
        });
    Ok(())
}

#[table(accessor = game_instance_entry_lease, scheduled(expire_instance_entry_lease))]
pub struct InstanceEntryLease {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub source: Identity,
    pub request_id: u64,
    pub guid: u64,
    pub connection: u128,
    pub instance_id: u64,
    pub map_id: u32,
    pub expires: i64,
}
crate::virtual_clock_exempt!(game_instance_entry_lease);

#[table(accessor = game_instance_entry_answer, outbox(remote::idc_instance_entry_answer))]
pub struct InstanceEntryAnswer {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub request: Vec<u8>,
    pub instance_id: u64,
    pub error: String,
}

crate::outside_table_catalog!(
    game_instance_entry,
    game_realm_instance_entry,
    game_world_instance_entry,
    game_instance_entry_lease,
    game_instance_entry_answer
);

#[reducer]
pub fn idc_world_instance_entry(
    ctx: &ReducerContext,
    source: Identity,
    request: Vec<u8>,
    party: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let admission = decode(ctx, &request)?;
    let decoded = admission.request;
    if !directory::owns_map(ctx, decoded.map)
        || (!crate::peer::is_peer(ctx, source) && source != ctx.database_identity())
    {
        return Err("Instance Shard routing changed".into());
    }
    let guid = decoded.actor.guid;
    if let Ok(party) = bsatn::from_slice::<AdmissionParty>(&party) {
        realm_party::apply_snapshot(ctx, &party.snapshot)?;
        if crate::group::group_of(ctx, guid).map_or(0, |member| member.group_id) != party.group {
            return Err("Group changed during Instance admission".into());
        }
    } else {
        realm_party::apply_snapshot(ctx, &party)?;
    }
    if source == ctx.database_identity() {
        if !ctx
            .db
            .game_instance_entry()
            .id()
            .find(decoded.id)
            .is_some_and(|pending| pending.guid == guid && pending.request == request)
        {
            return Err("Instance admission is no longer pending".into());
        }
        if decoded.actor.connection == 0 {
            super::package_relocation::validate_admission(ctx, decoded.id)?;
        } else if !ctx
            .db
            .game_native_player()
            .connection_id()
            .find(ConnectionId::from_u128(decoded.actor.connection))
            .is_some_and(|player| player.character_guid == guid && player.teleport.is_none())
        {
            return Err("World Session changed".into());
        }
        if crate::taxi::is_in_flight(ctx, guid)
            || super::realm_petition::pending(ctx, guid)
            || super::realm_auction::pending(ctx, guid)
        {
            return Err("Character cannot enter an Instance yet".into());
        }
        let trigger = ctx
            .db
            .game_area_trigger()
            .id()
            .find(decoded.trigger)
            .ok_or("Instance entrance is missing")?;
        let actor = super::available_entity(ctx, guid).map_err(|error| error.to_string())?;
        if actor.map_id != trigger.map_id
            || !crate::areatrigger::AreaTriggerVolume::of(&trigger).contains_client_position(
                [trigger.x, trigger.y, trigger.z],
                [actor.x, actor.y, actor.z],
            )
        {
            return Err("Character left the Instance entrance".into());
        }
    } else if crate::helpers::character_by_guid(ctx, guid).is_some()
        || crate::transfer::is_in_transit(ctx, guid)
    {
        return Err("destination already holds this Character".into());
    }
    let binding_world = admission.binding_world.or_else(|| {
        ctx.db
            .game_map_owner()
            .map_id()
            .find(decoded.map)
            .map(|owner| owner.world)
    });
    if decoded.binding != 0 && binding_world == Some(ctx.database_identity()) {
        instance::bind_character(
            ctx,
            guid,
            decoded.binding,
            decoded.map,
            ctx.database_identity(),
        );
    }
    let instance_id = instance::resolve_or_create_instance(ctx, guid, decoded.map)?;
    if let Some(mut instance) = ctx.db.game_instance().instance_id().find(instance_id) {
        instance.last_empty_at_micros = 0;
        ctx.db.game_instance().instance_id().update(instance);
    }
    // Bindings travel with the Character; the admission lease protects this allocation until import.
    ctx.db
        .game_instance_binding()
        .by_character()
        .filter(guid)
        .filter(|binding| binding.map_id == decoded.map)
        .collect::<Vec<_>>()
        .into_iter()
        .for_each(|binding| {
            ctx.db.game_instance_binding().id().delete(binding.id);
        });
    ctx.db
        .game_instance_entry_lease()
        .insert(InstanceEntryLease {
            id: 0,
            scheduled_at: spacetimedb::Timestamp::from_micros_since_unix_epoch(decoded.expires)
                .into(),
            source,
            request_id: decoded.id,
            guid,
            connection: decoded.actor.connection,
            instance_id,
            map_id: decoded.map,
            expires: decoded.expires,
        });
    if source == ctx.database_identity() {
        finish(ctx, &request, instance_id, "", Some(source))?;
        return Ok(());
    }
    ctx.db
        .game_instance_entry_answer()
        .insert(InstanceEntryAnswer {
            id: 0,
            database: remote::Identity(source),
            request,
            instance_id,
            error: String::new(),
        });
    Ok(())
}

#[reducer(on_result(game_world_instance_entry))]
pub fn world_instance_entry_answered(
    ctx: &ReducerContext,
    call: WorldInstanceEntry,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        ctx.db
            .game_instance_entry_answer()
            .insert(InstanceEntryAnswer {
                id: 0,
                database: call.source,
                request: call.request,
                instance_id: 0,
                error,
            });
    }
}

#[reducer(on_result(game_realm_instance_entry))]
pub fn realm_instance_entry_answered(
    ctx: &ReducerContext,
    call: RealmInstanceEntry,
    result: Result<(), String>,
) -> Result<(), String> {
    if let Err(error) = result {
        finish(ctx, &call.request, 0, &error, None)?;
    }
    Ok(())
}

#[reducer]
pub fn idc_instance_entry_answer(
    ctx: &ReducerContext,
    request: Vec<u8>,
    instance_id: u64,
    error: String,
) -> Result<(), String> {
    if !crate::peer::is_peer(ctx, ctx.sender()) || realm_shard(ctx).is_none() {
        return Err("caller is not a Peer Shard".into());
    }
    let decoded = admission(&request)?.request;
    if instance_id == 0 {
        require_sender(ctx, realm_shard(ctx))?;
    } else if !super::instance_allocation::hosts(ctx, decoded.map, ctx.sender()) {
        return Err("Instance reply is from another map's Shard".into());
    }
    finish(ctx, &request, instance_id, &error, Some(ctx.sender()))?;
    Ok(())
}

fn finish(
    ctx: &ReducerContext,
    request: &[u8],
    instance_id: u64,
    error: &str,
    world: Option<Identity>,
) -> Result<(), String> {
    let decoded = admission(request)?.request;
    if decoded.actor.connection != 0 {
        finish_native(ctx, request, instance_id, error, world);
    } else if ctx
        .db
        .game_instance_entry()
        .id()
        .find(decoded.id)
        .is_some_and(|pending| pending.request == request)
    {
        if ctx.timestamp.to_micros_since_unix_epoch() >= decoded.expires {
            super::package_relocation::expired(ctx, decoded.id);
        } else {
            super::package_relocation::admitted(ctx, decoded.id, world, instance_id, error)?;
        }
        ctx.db.game_instance_entry().id().delete(decoded.id);
    }
    Ok(())
}

fn finish_native(
    ctx: &ReducerContext,
    request: &[u8],
    instance_id: u64,
    error: &str,
    world: Option<Identity>,
) {
    let Ok(decoded) = decode(ctx, request) else {
        return;
    };
    let decoded = decoded.request;
    let Some(pending) = ctx
        .db
        .game_instance_entry()
        .id()
        .find(decoded.id)
        .filter(|pending| pending.request == request)
    else {
        return;
    };
    ctx.db.game_instance_entry().id().delete(pending.id);
    let connection = ConnectionId::from_u128(decoded.actor.connection);
    if !ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .is_some_and(|player| player.character_guid == pending.guid && player.teleport.is_none())
    {
        return;
    }
    if !error.is_empty()
        || world.is_some_and(|world| {
            world != ctx.database_identity() && !crate::peer::is_peer(ctx, world)
        })
        || crate::taxi::is_in_flight(ctx, pending.guid)
        || super::realm_petition::pending(ctx, pending.guid)
        || super::realm_auction::pending(ctx, pending.guid)
    {
        notice(ctx, connection);
        return;
    }
    let Some(trigger) = ctx.db.game_area_trigger().id().find(decoded.trigger) else {
        return;
    };
    let Ok(actor) = super::available_entity(ctx, pending.guid) else {
        return;
    };
    if actor.map_id != trigger.map_id
        || !crate::areatrigger::AreaTriggerVolume::of(&trigger).contains_client_position(
            [trigger.x, trigger.y, trigger.z],
            [actor.x, actor.y, actor.z],
        )
    {
        return;
    }
    instance::bind_character(
        ctx,
        pending.guid,
        instance_id,
        decoded.map,
        world.unwrap_or(ctx.database_identity()),
    );
    let (x, y, z, o) = (decoded.x, decoded.y, decoded.z, decoded.o);
    if let Err(refusal) =
        crate::world::teleport(ctx, pending.guid, decoded.map, instance_id, [x, y, z, o])
    {
        spacetimedb::log::warn!(
            "instance entry: {} admitted to instance {instance_id} but teleport refused: {refusal}",
            pending.guid
        );
        return;
    }
    if world == Some(ctx.database_identity()) && actor.map_id == decoded.map {
        arrived(ctx, pending.guid, decoded.map, instance_id);
    }
}

fn notice(ctx: &ReducerContext, connection: ConnectionId) {
    let _ = super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
            notification: "Unable to enter that dungeon. Please try again.".into(),
        })),
        false,
    );
}

#[reducer]
pub fn expire_instance_entry(ctx: &ReducerContext, entry: InstanceEntry) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    if let Some(pending) = ctx.db.game_instance_entry().id().find(entry.id) {
        ctx.db.game_instance_entry().id().delete(entry.id);
        if let Ok(admission) = admission(&pending.request) {
            let request = admission.request;
            if request.actor.connection == 0 {
                super::package_relocation::expired(ctx, request.id);
                return;
            }
            let connection = ConnectionId::from_u128(request.actor.connection);
            if ctx
                .db
                .game_native_player()
                .connection_id()
                .find(connection)
                .is_some_and(|player| player.character_guid == pending.guid)
            {
                notice(ctx, connection);
            }
        }
    }
}

#[reducer]
pub fn expire_instance_entry_lease(ctx: &ReducerContext, lease: InstanceEntryLease) {
    if crate::time::scheduler_authorized(ctx) {
        ctx.db.game_instance_entry_lease().id().delete(lease.id);
    }
}

pub(super) fn imported(
    ctx: &ReducerContext,
    guid: u64,
    connection: u128,
    map: u32,
    instance_id: u64,
    request_id: Option<u64>,
) -> Result<(), String> {
    if !instance::is_dungeon_map(map) {
        return Ok(());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let lease = ctx
        .db
        .game_instance_entry_lease()
        .iter()
        .find(|lease| {
            lease.source == ctx.sender()
                && request_id.is_none_or(|id| lease.request_id == id)
                && lease.guid == guid
                && lease.connection == connection
                && lease.map_id == map
                && lease.instance_id == instance_id
                && lease.expires > now
        })
        .ok_or("dungeon admission is missing or expired")?;
    if !ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_some_and(|instance| instance.map_id == map && !instance.reset_requested)
    {
        return Err("dungeon Instance has expired".into());
    }
    realm_party::restore(ctx, guid);
    instance::bind_character(ctx, guid, instance_id, map, ctx.database_identity());
    ctx.db.game_instance_entry_lease().id().delete(lease.id);
    Ok(())
}

pub(super) fn arrived(ctx: &ReducerContext, guid: u64, map: u32, instance: u64) {
    let leases = ctx.db.game_instance_entry_lease();
    let ids: Vec<_> = leases
        .iter()
        .filter(|lease| {
            lease.source == ctx.database_identity()
                && lease.guid == guid
                && lease.map_id == map
                && lease.instance_id == instance
        })
        .map(|lease| lease.id)
        .collect();
    for id in ids {
        leases.id().delete(id);
    }
}

pub(crate) fn occupied(ctx: &ReducerContext) -> Vec<u64> {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    ctx.db
        .game_instance_entry_lease()
        .iter()
        .filter(|lease| lease.expires > now)
        .map(|lease| lease.instance_id)
        .collect()
}

pub(super) fn return_from_reaped(
    ctx: &ReducerContext,
    connection: &super::NativeWorldConnection,
    guid: u64,
    entry: crate::character::WorldEntry,
    teleport_counter: u32,
) -> Result<bool, String> {
    use crate::game_character;
    let character =
        crate::helpers::character_by_guid(ctx, guid).ok_or("Character no longer exists")?;
    let character_account = character.account_id;
    let origin = super::teleport::NativeTeleportOrigin {
        map_id: character.map_id,
        instance_id: character.pending_instance_id,
        zone_id: character.zone_id,
        x: character.x,
        y: character.y,
        z: character.z,
        orientation: character.orientation,
    };
    let mut character = character;
    crate::character::restore_reaped_instance(ctx, &mut character);
    if character.map_id != origin.map_id
        && directory::transfer_target(ctx, character.map_id).is_some()
    {
        ctx.db.game_character().guid().update(character);
        if super::transfer::begin_character(
            ctx,
            connection,
            character_account,
            guid,
            teleport_counter,
            Some(origin),
            entry == crate::character::WorldEntry::WorldPort,
        )? {
            return Ok(true);
        }
        return Err("Instance return has no destination World Shard".into());
    }
    Ok(false)
}
