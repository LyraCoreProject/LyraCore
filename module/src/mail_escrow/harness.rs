//! In-memory interruption harness for the cross-database mail protocol.

use super::*;
use crate::items::ItemSnapshot;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
#[derive(Clone)]
struct XEscrow {
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    postage: u32,
    created_micros: i64,
    delivered: bool,
    payout: bool,
    mail_id: u64,
    item: ItemSnapshot,
    cod: u32,
}
#[derive(Default)]
struct FakeLedger {
    escrows: RefCell<HashMap<u64, XEscrow>>,
    now: Cell<i64>,
    reaper_armed: Cell<bool>,
}

impl FakeLedger {
    fn fenced_copper(&self) -> u32 {
        self.escrows
            .borrow()
            .values()
            .map(|e| e.money.saturating_add(e.postage))
            .sum()
    }
    fn fenced_items(&self) -> usize {
        self.escrows
            .borrow()
            .values()
            .filter(|e| !e.item.is_empty())
            .count()
    }
    fn has_fence(&self, escrow_id: u64) -> bool {
        self.escrows.borrow().contains_key(&escrow_id)
    }
    fn advance(&self, micros: i64) {
        self.now.set(self.now.get() + micros);
    }
}

impl EscrowLedger for FakeLedger {
    fn escrow(&self, escrow_id: u64) -> Option<MailEscrow> {
        self.escrows.borrow().get(&escrow_id).map(|e| MailEscrow {
            escrow_id,
            sender_guid: e.sender_guid,
            recipient_guid: e.recipient_guid,
            subject: e.subject.clone(),
            body: e.body.clone(),
            money: e.money,
            postage: e.postage,
            created_micros: e.created_micros,
            delivered: e.delivered,
            payout: e.payout,
            mail_id: e.mail_id,
            item_entry: e.item.entry,
            item_stack_count: e.item.stack_count,
            item_durability: e.item.durability,
            item_enchant_id: e.item.enchant_id,
            item_soulbound: e.item.soulbound,
            cod: e.cod,
        })
    }
    fn file_escrow(&mut self, row: MailEscrow) {
        let item = row.item();
        self.escrows.borrow_mut().insert(
            row.escrow_id,
            XEscrow {
                sender_guid: row.sender_guid,
                recipient_guid: row.recipient_guid,
                subject: row.subject,
                body: row.body,
                money: row.money,
                postage: row.postage,
                created_micros: row.created_micros,
                delivered: row.delivered,
                payout: row.payout,
                mail_id: row.mail_id,
                item,
                cod: row.cod,
            },
        );
    }
    fn attest_delivery(&mut self, escrow_id: u64) {
        if let Some(row) = self.escrows.borrow_mut().get_mut(&escrow_id) {
            row.delivered = true;
        }
    }
    fn delete_escrow(&mut self, escrow_id: u64) {
        self.escrows.borrow_mut().remove(&escrow_id);
    }
    fn arm_reaper(&mut self) {
        self.reaper_armed.set(true);
    }
    fn now_micros(&self) -> i64 {
        self.now.get()
    }
}

impl ReapSink for FakeLedger {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        self.escrows
            .borrow()
            .iter()
            .map(|(id, e)| (*id, e.sender_guid, e.created_micros, e.delivered))
            .collect()
    }
}
#[derive(Default)]
pub struct FakeShard {
    purses: RefCell<HashMap<u64, u32>>,
    receipts: RefCell<HashMap<u64, (u64, u64)>>,
    items: RefCell<HashMap<u64, (u64, ItemSnapshot)>>,
    bags_full: Cell<bool>,
    next_item_guid: Cell<u64>,
    ledger: FakeLedger,
}

impl FakeShard {
    fn with_purse(guid: u64, copper: u32) -> Self {
        let db = Self::default();
        db.purses.borrow_mut().insert(guid, copper);
        db
    }
    fn purse_of(&self, guid: u64) -> u32 {
        self.purses.borrow().get(&guid).copied().unwrap_or(0)
    }
    fn bags_of(&self, owner: u64) -> Vec<ItemSnapshot> {
        let mut held: Vec<ItemSnapshot> = self
            .items
            .borrow()
            .values()
            .filter(|(o, _)| *o == owner)
            .map(|(_, i)| *i)
            .collect();
        held.sort_by_key(|i| i.entry);
        held
    }
    fn give_item(&self, owner: u64, guid: u64, item: ItemSnapshot) {
        self.items.borrow_mut().insert(guid, (owner, item));
    }
    fn fenced_copper(&self) -> u32 {
        self.ledger.fenced_copper()
    }
    fn fenced_items(&self) -> usize {
        self.ledger.fenced_items()
    }
    fn has_fence(&self, escrow_id: u64) -> bool {
        self.ledger.has_fence(escrow_id)
    }
    fn advance(&self, micros: i64) {
        self.ledger.advance(micros);
    }
}

impl EscrowLedger for FakeShard {
    fn escrow(&self, escrow_id: u64) -> Option<MailEscrow> {
        self.ledger.escrow(escrow_id)
    }
    fn file_escrow(&mut self, row: MailEscrow) {
        self.ledger.file_escrow(row);
    }
    fn attest_delivery(&mut self, escrow_id: u64) {
        self.ledger.attest_delivery(escrow_id);
    }
    fn delete_escrow(&mut self, escrow_id: u64) {
        self.ledger.delete_escrow(escrow_id);
    }
    fn arm_reaper(&mut self) {
        self.ledger.arm_reaper();
    }
    fn now_micros(&self) -> i64 {
        self.ledger.now_micros()
    }
}

impl FenceSink for FakeShard {
    fn purse(&self, sender_guid: u64) -> Option<u32> {
        self.purses.borrow().get(&sender_guid).copied()
    }
    fn debit_purse(&mut self, sender_guid: u64, amount: u32) {
        if let Some(p) = self.purses.borrow_mut().get_mut(&sender_guid) {
            *p = p.saturating_sub(amount);
        }
    }
    fn detach_item(&mut self, sender_guid: u64, item_guid: u64) -> Result<ItemSnapshot, String> {
        let owned = self
            .items
            .borrow()
            .get(&item_guid)
            .map(|(o, i)| (*o, i.soulbound));
        match crate::mail::plan_attach(item_guid, owned, sender_guid) {
            crate::mail::Attach::Nothing => return Ok(ItemSnapshot::default()),
            crate::mail::Attach::NotYours => {
                return Err(lyracore_shared::mail::NOT_YOUR_ITEM.to_string())
            }
            crate::mail::Attach::Soulbound => {
                return Err(lyracore_shared::mail::ITEM_IS_SOULBOUND.to_string())
            }
            crate::mail::Attach::Detach => {}
        }
        Ok(self.items.borrow_mut().remove(&item_guid).expect("owned").1)
    }
}

impl PayoutSink for FakeShard {
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery> {
        self.receipts
            .borrow()
            .get(&escrow_id)
            .map(|(mail_id, payee)| MailDelivery {
                escrow_id,
                mail_id: *mail_id,
                recipient_guid: *payee,
                created_micros: 0,
            })
    }
    fn credit_purse(&mut self, payee_guid: u64, amount: u32) -> bool {
        match self.purses.borrow_mut().get_mut(&payee_guid) {
            Some(p) => {
                *p = p.saturating_add(amount);
                true
            }
            None => false,
        }
    }
    fn grant_item(&mut self, payee_guid: u64, item: &ItemSnapshot) -> Result<(), String> {
        if !self.purses.borrow().contains_key(&payee_guid) {
            return Err(lyracore_shared::mail::NOT_IN_WORLD.to_string());
        }
        if self.bags_full.get() {
            return Err(lyracore_shared::mail::INVENTORY_FULL.to_string());
        }
        let guid = self.next_item_guid.get() + 1;
        self.next_item_guid.set(guid);
        self.items.borrow_mut().insert(guid, (payee_guid, *item));
        Ok(())
    }
    fn file_receipt(&mut self, row: MailDelivery) {
        self.receipts
            .borrow_mut()
            .insert(row.escrow_id, (row.mail_id, row.recipient_guid));
    }
    fn now_micros(&self) -> i64 {
        self.ledger.now_micros()
    }
}

impl ReapSink for FakeShard {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        self.ledger.escrows()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct XMail {
    id: u64,
    recipient_guid: u64,
    sender_guid: u64,
    subject: String,
    body: String,
    money: u32,
    cod: u32,
    item: ItemSnapshot,
}
#[derive(Default)]
pub struct FakeMailPlane {
    mails: RefCell<Vec<XMail>>,
    receipts: RefCell<HashMap<u64, (u64, u64)>>,
    next_mail_id: Cell<u64>,
    ledger: FakeLedger,
}

impl FakeMailPlane {
    fn mailbox_of(&self, recipient_guid: u64) -> Vec<XMail> {
        self.mails
            .borrow()
            .iter()
            .filter(|m| m.recipient_guid == recipient_guid)
            .cloned()
            .collect()
    }
    fn money_in_mailbox(&self, recipient_guid: u64) -> u32 {
        self.mailbox_of(recipient_guid)
            .iter()
            .map(|m| m.money)
            .sum()
    }
    fn items_in_mailbox(&self, recipient_guid: u64) -> Vec<ItemSnapshot> {
        self.mailbox_of(recipient_guid)
            .into_iter()
            .map(|m| m.item)
            .filter(|i| !i.is_empty())
            .collect()
    }
    fn fenced_copper(&self) -> u32 {
        self.ledger.fenced_copper()
    }
    fn fenced_items(&self) -> usize {
        self.ledger.fenced_items()
    }
    fn has_fence(&self, escrow_id: u64) -> bool {
        self.ledger.has_fence(escrow_id)
    }
    fn advance(&self, micros: i64) {
        self.ledger.advance(micros);
    }
}

impl EscrowLedger for FakeMailPlane {
    fn escrow(&self, escrow_id: u64) -> Option<MailEscrow> {
        self.ledger.escrow(escrow_id)
    }
    fn file_escrow(&mut self, row: MailEscrow) {
        self.ledger.file_escrow(row);
    }
    fn attest_delivery(&mut self, escrow_id: u64) {
        self.ledger.attest_delivery(escrow_id);
    }
    fn delete_escrow(&mut self, escrow_id: u64) {
        self.ledger.delete_escrow(escrow_id);
    }
    fn arm_reaper(&mut self) {
        self.ledger.arm_reaper();
    }
    fn now_micros(&self) -> i64 {
        self.ledger.now_micros()
    }
}

impl ReapSink for FakeMailPlane {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        self.ledger.escrows()
    }
}

impl TakeFenceSink for FakeMailPlane {
    fn mail(&self, mail_id: u64) -> Option<(u64, u32)> {
        self.mails
            .borrow()
            .iter()
            .find(|m| m.id == mail_id)
            .map(|m| (m.recipient_guid, m.money))
    }
    fn clear_mail_money(&mut self, mail_id: u64) {
        if let Some(m) = self.mails.borrow_mut().iter_mut().find(|m| m.id == mail_id) {
            m.money = 0;
        }
    }
    fn mail_item(&self, mail_id: u64) -> Option<(u64, ItemSnapshot)> {
        self.mails
            .borrow()
            .iter()
            .find(|m| m.id == mail_id)
            .map(|m| (m.recipient_guid, m.item))
    }
    fn clear_mail_item(&mut self, mail_id: u64) {
        if let Some(m) = self.mails.borrow_mut().iter_mut().find(|m| m.id == mail_id) {
            m.item = ItemSnapshot::default();
        }
    }
}

impl DeliverySink for FakeMailPlane {
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery> {
        self.receipts
            .borrow()
            .get(&escrow_id)
            .map(|(mail_id, recipient_guid)| MailDelivery {
                escrow_id,
                mail_id: *mail_id,
                recipient_guid: *recipient_guid,
                created_micros: 0,
            })
    }
    fn deliver(&mut self, sender_guid: u64, letter: &Letter, item: &ItemSnapshot) -> u64 {
        let id = self.next_mail_id.get() + 1;
        self.next_mail_id.set(id);
        self.mails.borrow_mut().push(XMail {
            id,
            recipient_guid: letter.recipient_guid,
            sender_guid,
            subject: letter.subject.clone(),
            body: letter.body.clone(),
            money: letter.money,
            cod: letter.cod,
            item: *item,
        });
        id
    }
    fn settle_cod(&mut self, mail_id: u64) {
        if let Some(m) = self.mails.borrow_mut().iter_mut().find(|m| m.id == mail_id) {
            m.cod = 0;
        }
    }
    fn file_receipt(&mut self, row: MailDelivery) {
        self.receipts
            .borrow_mut()
            .insert(row.escrow_id, (row.mail_id, row.recipient_guid));
    }
    fn now_micros(&self) -> i64 {
        self.ledger.now_micros()
    }
}

const ESCROW: u64 = 0x5EED_0001;
const SENDER: u64 = 11;
const RECIPIENT: u64 = 22;
const PURSE: u32 = 500;
const MONEY: u32 = 100;
const POSTAGE: u32 = 30;
const COST: u32 = MONEY + POSTAGE;
const ITEM_GUID: u64 = 0x4000_0000_0000_0011;
const NO_ITEM: u64 = 0;
const NO_COD_MAIL: u64 = 0;
fn sword() -> ItemSnapshot {
    ItemSnapshot {
        entry: 5_090_001,
        stack_count: 1,
        durability: 42,
        enchant_id: 7,
        soulbound: false,
    }
}

fn letter() -> Letter {
    Letter {
        recipient_guid: RECIPIENT,
        subject: "Your sword".into(),
        body: "left it at the inn".into(),
        money: MONEY,
        postage: POSTAGE,
        cod: 0,
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Killed {
    BeforeFence,
    AfterFence,
    AfterCommit,
    AfterConfirm,
    Never,
}
fn drive(
    shard: &mut FakeShard,
    plane: &mut FakeMailPlane,
    escrow_id: u64,
    item_guid: u64,
    killed: Killed,
) -> Result<(), String> {
    let items = claimable_items(shard, plane);
    let check = |shard: &FakeShard, plane: &FakeMailPlane| {
        assert_value_is_in_exactly_one_place(shard, plane);
        assert_the_item_is_never_lost_or_duplicated(shard, plane, items);
    };
    check(shard, plane);
    if killed == Killed::BeforeFence {
        return Ok(());
    }
    apply_fence(shard, escrow_id, SENDER, letter(), item_guid, NO_COD_MAIL)?;
    check(shard, plane);
    if killed == Killed::AfterFence {
        return Ok(());
    }
    let item = shard
        .escrow(escrow_id)
        .map(|e| e.item())
        .unwrap_or_default();
    apply_commit(plane, escrow_id, SENDER, &letter(), &item, NO_COD_MAIL)?;
    check(shard, plane);
    if killed == Killed::AfterCommit {
        return Ok(());
    }
    apply_confirm(shard, escrow_id)?;
    check(shard, plane);
    if killed == Killed::AfterConfirm {
        return Ok(());
    }
    apply_settle(shard, escrow_id)?;
    check(shard, plane);
    Ok(())
}
fn assert_value_is_in_exactly_one_place(shard: &FakeShard, plane: &FakeMailPlane) {
    let purse = shard.purse_of(SENDER);
    assert!(
        purse == PURSE || purse == PURSE - COST,
        "the purse went somewhere it should not ({purse}): a fence debits it exactly once and \
         nothing ever credits it back"
    );
    let delivered = plane.mailbox_of(RECIPIENT);
    assert!(
        delivered.len() <= 1,
        "the letter was delivered {} times — the escrow id is the idempotency key precisely so a \
         replayed commit writes one mail",
        delivered.len()
    );
    if purse == PURSE - COST {
        assert!(
            shard.fenced_copper() == COST || !delivered.is_empty(),
            "the sender has paid, the fence is gone and no mail row exists — the value is nowhere, \
             which is the one unrecoverable outcome delete-last exists to make unreachable"
        );
    }
}
fn claimable_items(shard: &FakeShard, plane: &FakeMailPlane) -> usize {
    shard.bags_of(SENDER).len()
        + shard.bags_of(RECIPIENT).len()
        + plane.items_in_mailbox(RECIPIENT).len()
}
fn assert_the_item_is_never_lost_or_duplicated(
    shard: &FakeShard,
    plane: &FakeMailPlane,
    expected: usize,
) {
    let claimable = claimable_items(shard, plane);
    let fenced = shard.fenced_items() + plane.fenced_items();
    assert!(
        claimable <= expected,
        "the attachment is claimable from {claimable} places (expected at most {expected}) — an \
         item is the value that cannot be recreated, so duplication is the failure this ordering \
         exists to make unreachable"
    );
    assert!(
        claimable + fenced >= expected,
        "the attachment is claimable from {claimable} places and fenced in {fenced} — it is \
         nowhere, which is the one unrecoverable outcome delete-last exists to make unreachable"
    );
}

fn fixture() -> (FakeShard, FakeMailPlane) {
    (
        FakeShard::with_purse(SENDER, PURSE),
        FakeMailPlane::default(),
    )
}
#[test]
fn the_four_step_sequence_moves_coin_between_two_databases() {
    let (mut shard, mut plane) = fixture();

    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::Never).expect("the drive completes");

    assert_eq!(shard.purse_of(SENDER), PURSE - COST, "one debit, not two");
    let inbox = plane.mailbox_of(RECIPIENT);
    assert_eq!(inbox.len(), 1);
    assert_eq!(
        inbox[0].money, MONEY,
        "the coin travelled, the postage did not"
    );
    assert_eq!(inbox[0].sender_guid, SENDER);
    assert_eq!(inbox[0].subject, "Your sword");
    assert!(!shard.has_fence(ESCROW), "the fence is settled");
}
#[test]
fn the_first_fence_arms_the_reaper() {
    let (mut shard, _plane) = fixture();
    assert!(!shard.ledger.reaper_armed.get());

    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM, NO_COD_MAIL).expect("fenced");

    assert!(shard.ledger.reaper_armed.get());
}
#[test]
fn a_driver_killed_after_the_fence_leaves_the_value_held_and_never_refunded() {
    let (mut shard, mut plane) = fixture();

    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::AfterFence).expect("the fence lands");

    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
    assert_eq!(shard.fenced_copper(), COST, "the ledger holds it");
    assert!(plane.mailbox_of(RECIPIENT).is_empty());

    shard.advance(MAIL_ESCROW_STALE_MICROS * 100);
    apply_reap(&mut shard);

    assert!(
        shard.has_fence(ESCROW),
        "an unattested fence is HELD, not refunded: refunding it against a commit that did land \
         puts the coin in the purse AND in the mailbox"
    );
    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
    assert_value_is_in_exactly_one_place(&shard, &plane);
}
#[test]
fn a_driver_killed_after_the_commit_replays_forward_into_one_letter() {
    let (mut shard, mut plane) = fixture();

    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::AfterCommit).expect("the commit lands");
    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
    assert!(shard.has_fence(ESCROW), "nothing has attested the delivery");

    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::Never).expect("the re-drive completes");

    assert_eq!(
        plane.mailbox_of(RECIPIENT).len(),
        1,
        "the re-driven commit found its receipt and wrote nothing"
    );
    assert_eq!(shard.purse_of(SENDER), PURSE - COST, "and debited nothing");
    assert!(!shard.has_fence(ESCROW));
}
#[test]
fn a_driver_killed_after_the_attestation_is_rolled_forward_by_the_reaper() {
    let (mut shard, mut plane) = fixture();

    drive(
        &mut shard,
        &mut plane,
        ESCROW,
        NO_ITEM,
        Killed::AfterConfirm,
    )
    .expect("the attestation lands");
    assert!(shard.has_fence(ESCROW));

    shard.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut shard);

    assert!(!shard.has_fence(ESCROW), "rolled forward");
    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
    assert_value_is_in_exactly_one_place(&shard, &plane);
}
#[test]
fn a_settle_replayed_after_the_drive_completed_is_a_no_op() {
    let (mut shard, mut plane) = fixture();
    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::Never).expect("the drive completes");

    apply_settle(&mut shard, ESCROW).expect("a replayed settle is not an error");

    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
}
#[test]
fn a_driver_killed_before_the_fence_costs_the_sender_nothing() {
    let (mut shard, mut plane) = fixture();

    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::BeforeFence).expect("nothing happened");

    assert_eq!(shard.purse_of(SENDER), PURSE);
    assert!(plane.mailbox_of(RECIPIENT).is_empty());
    shard.advance(MAIL_ESCROW_STALE_MICROS * 10);
    apply_reap(&mut shard); // an empty ledger reaps nothing
    assert_eq!(shard.purse_of(SENDER), PURSE);
}
#[test]
fn a_replayed_fence_debits_the_purse_once() {
    let (mut shard, _plane) = fixture();

    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM, NO_COD_MAIL).expect("first");
    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM, NO_COD_MAIL)
        .expect("replay is a no-op, not an error");

    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
    assert_eq!(shard.fenced_copper(), COST);
}
#[test]
fn an_escrow_id_reused_for_another_sender_is_refused() {
    let (mut shard, _plane) = fixture();
    shard.purses.borrow_mut().insert(99, PURSE);
    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM, NO_COD_MAIL).expect("first");

    let err = apply_fence(&mut shard, ESCROW, 99, letter(), NO_ITEM, NO_COD_MAIL)
        .expect_err("the id is taken");

    assert!(err.contains("already fenced"), "{err}");
    assert_eq!(shard.purse_of(99), PURSE, "the second sender paid nothing");
}
#[test]
fn a_replayed_commit_produces_one_mail_and_not_two() {
    let (_shard, mut plane) = fixture();

    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
        NO_COD_MAIL,
    )
    .expect("first");
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
        NO_COD_MAIL,
    )
    .expect("replay");
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
        NO_COD_MAIL,
    )
    .expect("replay again");

    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
}
#[test]
fn re_fencing_an_id_for_a_different_amount_is_refused() {
    let (mut shard, _plane) = fixture();
    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM, NO_COD_MAIL).expect("first");

    let err = apply_fence(
        &mut shard,
        ESCROW,
        SENDER,
        Letter {
            money: MONEY * 2,
            ..letter()
        },
        NO_ITEM,
        NO_COD_MAIL,
    )
    .expect_err("a different letter under the same id");

    assert!(err.contains("different amount"), "{err}");
    assert_eq!(shard.purse_of(SENDER), PURSE - COST, "debited once");
    assert_eq!(shard.fenced_copper(), COST);
}
#[test]
fn an_escrow_id_that_already_delivered_to_another_recipient_is_refused() {
    let (_shard, mut plane) = fixture();
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
        NO_COD_MAIL,
    )
    .expect("first letter");

    let err = apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &Letter {
            recipient_guid: RECIPIENT + 1,
            ..letter()
        },
        &ItemSnapshot::default(),
        NO_COD_MAIL,
    )
    .expect_err("the id belongs to another letter");

    assert!(err.contains("another recipient"), "{err}");
    assert!(plane.mailbox_of(RECIPIENT + 1).is_empty());
}
#[test]
fn escrow_id_zero_is_reserved_on_both_planes() {
    let (mut shard, mut plane) = fixture();

    apply_fence(&mut shard, 0, SENDER, letter(), NO_ITEM, NO_COD_MAIL).expect_err("reserved");
    apply_commit(
        &mut plane,
        0,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
        NO_COD_MAIL,
    )
    .expect_err("reserved");

    assert_eq!(shard.purse_of(SENDER), PURSE);
    assert!(plane.mailbox_of(RECIPIENT).is_empty());
}
#[test]
fn an_unaffordable_letter_fences_nothing() {
    let mut shard = FakeShard::with_purse(SENDER, COST - 1);

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM, NO_COD_MAIL)
        .expect_err("cannot pay");

    assert!(
        err.contains(lyracore_shared::mail::NOT_ENOUGH_MONEY),
        "{err}"
    );
    assert_eq!(shard.purse_of(SENDER), COST - 1);
    assert!(!shard.has_fence(ESCROW));
}
#[test]
fn a_settle_before_the_attestation_is_refused_and_the_fence_survives() {
    let (mut shard, mut plane) = fixture();
    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::AfterCommit).expect("committed");

    let err = apply_settle(&mut shard, ESCROW).expect_err("not attested");

    assert!(err.contains("not attested"), "{err}");
    assert!(shard.has_fence(ESCROW), "the value is still held");
    assert_eq!(shard.fenced_copper(), COST);
}
#[test]
fn an_attestation_for_an_unknown_escrow_is_refused() {
    let (mut shard, _plane) = fixture();

    let err = apply_confirm(&mut shard, ESCROW).expect_err("nothing is fenced");

    assert!(err.contains("nothing fenced"), "{err}");
}
#[test]
fn a_settle_that_reaches_a_database_holding_no_fence_is_not_an_error() {
    let mut elsewhere = FakeShard::default();

    apply_settle(&mut elsewhere, ESCROW).expect("nothing to do here");
}
#[test]
fn the_reaper_does_not_touch_a_fence_a_driver_is_still_working_on() {
    let (mut shard, mut plane) = fixture();
    drive(
        &mut shard,
        &mut plane,
        ESCROW,
        NO_ITEM,
        Killed::AfterConfirm,
    )
    .expect("attested");

    shard.advance(MAIL_ESCROW_STALE_MICROS - 1);
    apply_reap(&mut shard);

    assert!(shard.has_fence(ESCROW), "still within the stale window");
}
#[test]
fn the_reaper_judges_each_fence_on_its_own_evidence() {
    let (mut shard, mut plane) = fixture();
    let held = ESCROW + 1;
    drive(
        &mut shard,
        &mut plane,
        ESCROW,
        NO_ITEM,
        Killed::AfterConfirm,
    )
    .expect("attested");
    apply_fence(
        &mut shard,
        held,
        SENDER,
        Letter {
            money: 0,
            postage: POSTAGE,
            ..letter()
        },
        NO_ITEM,
        NO_COD_MAIL,
    )
    .expect("a second, unattested fence");

    shard.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut shard);

    assert!(!shard.has_fence(ESCROW), "attested — rolled forward");
    assert!(shard.has_fence(held), "unattested — held");
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TakeKilled {
    AfterFence,
    AfterPayout,
    AfterConfirm,
    Never,
}
const TAKE: u64 = 0x5EED_0002;
fn delivered_fixture() -> (FakeShard, FakeMailPlane, u64) {
    let (mut shard, mut plane) = fixture();
    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::Never).expect("the letter is delivered");
    shard.purses.borrow_mut().insert(RECIPIENT, 0);
    let mail_id = plane.mailbox_of(RECIPIENT)[0].id;
    (shard, plane, mail_id)
}
fn drive_take(
    shard: &mut FakeShard,
    plane: &mut FakeMailPlane,
    escrow_id: u64,
    mail_id: u64,
    amount: u32,
    killed: TakeKilled,
) -> Result<(), String> {
    assert_the_take_is_in_exactly_one_place(shard, plane, mail_id);
    apply_take_fence(plane, escrow_id, RECIPIENT, mail_id, amount)?;
    assert_the_take_is_in_exactly_one_place(shard, plane, mail_id);
    if killed == TakeKilled::AfterFence {
        return Ok(());
    }
    apply_payout(shard, escrow_id, RECIPIENT, mail_id, amount)?;
    assert_the_take_is_in_exactly_one_place(shard, plane, mail_id);
    if killed == TakeKilled::AfterPayout {
        return Ok(());
    }
    apply_confirm(plane, escrow_id)?;
    assert_the_take_is_in_exactly_one_place(shard, plane, mail_id);
    if killed == TakeKilled::AfterConfirm {
        return Ok(());
    }
    apply_settle(plane, escrow_id)?;
    assert_the_take_is_in_exactly_one_place(shard, plane, mail_id);
    Ok(())
}
fn assert_the_take_is_in_exactly_one_place(
    shard: &FakeShard,
    plane: &FakeMailPlane,
    _mail_id: u64,
) {
    let purse = shard.purse_of(RECIPIENT);
    assert!(
        purse == 0 || purse == MONEY,
        "the purse holds {purse}: a take credits it exactly once"
    );
    let in_row = plane.money_in_mailbox(RECIPIENT);
    assert!(
        in_row == 0 || in_row == MONEY,
        "the mail row holds {in_row}: the copper leaves it once and never comes back"
    );
    if in_row == 0 && purse == 0 {
        assert_eq!(
            plane.fenced_copper(),
            MONEY,
            "the row is empty, the purse is uncredited and no fence holds the copper — the value is \
             nowhere, which is the one unrecoverable outcome delete-last exists to make unreachable"
        );
    }
}
#[test]
fn the_take_moves_a_mails_copper_into_the_purse_on_another_database() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();

    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::Never,
    )
    .expect("the take completes");

    assert_eq!(shard.purse_of(RECIPIENT), MONEY);
    let inbox = plane.mailbox_of(RECIPIENT);
    assert_eq!(inbox.len(), 1, "a mail emptied of money is still a letter");
    assert_eq!(inbox[0].money, 0);
    assert!(!plane.has_fence(TAKE), "settled");
}
#[test]
fn taking_the_same_mail_twice_credits_the_purse_once() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();
    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::Never,
    )
    .expect("the first take completes");

    let err = apply_take_fence(&mut plane, TAKE + 1, RECIPIENT, mail_id, MONEY)
        .expect_err("there is nothing left in it");

    assert!(
        err.contains(lyracore_shared::mail::NOTHING_TO_TAKE),
        "{err}"
    );
    assert_eq!(shard.purse_of(RECIPIENT), MONEY, "credited once");
}
#[test]
fn a_take_by_a_character_who_is_not_the_recipient_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_fixture();

    let err = apply_take_fence(&mut plane, TAKE, SENDER, mail_id, MONEY)
        .expect_err("the letter is not the sender's to empty");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_MAIL), "{err}");
    assert_eq!(plane.money_in_mailbox(RECIPIENT), MONEY);
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn a_take_killed_after_the_fence_is_held_and_then_re_driven_forward() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();

    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::AfterFence,
    )
    .expect("the fence lands");
    assert_eq!(shard.purse_of(RECIPIENT), 0);
    assert_eq!(plane.fenced_copper(), MONEY);

    plane.advance(MAIL_ESCROW_STALE_MICROS * 100);
    apply_reap(&mut plane);
    assert!(
        plane.has_fence(TAKE),
        "no attestation, so the mail plane never reads 'the purse was not credited'"
    );

    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::Never,
    )
    .expect("the re-drive completes");
    assert_eq!(shard.purse_of(RECIPIENT), MONEY);
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn a_take_killed_after_the_payout_replays_into_one_credit() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();

    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::AfterPayout,
    )
    .expect("the payout lands");
    assert_eq!(shard.purse_of(RECIPIENT), MONEY);
    assert!(plane.has_fence(TAKE), "nothing has attested it yet");

    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::Never,
    )
    .expect("the re-drive completes");

    assert_eq!(shard.purse_of(RECIPIENT), MONEY, "credited once, not twice");
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn a_take_killed_after_the_attestation_is_rolled_forward_by_the_reaper() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();

    drive_take(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        MONEY,
        TakeKilled::AfterConfirm,
    )
    .expect("the attestation lands");
    assert!(plane.has_fence(TAKE));

    plane.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut plane);

    assert!(!plane.has_fence(TAKE), "rolled forward");
    assert_eq!(shard.purse_of(RECIPIENT), MONEY);
    assert_the_take_is_in_exactly_one_place(&shard, &plane, mail_id);
}
#[test]
fn a_payout_to_a_character_with_no_purse_here_leaves_the_fence_holding_the_copper() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();
    shard.purses.borrow_mut().remove(&RECIPIENT);
    apply_take_fence(&mut plane, TAKE, RECIPIENT, mail_id, MONEY).expect("fenced");

    let err =
        apply_payout(&mut shard, TAKE, RECIPIENT, mail_id, MONEY).expect_err("no live entity here");

    assert!(err.contains(lyracore_shared::mail::NOT_IN_WORLD), "{err}");
    assert_eq!(plane.fenced_copper(), MONEY, "still held, not lost");
    assert!(
        apply_settle(&mut plane, TAKE).is_err(),
        "and unattested, so nothing can destroy it"
    );
}
#[test]
fn a_take_driven_for_the_wrong_amount_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_fixture();

    let err = apply_take_fence(&mut plane, TAKE, RECIPIENT, mail_id, MONEY + 1)
        .expect_err("the row does not hold that much");

    assert!(err.contains("refusing to fence an amount"), "{err}");
    assert_eq!(plane.money_in_mailbox(RECIPIENT), MONEY, "untouched");
}
#[test]
fn a_take_escrow_id_reused_for_another_take_is_refused_on_both_sides() {
    let (mut shard, mut plane, mail_id) = delivered_fixture();
    apply_take_fence(&mut plane, TAKE, RECIPIENT, mail_id, MONEY).expect("fenced");
    apply_payout(&mut shard, TAKE, RECIPIENT, mail_id, MONEY).expect("paid");

    let err = apply_take_fence(&mut plane, TAKE, RECIPIENT, mail_id + 1, MONEY)
        .expect_err("the id belongs to another take");
    assert!(err.contains("already fenced for another take"), "{err}");

    let err = apply_payout(&mut shard, TAKE, SENDER, mail_id, MONEY)
        .expect_err("the receipt names another payee");
    assert!(err.contains("already paid out"), "{err}");
    assert_eq!(
        shard.purse_of(SENDER),
        PURSE - COST,
        "and paid them nothing"
    );
}
fn item_fixture() -> (FakeShard, FakeMailPlane) {
    let (shard, plane) = fixture();
    shard.give_item(SENDER, ITEM_GUID, sword());
    (shard, plane)
}
#[test]
fn a_mailed_item_leaves_the_senders_bags_and_arrives_with_its_state_intact() {
    let (mut shard, mut plane) = item_fixture();

    drive(&mut shard, &mut plane, ESCROW, ITEM_GUID, Killed::Never).expect("the drive completes");

    assert!(
        shard.bags_of(SENDER).is_empty(),
        "the item left the sender's bags at SEND, not at take — otherwise a send-and-logout \
         duplicates it"
    );
    assert_eq!(
        plane.items_in_mailbox(RECIPIENT),
        vec![sword()],
        "a damaged, enchanted item must not arrive repaired or stripped — that is a free repair \
         and an enchant-laundering exploit, not a rounding error"
    );
}
#[test]
fn a_soulbound_item_is_refused_at_send_and_stays_in_the_senders_bags() {
    let (mut shard, _plane) = fixture();
    shard.give_item(
        SENDER,
        ITEM_GUID,
        ItemSnapshot {
            soulbound: true,
            ..sword()
        },
    );

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter(), ITEM_GUID, NO_COD_MAIL)
        .expect_err("a bound item is not mailable");

    assert!(
        err.contains(lyracore_shared::mail::ITEM_IS_SOULBOUND),
        "{err}"
    );
    assert_eq!(shard.bags_of(SENDER).len(), 1, "still theirs");
    assert_eq!(shard.purse_of(SENDER), PURSE, "and it cost them nothing");
    assert!(!shard.has_fence(ESCROW));
}
#[test]
fn an_unworn_bind_on_equip_item_is_mailable() {
    let (mut shard, mut plane) = item_fixture();

    drive(&mut shard, &mut plane, ESCROW, ITEM_GUID, Killed::Never).expect("the drive completes");

    assert_eq!(plane.items_in_mailbox(RECIPIENT).len(), 1);
    assert!(
        !plane.items_in_mailbox(RECIPIENT)[0].soulbound,
        "it arrives as unbound as it left"
    );
}
#[test]
fn attaching_an_item_the_sender_does_not_own_fences_nothing() {
    let (mut shard, _plane) = fixture();
    shard.give_item(RECIPIENT, ITEM_GUID, sword());

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter(), ITEM_GUID, NO_COD_MAIL)
        .expect_err("it is not the sender's");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_ITEM), "{err}");
    assert_eq!(shard.bags_of(RECIPIENT).len(), 1, "the owner still has it");
    assert_eq!(shard.purse_of(SENDER), PURSE);
    assert!(!shard.has_fence(ESCROW));
}
#[test]
fn a_fenced_item_cannot_be_attached_to_a_second_letter() {
    let (mut shard, _plane) = item_fixture();
    apply_fence(&mut shard, ESCROW, SENDER, letter(), ITEM_GUID, NO_COD_MAIL).expect("fenced");

    let err = apply_fence(
        &mut shard,
        ESCROW + 1,
        SENDER,
        letter(),
        ITEM_GUID,
        NO_COD_MAIL,
    )
    .expect_err("it is in flight, so it is nobody's");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_ITEM), "{err}");
    assert!(shard.bags_of(SENDER).is_empty());
    assert_eq!(shard.fenced_items(), 1, "held exactly once");
}
#[test]
fn an_item_send_killed_after_the_fence_leaves_it_held_and_never_returned() {
    let (mut shard, mut plane) = item_fixture();

    drive(
        &mut shard,
        &mut plane,
        ESCROW,
        ITEM_GUID,
        Killed::AfterFence,
    )
    .expect("the fence lands");
    assert!(shard.bags_of(SENDER).is_empty());
    assert_eq!(shard.fenced_items(), 1);

    shard.advance(MAIL_ESCROW_STALE_MICROS * 100);
    apply_reap(&mut shard);

    assert!(shard.has_fence(ESCROW), "unattested — held, not returned");
    assert_the_item_is_never_lost_or_duplicated(&shard, &plane, 1);
}
#[test]
fn an_item_send_killed_after_the_commit_replays_forward_into_one_item() {
    let (mut shard, mut plane) = item_fixture();

    drive(
        &mut shard,
        &mut plane,
        ESCROW,
        ITEM_GUID,
        Killed::AfterCommit,
    )
    .expect("committed");
    assert_eq!(plane.items_in_mailbox(RECIPIENT).len(), 1);

    drive(&mut shard, &mut plane, ESCROW, ITEM_GUID, Killed::Never).expect("re-driven");

    assert_eq!(
        plane.items_in_mailbox(RECIPIENT),
        vec![sword()],
        "the re-driven commit found its receipt and wrote no second letter"
    );
    assert!(shard.bags_of(SENDER).is_empty());
    assert!(!shard.has_fence(ESCROW));
}
#[test]
fn an_item_send_killed_after_the_attestation_is_rolled_forward_by_the_reaper() {
    let (mut shard, mut plane) = item_fixture();

    drive(
        &mut shard,
        &mut plane,
        ESCROW,
        ITEM_GUID,
        Killed::AfterConfirm,
    )
    .expect("attested");
    assert!(shard.has_fence(ESCROW));

    shard.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut shard);

    assert!(!shard.has_fence(ESCROW), "rolled forward");
    assert_eq!(plane.items_in_mailbox(RECIPIENT), vec![sword()]);
    assert_the_item_is_never_lost_or_duplicated(&shard, &plane, 1);
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ItemTakeKilled {
    AfterFence,
    AfterGrant,
    AfterConfirm,
    Never,
}
fn delivered_item_fixture() -> (FakeShard, FakeMailPlane, u64) {
    let (mut shard, mut plane) = item_fixture();
    drive(&mut shard, &mut plane, ESCROW, ITEM_GUID, Killed::Never).expect("delivered");
    shard.purses.borrow_mut().insert(RECIPIENT, 0);
    let mail_id = plane.mailbox_of(RECIPIENT)[0].id;
    (shard, plane, mail_id)
}
fn drive_take_item(
    shard: &mut FakeShard,
    plane: &mut FakeMailPlane,
    escrow_id: u64,
    mail_id: u64,
    killed: ItemTakeKilled,
) -> Result<(), String> {
    let items = claimable_items(shard, plane).max(1);
    let check = |shard: &FakeShard, plane: &FakeMailPlane| {
        assert_the_item_is_never_lost_or_duplicated(shard, plane, items);
    };
    check(shard, plane);
    apply_take_item_fence(plane, escrow_id, RECIPIENT, mail_id, sword().entry)?;
    check(shard, plane);
    if killed == ItemTakeKilled::AfterFence {
        return Ok(());
    }
    let item = plane
        .escrow(escrow_id)
        .map(|e| e.item())
        .unwrap_or_default();
    apply_item_payout(shard, escrow_id, RECIPIENT, mail_id, &item)?;
    check(shard, plane);
    if killed == ItemTakeKilled::AfterGrant {
        return Ok(());
    }
    apply_confirm(plane, escrow_id)?;
    check(shard, plane);
    if killed == ItemTakeKilled::AfterConfirm {
        return Ok(());
    }
    apply_settle(plane, escrow_id)?;
    check(shard, plane);
    Ok(())
}
#[test]
fn the_take_moves_a_mails_item_into_the_bags_on_another_database() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();

    drive_take_item(&mut shard, &mut plane, TAKE, mail_id, ItemTakeKilled::Never)
        .expect("the take completes");

    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()]);
    assert!(plane.items_in_mailbox(RECIPIENT).is_empty());
    assert_eq!(
        plane.mailbox_of(RECIPIENT).len(),
        1,
        "a mail emptied of its item is still a letter"
    );
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn a_take_into_a_full_bag_is_refused_and_the_item_is_never_destroyed() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();
    shard.bags_full.set(true);

    let err = drive_take_item(&mut shard, &mut plane, TAKE, mail_id, ItemTakeKilled::Never)
        .expect_err("there is nowhere to put it");

    assert!(err.contains(lyracore_shared::mail::INVENTORY_FULL), "{err}");
    assert!(shard.bags_of(RECIPIENT).is_empty());
    assert_eq!(plane.fenced_items(), 1, "held, not lost");
    assert!(
        apply_settle(&mut plane, TAKE).is_err(),
        "and unattested, so nothing can destroy it"
    );

    shard.bags_full.set(false);
    drive_take_item(&mut shard, &mut plane, TAKE, mail_id, ItemTakeKilled::Never)
        .expect("the re-drive completes once there is room");
    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()]);
}
#[test]
fn taking_the_same_item_twice_grants_it_once() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();
    drive_take_item(&mut shard, &mut plane, TAKE, mail_id, ItemTakeKilled::Never).expect("taken");

    let err = apply_take_item_fence(&mut plane, TAKE + 1, RECIPIENT, mail_id, sword().entry)
        .expect_err("there is nothing left in it");

    assert!(
        err.contains(lyracore_shared::mail::NOTHING_TO_TAKE),
        "{err}"
    );
    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()], "granted once");
}
#[test]
fn an_item_take_by_a_character_who_is_not_the_recipient_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_item_fixture();

    let err = apply_take_item_fence(&mut plane, TAKE, SENDER, mail_id, sword().entry)
        .expect_err("the letter is not the sender's to empty");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_MAIL), "{err}");
    assert_eq!(plane.items_in_mailbox(RECIPIENT), vec![sword()]);
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn an_item_take_killed_after_the_fence_is_held_and_then_re_driven_forward() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();

    drive_take_item(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        ItemTakeKilled::AfterFence,
    )
    .expect("the fence lands");
    assert!(shard.bags_of(RECIPIENT).is_empty());
    assert_eq!(plane.fenced_items(), 1);

    plane.advance(MAIL_ESCROW_STALE_MICROS * 100);
    apply_reap(&mut plane);
    assert!(plane.has_fence(TAKE), "unattested — held");

    drive_take_item(&mut shard, &mut plane, TAKE, mail_id, ItemTakeKilled::Never)
        .expect("the re-drive completes");
    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()]);
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn an_item_take_killed_after_the_grant_replays_into_one_item() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();

    drive_take_item(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        ItemTakeKilled::AfterGrant,
    )
    .expect("the grant lands");
    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()]);
    assert!(plane.has_fence(TAKE), "nothing has attested it yet");

    drive_take_item(&mut shard, &mut plane, TAKE, mail_id, ItemTakeKilled::Never)
        .expect("the re-drive completes");

    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()], "granted once");
    assert!(!plane.has_fence(TAKE));
}
#[test]
fn an_item_take_killed_after_the_attestation_is_rolled_forward_by_the_reaper() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();

    drive_take_item(
        &mut shard,
        &mut plane,
        TAKE,
        mail_id,
        ItemTakeKilled::AfterConfirm,
    )
    .expect("attested");

    plane.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut plane);

    assert!(!plane.has_fence(TAKE), "rolled forward");
    assert_eq!(shard.bags_of(RECIPIENT), vec![sword()]);
    assert_the_item_is_never_lost_or_duplicated(&shard, &plane, 1);
}
#[test]
fn an_item_take_driven_for_the_wrong_item_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_item_fixture();

    let err = apply_take_item_fence(&mut plane, TAKE, RECIPIENT, mail_id, sword().entry + 1)
        .expect_err("the letter does not hold that item");

    assert!(err.contains("refusing to fence an item"), "{err}");
    assert_eq!(
        plane.items_in_mailbox(RECIPIENT),
        vec![sword()],
        "untouched"
    );
}
#[test]
fn an_item_payout_to_a_character_who_is_not_live_here_leaves_the_fence_holding_it() {
    let (mut shard, mut plane, mail_id) = delivered_item_fixture();
    shard.purses.borrow_mut().remove(&RECIPIENT);
    apply_take_item_fence(&mut plane, TAKE, RECIPIENT, mail_id, sword().entry).expect("fenced");

    let err = apply_item_payout(&mut shard, TAKE, RECIPIENT, mail_id, &sword())
        .expect_err("no live entity here");

    assert!(err.contains(lyracore_shared::mail::NOT_IN_WORLD), "{err}");
    assert_eq!(plane.fenced_items(), 1, "still held, not lost");
}
const COD: u32 = 250;
const PAYMENT: u64 = 0x5EED_0003;
fn priced_mail_fixture() -> (FakeShard, FakeMailPlane, u64) {
    let mut plane = FakeMailPlane::default();
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &Letter {
            money: 0,
            cod: COD,
            ..letter()
        },
        &sword(),
        NO_COD_MAIL,
    )
    .expect("the seller's letter is delivered");
    let mail_id = plane.mailbox_of(RECIPIENT)[0].id;
    (FakeShard::with_purse(RECIPIENT, PURSE), plane, mail_id)
}
fn drive_payment(
    shard: &mut FakeShard,
    plane: &mut FakeMailPlane,
    mail_id: u64,
    killed: Killed,
) -> Result<(), String> {
    let payment = Letter {
        recipient_guid: SENDER,
        subject: "COD Payment: Your sword".into(),
        body: String::new(),
        money: COD,
        postage: 0,
        cod: 0,
    };
    if killed == Killed::BeforeFence {
        return Ok(());
    }
    apply_fence(shard, PAYMENT, RECIPIENT, payment.clone(), NO_ITEM, mail_id)?;
    if killed == Killed::AfterFence {
        return Ok(());
    }
    apply_commit(
        plane,
        PAYMENT,
        RECIPIENT,
        &payment,
        &ItemSnapshot::default(),
        mail_id,
    )?;
    if killed == Killed::AfterCommit {
        return Ok(());
    }
    apply_confirm(shard, PAYMENT)?;
    if killed == Killed::AfterConfirm {
        return Ok(());
    }
    apply_settle(shard, PAYMENT)
}
fn assert_the_payment_is_in_exactly_one_place(shard: &FakeShard, plane: &FakeMailPlane) {
    let purse = shard.purse_of(RECIPIENT);
    assert!(
        purse == PURSE || purse == PURSE - COD,
        "the buyer's purse went somewhere it should not ({purse}): the payment debits it once and \
         nothing credits it back"
    );
    let paid = plane.money_in_mailbox(SENDER);
    assert!(
        paid == 0 || paid == COD,
        "the seller was paid {paid}, which is neither nothing nor the price — a replayed commit \
         must write one payment"
    );
    if purse == PURSE - COD {
        assert!(
            shard.fenced_copper() == COD || paid == COD,
            "the buyer has paid, the fence is gone and the seller has nothing — the copper is \
             nowhere, which is what delete-last exists to make unreachable"
        );
    }
}
#[test]
fn a_cod_payment_debits_the_buyer_pays_the_seller_and_settles_the_price() {
    let (mut shard, mut plane, mail_id) = priced_mail_fixture();

    drive_payment(&mut shard, &mut plane, mail_id, Killed::Never).expect("the payment completes");

    assert_eq!(shard.purse_of(RECIPIENT), PURSE - COD, "debited once");
    let seller = plane.mailbox_of(SENDER);
    assert_eq!(seller.len(), 1, "one payment mail");
    assert_eq!(seller[0].money, COD, "carrying the price");
    assert_eq!(seller[0].sender_guid, RECIPIENT, "from the buyer");
    assert_eq!(
        plane.mailbox_of(RECIPIENT)[0].cod,
        0,
        "and the price is no longer owed — a second click charges nothing"
    );
    assert!(!shard.has_fence(PAYMENT), "settled");
    assert_the_payment_is_in_exactly_one_place(&shard, &plane);
}
#[test]
fn a_cod_payment_killed_at_any_step_re_drives_into_one_charge_and_one_payout() {
    for killed in [
        Killed::AfterFence,
        Killed::AfterCommit,
        Killed::AfterConfirm,
    ] {
        let (mut shard, mut plane, mail_id) = priced_mail_fixture();

        drive_payment(&mut shard, &mut plane, mail_id, killed).expect("the drive reaches its end");
        assert_the_payment_is_in_exactly_one_place(&shard, &plane);
        assert_eq!(
            shard.purse_of(RECIPIENT),
            PURSE - COD,
            "{killed:?}: past the fence the buyer has PAID, and nothing refunds a fence"
        );

        drive_payment(&mut shard, &mut plane, mail_id, Killed::Never)
            .unwrap_or_else(|e| panic!("{killed:?}: the re-drive completes: {e}"));

        assert_eq!(
            shard.purse_of(RECIPIENT),
            PURSE - COD,
            "{killed:?}: charged once"
        );
        assert_eq!(
            plane.mailbox_of(SENDER).len(),
            1,
            "{killed:?}: paid once — the replayed commit found its receipt"
        );
        assert_eq!(plane.mailbox_of(RECIPIENT)[0].cod, 0, "{killed:?}: settled");
        assert!(
            !shard.has_fence(PAYMENT),
            "{killed:?}: and the fence is gone"
        );
    }
}
#[test]
fn a_cod_payment_that_never_committed_leaves_the_price_owed_and_the_item_in_the_letter() {
    let (mut shard, mut plane, mail_id) = priced_mail_fixture();

    drive_payment(&mut shard, &mut plane, mail_id, Killed::AfterFence).expect("the fence lands");

    assert_eq!(shard.fenced_copper(), COD, "held, not lost");
    assert!(
        plane.mailbox_of(SENDER).is_empty(),
        "the seller has nothing yet"
    );
    assert_eq!(plane.mailbox_of(RECIPIENT)[0].cod, COD, "still owed");
    assert_eq!(plane.items_in_mailbox(RECIPIENT), vec![sword()]);
}
#[test]
fn a_buyer_who_cannot_afford_the_price_is_refused_and_nothing_moves() {
    let (mut plane, mail_id) = {
        let (_, plane, mail_id) = priced_mail_fixture();
        (plane, mail_id)
    };
    let mut shard = FakeShard::with_purse(RECIPIENT, COD - 1);

    let err = drive_payment(&mut shard, &mut plane, mail_id, Killed::Never)
        .expect_err("the purse cannot cover the price");

    assert!(
        err.contains(lyracore_shared::mail::NOT_ENOUGH_MONEY),
        "{err}"
    );
    assert_eq!(shard.purse_of(RECIPIENT), COD - 1, "charged nothing");
    assert_eq!(shard.fenced_copper(), 0, "and nothing fenced");
    assert_eq!(plane.mailbox_of(RECIPIENT)[0].cod, COD, "still owed");
    assert_eq!(plane.items_in_mailbox(RECIPIENT), vec![sword()]);
}
