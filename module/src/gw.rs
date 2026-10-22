use lyracore_shared::loot::LootBoundaryFailure;
use spacetimedb::{reducer, table, Identity, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::game_account;
use crate::game_world_entity;
use crate::helpers::{acting_entity_by_guid, require_operator};

#[table(accessor = game_gateway_lease)]
pub struct GatewayLease {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub identity: Identity,
    pub last_heartbeat: Timestamp,
}

#[table(accessor = game_gateway_session)]
pub struct GatewaySession {
    #[primary_key]
    pub entity_guid: u64,
    pub lease_id: u64,
}

#[table(accessor = game_gateway_lease_reaper_schedule, scheduled(reap_gateway_leases))]
pub struct GatewayLeaseReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) const LEASE_REAP_MICROS: i64 = 15_000_000;
pub(crate) const LEASE_TTL_MICROS: i64 = 60_000_000;

pub(crate) fn lease_expired(now_micros: i64, heartbeat_micros: i64, ttl_micros: i64) -> bool {
    now_micros - heartbeat_micros > ttl_micros
}

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

#[reducer]
pub fn reap_gateway_leases(
    ctx: &ReducerContext,
    _schedule: GatewayLeaseReaperSchedule,
) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    crate::account_ownership::reap_account_fences(ctx);
    crate::account_ownership::reap_account_claims(ctx);
    let now = ctx.timestamp.to_micros_since_unix_epoch();
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

fn actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    crate::taxi::reject_action_while_in_flight(ctx, actor_guid)?;
    Ok(actor)
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_movement_update(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    opcode: u16,
    movement_info: Vec<u8>,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    move_time_ms: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    if crate::taxi::movement_is_suppressed(ctx, actor_guid) {
        return Ok(());
    }
    let mover = actor(ctx, actor_guid)?;
    crate::world::apply_movement_update(ctx, mover, opcode, movement_info, x, y, z, o, move_time_ms)
}

#[derive(spacetimedb::SpacetimeType)]
pub struct GwMove {
    pub actor: crate::account_ownership::SessionActor,
    pub opcode: u16,
    pub movement_info: Vec<u8>,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
    pub move_time_ms: u32,
}

fn apply_movement_batch(
    moves: Vec<GwMove>,
    mut suppressed: impl FnMut(u64) -> bool,
    mut apply: impl FnMut(GwMove) -> Result<(), String>,
) {
    for movement in moves {
        if suppressed(movement.actor.guid) {
            continue;
        }
        let actor_guid = movement.actor.guid;
        if let Err(error) = apply(movement) {
            spacetimedb::log::debug!(
                "gw_movement_batch: move for {} rejected: {error}",
                actor_guid
            );
        }
    }
}

#[reducer]
pub fn gw_movement_batch(ctx: &ReducerContext, moves: Vec<GwMove>) -> Result<(), String> {
    require_operator(ctx)?;
    apply_movement_batch(
        moves,
        |actor_guid| crate::taxi::movement_is_suppressed(ctx, actor_guid),
        |m| {
            let actor_guid = crate::account_ownership::require_actor(ctx, m.actor)?;
            let mover = actor(ctx, actor_guid)?;
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
    // Drain motion inside the batch transaction so its subscription sweep runs once.
    crate::motion::publish_staged(ctx);
    Ok(())
}

#[reducer]
pub fn gw_attack(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::attack(ctx, actor_guid, target_guid)
}

#[reducer]
pub fn gw_cast_at(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::cast_at(ctx, actor_guid, spell_id, target_guid)
}

#[reducer]
pub fn gw_cast_spell_at(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    spell_id: u32,
    target_guid: u64,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let caster = actor(ctx, actor_guid)?;
    crate::spell::apply_cast_spell_at(ctx, &caster, spell_id, target_guid, x, y, z)
}

fn loot_actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor = acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| {
        loot_boundary_failure(LootBoundaryFailure::MissingActor, "mover not in world")
    })?;
    if crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(crate::loot::refused(
            lyracore_shared::loot::LootRefusal::LooterUnavailable,
            "looter is in taxi flight",
        ));
    }
    Ok(actor)
}

fn loot_boundary_failure(failure: LootBoundaryFailure, detail: &str) -> String {
    let tag = failure.as_tag();
    spacetimedb::log::error!("loot boundary failure {tag}: {detail}");
    tag.to_string()
}

fn loot_operator_error(detail: String) -> String {
    loot_boundary_failure(LootBoundaryFailure::OperatorRejected, &detail)
}

#[reducer]
pub fn gw_take_loot(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::actor::take_loot(ctx, actor_guid, corpse_guid, loot_slot)
}

#[reducer]
pub fn gw_open_creature_loot(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    corpse_guid: u64,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::actor::open_creature_loot(ctx, actor_guid, corpse_guid)
}

#[reducer]
pub fn gw_ranged_attack(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::ranged_attack(ctx, actor_guid, target_guid, spell_id)
}

#[reducer]
pub fn gw_stop_attack(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::stop_attack(ctx, actor_guid)
}

#[reducer]
pub fn gw_set_sheathed(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    state: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let actor = actor(ctx, actor_guid)?;
    crate::world::apply_set_sheathed(ctx, actor, state)
}

#[reducer]
pub fn gw_set_stand_state(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    state: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let actor = actor(ctx, actor_guid)?;
    crate::world::apply_set_stand_state(ctx, actor, state)
}

#[reducer]
pub fn gw_accept_quest(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::accept_quest(ctx, actor_guid, giver_guid, quest_entry)
}

#[reducer]
pub fn gw_turn_in_quest(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::turn_in_quest(ctx, actor_guid, giver_guid, quest_entry, reward_index)?;
    // Only World Sessions drive Reward Letters; a bot turn-in would leave its letter held.
    crate::mail_reward::file_reward_letter(ctx, actor_guid, giver_guid, quest_entry)
}

fn item_actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    if crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(crate::items::refused(crate::items::refuse(
            lyracore_shared::item::ItemRefusal::NotRightNow,
            "Character is in taxi flight",
        )));
    }
    Ok(actor)
}

#[reducer]
pub fn gw_use_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    item_actor(ctx, actor_guid)?;
    crate::actor::use_item(ctx, actor_guid, slot).map_err(crate::items::refused)
}

#[reducer]
pub fn gw_cast_item_target(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    spell_id: u32,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::cast_item_target(ctx, actor_guid, spell_id, slot)
}

#[reducer]
pub fn gw_loot_money(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::actor::loot_money(ctx, actor_guid, target_guid)
}

#[reducer]
pub fn gw_buy_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::buy_item(ctx, actor_guid, vendor_guid, item_entry, count)
}

#[reducer]
pub fn gw_sell_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::sell_item(ctx, actor_guid, vendor_guid, slot)
}

#[reducer]
pub fn gw_equip_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    from_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    item_actor(ctx, actor_guid)?;
    crate::actor::equip_item(ctx, actor_guid, from_slot).map_err(crate::items::refused)
}

#[reducer]
pub fn gw_use_gameobject(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    go_guid: u64,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::actor::use_gameobject(ctx, actor_guid, go_guid)
}

fn trainer_actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    if crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(crate::trainer::refused(
            lyracore_shared::trainer::TrainerRefusal::Unavailable,
            "buyer is in taxi flight",
        ));
    }
    Ok(actor)
}

#[reducer]
pub fn gw_trainer_buy(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    trainer_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    trainer_actor(ctx, actor_guid)?;
    crate::actor::trainer_buy(ctx, actor_guid, trainer_guid, spell_id)
}

#[reducer]
pub fn gw_respond_resurrect(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    accept: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::respond_resurrect(ctx, actor_guid, accept)
}

#[reducer]
pub fn gw_repop(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::repop(ctx, actor_guid)
}

#[reducer]
pub fn gw_self_resurrect(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::actor::self_resurrect(ctx, actor_guid)
}

#[reducer]
pub fn gw_spirit_res(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    healer_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::world::do_spirit_healer_res(ctx, actor_guid, healer_guid)
}

#[reducer]
pub fn gw_accept_group_invite(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::actor::accept_group_invite(ctx, actor_guid)
}

#[reducer]
pub fn gw_set_target(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let entity =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    crate::world::apply_set_target(ctx, entity, target_guid)
}

#[reducer]
pub fn gw_send_chat(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_send_chat(ctx, sender, chat_type, language, message)
}

#[reducer]
pub fn gw_send_emote(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    text_emote: u32,
    emote_anim: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let sender = actor(ctx, actor_guid)?;
    crate::chat::apply_send_emote(ctx, sender, text_emote, emote_anim, target_guid)
}

#[reducer]
pub fn gw_player_login(
    ctx: &ReducerContext,
    account_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    enter_world(
        ctx,
        account_id,
        request_actor,
        crate::world::WorldEntry::Login,
    )
}

#[reducer]
pub fn gw_player_world_port(
    ctx: &ReducerContext,
    account_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    enter_world(
        ctx,
        account_id,
        request_actor,
        crate::world::WorldEntry::WorldPort,
    )
}

fn enter_world(
    ctx: &ReducerContext,
    account_id: u64,
    request_actor: crate::account_ownership::SessionActor,
    entry: crate::world::WorldEntry,
) -> Result<(), String> {
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
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
    crate::world::apply_player_login(ctx, &account, character_guid, owner, entry)?;
    // Bind the lease only after login succeeds so a Refusal leaves no reaper-owned Session.
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

#[reducer]
pub fn gw_set_away(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    kind: u8,
    message: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let entity =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    crate::away::apply_set_away(ctx, entity, kind, message)
}

fn contact_actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    if crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(crate::chat::refused_contact(
            lyracore_shared::social::ContactRefusal::ActorUnavailable,
            &format!("actor {actor_guid} is in taxi flight"),
        ));
    }
    Ok(actor)
}

#[reducer]
pub fn gw_add_friend(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
    target_race: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let sender = contact_actor(ctx, actor_guid)?;
    crate::chat::add_contact(ctx, sender, target_guid, false, Some(target_race))
}

#[reducer]
pub fn gw_del_friend(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let sender = contact_actor(ctx, actor_guid)?;
    crate::chat::remove_contact(ctx, sender, target_guid, false)
}

#[reducer]
pub fn gw_add_ignore(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let sender = contact_actor(ctx, actor_guid)?;
    crate::chat::add_contact(ctx, sender, target_guid, true, None)
}

#[reducer]
pub fn gw_del_ignore(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let sender = contact_actor(ctx, actor_guid)?;
    crate::chat::remove_contact(ctx, sender, target_guid, true)
}

fn group_actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    if crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(crate::group::refused(
            lyracore_shared::group::GroupRefusal::ActorUnavailable,
            &format!("actor {actor_guid} is in taxi flight"),
        ));
    }
    Ok(actor)
}

#[reducer]
pub fn gw_group_invite(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::group::invite_core(ctx, actor_guid, target_guid)
}

#[reducer]
pub fn gw_group_decline(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::group::decline_invite_for(ctx, actor_guid)
}

#[reducer]
pub fn gw_initiate_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_initiate_trade(ctx, acting, target_guid)
}

#[reducer]
pub fn gw_begin_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_begin_trade(ctx, acting)
}

#[reducer]
pub fn gw_cancel_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_cancel_trade(ctx, acting)
}

#[reducer]
pub fn gw_set_trade_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    trade_slot: u8,
    inv_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_set_trade_item(ctx, acting, trade_slot, inv_slot)
}

#[reducer]
pub fn gw_clear_trade_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    trade_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_clear_trade_item(ctx, acting, trade_slot)
}

#[reducer]
pub fn gw_set_trade_gold(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    copper: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_set_trade_gold(ctx, acting, copper)
}

#[reducer]
pub fn gw_accept_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_accept_trade(ctx, acting)
}

#[reducer]
pub fn gw_unaccept_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_unaccept_trade(ctx, acting)
}

#[reducer]
pub fn gw_busy_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_busy_trade(ctx, acting)
}

#[reducer]
pub fn gw_ignore_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    crate::trade::apply_ignore_trade(ctx, acting)
}

#[reducer]
pub fn gw_duel_accept(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    flag_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::duel::accept_duel(ctx, actor_guid, flag_guid);
    Ok(())
}

#[reducer]
pub fn gw_duel_cancel(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    flag_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::duel::cancel_duel(ctx, actor_guid, flag_guid);
    Ok(())
}

#[reducer]
pub fn gw_group_leave(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::group::leave_group_for(ctx, actor_guid)
}

#[reducer]
pub fn gw_group_uninvite(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::group::uninvite_from_group(ctx, actor_guid, target_guid)
}

#[reducer]
pub fn gw_group_loot_method(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    loot_setting: u8,
    master_guid: u64,
    loot_threshold: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::group::set_loot_method_for(ctx, actor_guid, loot_setting, master_guid, loot_threshold)
}

#[reducer]
pub fn gw_push_quest_to_party(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    quest_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::quest::apply_push_quest_to_party(ctx, actor_guid, quest_entry)
}

#[reducer]
pub fn gw_move_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    item_actor(ctx, actor_guid)?;
    crate::items::apply_item_move(ctx, actor_guid, from_slot, to_slot)
        .map_err(crate::items::refused)
}

#[reducer]
pub fn gw_destroy_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    slot: u8,
    count: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    item_actor(ctx, actor_guid)?;
    crate::items::apply_item_destroy(ctx, actor_guid, slot, count).map_err(crate::items::refused)
}

#[reducer]
pub fn gw_split_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    from_slot: u8,
    to_slot: u8,
    count: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    item_actor(ctx, actor_guid)?;
    crate::items::apply_item_split(ctx, actor_guid, from_slot, count, to_slot)
        .map_err(crate::items::refused)
}

#[reducer]
pub fn gw_unequip_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    from_slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    item_actor(ctx, actor_guid)?;
    crate::items::apply_unequip_item(ctx, actor_guid, from_slot).map_err(crate::items::refused)
}

#[reducer]
pub fn gw_auto_bank_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_auto_bank_item(ctx, actor_guid, slot)
}

#[reducer]
pub fn gw_buyback_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_buyback_item(ctx, actor_guid, vendor_guid, slot)
}

#[reducer]
pub fn gw_repair_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    npc_guid: u64,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::items::apply_player_repair(ctx, actor_guid, npc_guid, slot)
}

#[reducer]
pub fn gw_buy_bank_slot(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    banker_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::items::buy_bank_slot_result(crate::items::apply_buy_bank_slot(
        ctx,
        actor_guid,
        banker_guid,
    ))
}

#[reducer]
pub fn gw_disenchant(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    slot: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::professions::apply_disenchant(ctx, actor_guid, slot)
}

#[reducer]
pub fn gw_enchant_item(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::professions::apply_enchant_item(ctx, actor_guid, target_slot, enchant_id)
}

#[reducer]
pub fn gw_fish(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::professions::apply_fish(ctx, actor_guid)
}

#[reducer]
pub fn gw_bind_home(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    innkeeper_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::items::innkeeper_access_gate(ctx, actor_guid, innkeeper_guid)?;
    crate::world::set_home(ctx, actor_guid);
    Ok(())
}

#[reducer]
pub fn gw_skin(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    corpse_guid: u64,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::professions::skin_corpse(ctx, actor_guid, corpse_guid)
}

#[reducer]
pub fn gw_loot_roll(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    corpse_guid: u64,
    loot_slot: u32,
    vote: u8,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::loot::cast_vote_on(ctx, corpse_guid, loot_slot as u8, actor_guid, vote)
}

#[reducer]
pub fn gw_loot_master_give(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    corpse_guid: u64,
    loot_slot: u8,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx).map_err(loot_operator_error)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    loot_actor(ctx, actor_guid)?;
    crate::loot::apply_master_give(ctx, actor_guid, corpse_guid, loot_slot, target_guid)
}

#[reducer]
pub fn gw_cast_spell(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let caster = actor(ctx, actor_guid)?;
    crate::spell::do_cast_spell(ctx, caster, spell_id, target_guid)
}

#[reducer]
pub fn gw_cancel_cast(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let caster = actor(ctx, actor_guid)?;
    crate::spell::do_cancel_cast(ctx, caster)
}

#[reducer]
pub fn gw_cancel_aura(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    spell_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::spell::do_cancel_aura(ctx, actor_guid, spell_id)
}

#[reducer]
pub fn gw_gossip_select(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    npc_guid: u64,
    option_id: u32,
    option_row_id: u32,
    source_menu_id: u32,
    selected_action: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::world::apply_gossip_select(
        ctx,
        actor_guid,
        npc_guid,
        option_id,
        option_row_id,
        source_menu_id,
        selected_action,
    )
}

#[reducer]
pub fn gw_inspect(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let inspector = actor(ctx, actor_guid)?;
    crate::world::apply_inspect(ctx, inspector, target_guid)
}

#[reducer]
pub fn gw_learn_talent(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    talent_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let learner = actor(ctx, actor_guid)?;
    crate::talent::do_learn_talent(ctx, actor_guid, learner.owner_identity, talent_id).map(|_| ())
}

#[reducer]
pub fn gw_reset_talents(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    trainer_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::talent::do_reset_talents(ctx, actor_guid, trainer_guid).map(|_| ())
}

#[reducer]
pub fn gw_pick_lock(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    go_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::gameobject::apply_pick_lock(ctx, actor_guid, go_guid)
}

#[reducer]
pub fn gw_set_action_button(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    button: u8,
    action: u32,
    action_type: u8,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let character = actor(ctx, actor_guid)?;
    crate::action_bar::apply_set_action_button(ctx, character, button, action, action_type)
}

#[reducer]
pub fn gw_set_faction_at_war(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    reputation_index: u32,
    at_war: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let player = actor(ctx, actor_guid)?;
    crate::reputation::apply_set_faction_at_war(ctx, player, reputation_index, at_war)
}

#[reducer]
pub fn gw_set_watched_faction(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    reputation_index: i32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    crate::reputation::apply_set_watched_faction(ctx, actor_guid, reputation_index)
}

#[reducer]
pub fn gw_abandon_quest(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    quest_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::quest::apply_abandon_quest(ctx, actor_guid, quest_entry)
}

#[reducer]
pub fn gw_reclaim_corpse(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    _corpse_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let player = actor(ctx, actor_guid)?;
    crate::corpse::apply_reclaim_corpse(ctx, player)
}

#[reducer]
pub fn gw_enter_areatrigger(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    trigger_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::quest::apply_enter_areatrigger(ctx, actor_guid, trigger_id);
    Ok(())
}

#[reducer]
pub fn gw_pet_command(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    data: u32,
    target_guid: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::creatures::apply_pet_command(ctx, actor_guid, data, target_guid)
}

#[reducer]
pub fn gw_client_command(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    cmd: String,
    payload: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    crate::bridge::apply_client_command(ctx, actor_guid, &cmd, &payload)
}

#[reducer]
pub fn gw_gm_command(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    alpha_test_tools: bool,
    text: String,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
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
            actor: crate::account_ownership::SessionActor {
                guid: actor_guid,
                ownership: None,
            },
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
            .get_mut(&movement.actor.guid)
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

    #[test]
    fn a_lease_dies_past_the_ttl_and_survives_missed_beats_up_to_it() {
        assert!(!lease_expired(1_000_000, 1_000_000, LEASE_TTL_MICROS));
        assert!(!lease_expired(LEASE_TTL_MICROS, 0, LEASE_TTL_MICROS));
        assert!(lease_expired(LEASE_TTL_MICROS + 1, 0, LEASE_TTL_MICROS));
        const _: () = assert!(
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
    }
}
