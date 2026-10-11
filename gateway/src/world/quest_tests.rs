//! Quest traffic over the world session. Which screen a quest opens, and which durable request it
//! makes, is decided and proved at the `Quest::handle` seam (`handlers/quest.rs`). What is
//! left here is only what the seam cannot see: that a quest opcode reaches the seam through the
//! full handshake + login + cipher, and that the bodies it answers with survive the encrypted
//! frame.

use super::*;

#[test]
fn quest_choose_reward_relays_inventory_before_completion_over_the_cipher() {
    // The socket-level contract: the subscribed reducer callback has already queued the complete
    // item insertion when turn_in_quest returns, so the session's completion presentation must sit
    // behind CREATE, its inventory pointer and gain feedback on the one writer queue.
    let mut s = quest_store();
    let detail = detail_view(1234, "A Threat Within");
    s.quest.quest_details = vec![detail.clone()];
    let reward_item = codec::ItemInstanceView {
        guid: 0x4000_0000_0000_0042,
        entry: 25,
        owner_guid: 1,
        slot: 23,
        stack_count: 2,
        durability: 20,
        max_durability: 20,
        container_slots: 0,
        random_property_id: 0,
        random_property_enchant_ids: [0; 3],
        item_text_id: 0,
        enchantment: 0,
    };
    s.session.turn_in_reward_item = Some(reward_item.clone());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_QUESTGIVER_CHOOSE_REWARD {
        guid: Guid::new(50),
        quest_id: 1234,
        reward: 2,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let framed = |message: ServerOpcodeMessage| {
        let mut bytes = Vec::new();
        message.write_unencrypted_server(&mut bytes).unwrap();
        (
            u16::from_le_bytes([bytes[2], bytes[3]]),
            bytes[4..].to_vec(),
        )
    };
    let expected = [
        framed(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_item_create_object(&reward_item),
        ))),
        framed(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_inv_slot_values(
                reward_item.owner_guid,
                reward_item.slot,
                reward_item.guid,
            )
            .unwrap(),
        ))),
        framed(ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(Box::new(
            codec::build_item_push_result(
                reward_item.owner_guid,
                255,
                reward_item.slot as u32,
                reward_item.entry,
                reward_item.stack_count,
                false,
                0,
            ),
        ))),
        framed(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(
            Box::new(codec::build_quest_complete(&detail)),
        )),
    ];
    let actual = std::array::from_fn(|_| read_raw_frame(&mut client, &mut c_dec));
    assert_eq!(
        actual, expected,
        "inventory visibility must precede completion"
    );
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.quest.turned_in.lock().unwrap().as_slice(),
        &[(1, 50, 1234, 2)]
    );
}

#[test]
fn login_creates_the_character_with_existing_quest_progress() {
    assert_quest_entry(false, false);
}

#[test]
fn same_shard_worldport_creates_the_character_with_existing_quest_progress() {
    assert_quest_entry(true, false);
}

#[test]
fn cross_shard_worldport_reads_existing_quests_from_the_destination() {
    assert_quest_entry(true, true);
}

#[test]
fn login_ends_before_create_when_the_quest_log_read_fails() {
    let mut store = quest_store();
    store.quest.quest_log_read_error = Some("quest log unavailable".into());
    let (mut client, server_end) = world_session_socket_pair();
    let server =
        std::thread::spawn(move || run_world_session(server_end, std::sync::Arc::new(store)));
    let (mut enc, mut dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut enc)
        .unwrap();
    assert!(ServerOpcodeMessage::read_encrypted(&mut client, &mut dec).is_err());
    drop(client);
    let error = server.join().unwrap().unwrap_err();
    assert!(format!("{error:#}").contains("quest log unavailable"));
}

#[test]
fn quest_progress_during_entry_reaches_the_character_after_create() {
    assert_quest_reconciliation(Some(4));
}

#[test]
fn a_quest_removed_during_entry_is_cleared_after_create() {
    assert_quest_reconciliation(None);
}

fn assert_quest_reconciliation(count: Option<u32>) {
    let slot = codec::update_mask::QuestLogSlot {
        slot: 0,
        quest_id: 777,
        counts: vec![3],
        state: 0,
        timer: 0,
    };
    let mut store = quest_store();
    store.quest.quest_log_slots = vec![slot.clone()];
    store.quest.quest_log_after_subscribe = Some(Ok(count
        .map(|count| codec::update_mask::QuestLogSlot {
            counts: vec![count],
            ..slot
        })
        .into_iter()
        .collect()));
    let (mut client, mut enc, mut dec, server) = enter_world(std::sync::Arc::new(store), 1);
    let (opcode, body) = read_raw_frame(&mut client, &mut dec);
    assert_eq!(opcode, 0x00A9);
    let updates = lyracore_shared::values_mask::parse_values_updates(&body);
    let fields = &updates[0].fields;
    assert!(fields.contains(&(198, if count.is_some() { 777 } else { 0 })));
    assert!(fields.contains(&(199, count.unwrap_or(0))));
    assert!(
        !fields.iter().any(|(index, _)| *index == 2),
        "reconciliation must omit OBJECT_FIELD_TYPE"
    );
    wow_world_messages::vanilla::CMSG_PING {
        sequence_id: 647,
        round_time_in_ms: 0,
    }
    .write_encrypted_client(&mut client, &mut enc)
    .unwrap();
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut dec).unwrap(),
        ServerOpcodeMessage::SMSG_PONG(_)
    ));
    drop(client);
    server.join().unwrap();
}

#[test]
fn a_failed_quest_reconciliation_closes_the_world_session() {
    let mut store = quest_store();
    store.quest.quest_log_after_subscribe = Some(Err("quest log unavailable".into()));
    let (mut client, _, _, server) = enter_world(std::sync::Arc::new(store), 1);
    let mut byte = [0];
    assert_eq!(
        client.read(&mut byte).unwrap(),
        0,
        "the writer must close the socket"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn disabled_quest_descriptors_skip_the_durable_read() {
    const CHILD_ENV: &str = "LYRACORE_TEST_QUEST_DESCRIPTORS_DISABLED";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "world::tests::quest_tests::disabled_quest_descriptors_skip_the_durable_read",
            ])
            .env(CHILD_ENV, "1")
            .env("LYRACORE_QUEST_LOG", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    let mut store = quest_store();
    store.quest.quest_log_read_error = Some("quest reads must be disabled".into());
    let (client, _, _, server) = enter_world(std::sync::Arc::new(store), 1);
    drop(client);
    server.join().unwrap();
}

fn assert_quest_entry(worldport: bool, cross_shard: bool) {
    let slots = vec![
        codec::update_mask::QuestLogSlot {
            slot: 3,
            quest_id: 777,
            counts: vec![3, 2, 1, 4],
            state: 1,
            timer: 123456,
        },
        codec::update_mask::QuestLogSlot {
            slot: 19,
            quest_id: 888,
            counts: vec![7],
            state: 2,
            timer: 0,
        },
    ];
    let mut s = quest_store();
    s.session.entity_in_world = false;
    let mut ported = warrior_entity();
    ported.map_id = 1;
    s.session.worldport_entity = Some(ported.clone());
    if cross_shard {
        let mut destination = quest_store();
        destination.session.worldport_entity = Some(ported);
        destination.quest.quest_log_slots = slots.clone();
        s.topology.home_after_flip = Some(std::sync::Arc::new(destination));
    } else {
        s.quest.quest_log_slots = slots.clone();
    }
    let store = std::sync::Arc::new(s);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let mut frames = drain_world_entry(&mut client, &mut c_dec);
    if worldport {
        // Drain all login traffic before the map acknowledgement.
        wow_world_messages::vanilla::CMSG_PING {
            sequence_id: 647,
            round_time_in_ms: 0,
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        while read_raw_frame(&mut client, &mut c_dec).0 != 0x01DD {}
        MSG_MOVE_WORLDPORT_ACK {}
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
        frames = drain_world_entry(&mut client, &mut c_dec);
    }
    assert_create_quest_progress(&frames);
    wow_world_messages::vanilla::CMSG_PING {
        sequence_id: 647,
        round_time_in_ms: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(
        matches!(
            ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
            ServerOpcodeMessage::SMSG_PONG(_)
        ),
        "world entry must not replay quest VALUES after CREATE"
    );
    drop(client);
    server.join().unwrap();
}

fn assert_create_quest_progress(frames: &[ServerOpcodeMessage]) {
    let mask = frames
        .iter()
        .find_map(|message| {
            let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) = message else {
                return None;
            };
            update.objects.iter().find_map(|object| match object {
                Object::CreateObject2 { guid3, mask2, .. } if guid3.guid() == 1 => {
                    Some(mask2.clone())
                }
                _ => None,
            })
        })
        .expect("world entry must create the Character");
    // Reframe the received CREATE descriptor as VALUES for the independent field decoder.
    let values = wow_world_messages::vanilla::SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(1),
            mask1: mask,
        }],
    };
    let mut bytes = Vec::new();
    values.write_unencrypted_server(&mut bytes).unwrap();
    let updates = lyracore_shared::values_mask::parse_values_updates(&bytes[4..]);
    let fields = &updates[0].fields;
    let field = |index| {
        fields
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, value)| *value)
            .unwrap_or(0)
    };
    assert_eq!(field(207), 777, "CREATE must contain the existing quest");
    assert_eq!(
        field(208),
        0x01101083,
        "CREATE must preserve counters and state"
    );
    assert_eq!(field(209), 123456, "CREATE must preserve the timer");
    assert_eq!(field(255), 888, "the last quest slot must survive");
    assert_eq!(field(256), 0x02000007);
    assert_eq!(field(198), 0, "empty slots stay empty");
}

#[test]
fn quest_hello_reaches_the_quest_module_and_its_raw_details_body_survives_the_cipher() {
    // The socket-level contract: dispatch routes HELLO to the quest module, and the raw-encoded
    // DETAILS screen it returns crosses the encrypted frame intact. Which screen a giver opens is
    // decided at the `Quest::handle` seam and proved there.
    const OP_QUEST_DETAILS: u16 = 0x0188;
    let mut s = quest_store();
    s.quest.quest_evals = vec![eval(1234, codec::ROLE_START, false, false)];
    s.quest.quest_details = vec![detail_view(1234, "A Threat Within")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_QUESTGIVER_HELLO {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_QUEST_DETAILS);
    assert_eq!(
        &body[..12],
        &codec::build_quest_details_raw(50, &detail_view(1234, "A Threat Within")).1[..12],
        "giver guid + quest id reached the wire unchanged"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn quest_query_answers_the_raw_definition_body_through_the_cipher() {
    // The socket-level contract for the client's cold-cache definition query: the hand-rolled 5875
    // body crosses the encrypted frame intact. Which quests answer at all is proved at the seam.
    const OP_QUEST_QUERY_RESPONSE: u16 = 0x005D;
    let mut s = quest_store();
    s.quest.quest_details = vec![detail_view(1234, "A Threat Within")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_QUEST_QUERY { quest_id: 1234 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_QUEST_QUERY_RESPONSE);
    assert_eq!(
        body,
        codec::build_quest_query_response_raw(&detail_view(1234, "A Threat Within")).1
    );
    drop(client);
    server.join().unwrap();
}
