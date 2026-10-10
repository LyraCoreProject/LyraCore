//! The auction house: the views through their dispatcher, and the raw browse path over an
//! encrypted World Session.

use super::handlers::{store_with, InMemoryAuctionActions};
use super::*;

const PLAYER: AuctionActionPlayer = AuctionActionPlayer { self_guid: Some(1) };

/// Dispatch one auction message and return the packets the session would send for it.
fn run(actions: &InMemoryAuctionActions, msg: impl Into<ClientOpcodeMessage>) -> Vec<Outbound> {
    match dispatch_auction_action(actions, PLAYER, msg.into()).unwrap() {
        AuctionActionOutcome::Handled { outbound } => outbound,
        AuctionActionOutcome::PassThrough(_) => {
            panic!("the auction dispatcher passed the message on")
        }
    }
}

/// The one typed packet a request answered with.
fn only_message(mut sent: Vec<Outbound>) -> ServerOpcodeMessage {
    match (sent.pop(), sent.is_empty()) {
        (Some(Outbound::One(message)), true) => message,
        _ => panic!("expected exactly one typed packet"),
    }
}

#[test]
fn auction_house_round_trip_stays_typed_over_the_dispatcher() {
    let actions = store_with(Some(imported_auction_interaction()));
    let auctioneer = Guid::new(42);

    match only_message(run(&actions, MSG_AUCTION_HELLO_Client { auctioneer })) {
        ServerOpcodeMessage::MSG_AUCTION_HELLO(message) => {
            assert_eq!(message.auctioneer, auctioneer);
            assert_eq!(message.auction_house.as_int(), 1);
        }
        other => panic!("expected auction hello, got {other}"),
    }

    let owner = CMSG_AUCTION_LIST_OWNER_ITEMS {
        auctioneer,
        ..Default::default()
    };
    match only_message(run(&actions, owner)) {
        ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty owner view, got {other}"),
    }

    let bidder = CMSG_AUCTION_LIST_BIDDER_ITEMS {
        auctioneer,
        ..Default::default()
    };
    match only_message(run(&actions, bidder)) {
        ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty bidder view, got {other}"),
    }
}

#[test]
fn raw_auction_browse_answers_an_empty_page_over_the_encrypted_session() {
    // The raw CMSG_AUCTION_LIST_ITEMS path runs before typed dispatch in the session read loop,
    // so only a socket reaches it.
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

    CMSG_AUCTION_LIST_ITEMS {
        auctioneer: Guid::new(42),
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty browse view, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn an_unresolved_auctioneer_gets_no_reply() {
    // An auctioneer the Module does not resolve (no interaction) gets no reply at all: the refusal
    // is silent and the dispatcher returns Ok, so the session keeps serving.
    let actions = store_with(None);
    let sent = run(
        &actions,
        MSG_AUCTION_HELLO_Client {
            auctioneer: Guid::new(999),
        },
    );
    assert!(sent.is_empty());
}

#[test]
fn refused_auctioneer_interaction_keeps_the_encrypted_world_session_alive() {
    // A resolvable requester with no seeded in-world Characters (`entity_in_world: false`
    // overrides `quest_store`'s blanket flag), so the WHO answer this test cares about is the
    // empty-but-present reply, not "no answer for an unknown requester" (a different rule, pinned
    // in `social.rs`'s own WHO tests). It stays a socket test because the sentinel proves the
    // session survives the silent refusal, and nothing else pins that WHO reply.
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
