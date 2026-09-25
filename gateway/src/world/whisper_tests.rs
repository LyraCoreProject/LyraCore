//! Whisper target facts over the multi-shard Fake, and the whisper arm end to end.
//!
//! What executes here is production `world::whisper::target_facts` against the in-memory
//! multi-database topology the group tests use (`party_tests::party_topology`: Ginger in the open
//! world on `world`, Vim inside the dungeon on `instances`, plus an offline Character and a
//! playerbot). The Module's whisper rules are pinned in `module/src/realm_chat.rs`; the dispatch
//! arm's Refusal answers are pinned in `handlers/chat.rs`.

use super::party_tests::{character, party_topology, BOT, DORMANT, GINGER, TRIN, VIM};
use super::*;

fn facts(guid: u64, name: &str) -> WhisperTargetFacts {
    WhisperTargetFacts {
        guid,
        race: 1,
        name: name.to_string(),
        ignores_speaker: false,
        away_kind: 0,
        away_message: String::new(),
    }
}

/// The live failure this path exists for: `/w Vim` from the open world, with Vim's row on the
/// instances database. The name resolves realm-wide, case-insensitively.
#[test]
fn a_whisper_target_on_another_shard_resolves_realm_wide() {
    let (_realm, world, instances, _calls) = party_topology();
    assert_eq!(
        world.character_guid_by_name("Vim").unwrap(),
        None,
        "fixture: Vim's row lives on the instances shard"
    );
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "vim").unwrap(),
        Some(facts(VIM, "Vim"))
    );
    assert_eq!(
        whisper::target_facts(instances.as_ref(), VIM, "ginger").unwrap(),
        Some(facts(GINGER, "Ginger")),
        "and from inside the dungeon, out"
    );
}

#[test]
fn an_unknown_name_reaches_nobody() {
    let (_realm, world, _instances, _calls) = party_topology();
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Nobodyatall").unwrap(),
        None
    );
}

/// No offline whispering in vanilla: the gate is `game_character.online`, the session flag.
#[test]
fn an_offline_character_reaches_nobody() {
    let (_realm, world, _instances, _calls) = party_topology();
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Dormant").unwrap(),
        None
    );
}

/// A playerbot has a live entity and never logs in, so its session flag stays false. Whisper
/// reads the session flag, so a bot stays unwhisperable, as it was before Realm Presence.
#[test]
fn a_playerbot_reaches_nobody_because_the_gate_reads_the_session_flag() {
    let (_realm, world, _instances, _calls) = party_topology();
    assert!(
        presence::live_anywhere(world.as_ref(), BOT),
        "fixture: the bot's live entity is right there"
    );
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Botty").unwrap(),
        None
    );
}

/// The ignore verdict is read from the Shard that holds the target's contact rows, which is not
/// the Shard the whisper was typed on.
#[test]
fn the_ignore_verdict_is_read_from_whichever_shard_holds_the_target() {
    let (_realm, world, instances, _calls) = party_topology();
    instances.contacts.lock().unwrap().push((VIM, GINGER, true));
    assert!(
        world.contact_lists(VIM).unwrap().1.is_empty(),
        "fixture: the sender's own shard knows nothing about Vim's ignore list"
    );
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Vim").unwrap(),
        Some(WhisperTargetFacts {
            ignores_speaker: true,
            ..facts(VIM, "Vim")
        })
    );
}

#[test]
fn an_ignore_of_somebody_else_is_not_the_speakers() {
    let (_realm, world, _instances, _calls) = party_topology();
    world.contacts.lock().unwrap().push((TRIN, VIM, true));
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Trin").unwrap(),
        Some(facts(TRIN, "Trin"))
    );
}

/// A peer Shard that cannot answer contributes no ignore. Reading its `Err` as "ignored" would
/// drop every whisper on the Realm and tell every sender they are ignored while it is down.
#[test]
fn an_unreachable_shard_does_not_make_a_whisper_look_ignored() {
    let (_realm, world, _instances, _calls) = party_topology();
    let broken = std::sync::Arc::new(InMemoryStore {
        shard: "unreachable".into(),
        contact_lists_error: Some("shard is unreachable".into()),
        ..Default::default()
    });
    world.peers.lock().unwrap().push(broken);
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Trin").unwrap(),
        Some(facts(TRIN, "Trin"))
    );
}

/// An AFK target on another Shard: the Away Status comes from its live entity's PLAYER_FLAGS
/// (0x02) and the Auto-Reply from that same Shard.
#[test]
fn an_afk_target_conveys_its_auto_reply_from_its_own_shard() {
    let (_realm, world, instances, _calls) = party_topology();
    instances.member_entities.lock().unwrap().push((
        VIM,
        codec::MemberEntity {
            player_flags: 0x02,
            ..Default::default()
        },
    ));
    instances
        .auto_replies
        .lock()
        .unwrap()
        .insert(VIM, "brb food".to_string());
    world
        .auto_replies
        .lock()
        .unwrap()
        .insert(VIM, "stale text on the wrong shard".to_string());
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Vim").unwrap(),
        Some(WhisperTargetFacts {
            away_kind: 0x14,
            away_message: "brb food".to_string(),
            ..facts(VIM, "Vim")
        })
    );
}

/// DND (0x04) with no stored text conveys an empty Auto-Reply; the Module answers with the
/// default "Do not Disturb".
#[test]
fn a_dnd_target_without_stored_text_conveys_an_empty_reply() {
    let (_realm, world, instances, _calls) = party_topology();
    instances.member_entities.lock().unwrap().push((
        VIM,
        codec::MemberEntity {
            player_flags: 0x04,
            ..Default::default()
        },
    ));
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Vim").unwrap(),
        Some(WhisperTargetFacts {
            away_kind: 0x15,
            ..facts(VIM, "Vim")
        })
    );
}

/// A stored row without the flag is not an Away Status: PLAYER_FLAGS is the state observers see.
#[test]
fn a_reply_row_without_the_away_flag_is_ignored() {
    let (_realm, world, instances, _calls) = party_topology();
    instances.member_entities.lock().unwrap().push((
        VIM,
        codec::MemberEntity {
            player_flags: 0x10,
            ..Default::default()
        },
    ));
    instances
        .auto_replies
        .lock()
        .unwrap()
        .insert(VIM, "left over".to_string());
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Vim").unwrap(),
        Some(facts(VIM, "Vim"))
    );
}

/// Names are unique per Shard, not per Realm. A logged-out homonym on the sender's own Shard must
/// not shadow the Vim who is online inside the dungeon.
#[test]
fn a_homonym_on_the_senders_own_shard_does_not_shadow_the_online_target() {
    const HOMONYM: u64 = 9;
    let realm = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore-realm".into(),
        is_realm: true,
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger"), character(HOMONYM, "Vim")],
        live_guids: vec![GINGER, HOMONYM],
        offline_guids: vec![HOMONYM],
        ..Default::default()
    });
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        realm: Some(realm.clone()),
        characters: vec![character(VIM, "Vim")],
        live_guids: vec![VIM],
        ..Default::default()
    });
    for shard in [&world, &instances] {
        *shard.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    }
    assert_eq!(
        presence::resolve_by_name(world.as_ref(), "Vim").unwrap(),
        Some(HOMONYM),
        "fixture: first-hit-wins resolves the name to the sender's own shard's homonym"
    );
    assert_eq!(
        whisper::target_facts(world.as_ref(), GINGER, "Vim").unwrap(),
        Some(facts(VIM, "Vim"))
    );
}

/// A candidate whose Realm Presence cannot be read is not reported missing: the read error is the
/// answer, and the arm logs it and answers `SMSG_CHAT_PLAYER_NOT_FOUND`.
#[test]
fn an_unreadable_candidate_is_an_error_not_a_missing_character() {
    let (_realm, world, _instances, _calls) = party_topology();
    let broken = std::sync::Arc::new(InMemoryStore {
        shard: "unreachable".into(),
        world_shard_set_error: Some("shard is unreachable".into()),
        ..Default::default()
    });
    world.peers.lock().unwrap().push(broken);
    assert_eq!(
        world.character_guid_by_name("Dormant").unwrap(),
        Some(DORMANT)
    );
    let error = whisper::target_facts(world.as_ref(), GINGER, "Dormant")
        .expect_err("an offline claim needs every Shard to vouch for it");
    assert!(error.to_string().contains("unreachable"), "got {error}");
}

/// Say, yell and emotes stay on the player's own Shard: they are spatial, which is the rule that
/// moved the whisper, which is not, onto Realm-core.
#[test]
fn say_yell_and_emotes_stay_on_the_players_own_shard_when_sharded() {
    let (realm, world, _instances, calls) = party_topology();

    world
        .send_chat(7, 0, 0, 0, "hello Elwynn".into())
        .expect("say");
    world.send_chat(7, 0, 1, 0, "HELP".into()).expect("yell");
    world.send_emote(7, 0, 4, 4, TRIN).expect("targeted emote");

    assert_eq!(
        world.chats.lock().unwrap().clone(),
        vec![
            (0, 0, "hello Elwynn".to_string()),
            (1, 0, "HELP".to_string())
        ],
        "say/yell must still be the shard's own broadcast rows"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "no spatial chat may reach another database. Calls were {log:?}"
    );
    assert!(realm.realm_whispers.lock().unwrap().is_empty());
}

/// A whisper line names its sender by guid and the client resolves the name over
/// `CMSG_NAME_QUERY`. Answered from the asking Shard alone, a cross-shard sender renders nameless.
#[test]
fn a_name_query_resolves_a_character_on_another_shard() {
    let (_realm, world, instances, _calls) = party_topology();
    assert!(
        world.character_by_guid(VIM).unwrap().is_none(),
        "fixture: the asking session's shard has no row for a character inside the instance"
    );
    assert_eq!(
        presence::character_anywhere(world.as_ref(), VIM)
            .unwrap()
            .map(|c| c.name),
        Some("Vim".to_string())
    );
    assert_eq!(
        presence::character_anywhere(instances.as_ref(), GINGER)
            .unwrap()
            .map(|c| c.name),
        Some("Ginger".to_string())
    );
    assert!(presence::character_anywhere(world.as_ref(), 4242)
        .unwrap()
        .is_none());
}

/// End to end over a real socket, through `run_world_session`'s own dispatch. The speaker guid
/// the Module acts on is an argument, so it must be the guid this socket entered the world with:
/// anything else is impersonation. An unresolvable name answers `SMSG_CHAT_PLAYER_NOT_FOUND` with
/// the typed name.
#[test]
fn a_real_session_whispers_across_shards_as_its_own_character() {
    let (realm, _world, instances, _calls) = party_topology();
    let session_shard = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        speaker_facts: Some(SpeakerFacts {
            race: 1,
            chat_tag: 1,
            name: "Ginger".to_string(),
        }),
        ..Default::default()
    });
    *session_shard.peers.lock().unwrap() = vec![session_shard.clone(), instances.clone()];
    *instances.peers.lock().unwrap() = vec![session_shard.clone(), instances.clone()];

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = session_shard.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(GINGER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..WORLD_ENTRY_PACKETS {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Whisper {
            target_player: "vim".into(),
        },
        language: Language::Common,
        message: "meet me inside".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Whisper {
            target_player: "Nobodyatall".into(),
        },
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The successful whisper answers nothing (its lines return on the Relay), so the next reply is
    // the second whisper's.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(m) => assert_eq!(m.name, "Nobodyatall"),
        other => panic!("expected SMSG_CHAT_PLAYER_NOT_FOUND, got {other}"),
    }
    drop(client);
    server.join().unwrap();

    assert_eq!(
        session_shard.realm_whispers.lock().unwrap().clone(),
        vec![(
            GINGER,
            WhisperRequest {
                language: 7,
                message: "meet me inside".to_string(),
                speaker: SpeakerFacts {
                    race: 1,
                    chat_tag: 1,
                    name: "Ginger".to_string(),
                },
                target: facts(VIM, "Vim"),
            }
        )]
    );
}
