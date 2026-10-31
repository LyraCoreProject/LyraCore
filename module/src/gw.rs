use lyracore_shared::loot::LootBoundaryFailure;
use spacetimedb::{reducer, table, Identity, ReducerContext, ScheduleAt, Timestamp};

use crate::game_account;
use crate::group::GroupAction;
use crate::helpers::{acting_entity_by_guid, require_operator};

// Retired Gateway lease tables keep their declarations; see docs/danger-zones.md.
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

// The schedule keeps its published name; it now reaps expired Account Claims only.
#[table(accessor = game_gateway_lease_reaper_schedule, scheduled(reap_gateway_leases))]
pub struct GatewayLeaseReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) const ACCOUNT_CLAIM_REAP_MICROS: i64 = 15_000_000;

#[reducer]
pub fn reap_gateway_leases(
    ctx: &ReducerContext,
    _schedule: GatewayLeaseReaperSchedule,
) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".to_string());
    }
    crate::account_ownership::reap_account_claims(ctx);
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
    let movement = gw_movement(&movement_info, x, y, z, o, move_time_ms);
    crate::world::apply_movement_update(ctx, mover, opcode, &movement, movement_info)
}

/// The pose arguments are authoritative; a body that does not decode supplies only its flag word.
fn gw_movement(
    movement_info: &[u8],
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    move_time_ms: u32,
) -> lyracore_protocol::codec::ClientMovement {
    use lyracore_protocol::codec::ClientMovement;
    ClientMovement::decode(movement_info)
        .unwrap_or_else(|_| {
            let flags = movement_info
                .get(0..4)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            ClientMovement::from_flags(flags)
        })
        .with_pose(
            move_time_ms,
            lyracore_protocol::messages::vanilla::Vector3d { x, y, z },
            o,
        )
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
            let movement = gw_movement(&m.movement_info, m.x, m.y, m.z, m.o, m.move_time_ms);
            crate::world::apply_movement_update(ctx, mover, m.opcode, &movement, m.movement_info)
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
    Ok(crate::spell::apply_cast_spell_at(
        ctx,
        &caster,
        spell_id,
        target_guid,
        x,
        y,
        z,
    )?)
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
        .map_err(|refusal| refusal.as_tag().into())
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
    Ok(crate::actor::ranged_attack(
        ctx,
        actor_guid,
        target_guid,
        spell_id,
    )?)
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
    Ok(crate::world::apply_set_sheathed(ctx, actor, state)?)
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
    Ok(crate::mail_reward::file_reward_letter(
        ctx,
        actor_guid,
        giver_guid,
        quest_entry,
    )?)
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
    Ok(crate::actor::cast_item_target(
        ctx, actor_guid, spell_id, slot,
    )?)
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
    crate::actor::buy_item(ctx, actor_guid, vendor_guid, item_entry, count).map_err(String::from)
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
    crate::actor::sell_item(ctx, actor_guid, vendor_guid, slot).map_err(String::from)
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
        )
        .as_tag()
        .into());
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
        .map_err(|refusal| refusal.as_tag().into())
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
    Ok(crate::actor::respond_resurrect(ctx, actor_guid, accept)?)
}

#[reducer]
pub fn gw_repop(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    Ok(crate::actor::repop(ctx, actor_guid)?)
}

#[reducer]
pub fn gw_self_resurrect(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    Ok(crate::actor::self_resurrect(ctx, actor_guid)?)
}

#[reducer]
pub fn gw_spirit_res(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    Ok(crate::actor::spirit_res(ctx, actor_guid)?)
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
    Ok(crate::chat::apply_send_emote(
        ctx,
        sender,
        text_emote,
        emote_anim,
        target_guid,
    )?)
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
        "account has no bound identity — establish_session must run before login".to_string()
    })?;
    crate::world::apply_player_login(ctx, &account, character_guid, owner, entry)
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
    Ok(crate::away::apply_set_away(ctx, entity, kind, message)?)
}

fn contact_actor(ctx: &ReducerContext, actor_guid: u64) -> Result<crate::WorldEntity, String> {
    let actor =
        acting_entity_by_guid(ctx, actor_guid).ok_or_else(|| "mover not in world".to_string())?;
    if crate::taxi::is_in_flight(ctx, actor_guid) {
        return Err(crate::chat::refused_contact(
            lyracore_shared::social::ContactRefusal::ActorUnavailable,
            &format!("actor {actor_guid} is in taxi flight"),
        )
        .as_tag()
        .into());
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
        .map_err(|refusal| refusal.as_tag().into())
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
        .map_err(|refusal| refusal.as_tag().into())
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
        .map_err(|refusal| refusal.as_tag().into())
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
        .map_err(|refusal| refusal.as_tag().into())
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
    crate::group::apply_action_for(ctx, actor_guid, GroupAction::Invite(target_guid))
}

#[reducer]
pub fn gw_group_decline(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    group_actor(ctx, actor_guid)?;
    crate::group::apply_action_for(ctx, actor_guid, GroupAction::Decline)
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
    Ok(crate::trade::apply_initiate_trade(
        ctx,
        acting,
        target_guid,
    )?)
}

#[reducer]
pub fn gw_begin_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    Ok(crate::trade::apply_begin_trade(ctx, acting)?)
}

#[reducer]
pub fn gw_cancel_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    Ok(crate::trade::apply_cancel_trade(ctx, acting)?)
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
    Ok(crate::trade::apply_set_trade_item(
        ctx, acting, trade_slot, inv_slot,
    )?)
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
    Ok(crate::trade::apply_clear_trade_item(
        ctx, acting, trade_slot,
    )?)
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
    Ok(crate::trade::apply_set_trade_gold(ctx, acting, copper)?)
}

#[reducer]
pub fn gw_accept_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    Ok(crate::trade::apply_accept_trade(ctx, acting)?)
}

#[reducer]
pub fn gw_unaccept_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    Ok(crate::trade::apply_unaccept_trade(ctx, acting)?)
}

#[reducer]
pub fn gw_busy_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    Ok(crate::trade::apply_busy_trade(ctx, acting)?)
}

#[reducer]
pub fn gw_ignore_trade(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let acting = actor(ctx, actor_guid)?;
    Ok(crate::trade::apply_ignore_trade(ctx, acting)?)
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
    crate::group::apply_action_for(ctx, actor_guid, GroupAction::Uninvite(target_guid))
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
    crate::group::apply_action_for(
        ctx,
        actor_guid,
        GroupAction::LootMethod {
            method: loot_setting,
            master: master_guid,
            threshold: loot_threshold,
        },
    )
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
    Ok(crate::quest::apply_push_quest_to_party(
        ctx,
        actor_guid,
        quest_entry,
    )?)
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
    Ok(crate::items::apply_buyback_item(
        ctx,
        actor_guid,
        vendor_guid,
        slot,
    )?)
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
    crate::items::apply_buy_bank_slot(ctx, actor_guid, banker_guid).map_err(String::from)
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
    Ok(crate::professions::apply_disenchant(ctx, actor_guid, slot)?)
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
    Ok(crate::professions::apply_enchant_item(
        ctx,
        actor_guid,
        target_slot,
        enchant_id,
    )?)
}

#[reducer]
pub fn gw_fish(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    actor(ctx, actor_guid)?;
    Ok(crate::professions::apply_fish(ctx, actor_guid)?)
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
    Ok(crate::professions::skin_corpse(
        ctx,
        actor_guid,
        corpse_guid,
    )?)
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
    Ok(crate::loot::cast_vote_on(
        ctx,
        corpse_guid,
        loot_slot as u8,
        actor_guid,
        vote,
    )?)
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
    Ok(crate::loot::apply_master_give(
        ctx,
        actor_guid,
        corpse_guid,
        loot_slot,
        target_guid,
    )?)
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
    Ok(crate::spell::do_cast_spell(
        ctx,
        caster,
        spell_id,
        target_guid,
    )?)
}

#[reducer]
pub fn gw_cancel_cast(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let caster = actor(ctx, actor_guid)?;
    Ok(crate::spell::do_cancel_cast(ctx, caster)?)
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
    Ok(crate::spell::do_cancel_aura(ctx, actor_guid, spell_id)?)
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
        crate::gossip::Selection {
            option_id,
            row_id: option_row_id,
            menu_id: source_menu_id,
            root_menu_id: source_menu_id,
            action: selected_action,
        },
    )
    .map(|_| ())
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
    Ok(crate::world::apply_inspect(ctx, inspector, target_guid)?)
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
    Ok(
        crate::talent::do_learn_talent(ctx, actor_guid, learner.owner_identity, talent_id)
            .map(|_| ())?,
    )
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
    Ok(crate::gameobject::apply_pick_lock(
        ctx, actor_guid, go_guid,
    )?)
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
    Ok(crate::action_bar::apply_set_action_button(
        ctx,
        character,
        button,
        action,
        action_type,
    )?)
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
    Ok(crate::quest::apply_abandon_quest(
        ctx,
        actor_guid,
        quest_entry,
    )?)
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
    Ok(crate::corpse::apply_reclaim_corpse(ctx, player)?)
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
    Ok(crate::quest::request_enter_areatrigger(
        ctx, actor_guid, trigger_id,
    )?)
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
    Ok(crate::creatures::apply_pet_command(
        ctx,
        actor_guid,
        data,
        target_guid,
    )?)
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
    Ok(crate::bridge::apply_client_command(
        ctx, actor_guid, &cmd, &payload,
    )?)
}

#[cfg(test)]
mod tests {
    use super::{apply_movement_batch, GwMove};

    fn movement(actor_guid: u64) -> GwMove {
        GwMove {
            actor: crate::account_ownership::SessionActor {
                guid: actor_guid,
                ownership: None,
            },
            opcode: lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16,
            movement_info: Vec::new(),
            x: 1.0,
            y: 2.0,
            z: 3.0,
            o: 0.5,
            move_time_ms: 10,
        }
    }

    #[test]
    fn a_batch_skips_suppressed_movers_and_continues_past_a_rejected_move() {
        let mut applied = Vec::new();
        apply_movement_batch(
            vec![movement(1), movement(2), movement(3), movement(4)],
            |guid| guid == 2,
            |movement| {
                applied.push(movement.actor.guid);
                if movement.actor.guid == 3 {
                    Err("mover not in world".to_string())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(applied, vec![1, 3, 4]);
    }
}
