//! GameObject use over a World Session: a quest giver opens its quest menu, and every other type
//! is a Loot Window use.

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
fn door_gameobject_use_with_no_loot_opens_no_loot_window() {
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
fn door_gameobject_use_with_loot_opens_the_shared_loot_window() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(0);
    s.loot_window
        .corpse_loot_by_viewer
        .insert(1, vec![(0, 2589, 1, 200, 0)]);
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

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_RESPONSE(response) => {
            assert_eq!(response.guid.guid(), 91);
            assert_eq!(response.items.len(), 1);
        }
        other => panic!("expected the door's loot window, got {other}"),
    }
    // The take finds the window open on the door.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed) => assert_eq!(removed.slot, 0),
        other => panic!("expected the take to clear the slot, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.loot_window.items_taken.lock().unwrap()[..], [(91, 0)]);
}
