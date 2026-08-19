//! The TRUSTED GATEWAY verb surface (#468 stage 4a).
//!
//! One reducer family — `gw_<verb>(ctx, actor_guid, ...)` — for a gateway that holds a SHARED
//! SpacetimeDB connection instead of one connection per player. The sender-shaped player reducers
//! resolve "who is acting" from `ctx.sender()`; these resolve it from an explicit `actor_guid`,
//! because on a shared connection every call arrives from the same identity.
//!
//! Design rules (mirroring `actor.rs`, which this surface consumes):
//! - **Every reducer's first act is `require_operator`.** That is the entire trust model: the
//!   shared connection's identity is the claimed operator, and a direct anonymous SpacetimeDB
//!   client that bypasses the gateway is refused before any actor resolution happens. The
//!   self-scan test at the bottom of this file pins this ordering for every reducer here.
//! - **The actor resolves through [`crate::helpers::acting_entity_by_guid`]** — the guid-keyed
//!   twin of `entity_by_owner`, carrying the SAME in-transit transfer fence. Never `live_entity`
//!   (it skips the fence) and never a bare `.guid().find(...)`.
//! - **No behavior**: each verb delegates to the same core its sender-shaped sibling calls
//!   (`world::apply_movement_update`, the `actor.rs` verbs, ...). Gates live in the cores and
//!   cannot drift between the two entries.
//!
//! These are THE player-verb surface (#483): the sender-shaped (`ctx.sender`-authorized) twins
//! are deleted, and every player action reaches the module through a `gw_*` verb on the
//! gateway's privileged connection.

use spacetimedb::{reducer, table, Identity, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::game_account;
use crate::game_world_entity;
use crate::helpers::{acting_entity_by_guid, require_operator};

// ===========================================================================================
//  The gateway lease — bounded ghost lifetime for shared-connection sessions [server]
// ===========================================================================================
//
// THE #468 sharp edge: under one shared connection, a gateway crash fires ONE
// `client_disconnected` instead of one per player, so nothing per-player tears world entities
// down and a crash would leave every seated player as a permanent world ghost. The fix is a
// LEASE: each gateway heartbeats its row; sessions opened through the shared connection bind
// their entity to the lease (stage 4d writes `GatewaySession` rows at `gw_player_login`); a
// scheduled reaper removes the world entities of any lease that stops heartbeating.
//
// Deliberately TTL-ONLY — no sweep on the shared connection's own disconnect. The coordinator
// connection reconnects routinely (module republish closes every websocket, #278), and a
// disconnect-triggered sweep would mass-despawn every player on a live gateway that is merely
// reconnecting. A dead gateway simply stops heartbeating and lapses; ghost lifetime is bounded
// by TTL + reap interval (~75 s worst case) no matter HOW the gateway died.

/// One row per live gateway process, keyed by its operator identity. Private. [server]
#[table(accessor = game_gateway_lease)]
pub struct GatewayLease {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub identity: Identity,
    pub last_heartbeat: Timestamp,
}

/// entity_guid → lease binding for a session riding the shared connection. Written by
/// `gw_player_login` (stage 4d); until then the table is empty and the reaper below is inert.
/// Private. [server]
#[table(accessor = game_gateway_session)]
pub struct GatewaySession {
    #[primary_key]
    pub entity_guid: u64,
    pub lease_id: u64,
}

/// The reaper's schedule row — armed by `seed::init` on a fresh database and ensured by
/// `debug_repair_after_publish` on every publish (the #465 lesson: `init` never re-runs on an
/// auto-migrate republish). [server]
#[table(accessor = game_gateway_lease_reaper_schedule, scheduled(reap_gateway_leases))]
pub struct GatewayLeaseReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// How often the reaper checks leases.
pub(crate) const LEASE_REAP_MICROS: i64 = 15_000_000;
/// How long a lease survives without a heartbeat. Three missed 15 s heartbeats plus slack: a
/// gateway that cannot heartbeat for a full minute is not serving its players anyway.
pub(crate) const LEASE_TTL_MICROS: i64 = 60_000_000;

/// The reap decision, pure: a lease is dead when its last heartbeat is older than the TTL.
pub(crate) fn lease_expired(now_micros: i64, heartbeat_micros: i64, ttl_micros: i64) -> bool {
    now_micros - heartbeat_micros > ttl_micros
}

/// Upsert the calling gateway's lease. The gateway calls this every ~15 s on its shared
/// connection; the FIRST call creates the lease (world entry before the first heartbeat is
/// impossible — `gw_player_login` will resolve the lease by `ctx.sender()`).
#[reducer]
pub fn gw_heartbeat(ctx: &ReducerContext) -> Result<(), String> {
    require_operator(ctx)?;
    let leases = ctx.db.game_gateway_lease();
    match leases.identity().find(ctx.sender()) {
        Some(mut lease) => {
            lease.last_heartbeat = ctx.timestamp;
            leases.id().update(lease);
        }
        None => {
            leases.insert(GatewayLease {
                id: 0,
                identity: ctx.sender(),
                last_heartbeat: ctx.timestamp,
            });
        }
    }
    Ok(())
}

/// Remove the world entities of every lapsed lease, then the lease itself. Scheduled — the
/// sender guard is the standard "only the scheduler may fire this" fence every scheduled
/// reducer here carries.
#[reducer]
pub fn reap_gateway_leases(
    ctx: &ReducerContext,
    _schedule: GatewayLeaseReaperSchedule,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    // Collect first: the loops delete from the tables they walk.
    let dead: Vec<GatewayLease> = ctx
        .db
        .game_gateway_lease()
        .iter()
        .filter(|l| {
            lease_expired(
                now,
                l.last_heartbeat.to_micros_since_unix_epoch(),
                LEASE_TTL_MICROS,
            )
        })
        .collect();
    for lease in dead {
        let sessions: Vec<GatewaySession> = ctx
            .db
            .game_gateway_session()
            .iter()
            .filter(|s| s.lease_id == lease.id)
            .collect();
        spacetimedb::log::warn!(
            "gateway lease {} lapsed (no heartbeat for >{}s) — despawning {} leased session(s)",
            lease.id,
            LEASE_TTL_MICROS / 1_000_000,
            sessions.len()
        );
        for session in sessions {
            // Resolve the entity's owner identity so the removal takes the SAME logout path a
            // per-player disconnect takes (hooks fire, pets despawn, corpse rules apply).
            if let Some(entity) = ctx.db.game_world_entity().guid().find(session.entity_guid) {
                crate::world::remove_from_world(ctx, entity.owner_identity);
            }
            ctx.db
                .game_gateway_session()
                .entity_guid()
                .delete(session.entity_guid);
        }
        ctx.db.game_gateway_lease().id().delete(lease.id);
    }
    Ok(())
}

/// Resolve the acting entity for a gateway verb, with the sender path's exact error string —
/// `is_desync_error` on the gateway classifies on "not in world", and the two entries must be
/// indistinguishable to that classifier.
fn actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor = acting_entity_by_guid(ctx, actor_guid)
        .ok_or_else(|| "mover not in world".to_string())?;
    crate::taxi::reject_action_while_in_flight(ctx, actor_guid)?;
    Ok(actor)
}

/// [`crate::world::movement_update`] with the mover named by guid — the movement heartbeat, the
/// hottest call on the shared connection.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_movement_update(
    ctx: &ReducerContext,
    actor_guid: u64,
    opcode: u16,
    movement_info: Vec<u8>,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    move_time_ms: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    if crate::taxi::movement_is_suppressed(ctx, actor_guid) {
        return Ok(());
    }
    let mover = actor(ctx, actor_guid)?;
    crate::world::apply_movement_update(ctx, mover, opcode, movement_info, x, y, z, o, move_time_ms)
}

/// One movement update inside a [`gw_movement_batch`] call (#482). Field-for-field the
/// `gw_movement_update` argument list with the actor made explicit per entry.
#[derive(spacetimedb::SpacetimeType)]
pub struct GwMove {
    pub actor_guid: u64,
    pub opcode: u16,
    pub movement_info: Vec<u8>,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
    pub move_time_ms: u32,
}

/// Apply a batch entry-by-entry. Suppressed and failed entries are isolated from their neighbors.
fn apply_movement_batch(
    moves: Vec<GwMove>,
    mut suppressed: impl FnMut(u64) -> bool,
    mut apply: impl FnMut(GwMove) -> Result<(), String>,
) {
    for movement in moves {
        if suppressed(movement.actor_guid) {
            continue;
        }
        let actor_guid = movement.actor_guid;
        if let Err(error) = apply(movement) {
            spacetimedb::log::debug!(
                "gw_movement_batch: move for {} rejected: {error}",
                actor_guid
            );
        }
    }
}

/// #482: ONE transaction per gateway tick instead of one per player heartbeat. The measured wall
/// behind this: ~10k `gw_movement_update` transactions/s at ~43µs of per-transaction machinery
/// each put the writer at 92% while the actual row work (the batched `publish_motion` side) was
/// a fraction of that. The gateway collects a tick's worth of movement (25–50ms) and sends it
/// here; each entry runs the SAME `apply_movement_update` core with the same per-entry
/// validation. A failing entry (mover despawned mid-tick, NaN guard, in-transit fence) is LOGGED
/// and skipped — one bad move must never roll back the other few hundred players' tick.
#[reducer]
pub fn gw_movement_batch(ctx: &ReducerContext, moves: Vec<GwMove>) -> Result<(), String> {
    require_operator(ctx)?;
    apply_movement_batch(
        moves,
        |actor_guid| crate::taxi::movement_is_suppressed(ctx, actor_guid),
        |m| {
            let mover = actor(ctx, m.actor_guid)?;
            crate::world::apply_movement_update(
                ctx,
                mover,
                m.opcode,
                m.movement_info,
                m.x,
                m.y,
                m.z,
                m.o,
                m.move_time_ms,
            )
        },
    );
    // #482: drain the staged motion INSIDE this transaction — the subscription sweep is paid
    // once per batch either way (#461's whole point), and inline draining removes the publish
    // schedule's 50ms latency quantum plus the pending-row churn for this path.
    crate::motion::publish_staged(ctx);
    Ok(())
}

// ===========================================================================================
//  Action verbs — thin gated wrappers over the actor.rs cores
// ===========================================================================================

/// [`crate::actor::attack`] behind the gateway gate — melee auto-attack engage.
#[reducer]
pub fn gw_attack(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::attack(ctx, actor_guid, target_guid)
}

/// [`crate::actor::cast_at`] behind the gateway gate — the full cast core with every gate.
#[reducer]
pub fn gw_cast_at(
    ctx: &ReducerContext,
    actor_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::cast_at(ctx, actor_guid, spell_id, target_guid)
}

/// [`crate::spell::scheduler::apply_cast_spell_at`] behind the gateway gate — the GROUND-TARGET
/// cast form (`CMSG`-routed `cast_spell_at`): spellbook + NaN + self-target-default gates, target
/// coords carried. Distinct from [`gw_cast_at`], which is the unit-target instant core (#479).
#[reducer]
pub fn gw_cast_spell_at(
    ctx: &ReducerContext,
    actor_guid: u64,
    spell_id: u32,
    target_guid: u64,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let caster = actor(ctx, actor_guid)?;
    crate::spell::apply_cast_spell_at(ctx, &caster, spell_id, target_guid, x, y, z)
}

/// [`crate::actor::take_loot`] behind the gateway gate — one loot slot off a corpse/GO.
#[reducer]
pub fn gw_take_loot(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::take_loot(ctx, actor_guid, corpse_guid, loot_slot)
}

/// [`crate::actor::ranged_attack`] behind the gateway gate — Auto Shot / wand engage.
#[reducer]
pub fn gw_ranged_attack(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::ranged_attack(ctx, actor_guid, target_guid, spell_id)
}

/// [`crate::actor::stop_attack`] behind the gateway gate — disarm the actor's outgoing swing.
#[reducer]
pub fn gw_stop_attack(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::stop_attack(ctx, actor_guid)
}

/// [`crate::world::apply_set_sheathed`] behind the gateway gate — the `CMSG_SETSHEATHED` a client
/// sends on `Z`. `state` is raw client input and is range-checked in the apply fn, not here. [#101]
#[reducer]
pub fn gw_set_sheathed(ctx: &ReducerContext, actor_guid: u64, state: u8) -> Result<(), String> {
    require_operator(ctx)?;
    let actor = actor(ctx, actor_guid)?;
    crate::world::apply_set_sheathed(ctx, actor, state)
}

/// [`crate::actor::accept_quest`] behind the gateway gate.
#[reducer]
pub fn gw_accept_quest(
    ctx: &ReducerContext,
    actor_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::accept_quest(ctx, actor_guid, giver_guid, quest_entry)
}

/// [`crate::actor::turn_in_quest`] behind the gateway gate.
#[reducer]
pub fn gw_turn_in_quest(
    ctx: &ReducerContext,
    actor_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::turn_in_quest(ctx, actor_guid, giver_guid, quest_entry, reward_index)
}

/// [`crate::actor::use_item`] behind the gateway gate.
#[reducer]
pub fn gw_use_item(ctx: &ReducerContext, actor_guid: u64, slot: u8) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::use_item(ctx, actor_guid, slot)
}

/// Resolve a spell whose explicit target is an owned inventory item. The effect kind selects the
/// operation inside the module; the gateway only translates the item guid to a slot.
#[reducer]
pub fn gw_cast_item_target(
    ctx: &ReducerContext,
    actor_guid: u64,
    spell_id: u32,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::cast_item_target(ctx, actor_guid, spell_id, slot)
}

/// [`crate::actor::loot_money`] behind the gateway gate.
#[reducer]
pub fn gw_loot_money(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::loot_money(ctx, actor_guid, target_guid)
}

/// [`crate::actor::buy_item`] behind the gateway gate.
#[reducer]
pub fn gw_buy_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::buy_item(ctx, actor_guid, vendor_guid, item_entry, count)
}

/// [`crate::actor::sell_item`] behind the gateway gate.
#[reducer]
pub fn gw_sell_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::sell_item(ctx, actor_guid, vendor_guid, slot)
}

/// [`crate::actor::equip_item`] behind the gateway gate.
#[reducer]
pub fn gw_equip_item(ctx: &ReducerContext, actor_guid: u64, from_slot: u8) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::equip_item(ctx, actor_guid, from_slot)
}

/// [`crate::actor::use_gameobject`] behind the gateway gate.
#[reducer]
pub fn gw_use_gameobject(
    ctx: &ReducerContext,
    actor_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::use_gameobject(ctx, actor_guid, go_guid)
}

/// [`crate::actor::trainer_buy`] behind the gateway gate.
#[reducer]
pub fn gw_trainer_buy(
    ctx: &ReducerContext,
    actor_guid: u64,
    trainer_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::trainer_buy(ctx, actor_guid, trainer_guid, spell_id)
}

/// [`crate::actor::respond_resurrect`] behind the gateway gate — accept/decline a pending rez.
#[reducer]
pub fn gw_respond_resurrect(
    ctx: &ReducerContext,
    actor_guid: u64,
    accept: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::respond_resurrect(ctx, actor_guid, accept)
}

/// [`crate::actor::repop`] behind the gateway gate — a dead actor releases to the graveyard.
#[reducer]
pub fn gw_repop(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::repop(ctx, actor_guid)
}

/// [`crate::actor::spirit_res`] behind the gateway gate — ghost res at the spirit healer.
#[reducer]
pub fn gw_spirit_res(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::spirit_res(ctx, actor_guid)
}

/// [`crate::actor::accept_group_invite`] behind the gateway gate.
#[reducer]
pub fn gw_accept_group_invite(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::actor::accept_group_invite(ctx, actor_guid)
}

/// [`crate::world::set_target`] with the actor named by guid — `CMSG_SET_SELECTION`, second
/// hottest call after movement.
#[reducer]
pub fn gw_set_target(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let player = actor(ctx, actor_guid)?;
    crate::world::apply_set_target(ctx, player, target_guid)
}

/// [`crate::chat::send_chat`] with the speaker named by guid — SAY/YELL.
#[reducer]
pub fn gw_send_chat(
    ctx: &ReducerContext,
    actor_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_send_chat(ctx, sender, chat_type, language, message)
}

/// [`crate::chat::send_emote`] with the emoter named by guid.
#[reducer]
pub fn gw_send_emote(
    ctx: &ReducerContext,
    actor_guid: u64,
    text_emote: u32,
    emote_anim: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_send_emote(ctx, sender, text_emote, emote_anim, target_guid)
}

/// The shared-connection login (#468 stage 4d): enter the world with the account named by id and
/// the session bound to this gateway's lease. Delegates to the same [`crate::world::apply_player_login`]
/// core the sender path uses; the OWNER identity stamped onto the entity and the character's
/// owner-RLS rows is the account's BOUND identity (from `establish_session`), so a per-player
/// connection presenting that identity still sees everything it would have — mixed-mode safe.
///
/// Ordering contracts, both fail-closed:
/// - the account must have a bound identity (`establish_session` ran) — a gateway cannot log in an
///   account that never authenticated;
/// - this gateway must hold a lease (`gw_heartbeat` ran) — a login that could not be bound would
///   be a ghost the reaper can never see, which is the one unbounded-ghost hole this surface
///   exists to close.
#[reducer]
pub fn gw_player_login(
    ctx: &ReducerContext,
    account_id: u64,
    character_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or_else(|| format!("no such account: {account_id}"))?;
    let owner = account.identity.ok_or_else(|| {
        "account has no bound identity — establish_session must run before a gateway login"
            .to_string()
    })?;
    let lease = ctx
        .db
        .game_gateway_lease()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| {
            "no lease for this gateway — gw_heartbeat must run before logins".to_string()
        })?;
    crate::world::apply_player_login(ctx, &account, character_guid, owner)?;
    // Bind AFTER the login succeeds: a refused login must not leave a session row for the reaper
    // to chase. Upsert — a ghost-relog through the gateway rebinds the same guid.
    let sessions = ctx.db.game_gateway_session();
    let row = GatewaySession {
        entity_guid: character_guid,
        lease_id: lease.id,
    };
    if sessions.entity_guid().find(character_guid).is_some() {
        sessions.entity_guid().update(row);
    } else {
        sessions.insert(row);
    }
    Ok(())
}

/// The shared-connection logout: remove the actor from the world through the SAME path a
/// per-player disconnect takes, and unbind its lease session so the reaper never revisits it.
/// Resolves the entity WITHOUT the in-transit fence — a leave must always succeed, and a
/// mid-transfer character has no live entity to remove anyway (the lookup just misses).
#[reducer]
pub fn gw_leave_world(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    if let Some(entity) = ctx.db.game_world_entity().guid().find(actor_guid) {
        crate::world::remove_from_world(ctx, entity.owner_identity);
    }
    ctx.db.game_gateway_session().entity_guid().delete(actor_guid);
    Ok(())
}

// ===========================================================================================
//  Chat / social (#479 batch B)
// ===========================================================================================

/// [`crate::chat::apply_send_roll`] with the roller named by guid — `MSG_RANDOM_ROLL`.
#[reducer]
pub fn gw_send_roll(
    ctx: &ReducerContext,
    actor_guid: u64,
    min_roll: u32,
    max_roll: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let roller = actor(ctx, actor_guid)?;
    crate::chat::apply_send_roll(ctx, roller, min_roll, max_roll)
}

/// [`crate::chat::apply_send_whisper`] with the speaker named by guid — the SHARD whisper plane.
#[reducer]
pub fn gw_send_whisper(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_name: String,
    message: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_send_whisper(ctx, sender, target_name, message)
}

/// [`crate::chat::apply_party_chat`] with the speaker named by guid — `/p`.
#[reducer]
pub fn gw_party_chat(ctx: &ReducerContext, actor_guid: u64, text: String) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_party_chat(ctx, sender, text)
}

/// [`crate::chat::apply_join_channel`] with the joiner named by guid.
#[reducer]
pub fn gw_join_channel(
    ctx: &ReducerContext,
    actor_guid: u64,
    channel: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_join_channel(ctx, sender, channel)
}

/// [`crate::chat::apply_leave_channel`] with the leaver named by guid.
#[reducer]
pub fn gw_leave_channel(
    ctx: &ReducerContext,
    actor_guid: u64,
    channel: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_leave_channel(ctx, sender, channel)
}

/// [`crate::chat::apply_send_channel_message`] with the speaker named by guid.
#[reducer]
pub fn gw_send_channel_message(
    ctx: &ReducerContext,
    actor_guid: u64,
    channel: String,
    message: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_send_channel_message(ctx, sender, channel, message)
}

/// [`crate::chat::add_contact`] (friend arm) with the owner named by guid.
#[reducer]
pub fn gw_add_friend(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::add_contact(ctx, sender, target_guid, false)
}

/// [`crate::chat::remove_contact`] (friend arm) with the owner named by guid.
#[reducer]
pub fn gw_del_friend(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::remove_contact(ctx, sender, target_guid, false)
}

/// [`crate::chat::add_contact`] (ignore arm) with the owner named by guid.
#[reducer]
pub fn gw_add_ignore(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::add_contact(ctx, sender, target_guid, true)
}

/// [`crate::chat::remove_contact`] (ignore arm) with the owner named by guid.
#[reducer]
pub fn gw_del_ignore(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::remove_contact(ctx, sender, target_guid, true)
}

// ===========================================================================================
//  Group (#479 batch B)
// ===========================================================================================

/// [`crate::group::invite_core`] with the inviter named by guid.
#[reducer]
pub fn gw_group_invite(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::group::invite_core(ctx, actor_guid, target_guid)
}

/// [`crate::group::decline_invite_for`] with the decliner named by guid.
#[reducer]
pub fn gw_group_decline(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::group::decline_invite_for(ctx, actor_guid)
}

/// [`crate::trade::apply_initiate_trade`] — propose a Trade Session against `target_guid` (#120).
/// Refusals are protocol answers on the trade-event relay, never an `Err` here.
#[reducer]
pub fn gw_initiate_trade(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_initiate_trade(ctx, acting, target_guid)
}

/// [`crate::trade::apply_begin_trade`] — the proposed target's client answered; open both windows.
#[reducer]
pub fn gw_begin_trade(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_begin_trade(ctx, acting)
}

/// [`crate::trade::apply_cancel_trade`] — tear the actor's Trade Session down, `TradeCanceled` to
/// both parties.
#[reducer]
pub fn gw_cancel_trade(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_cancel_trade(ctx, acting)
}

/// [`crate::trade::apply_set_trade_item`] — offer the item in absolute inventory slot `inv_slot`
/// in window slot `trade_slot` (#121). The gateway already mapped the client's (bag, slot) pair.
#[reducer]
pub fn gw_set_trade_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    trade_slot: u8,
    inv_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_set_trade_item(ctx, acting, trade_slot, inv_slot)
}

/// [`crate::trade::apply_clear_trade_item`] (#121).
#[reducer]
pub fn gw_clear_trade_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    trade_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_clear_trade_item(ctx, acting, trade_slot)
}

/// [`crate::trade::apply_set_trade_gold`] — `copper` is the offered amount (#121).
#[reducer]
pub fn gw_set_trade_gold(ctx: &ReducerContext, actor_guid: u64, copper: u32) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_set_trade_gold(ctx, acting, copper)
}

/// [`crate::trade::apply_accept_trade`] — accept the current offer; dual-accept runs the Trade
/// Commit (#122).
#[reducer]
pub fn gw_accept_trade(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_accept_trade(ctx, acting)
}

/// [`crate::trade::apply_unaccept_trade`] — withdraw an accept (#122).
#[reducer]
pub fn gw_unaccept_trade(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_unaccept_trade(ctx, acting)
}

/// [`crate::trade::apply_busy_trade`] — decline a proposal as busy (#123).
#[reducer]
pub fn gw_busy_trade(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_busy_trade(ctx, acting)
}

/// [`crate::trade::apply_ignore_trade`] — decline a proposal via ignore (#123).
#[reducer]
pub fn gw_ignore_trade(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_ignore_trade(ctx, acting)
}

/// The challenged character accepts the Duel identified by the client-supplied arbiter flag.
#[reducer]
pub fn gw_duel_accept(ctx: &ReducerContext, actor_guid: u64, flag_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::duel::accept_duel(ctx, actor_guid, flag_guid);
    Ok(())
}

/// Cancel the Duel identified by the client-supplied arbiter flag. Forged/stale flags are no-ops.
#[reducer]
pub fn gw_duel_cancel(ctx: &ReducerContext, actor_guid: u64, flag_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::duel::cancel_duel(ctx, actor_guid, flag_guid);
    Ok(())
}

/// [`crate::group::leave_group_for`] with the leaver named by guid.
#[reducer]
pub fn gw_group_leave(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::group::leave_group_for(ctx, actor_guid)
}

/// [`crate::group::uninvite_from_group`] with the leader named by guid.
#[reducer]
pub fn gw_group_uninvite(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::group::uninvite_from_group(ctx, actor_guid, target_guid)
}

/// [`crate::group::set_loot_method_for`] with the leader named by guid.
#[reducer]
pub fn gw_group_loot_method(
    ctx: &ReducerContext,
    actor_guid: u64,
    loot_setting: u8,
    master_guid: u64,
    loot_threshold: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::group::set_loot_method_for(
        ctx,
        actor_guid,
        loot_setting,
        master_guid,
        loot_threshold,
    )
}

/// [`crate::quest::apply_push_quest_to_party`] with the sharer named by guid.
#[reducer]
pub fn gw_push_quest_to_party(
    ctx: &ReducerContext,
    actor_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::quest::apply_push_quest_to_party(ctx, actor_guid, quest_entry)
}

// ===========================================================================================
//  Inventory / vendor / professions (#479 batch B)
// ===========================================================================================

/// [`crate::items::apply_item_move`] with the owner named by guid.
#[reducer]
pub fn gw_move_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_item_move(ctx, actor_guid, from_slot, to_slot)
}

/// [`crate::items::apply_unequip_item`] with the owner named by guid.
#[reducer]
pub fn gw_unequip_item(ctx: &ReducerContext, actor_guid: u64, from_slot: u8) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_unequip_item(ctx, actor_guid, from_slot)
}

/// [`crate::items::apply_auto_bank_item`] with the owner named by guid — right-click to bank / right-
/// click to withdraw, `CMSG_AUTOBANK_ITEM` / `CMSG_AUTOSTORE_BANK_ITEM`. The direction is inferred
/// from `slot` in the module.
#[reducer]
pub fn gw_auto_bank_item(ctx: &ReducerContext, actor_guid: u64, slot: u8) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_auto_bank_item(ctx, actor_guid, slot)
}

/// [`crate::items::apply_buyback_item`] with the buyer named by guid.
#[reducer]
pub fn gw_buyback_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_buyback_item(ctx, actor_guid, vendor_guid, slot)
}

/// [`crate::items::apply_player_repair`] with the owner named by guid — the NPC-gated repair.
#[reducer]
pub fn gw_repair_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    npc_guid: u64,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_player_repair(ctx, actor_guid, npc_guid, slot)
}

/// [`crate::items::apply_buy_bank_slot`] with the buyer named by guid. A refusal leads with its
/// `SMSG_BUY_BANK_SLOT_RESULT` code in brackets so the relay maps it by code, not by prose.
#[reducer]
pub fn gw_buy_bank_slot(
    ctx: &ReducerContext,
    actor_guid: u64,
    banker_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::buy_bank_slot_result(crate::items::apply_buy_bank_slot(
        ctx,
        actor_guid,
        banker_guid,
    ))
}

/// [`crate::professions::apply_disenchant`] with the enchanter named by guid.
#[reducer]
pub fn gw_disenchant(ctx: &ReducerContext, actor_guid: u64, slot: u8) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::professions::apply_disenchant(ctx, actor_guid, slot)
}

/// [`crate::professions::apply_enchant_item`] with the enchanter named by guid.
#[reducer]
pub fn gw_enchant_item(
    ctx: &ReducerContext,
    actor_guid: u64,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::professions::apply_enchant_item(ctx, actor_guid, target_slot, enchant_id)
}

/// [`crate::professions::apply_fish`] with the fisher named by guid.
#[reducer]
pub fn gw_fish(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::professions::apply_fish(ctx, actor_guid)
}

/// [`crate::world::set_home`] with the binder named by guid — the innkeeper hearth bind. The bind
/// names no NPC, so ungated it would let any client hearth anywhere on the map.
#[reducer]
pub fn gw_bind_home(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::items::innkeeper_access_gate(ctx, actor_guid)?;
    crate::world::set_home(ctx, actor_guid);
    Ok(())
}

// ===========================================================================================
//  Loot (#479 batch B)
// ===========================================================================================

/// [`crate::professions::skin_corpse`] with the skinner named by guid.
#[reducer]
pub fn gw_skin(ctx: &ReducerContext, actor_guid: u64, corpse_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::professions::skin_corpse(ctx, actor_guid, corpse_guid)
}

/// [`crate::loot::cast_vote_on`] with the voter named by guid — `CMSG_LOOT_ROLL`.
#[reducer]
pub fn gw_loot_roll(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
    loot_slot: u32,
    vote: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::loot::cast_vote_on(ctx, corpse_guid, loot_slot as u8, actor_guid, vote)
}

/// [`crate::loot::apply_master_give`] with the master looter named by guid.
#[reducer]
pub fn gw_loot_master_give(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::loot::apply_master_give(ctx, actor_guid, corpse_guid, loot_slot, target_guid)
}

// ===========================================================================================
//  Spells (#479 batch B)
// ===========================================================================================

/// [`crate::spell::do_cast_spell`] with the caster named by guid — the SPELLBOOK-GATED player cast
/// (`target_guid == 0` self-casts). Distinct from [`gw_cast_at`], which drives `resolve_cast_at`
/// directly and skips that gate.
#[reducer]
pub fn gw_cast_spell(
    ctx: &ReducerContext,
    actor_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let caster = actor(ctx, actor_guid)?;
    crate::spell::do_cast_spell(ctx, caster, spell_id, target_guid)
}

/// [`crate::spell::do_cancel_cast`] with the caster named by guid.
#[reducer]
pub fn gw_cancel_cast(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let caster = actor(ctx, actor_guid)?;
    crate::spell::do_cancel_cast(ctx, caster)
}

/// [`crate::spell::do_cancel_aura`] with the aura's owner named by guid.
#[reducer]
pub fn gw_cancel_aura(ctx: &ReducerContext, actor_guid: u64, spell_id: u32) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::spell::do_cancel_aura(ctx, actor_guid, spell_id)
}

// ===========================================================================================
//  World / misc (#479 batch B)
// ===========================================================================================

/// [`crate::world::apply_gossip_select`] with the clicker named by guid — the notify-only hook.
#[reducer]
pub fn gw_gossip_select(
    ctx: &ReducerContext,
    actor_guid: u64,
    npc_guid: u64,
    option_id: u32,
    option_row_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::world::apply_gossip_select(ctx, actor_guid, npc_guid, option_id, option_row_id)
}

/// [`crate::world::apply_inspect`] with the inspector named by guid.
#[reducer]
pub fn gw_inspect(ctx: &ReducerContext, actor_guid: u64, target_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let inspector = actor(ctx, actor_guid)?;
    crate::world::apply_inspect(ctx, inspector, target_guid)
}

/// [`crate::talent::do_learn_talent`] with the learner named by guid. The owner identity stamped on
/// the learned rows is the ACTOR's binding, never `ctx.sender()` (the shared connection's operator).
#[reducer]
pub fn gw_learn_talent(ctx: &ReducerContext, actor_guid: u64, talent_id: u32) -> Result<(), String> {
    require_operator(ctx)?;
    let learner = actor(ctx, actor_guid)?;
    crate::talent::do_learn_talent(ctx, actor_guid, learner.owner_identity, talent_id).map(|_| ())
}

/// [`crate::talent::do_reset_talents`] behind the gateway gate — the "I wish to unlearn my
/// talents." gossip option (work-item 198's respec primitive, wired to gossip by #516).
#[reducer]
pub fn gw_reset_talents(
    ctx: &ReducerContext,
    actor_guid: u64,
    trainer_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::talent::do_reset_talents(ctx, actor_guid, trainer_guid).map(|_| ())
}

/// [`crate::gameobject::apply_pick_lock`] with the picker named by guid.
#[reducer]
pub fn gw_pick_lock(ctx: &ReducerContext, actor_guid: u64, go_guid: u64) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::gameobject::apply_pick_lock(ctx, actor_guid, go_guid)
}

/// [`crate::action_bar::apply_set_action_button`] with the bar's owner named by guid.
#[reducer]
pub fn gw_set_action_button(
    ctx: &ReducerContext,
    actor_guid: u64,
    button: u8,
    action: u32,
    action_type: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let character = actor(ctx, actor_guid)?;
    crate::action_bar::apply_set_action_button(ctx, character, button, action, action_type)
}

/// [`crate::reputation::apply_set_faction_at_war`] with the rep pane's owner named by guid.
#[reducer]
pub fn gw_set_faction_at_war(
    ctx: &ReducerContext,
    actor_guid: u64,
    reputation_index: u32,
    at_war: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    let player = actor(ctx, actor_guid)?;
    crate::reputation::apply_set_faction_at_war(ctx, player, reputation_index, at_war)
}

/// [`crate::quest::apply_abandon_quest`] with the abandoner named by guid.
#[reducer]
pub fn gw_abandon_quest(
    ctx: &ReducerContext,
    actor_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::quest::apply_abandon_quest(ctx, actor_guid, quest_entry)
}

/// [`crate::corpse::apply_reclaim_corpse`] with the ghost named by guid. `_corpse_guid` is carried
/// for wire-shape parity with the sender reducer and ignored there too — the corpse is derived from
/// the reclaiming player, never the packet.
#[reducer]
pub fn gw_reclaim_corpse(
    ctx: &ReducerContext,
    actor_guid: u64,
    _corpse_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let player = actor(ctx, actor_guid)?;
    crate::corpse::apply_reclaim_corpse(ctx, player)
}

/// [`crate::quest::apply_enter_areatrigger`] with the entrant named by guid.
#[reducer]
pub fn gw_enter_areatrigger(
    ctx: &ReducerContext,
    actor_guid: u64,
    trigger_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::quest::apply_enter_areatrigger(ctx, actor_guid, trigger_id);
    Ok(())
}

/// [`crate::creatures::apply_pet_command`] with the pet's owner named by guid.
#[reducer]
pub fn gw_pet_command(
    ctx: &ReducerContext,
    actor_guid: u64,
    data: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::creatures::apply_pet_command(ctx, actor_guid, data, target_guid)
}





/// [`crate::bridge::apply_client_command`] with the sender named by guid (#479, operator call:
/// the GM/bridge surface rides the gateway path like every other verb; authorization was never
/// the connection identity).
#[reducer]
pub fn gw_client_command(
    ctx: &ReducerContext,
    actor_guid: u64,
    cmd: String,
    payload: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    actor(ctx, actor_guid)?;
    crate::bridge::apply_client_command(ctx, actor_guid, &cmd, &payload)
}

/// [`crate::gm::apply_gm_command`] with the Actor named by guid. The Gateway conveys the Account's
/// current Realm-core Alpha Test Tools authority; the Module combines it with the Character's GM
/// level and remains the final Gate.
#[reducer]
pub fn gw_gm_command(
    ctx: &ReducerContext,
    actor_guid: u64,
    alpha_test_tools: bool,
    text: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let caller = actor(ctx, actor_guid)?;
    crate::gm::apply_gm_command(ctx, caller, alpha_test_tools, text)
}

#[cfg(test)]
mod tests {
    use super::{apply_movement_batch, lease_expired, GwMove, LEASE_REAP_MICROS, LEASE_TTL_MICROS};
    use std::collections::{HashMap, HashSet};

    const HEARTBEAT: u16 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;
    const START: u16 = lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD as u16;

    #[derive(Clone, Debug, PartialEq)]
    struct MotionState {
        x: f32,
        move_time_ms: u32,
        relay: Option<(u16, Vec<u8>)>,
    }

    fn movement(actor_guid: u64, opcode: u16, x: f32, move_time_ms: u32, body: &[u8]) -> GwMove {
        GwMove {
            actor_guid,
            opcode,
            movement_info: body.to_vec(),
            x,
            y: 2.0,
            z: 3.0,
            o: 0.5,
            move_time_ms,
        }
    }

    fn apply_model(states: &mut HashMap<u64, MotionState>, movement: GwMove) -> Result<(), String> {
        let state = states
            .get_mut(&movement.actor_guid)
            .ok_or_else(|| "mover not in world".to_string())?;
        if crate::world::movement_is_accepted(
            state.move_time_ms,
            movement.move_time_ms,
            movement.x,
            movement.y,
            movement.z,
            movement.o,
        ) {
            state.x = movement.x;
            state.move_time_ms = movement.move_time_ms;
            state.relay = Some((movement.opcode, movement.movement_info));
        }
        Ok(())
    }

    fn state(x: f32, move_time_ms: u32) -> MotionState {
        MotionState {
            x,
            move_time_ms,
            relay: None,
        }
    }

    /// The lease decision, pinned by value: alive inside the TTL, dead past it, and the TTL must
    /// exceed the reap interval by enough that one delayed heartbeat cannot despawn a live
    /// gateway's players (the whole point of TTL-only over sweep-on-disconnect).
    #[test]
    fn a_lease_dies_past_the_ttl_and_survives_missed_beats_up_to_it() {
        assert!(!lease_expired(1_000_000, 1_000_000, LEASE_TTL_MICROS));
        assert!(!lease_expired(LEASE_TTL_MICROS, 0, LEASE_TTL_MICROS));
        assert!(lease_expired(LEASE_TTL_MICROS + 1, 0, LEASE_TTL_MICROS));
        assert!(
            LEASE_TTL_MICROS >= 3 * LEASE_REAP_MICROS,
            "TTL must tolerate at least two missed heartbeats plus a reap interval"
        );
    }

    #[test]
    fn missing_and_invalid_entries_do_not_reject_valid_neighbors() {
        let mut states = HashMap::from([(1, state(0.0, 0)), (3, state(0.0, 0))]);
        apply_movement_batch(
            vec![
                movement(1, HEARTBEAT, 1.0, 10, &[1]),
                movement(2, HEARTBEAT, 2.0, 10, &[2]),
                movement(1, HEARTBEAT, f32::NAN, 11, &[9]),
                movement(3, HEARTBEAT, 3.0, 10, &[3]),
            ],
            |_| false,
            |movement| apply_model(&mut states, movement),
        );

        assert_eq!(
            states[&1],
            MotionState {
                x: 1.0,
                move_time_ms: 10,
                relay: Some((HEARTBEAT, vec![1]))
            }
        );
        assert_eq!(
            states[&3],
            MotionState {
                x: 3.0,
                move_time_ms: 10,
                relay: Some((HEARTBEAT, vec![3]))
            }
        );
    }

    #[test]
    fn stale_batch_motion_cannot_overtake_newer_state() {
        let mut states = HashMap::from([(7, state(0.0, 0))]);
        apply_model(&mut states, movement(7, START, 8.0, 200, &[8])).unwrap();
        let after_immediate = states[&7].clone();

        apply_movement_batch(
            vec![
                movement(7, HEARTBEAT, 9.0, 200, &[9]),
                movement(7, HEARTBEAT, 10.0, 199, &[10]),
            ],
            |_| false,
            |movement| apply_model(&mut states, movement),
        );

        assert_eq!(states[&7], after_immediate);
    }

    #[test]
    fn taxi_suppression_is_per_entry() {
        let mut states = HashMap::from([(4, state(0.0, 0)), (5, state(0.0, 0))]);
        let suppressed = HashSet::from([4]);
        apply_movement_batch(
            vec![
                movement(4, HEARTBEAT, 4.0, 10, &[4]),
                movement(5, HEARTBEAT, 5.0, 10, &[5]),
            ],
            |guid| suppressed.contains(&guid),
            |movement| apply_model(&mut states, movement),
        );

        assert_eq!(states[&4], state(0.0, 0));
        assert_eq!(states[&5].relay, Some((HEARTBEAT, vec![5])));
    }

    #[test]
    fn single_and_batch_paths_commit_the_same_state_and_raw_relay() {
        let mut single = HashMap::from([(11, state(0.0, 0))]);
        let mut batch = single.clone();
        apply_model(&mut single, movement(11, START, 12.0, 50, &[1, 2, 3, 4])).unwrap();
        apply_movement_batch(
            vec![movement(11, START, 12.0, 50, &[1, 2, 3, 4])],
            |_| false,
            |movement| apply_model(&mut batch, movement),
        );

        assert_eq!(batch, single);
        assert_eq!(batch[&11].relay, Some((START, vec![1, 2, 3, 4])));
    }

    #[test]
    fn batch_publishes_accepted_motion_before_returning() {
        let mut states = HashMap::from([(12, state(0.0, 0))]);
        apply_movement_batch(
            vec![movement(12, HEARTBEAT, 6.0, 60, &[6])],
            |_| false,
            |movement| apply_model(&mut states, movement),
        );
        assert_eq!(states[&12].relay, Some((HEARTBEAT, vec![6])));

        let body = crate::test_scan::code_of(include_str!("gw.rs"), "pub fn gw_movement_batch(");
        let apply_at = body
            .find("apply_movement_batch(")
            .expect("batch application");
        let publish_at = body.find("publish_staged(ctx)").expect("inline publish");
        assert!(
            apply_at < publish_at,
            "the reducer must publish after applying the whole batch"
        );
    }
    /// Every reducer in THIS file must open with its gate — `require_operator` for a gateway
    /// verb, the scheduler-only sender fence for a scheduled reducer — before it resolves or
    /// touches anything else. The trust model of the whole surface, pinned structurally: split
    /// the file on the reducer attribute, and each following fn body's FIRST statement must be
    /// one of the two gates. A verb added without one fails here, not in review.
    #[test]
    fn every_gateway_verb_gates_on_require_operator_first() {
        let src = include_str!("gw.rs");
        // Built at runtime so this test's own source can never match the needle.
        let needle = format!("#[{}]", "reducer");
        let mut chunks = src.split(needle.as_str());
        chunks.next(); // preamble
        let mut seen = 0;
        for chunk in chunks {
            let body = chunk
                .split_once('{')
                .map(|(_, b)| b)
                .unwrap_or("")
                .trim_start();
            assert!(
                body.starts_with("require_operator(ctx)?;")
                    || body.starts_with("if ctx.sender() != ctx.database_identity()"),
                "a reducer here must open with require_operator(ctx)?; (gateway verb) or the \
                 scheduler-only sender fence (scheduled reducer) — got:\n{}",
                &body[..body.len().min(120)]
            );
            seen += 1;
        }
        assert!(seen >= 1, "the scan found no reducers — needle drifted");
    }
}
