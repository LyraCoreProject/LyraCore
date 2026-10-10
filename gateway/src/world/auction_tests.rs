//! The auction house over an encrypted World Session.

use super::*;

#[test]
fn auction_house_round_trip_stays_typed_and_ordered_over_an_encrypted_session() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            auction: AuctionState {
                auction_interaction: Some(imported_auction_interaction()),
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    let auctioneer = Guid::new(42);

    MSG_AUCTION_HELLO_Client { auctioneer }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_AUCTION_HELLO(message) => {
            assert_eq!(message.auctioneer, auctioneer);
            assert_eq!(message.auction_house.as_int(), 1);
        }
        other => panic!("expected auction hello first, got {other}"),
    }

    CMSG_AUCTION_LIST_ITEMS {
        auctioneer,
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty browse view second, got {other}"),
    }

    CMSG_AUCTION_LIST_OWNER_ITEMS {
        auctioneer,
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty owner view third, got {other}"),
    }

    CMSG_AUCTION_LIST_BIDDER_ITEMS {
        auctioneer,
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty bidder view fourth, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn refused_auctioneer_interaction_keeps_the_encrypted_world_session_alive() {
    // A resolvable requester with no seeded in-world Characters (`entity_in_world: false`
    // overrides `quest_store`'s blanket flag), so the WHO answer this test cares about is the
    // empty-but-present reply, not "no answer for an unknown requester" (a different rule, pinned
    // in `social.rs`'s own WHO tests).
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Tester".into(),
                race: 1,
                ..Default::default()
            }],
            session: SessionState {
                entity_in_world: false,
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    MSG_AUCTION_HELLO_Client {
        auctioneer: Guid::new(999),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_WHO::default()
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // RAW-encoded (codec::build_who_response_raw); the auction refusal must be silent and leave
    // WHO's own empty-roster reply next.
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, codec::social::SMSG_WHO_OPCODE);
    assert_eq!(
        &body[0..8],
        &[0u8; 8],
        "no in-world Characters: listed and online both 0"
    );

    drop(client);
    server.join().unwrap();
}
