//! The two cancellation routes: `CMSG_CANCEL_CAST` (Esc, moved, recast) and `CMSG_CANCEL_AURA`
//! (the player right-clicked a buff icon).
//!
//! Both are best-effort and silent. The client has already dropped the cast bar or the buff icon
//! before it sends either one, so the gateway answers nothing; the pending-cast and aura relays are
//! the only senders. Both race a durable state change — a cast that just completed, an aura that
//! just expired — so a refusal is normal traffic and must never end the session.

use super::*;

/// `CMSG_CANCEL_CAST`: drop the caller's pending cast so its scheduled completion cannot fire a
/// phantom `SMSG_SPELL_GO` that wedges the client in "Another action is in progress". The client's
/// spell id is unused: the caller has at most one pending cast, which names it.
pub(super) fn cancel_cast<St: CastStore + ?Sized>(
    store: &St,
    player: CastPlayer,
) -> Result<CastOutcome> {
    best_effort(
        player,
        "cancel_cast",
        store.cancel_cast(player.account_id, player.self_guid.unwrap_or(0)),
    )
}

/// `CMSG_CANCEL_AURA`: remove the caller's own aura named by the wire spell id. The aura relay then
/// re-syncs the buff bar.
pub(super) fn cancel_aura<St: CastStore + ?Sized>(
    store: &St,
    player: CastPlayer,
    spell_id: u32,
) -> Result<CastOutcome> {
    best_effort(
        player,
        "cancel_aura",
        store.cancel_aura(player.account_id, player.self_guid.unwrap_or(0), spell_id),
    )
}

/// The shared cancellation outcome: nothing on the wire, no session transition, and a refusal that
/// is logged rather than raised. Only a dead reducer transport ends the session, because no later
/// request could be served either.
fn best_effort(player: CastPlayer, what: &str, result: Result<()>) -> Result<CastOutcome> {
    if let Err(e) = result {
        if is_transport_failure(&e) {
            return Err(e);
        }
        log::debug!("world: {what} ignored (account {}): {e}", player.account_id);
    }
    Ok(CastOutcome::Handled {
        transition: CastTransition::default(),
        outbound: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    use wow_world_messages::vanilla::{CMSG_CANCEL_AURA, CMSG_CANCEL_CAST};

    /// A store that refuses both cancellations, as a race with completion or expiry would.
    fn refusing_store(error: &str) -> InMemoryCasts {
        InMemoryCasts {
            cancel_error: Some(error.into()),
            ..Default::default()
        }
    }

    fn cancel_cast_msg() -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CANCEL_CAST(CMSG_CANCEL_CAST { id: 133 })
    }

    fn cancel_aura_msg(id: u32) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CANCEL_AURA(CMSG_CANCEL_AURA { id })
    }

    #[test]
    fn cancelling_a_cast_asks_for_the_callers_pending_cast_and_answers_nothing() {
        let store = InMemoryCasts::default();

        let (transition, outbound) =
            handled(dispatch_cast(&store, player(), cancel_cast_msg()).unwrap());

        assert_eq!(
            store.cancel_cast_calls.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER)],
            "the caller names the cast; the client's spell id is unused"
        );
        assert!(
            outbound.is_empty(),
            "the client already dropped its cast bar"
        );
        assert_eq!(transition, CastTransition::default());
    }

    #[test]
    fn cancelling_an_aura_passes_the_wire_spell_id() {
        let store = InMemoryCasts::default();

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cancel_aura_msg(5555)).unwrap());

        assert_eq!(
            store.cancel_aura_calls.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 5555)]
        );
        assert!(outbound.is_empty(), "the aura relay re-syncs the buff bar");
    }

    #[test]
    fn a_refused_cancellation_stays_silent_and_keeps_the_session_alive() {
        for (what, msg) in [("cast", cancel_cast_msg()), ("aura", cancel_aura_msg(5555))] {
            let store = refusing_store("nothing to cancel");

            let (_, outbound) = handled(
                dispatch_cast(&store, player(), msg)
                    .unwrap_or_else(|_| panic!("a losing {what} race must not end the session")),
            );

            assert!(outbound.is_empty(), "{what}: a refusal reaches no client");
        }
    }

    #[test]
    fn a_dead_reducer_transport_during_cancellation_is_session_fatal() {
        let store = refusing_store("gw_cancel_cast reducer transport disconnected: channel closed");

        assert!(dispatch_cast(&store, player(), cancel_cast_msg()).is_err());
    }

    #[test]
    fn a_player_with_no_character_in_world_cancels_against_a_zero_actor() {
        let store = InMemoryCasts::default();
        let player = CastPlayer {
            self_guid: None,
            ..player()
        };

        handled(dispatch_cast(&store, player, cancel_aura_msg(5555)).unwrap());

        assert_eq!(
            store.cancel_aura_calls.lock().unwrap().as_slice(),
            &[(ACCOUNT, 0, 5555)],
            "the durable call rejects guid 0 rather than the gateway panicking"
        );
    }
}
