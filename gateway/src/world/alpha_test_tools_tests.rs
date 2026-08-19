//! Account-owned Alpha Test Tools through a Headless Client.
//!
//! The Fake models the production Store operation's two outcomes: Realm-core answers current
//! Account authority, then the Home Shard accepts or refuses the Durable Request. Module tests
//! own command parsing and teleport coordinates.

use super::*;

fn alpha_test_tools_store(
    enabled: bool,
) -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let authority = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(enabled));
    let store = std::sync::Arc::new(InMemoryStore {
        gm_alpha_test_tools: Some(authority.clone()),
        ..quest_store()
    });
    (store, authority)
}

fn send_dot_say(client: &mut UnixStream, encrypt: &mut EncrypterHalf, text: &str) {
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: text.into(),
    }
    .write_encrypted_client(client, encrypt)
    .unwrap();
}

fn assert_quest_status_reply(
    client: &mut UnixStream,
    encrypt: &mut EncrypterHalf,
    decrypt: &mut DecrypterHalf,
) {
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut *client, encrypt)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(client, decrypt).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected quest-status sentinel, got {other}"),
    }
}

#[test]
fn alpha_test_tools_dispatch_speed_and_tele_without_say_chat() {
    let (store, _authority) = alpha_test_tools_store(true);
    let (mut client, mut encrypt, mut decrypt, server) = enter_world(store.clone(), 1);

    send_dot_say(&mut client, &mut encrypt, ".speed 3");
    send_dot_say(&mut client, &mut encrypt, ".tele stormwind");
    // Neither command sends chat on success. The next response must be this unrelated reply.
    assert_quest_status_reply(&mut client, &mut encrypt, &mut decrypt);

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.gm_commands.lock().unwrap().as_slice(),
        &[
            ("TESTER".to_string(), ".speed 3".to_string()),
            ("TESTER".to_string(), ".tele stormwind".to_string()),
        ]
    );
    assert_eq!(
        store.gm_authority_results.lock().unwrap().as_slice(),
        &[true, true],
        "each command carries the Realm-core authority result"
    );
    assert_eq!(
        store.gm_gameplay_changes.lock().unwrap().as_slice(),
        &[".speed 3".to_string(), ".tele stormwind".to_string()]
    );
    assert!(
        store.chats.lock().unwrap().is_empty(),
        "dot-Say commands never create ordinary Say chat"
    );
}

#[test]
fn alpha_only_destructive_command_is_private_and_changes_nothing() {
    let (store, _authority) = alpha_test_tools_store(true);
    let (mut client, mut encrypt, mut decrypt, server) = enter_world(store.clone(), 1);

    send_dot_say(&mut client, &mut encrypt, ".god");
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut decrypt).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(message) => {
            assert_eq!(message.message, "permission denied");
            assert!(matches!(
                message.chat_type,
                wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
            ));
        }
        other => panic!("expected private system refusal, got {other}"),
    }
    assert_quest_status_reply(&mut client, &mut encrypt, &mut decrypt);

    drop(client);
    server.join().unwrap();
    assert!(
        store.gm_gameplay_changes.lock().unwrap().is_empty(),
        "a refused command makes no Home Shard gameplay change"
    );
    assert!(
        store.chats.lock().unwrap().is_empty(),
        "a refusal never creates ordinary Say chat"
    );
}

#[test]
fn revocation_refuses_the_next_command_without_ending_the_world_session() {
    let (store, authority) = alpha_test_tools_store(true);
    let (mut client, mut encrypt, mut decrypt, server) = enter_world(store.clone(), 1);

    send_dot_say(&mut client, &mut encrypt, ".speed 3");
    assert_quest_status_reply(&mut client, &mut encrypt, &mut decrypt);

    authority.store(false, std::sync::atomic::Ordering::SeqCst);
    send_dot_say(&mut client, &mut encrypt, ".speed 4");
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut decrypt).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(message) => {
            assert_eq!(message.message, "permission denied");
            assert!(matches!(
                message.chat_type,
                wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
            ));
        }
        other => panic!("expected private system refusal after revocation, got {other}"),
    }
    assert_quest_status_reply(&mut client, &mut encrypt, &mut decrypt);

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.gm_authority_results.lock().unwrap().as_slice(),
        &[true, false]
    );
    assert_eq!(
        store.gm_gameplay_changes.lock().unwrap().as_slice(),
        &[".speed 3".to_string()],
        "revocation stops the next Home Shard gameplay change"
    );
    assert!(store.chats.lock().unwrap().is_empty());
}
