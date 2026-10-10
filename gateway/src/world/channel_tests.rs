//! Chat Channel opcodes over an encrypted World Session.

use super::*;

#[test]
fn join_channel_runs_the_channel_op_as_the_sessions_character() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    wow_world_messages::vanilla::CMSG_JOIN_CHANNEL {
        channel_name: "Trade - City".into(),
        channel_password: String::new(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // YOU_JOINED returns on the Relay
        other => panic!("expected the sentinel (no reply on a successful join), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let ops = store.channel.channel_ops.lock().unwrap();
    assert_eq!(ops.len(), 1);
    let (actor_guid, op, request) = &ops[0];
    assert_eq!((*actor_guid, *op), (1, 0));
    assert_eq!(request.channel_name, "Trade - City");
}

/// A refused join answers WRONG_PASSWORD 0x04 on the session's own socket.
#[test]
fn a_refused_join_answers_the_notice_on_the_session() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    s.channel.channel_outcome = Some(ChannelOutcome::Refused(
        lyracore_shared::channel::ChannelRefusal::WrongPassword,
    ));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    wow_world_messages::vanilla::CMSG_JOIN_CHANNEL {
        channel_name: "Rx".into(),
        channel_password: "guess".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(notify) => {
            assert_eq!(
                notify.notify_type,
                wow_world_messages::vanilla::ChatNotify::WrongPasswordNotice
            );
            assert_eq!(notify.channel_name, "Rx");
        }
        other => panic!("expected SMSG_CHANNEL_NOTIFY, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}
