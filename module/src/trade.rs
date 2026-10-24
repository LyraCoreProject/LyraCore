use spacetimedb::{table, Identity, ReducerContext, Table, Timestamp};

use lyracore_shared::trade::event_kind;

use crate::helpers::{player_interaction_gate, PlayerInteractionDenied};
use crate::{
    game_character_contact, game_faction_template, game_item_instance, game_item_template,
    game_world_entity, WorldEntity,
};

#[table(accessor = game_trade_session,
        index(accessor = by_initiator, btree(columns = [initiator_guid])),
        index(accessor = by_target, btree(columns = [target_guid])))]
pub struct TradeSession {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub initiator_guid: u64,
    pub target_guid: u64,
    pub open: bool,
    pub initiator_accepted: bool,
    pub target_accepted: bool,
    pub initiator_gold: u32,
    pub target_gold: u32,
    pub created_at: Timestamp,
}

crate::character_owned!(delete, fn sweep_delete_game_trade_session(ctx, character_guid) {
    let sessions = ctx.db.game_trade_session();
    for s in sessions.by_initiator().filter(&character_guid).collect::<Vec<_>>() {
        remove_session(ctx, &s);
    }
    for s in sessions.by_target().filter(&character_guid).collect::<Vec<_>>() {
        remove_session(ctx, &s);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_trade_session());

#[table(accessor = game_trade_slot, index(accessor = by_session, btree(columns = [session_id])))]
pub struct TradeSlot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub session_id: u64,
    pub owner_guid: u64,
    pub trade_slot: u8,
    pub item_guid: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_trade_slot(ctx, character_guid) {
    let slots = ctx.db.game_trade_slot();
    let owned: Vec<u64> = slots
.iter()
.filter(|s| s.owner_guid == character_guid)
.map(|s| s.id)
.collect();
    for id in owned {
        slots.id().delete(id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_trade_slot());

#[table(accessor = game_trade_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct TradeEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub kind: u8,
    pub other_guid: u64,
    pub created_at: Timestamp,
    pub recipient_guid: u64,
    pub payload: String,
}

fn push_trade_event(ctx: &ReducerContext, recipient_guid: u64, kind: u8, other_guid: u64) {
    push_trade_event_payload(ctx, recipient_guid, kind, other_guid, String::new());
}

fn push_trade_event_payload(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    other_guid: u64,
    payload: String,
) {
    let bound = crate::helpers::character_by_guid(ctx, recipient_guid).map(|c| c.owner_identity);
    let event = ctx.db.game_trade_event().insert(TradeEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        kind,
        other_guid,
        created_at: ctx.timestamp,
        recipient_guid,
        payload,
    });
    crate::native::trade_event(ctx, &event);
}

pub(crate) fn session_involving(ctx: &ReducerContext, guid: u64) -> Option<TradeSession> {
    let sessions = ctx.db.game_trade_session();
    sessions
        .by_initiator()
        .filter(&guid)
        .next()
        .or_else(|| sessions.by_target().filter(&guid).next())
}

pub(crate) fn item_is_offered(ctx: &ReducerContext, owner_guid: u64, item_guid: u64) -> bool {
    session_involving(ctx, owner_guid).is_some_and(|session| {
        ctx.db
            .game_trade_slot()
            .by_session()
            .filter(&session.id)
            .any(|slot| slot.owner_guid == owner_guid && slot.item_guid == item_guid)
    })
}

fn remove_session(ctx: &ReducerContext, session: &TradeSession) {
    let slots = ctx.db.game_trade_slot();
    for slot in slots.by_session().filter(&session.id).collect::<Vec<_>>() {
        slots.id().delete(slot.id);
    }
    ctx.db.game_trade_session().id().delete(session.id);
}

pub(crate) fn gate_refusal_kind(denied: PlayerInteractionDenied) -> u8 {
    match denied {
        PlayerInteractionDenied::ActorDead => event_kind::YOU_DEAD,
        PlayerInteractionDenied::NoTarget | PlayerInteractionDenied::TargetNotPlayer => {
            event_kind::NO_TARGET
        }
        PlayerInteractionDenied::TargetDead => event_kind::TARGET_DEAD,
        PlayerInteractionDenied::DifferentPartition | PlayerInteractionDenied::OutOfRange => {
            event_kind::TARGET_TO_FAR
        }
    }
}

pub(crate) fn initiate_verdict(
    gate: Result<(), PlayerInteractionDenied>,
    target_ignores_initiator: bool,
    friendly: bool,
    initiator_busy: bool,
    target_busy: bool,
) -> Result<(), u8> {
    if let Err(denied) = gate {
        return Err(gate_refusal_kind(denied));
    }
    if target_ignores_initiator {
        return Err(event_kind::IGNORE_YOU);
    }
    if !friendly {
        return Err(event_kind::WRONG_FACTION);
    }
    if initiator_busy || target_busy {
        return Err(event_kind::BUSY);
    }
    Ok(())
}

pub(crate) fn apply_initiate_trade(
    ctx: &ReducerContext,
    actor: WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    if target_guid == actor.guid {
        push_trade_event(ctx, actor.guid, event_kind::NO_TARGET, target_guid);
        return Ok(());
    }
    let target = crate::helpers::acting_entity_by_guid(ctx, target_guid);
    let friendly = ctx.db.game_faction_template().count() == 0
        || target.as_ref().is_none_or(|t| {
            crate::faction::is_friendly(ctx, actor.faction_template, t.faction_template)
        });
    let target_ignores_initiator = ctx
        .db
        .game_character_contact()
        .by_owner()
        .filter(&target_guid)
        .any(|c| c.is_ignore && c.target_guid == actor.guid);
    let verdict = initiate_verdict(
        player_interaction_gate(&actor, target.as_ref()),
        target_ignores_initiator,
        friendly,
        session_involving(ctx, actor.guid).is_some(),
        session_involving(ctx, target_guid).is_some(),
    );
    match verdict {
        Err(kind) => push_trade_event(ctx, actor.guid, kind, target_guid),
        Ok(()) => {
            ctx.db.game_trade_session().insert(TradeSession {
                id: 0,
                initiator_guid: actor.guid,
                target_guid,
                open: false,
                initiator_accepted: false,
                target_accepted: false,
                initiator_gold: 0,
                target_gold: 0,
                created_at: ctx.timestamp,
            });
            push_trade_event(ctx, target_guid, event_kind::BEGIN_TRADE, actor.guid);
        }
    }
    Ok(())
}

// Client slot 6 is visible to the partner but never changes ownership.
pub(crate) const WILL_NOT_BE_TRADED_SLOT: u8 = 6;

pub(crate) fn offer_slot_view(
    trade_slot: u8,
    inst: &crate::ItemInstance,
    tmpl: &crate::ItemTemplate,
) -> lyracore_shared::trade::OfferSlot {
    lyracore_shared::trade::OfferSlot {
        trade_slot,
        entry: inst.entry,
        display_id: tmpl.display_id,
        stack_count: inst.stack_count,
        enchantment: inst.enchant_id,
        durability: inst.durability,
        max_durability: tmpl.max_durability,
        random_property_id: inst.random_property_id,
    }
}

fn touch_session(ctx: &ReducerContext, mut session: TradeSession) -> TradeSession {
    session.created_at = ctx.timestamp;
    ctx.db.game_trade_session().id().update(session)
}

fn push_offer_events(ctx: &ReducerContext, session: &TradeSession, offerer_guid: u64) {
    let slots = ctx.db.game_trade_slot();
    let mut views = Vec::new();
    for row in slots.by_session().filter(&session.id).collect::<Vec<_>>() {
        if row.owner_guid != offerer_guid {
            continue;
        }
        let resolved = ctx
            .db
            .game_item_instance()
            .guid()
            .find(row.item_guid)
            .and_then(|inst| {
                ctx.db
                    .game_item_template()
                    .entry()
                    .find(inst.entry)
                    .map(|tmpl| offer_slot_view(row.trade_slot, &inst, &tmpl))
            });
        match resolved {
            Some(view) => views.push(view),
            None => {
                slots.id().delete(row.id);
            }
        }
    }
    views.sort_by_key(|v| v.trade_slot);
    let gold = if offerer_guid == session.initiator_guid {
        session.initiator_gold
    } else {
        session.target_gold
    };
    let payload = lyracore_shared::trade::encode_offer(gold, &views);
    let partner = if offerer_guid == session.initiator_guid {
        session.target_guid
    } else {
        session.initiator_guid
    };
    push_trade_event_payload(
        ctx,
        offerer_guid,
        event_kind::OFFER_SELF,
        partner,
        payload.clone(),
    );
    push_trade_event_payload(
        ctx,
        partner,
        event_kind::OFFER_PARTNER,
        offerer_guid,
        payload,
    );
}

fn reset_accepts(ctx: &ReducerContext, mut session: TradeSession) -> TradeSession {
    if !mutation_clears_accepts(session.initiator_accepted, session.target_accepted) {
        return session;
    }
    session.initiator_accepted = false;
    session.target_accepted = false;
    let session = ctx.db.game_trade_session().id().update(session);
    push_trade_event(
        ctx,
        session.initiator_guid,
        event_kind::BACK_TO_TRADE,
        session.target_guid,
    );
    push_trade_event(
        ctx,
        session.target_guid,
        event_kind::BACK_TO_TRADE,
        session.initiator_guid,
    );
    session
}

pub(crate) fn refresh_offer(ctx: &ReducerContext, owner_guid: u64) {
    if let Some(session) = session_involving(ctx, owner_guid).filter(|session| session.open) {
        let session = reset_accepts(ctx, session);
        let session = touch_session(ctx, session);
        push_offer_events(ctx, &session, owner_guid);
    }
}

pub(crate) fn apply_set_trade_item(
    ctx: &ReducerContext,
    actor: WorldEntity,
    trade_slot: u8,
    inv_slot: u8,
) -> Result<(), String> {
    let Some(session) = session_involving(ctx, actor.guid).filter(|s| s.open) else {
        return Ok(());
    };
    if trade_slot > WILL_NOT_BE_TRADED_SLOT {
        return Err(format!("bad trade slot {trade_slot}"));
    }
    if inv_slot < lyracore_shared::constants::starter_item::BACKPACK_SLOT_0 {
        return Err("equipped items cannot be traded".to_string());
    }
    if inv_slot >= lyracore_shared::constants::starter_item::BACKPACK_SLOT_0 + 16 {
        return Err("bag items cannot be traded yet".to_string());
    }
    let inst = crate::items::item_in_slot(ctx, actor.guid, inv_slot).ok_or("no item in slot")?;
    if inst.soulbound {
        push_offer_events(ctx, &session, actor.guid);
        return Ok(());
    }
    let slots = ctx.db.game_trade_slot();
    for row in slots.by_session().filter(&session.id).collect::<Vec<_>>() {
        if row.owner_guid != actor.guid {
            continue;
        }
        if row.item_guid == inst.guid && row.trade_slot != trade_slot {
            push_offer_events(ctx, &session, actor.guid);
            return Ok(());
        }
        if row.trade_slot == trade_slot {
            slots.id().delete(row.id);
        }
    }
    slots.insert(TradeSlot {
        id: 0,
        session_id: session.id,
        owner_guid: actor.guid,
        trade_slot,
        item_guid: inst.guid,
    });
    let session = reset_accepts(ctx, session);
    let session = touch_session(ctx, session);
    push_offer_events(ctx, &session, actor.guid);
    Ok(())
}

pub(crate) fn apply_clear_trade_item(
    ctx: &ReducerContext,
    actor: WorldEntity,
    trade_slot: u8,
) -> Result<(), String> {
    let Some(session) = session_involving(ctx, actor.guid).filter(|s| s.open) else {
        return Ok(());
    };
    if trade_slot > WILL_NOT_BE_TRADED_SLOT {
        return Err(format!("bad trade slot {trade_slot}"));
    }
    let slots = ctx.db.game_trade_slot();
    let mut removed = false;
    for row in slots.by_session().filter(&session.id).collect::<Vec<_>>() {
        if row.owner_guid == actor.guid && row.trade_slot == trade_slot {
            slots.id().delete(row.id);
            removed = true;
        }
    }
    if !removed {
        return Ok(());
    }
    let session = reset_accepts(ctx, session);
    let session = touch_session(ctx, session);
    push_offer_events(ctx, &session, actor.guid);
    Ok(())
}

pub(crate) fn apply_set_trade_gold(
    ctx: &ReducerContext,
    actor: WorldEntity,
    copper: u32,
) -> Result<(), String> {
    let Some(mut session) = session_involving(ctx, actor.guid).filter(|s| s.open) else {
        return Ok(());
    };
    let current = if actor.guid == session.initiator_guid {
        session.initiator_gold
    } else {
        session.target_gold
    };
    if current == copper {
        return Ok(());
    }
    if actor.guid == session.initiator_guid {
        session.initiator_gold = copper;
    } else {
        session.target_gold = copper;
    }
    let session = reset_accepts(ctx, session);
    let session = touch_session(ctx, session);
    push_offer_events(ctx, &session, actor.guid);
    Ok(())
}

pub(crate) fn apply_begin_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    let sessions = ctx.db.game_trade_session();
    let Some(mut session) = sessions
        .by_target()
        .filter(&actor.guid)
        .next()
        .filter(|s| !s.open)
    else {
        return Ok(());
    };
    session.open = true;
    session.created_at = ctx.timestamp;
    let (initiator, target) = (session.initiator_guid, session.target_guid);
    sessions.id().update(session);
    push_trade_event(ctx, initiator, event_kind::OPEN_WINDOW, target);
    push_trade_event(ctx, target, event_kind::OPEN_WINDOW, initiator);
    Ok(())
}

pub(crate) fn apply_cancel_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    cancel_trade_for(ctx, actor.guid);
    Ok(())
}

pub(crate) const TRADE_IDLE_TTL_MICROS: i64 = crate::INVITE_TTL_MICROS;

pub(crate) fn session_is_stale(last_action_micros: i64, now_micros: i64) -> bool {
    now_micros - last_action_micros > TRADE_IDLE_TTL_MICROS
}

pub(crate) fn apply_busy_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    decline_proposal(ctx, actor.guid, event_kind::BUSY);
    Ok(())
}

pub(crate) fn apply_ignore_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    decline_proposal(ctx, actor.guid, event_kind::IGNORE_YOU);
    Ok(())
}

fn decline_proposal(ctx: &ReducerContext, target_guid: u64, kind: u8) {
    let Some(session) = ctx
        .db
        .game_trade_session()
        .by_target()
        .filter(&target_guid)
        .next()
        .filter(|s| !s.open)
    else {
        return;
    };
    remove_session(ctx, &session);
    push_trade_event(ctx, session.initiator_guid, kind, session.target_guid);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommitSide {
    pub guid: u64,
    pub gold_offered: u32,
    pub gold_balance: u32,
    pub items_offered: u32,
    pub free_bag_slots: u32,
    pub offers_soulbound: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitRefusal {
    GoldShort { side: u64 },
    GoldOverflow { side: u64 },
    Soulbound { side: u64 },
    InventoryFull { side: u64 },
}

pub(crate) fn gold_after(balance: u32, outgoing: u32, incoming: u32) -> Option<u32> {
    if outgoing > balance {
        return None;
    }
    u32::try_from(balance as u64 - outgoing as u64 + incoming as u64).ok()
}

pub(crate) fn commit_verdict(a: &CommitSide, b: &CommitSide) -> Result<(), CommitRefusal> {
    for side in [a, b] {
        if side.gold_offered > side.gold_balance {
            return Err(CommitRefusal::GoldShort { side: side.guid });
        }
        if side.offers_soulbound {
            return Err(CommitRefusal::Soulbound { side: side.guid });
        }
    }
    for (side, other) in [(a, b), (b, a)] {
        if gold_after(side.gold_balance, side.gold_offered, other.gold_offered).is_none() {
            return Err(CommitRefusal::GoldOverflow { side: side.guid });
        }
        if side.free_bag_slots + side.items_offered < other.items_offered {
            return Err(CommitRefusal::InventoryFull { side: side.guid });
        }
    }
    Ok(())
}

pub(crate) fn mutation_clears_accepts(initiator_accepted: bool, target_accepted: bool) -> bool {
    initiator_accepted || target_accepted
}

pub(crate) fn apply_accept_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    let Some(mut session) = session_involving(ctx, actor.guid).filter(|s| s.open) else {
        return Ok(());
    };
    let i_am_initiator = actor.guid == session.initiator_guid;
    if i_am_initiator {
        session.initiator_accepted = true;
    } else {
        session.target_accepted = true;
    }
    if session.initiator_accepted && session.target_accepted {
        return run_trade_commit(ctx, session, &actor);
    }
    let partner = if i_am_initiator {
        session.target_guid
    } else {
        session.initiator_guid
    };
    let me = actor.guid;
    touch_session(ctx, session);
    push_trade_event(ctx, partner, event_kind::TRADE_ACCEPT, me);
    Ok(())
}

pub(crate) fn apply_unaccept_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    let Some(mut session) = session_involving(ctx, actor.guid).filter(|s| s.open) else {
        return Ok(());
    };
    // Clear even after rollback because the client can still believe it accepted.
    let i_am_initiator = actor.guid == session.initiator_guid;
    if i_am_initiator {
        session.initiator_accepted = false;
    } else {
        session.target_accepted = false;
    }
    let partner = if i_am_initiator {
        session.target_guid
    } else {
        session.initiator_guid
    };
    let me = actor.guid;
    touch_session(ctx, session);
    push_trade_event(ctx, partner, event_kind::BACK_TO_TRADE, me);
    Ok(())
}

fn gather_commit_side(
    ctx: &ReducerContext,
    session: &TradeSession,
    entity: &WorldEntity,
) -> (CommitSide, Vec<crate::ItemInstance>) {
    let gold_offered = if entity.guid == session.initiator_guid {
        session.initiator_gold
    } else {
        session.target_gold
    };
    let mut items = Vec::new();
    let mut offers_soulbound = false;
    for row in ctx.db.game_trade_slot().by_session().filter(&session.id) {
        if row.owner_guid != entity.guid || row.trade_slot >= WILL_NOT_BE_TRADED_SLOT {
            continue;
        }
        if let Some(inst) = ctx.db.game_item_instance().guid().find(row.item_guid) {
            if inst.owner_guid == entity.guid {
                offers_soulbound |= inst.soulbound;
                items.push(inst);
            }
        }
    }
    (
        CommitSide {
            guid: entity.guid,
            gold_offered,
            gold_balance: entity.money,
            items_offered: items.len() as u32,
            free_bag_slots: crate::items::count_free_inventory_slots(ctx, entity.guid),
            offers_soulbound,
        },
        items,
    )
}

fn run_trade_commit(
    ctx: &ReducerContext,
    session: TradeSession,
    acceptor: &WorldEntity,
) -> Result<(), String> {
    let partner_guid = if acceptor.guid == session.initiator_guid {
        session.target_guid
    } else {
        session.initiator_guid
    };
    let partner = crate::helpers::acting_entity_by_guid(ctx, partner_guid);
    if let Err(denied) = player_interaction_gate(acceptor, partner.as_ref()) {
        push_trade_event(ctx, acceptor.guid, gate_refusal_kind(denied), partner_guid);
        let session = reset_accepts(ctx, session);
        touch_session(ctx, session);
        return Ok(());
    }
    let partner = partner.expect("gate passed, partner present");

    let (acceptor_side, acceptor_items) = gather_commit_side(ctx, &session, acceptor);
    let (partner_side, partner_items) = gather_commit_side(ctx, &session, &partner);
    match commit_verdict(&acceptor_side, &partner_side) {
        Err(CommitRefusal::InventoryFull { side }) => {
            remove_session(ctx, &session);
            let (full, other) = if side == acceptor.guid {
                (acceptor.guid, partner.guid)
            } else {
                (partner.guid, acceptor.guid)
            };
            push_trade_event(ctx, full, event_kind::INV_FULL_SELF, other);
            push_trade_event(ctx, other, event_kind::INV_FULL_PARTNER, full);
            Ok(())
        }
        Err(CommitRefusal::GoldShort { side }) | Err(CommitRefusal::GoldOverflow { side }) => {
            remove_session(ctx, &session);
            let (failed, other) = if side == acceptor.guid {
                (acceptor.guid, partner.guid)
            } else {
                (partner.guid, acceptor.guid)
            };
            push_trade_event(ctx, failed, event_kind::GOLD_FAIL_SELF, other);
            push_trade_event(ctx, other, event_kind::GOLD_FAIL_PARTNER, failed);
            Ok(())
        }
        Err(CommitRefusal::Soulbound { .. }) => {
            cancel_trade_for(ctx, session.initiator_guid);
            Ok(())
        }
        Ok(()) => {
            // Reserve incoming GUIDs before removing outgoing rows so identities cannot be reused.
            let mut partner_guids =
                crate::items::allocate_item_guids(ctx, acceptor_items.len() + partner_items.len())?;
            let acceptor_guids = partner_guids.split_off(acceptor_items.len());
            // Remove both outgoing offers first; their freed slots count toward space.
            for inst in acceptor_items.iter().chain(partner_items.iter()) {
                crate::ItemInstance::delete(ctx, inst.guid);
            }
            for (inst, guid) in acceptor_items.iter().zip(partner_guids) {
                let tmpl = ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(inst.entry)
                    .ok_or_else(|| format!("trade commit: missing item template {}", inst.entry))?;
                crate::items::store_instance_state(
                    ctx,
                    partner.guid,
                    partner.owner_identity,
                    &tmpl,
                    Some(guid),
                    &crate::items::ItemSnapshot::from(inst),
                )?;
            }
            for (inst, guid) in partner_items.iter().zip(acceptor_guids) {
                let tmpl = ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(inst.entry)
                    .ok_or_else(|| format!("trade commit: missing item template {}", inst.entry))?;
                crate::items::store_instance_state(
                    ctx,
                    acceptor.guid,
                    acceptor.owner_identity,
                    &tmpl,
                    Some(guid),
                    &crate::items::ItemSnapshot::from(inst),
                )?;
            }
            let entities = ctx.db.game_world_entity();
            let mut acceptor_row = entities
                .guid()
                .find(acceptor.guid)
                .ok_or("trade commit: acceptor vanished mid-transaction")?;
            acceptor_row.money = gold_after(
                acceptor_row.money,
                acceptor_side.gold_offered,
                partner_side.gold_offered,
            )
            .ok_or("trade commit: gold arithmetic refused")?;
            acceptor_row.update(ctx);
            let mut partner_row = entities
                .guid()
                .find(partner.guid)
                .ok_or("trade commit: partner vanished mid-transaction")?;
            partner_row.money = gold_after(
                partner_row.money,
                partner_side.gold_offered,
                acceptor_side.gold_offered,
            )
            .ok_or("trade commit: gold arithmetic refused")?;
            partner_row.update(ctx);

            remove_session(ctx, &session);
            push_trade_event(ctx, acceptor.guid, event_kind::TRADE_COMPLETE, partner.guid);
            push_trade_event(ctx, partner.guid, event_kind::TRADE_COMPLETE, acceptor.guid);
            Ok(())
        }
    }
}

pub(crate) fn cancel_trade_for(ctx: &ReducerContext, guid: u64) {
    let Some(session) = session_involving(ctx, guid) else {
        return;
    };
    remove_session(ctx, &session);
    push_trade_event(
        ctx,
        session.initiator_guid,
        event_kind::TRADE_CANCELED,
        session.target_guid,
    );
    push_trade_event(
        ctx,
        session.target_guid,
        event_kind::TRADE_CANCELED,
        session.initiator_guid,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gold_after_refuses_underflow_and_the_u32_cap_exactly() {
        assert_eq!(gold_after(100, 30, 5), Some(75));
        assert_eq!(gold_after(100, 100, 0), Some(0), "empty the purse exactly");
        assert_eq!(gold_after(100, 101, 0), None, "offering more than held");
        assert_eq!(
            gold_after(u32::MAX - 10, 0, 10),
            Some(u32::MAX),
            "landing exactly on the cap is legal"
        );
        assert_eq!(
            gold_after(u32::MAX - 10, 0, 11),
            None,
            "one past the cap refuses"
        );
        assert_eq!(
            gold_after(u32::MAX, u32::MAX, u32::MAX),
            Some(u32::MAX),
            "full swap at the cap stays in range"
        );
    }

    #[test]
    fn commit_verdict_names_the_refusing_side_for_each_floor_check() {
        let side = |guid, gold_offered, gold_balance, items_offered, free_bag_slots| CommitSide {
            guid,
            gold_offered,
            gold_balance,
            items_offered,
            free_bag_slots,
            offers_soulbound: false,
        };

        let a = side(1, 50, 100, 1, 0);
        let b = side(2, 0, 20, 1, 0);
        assert_eq!(commit_verdict(&a, &b), Ok(()));

        assert_eq!(
            commit_verdict(&side(1, 101, 100, 0, 16), &b),
            Err(CommitRefusal::GoldShort { side: 1 })
        );
        assert_eq!(
            commit_verdict(&a, &side(2, 21, 20, 0, 16)),
            Err(CommitRefusal::GoldShort { side: 2 })
        );

        assert_eq!(
            commit_verdict(&side(1, 50, 100, 0, 16), &side(2, 0, u32::MAX - 49, 0, 16)),
            Err(CommitRefusal::GoldOverflow { side: 2 })
        );

        let mut sb = side(1, 0, 0, 1, 16);
        sb.offers_soulbound = true;
        assert_eq!(
            commit_verdict(&sb, &b),
            Err(CommitRefusal::Soulbound { side: 1 })
        );

        assert_eq!(
            commit_verdict(&side(1, 0, 0, 2, 0), &side(2, 0, 0, 2, 0)),
            Ok(())
        );
        assert_eq!(
            commit_verdict(&side(1, 0, 0, 1, 0), &side(2, 0, 0, 2, 0)),
            Err(CommitRefusal::InventoryFull { side: 1 })
        );
    }

    #[test]
    fn any_offer_mutation_clears_accepts_when_either_party_accepted() {
        assert!(!mutation_clears_accepts(false, false));
        assert!(mutation_clears_accepts(true, false));
        assert!(mutation_clears_accepts(false, true));
        assert!(mutation_clears_accepts(true, true));
    }

    #[test]
    fn a_session_is_stale_strictly_past_the_ttl_since_its_last_action() {
        let ttl = TRADE_IDLE_TTL_MICROS;
        assert!(!session_is_stale(1_000, 1_000 + ttl));
        assert!(session_is_stale(1_000, 1_000 + ttl + 1));
        assert!(!session_is_stale(1_000, 999), "clock skew never reaps");
    }

    #[test]
    fn offer_slot_view_joins_instance_state_with_template_identity() {
        let mut tmpl = crate::items::tests::blank_template(2589);
        tmpl.display_id = 7026;
        tmpl.max_durability = 40;
        let inst = crate::ItemInstance {
            guid: 0x4000_0001,
            entry: 2589,
            owner_identity: Identity::ZERO,
            owner_guid: 1,
            slot: 23,
            stack_count: 20,
            durability: 34,
            created_at: Timestamp::UNIX_EPOCH,
            enchant_id: 2564,
            soulbound: false,
            random_property_id: 117,
            item_text_id: 0,
        };
        assert_eq!(
            offer_slot_view(6, &inst, &tmpl),
            lyracore_shared::trade::OfferSlot {
                trade_slot: 6,
                entry: 2589,
                display_id: 7026,
                stack_count: 20,
                enchantment: 2564,
                durability: 34,
                max_durability: 40,
                random_property_id: 117,
            }
        );
    }

    #[test]
    fn initiate_verdict_names_each_refusal_and_holds_the_check_order() {
        use PlayerInteractionDenied as D;
        let ok: Result<(), D> = Ok(());

        assert_eq!(initiate_verdict(ok, false, true, false, false), Ok(()));

        assert_eq!(
            initiate_verdict(Err(D::ActorDead), false, true, false, false),
            Err(event_kind::YOU_DEAD)
        );
        assert_eq!(
            initiate_verdict(Err(D::NoTarget), false, true, false, false),
            Err(event_kind::NO_TARGET)
        );
        assert_eq!(
            initiate_verdict(Err(D::TargetNotPlayer), false, true, false, false),
            Err(event_kind::NO_TARGET)
        );
        assert_eq!(
            initiate_verdict(Err(D::TargetDead), false, true, false, false),
            Err(event_kind::TARGET_DEAD)
        );
        assert_eq!(
            initiate_verdict(Err(D::DifferentPartition), false, true, false, false),
            Err(event_kind::TARGET_TO_FAR)
        );
        assert_eq!(
            initiate_verdict(Err(D::OutOfRange), false, true, false, false),
            Err(event_kind::TARGET_TO_FAR)
        );

        assert_eq!(
            initiate_verdict(ok, true, true, false, false),
            Err(event_kind::IGNORE_YOU)
        );

        assert_eq!(
            initiate_verdict(ok, false, false, false, false),
            Err(event_kind::WRONG_FACTION)
        );
        assert_eq!(
            initiate_verdict(ok, false, true, true, false),
            Err(event_kind::BUSY)
        );
        assert_eq!(
            initiate_verdict(ok, false, true, false, true),
            Err(event_kind::BUSY)
        );

        assert_eq!(
            initiate_verdict(Err(D::ActorDead), true, false, true, true),
            Err(event_kind::YOU_DEAD)
        );
        assert_eq!(
            initiate_verdict(ok, true, false, true, true),
            Err(event_kind::IGNORE_YOU)
        );
        assert_eq!(
            initiate_verdict(ok, false, false, true, true),
            Err(event_kind::WRONG_FACTION)
        );
    }
}
