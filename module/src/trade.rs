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
    WorldEntity,
};

/// One Trade Session per player PAIR — and at most one per player, in either seat (the
/// one-session-per-player invariant, enforced by [`initiate_verdict`]'s busy check). `open` is the
/// handshake state: false = proposed (`BeginTrade` sent, window not yet open), true = both windows
/// open. The accept flags and gold offers are #122's Trade Commit state, landed with the table so
/// the bindings/schema chore runs once (they stay false/0 until then). Private — clients see trade
/// state only through [`TradeEvent`]. Reaped on idle by #123; torn down on logout here. [entity]
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
        return Err(match denied {
            PlayerInteractionDenied::ActorDead => event_kind::YOU_DEAD,
            PlayerInteractionDenied::NoTarget | PlayerInteractionDenied::TargetNotPlayer => {
                event_kind::NO_TARGET
            }
            PlayerInteractionDenied::TargetDead => event_kind::TARGET_DEAD,
            PlayerInteractionDenied::DifferentPartition | PlayerInteractionDenied::OutOfRange => {
                event_kind::TARGET_TO_FAR
            }
        });
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
    // Equipment (slots 0..=18) and bag containers can't be offered — vanilla requires the item
    // loose in a bag; the client enforces this, so a violation is a malformed/hacked client.
    if inv_slot < lyracore_shared::constants::starter_item::BACKPACK_SLOT_0 {
        return Err("equipped items cannot be traded".to_string());
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
    for row in slots.by_session().filter(&session.id).collect::<Vec<_>>() {
        if row.owner_guid == actor.guid && row.trade_slot == trade_slot {
            slots.id().delete(row.id);
        }
    }
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
    if actor.guid == session.initiator_guid {
        session.initiator_gold = copper;
    } else {
        session.target_gold = copper;
    }
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
