//! The loot window and GameObject use over an encrypted World Session.

use super::*;

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
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(68),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(details) => {
            assert_eq!(details.quest_id, 1234)
        }
        other => panic!("expected quest details from a questgiver GameObject, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store
        .loot_window
        .gameobjects_used
        .lock()
        .unwrap()
        .is_empty());
    assert!(store
        .loot_window
        .corpse_loot_reads
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn non_chest_gameobject_preserves_the_general_use_path() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(0);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(91),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store
            .loot_window
            .gameobjects_used
            .lock()
            .unwrap()
            .as_slice(),
        &[91]
    );
    assert_eq!(
        store
            .loot_window
            .corpse_loot_reads
            .lock()
            .unwrap()
            .as_slice(),
        &[(91, 1)]
    );
}

#[test]
fn chest_dispatch_opens_the_shared_window_and_tracks_its_target() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(lyracore_shared::constants::go_type::CHEST);
    s.loot_window
        .corpse_loot_by_viewer
        .insert(1, vec![(4, 117, 2, 321, 0)]);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, OP_LOOT_RESPONSE);
    assert_eq!(&body[0..8], &90u64.to_le_bytes());
    assert_eq!(loot_item_bytes(&body, 0), (4, 117, 2, 321));

    CMSG_AUTOSTORE_LOOT_ITEM { item_slot: 4 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed) => assert_eq!(removed.slot, 4),
        other => panic!("expected SMSG_LOOT_REMOVED, got {other}"),
    }

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store
            .loot_window
            .gameobjects_used
            .lock()
            .unwrap()
            .as_slice(),
        &[90]
    );
    assert_eq!(
        store
            .loot_window
            .corpse_loot_reads
            .lock()
            .unwrap()
            .as_slice(),
        &[(90, 1)]
    );
    assert_eq!(
        store.loot_window.items_looted.lock().unwrap().as_slice(),
        &[(90, 4)]
    );
}

#[test]
fn loot_before_player_login_is_handled_without_panicking() {
    let store = std::sync::Arc::new(tester_store(7));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("loot without a selected player must not panic")
        .expect("loot without a selected player remains a handled no-op");
    assert!(store
        .loot_window
        .corpse_loot_reads
        .lock()
        .unwrap()
        .is_empty());
    assert!(store.loot_window.skinned.lock().unwrap().is_empty());
}

#[test]
fn loot_with_a_zero_player_guid_is_handled_without_panicking() {
    let mut entity = warrior_entity();
    entity.guid = 0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(entity),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 0);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("loot with a zero player guid must not panic");
    assert!(store
        .loot_window
        .corpse_loot_reads
        .lock()
        .unwrap()
        .is_empty());
    assert!(store.loot_window.skinned.lock().unwrap().is_empty());
}

#[test]
fn skinning_refusal_keeps_the_world_session_alive() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            loot_window: LootWindowState {
                skinning_refusal: Some(LootWindowRefusal::Unanswered),
                ..base.loot_window
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    for target_guid in [60, 61] {
        CMSG_LOOT {
            guid: Guid::new(target_guid),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
        assert_eq!(opcode, OP_LOOT_RESPONSE);
        assert_eq!(&body[0..8], &target_guid.to_le_bytes());
    }

    drop(client);
    server.join().unwrap();
    assert!(store.loot_window.skinned.lock().unwrap().is_empty());
}

#[test]
fn skinning_infrastructure_failure_ends_the_world_session() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            loot_window: LootWindowState {
                skinning_failure: Some("gw_skin reducer timed out after 10s".to_string()),
                ..base.loot_window
            },
            ..base
        }
    });
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store, 1);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    assert!(
        server.join().is_err(),
        "a skinning timeout must end the World Session"
    );
}

#[test]
fn loot_opens_the_window_and_loot_money_drives_the_tracked_guid() {
    // CMSG_LOOT arms the open-loot state and replies the RAW loot window (guid + money in the body);
    // CMSG_LOOT_MONEY (which carries NO guid) must then hit the TRACKED corpse. A
    // SOLO money loot sends ONLY SMSG_LOOT_CLEAR_MONEY — the unconditional SMSG_LOOT_MONEY_NOTIFY
    // is gone (vanilla never sends it to a solo looter; the client prints its own local "You loot X
    // copper" line). A corpse with money is NOT skinned.
    let mut s = quest_store();
    s.loot_window.corpse_money = 25;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_LOOT_RESPONSE);
    assert_eq!(
        &body[0..8],
        &60u64.to_le_bytes(),
        "loot window names the corpse guid"
    );
    assert_eq!(
        &body[9..13],
        &25u32.to_le_bytes(),
        "loot window shows the corpse's copper"
    );

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY => {}
        other => panic!(
            "expected SMSG_LOOT_CLEAR_MONEY directly (no notify for a solo looter), got {other}"
        ),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_window.money_looted.lock().unwrap().as_slice(),
        &[60],
        "the TRACKED guid was looted"
    );
    assert!(
        store.loot_window.skinned.lock().unwrap().is_empty(),
        "a corpse with money is not skinned"
    );
}

#[test]
fn loot_money_with_zero_copper_still_clears_with_no_notify() {
    // amount == 0: the same no-notify contract as any solo loot — CLEAR_MONEY still
    // goes out so the client's loot window drops its money row.
    let store = std::sync::Arc::new(quest_store()); // corpse_money = 0
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = read_raw_frame(&mut client, &mut c_dec); // the loot window
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY => {} // and NOT a NOTIFY first
        other => {
            panic!("expected SMSG_LOOT_CLEAR_MONEY directly (no notify for 0 copper), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_window.money_looted.lock().unwrap().as_slice(),
        &[60]
    );
}

#[test]
fn loot_release_clears_the_tracked_target_so_take_requests_are_noops() {
    let mut s = quest_store();
    s.loot_window.corpse_money = 25; // non-empty so the skin fallback stays out of the picture
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);
    CMSG_LOOT_RELEASE {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(r) => assert_eq!(r.guid.guid(), 60),
        other => panic!("expected SMSG_LOOT_RELEASE_RESPONSE, got {other}"),
    }
    // The window is closed — stray targetless take requests must not reach the store.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_AUTOSTORE_LOOT_ITEM { item_slot: 3 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "release cleared the tracked target"
    );
    assert!(
        store.loot_window.items_looted.lock().unwrap().is_empty(),
        "release cleared the tracked target before an item take"
    );
}

fn loot_item_bytes(body: &[u8], index: usize) -> (u8, u32, u32, u32) {
    // Item N starts at byte 14 (8 guid + 1 method + 4 money + 1 count), 22 bytes each.
    let base = 14 + index * 22;
    let slot = body[base];
    let item_id = u32::from_le_bytes(body[base + 1..base + 5].try_into().unwrap());
    let count = u32::from_le_bytes(body[base + 5..base + 9].try_into().unwrap());
    let display_id = u32::from_le_bytes(body[base + 9..base + 13].try_into().unwrap());
    (slot, item_id, count, display_id)
}
