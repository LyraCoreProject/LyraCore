//! Melee attack opcodes through their dispatcher.

use super::family::{ProtocolFamily, ProtocolSession};
use super::handlers::InMemoryMeleeActions;
use super::*;

fn try_run(
    actions: &InMemoryMeleeActions,
    session: &mut ProtocolSession,
    msg: impl Into<ClientOpcodeMessage>,
) -> Result<Vec<Outbound>> {
    Ok(handlers::Melee::handle(actions, session, msg.into().into())?.outbound)
}

fn run(
    actions: &InMemoryMeleeActions,
    session: &mut ProtocolSession,
    msg: impl Into<ClientOpcodeMessage>,
) -> Vec<Outbound> {
    try_run(actions, session, msg).unwrap()
}

fn swing(target: u64) -> CMSG_ATTACKSWING {
    CMSG_ATTACKSWING {
        guid: Guid::new(target),
    }
}

#[test]
fn attackswing_ok_replies_attackstart_and_stop_echoes_then_clears() {
    let actions = InMemoryMeleeActions::default();
    let mut session = ProtocolSession::in_world(7, 1);

    match run(&actions, &mut session, swing(90)).as_slice() {
        [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(a))] => {
            assert_eq!(a.attacker.guid(), 1, "self guid");
            assert_eq!(a.victim.guid(), 90);
        }
        _ => panic!("expected one SMSG_ATTACKSTART"),
    }
    // Stop echoes the armed target and clears it.
    match run(&actions, &mut session, CMSG_ATTACKSTOP {}).as_slice() {
        [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(a))] => {
            assert_eq!(a.player.guid(), 1);
            assert_eq!(a.enemy.guid(), 90);
        }
        _ => panic!("expected one SMSG_ATTACKSTOP"),
    }
    // A second stop finds no armed target, so there is no second echo.
    assert!(run(&actions, &mut session, CMSG_ATTACKSTOP {}).is_empty());
}

#[test]
fn melee_opcodes_at_character_select_answer_nothing_and_do_not_fail() {
    // No WorldEntity yet, so the seam has no attacker guid to name and no combat state to change.
    // It makes no durable request.
    let actions = InMemoryMeleeActions::default();
    let mut session = ProtocolSession::new(7, "TESTER".into());

    assert!(run(&actions, &mut session, swing(90)).is_empty());
    assert!(run(&actions, &mut session, CMSG_ATTACKSTOP {}).is_empty());
    assert!(actions.start_requests.lock().unwrap().is_empty());
    assert!(actions.stop_requests.lock().unwrap().is_empty());
}

#[test]
fn attackswing_desync_error_is_session_fatal() {
    // A desync-classified start_attack failure (the player's OWN entity is gone) must PROPAGATE
    // as session-fatal so the socket tears down for a clean relog. Which failures are fatal, and
    // which answer a refusal instead, belongs to the melee seam's own tests.
    let actions = InMemoryMeleeActions {
        start_error: Some("no live entity for guid 1".into()),
        ..Default::default()
    };
    let mut session = ProtocolSession::in_world(7, 1);

    let err = try_run(&actions, &mut session, swing(90))
        .err()
        .expect("a desync on attackswing must end the session with an error");
    assert!(
        format!("{err:#}").contains("desync"),
        "the error should carry the desync context, got: {err:#}"
    );
}
