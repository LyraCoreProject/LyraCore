//! Where the mail-escrow protocol is **executed** rather than scanned — two in-memory databases,
//! the real step bodies, and a driver that can be killed between any two of them.
//!
//! # Why it exists
//!
//! `ReducerContext` cannot be constructed in a unit test, so a step body written straight against
//! it can only be pinned by its own TEXT. `transfer/harness.rs` records what that is worth: 17 of
//! 21 mutations against that surface left the suite green. Every step here is written against a
//! sink instead, and what runs below is [`apply_fence`], [`apply_commit`], [`apply_confirm`],
//! [`apply_settle`] and [`apply_reap`] themselves.
//!
//! # The two databases
//!
//! [`FakeShard`] is the player's own shard: the purse, an escrow ledger a letter fences into, and
//! the receipts a payout credits through. [`FakeMailPlane`] is realm-core: mail rows, delivery
//! receipts, and its own ledger, because a TAKE fences where the copper is. Neither can read the
//! other, which is the whole point — the harness is what makes "unconsultable" a property the tests
//! can enforce rather than a claim in a comment.
//!
//! # The instrument: value is in exactly one place
//!
//! [`assert_value_is_in_exactly_one_place`] runs after every step of every drive. It is three
//! assertions — the sender is never credited back, the letter is never delivered twice, and the
//! value is never nowhere — and it is what actually fails when an ordering is wrong. The individual
//! tests then name WHICH interruption they are walking.
//!
//! What this cannot model, exactly as `transfer/harness.rs` cannot: SpacetimeDB's transaction
//! rollback. A real `Err` unwinds every write the reducer made; a fake keeps them. So every refusal
//! test below asserts on the ROW that licenses the next step, never on "nothing was written".

use super::*;
use crate::mail::ItemSnapshot;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

// -------------------------------------------------------------------------------------------
//  The escrow ledger, which BOTH databases have
// -------------------------------------------------------------------------------------------

/// The escrow row's columns, kept as their own struct so the fake never needs [`MailEscrow`] to be
/// `Clone`. It mirrors every column: the step bodies read the letter back out of the row (that is
/// what makes a stalled escrow re-drivable), so a fake that dropped the letter would make the
/// forward-recovery tests meaningless.
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
}

/// `game_mail_escrow` on ONE database. Its own struct because both fakes hold one: a letter fences
/// on the sender's shard and a take fences on the mail plane, and the settle, the attestation and
/// the reaper are the same code either way.
#[derive(Default)]
struct FakeLedger {
    escrows: RefCell<HashMap<u64, XEscrow>>,
    now: Cell<i64>,
    reaper_armed: Cell<bool>,
}

impl FakeLedger {
    /// The copper this ledger is holding — what has left one place and not yet reached the other.
    fn fenced_copper(&self) -> u32 {
        self.escrows
            .borrow()
            .values()
            .map(|e| e.money.saturating_add(e.postage))
            .sum()
    }
    /// How many fences here are holding an item — the item's answer to `fenced_copper`, and the
    /// term the "never in zero places" assertion needs for an attachment.
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

// -------------------------------------------------------------------------------------------
//  The sender's (and taker's) shard
// -------------------------------------------------------------------------------------------

/// One "database" on the player's side: the purse, the escrow ledger a letter fences into, and the
/// payout receipts a take credits through. `RefCell`, not `Mutex`: a re-entrant access PANICS (a
/// named test failure) instead of deadlocking, the same choice `transfer::harness::FakeDb` makes.
#[derive(Default)]
pub struct FakeShard {
    /// `game_world_entity.money` per guid. A guid with no entry has no live entity here, which is
    /// the module's own answer for character select and for a character mid-transfer.
    purses: RefCell<HashMap<u64, u32>>,
    /// Payout receipts: `escrow_id -> (mail_id, payee)`. The take direction's idempotency key, and
    /// it lives HERE because this is where the purse it protects is.
    receipts: RefCell<HashMap<u64, (u64, u64)>>,
    /// `game_item_instance` here: `guid -> (owner, state)`. Deleting from this map is what the
    /// fence does, and it is the whole "a fenced item is reachable by nothing" property — every
    /// other item path in the module resolves a guid through this same table.
    items: RefCell<HashMap<u64, (u64, ItemSnapshot)>>,
    /// The bags have no room. `false` (the default) is the ordinary case; a test flips it to walk
    /// the full-bag refusal, which is where a naive take destroys the item.
    bags_full: Cell<bool>,
    /// Mints a guid for an item this shard creates, standing in for `items::next_item_guid`.
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
    /// Every item `owner` holds here, as its snapshot state. The assertion surface for "it left the
    /// bags", "it arrived unchanged" and "it never arrived twice".
    fn bags_of(&self, owner: u64) -> Vec<ItemSnapshot> {
        let mut held: Vec<ItemSnapshot> = self
            .items
            .borrow()
            .values()
            .filter(|(o, _)| *o == owner)
            .map(|(_, i)| i.clone())
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
    /// The production shim calls `mail::detach_item`, which reads the same
    /// [`crate::mail::plan_attach`] verdict this does and then DELETES the row. The delete is
    /// modelled faithfully because it is the property under test.
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
            // No live entity here — the module's own answer for character select and mid-hop.
            None => false,
        }
    }
    /// `items::store_instance_state`'s shape: one new row carrying the recorded state, or the
    /// item module's own full-bag refusal.
    fn grant_item(&mut self, payee_guid: u64, item: &ItemSnapshot) -> Result<(), String> {
        if !self.purses.borrow().contains_key(&payee_guid) {
            return Err(lyracore_shared::mail::NOT_IN_WORLD.to_string());
        }
        if self.bags_full.get() {
            return Err(lyracore_shared::mail::INVENTORY_FULL.to_string());
        }
        let guid = self.next_item_guid.get() + 1;
        self.next_item_guid.set(guid);
        self.items
            .borrow_mut()
            .insert(guid, (payee_guid, item.clone()));
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

// -------------------------------------------------------------------------------------------
//  The mail plane
// -------------------------------------------------------------------------------------------

/// One delivered mail row, in the columns the escrow decides.
#[derive(Clone, Debug, PartialEq, Eq)]
struct XMail {
    id: u64,
    recipient_guid: u64,
    sender_guid: u64,
    subject: String,
    body: String,
    money: u32,
    item: ItemSnapshot,
}

/// realm-core: the authoritative mail rows, the delivery receipts that make a commit idempotent,
/// and its own escrow ledger — a TAKE fences here, because here is where the copper is. It holds no
/// purse, because the real one holds no characters.
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
    /// The attachments sitting in this mailbox — the term "the item is still in the letter" is
    /// asserted through.
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
            .map(|m| (m.recipient_guid, m.item.clone()))
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
        // The real sink calls `mail::insert_mail`, whose `id` is `auto_inc`; the fake mints the
        // same way, so a second delivery is a second ROW rather than an overwrite — which is what
        // makes a duplicated letter visible here at all.
        let id = self.next_mail_id.get() + 1;
        self.next_mail_id.set(id);
        self.mails.borrow_mut().push(XMail {
            id,
            recipient_guid: letter.recipient_guid,
            sender_guid,
            subject: letter.subject.clone(),
            body: letter.body.clone(),
            money: letter.money,
            item: item.clone(),
        });
        id
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

// -------------------------------------------------------------------------------------------
//  The fixture and the drive
// -------------------------------------------------------------------------------------------

const ESCROW: u64 = 0x5EED_0001;
const SENDER: u64 = 11;
const RECIPIENT: u64 = 22;
const PURSE: u32 = 500;
const MONEY: u32 = 100;
const POSTAGE: u32 = 30;
/// What leaves the purse for this fixture's letter.
const COST: u32 = MONEY + POSTAGE;
/// The sender's item instance guid, and the "no attachment" sentinel the wire sends for a letter
/// that carries none.
const ITEM_GUID: u64 = 0x4000_0000_0000_0011;
const NO_ITEM: u64 = 0;

/// A DAMAGED, ENCHANTED weapon — every column mailing must not launder, set to something a
/// template-based re-grant would get wrong. Reserved entry, per the fixture-id rule.
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
    }
}

/// The step a driver dies immediately after — the transaction committed, the next call never went
/// out. `Never` is the drive that runs to the end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Killed {
    BeforeFence,
    AfterFence,
    AfterCommit,
    AfterConfirm,
    Never,
}

/// The gateway's drive, as the module sees it: fence on the sender's shard, commit on the mail
/// plane, attest, settle. The `killed` argument is the crash point — everything after it simply
/// never happens, which is exactly what a dead gateway looks like from both databases.
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
    apply_fence(shard, escrow_id, SENDER, letter(), item_guid)?;
    check(shard, plane);
    if killed == Killed::AfterFence {
        return Ok(());
    }
    // The attachment rides from the fence to the mail plane through the DRIVER, re-read off the
    // escrow row — the same derivation `world::mail::redrive` makes, so a re-drive and a fresh
    // drive commit the identical letter.
    let item = shard
        .escrow(escrow_id)
        .map(|e| e.item())
        .unwrap_or_default();
    apply_commit(plane, escrow_id, SENDER, &letter(), &item)?;
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

/// **The invariant, checked after every step of every drive.** Three claims, and each one is a
/// different way the move could go wrong:
///
/// 1. the sender is never credited back — the purse only ever goes down, so no interruption can
///    duplicate the coin by refunding one that also arrived;
/// 2. the letter is delivered at most once — a replayed commit must not write a second mail row;
/// 3. the value is never NOWHERE — before the mail row exists the fence holds it, and the fence is
///    only destroyed after the mail row is attested.
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

/// How many copies of the attachment somebody could USE right now — in a set of bags, or sitting in
/// a mailbox waiting to be taken. A fenced item is deliberately NOT counted: it is in nobody's
/// reach, which is the whole point of a fence.
fn claimable_items(shard: &FakeShard, plane: &FakeMailPlane) -> usize {
    shard.bags_of(SENDER).len()
        + shard.bags_of(RECIPIENT).len()
        + plane.items_in_mailbox(RECIPIENT).len()
}

/// **The attachment's invariant, checked after every step of every drive.** An item is the value
/// that cannot be recreated if the protocol loses it, so both halves are asserted:
///
/// 1. **never duplicated** — at most `expected` claimable copies, so no interruption can leave it
///    in two places somebody could spend it from;
/// 2. **never nowhere** — claimable plus fenced never falls below `expected`, which is delete-last
///    stated in items.
///
/// What is deliberately NOT asserted is that the fence and the mailbox never hold it at once.
/// Between the commit and its settle they both do, and that overlap IS delete-last — the same
/// window the copper's `assert_the_take_is_in_exactly_one_place` documents.
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

// -------------------------------------------------------------------------------------------
//  The happy path
// -------------------------------------------------------------------------------------------

/// The whole sequence across two databases: the coin leaves the purse, arrives in the mailbox
/// exactly once, and the fence is gone.
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

/// The fence arms the reaper on its way through — recovery that is never scheduled is no recovery
/// at all, and `init` cannot arm an already-published database.
#[test]
fn the_first_fence_arms_the_reaper() {
    let (mut shard, _plane) = fixture();
    assert!(!shard.ledger.reaper_armed.get());

    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM).expect("fenced");

    assert!(shard.ledger.reaper_armed.get());
}

// -------------------------------------------------------------------------------------------
//  Interruption at every point
// -------------------------------------------------------------------------------------------

/// **Killed after the fence.** The coin is out of the purse and in the ledger, no mail row exists,
/// and the reaper HOLDS however long it waits — the mail plane cannot be consulted from here, so
/// "I see no attestation" is not evidence the letter failed.
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

/// **Killed after the commit.** The mail row exists, the fence is still standing, and the drive is
/// re-run from the top with the SAME escrow id: the commit replays into a no-op and the letter is
/// delivered once.
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

/// **Killed after the attestation.** The mail row is durable and the source knows it, so the reaper
/// can finish the job by itself — no gateway needed.
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

/// **Killed after the settle**, i.e. after everything. A replayed settle finds no fence and says so
/// rather than failing, so a driver that never learned its last call landed can simply repeat it.
#[test]
fn a_settle_replayed_after_the_drive_completed_is_a_no_op() {
    let (mut shard, mut plane) = fixture();
    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::Never).expect("the drive completes");

    apply_settle(&mut shard, ESCROW).expect("a replayed settle is not an error");

    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
}

/// **Killed before anything.** Named so the enumeration of interruption points is complete in the
/// test file rather than only in a comment: nothing happened, and nothing is owed.
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

// -------------------------------------------------------------------------------------------
//  Replay and collision
// -------------------------------------------------------------------------------------------

/// A fence replayed under the same id debits the purse ONCE. This is the retry a driver makes when
/// it crashed without learning whether its call landed.
#[test]
fn a_replayed_fence_debits_the_purse_once() {
    let (mut shard, _plane) = fixture();

    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM).expect("first");
    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM)
        .expect("replay is a no-op, not an error");

    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
    assert_eq!(shard.fenced_copper(), COST);
}

/// An escrow id already fenced for a DIFFERENT sender is refused. Answering "already done" would
/// tell the driver its fence landed, and it would go on to settle a stranger's.
#[test]
fn an_escrow_id_reused_for_another_sender_is_refused() {
    let (mut shard, _plane) = fixture();
    shard.purses.borrow_mut().insert(99, PURSE);
    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM).expect("first");

    let err = apply_fence(&mut shard, ESCROW, 99, letter(), NO_ITEM).expect_err("the id is taken");

    assert!(err.contains("already fenced"), "{err}");
    assert_eq!(shard.purse_of(99), PURSE, "the second sender paid nothing");
}

/// A commit replayed under the same id writes no second mail row — the property the whole
/// caller-chosen id exists for.
#[test]
fn a_replayed_commit_produces_one_mail_and_not_two() {
    let (_shard, mut plane) = fixture();

    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
    )
    .expect("first");
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
    )
    .expect("replay");
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
    )
    .expect("replay again");

    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
}

/// The same id fenced again for a different amount is refused, and the first letter's value is
/// left exactly as it was — a retry carries the same letter, so this is a second one.
#[test]
fn re_fencing_an_id_for_a_different_amount_is_refused() {
    let (mut shard, _plane) = fixture();
    apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM).expect("first");

    let err = apply_fence(
        &mut shard,
        ESCROW,
        SENDER,
        Letter {
            money: MONEY * 2,
            ..letter()
        },
        NO_ITEM,
    )
    .expect_err("a different letter under the same id");

    assert!(err.contains("different amount"), "{err}");
    assert_eq!(shard.purse_of(SENDER), PURSE - COST, "debited once");
    assert_eq!(shard.fenced_copper(), COST);
}

/// An escrow id whose receipt names ANOTHER recipient is refused on the mail plane. Answering
/// "already delivered" would tell the driver its letter arrived and let it settle a fence whose
/// value went nowhere — the loss the destination-side collision guard exists for.
#[test]
fn an_escrow_id_that_already_delivered_to_another_recipient_is_refused() {
    let (_shard, mut plane) = fixture();
    apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &letter(),
        &ItemSnapshot::default(),
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
    )
    .expect_err("the id belongs to another letter");

    assert!(err.contains("another recipient"), "{err}");
    assert!(plane.mailbox_of(RECIPIENT + 1).is_empty());
}

/// Escrow id 0 is the "no escrow" sentinel on both sides. A drive that passed it would file a fence
/// nothing could address and a receipt every other letter would replay into.
#[test]
fn escrow_id_zero_is_reserved_on_both_planes() {
    let (mut shard, mut plane) = fixture();

    apply_fence(&mut shard, 0, SENDER, letter(), NO_ITEM).expect_err("reserved");
    apply_commit(&mut plane, 0, SENDER, &letter(), &ItemSnapshot::default()).expect_err("reserved");

    assert_eq!(shard.purse_of(SENDER), PURSE);
    assert!(plane.mailbox_of(RECIPIENT).is_empty());
}

// -------------------------------------------------------------------------------------------
//  The refusals
// -------------------------------------------------------------------------------------------

/// A sender who cannot afford the letter is refused before any write — the guarantee that a
/// rejected send costs nothing.
#[test]
fn an_unaffordable_letter_fences_nothing() {
    let mut shard = FakeShard::with_purse(SENDER, COST - 1);

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter(), NO_ITEM).expect_err("cannot pay");

    assert!(
        err.contains(lyracore_shared::mail::NOT_ENOUGH_MONEY),
        "{err}"
    );
    assert_eq!(shard.purse_of(SENDER), COST - 1);
    assert!(!shard.has_fence(ESCROW));
}

/// **Delete-last, executed.** The settle refuses while the delivery is unattested, so the fence
/// survives the call that would have destroyed it.
#[test]
fn a_settle_before_the_attestation_is_refused_and_the_fence_survives() {
    let (mut shard, mut plane) = fixture();
    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::AfterCommit).expect("committed");

    let err = apply_settle(&mut shard, ESCROW).expect_err("not attested");

    assert!(err.contains("not attested"), "{err}");
    assert!(shard.has_fence(ESCROW), "the value is still held");
    assert_eq!(shard.fenced_copper(), COST);
}

/// An attestation for an id nothing is fenced under is refused rather than silently accepted: it
/// would be an attestation about somebody else's letter, and the only thing it licenses is
/// destroying value.
#[test]
fn an_attestation_for_an_unknown_escrow_is_refused() {
    let (mut shard, _plane) = fixture();

    let err = apply_confirm(&mut shard, ESCROW).expect_err("nothing is fenced");

    assert!(err.contains("nothing fenced"), "{err}");
}

/// A settle driven against the wrong database finds no fence and answers `Ok` — the gateway fans
/// calls at shards, and "there is nothing here" is a normal outcome, not a failure. The value is
/// untouched either way.
#[test]
fn a_settle_that_reaches_a_database_holding_no_fence_is_not_an_error() {
    let mut elsewhere = FakeShard::default();

    apply_settle(&mut elsewhere, ESCROW).expect("nothing to do here");
}

// -------------------------------------------------------------------------------------------
//  The reaper
// -------------------------------------------------------------------------------------------

/// The reaper leaves a fence that is still young alone: it is the net for a CRASHED driver, never a
/// race against a slow one.
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

/// Several escrows at once: each is judged on its own evidence. The attested one settles, the
/// unattested one is held — a single verdict for the whole table would be either a dupe or a
/// permanent freeze.
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
    )
    .expect("a second, unattested fence");

    shard.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut shard);

    assert!(!shard.has_fence(ESCROW), "attested — rolled forward");
    assert!(shard.has_fence(held), "unattested — held");
}

// =============================================================================================
//  The TAKE: the same mechanism, driven the other way
// =============================================================================================
//
// The fence is on the MAIL plane now (the copper is in a mail row, not a purse) and the destination
// is the taker's own shard. Steps 3 and 4 are the SAME `apply_confirm` / `apply_settle`, run against
// the mail plane's ledger — which is the whole reason the send direction was built as a mechanism.

/// The take's crash points, named the way [`Killed`] names the send's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TakeKilled {
    AfterFence,
    AfterPayout,
    AfterConfirm,
    Never,
}

/// The escrow id a take drives under. Distinct from [`ESCROW`] so a test can hold a letter and a
/// take in flight at once.
const TAKE: u64 = 0x5EED_0002;

/// Deliver one letter and settle it, so the mail plane holds a row with [`MONEY`] in it and the
/// recipient has a purse to take it into. The starting state of every take test.
fn delivered_fixture() -> (FakeShard, FakeMailPlane, u64) {
    let (mut shard, mut plane) = fixture();
    drive(&mut shard, &mut plane, ESCROW, NO_ITEM, Killed::Never).expect("the letter is delivered");
    shard.purses.borrow_mut().insert(RECIPIENT, 0);
    let mail_id = plane.mailbox_of(RECIPIENT)[0].id;
    (shard, plane, mail_id)
}

/// The gateway's take drive: fence on the mail plane, pay out on the taker's shard, attest, settle.
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

/// **The take's invariant**, checked after every step. [`assert_value_is_in_exactly_one_place`]'s
/// three claims, pointed the other way:
///
/// 1. the purse is credited at most once — no interruption can pay a take twice;
/// 2. the mail row is never re-filled, so a take cannot be undone into a second helping;
/// 3. the copper is never NOWHERE — while the row is empty and the purse uncredited, the mail
///    plane's fence is holding it.
///
/// Note what is NOT asserted: that the purse and the fence never hold it at once. Between the
/// payout and the settle they both do, and that overlap is delete-last itself — the same window the
/// send direction has between the commit and its settle.
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

/// The whole take across two databases: the row is emptied, the purse is credited once, and the
/// letter survives.
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

/// **Taking twice credits once.** The second take finds a row with nothing in it — the clear and
/// the fence were one transaction, so there is no window where both takes see the copper.
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

/// **A take by anyone who is not the recipient is refused**, and the mail keeps its copper. The
/// mail id is client-supplied, so this is the authorization boundary and not a sanity check.
#[test]
fn a_take_by_a_character_who_is_not_the_recipient_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_fixture();

    let err = apply_take_fence(&mut plane, TAKE, SENDER, mail_id, MONEY)
        .expect_err("the letter is not the sender's to empty");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_MAIL), "{err}");
    assert_eq!(plane.money_in_mailbox(RECIPIENT), MONEY);
    assert!(!plane.has_fence(TAKE));
}

/// **Killed after the take fence.** The copper is out of the row and held on the mail plane; the
/// reaper HOLDS it (the purse is on a database it cannot consult), and a re-drive finishes the job.
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

/// **Killed after the payout.** The purse is credited and the fence still stands; the re-drive's
/// payout replays into a no-op, so the purse is credited once.
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

/// **Killed after the attestation.** The mail plane knows the purse holds the copper, so its own
/// reaper finishes the take with no gateway at all — the same `apply_reap` the send direction uses.
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

/// A payout to a character who is not live on this database is refused, and the fence keeps the
/// copper — a taker who logged out or hopped mid-drive gets paid on the next re-drive, not never.
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

/// The gateway carries the amount from one database to the other, so a stale read is the one way
/// the two could disagree. The fence refuses rather than fencing less than the payout will credit.
#[test]
fn a_take_driven_for_the_wrong_amount_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_fixture();

    let err = apply_take_fence(&mut plane, TAKE, RECIPIENT, mail_id, MONEY + 1)
        .expect_err("the row does not hold that much");

    assert!(err.contains("refusing to fence an amount"), "{err}");
    assert_eq!(plane.money_in_mailbox(RECIPIENT), MONEY, "untouched");
}

/// An escrow id already driving another take is refused on both sides: the fence would empty a
/// second mail under one key, and the payout would credit a second purse from one receipt.
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

// =============================================================================================
//  The ITEM: the same mechanism, carrying the value that cannot be recreated
// =============================================================================================
//
// Coin can be minted back; an item cannot. So these walk the same four steps and the same crash
// points, and the instrument is `assert_the_item_has_exactly_one_owner` — which every drive above
// already runs, because the send drive is the same function.

/// A sender who is holding the fixture sword, and a mail plane with nothing on it.
fn item_fixture() -> (FakeShard, FakeMailPlane) {
    let (shard, plane) = fixture();
    shard.give_item(SENDER, ITEM_GUID, sword());
    (shard, plane)
}

/// **The whole send, with the item.** It leaves the bags, it arrives once, and every column that
/// makes it worth something arrives with it.
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

/// **A soulbound instance is refused at send and stays in the bags.** The one refusal that is
/// about the ITEM rather than the sender, and it fires before anything is written.
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

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter(), ITEM_GUID)
        .expect_err("a bound item is not mailable");

    assert!(
        err.contains(lyracore_shared::mail::ITEM_IS_SOULBOUND),
        "{err}"
    );
    assert_eq!(shard.bags_of(SENDER).len(), 1, "still theirs");
    assert_eq!(shard.purse_of(SENDER), PURSE, "and it cost them nothing");
    assert!(!shard.has_fence(ESCROW));
}

/// **An unworn bind-on-equip item IS mailable.** The verdict reads the instance's bind state, not
/// the template's bonding, so the unworn drop a player passes to an alt goes through.
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

/// **Attaching an item the sender does not own is refused**, and so is one that does not exist.
/// The item guid is client-supplied, so this is the authorization boundary on a send.
#[test]
fn attaching_an_item_the_sender_does_not_own_fences_nothing() {
    let (mut shard, _plane) = fixture();
    shard.give_item(RECIPIENT, ITEM_GUID, sword());

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter(), ITEM_GUID)
        .expect_err("it is not the sender's");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_ITEM), "{err}");
    assert_eq!(shard.bags_of(RECIPIENT).len(), 1, "the owner still has it");
    assert_eq!(shard.purse_of(SENDER), PURSE);
    assert!(!shard.has_fence(ESCROW));
}

/// **A fenced item is reachable by nothing.** The instance row is gone, so the second letter's
/// attach finds no item — and every other path that reads a player's items resolves through that
/// same table, which is why no vendor, equip, use or trade path had to learn an in-flight rule.
#[test]
fn a_fenced_item_cannot_be_attached_to_a_second_letter() {
    let (mut shard, _plane) = item_fixture();
    apply_fence(&mut shard, ESCROW, SENDER, letter(), ITEM_GUID).expect("fenced");

    let err = apply_fence(&mut shard, ESCROW + 1, SENDER, letter(), ITEM_GUID)
        .expect_err("it is in flight, so it is nobody's");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_ITEM), "{err}");
    assert!(shard.bags_of(SENDER).is_empty());
    assert_eq!(shard.fenced_items(), 1, "held exactly once");
}

/// **Killed after the item fence.** The item is in the ledger and nowhere else; the reaper holds
/// it forever rather than handing it back against a commit that may have landed.
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

/// **Killed after the item commit.** The mail row holds it, the fence still stands, and the
/// re-drive replays the commit into its receipt — one item in the mailbox, not two.
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

/// **Killed after the attestation.** The mail row is durable and the source knows it, so the
/// reaper finishes the item's move with no gateway at all.
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

// ---------------------------------------------------------------------------------------------
//  Taking the item back out
// ---------------------------------------------------------------------------------------------

/// The crash points of an item take, named the way [`TakeKilled`] names the money take's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ItemTakeKilled {
    AfterFence,
    AfterGrant,
    AfterConfirm,
    Never,
}

/// Deliver one letter carrying the sword and settle it, so the mail plane holds a row with an
/// attachment and the recipient has a shard to take it onto.
fn delivered_item_fixture() -> (FakeShard, FakeMailPlane, u64) {
    let (mut shard, mut plane) = item_fixture();
    drive(&mut shard, &mut plane, ESCROW, ITEM_GUID, Killed::Never).expect("delivered");
    shard.purses.borrow_mut().insert(RECIPIENT, 0);
    let mail_id = plane.mailbox_of(RECIPIENT)[0].id;
    (shard, plane, mail_id)
}

/// The gateway's item-take drive: fence on the mail plane, grant on the taker's shard, attest,
/// settle. Steps 3 and 4 are the SAME `apply_confirm` / `apply_settle` both other directions use.
fn drive_take_item(
    shard: &mut FakeShard,
    plane: &mut FakeMailPlane,
    escrow_id: u64,
    mail_id: u64,
    killed: ItemTakeKilled,
) -> Result<(), String> {
    // What is claimable when this take starts is what must be claimable when it ends, however it
    // is interrupted: one sword, in one place at a time.
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

/// **The whole take across two databases.** The item lands in the bags carrying the state it was
/// sent with, and the letter survives with nothing in it.
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

/// **A take into a full bag does not destroy the item.** The grant refuses, the fence keeps it,
/// and the next re-drive lands it once there is room — the case a naive implementation loses.
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

/// **Taking the same item twice grants it once.** The clear and the fence are one transaction, so
/// the second take finds a letter with nothing in it.
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

/// **A take by anyone who is not the recipient is refused**, and the item stays in the letter. The
/// mail id is client-supplied, so this is the authorization boundary and not a sanity check.
#[test]
fn an_item_take_by_a_character_who_is_not_the_recipient_is_refused() {
    let (_shard, mut plane, mail_id) = delivered_item_fixture();

    let err = apply_take_item_fence(&mut plane, TAKE, SENDER, mail_id, sword().entry)
        .expect_err("the letter is not the sender's to empty");

    assert!(err.contains(lyracore_shared::mail::NOT_YOUR_MAIL), "{err}");
    assert_eq!(plane.items_in_mailbox(RECIPIENT), vec![sword()]);
    assert!(!plane.has_fence(TAKE));
}

/// **Killed after the take fence.** The item is out of the letter and held on the mail plane; the
/// reaper cannot judge it, and the re-drive finishes the job.
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

/// **Killed after the grant.** The bags hold it and the fence still stands; the re-drive's grant
/// replays into its receipt, so the item is created once and not twice.
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

/// **Killed after the attestation.** The mail plane knows the bags hold it, so its own reaper
/// finishes the take with no gateway at all.
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

/// The gateway carries the entry from one database to the other, so a stale read is the one way
/// the two could disagree. The fence refuses rather than fencing an item the grant would not match.
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

/// A grant to a character who is not live on this database is refused and the fence keeps the
/// item — a taker who logged out or hopped mid-drive gets it on the next re-drive, not never.
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
