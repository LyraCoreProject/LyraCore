//! Chat lines over an encrypted World Session: Speech, the GM dot-command, away status, Realm Chat
//! Lines and the Chat Flood Limiter.

use super::*;

#[test]
fn messagechat_say_and_yell_route_to_chat_types_0_and_1() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hi".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Yell,
        language: Language::Universal,
        message: "HEY".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // no reply on success — the speaker sees their line via the broadcast relay
    server.join().unwrap();
    assert_eq!(
        store.speech.chats.lock().unwrap().as_slice(),
        &[(0, 0, "hi".to_string()), (1, 0, "HEY".to_string())],
        "Say → type 0, Yell → type 1, language threaded"
    );
}

#[test]
fn messagechat_emote_routes_to_chat_type_3() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Emote,
        language: Language::Common,
        message: "waves wildly.".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // no reply on success — the speaker sees their line via the broadcast relay
    server.join().unwrap();
    assert_eq!(
        store.speech.chats.lock().unwrap().as_slice(),
        &[(3, 7, "waves wildly.".to_string())],
        "/e → type 3, language threaded to the Module (which stores it as Universal)"
    );
}

#[test]
fn messagechat_dot_say_diverts_to_gm_command_never_touching_chat() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".heal".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful dot-command
        other => panic!("expected the sentinel (no reply on gm_command success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.speech.gm_commands.lock().unwrap().as_slice(),
        &[("TESTER".to_string(), ".heal".to_string())],
        "the proof-validated Account name and raw dot-command must reach the Store together"
    );
    assert!(
        store.speech.chats.lock().unwrap().is_empty(),
        "a dot-command must NEVER reach send_chat"
    );
}

#[test]
fn messagechat_non_dot_say_is_byte_identical_to_before_223() {
    // The 223 divert must be a no-op for ordinary chat: a Say line NOT starting with '.' still routes
    // to send_chat exactly as before, and never touches gm_command.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hi".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.speech.chats.lock().unwrap().as_slice(),
        &[(0u8, 0u8, "hi".to_string())]
    );
    assert!(
        store.speech.gm_commands.lock().unwrap().is_empty(),
        "a plain Say line must never reach gm_command"
    );
}

#[test]
fn messagechat_dot_say_error_relays_a_system_chat_line_to_the_sender_only() {
    // A rejected dot-command (bad gm_level, unknown command, bad args) is relayed back
    // to the SENDER as a System SMSG_MESSAGECHAT carrying the module's raw message VERBATIM — no
    // "reducer failed" wrapper prefix, no broadcast, no game_chat_event row.
    let mut s = quest_store();
    s.speech.gm_command_error = Some("permission denied".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".god".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(m) => {
            assert_eq!(m.message, "permission denied");
            assert!(
                matches!(
                    m.chat_type,
                    wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
                ),
                "expected a System chat line, got {:?}",
                m.chat_type
            );
        }
        other => panic!("expected SMSG_MESSAGECHAT System, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store.speech.chats.lock().unwrap().is_empty());
}

/// `quest_store` plus the session's own Character row, which the `sync` sentinel needs.
fn chat_session_store() -> WorldFake {
    let base = quest_store();
    WorldFake {
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Warrior".into(),
            ..Default::default()
        }],
        ..base
    }
}

/// `/afk Brb` becomes one `set_away` request for the session's own Character, with no reply: the
/// client prints its own notice and observers see PLAYER_FLAGS on the entity Relay.
#[test]
fn messagechat_afk_and_dnd_become_set_away_requests_from_the_sessions_character() {
    let store = std::sync::Arc::new(chat_session_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    sync(&mut client, &mut c_enc, &mut c_dec, |client, enc| {
        for (chat_type, message) in [
            (CMSG_MESSAGECHAT_ChatType::Afk, "Brb"),
            (CMSG_MESSAGECHAT_ChatType::Dnd, ""),
        ] {
            CMSG_MESSAGECHAT {
                chat_type,
                language: Language::Universal,
                message: message.into(),
            }
            .write_encrypted_client(&mut *client, &mut *enc)
            .unwrap();
        }
    });
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.chat.away_requests.lock().unwrap().clone(),
        vec![(1, 0x14, "Brb".to_string()), (1, 0x15, String::new())]
    );
}

/// cm:ChatHandler.cpp:801-815: `CMSG_CHAT_IGNORED` sends IGNORED to the Character whose line the
/// client dropped, carrying the ignorer's own name.
#[test]
fn chat_ignored_becomes_an_ignored_line_to_the_dropped_speaker() {
    let mut s = chat_session_store();
    s.chat.speaker_facts = Some(SpeakerFacts {
        race: 1,
        chat_tag: 1,
        name: "Warrior".to_string(),
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    sync(&mut client, &mut c_enc, &mut c_dec, |client, enc| {
        wow_world_messages::vanilla::CMSG_CHAT_IGNORED {
            guid: Guid::new(42),
        }
        .write_encrypted_client(client, enc)
        .unwrap();
    });
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(
        requests.as_slice(),
        &[(
            1,
            RealmChatRequest {
                kind: 0x16,
                language: 0,
                channel_name: String::new(),
                target_guid: 42,
                message: "Warrior".to_string(),
                speaker: SpeakerFacts {
                    race: 1,
                    chat_tag: 1,
                    name: "Warrior".to_string(),
                },
            }
        )]
    );
}

#[test]
fn messagechat_guild_becomes_one_realm_chat_request_from_the_sessions_character() {
    // `/g` reaches the Realm Chat path with the guid it entered the world with, the same shape as
    // `/p`. No reply on success: the speaker hears the line through the Relay like every member.
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Guild,
        language: Language::Common,
        message: "hello guild".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /g
        other => panic!("expected the sentinel (no reply on /g success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (speaker_guid, request) = &requests[0];
    assert_eq!(*speaker_guid, 1);
    assert_eq!(request.kind, lyracore_shared::chat::chat_kind::GUILD);
    assert_eq!(request.message, "hello guild");
    assert!(
        store.speech.chats.lock().unwrap().is_empty(),
        "a guild line never becomes a say line"
    );
}

#[test]
fn messagechat_officer_becomes_one_realm_chat_request_from_the_sessions_character() {
    // `/o` follows the same path as `/g` with its own Chat Kind.
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Officer,
        language: Language::Common,
        message: "officers only".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /o
        other => panic!("expected the sentinel (no reply on /o success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (speaker_guid, request) = &requests[0];
    assert_eq!(*speaker_guid, 1);
    assert_eq!(request.kind, lyracore_shared::chat::chat_kind::OFFICER);
    assert_eq!(request.message, "officers only");
}

#[test]
fn messagechat_party_becomes_one_realm_chat_request_from_the_sessions_character() {
    // The session's `/p` reaches the Realm Chat path with the guid it entered the world with. No
    // reply on success: the speaker hears the line through the Relay like every other member.
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Common,
        message: "form up".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /p
        other => panic!("expected the sentinel (no reply on /p success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (speaker_guid, request) = &requests[0];
    assert_eq!(*speaker_guid, 1);
    assert_eq!(request.kind, lyracore_shared::chat::chat_kind::PARTY);
    assert_eq!(request.language, 7);
    assert_eq!(request.message, "form up");
    assert!(
        store.speech.chats.lock().unwrap().is_empty(),
        "a party line never becomes a say line"
    );
}

#[test]
fn messagechat_party_from_an_ungrouped_caller_replies_not_in_group() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    s.chat.realm_chat_outcome = Some(ChatOutcome::Refused(
        lyracore_shared::chat::ChatRefusal::NotInGroup,
    ));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(
                r.result,
                wow_world_messages::vanilla::PartyResult::NotInGroup
            );
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT(NotInGroup), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// Character 1 in the world, with a Character row so `sync`'s sentinel is answered.
fn chat_store() -> WorldFake {
    let base = quest_store();
    WorldFake {
        chat: ChatState {
            speaker_facts: Some(human_speaker()),
            ..base.chat
        },
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            ..Default::default()
        }],
        ..base
    }
}

fn say_line(message: &str) -> CMSG_MESSAGECHAT {
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Common,
        message: message.into(),
    }
}

fn notification(message: ServerOpcodeMessage) -> String {
    match message {
        ServerOpcodeMessage::SMSG_NOTIFICATION(notice) => notice.notification,
        other => panic!("expected SMSG_NOTIFICATION, got {other}"),
    }
}

#[test]
fn a_flooding_session_is_muted_before_any_chat_durable_request() {
    // cm:Player.cpp:16344-16377: eleven fast lines mute the speaker for ten seconds. The twelfth
    // line and the party line after it answer the cmangos notice and reach no reducer.
    let store = std::sync::Arc::new(chat_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    for n in 0..12 {
        say_line(&format!("line {n}"))
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
    }
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Common,
        message: "form up".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            notification(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()),
            "You must wait 10 Second(s). before speaking again."
        );
    }
    sync(&mut client, &mut c_enc, &mut c_dec, |_, _| {});
    drop(client);
    server.join().unwrap();
    assert_eq!(store.speech.chats.lock().unwrap().len(), 11);
    assert!(store.chat.realm_chats.lock().unwrap().is_empty());
}

#[test]
fn a_muted_session_still_sends_addon_lines() {
    // cm:ChatHandler.cpp:113-119: the addon language is never flood-controlled.
    let store = std::sync::Arc::new(chat_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    for n in 0..12 {
        say_line(&format!("line {n}"))
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
    }
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Guild,
        language: Language::Addon,
        message: "LCTEST\tping".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    notification(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap());
    sync(&mut client, &mut c_enc, &mut c_dec, |_, _| {});
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1.language, 0xFFFF_FFFF);
}

#[test]
fn a_game_master_is_never_muted_for_flooding() {
    // cm:Player.cpp:16346-16348 skips the flood count for any account above SEC_PLAYER.
    let mut s = chat_store();
    s.chat.gm_level = 1;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        for n in 0..15 {
            say_line(&format!("line {n}"))
                .write_encrypted_client(&mut *c, &mut *e)
                .unwrap();
        }
    });
    drop(client);
    server.join().unwrap();
    assert_eq!(store.speech.chats.lock().unwrap().len(), 15);
}

#[test]
fn a_say_line_in_a_language_the_speaker_does_not_know_answers_the_vanilla_notice() {
    // cm:ChatHandler.cpp:107-110 with cm mangos.sql:4044.
    let mut s = chat_store();
    s.speech.send_chat_outcome = Some(ChatOutcome::Refused(
        lyracore_shared::chat::ChatRefusal::UnknownLanguage,
    ));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Orcish,
        message: "zug zug".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(
        notification(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()),
        "You don't know that language"
    );
    sync(&mut client, &mut c_enc, &mut c_dec, |_, _| {});
    drop(client);
    server.join().unwrap();
}
