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
use crate::{game_faction_template, WorldEntity};

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
}

/// Address one trade status to `recipient_guid` (the `group::push_event` shape, minus the name
/// resolution — no trade status carries a name). The identity resolves through the
/// [`character_by_guid`] fence: a mid-transfer recipient reads absent and the row is addressed to
/// `Identity::ZERO` (nobody), which is the right delivery for a partner who just left the shard.
fn push_trade_event(ctx: &ReducerContext, recipient_guid: u64, kind: u8, other_guid: u64) {
    let bound = crate::helpers::character_by_guid(ctx, recipient_guid).map(|c| c.owner_identity);
    ctx.db.game_trade_event().insert(TradeEvent {
        id: 0,
        recipient_identity: crate::helpers::event_recipient_identity(bound),
        kind,
        other_guid,
        created_at: ctx.timestamp,
        recipient_guid,
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
    let verdict = initiate_verdict(
        player_interaction_gate(&actor, target.as_ref()),
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

    /// Every refusal [`initiate_verdict`] owes the initiator, its wire kind, and the pinned check
    /// order: a gate refusal (dead/absent/range) outranks WrongFaction, which outranks Busy — so
    /// a dead cross-faction player in a trade hears YouDead, not Busy.
    #[test]
    fn initiate_verdict_names_each_refusal_and_holds_the_check_order() {
        use PlayerInteractionDenied as D;
        let ok: Result<(), D> = Ok(());

        assert_eq!(initiate_verdict(ok, true, false, false), Ok(()));

        // Gate refusals map onto their wire kinds.
        assert_eq!(
            initiate_verdict(Err(D::ActorDead), true, false, false),
            Err(event_kind::YOU_DEAD)
        );
        assert_eq!(
            initiate_verdict(Err(D::NoTarget), true, false, false),
            Err(event_kind::NO_TARGET)
        );
        assert_eq!(
            initiate_verdict(Err(D::TargetNotPlayer), true, false, false),
            Err(event_kind::NO_TARGET)
        );
        assert_eq!(
            initiate_verdict(Err(D::TargetDead), true, false, false),
            Err(event_kind::TARGET_DEAD)
        );
        assert_eq!(
            initiate_verdict(Err(D::DifferentPartition), true, false, false),
            Err(event_kind::TARGET_TO_FAR)
        );
        assert_eq!(
            initiate_verdict(Err(D::OutOfRange), true, false, false),
            Err(event_kind::TARGET_TO_FAR)
        );

        // Faction and busy refusals — either seat busy answers Busy.
        assert_eq!(
            initiate_verdict(ok, false, false, false),
            Err(event_kind::WRONG_FACTION)
        );
        assert_eq!(initiate_verdict(ok, true, true, false), Err(event_kind::BUSY));
        assert_eq!(initiate_verdict(ok, true, false, true), Err(event_kind::BUSY));

        // Pinned precedence: gate > faction > busy.
        assert_eq!(
            initiate_verdict(Err(D::ActorDead), false, true, true),
            Err(event_kind::YOU_DEAD)
        );
        assert_eq!(
            initiate_verdict(ok, false, true, true),
            Err(event_kind::WRONG_FACTION)
        );
    }
}
