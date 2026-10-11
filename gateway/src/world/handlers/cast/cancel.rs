//! Cast, channel and aura cancellation. Durable changes drive the protocol feedback.

use super::*;

/// `CMSG_CANCEL_CAST`: drop the caller's pending cast so its scheduled completion cannot fire a
/// phantom `SMSG_SPELL_GO` that wedges the client in "Another action is in progress". The client's
/// spell id is unused: the caller has at most one pending cast, which names it.
pub(super) fn cancel_cast<St: CastStore + ?Sized>(
    store: &St,
    session: &mut ProtocolSession,
) -> Result<ProtocolReply> {
    best_effort(session, "cancel_cast", |actor| store.cancel_cast(actor))
}

/// `CMSG_CANCEL_AURA`: remove the caller's own aura named by the wire spell id. The aura relay then
/// re-syncs the buff bar.
pub(super) fn cancel_aura<St: CastStore + ?Sized>(
    store: &St,
    session: &mut ProtocolSession,
    spell_id: u32,
) -> Result<ProtocolReply> {
    best_effort(session, "cancel_aura", |actor| {
        store.cancel_aura(actor, spell_id)
    })
}

/// The shared cancellation outcome: nothing on the wire, no session transition, and a Refusal that
/// is logged rather than raised. Only a Transport Loss ends the session, because no later request
/// could be served either. A session with no Actor has nothing to cancel.
fn best_effort(
    session: &mut ProtocolSession,
    what: &str,
    request: impl FnOnce(Actor) -> Result<()>,
) -> Result<ProtocolReply> {
    if let Some(Err(e)) = session.actor().map(request) {
        let reason = refusal_reason(e)?;
        log::debug!(
            "world: {what} ignored (account {}): {reason}",
            session.account_id
        );
    }
    Ok(ProtocolReply::default())
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

        let (repeating, outbound) =
            handled(run_cast(&store, session(), cancel_cast_msg()).unwrap());

        assert_eq!(
            store.cancel_cast_calls.lock().unwrap().as_slice(),
            &[CASTER],
            "the caller names the cast; the client's spell id is unused"
        );
        assert!(
            outbound.is_empty(),
            "the client already dropped its cast bar"
        );
        assert!(!repeating);
    }

    #[test]
    fn cancelling_an_aura_passes_the_wire_spell_id() {
        let store = InMemoryCasts::default();

        let (_, outbound) = handled(run_cast(&store, session(), cancel_aura_msg(5555)).unwrap());

        assert_eq!(
            store.cancel_aura_calls.lock().unwrap().as_slice(),
            &[(CASTER, 5555)]
        );
        assert!(outbound.is_empty(), "the aura relay re-syncs the buff bar");
    }

    #[test]
    fn a_refused_cancellation_stays_silent_and_keeps_the_session_alive() {
        for (what, msg) in [("cast", cancel_cast_msg()), ("aura", cancel_aura_msg(5555))] {
            let store = refusing_store("nothing to cancel");

            let (_, outbound) = handled(
                run_cast(&store, session(), msg)
                    .unwrap_or_else(|_| panic!("a losing {what} race must not end the session")),
            );

            assert!(outbound.is_empty(), "{what}: a refusal reaches no client");
        }
    }

    #[test]
    fn a_dead_reducer_transport_during_cancellation_is_session_fatal() {
        let store = InMemoryCasts {
            transport_lost: true,
            ..Default::default()
        };

        assert!(run_cast(&store, session(), cancel_cast_msg()).is_err());
    }

    #[test]
    fn a_player_with_no_character_in_world_has_nothing_to_cancel() {
        let store = InMemoryCasts::default();
        let player = ProtocolSession::new(ACCOUNT, "TESTER".into());

        handled(run_cast(&store, player, cancel_aura_msg(5555)).unwrap());

        assert!(store.cancel_aura_calls.lock().unwrap().is_empty());
    }
}
