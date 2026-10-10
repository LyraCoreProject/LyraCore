//! Quest traffic over the world session. Which screen a quest opens, and which durable request it
//! makes, is decided and proved at the `dispatch_quest_action` seam (`handlers/quest.rs`). What is
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
fn login_sends_the_quest_log_descriptor_raw_update_after_the_create_packet() {
    let slots = vec![codec::update_mask::QuestLogSlot {
        slot: 3,
        quest_id: 777,
        counts: Vec::new(),
        state: 0,
        timer: 0,
    }];
    let mut s = quest_store();
    s.quest.quest_log_slots = slots.clone();
    let store = std::sync::Arc::new(s);

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // World entry — discarded, this test is about what comes right after.
    drain_world_entry(&mut client, &mut c_dec);
    // gtker's typed reader rejects this raw partial VALUES body (no OBJECT_FIELD_TYPE), so read it
    // RAW and compare it against the same builder the seam's `quest_log_update` calls.
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    let mask = codec::update_mask::full_quest_log_mask(&slots);
    assert_eq!((opcode, body), codec::build_values_update_raw(1, &mask));

    drop(client);
    server.join().unwrap();
}

#[test]
fn quest_hello_reaches_the_quest_module_and_its_raw_details_body_survives_the_cipher() {
    // The socket-level contract: dispatch routes HELLO to the quest module, and the raw-encoded
    // DETAILS screen it returns crosses the encrypted frame intact. Which screen a giver opens is
    // decided at the `dispatch_quest_action` seam and proved there.
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
