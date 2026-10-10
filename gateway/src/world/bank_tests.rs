//! Bank opcodes, run through `handle_bank` against a Fake that holds only the Bank and Npc Stores
//! the handler is bounded on.

use super::handlers::{handle_bank, BankStore, NpcStore};
use super::trainer_tests::family_harness::{drain_outbound, in_world_conn, npc_store_refusing_by};
use super::*;
use std::collections::BTreeSet;
use std::sync::Mutex;

/// A bank that moves an item by its source slot: a carried slot goes into the bank, a banked slot
/// comes back. The Module infers that direction from the slot, so the Fake does too.
#[derive(Default)]
struct BankFake {
    npc_refuses: bool,
    /// Every banker the Module accepts a bag-slot purchase from.
    bankers: BTreeSet<u64>,
    /// Refuses `auto_bank_item` with this text, like a full bank.
    move_error: Option<String>,
    /// Refuses `buy_bank_slot` with this text.
    purchase_error: Option<String>,
    carried: Mutex<BTreeSet<u64>>,
    banked: Mutex<BTreeSet<u64>>,
    bought_slots: Mutex<u32>,
}

impl BankFake {
    fn carrying(slot: u8) -> Self {
        let bank = Self::default();
        bank.carried.lock().unwrap().insert(slot.into());
        bank
    }

    fn banking(slot: u8) -> Self {
        let bank = Self::default();
        bank.banked.lock().unwrap().insert(slot.into());
        bank
    }

    fn carried(&self) -> Vec<u64> {
        self.carried.lock().unwrap().iter().copied().collect()
    }

    fn banked(&self) -> Vec<u64> {
        self.banked.lock().unwrap().iter().copied().collect()
    }
}

npc_store_refusing_by!(BankFake, npc_refuses);

impl BankStore for BankFake {
    fn auto_bank_item(&self, _account_id: u64, _self_guid: u64, slot: u8) -> Result<()> {
        if let Some(error) = &self.move_error {
            return Err(anyhow!("{error}"));
        }
        let slot = u64::from(slot);
        let (mut carried, mut banked) = (self.carried.lock().unwrap(), self.banked.lock().unwrap());
        if carried.remove(&slot) {
            banked.insert(slot);
        } else if banked.remove(&slot) {
            carried.insert(slot);
        }
        Ok(())
    }

    fn buy_bank_slot(&self, _account_id: u64, _self_guid: u64, banker_guid: u64) -> Result<()> {
        if let Some(error) = &self.purchase_error {
            return Err(anyhow!("{error}"));
        }
        if !self.bankers.contains(&banker_guid) {
            return Err(anyhow!("[2] target is not a banker"));
        }
        *self.bought_slots.lock().unwrap() += 1;
        Ok(())
    }
}

/// Send `msg` as Character 1 of account 7 and return what the handler sent, in order.
/// Phase 5 retargets only this helper.
fn run(store: &BankFake, msg: impl Into<ClientOpcodeMessage>) -> Vec<ServerOpcodeMessage> {
    let (tx, rx) = SessionTx::with_depth(0);
    let mut conn = in_world_conn(7, 1);
    let passed_on = handle_bank(&tx, store, &mut conn, msg.into()).unwrap();
    assert!(passed_on.is_none(), "the bank family owns this opcode");
    drain_outbound(&rx)
}

fn kinds(sent: &[ServerOpcodeMessage]) -> String {
    sent.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn banker_activate_sends_smsg_show_bank_with_the_banker_guid() {
    let sent = run(
        &BankFake::default(),
        CMSG_BANKER_ACTIVATE {
            guid: Guid::new(77),
        },
    );
    assert!(
        matches!(sent.as_slice(), [ServerOpcodeMessage::SMSG_SHOW_BANK(p)] if p.guid.guid() == 77),
        "expected SMSG_SHOW_BANK for banker 77, got [{}]",
        kinds(&sent)
    );
}

#[test]
fn banker_activate_on_a_standing_refusing_banker_sends_no_reply() {
    let store = BankFake {
        npc_refuses: true,
        ..Default::default()
    };
    let sent = run(
        &store,
        CMSG_BANKER_ACTIVATE {
            guid: Guid::new(77),
        },
    );
    assert!(sent.is_empty(), "got [{}]", kinds(&sent));
}

/// Right-click a bag item with the bank open: the gateway names the source slot and the Module
/// resolves the free bank slot.
#[test]
fn autobank_item_from_the_main_bag_deposits_the_item() {
    let store = BankFake::carrying(23);
    let sent = run(
        &store,
        CMSG_AUTOBANK_ITEM {
            bag_index: 255,
            slot_index: 23,
        },
    );
    assert!(sent.is_empty(), "got [{}]", kinds(&sent));
    assert_eq!((store.carried(), store.banked()), (vec![], vec![23]));
}

/// Right-click a banked item: withdraw, through the same Store method as the deposit.
#[test]
fn autostore_bank_item_from_the_main_bag_withdraws_the_item() {
    let store = BankFake::banking(39);
    let sent = run(
        &store,
        CMSG_AUTOSTORE_BANK_ITEM {
            bag_index: 255,
            slot_index: 39,
        },
    );
    assert!(sent.is_empty(), "got [{}]", kinds(&sent));
    assert_eq!((store.carried(), store.banked()), (vec![39], vec![]));
}

/// A full destination (bank or carry space) is a per-action error. The client gets the existing
/// inventory-change failure, and the session goes on.
#[test]
fn autobank_item_err_sends_smsg_inventory_change_failure() {
    let store = BankFake {
        move_error: Some("bank full".into()),
        ..BankFake::carrying(23)
    };
    let sent = run(
        &store,
        CMSG_AUTOBANK_ITEM {
            bag_index: 255,
            slot_index: 23,
        },
    );
    assert!(
        matches!(
            sent.as_slice(),
            [ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_)]
        ),
        "expected SMSG_INVENTORY_CHANGE_FAILURE, got [{}]",
        kinds(&sent)
    );
    assert_eq!((store.carried(), store.banked()), (vec![23], vec![]));
}

/// Only the main pseudo-bag (255) is addressed, like the item handler. A sub-bag index is logged
/// and ignored, never fatal.
#[test]
fn autobank_item_from_a_sub_bag_is_unsupported_and_moves_nothing() {
    let store = BankFake::carrying(0);
    let sent = run(
        &store,
        CMSG_AUTOBANK_ITEM {
            bag_index: 19,
            slot_index: 0,
        },
    );
    assert!(sent.is_empty(), "got [{}]", kinds(&sent));
    assert_eq!((store.carried(), store.banked()), (vec![0], vec![]));
}

#[test]
fn autostore_bank_item_from_a_sub_bag_is_unsupported_and_moves_nothing() {
    let store = BankFake::banking(0);
    let sent = run(
        &store,
        CMSG_AUTOSTORE_BANK_ITEM {
            bag_index: 19,
            slot_index: 0,
        },
    );
    assert!(sent.is_empty(), "got [{}]", kinds(&sent));
    assert_eq!((store.carried(), store.banked()), (vec![], vec![0]));
}

/// The Fake accepts a purchase only from a banker it knows, so an `Ok` reply shows the wire's
/// banker guid reached the Store.
#[test]
fn buy_bank_slot_success_sends_ok_and_reaches_the_named_banker() {
    let store = BankFake {
        bankers: [88].into(),
        ..Default::default()
    };
    let sent = run(
        &store,
        CMSG_BUY_BANK_SLOT {
            guid: Guid::new(88),
        },
    );
    assert!(
        matches!(
            sent.as_slice(),
            [ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p)] if p.result == BuyBankSlotResult::Ok
        ),
        "expected an Ok slot purchase, got [{}]",
        kinds(&sent)
    );
    assert_eq!(*store.bought_slots.lock().unwrap(), 1);
}

/// The Module tags a refusal with its `SMSG_BUY_BANK_SLOT_RESULT` code in brackets. The gateway
/// reads the code, not the prose.
#[test]
fn buy_bank_slot_failure_maps_the_bracketed_code_to_the_matching_result() {
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
        let store = BankFake {
            purchase_error: Some(err.into()),
            ..Default::default()
        };
        let sent = run(
            &store,
            CMSG_BUY_BANK_SLOT {
                guid: Guid::new(88),
            },
        );
        let [ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p)] = sent.as_slice() else {
            panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got [{}]", kinds(&sent));
        };
        assert_eq!(p.result, want, "store error {err:?} must map to {want:?}");
        assert_eq!(*store.bought_slots.lock().unwrap(), 0);
    }
}
