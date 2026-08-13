//! Mail — the durable mailbox row and the rules both planes read it through.
//!
//! # The plane, and why the row shape does not care which one it is on
//!
//! `game_mail` is authoritative on **realm-core**, alongside the party and whisper state: a mail is
//! addressed to a character who may be offline or standing on another database, which is the exact
//! read that made a cross-boundary whisper impossible. A gateway with no realm handle
//! (`WorldStore::realm_store()` → `None`) reads and writes its OWN database instead — whisper's
//! two-plane shape, one rule with two entry points.
//!
//! Two properties keep realm-core cheap here, and both are load-bearing:
//!
//! - **A mail row is pure data.** It points at no `game_character`, no live entity and no
//!   `game_item_instance`; an attachment is a SNAPSHOT of the item's state, not a row that has to
//!   move between databases. Realm-core holding no characters therefore stops mattering.
//! - **Mail is a poll, not a push.** The client sends `CMSG_GET_MAIL_LIST` and
//!   `MSG_QUERY_NEXT_MAIL_TIME` itself, so the gateway reads on demand and builds the packet. No
//!   event table, no relay, no coordinator subscription of the whisper kind — and no
//!   `SMSG_RECEIVED_MAIL`, which is out of scope for the whole feature.
//!
//! # What is here, and what is not
//!
//! Here now: the table and its sweeps, the read path's seeder, mark-read, delete, sending a letter
//! with copper and ONE item attached, taking either back out, returning a mail to its sender, and
//! cash on delivery.
//!
//! **Cash on delivery is two moves, not one.** The price rides the row from send; taking the
//! ATTACHMENT debits the taker and posts the copper back to whoever wrote the letter. On this plane
//! all of it is one transaction ([`apply_take_item`]); on a sharded realm the payment is a letter
//! out of the taker's purse and rides [`crate::mail_escrow`] like any other. Nothing charges for
//! taking the COPPER out of a mail — a price buys the item.
//!
//! **An attachment is a SNAPSHOT, not a row.** At send the sender's `game_item_instance` is deleted
//! and its state copied into the mail row ([`ItemSnapshot`]); at take a fresh instance is created
//! carrying that state back. The item guid does not survive, and that is accepted — the recipient's
//! client learns the new one from the normal item create. Re-owning the row in place would be the
//! obvious alternative and is rejected: the gateway's item-update relay only notifies the NEW owner,
//! so it leaves a ghost item in the sender's bags until relog.
//!
//! **Both money paths in this file are the SINGLE-DATABASE plane.** On a sharded realm the purse is
//! on the sender's (or taker's) own shard and the mail row is on realm-core, so no transaction
//! spans them and the copper crosses through [`crate::mail_escrow`] instead — a fence, a commit
//! keyed by a caller-chosen id, and a delete-last settle. Where one transaction genuinely exists it
//! is used; the two paths must not be "unified".
//!
//! The wire-facing rules (the unread-mail float, the `item_text_id`, the expiry stamp) live in
//! `lyracore_shared::mail`: the gateway builds `SMSG_MAIL_LIST_RESULT` from a cache read, so a rule
//! kept here alone would have a second copy over there within one slice.

use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use crate::game_item_instance;
use crate::game_item_template;
use crate::game_world_entity;

/// One piece of mail, addressed by recipient GUID — the one realm-wide name a character has (a
/// bound identity is minted per (account, database) and names nobody elsewhere, which is why
/// `game_whisper_event` and `game_group_event` are addressed the same way).
///
/// Public like every other player-visible table, but no client ever subscribes it: the gateway
/// reads it through the owner token and answers the poll itself.
///
/// The attachment is a SNAPSHOT (`item_entry` .. `item_soulbound`), not a pointer at a live
/// `game_item_instance` row — mailing must not repair an item, strip its enchant or launder a
/// soulbound one, and a snapshot is what survives a database boundary. One attachment, because
/// vanilla's `CMSG_SEND_MAIL` carries exactly one item guid and the `Mail` wire struct has exactly
/// one item block. All zero when the letter carries no item. [entity]
#[table(accessor = game_mail, public, index(accessor = by_recipient, btree(columns = [recipient_guid])))]
pub struct Mail {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_guid: u64,
    /// The writer's character guid. The wire carries the sender as a `Guid` and the client resolves
    /// the name by `NAME_QUERY`, so there is no sender-name column to go stale.
    pub sender_guid: u64,
    pub subject: String,
    /// The letter. NOT sent in `SMSG_MAIL_LIST_RESULT` — the list advertises an `item_text_id` and
    /// the client fetches the text with `CMSG_ITEM_TEXT_QUERY`, answered from this column. The
    /// mail's own `id` doubles as that text id, so no separate text table exists.
    pub body: String,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    /// Attached copper the recipient takes out, and the cash-on-delivery price they owe before the
    /// ATTACHMENT is handed over. The two move in opposite directions and are independent: a letter
    /// can carry coin, a price, both or neither.
    pub money: u32,
    pub cod: u32,
    pub was_read: bool,
    /// Stamped for the client's expiry countdown only. Nothing expires a mail — see
    /// [`lyracore_shared::mail::EXPIRY_DAYS`]. "No reaper" is deliberate, because a row that
    /// silently destroys an attachment is the worse failure.
    pub created_at: Timestamp,
}

// Character-owned sweeps. Delete: a deleted character's mailbox goes with them (nobody can ever
// read it again, and the rows address a guid that may be re-minted).
crate::character_owned!(delete, fn sweep_delete_game_mail(ctx, character_guid) {
    let mails = ctx.db.game_mail();
    for r in mails.by_recipient().filter(&character_guid).collect::<Vec<_>>() {
        mails.id().delete(r.id);
    }
});
// CROSS-DATABASE transport: on the FALLBACK plane the mailbox lives on the character's own shard,
// so it has to follow them or walking into Deadmines empties it. On the realm plane this arm never
// fires — realm-core is not a shard a character transfers off, so its rows are never exported and
// cannot double up at the destination. `id` is an auto-inc surrogate, local to one database.
crate::character_owned!(transfer, fn sweep_transfer_game_mail(ctx, character_guid, io) {
    table = game_mail,
    by = by_recipient,
    remint = id,
});

/// One item's state, detached from the row that held it — every column mailing must not launder.
///
/// Durability and the enchant are here because a template-based re-grant would reset the first and
/// drop the second: a free repair, and an enchant-laundering exploit. `soulbound` is here because a
/// bind state that reset would make the mailbox a way to unbind gear. The guid is deliberately NOT
/// here (see the module doc).
///
/// `entry == 0` is "no attachment", the same sentinel the mail row's own columns carry.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct ItemSnapshot {
    pub entry: u32,
    pub stack_count: u32,
    pub durability: u32,
    pub enchant_id: u32,
    pub soulbound: bool,
}

impl ItemSnapshot {
    pub(crate) fn is_empty(&self) -> bool {
        self.entry == 0
    }
}

impl Mail {
    /// The attachment this row is carrying. One projection, so the take path and the fence cannot
    /// disagree about which columns make up an attachment.
    pub(crate) fn snapshot(&self) -> ItemSnapshot {
        ItemSnapshot {
            entry: self.item_entry,
            stack_count: self.item_stack_count,
            durability: self.item_durability,
            enchant_id: self.item_enchant_id,
            soulbound: self.item_soulbound,
        }
    }
}

/// **The mailable-or-refused verdict.** Pure, so the authorization boundary and the bind rule are
/// testable without a database — and shared, so the single-database send and the fence that starts
/// the sharded one cannot answer differently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Attach {
    /// The sender attached nothing (`item_guid` 0) — a text-and-coin letter.
    Nothing,
    /// No such instance, or it belongs to somebody else. One verdict for both, like
    /// [`TakeMoney::NotYours`]: an item guid is client-supplied, so a refusal must not be a way to
    /// probe which guids exist.
    NotYours,
    /// The instance has bound to its owner. Refused with its own line, and it stays in the bags —
    /// an unworn bind-on-equip item is UNBOUND and passes here.
    Soulbound,
    /// Take it out of the bags and into the letter.
    Detach,
}

/// `owned` is the `(owner_guid, soulbound)` of the named instance, or `None` when no such row
/// exists on this database.
pub(crate) fn plan_attach(item_guid: u64, owned: Option<(u64, bool)>, sender_guid: u64) -> Attach {
    if item_guid == 0 {
        return Attach::Nothing;
    }
    match owned {
        None => Attach::NotYours,
        Some((owner_guid, _)) if owner_guid != sender_guid => Attach::NotYours,
        Some((_, true)) => Attach::Soulbound,
        Some(_) => Attach::Detach,
    }
}

/// Take `item_guid` out of `sender_guid`'s bags and into a snapshot, or refuse and take nothing.
///
/// The DELETE is what makes the attachment unreachable everywhere else at once: no vendor, equip,
/// use, loot, trade or second mail has to learn a new "in flight" rule, because every one of them
/// resolves an item through this same table and now finds no row. The rejected alternative — a
/// `fenced: bool` on the instance — reads as the smaller change and is the larger one, and the one
/// path that forgot the flag would be the dupe.
pub(crate) fn detach_item(
    ctx: &ReducerContext,
    sender_guid: u64,
    item_guid: u64,
) -> Result<ItemSnapshot, String> {
    let items = ctx.db.game_item_instance();
    let owned = items.guid().find(item_guid);
    match plan_attach(
        item_guid,
        owned.as_ref().map(|i| (i.owner_guid, i.soulbound)),
        sender_guid,
    ) {
        Attach::Nothing => return Ok(ItemSnapshot::default()),
        Attach::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_ITEM.to_string()),
        Attach::Soulbound => return Err(lyracore_shared::mail::ITEM_IS_SOULBOUND.to_string()),
        Attach::Detach => {}
    }
    let inst = owned.expect("Detach is only reachable with a row");
    let snapshot = ItemSnapshot {
        entry: inst.entry,
        stack_count: inst.stack_count,
        durability: inst.durability,
        enchant_id: inst.enchant_id,
        soulbound: inst.soulbound,
    };
    items.guid().delete(item_guid);
    Ok(snapshot)
}

/// Re-create a snapshotted item in `payee_guid`'s bags, through the item module's OWN storage path.
///
/// Bag-space search, bag-family rules and the full-bag refusal are all
/// [`crate::items::store_instance_state`]'s, deliberately: a second copy here would drift, and the
/// full-bag case is the one where a naive implementation destroys the item. The `Err` rolls the
/// caller's whole transaction back, so a refused take leaves the letter holding its attachment.
pub(crate) fn grant_snapshot(
    ctx: &ReducerContext,
    payee_guid: u64,
    snapshot: &ItemSnapshot,
) -> Result<(), String> {
    if snapshot.is_empty() {
        return Err(lyracore_shared::mail::NOTHING_TO_TAKE.to_string());
    }
    let payee = crate::helpers::acting_entity_by_guid(ctx, payee_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_IN_WORLD.to_string())?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(snapshot.entry)
        .ok_or_else(|| format!("mail: no template for attached item {}", snapshot.entry))?;
    crate::items::store_instance_state(
        ctx,
        payee_guid,
        payee.owner_identity,
        &tmpl,
        snapshot.stack_count,
        snapshot.durability,
        snapshot.enchant_id,
        snapshot.soulbound,
    )
}

/// Insert one mail. The shared insert core: whichever plane holds the rows, this is the shape they
/// take, so a seeded mail and a sent one can never differ in the columns the list read projects.
#[allow(clippy::too_many_arguments)] // a row's columns, not a call's parameters
pub(crate) fn insert_mail(
    ctx: &ReducerContext,
    recipient_guid: u64,
    sender_guid: u64,
    subject: String,
    body: String,
    money: u32,
    cod: u32,
    item: &ItemSnapshot,
) -> u64 {
    ctx.db
        .game_mail()
        .insert(Mail {
            id: 0,
            recipient_guid,
            sender_guid,
            subject,
            body,
            item_entry: item.entry,
            item_stack_count: item.stack_count,
            item_durability: item.durability,
            item_enchant_id: item.enchant_id,
            item_soulbound: item.soulbound,
            money,
            cod,
            was_read: false,
            created_at: ctx.timestamp,
        })
        .id
}

/// Does `recipient_guid` have unread mail on THIS database? The answer behind
/// `MSG_QUERY_NEXT_MAIL_TIME` — the gateway reads its own cache rather than calling in, so this is
/// the module-side twin, used by the seeder's log line and available to the later mark-read slice.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn has_unread(ctx: &ReducerContext, recipient_guid: u64) -> bool {
    ctx.db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .any(|m| !m.was_read)
}

/// Seed one mail so the read path is demoable before anything can send. Operator-gated
/// like every debug reducer, and feature-gated out of a production build.
///
/// Plane-agnostic on purpose: call it against `lyracore-realm` on a sharded realm and against the
/// single database on a `lyracore dev up` fixture — it writes the same row either way, which is the
/// property the two-plane read is tested for.
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_seed_mail(
    ctx: &ReducerContext,
    recipient_guid: u64,
    sender_guid: u64,
    subject: String,
    body: String,
    money: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let id = insert_mail(
        ctx,
        recipient_guid,
        sender_guid,
        subject,
        body,
        money,
        0,
        &ItemSnapshot::default(),
    );
    spacetimedb::log::info!(
        "debug_seed_mail: mail {id} to {recipient_guid} from {sender_guid} (unread now: {})",
        has_unread(ctx, recipient_guid)
    );
    Ok(())
}

/// Take `copper` out of `payer_guid`'s purse, or refuse and take nothing. `unaffordable` is the
/// refusal text, because the postage and the cash-on-delivery price are different mistakes to a
/// player even though they are one debit to this function.
///
/// Reads the LIVE entity (`game_world_entity.money`), the way `trainer::buy` does: the durable
/// `game_character.money` is a mirror the logout persist writes, so debiting the row instead would
/// be overwritten by the payer's own session. Through `acting_entity_by_guid`, so a character
/// mid-transfer reads as absent rather than paying out of a purse that is in flight.
///
/// The refusal is the whole "a payer who cannot afford it is charged nothing" guarantee: it happens
/// before any write, in the same transaction as the debit that would follow.
pub(crate) fn debit_purse(
    ctx: &ReducerContext,
    payer_guid: u64,
    copper: u32,
    unaffordable: &str,
) -> Result<(), String> {
    let mut payer = crate::helpers::acting_entity_by_guid(ctx, payer_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_IN_WORLD.to_string())?;
    if payer.money < copper {
        return Err(unaffordable.to_string());
    }
    payer.money -= copper;
    ctx.db.game_world_entity().guid().update(payer);
    Ok(())
}

/// Write one sent letter and pay for it, in ONE transaction. **The single-database plane only.**
///
/// The whole cost — the postage plus any copper travelling with the letter — is one debit, computed
/// by [`lyracore_shared::mail::total_cost`] rather than passed in: the caller cannot be trusted to
/// have summed it, and a second copy of the sum is how the two planes drift. The debit happens
/// first and refuses atomically, which is the "a refused send costs the sender nothing" guarantee.
///
/// A sharded realm never reaches here: realm-core holds no purse, so the debit and the row cannot
/// share a transaction and the send goes through [`crate::mail_escrow`] instead — fence the copper
/// on the sender's own shard, commit the row here, settle last. Both planes still reach
/// [`insert_mail`], so the ROW is identical either way.
///
/// **This path must never route through [`crate::mail_escrow`], and the difference is not an
/// oversight to tidy up.** The escrow exists because two databases cannot share a transaction;
/// where they can, it is strictly worse — four transactions instead of one atomic write, plus a
/// fence that can be interrupted where nothing could be. Pinned by
/// `tests::the_single_database_money_paths_never_reach_the_escrow`.
/// The attachment is snapshotted BEFORE the debit and in the same transaction, so a send refused
/// for either reason costs the sender neither the coin nor the item.
///
/// `cod` is the price the RECIPIENT will owe for the attachment; it costs the sender nothing here
/// and is not part of the debit. The gateway has already dropped a price with nothing attached
/// (`lyracore_shared::mail::cod_at_send`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_send(
    ctx: &ReducerContext,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    cod: u32,
    item_guid: u64,
) -> Result<(), String> {
    let item = detach_item(ctx, sender_guid, item_guid)?;
    debit_purse(
        ctx,
        sender_guid,
        lyracore_shared::mail::total_cost(money),
        lyracore_shared::mail::NOT_ENOUGH_MONEY,
    )?;
    insert_mail(
        ctx,
        recipient_guid,
        sender_guid,
        subject,
        body,
        money,
        cod,
        &item,
    );
    Ok(())
}

/// What a `CMSG_MAIL_TAKE_MONEY` must do. Pure, so the authorization boundary and the
/// take-twice rule are testable without a database — and shared, so the single-database take and
/// the mail-plane fence that starts the sharded one cannot answer differently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TakeMoney {
    /// No such mail, or it is not the caller's. One verdict for both, like [`apply_mark_read`]'s:
    /// a crafted id must not be a way to learn which mail ids belong to somebody else.
    NotYours,
    /// The row holds no copper — a text-only letter, or one already emptied. The mail stays
    /// readable; only the money is gone.
    NothingToTake,
    /// Credit this much to the caller and clear the row's `money`.
    Take(u32),
}

/// `row` is the `(recipient_guid, money)` of the named mail, or `None` when no such row exists.
pub(crate) fn plan_take_money(row: Option<(u64, u32)>, caller_guid: u64) -> TakeMoney {
    match row {
        Some((recipient_guid, _)) if recipient_guid != caller_guid => TakeMoney::NotYours,
        Some((_, 0)) => TakeMoney::NothingToTake,
        Some((_, money)) => TakeMoney::Take(money),
        None => TakeMoney::NotYours,
    }
}

/// The named mail's `(recipient_guid, money)`. The read behind both takes — the single-database one
/// below and the mail-plane fence that starts the sharded one — so neither can invent its own idea
/// of who owns a row.
pub(crate) fn mail_money(ctx: &ReducerContext, mail_id: u64) -> Option<(u64, u32)> {
    ctx.db
        .game_mail()
        .id()
        .find(mail_id)
        .map(|m| (m.recipient_guid, m.money))
}

/// What a `CMSG_MAIL_TAKE_ITEM` must do. [`TakeMoney`]'s twin, and deliberately the same three
/// arms: the mail id is client-supplied either way, so the authorization boundary and the
/// take-twice rule are one rule pointed at two columns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TakeItem {
    /// No such mail, or it is not the caller's.
    NotYours,
    /// The letter carries no item — text-only, or one already taken. The mail stays readable.
    NothingToTake,
    /// Hand the snapshot to the caller and clear the row's attachment columns.
    Take,
}

/// `row` is the `(recipient_guid, item_entry)` of the named mail, or `None` when no such row exists.
pub(crate) fn plan_take_item(row: Option<(u64, u32)>, caller_guid: u64) -> TakeItem {
    match row {
        Some((recipient_guid, _)) if recipient_guid != caller_guid => TakeItem::NotYours,
        Some((_, 0)) => TakeItem::NothingToTake,
        Some(_) => TakeItem::Take,
        None => TakeItem::NotYours,
    }
}

/// The named mail's `(recipient_guid, attachment)`. [`mail_money`]'s twin, and the read behind both
/// item takes, so neither can invent its own idea of who owns a row.
pub(crate) fn mail_item(ctx: &ReducerContext, mail_id: u64) -> Option<(u64, ItemSnapshot)> {
    ctx.db
        .game_mail()
        .id()
        .find(mail_id)
        .map(|m| (m.recipient_guid, m.snapshot()))
}

/// Empty a mail of its attachment, leaving the letter readable until the recipient deletes it.
pub(crate) fn clear_mail_item(ctx: &ReducerContext, mail_id: u64) {
    let mails = ctx.db.game_mail();
    if let Some(row) = mails.id().find(mail_id) {
        mails.id().update(Mail {
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            ..row
        });
    }
}

/// Mark a mail's cash-on-delivery price as settled — the ONE write that makes "taking a COD mail
/// twice charges once" true, so it belongs in the same transaction as the payout it settles.
///
/// The rejected alternative was a `cod_paid: bool` beside the price: it keeps the number for the
/// window to render, and the window has nothing left to render once the attachment is gone.
pub(crate) fn clear_mail_cod(ctx: &ReducerContext, mail_id: u64) {
    let mails = ctx.db.game_mail();
    if let Some(mut row) = mails.id().find(mail_id) {
        row.cod = 0;
        mails.id().update(row);
    }
}

/// Empty a mail of its copper, leaving the letter readable until the recipient deletes it.
pub(crate) fn clear_mail_money(ctx: &ReducerContext, mail_id: u64) {
    let mails = ctx.db.game_mail();
    if let Some(mut row) = mails.id().find(mail_id) {
        row.money = 0;
        mails.id().update(row);
    }
}

/// A purse after a credit. Saturating, so a purse near `u32::MAX` cannot wrap down to nothing —
/// the same reason [`lyracore_shared::mail::total_cost`] saturates, in the other direction.
pub(crate) fn credited(purse: u32, amount: u32) -> u32 {
    purse.saturating_add(amount)
}

/// Take a mail's copper into the recipient's purse, in ONE transaction. **The single-database plane
/// only**, for the same reason as [`apply_send`]: on a sharded realm the row is on realm-core and
/// the purse is on the taker's own shard, so the take goes through [`crate::mail_escrow`] instead.
///
/// The clear and the credit are inseparable here, which is what makes "taking twice credits once"
/// structural rather than a check: the second call finds `money` already 0.
pub(crate) fn apply_take_money(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let money = match plan_take_money(mail_money(ctx, mail_id), recipient_guid) {
        TakeMoney::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        TakeMoney::NothingToTake => return Err(lyracore_shared::mail::NOTHING_TO_TAKE.to_string()),
        TakeMoney::Take(money) => money,
    };
    let mut taker = crate::helpers::acting_entity_by_guid(ctx, recipient_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_IN_WORLD.to_string())?;
    taker.money = credited(taker.money, money);
    ctx.db.game_world_entity().guid().update(taker);
    clear_mail_money(ctx, mail_id);
    Ok(())
}

/// Take a mail's item into the recipient's bags, paying any cash-on-delivery price for it, in ONE
/// transaction. **The single-database plane only**, for the same reason as [`apply_take_money`].
///
/// The grant runs BEFORE the clear, and everything here is one transaction — which is what makes
/// the full-bag case safe: [`grant_snapshot`]'s `Err` rolls the clear AND the debit back, so the
/// item stays in the letter and the taker keeps their gold.
///
/// # Cash on delivery
///
/// The debit, the grant and the seller's payout mail are the same transaction, so no partial
/// outcome exists: nobody gets an item nobody paid for, and nobody pays for one that never arrived.
/// The debit goes FIRST because it is the refusal a player can act on — a taker who cannot afford
/// the price is told so with the item untouched, free to return the mail instead. Clearing the
/// price is what makes a second click charge nothing.
pub(crate) fn apply_take_item(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let row = ctx.db.game_mail().id().find(mail_id);
    match plan_take_item(
        row.as_ref().map(|m| (m.recipient_guid, m.item_entry)),
        recipient_guid,
    ) {
        TakeItem::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        TakeItem::NothingToTake => return Err(lyracore_shared::mail::NOTHING_TO_TAKE.to_string()),
        TakeItem::Take => {}
    }
    let row = row.expect("Take is only reachable with a row");
    let settlement = lyracore_shared::mail::cod_settlement(
        row.cod,
        row.sender_guid,
        &row.subject,
        row.recipient_guid,
    );
    if let Some(s) = &settlement {
        debit_purse(
            ctx,
            s.payer_guid,
            s.copper,
            lyracore_shared::mail::COD_NOT_AFFORDABLE,
        )?;
    }
    grant_snapshot(ctx, recipient_guid, &row.snapshot())?;
    clear_mail_item(ctx, mail_id);
    if let Some(s) = settlement {
        clear_mail_cod(ctx, mail_id);
        insert_mail(
            ctx,
            s.payee_guid,
            s.payer_guid,
            s.subject,
            String::new(),
            s.copper,
            0,
            &ItemSnapshot::default(),
        );
    }
    Ok(())
}

/// Flip `mail_id`'s read state to read, for `recipient_guid`. A no-op (not an error) if it is
/// already read, so a repeated click never fails. `Err` merges "no such mail" with "not yours" —
/// see [`lyracore_shared::mail::NOT_YOUR_MAIL`] — because the row lookup is the ONLY authorization
/// this write has: whichever plane runs it, the caller passed no live entity to check against.
pub(crate) fn apply_mark_read(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let mails = ctx.db.game_mail();
    let row = mails
        .id()
        .find(mail_id)
        .filter(|m| m.recipient_guid == recipient_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_YOUR_MAIL.to_string())?;
    if !row.was_read {
        let mut row = row;
        row.was_read = true;
        mails.id().update(row);
    }
    Ok(())
}

/// Delete `mail_id` for `recipient_guid`. Destroys any attachment the row still carries — the
/// attachment lives in the row's own columns (no child row, no separate escrow), so there is
/// nothing left to separately clean up. Vanilla's confirmation prompt is client-side; the server
/// enforces only that the mail is the caller's.
pub(crate) fn apply_delete(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let mails = ctx.db.game_mail();
    let row = mails
        .id()
        .find(mail_id)
        .filter(|m| m.recipient_guid == recipient_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_YOUR_MAIL.to_string())?;
    mails.id().delete(row.id);
    Ok(())
}

/// What a `CMSG_MAIL_RETURN_TO_SENDER` must do. [`TakeItem`]'s shape: the caller's guid must match
/// the row's recipient, and a crafted id must not read any differently than a nonexistent one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReturnPlan {
    /// No such mail, or it is not the caller's.
    NotYours,
    /// Re-address it to whoever sent it.
    Return,
}

/// `recipient_guid` is the named mail's `recipient_guid`, or `None` when no such row exists.
pub(crate) fn plan_return(recipient_guid: Option<u64>, caller_guid: u64) -> ReturnPlan {
    match recipient_guid {
        Some(r) if r == caller_guid => ReturnPlan::Return,
        _ => ReturnPlan::NotYours,
    }
}

/// Return `mail_id` to whoever sent it: the SAME row, re-addressed in place. Never the escrow —
/// the row does not leave the plane that already holds it, so there is nothing to move across a
/// database boundary, only sender/recipient to swap.
///
/// Whatever the row still carries travels unchanged, including nothing at all: a mail already
/// emptied by [`apply_take_item`] or [`apply_take_money`] has nothing left to duplicate, because
/// those already cleared the attachment/money columns before this ever runs. `was_read` resets —
/// the letter is new mail to the original sender.
///
/// **The price does NOT travel.** A returned COD mail is addressed back to the character who set
/// the price, so carrying it would charge them their own asking price to get their own item back —
/// and pay it to the buyer who declined. Vanilla clears it here too. Declining a purchase must cost
/// the seller the sale and nothing else.
pub(crate) fn apply_return(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let mails = ctx.db.game_mail();
    let row = mails.id().find(mail_id);
    match plan_return(row.as_ref().map(|m| m.recipient_guid), recipient_guid) {
        ReturnPlan::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        ReturnPlan::Return => {}
    }
    let row = row.expect("Return is only reachable with a row");
    mails.id().update(Mail {
        recipient_guid: row.sender_guid,
        sender_guid: recipient_guid,
        was_read: false,
        cod: 0,
        ..row
    });
    Ok(())
}

/// The mail write surface's entry point (the mark-read/delete slice) — the first `realm_mail_*`
/// reducer. Mirrors [`crate::chat::realm_whisper`]'s shape and its reason: mail is
/// addressed by guid on a database that may hold no live entity for the caller, so this reducer
/// takes `recipient_guid` explicitly instead of deriving it from `ctx.sender()`.
///
/// **Operator-gated, and it has to be** — the same trust boundary `realm_whisper` sits on. A client
/// that could call this directly would flip the read state of anybody's mail in the realm; the
/// `recipient_guid` argument is the ENTIRE authorization otherwise, and only the gateway is trusted
/// to have already resolved it to the caller's own guid.
///
/// One reducer serves BOTH planes, unlike whisper's `realm_whisper`/`gw_send_whisper` split: the
/// mail READ path already treats realm-core and the single-database fallback symmetrically (one
/// `mail_list` read, asked of whichever handle owns the rows — `world::mail::mail_of`), because
/// unlike a whisper's name resolution, addressing a mail row needs no plane-specific lookup. The
/// gateway calls this same reducer on whichever handle `world::mail` picked, so the write cannot
/// drift from the read about which database is authoritative.
#[reducer]
pub fn realm_mail_mark_read(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_mark_read(ctx, recipient_guid, mail_id)
}

/// Post one letter, as `sender_guid`, to `recipient_guid`, paying for it in the same transaction.
/// **The single-database plane's whole send.**
///
/// **Operator-gated, and it has to be** — harder than the two above. Every gate that decides WHO may
/// write to whom (the recipient exists, the sender is in world, same faction, not yourself, standing
/// at a mailbox) runs in the gateway, because realm-core holds no characters and no gameobjects to
/// answer them with. So `sender_guid` here is not a claim this transaction can check: a client that
/// could call this would post mail as anybody on the realm, and the gateway passes only the guid it
/// authenticated for that socket.
///
/// A sharded realm calls [`crate::mail_escrow::realm_mail_fence`] and friends instead; the postage
/// rides that fence, which is why no separate postage reducer exists any more.
#[reducer]
#[allow(clippy::too_many_arguments)] // a reducer's arguments are the wire
pub fn realm_mail_send(
    ctx: &ReducerContext,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    cod: u32,
    item_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_send(
        ctx,
        sender_guid,
        recipient_guid,
        subject,
        body,
        money,
        cod,
        item_guid,
    )
}

/// Take a mail's attached item into `recipient_guid`'s bags. **The single-database plane's whole
/// take**, and the item twin of [`realm_mail_take_money`] down to the trust boundary.
#[reducer]
pub fn realm_mail_take_item(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_take_item(ctx, recipient_guid, mail_id)
}

/// Take a mail's copper into `recipient_guid`'s purse. **The single-database plane's whole take.**
///
/// Operator-gated, same boundary as its siblings: the row lookup scoped to `recipient_guid` is the
/// entire authorization, and only the gateway is trusted to have resolved that guid to the socket
/// it authenticated.
#[reducer]
pub fn realm_mail_take_money(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_take_money(ctx, recipient_guid, mail_id)
}

/// Has `payee_guid` room in their bags for one more item? A READ dressed as a reducer, because the
/// gateway cannot answer it without a second copy of the bag-space search.
///
/// **A sharded item take probes here BEFORE it fences.** The fence is a one-way move, so a full bag
/// discovered afterwards would strand the item in an escrow instead of leaving it in the letter.
/// The probe is advisory — a bag filled between the probe and the grant leaves the item held and
/// re-driven, never destroyed — but it is what makes the ordinary full-bag click a clean refusal
/// with the item still in the mail. The single-database plane needs none of this: there the grant
/// and the clear are one transaction, so the refusal rolls itself back.
#[reducer]
pub fn realm_mail_item_room(ctx: &ReducerContext, payee_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if crate::helpers::acting_entity_by_guid(ctx, payee_guid).is_none() {
        return Err(lyracore_shared::mail::NOT_IN_WORLD.to_string());
    }
    if !crate::items::has_free_slot(ctx, payee_guid) {
        return Err(lyracore_shared::mail::INVENTORY_FULL.to_string());
    }
    Ok(())
}

/// [`realm_mail_mark_read`]'s twin for delete. Same trust boundary, same one-reducer-both-planes
/// shape.
#[reducer]
pub fn realm_mail_delete(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_delete(ctx, recipient_guid, mail_id)
}

/// [`realm_mail_delete`]'s twin for return-to-sender. Same trust boundary, same one-reducer-both-
/// planes shape — the row never crosses a database, so returning it needs no escrow.
#[reducer]
pub fn realm_mail_return(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_return(ctx, recipient_guid, mail_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::read_scanned;

    /// **The authorization boundary on a take.** A mail id is client-supplied, so the row lookup
    /// scoped to the caller is the only thing between a crafted packet and another player's purse —
    /// and "no such mail" answers the same as "not yours", so the refusal leaks no id.
    #[test]
    fn taking_money_is_refused_for_a_mail_the_caller_is_not_the_recipient_of() {
        assert_eq!(plan_take_money(Some((7, 100)), 7), TakeMoney::Take(100));
        assert_eq!(plan_take_money(Some((7, 100)), 8), TakeMoney::NotYours);
        assert_eq!(plan_take_money(None, 7), TakeMoney::NotYours);
    }

    /// **Taking twice credits once.** The first take clears the row, so the second finds nothing —
    /// and the mail itself survives, because only the money was in it.
    #[test]
    fn a_mail_already_emptied_of_money_has_nothing_left_to_take() {
        assert_eq!(plan_take_money(Some((7, 0)), 7), TakeMoney::NothingToTake);
    }

    /// **The mailable-or-refused verdict, and the authorization boundary on an attachment.** An
    /// item guid is client-supplied, so "not yours" and "no such guid" answer identically — a
    /// crafted packet must not be able to mail someone else's gear, nor to learn which guids exist.
    #[test]
    fn attaching_an_item_the_sender_does_not_own_is_refused() {
        assert_eq!(plan_attach(4, Some((7, false)), 7), Attach::Detach);
        assert_eq!(plan_attach(4, Some((8, false)), 7), Attach::NotYours);
        assert_eq!(plan_attach(4, None, 7), Attach::NotYours);
    }

    /// **A bound instance is refused; an unworn bind-on-equip one is not.** The verdict reads the
    /// INSTANCE's `soulbound`, never the template's `bonding` — a BoE item that has never been
    /// equipped is unbound and is exactly the drop a player mails to an alt.
    #[test]
    fn a_soulbound_instance_is_refused_and_an_unworn_bind_on_equip_item_is_mailable() {
        assert_eq!(plan_attach(4, Some((7, true)), 7), Attach::Soulbound);
        assert_eq!(plan_attach(4, Some((7, false)), 7), Attach::Detach);
    }

    /// Guid 0 is the wire's "no item attached", not a guid to look up. A letter with no attachment
    /// must not be refused as "you do not own item 0".
    #[test]
    fn item_guid_zero_means_no_attachment_rather_than_a_missing_item() {
        assert_eq!(plan_attach(0, None, 7), Attach::Nothing);
        assert_eq!(plan_attach(0, Some((8, true)), 7), Attach::Nothing);
    }

    /// **The authorization boundary on an item take**, and the take-twice rule — [`plan_take_money`]'s
    /// twin, pinned separately because the two read different columns and could drift apart.
    #[test]
    fn taking_an_item_is_refused_for_a_mail_the_caller_is_not_the_recipient_of() {
        assert_eq!(plan_take_item(Some((7, 509_0001)), 7), TakeItem::Take);
        assert_eq!(plan_take_item(Some((7, 509_0001)), 8), TakeItem::NotYours);
        assert_eq!(plan_take_item(None, 7), TakeItem::NotYours);
        assert_eq!(plan_take_item(Some((7, 0)), 7), TakeItem::NothingToTake);
    }

    /// A snapshot with no entry is the empty attachment every text-only letter carries.
    #[test]
    fn an_attachment_with_no_entry_is_empty() {
        assert!(ItemSnapshot::default().is_empty());
        assert!(!ItemSnapshot {
            entry: 509_0001,
            ..Default::default()
        }
        .is_empty());
    }

    /// **The authorization boundary on a return.** A mail id is client-supplied, so the row lookup
    /// scoped to the caller is the only thing between a crafted packet and re-addressing someone
    /// else's mail — "no such mail" and "not yours" answer the same, leaking no id.
    #[test]
    fn returning_a_mail_is_refused_for_a_caller_who_is_not_the_recipient() {
        assert_eq!(plan_return(Some(7), 7), ReturnPlan::Return);
        assert_eq!(plan_return(Some(7), 8), ReturnPlan::NotYours);
        assert_eq!(plan_return(None, 7), ReturnPlan::NotYours);
    }

    /// A credit saturates rather than wrapping — the mirror of the debit's saturation, and the
    /// arithmetic that would otherwise turn a rich player's purse into an empty one.
    #[test]
    fn a_credited_purse_saturates_rather_than_wrapping() {
        assert_eq!(credited(100, 30), 130);
        assert_eq!(credited(u32::MAX, 1), u32::MAX);
    }

    /// **One rule, two planes.** The single-database send debits
    /// `lyracore_shared::mail::total_cost`; the sharded one debits the escrow's
    /// `Letter::fenced_copper`. They are two expressions of "postage plus the attached coin, in one
    /// debit", and a drift between them is a plane that charges differently for the same letter.
    #[test]
    fn both_planes_charge_the_same_total_for_the_same_letter() {
        for money in [0, 1, 100, u32::MAX - 1, u32::MAX] {
            let letter = crate::mail_escrow::Letter {
                recipient_guid: 1,
                subject: String::new(),
                body: String::new(),
                money,
                postage: lyracore_shared::mail::postage(),
                cod: 0,
            };
            assert_eq!(
                lyracore_shared::mail::total_cost(money),
                letter.fenced_copper(),
                "the two planes must charge the same for {money} copper attached"
            );
        }
    }

    /// **The mailbox proximity gate must never scan the spatial gameobject table.**
    ///
    /// The client passes the mailbox guid in every mail packet, so the gate is a PK lookup plus a
    /// map/instance/range check — `gameobject::usable_go`'s shape. `game_gameobject` is sharded, so
    /// an `.iter()` there returns a silent subset and a mailbox would work or not depending on
    /// which database the session happens to read. The module half is covered by
    /// `tripwires::partition_discipline_tripwire` (this file has no budget there); the gate itself
    /// runs in the GATEWAY, which that scan cannot see, so it is pinned from here.
    #[test]
    fn no_mail_code_path_iterates_the_spatial_gameobject_table() {
        for path in [
            "module/src/mail.rs",
            "gateway/src/stdb/reads/mail.rs",
            "gateway/src/world/mail.rs",
            "gateway/src/world/handlers/mail.rs",
        ] {
            // Built at runtime so this test's own source — which names both — can never match.
            let table = format!("{}{}", "game_gameobject", "()");
            let scan = format!("{}{}", ".iter", "()");
            let src = read_scanned(path).expect("module/ and gateway/ ship in every checkout");
            for (n, line) in src.lines().enumerate() {
                // Comment-stripped, so a scan that pins prose cannot pass for a scan that pins code.
                let code = line.split("//").next().unwrap_or(line);
                assert!(
                    !(code.contains(&table) && code.contains(&scan)),
                    "{path}:{} iterates the SPATIAL gameobject table. Sharding makes that a silent \
                     subset — the client names the mailbox guid, so resolve it by PK \
                     (`game_gameobject().guid().find(..)`) and range-check against the player's own \
                     entity, the shape `gameobject::usable_go` uses.",
                    n + 1
                );
            }
        }
    }

    /// **The single-database plane does the whole send in ONE transaction and does not go through
    /// the escrow.** A `lyracore dev up` gateway holds the purse and the mail row on one database,
    /// so the debit and the insert are already atomic; routing them through a fence would buy
    /// nothing and add three interruption points that cannot exist there.
    ///
    /// A source scan because the body needs a `ReducerContext` to execute. It is the cheapest
    /// available guard against the tidy-up that would "unify the two paths" — which reads like
    /// simplification and is a regression.
    #[test]
    fn the_single_database_money_paths_never_reach_the_escrow() {
        for signature in [
            "pub(crate) fn apply_send(",
            "pub(crate) fn apply_take_money(",
            "pub(crate) fn apply_take_item(",
        ] {
            let body = code_of(include_str!("mail.rs"), signature);
            assert!(
                !body.contains("escrow"),
                "`{signature}` is the ONE-TRANSACTION plane. The escrow is the mechanism for the \
                 case where a transaction cannot span the two databases; here one can. Body \
                 was:\n{body}"
            );
        }
    }

    /// **A returned COD mail must not charge its own writer.** The return re-addresses the row to
    /// the character who SET the price, so a price that travelled would bill the seller their own
    /// asking price to get their own item back — and pay it to the buyer who declined.
    ///
    /// A source scan because the body needs a `ReducerContext` to execute, and because the fact is
    /// one field in an update literal: exactly the line a later edit deletes as redundant.
    #[test]
    fn a_returned_mail_carries_no_cash_on_delivery_price() {
        let body = code_of(include_str!("mail.rs"), "pub(crate) fn apply_return(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.contains("cod: 0,"),
            "`apply_return` no longer zeroes the COD price. Returning a priced mail sends it back \
             to whoever set the price, so a price that survives is charged to the seller and paid \
             to the buyer who refused it. Body was:\n{body}"
        );
    }

    // ---- `realm_mail_mark_read` / `realm_mail_delete`'s operator gate ----
    //
    // A reducer body needs a live `ReducerContext`, so neither can be EXECUTED by a test in this
    // crate — same reason, same technique, as `chat.rs`'s pair for `realm_whisper`.

    use crate::test_scan::code_of;

    /// **The operator gate is the entire authorization of the mail write surface.** Every reducer
    /// here takes the acting character's guid as an argument rather than deriving it from
    /// `ctx.sender()`, so without the gate any identity that can reach the node flips the read state
    /// of, deletes, or POSTS AS anybody's mail in the realm — and empties their purse for the
    /// postage.
    ///
    /// Asserted as the FIRST STATEMENT of the body, not merely present in it — see
    /// `chat.rs::the_realm_whisper_reducer_is_operator_gated` for why a bare `contains` is not
    /// enough (a neutered gate, `if false { .. }` or an early return above it, still contains the
    /// text).
    #[test]
    fn the_realm_mail_write_reducers_are_operator_gated() {
        for signature in [
            "pub fn realm_mail_mark_read(",
            "pub fn realm_mail_delete(",
            "pub fn realm_mail_return(",
            "pub fn realm_mail_send(",
            "pub fn realm_mail_take_money(",
            "pub fn realm_mail_take_item(",
            "pub fn realm_mail_item_room(",
        ] {
            let body = code_of(include_str!("mail.rs"), signature);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{signature}` no longer OPENS with the operator gate. It takes the caller's guid \
                 as an argument, so the gate is the only thing between an arbitrary connection and \
                 mutating anybody's mailbox in the realm. Body was:\n{body}"
            );
        }
    }
}
