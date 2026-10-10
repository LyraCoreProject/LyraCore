//! Bank opcodes over an encrypted World Session.

use super::*;

#[test]
fn banker_activate_sends_smsg_show_bank_with_the_banker_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BANKER_ACTIVATE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOW_BANK(p) => assert_eq!(p.guid.guid(), 77),
        other => panic!("expected SMSG_SHOW_BANK, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn banker_activate_on_a_standing_refusing_banker_sends_no_reply() {
    // CMSG_PLAYED_TIME (the sentinel below) only replies once `character_by_guid` resolves the
    // caller's own guid, so give the store a character row for guid 1 (quest_store() has none) —
    // same setup as `inspect_refused_target_sends_no_reply`.
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            npc_refuses: true,
            characters: vec![codec::CharacterView {
                guid: 1,
                ..Default::default()
            }],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BANKER_ACTIVATE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: a follow-up request with a guaranteed reply. If the refused activate had wrongly
    // produced an SMSG_SHOW_BANK, it would arrive first and this match would fail.
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {} // no SMSG_SHOW_BANK for the refused banker
        other => {
            panic!("expected SMSG_PLAYED_TIME (no SMSG_SHOW_BANK for refused banker), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autobank_item_from_the_main_bag_dispatches_auto_bank_item() {
    // Right-click a bag item with the bank open (CMSG_AUTOBANK_ITEM) → the gateway names the source
    // slot and lets the module resolve the free bank slot; deposit and withdraw share one store method.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 255,
        slot_index: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bank.auto_banked_items.lock().unwrap().as_slice(),
        &[23]
    );
}

#[test]
fn autostore_bank_item_from_the_main_bag_dispatches_auto_bank_item() {
    // Right-click a banked item (CMSG_AUTOSTORE_BANK_ITEM) → withdraw, same store method as deposit.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BANK_ITEM {
        bag_index: 255,
        slot_index: 39,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bank.auto_banked_items.lock().unwrap().as_slice(),
        &[39]
    );
}

#[test]
fn autobank_item_err_sends_smsg_inventory_change_failure() {
    // A full destination (bank full, or carry space full) is a per-action error relayed as the
    // existing inventory-change-failure reply, never session-fatal.
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            trade_error: Some("bank full".into()),
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 255,
        slot_index: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autobank_item_from_a_sub_bag_is_unsupported_and_does_not_dispatch() {
    // Only the main pseudo-bag (255) is addressed, matching the item handler's restriction — a
    // sub-bag index is logged and ignored, never fatal.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 19,
        slot_index: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.bank.auto_banked_items.lock().unwrap().is_empty(),
        "a sub-bag source must not be routed through auto_bank_item"
    );
}

#[test]
fn autostore_bank_item_from_a_sub_bag_is_unsupported_and_does_not_dispatch() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BANK_ITEM {
        bag_index: 19,
        slot_index: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.bank.auto_banked_items.lock().unwrap().is_empty(),
        "a sub-bag source must not be routed through auto_bank_item"
    );
}

#[test]
fn buy_bank_slot_success_sends_ok_and_reaches_the_named_banker() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_BUY_BANK_SLOT {
        guid: Guid::new(88),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p) => {
            assert_eq!(p.result, BuyBankSlotResult::Ok);
        }
        other => panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bank.bought_bank_slots.lock().unwrap().as_slice(),
        &[88]
    );
}

#[test]
fn buy_bank_slot_failure_maps_the_bracketed_code_to_the_matching_result() {
    // The module tags a refusal with its `SMSG_BUY_BANK_SLOT_RESULT` code in brackets — parsed by
    // code, not by matching the prose.
    for (err, want) in [
        (
            "[0] no bank bag slots left to buy",
            BuyBankSlotResult::FailedTooMany,
        ),
        (
            "[1] not enough money (need 1000)",
            BuyBankSlotResult::InsufficientFunds,
        ),
        ("[2] target is not a banker", BuyBankSlotResult::NotBanker),
    ] {
        let mut s = quest_store();
        s.trade_error = Some(err.into());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_BUY_BANK_SLOT {
            guid: Guid::new(88),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p) => {
                assert_eq!(p.result, want, "store error {err:?} must map to {want:?}");
            }
            other => panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}
