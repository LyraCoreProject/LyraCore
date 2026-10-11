//! Trade requests and durable outcomes through the Trade Store. The Relay sends trade status;
//! these tests check the Fake's Trade Session and the absence of direct replies.

use super::handlers::{Trade, TradeStore};
use super::*;
use crate::stdb::ReducerCallError;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use wow_world_messages::vanilla::{
    CMSG_ACCEPT_TRADE, CMSG_BEGIN_TRADE, CMSG_BUSY_TRADE, CMSG_CANCEL_TRADE, CMSG_CLEAR_TRADE_ITEM,
    CMSG_IGNORE_TRADE, CMSG_INITIATE_TRADE, CMSG_SET_TRADE_GOLD, CMSG_SET_TRADE_ITEM,
    CMSG_UNACCEPT_TRADE,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Proposed {
        initiator: u64,
        target: u64,
    },
    Open {
        initiator: u64,
        target: u64,
    },
    Cancelled {
        by: u64,
    },
    Declined {
        by: u64,
        reason: Decline,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decline {
    Busy,
    IgnoresInitiator,
}

/// One Trade Session. The Module owns every gate and the accept reset; this keeps just enough to show
/// which seat did what, with which arguments.
#[derive(Default)]
struct Session {
    phase: Phase,
    /// `(seat, trade slot)` to the absolute inventory slot offered.
    items: BTreeMap<(u64, u8), u8>,
    /// Offered copper per seat.
    gold: BTreeMap<u64, u32>,
    accepted: BTreeSet<u64>,
}

#[derive(Default)]
struct TradeFake {
    session: Mutex<Session>,
    /// Answered once by the next request, in place of success.
    failure: Mutex<Option<ReducerCallError>>,
}

impl TradeFake {
    fn failing(failure: ReducerCallError) -> Self {
        Self {
            failure: Mutex::new(Some(failure)),
            ..Self::default()
        }
    }

    fn answer(&self) -> Result<()> {
        match self.failure.lock().unwrap().take() {
            Some(failure) => Err(failure.into()),
            None => Ok(()),
        }
    }

    fn phase(&self) -> Phase {
        self.session.lock().unwrap().phase
    }

    fn items(&self) -> Vec<((u64, u8), u8)> {
        let session = self.session.lock().unwrap();
        session.items.iter().map(|(k, v)| (*k, *v)).collect()
    }

    fn gold(&self) -> Vec<(u64, u32)> {
        let session = self.session.lock().unwrap();
        session.gold.iter().map(|(k, v)| (*k, *v)).collect()
    }

    fn accepted(&self) -> Vec<u64> {
        self.session
            .lock()
            .unwrap()
            .accepted
            .iter()
            .copied()
            .collect()
    }

    fn decline_if_proposed(&self, by: u64, reason: Decline) {
        let mut session = self.session.lock().unwrap();
        if matches!(session.phase, Phase::Proposed { .. }) {
            session.phase = Phase::Declined { by, reason };
        }
    }
}

impl TradeStore for TradeFake {
    fn initiate_trade(&self, actor: Actor, target_guid: u64) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        *self.session.lock().unwrap() = Session {
            phase: Phase::Proposed {
                initiator: self_guid,
                target: target_guid,
            },
            ..Default::default()
        };
        Ok(())
    }

    fn begin_trade(&self, actor: Actor) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        let mut session = self.session.lock().unwrap();
        if let Phase::Proposed { initiator, target } = session.phase {
            if target == self_guid {
                session.phase = Phase::Open { initiator, target };
            }
        }
        Ok(())
    }

    fn cancel_trade(&self, actor: Actor) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.session.lock().unwrap().phase = Phase::Cancelled { by: self_guid };
        Ok(())
    }

    fn set_trade_item(&self, actor: Actor, trade_slot: u8, inv_slot: u8) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        let mut session = self.session.lock().unwrap();
        session.items.insert((self_guid, trade_slot), inv_slot);
        Ok(())
    }

    fn clear_trade_item(&self, actor: Actor, trade_slot: u8) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.session
            .lock()
            .unwrap()
            .items
            .remove(&(self_guid, trade_slot));
        Ok(())
    }

    fn set_trade_gold(&self, actor: Actor, copper: u32) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.session.lock().unwrap().gold.insert(self_guid, copper);
        Ok(())
    }

    fn accept_trade(&self, actor: Actor) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.session.lock().unwrap().accepted.insert(self_guid);
        Ok(())
    }

    fn unaccept_trade(&self, actor: Actor) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.session.lock().unwrap().accepted.remove(&self_guid);
        Ok(())
    }

    fn busy_trade(&self, actor: Actor) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.decline_if_proposed(self_guid, Decline::Busy);
        Ok(())
    }

    fn ignore_trade(&self, actor: Actor) -> Result<()> {
        self.answer()?;
        let self_guid = actor.guid();
        self.decline_if_proposed(self_guid, Decline::IgnoresInitiator);
        Ok(())
    }
}

/// Handle `msg` as `seat`, a Character of Account 7.
fn run(store: &TradeFake, seat: u64, msg: impl Into<ClientOpcodeMessage>) {
    let mut session = ProtocolSession::in_world(7, seat);
    let reply = Trade::handle(store, &mut session, ProtocolRequest::Message(msg.into())).unwrap();
    assert!(reply.outbound.is_empty());
}

/// Initiating a trade with a targeted player names the wire's target.
#[test]
fn initiate_trade_proposes_to_the_wire_target_guid() {
    let store = TradeFake::default();
    run(&store, 1, CMSG_INITIATE_TRADE { guid: Guid::new(2) });
    assert_eq!(
        store.phase(),
        Phase::Proposed {
            initiator: 1,
            target: 2
        }
    );
}

/// The same flow works with the seats swapped: Character 2 initiates against Character 1.
#[test]
fn initiate_trade_proposes_from_the_other_side_too() {
    let store = TradeFake::default();
    run(&store, 2, CMSG_INITIATE_TRADE { guid: Guid::new(1) });
    assert_eq!(
        store.phase(),
        Phase::Proposed {
            initiator: 2,
            target: 1
        }
    );
}

/// The handshake round trip on one Trade Session: A proposes, B's client answers `CMSG_BEGIN_TRADE`
/// (the vanilla auto-reply to `BeginTrade`), then B cancels. Each step reads the phase the Fake
/// reached, so the steps also pin their order: a begin before the proposal opens nothing.
#[test]
fn the_handshake_flow_proposes_then_opens_then_cancels() {
    let store = TradeFake::default();

    run(&store, 1, CMSG_INITIATE_TRADE { guid: Guid::new(2) });
    assert_eq!(
        store.phase(),
        Phase::Proposed {
            initiator: 1,
            target: 2
        }
    );

    run(&store, 2, CMSG_BEGIN_TRADE {});
    assert_eq!(
        store.phase(),
        Phase::Open {
            initiator: 1,
            target: 2
        }
    );

    run(&store, 2, CMSG_CANCEL_TRADE {});
    assert_eq!(store.phase(), Phase::Cancelled { by: 2 });
}

/// Offer mutations carry the wire's arguments: set item (main bag to absolute slot), clear item,
/// and gold (the `Gold` wire type decoded back to copper).
#[test]
fn offer_mutations_carry_the_wire_arguments() {
    let store = TradeFake::default();
    run(
        &store,
        1,
        CMSG_SET_TRADE_ITEM {
            trade_slot: 2,
            bag: 255,
            slot: 23,
        },
    );
    assert_eq!(store.items(), [((1, 2), 23)]);

    run(&store, 1, CMSG_CLEAR_TRADE_ITEM { trade_slot: 2 });
    assert!(store.items().is_empty());

    run(
        &store,
        1,
        CMSG_SET_TRADE_GOLD {
            gold: wow_world_messages::vanilla::Gold::new(1_2345),
        },
    );
    assert_eq!(store.gold(), [(1, 1_2345)]);
}

/// Items inside an equipped sub-bag are out of scope: logged and ignored, never forwarded with a
/// bag-local slot number that would alias a main-bag slot.
#[test]
fn set_trade_item_from_a_sub_bag_is_ignored_not_misaddressed() {
    let store = TradeFake::default();
    run(
        &store,
        1,
        CMSG_SET_TRADE_ITEM {
            trade_slot: 0,
            bag: 19,
            slot: 2,
        },
    );
    assert!(store.items().is_empty());
}

/// The wire half of the full loop: A initiates, offers an item and accepts; B answers, offers gold
/// and accepts. Each seat's offer lands under its own seat. The swap itself (items, gold,
/// atomicity) is the Module's Trade Commit, tested there.
#[test]
fn the_full_loop_records_each_seats_offer_and_both_accepts() {
    let store = TradeFake::default();

    run(&store, 1, CMSG_INITIATE_TRADE { guid: Guid::new(2) });
    run(
        &store,
        1,
        CMSG_SET_TRADE_ITEM {
            trade_slot: 0,
            bag: 255,
            slot: 23,
        },
    );
    run(&store, 1, CMSG_ACCEPT_TRADE { unknown1: 0 });

    run(&store, 2, CMSG_BEGIN_TRADE {});
    run(
        &store,
        2,
        CMSG_SET_TRADE_GOLD {
            gold: wow_world_messages::vanilla::Gold::new(500),
        },
    );
    run(&store, 2, CMSG_ACCEPT_TRADE { unknown1: 0 });

    assert_eq!(
        store.phase(),
        Phase::Open {
            initiator: 1,
            target: 2
        }
    );
    assert_eq!(store.items(), [((1, 0), 23)]);
    assert_eq!(store.gold(), [(2, 500)]);
    assert_eq!(store.accepted(), [1, 2]);
}

/// After an accept, a further offer mutation and an explicit unaccept both reach the Store for the
/// acting seat. The reset itself (both flags cleared, `BackToTrade` to both) is the Module's rule.
#[test]
fn unaccept_and_post_accept_mutations_reach_the_acting_seat() {
    let store = TradeFake::default();
    run(&store, 1, CMSG_ACCEPT_TRADE { unknown1: 0 });
    assert_eq!(store.accepted(), [1]);

    run(
        &store,
        1,
        CMSG_SET_TRADE_GOLD {
            gold: wow_world_messages::vanilla::Gold::new(9),
        },
    );
    assert_eq!(store.gold(), [(1, 9)]);

    run(&store, 1, CMSG_UNACCEPT_TRADE {});
    assert!(store.accepted().is_empty());
}

/// The proposed target's client answers a `BeginTrade` it cannot take with `CMSG_BUSY_TRADE`
/// (already in a dialog) or `CMSG_IGNORE_TRADE` (initiator ignored). Each declines as the
/// declining side, with its own reason.
#[test]
fn decline_opcodes_decline_with_their_own_reason_for_the_declining_side() {
    for (reason, decline) in [
        (Decline::Busy, ClientOpcodeMessage::from(CMSG_BUSY_TRADE {})),
        (
            Decline::IgnoresInitiator,
            ClientOpcodeMessage::from(CMSG_IGNORE_TRADE {}),
        ),
    ] {
        let store = TradeFake::default();
        run(&store, 1, CMSG_INITIATE_TRADE { guid: Guid::new(2) });
        run(&store, 2, decline);
        assert_eq!(store.phase(), Phase::Declined { by: 2, reason });
    }
}

/// Either side can cancel: the initiator's own `CMSG_CANCEL_TRADE` cancels as themselves, the
/// counterpart of the handshake test's cancel by the target.
#[test]
fn cancel_trade_cancels_for_the_initiating_side_too() {
    let store = TradeFake::default();
    run(&store, 1, CMSG_INITIATE_TRADE { guid: Guid::new(2) });
    run(&store, 1, CMSG_CANCEL_TRADE {});
    assert_eq!(store.phase(), Phase::Cancelled { by: 1 });
}

#[test]
fn transport_loss_ends_the_world_session() {
    let store = TradeFake::failing(ReducerCallError::transport_lost("gw_accept_trade"));
    let mut session = ProtocolSession::in_world(7, 1);
    let msg = CMSG_ACCEPT_TRADE { unknown1: 1 };
    let result = Trade::handle(&store, &mut session, ProtocolRequest::Message(msg.into()));
    assert!(result.is_err());
}

#[test]
fn refusal_is_ignored_and_the_world_session_continues() {
    let store = TradeFake::failing(ReducerCallError::refused("gw_accept_trade", "no trade"));
    let mut session = ProtocolSession::in_world(7, 1);
    let msg = CMSG_ACCEPT_TRADE { unknown1: 1 };
    let reply = Trade::handle(&store, &mut session, ProtocolRequest::Message(msg.into())).unwrap();
    assert!(reply.outbound.is_empty());
    assert_eq!(store.accepted(), Vec::<u64>::new());
}
