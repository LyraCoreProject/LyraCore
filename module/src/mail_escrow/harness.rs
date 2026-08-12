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
//! [`FakeShard`] is the SENDER's shard: the purse and the escrow ledger. [`FakeMailPlane`] is
//! realm-core: mail rows and delivery receipts. Neither can read the other, which is the whole
//! point — the harness is what makes "unconsultable" a property the tests can enforce rather than a
//! claim in a comment.
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
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

// -------------------------------------------------------------------------------------------
//  The sender's shard
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
}

/// One "database" on the sender's side. `RefCell`, not `Mutex`: a re-entrant access PANICS (a named
/// test failure) instead of deadlocking, the same choice `transfer::harness::FakeDb` makes.
#[derive(Default)]
pub struct FakeShard {
    /// `game_world_entity.money` per guid. A guid with no entry has no live entity here, which is
    /// the module's own answer for character select and for a character mid-transfer.
    purses: RefCell<HashMap<u64, u32>>,
    escrows: RefCell<HashMap<u64, XEscrow>>,
    now: Cell<i64>,
    reaper_armed: Cell<bool>,
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
    /// The copper the ledger is holding for this letter — what the sender has paid and nobody has
    /// yet received.
    fn fenced_copper(&self) -> u32 {
        self.escrows
            .borrow()
            .values()
            .map(|e| e.money.saturating_add(e.postage))
            .sum()
    }
    fn has_fence(&self, escrow_id: u64) -> bool {
        self.escrows.borrow().contains_key(&escrow_id)
    }
    fn advance(&self, micros: i64) {
        self.now.set(self.now.get() + micros);
    }
}

impl EscrowLedger for FakeShard {
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
        })
    }
    fn file_escrow(&mut self, row: MailEscrow) {
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
    fn now_micros(&self) -> i64 {
        self.now.get()
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
    fn arm_reaper(&mut self) {
        self.reaper_armed.set(true);
    }
}

impl ReapSink for FakeShard {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        self.escrows
            .borrow()
            .iter()
            .map(|(id, e)| (*id, e.sender_guid, e.created_micros, e.delivered))
            .collect()
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
}

/// realm-core: the authoritative mail rows plus the delivery receipts that make a commit
/// idempotent. It holds no purse and no escrow, because the real one holds no characters.
#[derive(Default)]
pub struct FakeMailPlane {
    mails: RefCell<Vec<XMail>>,
    receipts: RefCell<HashMap<u64, (u64, u64)>>,
    next_mail_id: Cell<u64>,
    now: Cell<i64>,
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
    fn deliver(&mut self, sender_guid: u64, letter: &Letter) -> u64 {
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
        });
        id
    }
    fn file_receipt(&mut self, row: MailDelivery) {
        self.receipts
            .borrow_mut()
            .insert(row.escrow_id, (row.mail_id, row.recipient_guid));
    }
    fn now_micros(&self) -> i64 {
        self.now.get()
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
    killed: Killed,
) -> Result<(), String> {
    assert_value_is_in_exactly_one_place(shard, plane);
    if killed == Killed::BeforeFence {
        return Ok(());
    }
    apply_fence(shard, escrow_id, SENDER, letter())?;
    assert_value_is_in_exactly_one_place(shard, plane);
    if killed == Killed::AfterFence {
        return Ok(());
    }
    apply_commit(plane, escrow_id, SENDER, &letter())?;
    assert_value_is_in_exactly_one_place(shard, plane);
    if killed == Killed::AfterCommit {
        return Ok(());
    }
    apply_confirm(shard, escrow_id)?;
    assert_value_is_in_exactly_one_place(shard, plane);
    if killed == Killed::AfterConfirm {
        return Ok(());
    }
    apply_settle(shard, escrow_id)?;
    assert_value_is_in_exactly_one_place(shard, plane);
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

    drive(&mut shard, &mut plane, ESCROW, Killed::Never).expect("the drive completes");

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
    assert!(!shard.reaper_armed.get());

    apply_fence(&mut shard, ESCROW, SENDER, letter()).expect("fenced");

    assert!(shard.reaper_armed.get());
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

    drive(&mut shard, &mut plane, ESCROW, Killed::AfterFence).expect("the fence lands");

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

    drive(&mut shard, &mut plane, ESCROW, Killed::AfterCommit).expect("the commit lands");
    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
    assert!(shard.has_fence(ESCROW), "nothing has attested the delivery");

    drive(&mut shard, &mut plane, ESCROW, Killed::Never).expect("the re-drive completes");

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

    drive(&mut shard, &mut plane, ESCROW, Killed::AfterConfirm).expect("the attestation lands");
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
    drive(&mut shard, &mut plane, ESCROW, Killed::Never).expect("the drive completes");

    apply_settle(&mut shard, ESCROW).expect("a replayed settle is not an error");

    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
}

/// **Killed before anything.** Named so the enumeration of interruption points is complete in the
/// test file rather than only in a comment: nothing happened, and nothing is owed.
#[test]
fn a_driver_killed_before_the_fence_costs_the_sender_nothing() {
    let (mut shard, mut plane) = fixture();

    drive(&mut shard, &mut plane, ESCROW, Killed::BeforeFence).expect("nothing happened");

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

    apply_fence(&mut shard, ESCROW, SENDER, letter()).expect("first");
    apply_fence(&mut shard, ESCROW, SENDER, letter()).expect("replay is a no-op, not an error");

    assert_eq!(shard.purse_of(SENDER), PURSE - COST);
    assert_eq!(shard.fenced_copper(), COST);
}

/// An escrow id already fenced for a DIFFERENT sender is refused. Answering "already done" would
/// tell the driver its fence landed, and it would go on to settle a stranger's.
#[test]
fn an_escrow_id_reused_for_another_sender_is_refused() {
    let (mut shard, _plane) = fixture();
    shard.purses.borrow_mut().insert(99, PURSE);
    apply_fence(&mut shard, ESCROW, SENDER, letter()).expect("first");

    let err = apply_fence(&mut shard, ESCROW, 99, letter()).expect_err("the id is taken");

    assert!(err.contains("already fenced"), "{err}");
    assert_eq!(shard.purse_of(99), PURSE, "the second sender paid nothing");
}

/// A commit replayed under the same id writes no second mail row — the property the whole
/// caller-chosen id exists for.
#[test]
fn a_replayed_commit_produces_one_mail_and_not_two() {
    let (_shard, mut plane) = fixture();

    apply_commit(&mut plane, ESCROW, SENDER, &letter()).expect("first");
    apply_commit(&mut plane, ESCROW, SENDER, &letter()).expect("replay");
    apply_commit(&mut plane, ESCROW, SENDER, &letter()).expect("replay again");

    assert_eq!(plane.mailbox_of(RECIPIENT).len(), 1);
}

/// The same id fenced again for a different amount is refused, and the first letter's value is
/// left exactly as it was — a retry carries the same letter, so this is a second one.
#[test]
fn re_fencing_an_id_for_a_different_amount_is_refused() {
    let (mut shard, _plane) = fixture();
    apply_fence(&mut shard, ESCROW, SENDER, letter()).expect("first");

    let err = apply_fence(
        &mut shard,
        ESCROW,
        SENDER,
        Letter {
            money: MONEY * 2,
            ..letter()
        },
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
    apply_commit(&mut plane, ESCROW, SENDER, &letter()).expect("first letter");

    let err = apply_commit(
        &mut plane,
        ESCROW,
        SENDER,
        &Letter {
            recipient_guid: RECIPIENT + 1,
            ..letter()
        },
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

    apply_fence(&mut shard, 0, SENDER, letter()).expect_err("reserved");
    apply_commit(&mut plane, 0, SENDER, &letter()).expect_err("reserved");

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

    let err = apply_fence(&mut shard, ESCROW, SENDER, letter()).expect_err("cannot pay");

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
    drive(&mut shard, &mut plane, ESCROW, Killed::AfterCommit).expect("committed");

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
    drive(&mut shard, &mut plane, ESCROW, Killed::AfterConfirm).expect("attested");

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
    drive(&mut shard, &mut plane, ESCROW, Killed::AfterConfirm).expect("attested");
    apply_fence(
        &mut shard,
        held,
        SENDER,
        Letter {
            money: 0,
            postage: POSTAGE,
            ..letter()
        },
    )
    .expect("a second, unattested fence");

    shard.advance(MAIL_ESCROW_STALE_MICROS * 2);
    apply_reap(&mut shard);

    assert!(!shard.has_fence(ESCROW), "attested — rolled forward");
    assert!(shard.has_fence(held), "unattested — held");
}
