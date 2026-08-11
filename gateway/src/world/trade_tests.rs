//! Trade handshake dispatch (#120) — the wire E2E half of the ticket's acceptance: real cipher,
//! real packets, the `InMemoryStore` standing in for the module. What EXECUTES is the dispatch
//! chain and `handlers::trade`; what the recorders prove is which store verb each CMSG chose and
//! which arguments survived the wire, from BOTH sides of the trade. The other half — trade-event
//! rows decoding to `SMSG_TRADE_STATUS` on the recipient's socket — is pinned by
//! `stdb::subscriptions::tests::trade_event_kinds_decode_to_their_trade_status_variants` plus the
//! shared `private_recipient_audience` tests; the full push is the dev-smoke/live-client pass.

use super::*;
use wow_world_messages::vanilla::{
    CMSG_ACCEPT_TRADE, CMSG_BEGIN_TRADE, CMSG_BUSY_TRADE, CMSG_CANCEL_TRADE,
    CMSG_CLEAR_TRADE_ITEM, CMSG_IGNORE_TRADE, CMSG_INITIATE_TRADE, CMSG_SET_TRADE_GOLD,
    CMSG_SET_TRADE_ITEM, CMSG_UNACCEPT_TRADE,
};

/// **AC: initiating a trade with a targeted player reaches the store with the wire's target.**
#[test]
fn initiate_trade_dispatches_with_the_wire_target_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_INITIATE_TRADE {
        guid: Guid::new(2),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // every status rides the game_trade_event relay, no direct SMSG here
    server.join().unwrap();
    assert_eq!(store.initiated_trades.lock().unwrap().as_slice(), &[(1, 2)]);
}

/// **AC: tested from both sides** — the SAME wire flow works with the seats swapped: player 2
/// initiates against player 1.
#[test]
fn initiate_trade_dispatches_from_the_other_side_too() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 2);
    CMSG_INITIATE_TRADE {
        guid: Guid::new(1),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.initiated_trades.lock().unwrap().as_slice(), &[(2, 1)]);
}

/// **AC: the full handshake round trip, one store, both parties** — A proposes, B's client
/// answers `CMSG_BEGIN_TRADE` (the vanilla auto-reply to `BeginTrade`), then B cancels. The
/// per-shard call log pins the dispatch ORDER, the recorders pin who each verb acted as.
#[test]
fn the_handshake_flow_dispatches_initiate_then_begin_then_cancel() {
    let store = std::sync::Arc::new(quest_store());

    let (mut a, mut a_enc, _a_dec, a_server) = enter_world(store.clone(), 1);
    CMSG_INITIATE_TRADE {
        guid: Guid::new(2),
    }
    .write_encrypted_client(&mut a, &mut a_enc)
    .unwrap();
    drop(a);
    a_server.join().unwrap();

    let (mut b, mut b_enc, _b_dec, b_server) = enter_world(store.clone(), 2);
    CMSG_BEGIN_TRADE {}
        .write_encrypted_client(&mut b, &mut b_enc)
        .unwrap();
    CMSG_CANCEL_TRADE {}
        .write_encrypted_client(&mut b, &mut b_enc)
        .unwrap();
    drop(b);
    b_server.join().unwrap();

    assert_eq!(store.initiated_trades.lock().unwrap().as_slice(), &[(1, 2)]);
    assert_eq!(store.begun_trades.lock().unwrap().as_slice(), &[2]);
    assert_eq!(store.cancelled_trades.lock().unwrap().as_slice(), &[2]);
    let calls: Vec<String> = store
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, what)| what.contains("trade"))
        .map(|(_, what)| what.clone())
        .collect();
    assert_eq!(calls, ["initiate_trade", "begin_trade", "cancel_trade"]);
}

/// **AC (#121): offer mutations dispatch with the wire's arguments** — set item (main bag →
/// absolute slot passthrough), clear item, and gold (the `Gold` wire type decoded back to
/// copper). One client, three opcodes, three recorders.
#[test]
fn offer_mutations_dispatch_with_wire_arguments() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SET_TRADE_ITEM {
        trade_slot: 2,
        bag: 255,
        slot: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_CLEAR_TRADE_ITEM { trade_slot: 2 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_SET_TRADE_GOLD {
        gold: wow_world_messages::vanilla::Gold::new(1_2345),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.set_trade_items.lock().unwrap().as_slice(), &[(1, 2, 23)]);
    assert_eq!(store.cleared_trade_items.lock().unwrap().as_slice(), &[(1, 2)]);
    assert_eq!(store.set_trade_golds.lock().unwrap().as_slice(), &[(1, 1_2345)]);
}

/// **AC (#121): sub-bag items are out of scope, not mis-addressed** — a `CMSG_SET_TRADE_ITEM`
/// from an equipped sub-bag is logged and IGNORED (the `handle_item` posture), never forwarded
/// with a bag-local slot number that would alias a main-bag slot.
#[test]
fn set_trade_item_from_a_sub_bag_is_ignored_not_misaddressed() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SET_TRADE_ITEM {
        trade_slot: 0,
        bag: 19,
        slot: 2,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(store.set_trade_items.lock().unwrap().is_empty());
}

/// **AC (#122): the full loop's wire half** — A initiates and offers an item, B answers, offers
/// gold, and both accept; every opcode dispatches its verb as the right seat, in order. (The
/// swap itself — items, gold, atomicity — is the module's pure-tested commit core; the fake
/// store records the dispatch, per the settled seam.)
#[test]
fn the_full_loop_dispatches_offer_and_dual_accept_in_order() {
    let store = std::sync::Arc::new(quest_store());

    let (mut a, mut a_enc, _a_dec, a_server) = enter_world(store.clone(), 1);
    CMSG_INITIATE_TRADE { guid: Guid::new(2) }
        .write_encrypted_client(&mut a, &mut a_enc)
        .unwrap();
    CMSG_SET_TRADE_ITEM { trade_slot: 0, bag: 255, slot: 23 }
        .write_encrypted_client(&mut a, &mut a_enc)
        .unwrap();
    CMSG_ACCEPT_TRADE { unknown1: 0 }
        .write_encrypted_client(&mut a, &mut a_enc)
        .unwrap();
    drop(a);
    a_server.join().unwrap();

    let (mut b, mut b_enc, _b_dec, b_server) = enter_world(store.clone(), 2);
    CMSG_BEGIN_TRADE {}
        .write_encrypted_client(&mut b, &mut b_enc)
        .unwrap();
    CMSG_SET_TRADE_GOLD { gold: wow_world_messages::vanilla::Gold::new(500) }
        .write_encrypted_client(&mut b, &mut b_enc)
        .unwrap();
    CMSG_ACCEPT_TRADE { unknown1: 0 }
        .write_encrypted_client(&mut b, &mut b_enc)
        .unwrap();
    drop(b);
    b_server.join().unwrap();

    assert_eq!(store.initiated_trades.lock().unwrap().as_slice(), &[(1, 2)]);
    assert_eq!(store.set_trade_items.lock().unwrap().as_slice(), &[(1, 0, 23)]);
    assert_eq!(store.set_trade_golds.lock().unwrap().as_slice(), &[(2, 500)]);
    assert_eq!(store.accepted_trades.lock().unwrap().as_slice(), &[1, 2]);
}

/// **AC (#122): the accept-reset wire half** — after an accept, a further offer mutation and an
/// explicit unaccept both dispatch; the reset itself (both flags cleared, BackToTrade to both)
/// is the module's pure-tested rule.
#[test]
fn unaccept_and_post_accept_mutations_dispatch_for_the_acting_seat() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_ACCEPT_TRADE { unknown1: 0 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_SET_TRADE_GOLD { gold: wow_world_messages::vanilla::Gold::new(9) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_UNACCEPT_TRADE {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.accepted_trades.lock().unwrap().as_slice(), &[1]);
    assert_eq!(store.set_trade_golds.lock().unwrap().as_slice(), &[(1, 9)]);
    assert_eq!(store.unaccepted_trades.lock().unwrap().as_slice(), &[1]);
}

/// **AC (#123): the decline flow** — the proposed target's client answers a `BeginTrade` it
/// can't take with `CMSG_BUSY_TRADE` (already in a dialog) or `CMSG_IGNORE_TRADE` (initiator
/// ignored); each dispatches its own decline verb as the declining side.
#[test]
fn decline_opcodes_dispatch_their_own_verbs_for_the_declining_side() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 2);
    CMSG_BUSY_TRADE {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_IGNORE_TRADE {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.busy_trades.lock().unwrap().as_slice(), &[2]);
    assert_eq!(store.ignore_trades.lock().unwrap().as_slice(), &[2]);
}

/// **AC: either side can cancel** — the initiator's own `CMSG_CANCEL_TRADE` dispatches as
/// themselves (the counterpart of the flow test's B-side cancel).
#[test]
fn cancel_trade_dispatches_for_the_initiating_side_too() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_TRADE {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cancelled_trades.lock().unwrap().as_slice(), &[1]);
}
