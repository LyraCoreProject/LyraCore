//! Player-to-player trading (#8) — the Trade Session handshake (#120).
//!
//! A **Trade Session** (see `CONTEXT.md`) is transient module state, the `GroupInvite` template:
//! private rows, defensive teardown, never Escrow — after a crash or teardown the trade simply
//! never happened. The **Trade Commit** (the atomic item/gold swap, #122) and the offer relay
//! (#121) build on the tables here; this file lands the handshake: initiate → `BeginTrade` to the
//! target → `CMSG_BEGIN_TRADE` → `OpenWindow` to both → cancel/logout → `TradeCanceled` to both.
//!
//! All trade statuses — success AND refusal, both parties — ride ONE path: recipient-keyed
//! [`TradeEvent`] rows relayed by the gateway (the coordinator-relay law; the gateway cannot
//! address another player's socket). A refusal is a normal protocol answer to the INITIATOR, not
//! a reducer `Err`: `Err` here is reserved for actor-resolution failures (`gw.rs` convention).

use spacetimedb::{table, Identity, ReducerContext, Table, Timestamp};

use lyracore_shared::trade::event_kind;

use crate::helpers::{player_interaction_gate, PlayerInteractionDenied};
use crate::{
    game_character_contact, game_faction_template, game_item_instance, game_item_template,
    game_world_entity, WorldEntity,
};

/// One Trade Session per player PAIR — and at most one per player, in either seat (the
/// one-session-per-player invariant, enforced by [`initiate_verdict`]'s busy check). `open` is the
/// handshake state: false = proposed (`BeginTrade` sent, window not yet open), true = both windows
/// open. The accept flags and per-seat gold are the Trade Commit's state (#122): both true runs
/// [`run_trade_commit`]; any offer mutation clears both ([`reset_accepts`]). Private — clients see
/// trade state only through [`TradeEvent`]. Reaped on idle (#123, `gc.rs`); torn down on logout,
/// death, and transfer. [entity]
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

// Character-owned sweep: a deleted character's Trade Sessions go with it, in either seat (the
// GroupInvite pattern — the table only ever holds live negotiations, so both scans stay tiny).
// Delete-only: no owner_identity to restamp.
crate::character_owned!(delete, fn sweep_delete_game_trade_session(ctx, character_guid) {
    let sessions = ctx.db.game_trade_session();
    for s in sessions.by_initiator().filter(&character_guid).collect::<Vec<_>>() {
        remove_session(ctx, &s);
    }
    for s in sessions.by_target().filter(&character_guid).collect::<Vec<_>>() {
        remove_session(ctx, &s);
    }
});
// CROSS-DATABASE transport: a Trade Session is a live dialog between two players standing next to
// each other, and the partner is by definition NOT transferring with the character. Not
// transported — the session dies with the source copy, exactly as the logout teardown would (the
// in-transit fence already makes the transferring partner read absent).
crate::character_owned!(not_transported, fn sweep_transfer_game_trade_session());

/// One offered item in a Trade Session — child rows of [`TradeSession`] (the one-row-plus-child
/// idiom; a slot column per trade slot would be a fixed-width dead end). `trade_slot` 0..=5 are
/// the traded slots; 6 is the Will-Not-Be-Traded Slot, shown but never committed. Unused until
/// #121 sets offers — landed with the session table so the schema chore runs once. Private. [entity]
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

// Character-owned sweep: slot rows die with their session; this direct owner_guid sweep is the
// belt to that braces for a character deleted mid-negotiation. Delete-only.
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

/// A per-recipient trade-status notification (the `game_group_event` pattern): public, reaped by
/// the shared 1s event GC. `kind` is `lyracore_shared::trade::event_kind`; `other_guid` is the
/// counterparty (the wire payload for `BeginTrade`). `recipient_guid` is what the gateway's
/// shared-connection relay filters on (`private_recipient_audience`); `recipient_identity` keeps
/// the per-player RLS shape of its siblings. [event]
#[table(accessor = game_trade_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct TradeEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub kind: u8, // lyracore_shared::trade::event_kind::*
    pub other_guid: u64,
    pub created_at: Timestamp,
    pub recipient_guid: u64,
    // OFFER_* snapshot (`lyracore_shared::trade::encode_offer`); empty for the status kinds. A
    // plain column (String cannot be `#[default]`-ed — the macro's typecheck is const): fine
    // because this 1s-TTL table has no pre-payload row to migrate anywhere real (the
    // `game_group_event.payload` precedent).
    pub payload: String,
}

/// Address one trade status to `recipient_guid` (the `group::push_event` shape, minus the name
/// resolution — no trade status carries a name). The identity resolves through the
/// [`character_by_guid`] fence: a mid-transfer recipient reads absent and the row is addressed to
/// `Identity::ZERO` (nobody), which is the right delivery for a partner who just left the shard.
fn push_trade_event(ctx: &ReducerContext, recipient_guid: u64, kind: u8, other_guid: u64) {
    push_trade_event_payload(ctx, recipient_guid, kind, other_guid, String::new());
}

/// [`push_trade_event`] with an `OFFER_*` snapshot payload (#121).
fn push_trade_event_payload(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    other_guid: u64,
    payload: String,
) {
    let bound = crate::helpers::character_by_guid(ctx, recipient_guid).map(|c| c.owner_identity);
    ctx.db.game_trade_event().insert(TradeEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        kind,
        other_guid,
        created_at: ctx.timestamp,
        recipient_guid,
        payload,
    });
}

/// The Trade Session `guid` is in, either seat — the busy probe and the cancel target.
pub(crate) fn session_involving(ctx: &ReducerContext, guid: u64) -> Option<TradeSession> {
    let sessions = ctx.db.game_trade_session();
    sessions
        .by_initiator()
        .filter(&guid)
        .next()
        .or_else(|| sessions.by_target().filter(&guid).next())
}

/// Delete a session and its slot rows — the one spelling of teardown, shared by cancel, the
/// character sweep, and (#122) the Trade Commit.
fn remove_session(ctx: &ReducerContext, session: &TradeSession) {
    let slots = ctx.db.game_trade_slot();
    for slot in slots.by_session().filter(&session.id).collect::<Vec<_>>() {
        slots.id().delete(slot.id);
    }
    ctx.db.game_trade_session().id().delete(session.id);
}

/// A player-interaction-gate refusal as its wire kind — shared by the initiate verdict and the
/// commit-time re-validation (#122), so the two moments can never name the same failure
/// differently.
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

/// The pure initiate decision (the transfer-harness lesson: decisions pure, reducers thin): the
/// player-interaction gate's answer + the faction and busy probes → the refusal `event_kind` the
/// INITIATOR is told, or `Ok` to propose. Check order is pinned by the test below: gate refusals
/// outrank faction, faction outranks busy.
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
    // The AUTO-decline half of ignore (#123): the target never sees a proposal from someone they
    // ignore — the server answers IgnoreYou at initiate time (the whisper-drop precedent). The
    // client-volunteered CMSG_IGNORE_TRADE relay is the other half.
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

/// `CMSG_INITIATE_TRADE` core: validate via [`initiate_verdict`], then either tell the initiator
/// why not, or record the proposed Trade Session and tell the target `BeginTrade`. Reached via
/// `gw::gw_initiate_trade`; registered as the `initiate_trade` actor verb.
pub(crate) fn apply_initiate_trade(
    ctx: &ReducerContext,
    actor: WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    // Self-trade would PASS the interaction gate (you are a living player standing 0 yd from
    // yourself); refuse it first, as the absent-partner answer.
    if target_guid == actor.guid {
        push_trade_event(ctx, actor.guid, event_kind::NO_TARGET, target_guid);
        return Ok(());
    }
    // The FENCED partner lookup: a mid-transfer or offline partner reads absent → NoTarget.
    let target = crate::helpers::acting_entity_by_guid(ctx, target_guid);
    // Same fail-open convention as `world::apply_inspect`: no FactionTemplate data (dev/test
    // server) never blocks a trade. `true` for an absent partner — the gate refuses first.
    let friendly = ctx.db.game_faction_template().count() == 0
        || target.as_ref().is_none_or(|t| {
            crate::faction::is_friendly(ctx, actor.faction_template, t.faction_template)
        });
    // The whisper-enforcement shape (`chat::send_whisper`): does the TARGET have the initiator
    // on their ignore list?
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

/// The client's 7th window slot — shown to the partner, never committed (see `CONTEXT.md`,
/// Will-Not-Be-Traded Slot).
pub(crate) const WILL_NOT_BE_TRADED_SLOT: u8 = 6;

/// One offered item as the window renders it — the pure instance+template → wire-fields join
/// (the #121 "real stack count, durability, enchant" AC), factored out so the field mapping is
/// testable without a `ReducerContext`. `max_durability` comes from the TEMPLATE (instances only
/// carry current durability).
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
    }
}

/// Bump the session's `created_at` — every offer action refreshes it, which is what makes the
/// idle-TTL reap (#123) mean "2 minutes since the last ACTION", not "since the handshake".
/// Takes and returns the row (`update` consumes it) so callers keep a current copy without a
/// `Clone` bound.
fn touch_session(ctx: &ReducerContext, mut session: TradeSession) -> TradeSession {
    session.created_at = ctx.timestamp;
    ctx.db.game_trade_session().id().update(session)
}

/// Relay `offerer_guid`'s whole current offer to both windows (#121): one snapshot payload, two
/// rows — `OFFER_SELF` back to the offerer, `OFFER_PARTNER` to the other seat. A slot row whose
/// item no longer resolves (consumed/destroyed while offered) is deleted here rather than relayed
/// — self-healing, the same posture as the movement relay's ghost rows.
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
    push_trade_event_payload(ctx, partner, event_kind::OFFER_PARTNER, offerer_guid, payload);
}

/// The accept-reset rule's impure half (#122): if either party had accepted, clear BOTH flags
/// and tell both windows `BackToTrade`. Returns the (possibly updated) session copy.
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

/// `CMSG_SET_TRADE_ITEM` core (#121): place the item in inventory slot `inv_slot` into window
/// slot `trade_slot`. The gateway already mapped the client's (bag, slot) addressing onto the
/// absolute slot (the item-family convention). No open session is a silent no-op (client races a
/// cancel); a bad slot, an empty inventory slot, a soulbound item, or an item already offered in
/// another window slot is a per-action `Err` the gateway logs.
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
    // Only loose BACKPACK items (23..=38) can be offered. Below: equipment/containers — vanilla
    // requires the item loose, the client enforces it. Above: equipped-bag CONTENTS — the
    // gateway doesn't model sub-bags yet (`handlers/item.rs`), and the commit's room accounting
    // (`count_free_backpack_slots`) is backpack-only; letting a bag item in would let the verdict
    // count room its delivery can't use.
    if inv_slot < lyracore_shared::constants::starter_item::BACKPACK_SLOT_0 {
        return Err("equipped items cannot be traded".to_string());
    }
    if inv_slot >= lyracore_shared::constants::starter_item::BACKPACK_SLOT_0 + 16 {
        return Err("bag items cannot be traded yet".to_string());
    }
    let inst =
        crate::items::item_in_slot(ctx, actor.guid, inv_slot).ok_or("no item in slot")?;
    // `ItemInstance.soulbound` is the data model this check enforces (its doc names this work).
    // Refused with a CORRECTIVE echo, not an `Err`: an `Err` rolls the transaction back, so no
    // packet could reach the client and a locally-placed phantom would sit in its own pane. The
    // unchanged snapshot re-syncs both windows instead.
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
            // Same corrective-echo posture as the soulbound refusal above.
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

/// `CMSG_CLEAR_TRADE_ITEM` core (#121). Clearing an already-empty slot still echoes the snapshot
/// — cheap, and the client stays authoritative-synced.
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
    // Clearing an already-empty slot is a no-op, and a no-op is not a change — it must not
    // trip the accept-reset rule.
    if !removed {
        return Ok(());
    }
    let session = reset_accepts(ctx, session);
    let session = touch_session(ctx, session);
    push_offer_events(ctx, &session, actor.guid);
    Ok(())
}

/// `CMSG_SET_TRADE_GOLD` core (#121): record the offered copper on the actor's seat. Balance is
/// NOT checked here — the Trade Commit (#122) re-validates everything at the only moment it
/// matters, and the client caps its own input meanwhile.
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
    // Re-sending the same amount is a no-op, not a change — the accept-reset rule stays quiet.
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

/// `CMSG_BEGIN_TRADE` core: the target's client answered the proposal — open both windows. A
/// missing or already-open session is silently fine (client races a cancel); vanilla sends
/// nothing either. Reached via `gw::gw_begin_trade`; the `begin_trade` actor verb.
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
    // Opening counts as a trade action for the #123 idle reap.
    session.created_at = ctx.timestamp;
    let (initiator, target) = (session.initiator_guid, session.target_guid);
    sessions.id().update(session);
    push_trade_event(ctx, initiator, event_kind::OPEN_WINDOW, target);
    push_trade_event(ctx, target, event_kind::OPEN_WINDOW, initiator);
    Ok(())
}

/// `CMSG_CANCEL_TRADE` core. Reached via `gw::gw_cancel_trade`; the `cancel_trade` actor verb.
pub(crate) fn apply_cancel_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    cancel_trade_for(ctx, actor.guid);
    Ok(())
}

/// How long an idle Trade Session lives (#123). Its own named knob so trade idleness never
/// silently rides the group-invite dial — initialized FROM it because both are "a human is
/// looking at a dialog" windows today.
pub(crate) const TRADE_IDLE_TTL_MICROS: i64 = crate::INVITE_TTL_MICROS;

/// Is a session idle past [`TRADE_IDLE_TTL_MICROS`] (#123)? STRICTLY past — an exactly-at-TTL
/// session survives one more GC pass. `last_action_micros` is `created_at`, which
/// [`touch_session`] bumps on every trade action, so this measures idleness since the last
/// action, not total age. Pure; the sweep itself lives in `gc.rs` (the group-invite block shape).
pub(crate) fn session_is_stale(last_action_micros: i64, now_micros: i64) -> bool {
    now_micros - last_action_micros > TRADE_IDLE_TTL_MICROS
}

/// `CMSG_BUSY_TRADE` core (#123): the proposed target's client declined as busy — kill the
/// proposal, tell the initiator `Busy`. Only a PROPOSED (not yet open) session declines this way;
/// no proposal is a silent no-op (client races). The decliner's client showed nothing, so only
/// the initiator hears a status.
pub(crate) fn apply_busy_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    decline_proposal(ctx, actor.guid, event_kind::BUSY);
    Ok(())
}

/// `CMSG_IGNORE_TRADE` core (#123): the proposed target has the initiator ignored — kill the
/// proposal, tell the initiator `IgnoreYou`. Same shape as [`apply_busy_trade`].
pub(crate) fn apply_ignore_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    decline_proposal(ctx, actor.guid, event_kind::IGNORE_YOU);
    Ok(())
}

/// Kill `target_guid`'s pending proposal (a session whose window never opened) and answer the
/// initiator with `kind` — the shared body of the two decline verbs.
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

/// One seat's commit-relevant facts (#122) — gathered impurely by [`run_trade_commit`], judged
/// purely by [`commit_verdict`] (the transfer-planner lesson: decisions pure, reducers thin).
/// `items_offered` counts TRADED slots only — the Will-Not-Be-Traded Slot never reaches a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommitSide {
    pub guid: u64,
    pub gold_offered: u32,
    pub gold_balance: u32,
    pub items_offered: u32,
    pub free_bag_slots: u32,
    pub offers_soulbound: bool,
}

/// Why a dual-accept commit refuses (#122) — `side` names the culprit seat so the wire answer
/// (whose bags, whose tampering) is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitRefusal {
    /// A side offered more copper than it holds — the client caps input, so this is tampering.
    GoldShort { side: u64 },
    /// Receipt would push a purse past `u32::MAX` — no coin may be created or destroyed.
    GoldOverflow { side: u64 },
    /// A soulbound item is in an offer (a bind raced the offer-time refusal).
    Soulbound { side: u64 },
    /// A side's bags cannot fit the incoming items, NET of the slots its own offer frees.
    InventoryFull { side: u64 },
}

/// The purse after a commit leg: `balance - outgoing + incoming`, or `None` on underflow (more
/// offered than held) or past the u32 cap. The ONE spelling of the swap arithmetic — the verdict
/// refuses with it and the execution applies with it, so they can never disagree. Pure.
pub(crate) fn gold_after(balance: u32, outgoing: u32, incoming: u32) -> Option<u32> {
    if outgoing > balance {
        return None;
    }
    u32::try_from(balance as u64 - outgoing as u64 + incoming as u64).ok()
}

/// The pure commit gate (#122): tampered gold, soulbound, purse overflow, and net bag space,
/// checked a-side first (deterministic culprit). Presence/range re-validation happens before the
/// facts are even gathered (the player-interaction gate at commit time); faction cannot change.
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
        // GoldShort is excluded above, so a `None` here can only be the cap.
        if gold_after(side.gold_balance, side.gold_offered, other.gold_offered).is_none() {
            return Err(CommitRefusal::GoldOverflow { side: side.guid });
        }
        if side.free_bag_slots + side.items_offered < other.items_offered {
            return Err(CommitRefusal::InventoryFull { side: side.guid });
        }
    }
    Ok(())
}

/// The accept-reset rule's pure half (#122): does this offer mutation clear the accept flags?
/// Any change after EITHER accept voids both — the anti-scam floor.
pub(crate) fn mutation_clears_accepts(initiator_accepted: bool, target_accepted: bool) -> bool {
    initiator_accepted || target_accepted
}

/// `CMSG_ACCEPT_TRADE` core (#122): set the actor's accept flag; when BOTH seats have accepted,
/// run the Trade Commit in this same transaction. Until then the partner hears `TradeAccept`.
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

/// `CMSG_UNACCEPT_TRADE` core (#122): withdraw the actor's accept; the partner hears
/// `BackToTrade` (their own client already knows). A no-accept unaccept is a silent no-op.
pub(crate) fn apply_unaccept_trade(ctx: &ReducerContext, actor: WorldEntity) -> Result<(), String> {
    let Some(mut session) = session_involving(ctx, actor.guid).filter(|s| s.open) else {
        return Ok(());
    };
    // UNCONDITIONAL clear — deliberately no had-accepted early return: a rolled-back commit can
    // leave a client believing it accepted while the flag is false, and an idempotent unaccept is
    // that window's one-click recovery (the reducer-Err paths cannot status the client — an `Err`
    // rolls the event rows back with everything else).
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

/// One seat's commit facts + the offered instances behind them, gathered in-transaction.
/// A slot row whose item vanished or changed owner is SKIPPED (the push_offer_events self-heal
/// posture) — it simply isn't part of the swap.
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
        // The Will-Not-Be-Traded Slot (6) is shown, never committed.
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
            free_bag_slots: crate::items::count_free_backpack_slots(ctx, entity.guid),
            offers_soulbound,
        },
        items,
    )
}

/// Mint the received item on `receiver` — new guid (the giver's client watched the destroy),
/// instance state COPIED (stack, durability, enchant: the #8 free-repair rule), soulbound stays
/// false (soulbound never passes the gates). An `Err` here rolls the WHOLE commit back — the
/// verdict guaranteed room, so a miss means concurrent mutation and nothing may move.
fn deliver_traded_item(
    ctx: &ReducerContext,
    receiver: &WorldEntity,
    inst: &crate::ItemInstance,
    guid: u64,
) -> Result<(), String> {
    let slot = crate::items::first_free_backpack_slot(ctx, receiver.guid)
        .ok_or("trade commit: receiver bags filled mid-transaction")?;
    ctx.db.game_item_instance().insert(crate::ItemInstance {
        guid,
        entry: inst.entry,
        owner_identity: receiver.owner_identity,
        owner_guid: receiver.guid,
        slot,
        stack_count: inst.stack_count,
        durability: inst.durability,
        created_at: ctx.timestamp,
        enchant_id: inst.enchant_id,
        soulbound: false,
    });
    Ok(())
}

/// The Trade Commit (#122): re-validate presence/range, judge the pure [`commit_verdict`], then
/// swap items and gold atomically — the reducer transaction IS the atomicity; any `Err` rolls
/// everything back and nothing moves. `acceptor` is the seat whose accept completed the pair
/// (stale-window refusals answer them).
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
    // The stale-window guard: presence + range re-checked at the only moment that matters.
    let partner = crate::helpers::acting_entity_by_guid(ctx, partner_guid);
    if let Err(denied) = player_interaction_gate(acceptor, partner.as_ref()) {
        push_trade_event(ctx, acceptor.guid, gate_refusal_kind(denied), partner_guid);
        // Both accepts void; both windows return to the offer state.
        let session = reset_accepts(ctx, session);
        touch_session(ctx, session);
        return Ok(());
    }
    let partner = partner.expect("gate passed, partner present");

    let (acceptor_side, acceptor_items) = gather_commit_side(ctx, &session, acceptor);
    let (partner_side, partner_items) = gather_commit_side(ctx, &session, &partner);
    match commit_verdict(&acceptor_side, &partner_side) {
        Err(CommitRefusal::InventoryFull { side }) => {
            // The one organic refusal: the window closes with the inventory error, the full
            // side flagged on each client (`target_error` = "it was the other party").
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
            // A purse that cannot honour the offer (short = tampering, the client caps input;
            // overflow = a capped receiver). Window closes with NotEnoughMoney, culprit flagged.
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
            // A bind raced the offer-time refusal — genuinely exceptional; the trade dies.
            cancel_trade_for(ctx, session.initiator_guid);
            Ok(())
        }
        Ok(()) => {
            // Mint incoming guids from the PRE-delete high-water mark: after the deletes a seat
            // can hold zero rows, and `next_item_guid`'s birth-formula fallback could then
            // re-mint a guid this very swap just deleted — an insert on a deleted PK is an
            // UPDATE to the item relay, which renders the OLD item (the #8 ghost-item gotcha in
            // its other form). Pre-delete, max+1 can collide with nothing.
            let base_slot = lyracore_shared::constants::starter_item::BACKPACK_SLOT_0;
            let mut partner_next = crate::items::next_item_guid(ctx, partner.guid, base_slot);
            let mut acceptor_next = crate::items::next_item_guid(ctx, acceptor.guid, base_slot);
            // ALL outgoing rows leave first — their slots are the room the verdict counted on.
            let instances = ctx.db.game_item_instance();
            for inst in acceptor_items.iter().chain(partner_items.iter()) {
                instances.guid().delete(inst.guid);
            }
            for inst in &acceptor_items {
                deliver_traded_item(ctx, &partner, inst, partner_next)?;
                partner_next += 1;
            }
            for inst in &partner_items {
                deliver_traded_item(ctx, acceptor, inst, acceptor_next)?;
                acceptor_next += 1;
            }
            // Gold, both legs through the ONE arithmetic the verdict already approved, applied
            // to the freshly-read rows (in-transaction they equal the verdict's snapshot; using
            // the row's own money keeps a single source). The live `WorldEntity.money` row is
            // the coinage relay + persist mirror.
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
            entities.guid().update(acceptor_row);
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
            entities.guid().update(partner_row);

            remove_session(ctx, &session);
            push_trade_event(ctx, acceptor.guid, event_kind::TRADE_COMPLETE, partner.guid);
            push_trade_event(ctx, partner.guid, event_kind::TRADE_COMPLETE, acceptor.guid);
            Ok(())
        }
    }
}

/// Tear down `guid`'s Trade Session (if any) and tell BOTH parties `TradeCanceled` — the client
/// of the canceller expects the status too (it closes the window). Idempotent; also the logout
/// teardown (`world::remove_from_world`), where the event to the leaver is harmless dead weight
/// the 1s GC reaps.
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

    /// The swap arithmetic (#122): a purse never underflows (offering more than you hold) and
    /// never wraps past the u32 cap — `None` from either side means the commit must refuse, so
    /// no coin is ever created or destroyed. Boundaries pinned exactly.
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
        assert_eq!(gold_after(u32::MAX - 10, 0, 11), None, "one past the cap refuses");
        assert_eq!(
            gold_after(u32::MAX, u32::MAX, u32::MAX),
            Some(u32::MAX),
            "full swap at the cap stays in range"
        );
    }

    /// The commit gate (#122), enumerated per refusal: tampered gold, purse overflow, a raced
    /// soulbound, and bag space NET of the slots a side's own offer frees. The a-side is checked
    /// first, pinned so the wire kinds name a deterministic culprit.
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

        // Happy: A pays 50 of 100, B pays nothing; one item each way, no free slots needed
        // beyond what each side's own outgoing frees.
        let a = side(1, 50, 100, 1, 0);
        let b = side(2, 0, 20, 1, 0);
        assert_eq!(commit_verdict(&a, &b), Ok(()));

        // Gold short: a side offered copper it does not hold (client tampering).
        assert_eq!(
            commit_verdict(&side(1, 101, 100, 0, 16), &b),
            Err(CommitRefusal::GoldShort { side: 1 })
        );
        assert_eq!(
            commit_verdict(&a, &side(2, 21, 20, 0, 16)),
            Err(CommitRefusal::GoldShort { side: 2 })
        );

        // Overflow: B's purse would pass the cap on receipt.
        assert_eq!(
            commit_verdict(&side(1, 50, 100, 0, 16), &side(2, 0, u32::MAX - 49, 0, 16)),
            Err(CommitRefusal::GoldOverflow { side: 2 })
        );

        // Soulbound raced into an offer after the offer-time refusal.
        let mut sb = side(1, 0, 0, 1, 16);
        sb.offers_soulbound = true;
        assert_eq!(
            commit_verdict(&sb, &b),
            Err(CommitRefusal::Soulbound { side: 1 })
        );

        // Bag space is NET: zero free slots but a 2-for-2 swap fits; 2 incoming for 1 outgoing
        // with zero free does not, and the FULL side is the one named.
        assert_eq!(
            commit_verdict(&side(1, 0, 0, 2, 0), &side(2, 0, 0, 2, 0)),
            Ok(())
        );
        assert_eq!(
            commit_verdict(&side(1, 0, 0, 1, 0), &side(2, 0, 0, 2, 0)),
            Err(CommitRefusal::InventoryFull { side: 1 })
        );
    }

    /// The accept-reset rule's pure half (#122): ANY offer mutation clears the flags if either
    /// party had accepted — the anti-scam floor from the design session.
    #[test]
    fn any_offer_mutation_clears_accepts_when_either_party_accepted() {
        assert!(!mutation_clears_accepts(false, false));
        assert!(mutation_clears_accepts(true, false));
        assert!(mutation_clears_accepts(false, true));
        assert!(mutation_clears_accepts(true, true));
    }

    /// The accept-reset rule's impure half, pinned by source scan (the `economy.rs` money-line
    /// precedent — no `ReducerContext` harness can execute the mutators): all THREE offer
    /// mutators route through `reset_accepts`, plus the commit's stale-window refusal. Deleting
    /// any one call site fails here by name.
    #[test]
    fn every_offer_mutator_and_the_commit_refusal_route_through_reset_accepts() {
        let src = crate::test_scan::read_scanned("module/src/trade.rs")
            .expect("module/src/trade.rs ships in every checkout");
        // concat! so this test's own needle never counts itself (the build-scan-strip lesson).
        let needle = concat!("let session = reset_accepts", "(ctx, session);");
        let calls = src.matches(needle).count();
        assert_eq!(
            calls, 4,
            "set item, clear item, and set gold must EACH clear the accept flags on a real \
             change (the anti-scam floor), and the commit's stale-window refusal must reset \
             both seats; found {calls} reset_accepts call sites"
        );
    }

    /// The reap policy (#123): a session is stale only STRICTLY past the invite TTL since its
    /// last action — `touch_session` bumps `created_at` on every offer mutation and on window
    /// open, so this measures idleness, not total age. Boundary pinned: exactly-at-TTL survives.
    #[test]
    fn a_session_is_stale_strictly_past_the_ttl_since_its_last_action() {
        let ttl = TRADE_IDLE_TTL_MICROS;
        assert!(!session_is_stale(1_000, 1_000 + ttl));
        assert!(session_is_stale(1_000, 1_000 + ttl + 1));
        assert!(!session_is_stale(1_000, 999), "clock skew never reaps");
    }

    /// The window-visible fields come from the right source (#121): live per-instance state
    /// (stack, current durability, enchant) off the INSTANCE, static identity (display id, max
    /// durability) off the TEMPLATE. Distinct values per field so a transposition can't pass.
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
            }
        );
    }

    /// Every refusal [`initiate_verdict`] owes the initiator, its wire kind, and the pinned check
    /// order: a gate refusal (dead/absent/range) outranks WrongFaction, which outranks Busy — so
    /// a dead cross-faction player in a trade hears YouDead, not Busy.
    #[test]
    fn initiate_verdict_names_each_refusal_and_holds_the_check_order() {
        use PlayerInteractionDenied as D;
        let ok: Result<(), D> = Ok(());

        assert_eq!(initiate_verdict(ok, false, true, false, false), Ok(()));

        // Gate refusals map onto their wire kinds.
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

        // Ignore auto-decline (#123): the target never sees the proposal.
        assert_eq!(
            initiate_verdict(ok, true, true, false, false),
            Err(event_kind::IGNORE_YOU)
        );

        // Faction and busy refusals — either seat busy answers Busy.
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

        // Pinned precedence: gate > ignore > faction > busy.
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
