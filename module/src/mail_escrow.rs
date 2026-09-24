//! Delete-last transfer of mail value across databases, keyed by a caller-chosen escrow ID.

use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use lyracore_shared::mail::RewardHeader;

use crate::game_world_entity;
use crate::helpers::require_operator;
use crate::mail::game_mail;

#[cfg(test)]
mod harness;
pub(crate) const MAIL_ESCROW_STALE_MICROS: i64 = 30_000_000; // 30s
const MAIL_ESCROW_REAP_INTERVAL_MICROS: u64 = 5_000_000; // 5s
#[table(
    accessor = game_mail_escrow,
    index(accessor = by_sender, btree(columns = [sender_guid]))
)]
pub struct MailEscrow {
    #[primary_key]
    pub escrow_id: u64,
    pub sender_guid: u64,
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    pub money: u32,
    pub postage: u32,
    pub created_micros: i64,
    pub delivered: bool,
    #[default(false)]
    pub payout: bool,
    #[default(0u64)]
    pub mail_id: u64,
    #[default(0u32)]
    pub item_entry: u32,
    #[default(0u32)]
    pub item_stack_count: u32,
    #[default(0u32)]
    pub item_durability: u32,
    #[default(0u32)]
    pub item_enchant_id: u32,
    #[default(false)]
    pub item_soulbound: bool,
    #[default(0u32)]
    pub cod: u32,
    #[default(0)]
    pub random_property_id: u32,
    /// The Delivery Delay the fence resolved for its letter, so a re-driven commit keeps it. 0 for
    /// a take, a COD payment and every fence filed before the column existed.
    #[default(0u32)]
    pub delivery_delay_secs: u32,
    /// A Reward Letter's quest giver, as `game_mail.sender_kind` and `sender_entry` store it, and
    /// the Mail Template it names. 0 for every other fence, which is a Character's letter.
    #[default(0u8)]
    pub sender_kind: u8,
    #[default(0u32)]
    pub sender_entry: u32,
    #[default(0u32)]
    pub mail_template_id: u32,
    /// The attached Plain Letter's `ITEM_FIELD_ITEM_TEXT_ID`, as `game_mail.item_text_id`.
    #[default(0u32)]
    pub item_text_id: u32,
}

impl MailEscrow {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn item(&self) -> crate::items::ItemSnapshot {
        crate::items::ItemSnapshot {
            entry: self.item_entry,
            stack_count: self.item_stack_count,
            durability: self.item_durability,
            enchant_id: self.item_enchant_id,
            soulbound: self.item_soulbound,
            random_property_id: self.random_property_id,
            item_text_id: self.item_text_id,
        }
    }
}
#[table(
    accessor = game_mail_delivery,
    index(accessor = by_recipient, btree(columns = [recipient_guid]))
)]
pub struct MailDelivery {
    #[primary_key]
    pub escrow_id: u64,
    pub mail_id: u64,
    pub recipient_guid: u64,
    pub created_micros: i64,
}
#[table(accessor = game_mail_escrow_reaper_schedule, scheduled(reap_mail_escrows))]
pub struct MailEscrowReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}
crate::character_owned!(delete, fn sweep_delete_game_mail_escrow(ctx, character_guid) {
    let escrows = ctx.db.game_mail_escrow();
    for r in escrows.by_sender().filter(&character_guid).collect::<Vec<_>>() {
        escrows.escrow_id().delete(r.escrow_id);
    }
});
// A Home Shard escrow row is a letter its Character still owes the mail plane: a send whose purse
// was debited, or a Reward Letter. It travels with the Character, because the Gateway drives only
// the escrows the Character's current Home Shard holds, and a row left on the old Shard would be
// deleted with the Character there. The escrow id stays the receipt key on Realm-core, so a drive
// that races the hop still writes one letter. A take fence lives on Realm-core, which no Character
// leaves.
crate::character_owned!(transfer, fn sweep_transfer_game_mail_escrow(ctx, character_guid, io) {
    table = game_mail_escrow,
    by = by_sender,
    keep_key,
});
crate::character_owned!(delete, fn sweep_delete_game_mail_delivery(ctx, character_guid) {
    let receipts = ctx.db.game_mail_delivery();
    for r in receipts.by_recipient().filter(&character_guid).collect::<Vec<_>>() {
        receipts.escrow_id().delete(r.escrow_id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_mail_delivery());
/// A letter as its sender wrote it, with the postage paid for it. The commit turns it into a
/// [`crate::mail::Letter`], which adds the header.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Draft {
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    pub money: u32,
    pub postage: u32,
    pub cod: u32,
}

impl Draft {
    pub(crate) fn fenced_copper(&self) -> u32 {
        self.money.saturating_add(self.postage)
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FencePlan {
    Replay,
    IdCollision,
    ValueChanged,
    NoPurse,
    NotEnoughMoney,
    Fence,
}
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CommitPlan {
    Replay,
    IdCollision,
    Deliver,
}

const LEGACY_COD_PAYMENT_PREFIX: &str = "COD Payment: ";
/// The Mail a COD payment is for, as the mail plane sees it now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PricedMail {
    pub recipient_guid: u64,
    pub cod: u32,
    pub delivered: bool,
}
/// What a commit does with a COD payment. The payer's copper is already in the fence, so every
/// outcome except `Hold` must land it in a Mail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CodPayment {
    /// The payer owes a delivered price: pay the seller and settle the price.
    Pay,
    /// The payer owes the price, but the Mail is not delivered on this plane's clock yet, which
    /// runs behind the Gateway's. The payment waits in its fence for a later commit.
    Hold,
    /// Nobody owes the price any more: Mail Expiry or a return sent the Mail back, it was deleted,
    /// or the price was paid. The payment goes back to the payer as a Returned Mail.
    Refund,
}
pub(crate) fn plan_cod_payment(priced: Option<PricedMail>, payer_guid: u64) -> CodPayment {
    match priced {
        Some(m) if m.recipient_guid == payer_guid && m.cod > 0 && m.delivered => CodPayment::Pay,
        Some(m) if m.recipient_guid == payer_guid && m.cod > 0 => CodPayment::Hold,
        _ => CodPayment::Refund,
    }
}
pub(crate) fn plan_commit(receipted_for: Option<u64>, recipient_guid: u64) -> CommitPlan {
    match receipted_for {
        None => CommitPlan::Deliver,
        Some(r) if r == recipient_guid => CommitPlan::Replay,
        Some(_) => CommitPlan::IdCollision,
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TakeFenceId {
    Replay,
    Collision,
    Fresh,
}
pub(crate) fn plan_take_fence_id(
    fenced: Option<(u64, u64)>,
    payee_guid: u64,
    mail_id: u64,
) -> TakeFenceId {
    match fenced {
        None => TakeFenceId::Fresh,
        Some((payee, mail)) if payee == payee_guid && mail == mail_id => TakeFenceId::Replay,
        Some(_) => TakeFenceId::Collision,
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SettlePlan {
    AlreadyDone,
    NotDelivered,
    Complete,
}

pub(crate) fn plan_settle(has_escrow: bool, delivered: bool) -> SettlePlan {
    match (has_escrow, delivered) {
        (false, _) => SettlePlan::AlreadyDone,
        (true, false) => SettlePlan::NotDelivered,
        (true, true) => SettlePlan::Complete,
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeliveryEvidence {
    Attested,
    Unknown,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Recovery {
    Hold,
    Settle,
}
pub(crate) fn recovery(evidence: DeliveryEvidence, age_micros: i64) -> Recovery {
    if age_micros < MAIL_ESCROW_STALE_MICROS {
        return Recovery::Hold;
    }
    match evidence {
        DeliveryEvidence::Attested => Recovery::Settle,
        DeliveryEvidence::Unknown => Recovery::Hold,
    }
}
pub(crate) fn evidence_of(delivered: bool) -> DeliveryEvidence {
    if delivered {
        DeliveryEvidence::Attested
    } else {
        DeliveryEvidence::Unknown
    }
}
pub(crate) trait EscrowLedger {
    fn escrow(&self, escrow_id: u64) -> Option<MailEscrow>;
    fn file_escrow(&mut self, row: MailEscrow);
    fn attest_delivery(&mut self, escrow_id: u64);
    fn delete_escrow(&mut self, escrow_id: u64);
    fn arm_reaper(&mut self);
    fn now_micros(&self) -> i64;
}
pub(crate) trait FenceSink: EscrowLedger {
    fn purse(&self, sender_guid: u64) -> Option<u32>;
    fn debit_purse(&mut self, sender_guid: u64, amount: u32);
    fn detach_item(
        &mut self,
        sender_guid: u64,
        item_guid: u64,
    ) -> Result<crate::items::ItemSnapshot, String>;
}
pub(crate) trait ReapSink: EscrowLedger {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)>;
}
pub(crate) trait DeliverySink {
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery>;
    fn deliver(&mut self, letter: crate::mail::Letter) -> u64;
    fn priced_mail(&self, mail_id: u64) -> Option<PricedMail>;
    fn settle_cod(&mut self, mail_id: u64);
    fn file_receipt(&mut self, row: MailDelivery);
    fn now_micros(&self) -> i64;
}
pub(crate) trait TakeFenceSink: EscrowLedger {
    fn mail(&self, mail_id: u64) -> Option<(u64, u32)>;
    fn clear_mail_money(&mut self, mail_id: u64);
    fn mail_item(&self, mail_id: u64) -> Option<(u64, crate::items::ItemSnapshot)>;
    fn clear_mail_item(&mut self, mail_id: u64);
}
pub(crate) trait PayoutSink {
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery>;
    fn credit_purse(&mut self, payee_guid: u64, amount: u32) -> bool;
    fn grant_item(
        &mut self,
        payee_guid: u64,
        item: &crate::items::ItemSnapshot,
    ) -> Result<(), String>;
    fn file_receipt(&mut self, row: MailDelivery);
    fn now_micros(&self) -> i64;
}
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
    fn detach_item(
        &mut self,
        sender_guid: u64,
        item_guid: u64,
    ) -> Result<crate::items::ItemSnapshot, String> {
        crate::mail::detach_item(self.ctx, sender_guid, item_guid)
    }
}

impl TakeFenceSink for CtxDb<'_> {
    fn mail(&self, mail_id: u64) -> Option<(u64, u32)> {
        crate::mail::mail_money(self.ctx, mail_id)
    }
    fn clear_mail_money(&mut self, mail_id: u64) {
        crate::mail::clear_mail_money(self.ctx, mail_id);
    }
    fn mail_item(&self, mail_id: u64) -> Option<(u64, crate::items::ItemSnapshot)> {
        crate::mail::mail_item(self.ctx, mail_id)
    }
    fn clear_mail_item(&mut self, mail_id: u64) {
        crate::mail::clear_mail_item(self.ctx, mail_id);
    }
}

impl PayoutSink for CtxDb<'_> {
    fn receipt(&self, escrow_id: u64) -> Option<MailDelivery> {
        self.ctx.db.game_mail_delivery().escrow_id().find(escrow_id)
    }
    fn credit_purse(&mut self, payee_guid: u64, amount: u32) -> bool {
        let Some(mut e) = crate::helpers::acting_entity_by_guid(self.ctx, payee_guid) else {
            return false;
        };
        e.money = crate::mail::credited(e.money, amount);
        self.ctx.db.game_world_entity().guid().update(e);
        true
    }
    fn grant_item(
        &mut self,
        payee_guid: u64,
        item: &crate::items::ItemSnapshot,
    ) -> Result<(), String> {
        crate::mail::grant_snapshot(self.ctx, payee_guid, item)
    }
    fn file_receipt(&mut self, row: MailDelivery) {
        self.ctx.db.game_mail_delivery().insert(row);
    }
    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
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
    fn deliver(&mut self, letter: crate::mail::Letter) -> u64 {
        crate::mail::insert_letter(self.ctx, letter)
    }
    fn priced_mail(&self, mail_id: u64) -> Option<PricedMail> {
        self.ctx
            .db
            .game_mail()
            .id()
            .find(mail_id)
            .map(|m| PricedMail {
                recipient_guid: m.recipient_guid,
                cod: m.cod,
                delivered: m.is_delivered(self.ctx.timestamp),
            })
    }
    fn settle_cod(&mut self, mail_id: u64) {
        crate::mail::clear_mail_cod(self.ctx, mail_id);
    }
    fn file_receipt(&mut self, row: MailDelivery) {
        self.ctx.db.game_mail_delivery().insert(row);
    }
    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
}
pub(crate) fn apply_fence<S: FenceSink>(
    sink: &mut S,
    escrow_id: u64,
    sender_guid: u64,
    draft: Draft,
    item_guid: u64,
    mail_id: u64,
    same_account: bool,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    let cost = draft.fenced_copper();
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
    let item = sink.detach_item(sender_guid, item_guid)?;
    sink.debit_purse(sender_guid, cost);
    let created_micros = sink.now_micros();
    sink.file_escrow(MailEscrow {
        escrow_id,
        sender_guid,
        recipient_guid: draft.recipient_guid,
        subject: draft.subject,
        body: draft.body,
        money: draft.money,
        postage: draft.postage,
        created_micros,
        delivered: false,
        payout: false,
        mail_id,
        item_entry: item.entry,
        item_stack_count: item.stack_count,
        item_durability: item.durability,
        item_enchant_id: item.enchant_id,
        item_soulbound: item.soulbound,
        random_property_id: item.random_property_id,
        item_text_id: item.item_text_id,
        cod: draft.cod,
        delivery_delay_secs: crate::mail::delivery_delay_secs(!item.is_empty(), same_account),
        sender_kind: 0,
        sender_entry: 0,
        mail_template_id: 0,
    });
    sink.arm_reaper();
    log::info!(
        "mail escrow {escrow_id}: fenced {cost} copper and item {} from sender {sender_guid}",
        item.entry
    );
    Ok(())
}
/// File a Reward Letter as Escrow for its recipient, in the turn-in's transaction. The recipient's
/// session drives it to the mail plane as it drives a stalled send of its own. No purse pays: the
/// quest creates the letter's copper and item, as it creates quest money and items.
pub(crate) fn apply_file_reward<S: EscrowLedger>(
    sink: &mut S,
    escrow_id: u64,
    recipient_guid: u64,
    letter: &crate::mail_reward::RewardLetter,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    if sink.escrow(escrow_id).is_some() {
        return Err(format!(
            "mail escrow {escrow_id} is already fenced, refusing to file a Reward Letter under it"
        ));
    }
    let (sender_kind, sender_entry, mail_template_id) = RewardHeader::columns(Some(letter.header));
    let created_micros = sink.now_micros();
    sink.file_escrow(MailEscrow {
        escrow_id,
        sender_guid: recipient_guid,
        recipient_guid,
        subject: String::new(),
        body: letter.body.clone(),
        money: letter.money,
        postage: 0,
        created_micros,
        delivered: false,
        payout: false,
        mail_id: 0,
        item_entry: letter.item.entry,
        item_stack_count: letter.item.stack_count,
        item_durability: letter.item.durability,
        item_enchant_id: letter.item.enchant_id,
        item_soulbound: letter.item.soulbound,
        random_property_id: letter.item.random_property_id,
        item_text_id: letter.item.item_text_id,
        cod: 0,
        delivery_delay_secs: letter.delay_secs,
        sender_kind,
        sender_entry,
        mail_template_id,
    });
    sink.arm_reaper();
    log::info!(
        "mail escrow {escrow_id}: filed a Reward Letter from {:?} for {recipient_guid}",
        letter.header.giver
    );
    Ok(())
}
/// [`apply_file_reward`] on this database.
pub(crate) fn file_reward(
    ctx: &ReducerContext,
    escrow_id: u64,
    recipient_guid: u64,
    letter: &crate::mail_reward::RewardLetter,
) -> Result<(), String> {
    apply_file_reward(&mut CtxDb { ctx }, escrow_id, recipient_guid, letter)
}
#[allow(clippy::too_many_arguments)] // it takes the reducer's wire arguments
pub(crate) fn apply_commit<S: DeliverySink>(
    sink: &mut S,
    escrow_id: u64,
    sender_guid: u64,
    draft: &Draft,
    item: &crate::items::ItemSnapshot,
    cod_mail_id: u64,
    delivery_delay_secs: u32,
    reward: Option<RewardHeader>,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    match plan_commit(
        sink.receipt(escrow_id).map(|r| r.recipient_guid),
        draft.recipient_guid,
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
    // A commit that settles another mail's price is that price's payment. The take fence that
    // follows refuses a Mail the payment did not pay for, so the payment cannot buy an item.
    let pays_cod = cod_mail_id != 0;
    if pays_cod && reward.is_some() {
        return Err(format!(
            "mail escrow {escrow_id}: a Reward Letter pays no cash on delivery price"
        ));
    }
    let payment = pays_cod.then(|| plan_cod_payment(sink.priced_mail(cod_mail_id), sender_guid));
    if payment == Some(CodPayment::Hold) {
        return Err(format!(
            "mail escrow {escrow_id}: mail {cod_mail_id} owes {sender_guid} no delivered price yet \
             — holding the payment"
        ));
    }
    let subject = if pays_cod {
        // A payment fenced by the previous Gateway stored this prefix. The client adds it again
        // for a COD_PAYMENT letter.
        draft
            .subject
            .strip_prefix(LEGACY_COD_PAYMENT_PREFIX)
            .unwrap_or(&draft.subject)
            .to_string()
    } else {
        draft.subject.clone()
    };
    let letter = match reward {
        None => crate::mail::Letter::from_character(
            sender_guid,
            draft.recipient_guid,
            subject,
            draft.body.clone(),
            draft.money,
            draft.cod,
            *item,
        ),
        Some(header) => crate::mail::Letter::reward(
            header,
            draft.recipient_guid,
            draft.body.clone(),
            draft.money,
            *item,
        ),
    };
    // The Delivery Delay counts from this commit. A COD payment and its refund carry copper only
    // and arrive at once (cmangos `MailHandler.cpp:475-477`).
    let letter = match payment {
        None => letter.delayed(sink.now_micros(), delivery_delay_secs),
        Some(CodPayment::Pay) => letter.into_cod_payment(),
        // A refund. `Hold` returned above.
        Some(_) => letter.into_returned(),
    };
    let delivered_to = letter.recipient_guid;
    let mail_id = sink.deliver(letter);
    if payment == Some(CodPayment::Pay) {
        sink.settle_cod(cod_mail_id);
    }
    let created_micros = sink.now_micros();
    // The receipt names the Draft's recipient even for a refund, so a replayed commit finds it.
    sink.file_receipt(MailDelivery {
        escrow_id,
        mail_id,
        recipient_guid: draft.recipient_guid,
        created_micros,
    });
    log::info!(
        "mail escrow {escrow_id}: committed as mail {mail_id} for recipient {delivered_to}{}",
        if payment == Some(CodPayment::Refund) {
            format!(", the payment back because mail {cod_mail_id} owes no price")
        } else {
            String::new()
        }
    );
    Ok(())
}
pub(crate) fn apply_take_fence<S: TakeFenceSink>(
    sink: &mut S,
    escrow_id: u64,
    payee_guid: u64,
    mail_id: u64,
    expect_money: u32,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    match plan_take_fence_id(
        sink.escrow(escrow_id).map(|e| (e.sender_guid, e.mail_id)),
        payee_guid,
        mail_id,
    ) {
        TakeFenceId::Replay => return Ok(()),
        TakeFenceId::Collision => {
            return Err(format!(
                "mail escrow {escrow_id} is already fenced for another take — refusing to reuse \
                 the id for mail {mail_id}"
            ))
        }
        TakeFenceId::Fresh => {}
    }
    let money = match crate::mail::plan_take_money(sink.mail(mail_id), payee_guid) {
        crate::mail::TakeMoney::NotYours => {
            return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string())
        }
        crate::mail::TakeMoney::NothingToTake => {
            return Err(lyracore_shared::mail::NOTHING_TO_TAKE.to_string())
        }
        crate::mail::TakeMoney::Take(money) => money,
    };
    if money != expect_money {
        return Err(format!(
            "mail {mail_id} holds {money} copper, not the {expect_money} this take was driven for \
             — refusing to fence an amount the payout would not match"
        ));
    }
    sink.clear_mail_money(mail_id);
    let created_micros = sink.now_micros();
    sink.file_escrow(MailEscrow {
        escrow_id,
        sender_guid: payee_guid,
        recipient_guid: payee_guid,
        subject: String::new(),
        body: String::new(),
        money,
        postage: 0,
        created_micros,
        delivered: false,
        payout: true,
        mail_id,
        item_entry: 0,
        item_stack_count: 0,
        item_durability: 0,
        item_enchant_id: 0,
        item_soulbound: false,
        random_property_id: 0,
        item_text_id: 0,
        cod: 0,
        delivery_delay_secs: 0,
        sender_kind: 0,
        sender_entry: 0,
        mail_template_id: 0,
    });
    sink.arm_reaper();
    log::info!(
        "mail escrow {escrow_id}: fenced {money} copper out of mail {mail_id} for {payee_guid}"
    );
    Ok(())
}
pub(crate) fn apply_take_item_fence<S: TakeFenceSink>(
    sink: &mut S,
    escrow_id: u64,
    payee_guid: u64,
    mail_id: u64,
    expect_entry: u32,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    match plan_take_fence_id(
        sink.escrow(escrow_id).map(|e| (e.sender_guid, e.mail_id)),
        payee_guid,
        mail_id,
    ) {
        TakeFenceId::Replay => return Ok(()),
        TakeFenceId::Collision => {
            return Err(format!(
                "mail escrow {escrow_id} is already fenced for another take — refusing to reuse \
                 the id for mail {mail_id}"
            ))
        }
        TakeFenceId::Fresh => {}
    }
    let row = sink.mail_item(mail_id);
    match crate::mail::plan_take_item(row.as_ref().map(|(to, i)| (*to, i.entry)), payee_guid) {
        crate::mail::TakeItem::NotYours => {
            return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string())
        }
        crate::mail::TakeItem::NothingToTake => {
            return Err(lyracore_shared::mail::NOTHING_TO_TAKE.to_string())
        }
        crate::mail::TakeItem::Take => {}
    }
    let (_, item) = row.expect("Take is only reachable with a row");
    if item.entry != expect_entry {
        return Err(format!(
            "mail {mail_id} holds item {}, not the {expect_entry} this take was driven for — \
             refusing to fence an item the grant would not match",
            item.entry
        ));
    }
    sink.clear_mail_item(mail_id);
    let created_micros = sink.now_micros();
    sink.file_escrow(MailEscrow {
        escrow_id,
        sender_guid: payee_guid,
        recipient_guid: payee_guid,
        subject: String::new(),
        body: String::new(),
        money: 0,
        postage: 0,
        created_micros,
        delivered: false,
        payout: true,
        mail_id,
        item_entry: item.entry,
        item_stack_count: item.stack_count,
        item_durability: item.durability,
        item_enchant_id: item.enchant_id,
        item_soulbound: item.soulbound,
        random_property_id: item.random_property_id,
        item_text_id: item.item_text_id,
        cod: 0,
        delivery_delay_secs: 0,
        sender_kind: 0,
        sender_entry: 0,
        mail_template_id: 0,
    });
    sink.arm_reaper();
    log::info!(
        "mail escrow {escrow_id}: fenced item {} out of mail {mail_id} for {payee_guid}",
        item.entry
    );
    Ok(())
}
pub(crate) fn apply_payout<S: PayoutSink>(
    sink: &mut S,
    escrow_id: u64,
    payee_guid: u64,
    mail_id: u64,
    amount: u32,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    match plan_commit(
        sink.receipt(escrow_id).map(|r| r.recipient_guid),
        payee_guid,
    ) {
        CommitPlan::Replay => return Ok(()),
        CommitPlan::IdCollision => {
            return Err(format!(
                "mail escrow {escrow_id} already paid out to another character — refusing to reuse \
                 the id"
            ))
        }
        CommitPlan::Deliver => {}
    }
    if !sink.credit_purse(payee_guid, amount) {
        return Err(lyracore_shared::mail::NOT_IN_WORLD.to_string());
    }
    let created_micros = sink.now_micros();
    sink.file_receipt(MailDelivery {
        escrow_id,
        mail_id,
        recipient_guid: payee_guid,
        created_micros,
    });
    log::info!("mail escrow {escrow_id}: paid {amount} copper from mail {mail_id} to {payee_guid}");
    Ok(())
}
pub(crate) fn apply_item_payout<S: PayoutSink>(
    sink: &mut S,
    escrow_id: u64,
    payee_guid: u64,
    mail_id: u64,
    item: &crate::items::ItemSnapshot,
) -> Result<(), String> {
    if escrow_id == 0 {
        return Err("escrow_id 0 is reserved (it is the \"no escrow\" sentinel)".to_string());
    }
    match plan_commit(
        sink.receipt(escrow_id).map(|r| r.recipient_guid),
        payee_guid,
    ) {
        CommitPlan::Replay => return Ok(()),
        CommitPlan::IdCollision => {
            return Err(format!(
                "mail escrow {escrow_id} already paid out to another character — refusing to reuse \
                 the id"
            ))
        }
        CommitPlan::Deliver => {}
    }
    sink.grant_item(payee_guid, item)?;
    let created_micros = sink.now_micros();
    sink.file_receipt(MailDelivery {
        escrow_id,
        mail_id,
        recipient_guid: payee_guid,
        created_micros,
    });
    log::info!(
        "mail escrow {escrow_id}: granted item {} from mail {mail_id} to {payee_guid}",
        item.entry
    );
    Ok(())
}
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
pub(crate) fn apply_settle<S: EscrowLedger>(sink: &mut S, escrow_id: u64) -> Result<(), String> {
    let row = sink.escrow(escrow_id);
    match plan_settle(row.is_some(), row.as_ref().is_some_and(|r| r.delivered)) {
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
pub(crate) fn apply_reap<S: ReapSink>(sink: &mut S) {
    let now = sink.now_micros();
    for (escrow_id, sender_guid, created_micros, delivered) in sink.escrows() {
        let age = now - created_micros;
        match recovery(evidence_of(delivered), age) {
            Recovery::Hold => {
                if age >= MAIL_ESCROW_STALE_MICROS {
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
/// `same_account` says whether the sender and the recipient belong to one Realm Account. The fence
/// knows whether it detached an item, so it resolves the Delivery Delay and stores it.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn realm_mail_fence(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    postage: u32,
    item_guid: u64,
    cod: u32,
    mail_id: u64,
    same_account: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_fence(
        &mut CtxDb { ctx },
        escrow_id,
        sender_guid,
        Draft {
            recipient_guid,
            subject,
            body,
            money,
            postage,
            cod,
        },
        item_guid,
        mail_id,
        same_account,
    )
}
/// `delivery_delay_secs` is the Delivery Delay the fence stored. The letter arrives that long after
/// this commit. A Reward Letter's escrow row also stores `sender_kind`, `sender_entry` and
/// `mail_template_id`, and the commit writes the letter from its quest giver. They are 0 for a
/// Character's letter.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn realm_mail_commit(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    random_property_id: u32,
    cod: u32,
    cod_mail_id: u64,
    delivery_delay_secs: u32,
    sender_kind: u8,
    sender_entry: u32,
    mail_template_id: u32,
    item_text_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let sender_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_commit(
        &mut CtxDb { ctx },
        escrow_id,
        sender_guid,
        &Draft {
            recipient_guid,
            subject,
            body,
            money,
            postage: 0,
            cod,
        },
        &crate::items::ItemSnapshot {
            entry: item_entry,
            stack_count: item_stack_count,
            durability: item_durability,
            enchant_id: item_enchant_id,
            soulbound: item_soulbound,
            random_property_id,
            item_text_id,
        },
        cod_mail_id,
        delivery_delay_secs,
        RewardHeader::from_columns(sender_kind, sender_entry, mail_template_id)?,
    )
}
#[reducer]
pub fn realm_mail_take_money_fence(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
    mail_id: u64,
    expect_money: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_take_fence(
        &mut CtxDb { ctx },
        escrow_id,
        payee_guid,
        mail_id,
        expect_money,
    )
}
#[reducer]
pub fn realm_mail_payout(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
    mail_id: u64,
    amount: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_payout(&mut CtxDb { ctx }, escrow_id, payee_guid, mail_id, amount)
}
#[reducer]
pub fn realm_mail_take_item_fence(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
    mail_id: u64,
    expect_entry: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_take_item_fence(
        &mut CtxDb { ctx },
        escrow_id,
        payee_guid,
        mail_id,
        expect_entry,
    )
}
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn realm_mail_item_payout(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
    mail_id: u64,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    random_property_id: u32,
    item_text_id: u32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_item_payout(
        &mut CtxDb { ctx },
        escrow_id,
        payee_guid,
        mail_id,
        &crate::items::ItemSnapshot {
            entry: item_entry,
            stack_count: item_stack_count,
            durability: item_durability,
            enchant_id: item_enchant_id,
            soulbound: item_soulbound,
            random_property_id,
            item_text_id,
        },
    )
}
fn require_escrow_actor(
    ctx: &ReducerContext,
    escrow_id: u64,
    actor: crate::SessionActor,
) -> Result<(), String> {
    crate::account_ownership::require_actor(ctx, actor)?;
    if let Some(row) = ctx.db.game_mail_escrow().escrow_id().find(escrow_id) {
        crate::account_ownership::require_actor_for(ctx, actor, row.sender_guid)?;
    }
    Ok(())
}

#[reducer]
pub fn realm_mail_confirm_delivery(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_escrow_actor(ctx, escrow_id, request_actor)?;
    apply_confirm(&mut CtxDb { ctx }, escrow_id)
}
#[reducer]
pub fn realm_mail_settle(
    ctx: &ReducerContext,
    escrow_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_escrow_actor(ctx, escrow_id, request_actor)?;
    apply_settle(&mut CtxDb { ctx }, escrow_id)
}
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
    #[test]
    fn the_same_escrow_id_fenced_for_a_different_amount_is_refused() {
        assert_eq!(
            plan_fence(Some((7, 30)), 7, Some(500), 130),
            FencePlan::ValueChanged
        );
    }
    #[test]
    fn an_unaffordable_letter_and_an_absent_purse_are_refused_separately() {
        assert_eq!(plan_fence(None, 7, Some(10), 30), FencePlan::NotEnoughMoney);
        assert_eq!(plan_fence(None, 7, None, 30), FencePlan::NoPurse);
        assert_eq!(plan_fence(None, 7, Some(30), 30), FencePlan::Fence);
    }
    #[test]
    fn the_fenced_total_is_the_coin_plus_the_postage_and_saturates() {
        let draft = |money, postage| Draft {
            recipient_guid: 1,
            subject: String::new(),
            body: String::new(),
            money,
            postage,
            cod: 0,
        };
        assert_eq!(draft(0, 30).fenced_copper(), 30);
        assert_eq!(draft(100, 30).fenced_copper(), 130);
        assert_eq!(draft(u32::MAX, 30).fenced_copper(), u32::MAX);
    }
    #[test]
    fn a_commit_with_a_receipt_already_filed_writes_no_second_mail() {
        assert_eq!(plan_commit(Some(22), 22), CommitPlan::Replay);
        assert_eq!(plan_commit(None, 22), CommitPlan::Deliver);
    }
    #[test]
    fn a_replayed_take_fence_is_a_no_op_and_a_reused_id_is_refused() {
        assert_eq!(
            plan_take_fence_id(Some((7, 42)), 7, 42),
            TakeFenceId::Replay
        );
        assert_eq!(
            plan_take_fence_id(Some((7, 42)), 8, 42),
            TakeFenceId::Collision
        );
        assert_eq!(
            plan_take_fence_id(Some((7, 42)), 7, 43),
            TakeFenceId::Collision
        );
        assert_eq!(plan_take_fence_id(None, 7, 42), TakeFenceId::Fresh);
    }
    #[test]
    fn a_commit_whose_receipt_names_another_recipient_is_refused() {
        assert_eq!(plan_commit(Some(23), 22), CommitPlan::IdCollision);
    }
    #[test]
    fn a_fence_is_never_destroyed_before_the_mail_row_is_attested() {
        assert_eq!(plan_settle(true, false), SettlePlan::NotDelivered);
        assert_eq!(plan_settle(true, true), SettlePlan::Complete);
        assert_eq!(plan_settle(false, true), SettlePlan::AlreadyDone);
        assert_eq!(plan_settle(false, false), SettlePlan::AlreadyDone);
    }
    #[test]
    fn an_unattested_fence_is_held_forever_and_never_rolled_back() {
        let ancient = MAIL_ESCROW_STALE_MICROS * 1_000;
        assert_eq!(recovery(DeliveryEvidence::Unknown, ancient), Recovery::Hold);
        assert_eq!(
            recovery(DeliveryEvidence::Attested, ancient),
            Recovery::Settle
        );
    }
    #[test]
    fn a_fence_younger_than_the_stale_window_is_left_alone() {
        let fresh = MAIL_ESCROW_STALE_MICROS - 1;
        assert_eq!(recovery(DeliveryEvidence::Attested, fresh), Recovery::Hold);
        assert_eq!(recovery(DeliveryEvidence::Unknown, fresh), Recovery::Hold);
    }
    #[test]
    fn an_absent_attestation_is_unknown_and_not_a_negative() {
        assert_eq!(evidence_of(false), DeliveryEvidence::Unknown);
        assert_eq!(evidence_of(true), DeliveryEvidence::Attested);
    }
    #[test]
    fn the_production_adapter_is_the_pass_through_the_harness_assumes() {
        let src = include_str!("mail_escrow.rs");
        for (signature, want) in [
            (
                "impl EscrowLedger for CtxDb<'_> {",
                "{ fn escrow(&self, escrow_id: u64) -> Option<MailEscrow> { \
                  self.ctx.db.game_mail_escrow().escrow_id().find(escrow_id) } fn \
                  file_escrow(&mut self, row: MailEscrow) { \
                  self.ctx.db.game_mail_escrow().insert(row); } fn attest_delivery(&mut self, \
                  escrow_id: u64) { if let Some(row) = self.escrow(escrow_id) { self.ctx .db \
                  .game_mail_escrow() .escrow_id() .update(MailEscrow { delivered: true, ..row \
                  }); } } fn delete_escrow(&mut self, escrow_id: u64) { \
                  self.ctx.db.game_mail_escrow().escrow_id().delete(escrow_id); } fn \
                  arm_reaper(&mut self) { let sched = \
                  self.ctx.db.game_mail_escrow_reaper_schedule(); if \
                  sched.iter().next().is_none() { sched.insert(MailEscrowReaperSchedule { \
                  scheduled_id: 0, scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros( \
                  MAIL_ESCROW_REAP_INTERVAL_MICROS as i64, )), }); } } fn now_micros(&self) -> \
                  i64 { self.ctx.timestamp.to_micros_since_unix_epoch() } }",
            ),
            (
                "impl FenceSink for CtxDb<'_> {",
                "{ fn purse(&self, sender_guid: u64) -> Option<u32> { \
                  crate::helpers::acting_entity_by_guid(self.ctx, sender_guid).map(|e| e.money) \
                  } fn debit_purse(&mut self, sender_guid: u64, amount: u32) { if let Some(mut \
                  e) = crate::helpers::acting_entity_by_guid(self.ctx, sender_guid) { e.money = \
                  e.money.saturating_sub(amount); \
                  self.ctx.db.game_world_entity().guid().update(e); } } fn detach_item( &mut \
                  self, sender_guid: u64, item_guid: u64, ) -> Result<crate::items::ItemSnapshot, \
                  String> { crate::mail::detach_item(self.ctx, sender_guid, item_guid) } }",
            ),
            (
                "impl TakeFenceSink for CtxDb<'_> {",
                "{ fn mail(&self, mail_id: u64) -> Option<(u64, u32)> { \
                  crate::mail::mail_money(self.ctx, mail_id) } fn clear_mail_money(&mut self, \
                  mail_id: u64) { crate::mail::clear_mail_money(self.ctx, mail_id); } fn \
                  mail_item(&self, mail_id: u64) -> Option<(u64, crate::items::ItemSnapshot)> { \
                  crate::mail::mail_item(self.ctx, mail_id) } fn clear_mail_item(&mut self, \
                  mail_id: u64) { crate::mail::clear_mail_item(self.ctx, mail_id); } }",
            ),
            (
                "impl PayoutSink for CtxDb<'_> {",
                "{ fn receipt(&self, escrow_id: u64) -> Option<MailDelivery> { \
                  self.ctx.db.game_mail_delivery().escrow_id().find(escrow_id) } fn \
                  credit_purse(&mut self, payee_guid: u64, amount: u32) -> bool { let Some(mut \
                  e) = crate::helpers::acting_entity_by_guid(self.ctx, payee_guid) else { return \
                  false; }; e.money = crate::mail::credited(e.money, amount); \
                  self.ctx.db.game_world_entity().guid().update(e); true } fn grant_item( &mut \
                  self, payee_guid: u64, item: &crate::items::ItemSnapshot, ) -> Result<(), \
                  String> { crate::mail::grant_snapshot(self.ctx, payee_guid, item) } fn \
                  file_receipt(&mut self, row: MailDelivery) { \
                  self.ctx.db.game_mail_delivery().insert(row); } fn now_micros(&self) -> i64 { \
                  self.ctx.timestamp.to_micros_since_unix_epoch() } }",
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
                  self.ctx.db.game_mail_delivery().escrow_id().find(escrow_id) } fn \
                  deliver(&mut self, letter: crate::mail::Letter) -> u64 { \
                  crate::mail::insert_letter(self.ctx, letter) } fn priced_mail(&self, mail_id: \
                  u64) -> Option<PricedMail> { self.ctx .db .game_mail() .id() .find(mail_id) \
                  .map(|m| PricedMail { recipient_guid: m.recipient_guid, cod: m.cod, \
                  delivered: m.is_delivered(self.ctx.timestamp), }) } fn settle_cod(&mut \
                  self, mail_id: u64) { crate::mail::clear_mail_cod(self.ctx, mail_id); } fn \
                  file_receipt(&mut self, row: MailDelivery) { \
                  self.ctx.db.game_mail_delivery().insert(row); } fn now_micros(&self) -> i64 { \
                  self.ctx.timestamp.to_micros_since_unix_epoch() } }",
            ),
            (
                "pub fn realm_mail_fence(",
                "{ require_operator(ctx)?; let sender_guid = crate::account_ownership::require_actor(ctx, request_actor)?; apply_fence( &mut CtxDb { ctx }, escrow_id, \
                  sender_guid, Draft { recipient_guid, subject, body, money, postage, cod, }, \
                  item_guid, mail_id, same_account, ) }",
            ),
            (
                "pub fn realm_mail_commit(",
                "{ require_operator(ctx)?; let sender_guid = crate::account_ownership::require_actor(ctx, request_actor)?; apply_commit( &mut CtxDb { ctx }, escrow_id, \
                  sender_guid, &Draft { recipient_guid, subject, body, money, postage: 0, cod, \
                  }, &crate::items::ItemSnapshot { entry: item_entry, stack_count: \
                  item_stack_count, durability: item_durability, enchant_id: item_enchant_id, \
                  soulbound: item_soulbound, random_property_id, item_text_id, }, cod_mail_id, \
                  delivery_delay_secs, RewardHeader::from_columns(sender_kind, sender_entry, \
                  mail_template_id)?, ) }",
            ),
            (
                "pub fn realm_mail_take_money_fence(",
                "{ require_operator(ctx)?; let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?; apply_take_fence( &mut CtxDb { ctx }, escrow_id, \
                  payee_guid, mail_id, expect_money, ) }",
            ),
            (
                "pub fn realm_mail_payout(",
                "{ require_operator(ctx)?; let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?; apply_payout(&mut CtxDb { ctx }, escrow_id, \
                  payee_guid, mail_id, amount) }",
            ),
            (
                "pub fn realm_mail_take_item_fence(",
                "{ require_operator(ctx)?; let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?; apply_take_item_fence( &mut CtxDb { ctx }, \
                  escrow_id, payee_guid, mail_id, expect_entry, ) }",
            ),
            (
                "pub fn realm_mail_item_payout(",
                "{ require_operator(ctx)?; let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?; apply_item_payout( &mut CtxDb { ctx }, escrow_id, \
                  payee_guid, mail_id, &crate::items::ItemSnapshot { entry: item_entry, \
                  stack_count: item_stack_count, durability: item_durability, enchant_id: \
                  item_enchant_id, soulbound: item_soulbound, random_property_id, item_text_id, }, ) }",
            ),
            (
                "pub fn realm_mail_confirm_delivery(",
                "{ require_operator(ctx)?; require_escrow_actor(ctx, escrow_id, request_actor)?; apply_confirm(&mut CtxDb { ctx }, escrow_id) }",
            ),
            (
                "pub fn realm_mail_settle(",
                "{ require_operator(ctx)?; require_escrow_actor(ctx, escrow_id, request_actor)?; apply_settle(&mut CtxDb { ctx }, escrow_id) }",
            ),
            (
                "pub fn reap_mail_escrows(",
                "{ if ctx.sender() != ctx.database_identity() { return; } apply_reap(&mut CtxDb \
                  { ctx }); }",
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
    #[test]
    fn the_mail_escrow_reducers_are_operator_gated() {
        for signature in [
            "pub fn realm_mail_fence(",
            "pub fn realm_mail_commit(",
            "pub fn realm_mail_take_money_fence(",
            "pub fn realm_mail_payout(",
            "pub fn realm_mail_take_item_fence(",
            "pub fn realm_mail_item_payout(",
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
