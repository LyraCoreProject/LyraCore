//! GameObject use: its routing by GameObject type over a World Session, and the general-use path.

use super::death_tests::handle_loot_fake::HandleLootFake;
use super::death_tests::{in_world_conn, run, SELF_GUID};
use super::*;
use wow_world_messages::vanilla::CMSG_AUTOSTORE_LOOT_ITEM;

#[test]
fn questgiver_gameobject_bypasses_the_chest_lifecycle() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(lyracore_shared::constants::go_type::QUESTGIVER);
    s.quest.quest_evals = vec![eval(1234, codec::ROLE_START, false, false)];
    s.quest.quest_details = vec![detail_view(1234, "A Threat Within")];
    s.loot_window
        .corpse_loot_by_viewer
        .insert(1, vec![(0, 2589, 1, 200, 0)]);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(68),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(details) => {
            assert_eq!(details.quest_id, 1234)
        }
        other => panic!("expected quest details from a questgiver GameObject, got {other}"),
    }
    // The loot window would have answered before the status query's reply.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("a questgiver GameObject opened a loot window: {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn chest_gameobject_use_opens_the_shared_loot_window_for_that_chest() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(lyracore_shared::constants::go_type::CHEST);
    s.loot_window
        .corpse_loot_by_viewer
        .insert(1, vec![(0, 2589, 1, 200, 0)]);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_AUTOSTORE_LOOT_ITEM { item_slot: 0 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_RESPONSE(response) => {
            assert_eq!(response.guid.guid(), 77);
            assert_eq!(response.items.len(), 1);
        }
        other => panic!("expected the chest's loot window, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed) => assert_eq!(removed.slot, 0),
        other => panic!("expected the take to clear the slot, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.loot_window.gameobjects_used.lock().unwrap()[..], [77]);
    assert_eq!(store.loot_window.items_taken.lock().unwrap()[..], [(77, 0)]);
}

#[test]
fn door_gameobject_use_takes_the_general_path_and_opens_no_loot_window() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(0);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(91),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_AUTOSTORE_LOOT_ITEM { item_slot: 0 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    // A loot window or a take reply would arrive before the status query's answer.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("a door GameObject opened a loot window: {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.loot_window.gameobjects_used.lock().unwrap()[..], [91]);
    assert!(store.loot_window.items_taken.lock().unwrap().is_empty());
}

#[test]
fn gameobject_use_outside_the_chest_path_opens_the_window_for_its_loot() {
    let store = HandleLootFake::default().with_loot(SELF_GUID, vec![(4, 117, 2, 321, 0)]);
    let mut conn = in_world_conn();

    let sent = run(
        &store,
        &mut conn,
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(CMSG_GAMEOBJ_USE {
            guid: Guid::new(91),
        }),
    );

    let [Outbound::Raw { opcode, body }] = sent.as_slice() else {
        panic!("expected one loot window");
    };
    assert_eq!(*opcode, OP_LOOT_RESPONSE);
    assert_eq!(&body[0..8], &91u64.to_le_bytes());
    let WorldState::InWorld(iw) = &conn.state else {
        panic!("the connection left the world");
    };
    assert_eq!(iw.open_loot.target_guid, Some(91));
}
