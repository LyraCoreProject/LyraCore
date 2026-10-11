//! Vendor replies through the Protocol Family and encrypted World Session.

use super::handlers::{InMemoryVendorActions, Vendor};
use super::*;

fn run(actions: &InMemoryVendorActions, msg: impl Into<ClientOpcodeMessage>) -> Vec<Outbound> {
    Vendor::handle(
        actions,
        &mut ProtocolSession::in_world(7, 1),
        ProtocolRequest::Message(msg.into()),
    )
    .unwrap()
    .outbound
}

#[test]
fn buy_item_err_sends_smsg_buy_failed() {
    // When `buy_item` returns Err (e.g. "not enough money"), the gateway must send SMSG_BUY_FAILED
    // with the matching BuyResult code so the player gets an on-screen error.
    let actions = InMemoryVendorActions {
        buy_error: Some(|| {
            crate::stdb::ReducerCallError::refused(
                "gw_buy_item",
                "not enough money to buy that item",
            )
        }),
        ..Default::default()
    };
    let sent = run(
        &actions,
        CMSG_BUY_ITEM {
            vendor: Guid::new(99),
            item: 1234,
            amount: 1,
            unknown1: 1,
        },
    );
    match sent.as_slice() {
        [Outbound::One(ServerOpcodeMessage::SMSG_BUY_FAILED(p))] => {
            assert_eq!(p.guid.guid(), 99, "vendor guid echoed back");
            assert_eq!(p.item, 1234, "item entry echoed back");
            assert!(
                matches!(p.result, BuyResult::NotEnoughMoney),
                "BuyResult maps to NotEnoughMoney"
            );
        }
        other => panic!("expected one SMSG_BUY_FAILED, got {} packets", other.len()),
    }
}

#[test]
fn login_replays_a_persisted_buyback_ring_after_the_login_sequence() {
    // The ring survives logout, so world entry rebuilds the tab: one fabricated item CREATE per
    // entry, then the raw descriptor update. (An EMPTY ring emits nothing — every other login test
    // reads the login sequence and then EOF, which is that case.) This stays a socket test because
    // the replay's place in the world entry sequence is the fact under test.
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
    // BuybackSlot rides as 69..=81 on the wire; the ring is 0-based, so Slot1 (69) takes entry 0
    // and Slot13 (81) takes entry 12. The ring holds 13 entries so both ends name a real one, and
    // the later slot goes first so the earlier take does not shift it.
    let ring: Vec<(u32, u32, u32, u32)> = (0..13).map(|i| (100 + i, 1, 10, 0)).collect();
    let actions = InMemoryVendorActions {
        ring: std::sync::Mutex::new(ring.clone()),
        ..Default::default()
    };
    for slot in [BuybackSlot::Slot13, BuybackSlot::Slot1] {
        run(
            &actions,
            CMSG_BUYBACK_ITEM {
                guid: Guid::new(99),
                slot,
            },
        );
    }
    assert_eq!(actions.ring.lock().unwrap().as_slice(), &ring[1..12]);
}

#[test]
fn list_inventory_opens_the_vendor_window() {
    let actions = InMemoryVendorActions {
        stock: vec![codec::VendorItemView {
            item_entry: 4540,
            display_id: 6353,
            buy_price: 25,
            ..Default::default()
        }],
        ..Default::default()
    };
    let sent = run(
        &actions,
        CMSG_LIST_INVENTORY {
            guid: Guid::new(80),
        },
    );
    match sent.as_slice() {
        [Outbound::Raw { opcode, body }] => {
            assert_eq!(*opcode, codec::SMSG_LIST_INVENTORY_OPCODE);
            assert_eq!(&body[0..8], &80u64.to_le_bytes());
            assert_eq!(body[8], 1, "one stocked item");
        }
        _ => panic!("expected one raw vendor window, got {} packets", sent.len()),
    }
}
