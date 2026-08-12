//! Escrowed mail attachments — moving value across a database boundary that no transaction spans.
//!
//! # The problem, stated once
//!
//! A mail row is authoritative on **realm-core**; the sender's purse and item rows are on the
//! sender's own shard. Two databases, no transaction across them, and value has to cross. Every
//! naive ordering has a window: destroy the sender's copper first and a crash destroys value,
//! write the mail first and a crash duplicates it. Duplication is the worse half — it is the
//! economy-breaking one — so the whole design is built to make it unreachable and to accept a
//! FROZEN attachment as the failure mode instead.
//!
//! # The protocol
//!
//! ```text
//!   SENDER's shard (the purse)                     MAIL plane (realm-core)
//!   realm_mail_fence      ─── the letter ────►
//!     value leaves the purse INTO the escrow row
//!                                                  realm_mail_commit
//!                                                    the mail row + a delivery receipt
//!                        ◄── "it committed" ───
//!   realm_mail_confirm_delivery   (the attestation, durable HERE)
//!   realm_mail_settle             (destroy the fence — delete-LAST)
//! ```
//!
//! This is [`crate::transfer`]'s escrow with the nouns changed, deliberately so: that subsystem
//! solved this exact problem for characters and its comments record which orderings were rejected.
//! Three rules carry over unchanged.
//!
//! 1. **Delete-last.** [`apply_settle`] is the only step that destroys the fence, and it REFUSES
//!    without the attestation that the mail row is durable. So no reachable state has the value in
//!    zero places.
//! 2. **Caller-chosen idempotency key.** `escrow_id` is the gateway's, not `auto_inc`, and it is
//!    the key on BOTH sides: the fence row on the source and the delivery receipt on the mail
//!    plane. A replayed commit therefore produces one mail and not two.
//! 3. **The destination is UNCONSULTABLE from the source.** A local read cannot answer "did the
//!    mail arrive". `transfer`'s `cross_database` doc carries the lesson: reading "I cannot see it"
//!    as "it did not arrive" rolls back past the point of no return and DUPLICATES the value. So
//!    [`Recovery`] has no rollback arm at all — see there.
//!
//! # What the fence is, and why it is a MOVE rather than a flag
//!
//! Fencing takes the value OUT of the purse (and, when #147 lands, deletes the item row into a
//! snapshot on the escrow row) in the same transaction that files the escrow. Nothing is left
//! behind to spend, equip, sell, trade or mail again, and no call site anywhere in the module has
//! to learn a new rule: every existing "you do not have that" arm fires by itself.
//!
//! The rejected alternative is a `fenced: bool` on the purse/item row. It reads as the smaller
//! change and is the larger one — every vendor, trade, equip, loot and mail path would have to
//! learn the flag, and the ONE that forgot would be the dupe.
//!
//! # The single-database plane does not come here
//!
//! On a `lyracore dev up` gateway the purse and the mail row are on one database, so
//! `mail::apply_send` does the whole send in ONE transaction and files no escrow row. That is not a
//! shortcut to be tidied away later: an escrow is strictly worse there (four transactions where one
//! is atomic, plus a fence that can be interrupted where nothing could be). Pinned by
//! `mail::tests::the_single_database_send_path_never_reaches_the_escrow`.
//!
//! # Scope
//!
//! The mechanism only. Copper is what crosses today — the postage debit #145 shipped still runs on
//! its own path, see [`crate::mail::apply_send`] — and the attached-coin (#146) and item (#147)
//! slices are what put real value through here. The item snapshot columns are deliberately NOT
//! pre-declared: they are numeric and bool, so #147 END-appends them with typed defaults, which is
//! a clean additive migration. [server]

use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::game_world_entity;
use crate::helpers::require_operator;

#[cfg(test)]
mod harness;

// ===========================================================================================
//  Policy constants
// ===========================================================================================

/// How long a fence may sit without progressing before the reaper treats the driver as dead. The
/// same 30s `transfer` uses and for the same reason: the reaper is the net for a CRASHED driver,
/// never a race against a slow one.
pub(crate) const MAIL_ESCROW_STALE_MICROS: i64 = 30_000_000; // 30s

/// [`reap_mail_escrows`] cadence. A stalled fence is a player's coin (later, their item) sitting in
/// limbo, so recovery latency is felt.
const MAIL_ESCROW_REAP_INTERVAL_MICROS: u64 = 5_000_000; // 5s

// ===========================================================================================
//  Ledger tables
// ===========================================================================================

/// SOURCE-side fence: "this value has left the sender's purse and is on its way into a mail row on
/// the other database". Its existence IS the in-flight flag, and the row carries the WHOLE letter —
/// so a stalled escrow can be re-driven forward from itself, which is the only recovery direction
/// this design allows.
///
/// NOT public and NOT gateway-subscribed, the `game_transfer_out` precedent: no client and no relay
/// reads escrow rows. The gateway drives the protocol by CALLING the reducers below; it never needs
/// to read the ledger to do that. [server]
#[table(
    accessor = game_mail_escrow,
    index(accessor = by_sender, btree(columns = [sender_guid]))
)]
pub struct MailEscrow {
    /// Caller-chosen (the gateway's) escrow id — NOT `auto_inc`. It is the idempotency key shared
    /// with the mail plane, so it must be stable across a retry; 0 is reserved as the "no escrow"
    /// sentinel, exactly as `transfer_id` 0 is.
    #[primary_key]
    pub escrow_id: u64,
    pub sender_guid: u64,
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    /// Copper travelling WITH the letter — it lands in the mail row's `money` and the recipient
    /// takes it out. 0 for a text-only letter.
    pub money: u32,
    /// Copper the post office keeps. Fenced with the rest (one debit, never two) and destroyed by
    /// the settle, because postage buys the delivery rather than travelling in it.
    pub postage: u32,
    pub created_micros: i64,
    /// **The attestation: the mail row is committed on the other database.** Written by
    /// [`realm_mail_confirm_delivery`] once the gateway — the one component that can see both
    /// databases — has seen the commit return. [`apply_settle`] refuses without it, which is the
    /// whole of delete-last, and the reaper rolls an escrow forward only when it is set.
    ///
    /// A separate table (`transfer`'s `game_transfer_in`) would carry the same fact; a column does
    /// it in one row with one lifetime, and there is no blob here that a second row would need to
    /// hold — the escrow row already IS the letter.
    pub delivered: bool,
}

/// MAIL-plane receipt: "the mail row for this escrow id exists, and here it is". Keyed by the
/// caller-chosen escrow id, so a replayed [`realm_mail_commit`] finds it and writes nothing.
///
/// Kept FOREVER (bar the recipient's deletion sweep below), deliberately. Clearing it would need a
/// fifth step, and the only thing that step could buy is disk — while getting it wrong (deleting a
/// receipt whose fence is still unsettled) re-opens the duplicate a replayed commit would then
/// write. A permanent receipt is the conservative side of that trade. [server]
#[table(
    accessor = game_mail_delivery,
    index(accessor = by_recipient, btree(columns = [recipient_guid]))
)]
pub struct MailDelivery {
    #[primary_key]
    pub escrow_id: u64,
    /// The `game_mail` row this escrow became.
    pub mail_id: u64,
    pub recipient_guid: u64,
    pub created_micros: i64,
}

/// Drives [`reap_mail_escrows`]. Armed lazily by the first fence — `seed.rs`'s `init` only runs on
/// a FRESH database, so a schedule row added there would never arm an already-published one. [server]
#[table(accessor = game_mail_escrow_reaper_schedule, scheduled(reap_mail_escrows))]
pub struct MailEscrowReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

// A deleted sender must not leave a fence behind: it would hold value for a guid that no longer
// exists and, after guid reuse, for a stranger.
crate::character_owned!(delete, fn sweep_delete_game_mail_escrow(ctx, character_guid) {
    let escrows = ctx.db.game_mail_escrow();
    for r in escrows.by_sender().filter(&character_guid).collect::<Vec<_>>() {
        escrows.escrow_id().delete(r.escrow_id);
    }
});
// CROSS-DATABASE transport: NOT transported. The fence is a fact about the DATABASE that took the
// value out of a purse, not about the character — carrying it to the destination shard would move
// the claim away from the ledger that has to settle it, and the drive addresses the database that
// filed it. A character who hops mid-flight leaves the fence where its value came from; the reaper
// there still settles it, and the mail row it is waiting on is on realm-core either way.
crate::character_owned!(not_transported, fn sweep_transfer_game_mail_escrow());

// The receipt goes with the recipient's mailbox: `mail.rs`'s delete sweep destroys their mail rows,
// so the receipts naming those rows have nothing left to protect from a replay.
crate::character_owned!(delete, fn sweep_delete_game_mail_delivery(ctx, character_guid) {
    let receipts = ctx.db.game_mail_delivery();
    for r in receipts.by_recipient().filter(&character_guid).collect::<Vec<_>>() {
        receipts.escrow_id().delete(r.escrow_id);
    }
});
// CROSS-DATABASE transport: NOT transported. Receipts only exist on the plane that holds the
// authoritative mail rows, and a character never transfers off realm-core. On the single-database
// fallback none is ever written, because that plane does the whole send in one transaction.
crate::character_owned!(not_transported, fn sweep_transfer_game_mail_delivery());

// ===========================================================================================
//  Pure core — the entire decision surface of the protocol
// ===========================================================================================

/// The letter a fence carries, and the same values the commit writes into the mail row. Grouped so
/// the step bodies do not each take eight arguments, and so the fence and the commit provably talk
/// about the same letter.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Letter {
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    /// Copper that travels with the letter.
    pub money: u32,
    /// Copper the post office keeps.
    pub postage: u32,
}

impl Letter {
    /// What leaves the purse: the coin that travels plus the coin the post office keeps, in ONE
    /// debit. Saturating, so a crafted `money` near `u32::MAX` cannot wrap the total down to
    /// something the sender can afford — the same rule, and the same reason, as
    /// [`lyracore_shared::mail::total_cost`].
    pub(crate) fn fenced_copper(&self) -> u32 {
        self.money.saturating_add(self.postage)
    }
}

/// What [`apply_fence`] must do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FencePlan {
    /// This escrow id is already fenced FOR THIS SENDER — a retry, do nothing. The purse must not
    /// be debited twice for one letter.
    Replay,
    /// This escrow id is fenced for a DIFFERENT sender. Refuse — see [`plan_fence`].
    IdCollision,
    /// Same sender, same id, DIFFERENT value. Refuse — see [`plan_fence`].
    ValueChanged,
    /// The sender has no live entity on this database, so there is no purse to fence out of.
    NoPurse,
    /// The purse cannot cover the letter. Refuse before any write, so a rejected send costs
    /// nothing.
    NotEnoughMoney,
    /// Take the value out of the purse and file the escrow, in one transaction.
    Fence,
}

/// `fenced` is the `(sender, fenced copper)` of the escrow row filed under THIS id, or `None` when
/// the id is unused.
///
/// Both refusals exist because `escrow_id` is CALLER-chosen, so both collisions are reachable by
/// inputs:
///
/// - **`IdCollision`.** Answering `Replay` on an id reused for a different sender tells that driver
///   "your fence already landed" about a letter that was never fenced. It then commits the mail and
///   settles — destroying SOMEBODY ELSE's fence for a letter nobody paid for.
/// - **`ValueChanged`.** A retry carries the same letter by definition; a different amount under
///   the same id is a different letter, and answering `Replay` would let the commit deliver coin
///   the fence is not holding. Refusing makes the divergence loud instead of silent.
pub(crate) fn plan_fence(
    fenced: Option<(u64, u32)>,
    sender_guid: u64,
    purse: Option<u32>,
    cost: u32,
) -> FencePlan {
    if let Some((fenced_sender, fenced_copper)) = fenced {
        if fenced_sender != sender_guid {
            return FencePlan::IdCollision;
        }
        return if fenced_copper == cost {
            FencePlan::Replay
        } else {
            FencePlan::ValueChanged
        };
    }
    let Some(purse) = purse else {
        return FencePlan::NoPurse;
    };
    if purse < cost {
        return FencePlan::NotEnoughMoney;
    }
    FencePlan::Fence
}

/// What [`apply_commit`] must do, on the mail plane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CommitPlan {
    /// A receipt already names a mail row for this escrow id, addressed to the same recipient — the
    /// replay a driver that crashed without learning whether its call landed simply retries into.
    /// Writing a second mail row here is the duplication this whole subsystem exists to make
    /// unreachable.
    Replay,
    /// A receipt exists but names a DIFFERENT recipient: the id was reused for another letter.
    /// Refuse — answering `Replay` would tell the driver its letter was delivered when the mail
    /// row belongs to somebody else's, and it would then settle a fence whose value never went
    /// anywhere. The destination-side twin of [`FencePlan::IdCollision`].
    IdCollision,
    /// Write the mail row and its receipt, in one transaction.
    Deliver,
}

pub(crate) fn plan_commit(receipted_for: Option<u64>, recipient_guid: u64) -> CommitPlan {
    match receipted_for {
        None => CommitPlan::Deliver,
        Some(r) if r == recipient_guid => CommitPlan::Replay,
        Some(_) => CommitPlan::IdCollision,
    }
}

/// What [`apply_settle`] must do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SettlePlan {
    /// No fence under this id — a retry after a successful settle, or a call that reached the wrong
    /// database.
    AlreadyDone,
    /// Fenced but not attested. REFUSE: this is the only step that destroys the fence, and the
    /// value it holds is the only copy left of what the sender paid.
    NotDelivered,
    /// Destroy the fence. The attested mail row is what licenses it.
    Complete,
}

pub(crate) fn plan_settle(has_escrow: bool, delivered: bool) -> SettlePlan {
    match (has_escrow, delivered) {
        (false, _) => SettlePlan::AlreadyDone,
        (true, false) => SettlePlan::NotDelivered,
        (true, true) => SettlePlan::Complete,
    }
}

/// What the SOURCE can know about the mail plane, and the sharpest statement of this design.
///
/// There is deliberately no `NotDelivered` variant. The mail row is on another database: a local
/// read that finds no attestation has learned "not yet attested", never "not delivered". Minting a
/// `NotDelivered` from that read is exactly the mistake `transfer`'s `cross_database` doc records —
/// it rolls the escrow back past the point of no return and hands the sender back coin that is also
/// sitting in the recipient's mailbox.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeliveryEvidence {
    /// [`realm_mail_confirm_delivery`] ran: the mail row is durable on the other database.
    Attested,
    /// Nothing local says either way — which is every unattested escrow, always.
    Unknown,
}

/// What the reaper must do about a fence whose driver never came back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Recovery {
    /// Do nothing — not stale yet, or the mail plane cannot be consulted. A held fence is
    /// recoverable: the escrow row carries the whole letter, so the drive can be replayed forward
    /// (the commit is idempotent on the escrow id) once something that can see both databases comes
    /// back. That is the ONLY direction recovery runs in.
    Hold,
    /// The mail row is attested durable: finish the move the driver abandoned.
    Settle,
}

/// The recovery rule, and the most load-bearing function in this file.
///
/// Note what it cannot express: refunding a fence. A refund is only ever safe with proof that the
/// mail row does NOT exist, and the source cannot obtain that proof — so the source never refunds,
/// full stop. The party that can hold such proof is the gateway (it saw the commit refuse), and
/// wiring that judgement belongs with the slice that has real value to hand back; until then a
/// stalled fence is held, logged, and readable with `spacetime sql`.
pub(crate) fn recovery(evidence: DeliveryEvidence, age_micros: i64) -> Recovery {
    if age_micros < MAIL_ESCROW_STALE_MICROS {
        return Recovery::Hold;
    }
    match evidence {
        DeliveryEvidence::Attested => Recovery::Settle,
        DeliveryEvidence::Unknown => Recovery::Hold,
    }
}

/// The evidence an escrow row's own columns amount to. One line, but it is the line where a future
/// edit could invent a `NotDelivered` out of a missing attestation, so it is named and tested.
pub(crate) fn evidence_of(delivered: bool) -> DeliveryEvidence {
    if delivered {
        DeliveryEvidence::Attested
    } else {
        DeliveryEvidence::Unknown
    }
}

// ===========================================================================================
//  The SINKS — each step's database, reduced to exactly what that step touches
// ===========================================================================================

/// The escrow ledger on the SENDER's database. Supertrait of the two step sinks, so a row one step
/// writes means the same thing to the next.
///
/// `ReducerContext` cannot be constructed in a unit test, so without this seam the only thing a
/// test could do to a step body is read its text — the lesson `transfer/harness.rs` records after
/// 17 of 21 mutations survived that surface. [`CtxDb`] is the one production adapter and
/// `harness::FakeShard` is the test one, and the harness executes the REAL step bodies.
pub(crate) trait EscrowLedger {
    fn escrow(&self, escrow_id: u64) -> Option<MailEscrow>;
    fn file_escrow(&mut self, row: MailEscrow);
    /// Set `delivered` on the escrow row. A no-op when there is no row.
    fn attest_delivery(&mut self, escrow_id: u64);
    fn delete_escrow(&mut self, escrow_id: u64);
    fn now_micros(&self) -> i64;
}

/// What the FENCE step touches, beyond the ledger.
pub(crate) trait FenceSink: EscrowLedger {
    /// The sender's spendable copper here — `game_world_entity.money`, the live purse
    /// `trainer::buy` and `mail::charge_postage` read. `None` when the sender has no live entity on
    /// this database (character select, or mid-transfer).
    fn purse(&self, sender_guid: u64) -> Option<u32>;
    /// Take `amount` out of that purse. Called only after [`plan_fence`] said it is affordable.
    fn debit_purse(&mut self, sender_guid: u64, amount: u32);
    /// Arm the reaper schedule if it is not armed yet.
    fn arm_reaper(&mut self);
}

/// What the REAPER touches: the ledger, plus the census of fences to consider.
pub(crate) trait ReapSink: EscrowLedger {
    /// Every escrow row on this database, as `(escrow_id, sender_guid, created_micros, delivered)`.
    /// Collected up front because the loop mutates the table it walks.
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)>;
}

/// What the COMMIT step touches, on the MAIL plane. A different database entirely: it has no purse,
/// no escrow row and no way to ask the source anything.
pub(crate) trait DeliverySink {
    /// The receipt filed under this escrow id, if any — the idempotency key.
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery>;
    /// Write the mail row, through the same insert every other mail write takes, and answer its id.
    fn deliver(&mut self, sender_guid: u64, letter: &Letter) -> u64;
    fn file_receipt(&mut self, row: MailDelivery);
    fn now_micros(&self) -> i64;
}

/// The ONE production adapter: the real `ReducerContext`, wearing every sink at once.
///
/// **This layer is the seam's own blind spot.** The harness substitutes a fake for every line of
/// it, so a no-op'd method here is invisible to every test in the crate, and cargo-mutants cannot
/// help — it can only ask whether a test FAILS, and no test can execute a `ReducerContext` at all
/// (`transfer/tests.rs` measured 54 missed mutants across the same layer). So it stays pinned by
/// exact-shape equality in `tests::the_production_adapter_is_the_pass_through_the_harness_assumes`.
/// Every method here must stay a pass-through; the moment one grows a decision, the harness
/// underneath stops meaning what it claims.
struct CtxDb<'a> {
    ctx: &'a ReducerContext,
}

impl EscrowLedger for CtxDb<'_> {
    fn escrow(&self, escrow_id: u64) -> Option<MailEscrow> {
        self.ctx.db.game_mail_escrow().escrow_id().find(escrow_id)
    }
    fn file_escrow(&mut self, row: MailEscrow) {
        self.ctx.db.game_mail_escrow().insert(row);
    }
    fn attest_delivery(&mut self, escrow_id: u64) {
        if let Some(row) = self.escrow(escrow_id) {
            self.ctx
                .db
                .game_mail_escrow()
                .escrow_id()
                .update(MailEscrow {
                    delivered: true,
                    ..row
                });
        }
    }
    fn delete_escrow(&mut self, escrow_id: u64) {
        self.ctx.db.game_mail_escrow().escrow_id().delete(escrow_id);
    }
    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
}

impl FenceSink for CtxDb<'_> {
    fn purse(&self, sender_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, sender_guid).map(|e| e.money)
    }
    fn debit_purse(&mut self, sender_guid: u64, amount: u32) {
        if let Some(mut e) = crate::helpers::acting_entity_by_guid(self.ctx, sender_guid) {
            e.money = e.money.saturating_sub(amount);
            self.ctx.db.game_world_entity().guid().update(e);
        }
    }
    fn arm_reaper(&mut self) {
        let sched = self.ctx.db.game_mail_escrow_reaper_schedule();
        if sched.iter().next().is_none() {
            sched.insert(MailEscrowReaperSchedule {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                    MAIL_ESCROW_REAP_INTERVAL_MICROS as i64,
                )),
            });
        }
    }
}

impl ReapSink for CtxDb<'_> {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        self.ctx
            .db
            .game_mail_escrow()
            .iter()
            .map(|e| (e.escrow_id, e.sender_guid, e.created_micros, e.delivered))
            .collect()
    }
}

impl DeliverySink for CtxDb<'_> {
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery> {
        self.ctx.db.game_mail_delivery().escrow_id().find(escrow_id)
    }
    fn deliver(&mut self, sender_guid: u64, letter: &Letter) -> u64 {
        crate::mail::insert_mail(
            self.ctx,
            letter.recipient_guid,
            sender_guid,
            letter.subject.clone(),
            letter.body.clone(),
            letter.money,
        )
    }
    fn file_receipt(&mut self, row: MailDelivery) {
        self.ctx.db.game_mail_delivery().insert(row);
    }
    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
}

// ===========================================================================================
//  The steps — each written against a sink, each executed for real by `harness`
// ===========================================================================================

/// **Step 1, on the SENDER's shard — take the value out of the purse and fence it.**
///
/// One transaction: after it commits the copper is spendable nowhere, and the escrow row holds
/// everything the mail plane needs. Idempotent on `escrow_id`, so a driver that crashed without
/// learning whether its call landed simply calls again.
pub(crate) fn apply_fence<S: FenceSink>(
    sink: &mut S,
    escrow_id: u64,
    sender_guid: u64,
    letter: Letter,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    let cost = letter.fenced_copper();
    match plan_fence(
        sink.escrow(escrow_id)
            .map(|e| (e.sender_guid, e.money.saturating_add(e.postage))),
        sender_guid,
        sink.purse(sender_guid),
        cost,
    ) {
        FencePlan::Replay => return Ok(()),
        FencePlan::IdCollision => {
            return Err(format!(
                "mail escrow {escrow_id} is already fenced for another sender — refusing to reuse \
                 the id for {sender_guid}"
            ))
        }
        FencePlan::ValueChanged => {
            return Err(format!(
                "mail escrow {escrow_id} is already fenced for a different amount — refusing to \
                 reuse the id for a second letter"
            ))
        }
        FencePlan::NoPurse => return Err(lyracore_shared::mail::NOT_IN_WORLD.to_string()),
        FencePlan::NotEnoughMoney => {
            return Err(lyracore_shared::mail::NOT_ENOUGH_MONEY.to_string())
        }
        FencePlan::Fence => {}
    }

    // Debit BEFORE filing the row, in the one transaction that does both: the escrow row is the
    // receipt for value that has already left, so a row filed against a debit that did not happen
    // would license a commit the sender never paid for.
    sink.debit_purse(sender_guid, cost);
    let created_micros = sink.now_micros();
    sink.file_escrow(MailEscrow {
        escrow_id,
        sender_guid,
        recipient_guid: letter.recipient_guid,
        subject: letter.subject,
        body: letter.body,
        money: letter.money,
        postage: letter.postage,
        created_micros,
        delivered: false,
    });
    sink.arm_reaper();
    log::info!("mail escrow {escrow_id}: fenced {cost} copper from sender {sender_guid}");
    Ok(())
}

/// **Step 2, on the MAIL plane — write the mail row, idempotently.**
///
/// The escrow evidence is the CALL, not a local row: the fence is on another database and this one
/// has never heard of it. That is not a weakening — the reducer is operator-gated, so it is exactly
/// as forgeable as any other mail write.
pub(crate) fn apply_commit<S: DeliverySink>(
    sink: &mut S,
    escrow_id: u64,
    sender_guid: u64,
    letter: &Letter,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    match plan_commit(
        sink.receipt(escrow_id).map(|r| r.recipient_guid),
        letter.recipient_guid,
    ) {
        CommitPlan::Replay => return Ok(()),
        CommitPlan::IdCollision => {
            return Err(format!(
                "mail escrow {escrow_id} already delivered a letter to another recipient — \
                 refusing to reuse the id"
            ))
        }
        CommitPlan::Deliver => {}
    }
    let mail_id = sink.deliver(sender_guid, letter);
    let created_micros = sink.now_micros();
    // The receipt goes in the SAME transaction as the mail row. Split, a crash between them makes
    // the mail row invisible to the next replay and the letter arrives twice.
    sink.file_receipt(MailDelivery {
        escrow_id,
        mail_id,
        recipient_guid: letter.recipient_guid,
        created_micros,
    });
    log::info!(
        "mail escrow {escrow_id}: committed as mail {mail_id} for recipient {}",
        letter.recipient_guid
    );
    Ok(())
}

/// **Step 3, back on the SENDER's shard — the driver attests that the mail row is durable.**
///
/// The obligation this moves out of the module: nothing here can check that the commit really
/// happened. Attesting before a successful commit licenses destroying the fence with no mail row —
/// the one unrecoverable outcome. That ordering is the gateway's, exactly as `confirm_import`'s is,
/// and the module's guard is the other half: an UNATTESTED fence can never be destroyed or refunded
/// (see [`recovery`]), so a driver that dies before attesting leaves recoverable value rather than
/// lost value.
pub(crate) fn apply_confirm<S: EscrowLedger>(sink: &mut S, escrow_id: u64) -> Result<(), String> {
    let Some(row) = sink.escrow(escrow_id) else {
        return Err(format!(
            "mail escrow {escrow_id}: nothing fenced here to attest a delivery for"
        ));
    };
    if row.delivered {
        return Ok(()); // already attested — replay-safe like every other step
    }
    sink.attest_delivery(escrow_id);
    log::info!("mail escrow {escrow_id}: delivery attested; the fence may now be settled");
    Ok(())
}

/// **Step 4 — delete-last.** Destroys the fence, and the value with it: the coin that travelled now
/// lives in the mail row and the postage is spent. REFUSES while unattested — that guard is what
/// makes "the value is nowhere" unreachable.
pub(crate) fn apply_settle<S: EscrowLedger>(sink: &mut S, escrow_id: u64) -> Result<(), String> {
    let row = sink.escrow(escrow_id);
    match plan_settle(row.is_some(), row.as_ref().is_some_and(|r| r.delivered)) {
        // Says so rather than answering a silent `Ok`: the gateway reads success as "the fence is
        // gone", and a settle driven against the wrong database must not be indistinguishable from
        // one that did the work. Expected, not an error — hence `info`.
        SettlePlan::AlreadyDone => {
            log::info!(
                "mail escrow {escrow_id}: no fence on this database, nothing to settle (already \
                 settled here, or this call reached the wrong shard)"
            );
            Ok(())
        }
        SettlePlan::NotDelivered => Err(format!(
            "mail escrow {escrow_id}: delivery not attested — refusing to destroy the fence"
        )),
        SettlePlan::Complete => {
            sink.delete_escrow(escrow_id);
            log::info!("mail escrow {escrow_id}: settled — the letter carries the value now");
            Ok(())
        }
    }
}

/// The reaper: finishes what an abandoned driver started, and holds everything it cannot judge.
pub(crate) fn apply_reap<S: ReapSink>(sink: &mut S) {
    let now = sink.now_micros();
    for (escrow_id, sender_guid, created_micros, delivered) in sink.escrows() {
        let age = now - created_micros;
        match recovery(evidence_of(delivered), age) {
            Recovery::Hold => {
                if age >= MAIL_ESCROW_STALE_MICROS {
                    // Loud, and repeated every pass on purpose: this is value in limbo, and the
                    // only party that can resolve it is one that sees both databases. Held
                    // escrows are ~always zero, so the line costs nothing until it matters.
                    log::warn!(
                        "reap_mail_escrows: {escrow_id} (sender {sender_guid}) has been fenced for \
                         {age}us with no delivery attestation — HELD. The mail plane is \
                         unconsultable from here, so this is never read as \"it did not arrive\"."
                    );
                }
            }
            Recovery::Settle => {
                sink.delete_escrow(escrow_id);
                log::warn!(
                    "reap_mail_escrows: {escrow_id} abandoned after delivery — settled, sender \
                     {sender_guid}'s fence released"
                );
            }
        }
    }
}

// ===========================================================================================
//  Reducers — each one an operator gate plus its `apply_*` body
// ===========================================================================================

/// Fence a letter's value on the sender's own shard. Step 1.
///
/// **Operator-gated, and it has to be** — the whole `realm_mail_*` family's trust boundary. It
/// takes the payer's guid as an argument, so without the gate any connection that can reach the
/// node empties a stranger's purse.
#[reducer]
// A reducer's arguments are the wire; the letter cannot be one struct here. `begin_transfer` says
// the same thing about the same shape.
#[allow(clippy::too_many_arguments)]
pub fn realm_mail_fence(
    ctx: &ReducerContext,
    escrow_id: u64,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    postage: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    apply_fence(
        &mut CtxDb { ctx },
        escrow_id,
        sender_guid,
        Letter {
            recipient_guid,
            subject,
            body,
            money,
            postage,
        },
    )
}

/// Commit a fenced letter into a mail row on the mail plane. Step 2.
///
/// Operator-gated for the same reason `realm_mail_send` is: every gate that decides who may write
/// to whom ran in the gateway, because realm-core holds no characters, so `sender_guid` here is not
/// a claim this transaction can check.
#[reducer]
pub fn realm_mail_commit(
    ctx: &ReducerContext,
    escrow_id: u64,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    apply_commit(
        &mut CtxDb { ctx },
        escrow_id,
        sender_guid,
        // `postage` is not passed: what the post office keeps never reaches the mail plane, it is
        // burned by the settle on the shard that fenced it.
        &Letter {
            recipient_guid,
            subject,
            body,
            money,
            postage: 0,
        },
    )
}

/// Attest that the mail row is durable. Step 3, on the sender's shard. Operator-gated — an
/// attestation is a licence to destroy value, so it is exactly as trusted as the fence itself.
#[reducer]
pub fn realm_mail_confirm_delivery(ctx: &ReducerContext, escrow_id: u64) -> Result<(), String> {
    require_operator(ctx)?;
    apply_confirm(&mut CtxDb { ctx }, escrow_id)
}

/// Destroy the fence. Step 4, on the sender's shard. Operator-gated, same boundary.
#[reducer]
pub fn realm_mail_settle(ctx: &ReducerContext, escrow_id: u64) -> Result<(), String> {
    require_operator(ctx)?;
    apply_settle(&mut CtxDb { ctx }, escrow_id)
}

/// The reaper. Scheduler-identity gated rather than operator-gated, like every other scheduled
/// reducer: it is called by the database itself.
#[reducer]
pub fn reap_mail_escrows(ctx: &ReducerContext, _schedule: MailEscrowReaperSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    apply_reap(&mut CtxDb { ctx });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::code_of;

    // ---- the pure decision surface ----

    /// A replay is the retry a crashed driver makes; a REUSED id is a different sender's letter
    /// wearing the same key. Answering `Replay` to the second would tell that driver its fence
    /// landed and let it settle somebody else's.
    #[test]
    fn a_replayed_fence_is_a_no_op_and_a_reused_id_is_refused() {
        assert_eq!(
            plan_fence(Some((7, 30)), 7, Some(500), 30),
            FencePlan::Replay
        );
        assert_eq!(
            plan_fence(Some((8, 30)), 7, Some(500), 30),
            FencePlan::IdCollision
        );
        assert_eq!(plan_fence(None, 7, Some(500), 30), FencePlan::Fence);
    }

    /// The same id fenced for a different AMOUNT is a second letter wearing the first one's key. A
    /// retry carries the same letter by definition, so this is never a retry — and answering
    /// `Replay` would let the commit deliver coin the fence is not holding.
    #[test]
    fn the_same_escrow_id_fenced_for_a_different_amount_is_refused() {
        assert_eq!(
            plan_fence(Some((7, 30)), 7, Some(500), 130),
            FencePlan::ValueChanged
        );
    }

    /// Affordability is decided before any write, and a sender with no live entity here has no
    /// purse at all — the two refusals are distinct because they are different problems.
    #[test]
    fn an_unaffordable_letter_and_an_absent_purse_are_refused_separately() {
        assert_eq!(plan_fence(None, 7, Some(10), 30), FencePlan::NotEnoughMoney);
        assert_eq!(plan_fence(None, 7, None, 30), FencePlan::NoPurse);
        assert_eq!(plan_fence(None, 7, Some(30), 30), FencePlan::Fence);
    }

    /// One debit for the whole letter, and it saturates — a crafted `money` near `u32::MAX` must
    /// not wrap the total down into something the sender can afford.
    #[test]
    fn the_fenced_total_is_the_coin_plus_the_postage_and_saturates() {
        let letter = |money, postage| Letter {
            recipient_guid: 1,
            subject: String::new(),
            body: String::new(),
            money,
            postage,
        };
        assert_eq!(letter(0, 30).fenced_copper(), 30);
        assert_eq!(letter(100, 30).fenced_copper(), 130);
        assert_eq!(letter(u32::MAX, 30).fenced_copper(), u32::MAX);
    }

    /// The idempotency key does its whole job here: a receipt means the mail row exists, so the
    /// replay writes nothing.
    #[test]
    fn a_commit_with_a_receipt_already_filed_writes_no_second_mail() {
        assert_eq!(plan_commit(Some(22), 22), CommitPlan::Replay);
        assert_eq!(plan_commit(None, 22), CommitPlan::Deliver);
    }

    /// A receipt naming ANOTHER recipient is the id reused for a different letter. Reading it as a
    /// replay tells the driver its letter arrived, and it settles a fence whose value went nowhere.
    #[test]
    fn a_commit_whose_receipt_names_another_recipient_is_refused() {
        assert_eq!(plan_commit(Some(23), 22), CommitPlan::IdCollision);
    }

    /// **Delete-last.** The attestation is what licenses destroying the fence; without it the
    /// settle refuses, so the value can never be in zero places.
    #[test]
    fn a_fence_is_never_destroyed_before_the_mail_row_is_attested() {
        assert_eq!(plan_settle(true, false), SettlePlan::NotDelivered);
        assert_eq!(plan_settle(true, true), SettlePlan::Complete);
        assert_eq!(plan_settle(false, true), SettlePlan::AlreadyDone);
        assert_eq!(plan_settle(false, false), SettlePlan::AlreadyDone);
    }

    /// **The recovery rule.** A missing attestation is "not yet attested", never "not delivered",
    /// so an unattested fence is HELD however old it gets. The rejected alternative — treating age
    /// as evidence of non-arrival — hands the sender back coin that is also in the recipient's
    /// mailbox.
    #[test]
    fn an_unattested_fence_is_held_forever_and_never_rolled_back() {
        let ancient = MAIL_ESCROW_STALE_MICROS * 1_000;
        assert_eq!(recovery(DeliveryEvidence::Unknown, ancient), Recovery::Hold);
        assert_eq!(
            recovery(DeliveryEvidence::Attested, ancient),
            Recovery::Settle
        );
    }

    /// A fresh fence is a driver still working, not a crashed one — the reaper is the net for a
    /// dead driver, never a race against a slow one.
    #[test]
    fn a_fence_younger_than_the_stale_window_is_left_alone() {
        let fresh = MAIL_ESCROW_STALE_MICROS - 1;
        assert_eq!(recovery(DeliveryEvidence::Attested, fresh), Recovery::Hold);
        assert_eq!(recovery(DeliveryEvidence::Unknown, fresh), Recovery::Hold);
    }

    /// The one line where a future edit could invent proof of non-delivery out of a missing
    /// attestation.
    #[test]
    fn an_absent_attestation_is_unknown_and_not_a_negative() {
        assert_eq!(evidence_of(false), DeliveryEvidence::Unknown);
        assert_eq!(evidence_of(true), DeliveryEvidence::Attested);
    }

    // ---- the production adapter ----

    /// **[`CtxDb`] is the one layer no test underneath it can reach.** `harness` substitutes a fake
    /// for every line of it, so a no-op'd method here leaves the whole suite green while the
    /// mechanism does nothing — `debit_purse` as a no-op fences value that never left the purse
    /// (the dupe), `deliver` as a no-op files a receipt for a mail row that does not exist, and an
    /// early return in any shim makes the reducer the gateway calls do nothing at all.
    ///
    /// cargo-mutants cannot cover it either: it can only ask whether a test FAILS, and no headless
    /// test can execute a `ReducerContext` (`transfer/tests.rs` measured 54 missed mutants across
    /// the identical layer). So the instrument is equality — not `contains`, which a dead branch
    /// defeats. Re-bless a deliberate change here with the same care.
    #[test]
    fn the_production_adapter_is_the_pass_through_the_harness_assumes() {
        let src = include_str!("mail_escrow.rs");
        for (signature, want) in [
            (
                "impl EscrowLedger for CtxDb<'_> {",
                "{ fn escrow(&self, escrow_id: u64) -> Option<MailEscrow> { \
                 self.ctx.db.game_mail_escrow().escrow_id().find(escrow_id) } fn file_escrow(&mut \
                 self, row: MailEscrow) { self.ctx.db.game_mail_escrow().insert(row); } fn \
                 attest_delivery(&mut self, escrow_id: u64) { if let Some(row) = \
                 self.escrow(escrow_id) { self.ctx .db .game_mail_escrow() .escrow_id() \
                 .update(MailEscrow { delivered: true, ..row }); } } fn delete_escrow(&mut self, \
                 escrow_id: u64) { self.ctx.db.game_mail_escrow().escrow_id().delete(escrow_id); } \
                 fn now_micros(&self) -> i64 { self.ctx.timestamp.to_micros_since_unix_epoch() } }",
            ),
            (
                "impl FenceSink for CtxDb<'_> {",
                "{ fn purse(&self, sender_guid: u64) -> Option<u32> { \
                 crate::helpers::acting_entity_by_guid(self.ctx, sender_guid).map(|e| e.money) } fn \
                 debit_purse(&mut self, sender_guid: u64, amount: u32) { if let Some(mut e) = \
                 crate::helpers::acting_entity_by_guid(self.ctx, sender_guid) { e.money = \
                 e.money.saturating_sub(amount); self.ctx.db.game_world_entity().guid().update(e); \
                 } } fn arm_reaper(&mut self) { let sched = \
                 self.ctx.db.game_mail_escrow_reaper_schedule(); if sched.iter().next().is_none() { \
                 sched.insert(MailEscrowReaperSchedule { scheduled_id: 0, scheduled_at: \
                 ScheduleAt::Interval(TimeDuration::from_micros( MAIL_ESCROW_REAP_INTERVAL_MICROS \
                 as i64, )), }); } } }",
            ),
            (
                "impl ReapSink for CtxDb<'_> {",
                "{ fn escrows(&self) -> Vec<(u64, u64, i64, bool)> { self.ctx .db \
                 .game_mail_escrow() .iter() .map(|e| (e.escrow_id, e.sender_guid, \
                 e.created_micros, e.delivered)) .collect() } }",
            ),
            (
                "impl DeliverySink for CtxDb<'_> {",
                "{ fn receipt(&self, escrow_id: u64) -> Option<MailDelivery> { \
                 self.ctx.db.game_mail_delivery().escrow_id().find(escrow_id) } fn deliver(&mut \
                 self, sender_guid: u64, letter: &Letter) -> u64 { crate::mail::insert_mail( \
                 self.ctx, letter.recipient_guid, sender_guid, letter.subject.clone(), \
                 letter.body.clone(), letter.money, ) } fn file_receipt(&mut self, row: \
                 MailDelivery) { self.ctx.db.game_mail_delivery().insert(row); } fn \
                 now_micros(&self) -> i64 { self.ctx.timestamp.to_micros_since_unix_epoch() } }",
            ),
            (
                "pub fn realm_mail_fence(",
                "{ require_operator(ctx)?; apply_fence( &mut CtxDb { ctx }, escrow_id, sender_guid, \
                 Letter { recipient_guid, subject, body, money, postage, }, ) }",
            ),
            (
                "pub fn realm_mail_commit(",
                "{ require_operator(ctx)?; apply_commit( &mut CtxDb { ctx }, escrow_id, \
                 sender_guid, &Letter { recipient_guid, subject, body, money, postage: 0, }, ) }",
            ),
            (
                "pub fn realm_mail_confirm_delivery(",
                "{ require_operator(ctx)?; apply_confirm(&mut CtxDb { ctx }, escrow_id) }",
            ),
            (
                "pub fn realm_mail_settle(",
                "{ require_operator(ctx)?; apply_settle(&mut CtxDb { ctx }, escrow_id) }",
            ),
            (
                "pub fn reap_mail_escrows(",
                "{ if ctx.sender() != ctx.database_identity() { return; } apply_reap(&mut CtxDb { \
                 ctx }); }",
            ),
        ] {
            let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(
                crate::test_scan::shape_of(src, signature),
                want,
                "`{signature}` is no longer the exact pass-through `mod harness` assumes it is. \
                 The harness runs the shared body underneath this layer with a fake substituted \
                 for every line here, and nothing else covers an edit to it."
            );
        }
    }

    // ---- the operator gate ----

    /// **The operator gate is the entire authorization of this surface.** Every reducer here takes
    /// the acting guid as an argument, so without the gate any identity that can reach the node
    /// empties a purse, posts mail as anybody, or destroys a fence that is holding somebody's coin.
    ///
    /// Asserted as the FIRST STATEMENT of the body, not merely present in it: a neutered gate
    /// (`if false { .. }`, or an early return above it) still contains the text.
    #[test]
    fn the_mail_escrow_reducers_are_operator_gated() {
        for signature in [
            "pub fn realm_mail_fence(",
            "pub fn realm_mail_commit(",
            "pub fn realm_mail_confirm_delivery(",
            "pub fn realm_mail_settle(",
        ] {
            let body = code_of(include_str!("mail_escrow.rs"), signature);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ require_operator(ctx)?;"),
                "`{signature}` no longer OPENS with the operator gate. Body was:\n{body}"
            );
        }
    }
}
