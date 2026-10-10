//! Vendor opcodes over an encrypted World Session, and the buyback ring at login.

use super::*;

#[test]
fn buy_item_err_sends_smsg_buy_failed() {
    // When `buy_item` returns Err (e.g. "not enough money"), the gateway must send SMSG_BUY_FAILED
    // with the matching BuyResult code so the player gets an on-screen error.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some("not enough money to buy that item".into()),
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BUY_ITEM {
        vendor: Guid::new(99),
        item: 1234,
        amount: 1,
        unknown1: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_BUY_FAILED(p) => {
            assert_eq!(p.guid.guid(), 99, "vendor guid echoed back");
            assert_eq!(p.item, 1234, "item entry echoed back");
            assert!(
                matches!(p.result, BuyResult::NotEnoughMoney),
                "BuyResult maps to NotEnoughMoney"
            );
        }
        other => panic!("expected SMSG_BUY_FAILED, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn login_replays_a_persisted_buyback_ring_after_the_login_sequence() {
    // The ring survives logout, so world entry rebuilds the tab: one fabricated item CREATE per
    // entry, then the raw descriptor update. (An EMPTY ring emits nothing — every other login test
    // reads the login sequence and then EOF, which is that case.)
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            vendor: VendorState {
                buyback_ring: vec![(2589, 5, 120, 0), (4540, 1, 30, 0)],
                ..base.vendor
            },
            ..base
        }
    });
    let (mut client, _c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    for _ in 0..2 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_) => {}
            other => panic!("expected a fabricated buyback item CREATE, got {other}"),
        }
    }
    // The descriptor update is a hand-rolled partial VALUES mask gtker cannot decode; the frame is
    // consumed either way, and EOF after it proves nothing else was sent.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    assert!(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err());
    drop(client);
    server.join().unwrap();
}

#[test]
fn buyback_maps_the_wire_slot_enum_to_zero_based_ring_slots() {
    // BuybackSlot rides as 69..=81 on the wire; the store reducer takes 0-based ring slots —
    // Slot1 (69) → 0, Slot13 (81) → 12.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_BUYBACK_ITEM {
        guid: Guid::new(99),
        slot: BuybackSlot::Slot1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_BUYBACK_ITEM {
        guid: Guid::new(99),
        slot: BuybackSlot::Slot13,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // 248: a successful buyback now pushes the refreshed tab view (one raw VALUES per call —
    // the mock ring is empty, so no item CREATEs). Consume both frames before EOF; gtker cannot
    // DECODE a hand-rolled partial VALUES mask (no OBJECT_FIELD_TYPE — the raw path's whole
    // reason to exist), so tolerate the parse error: the frame bytes are consumed either way.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.vendor.bought_back.lock().unwrap().as_slice(),
        &[(99, 0), (99, 12)]
    );
}

#[test]
fn list_inventory_opens_the_vendor_window_over_the_socket() {
    let mut s = quest_store();
    s.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LIST_INVENTORY {
        guid: Guid::new(80),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(&body[0..8], &80u64.to_le_bytes());
    assert_eq!(body[8], 1, "one stocked item");
    drop(client);
    server.join().unwrap();
}
