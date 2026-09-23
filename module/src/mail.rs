//! Durable mail rows and single-database mailbox operations.
//! Attachments are item snapshots; cross-database moves live in `mail_escrow`.

use lyracore_shared::mail::{MailSender, CHECK_MASK_RETURNED};
use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use crate::game_item_instance;
use crate::game_item_template;
use crate::game_world_entity;
use crate::items::ItemSnapshot;
#[table(accessor = game_mail, public, index(accessor = by_recipient, btree(columns = [recipient_guid])))]
pub struct Mail {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_guid: u64,
    pub sender_guid: u64,
    pub subject: String,
    pub body: String,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub money: u32,
    pub cod: u32,
    pub was_read: bool,
    pub created_at: Timestamp,
    #[default(0)]
    pub random_property_id: u32,
    /// The vanilla `MailMessageType` code of the [`MailSender`]. 0 is a Character.
    #[default(0u8)]
    pub sender_kind: u8,
    /// The auction house id, or the creature or gameobject entry. 0 for a Character.
    #[default(0u32)]
    pub sender_entry: u32,
    /// `CHECK_MASK_*` bits other than READ, which is `was_read`.
    #[default(0u32)]
    pub check_flags: u32,
    #[default(0u32)]
    pub mail_template_id: u32,
    /// When the recipient can first see the mail. 0 means from creation.
    #[default(0i64)]
    pub deliver_micros: i64,
}
crate::character_owned!(delete, fn sweep_delete_game_mail(ctx, character_guid) {
    let mails = ctx.db.game_mail();
    for r in mails.by_recipient().filter(&character_guid).collect::<Vec<_>>() {
        mails.id().delete(r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_mail(ctx, character_guid, io) {
    table = game_mail,
    by = by_recipient,
    remint = id,
});
impl Mail {
    pub(crate) fn sender(&self) -> MailSender {
        MailSender::from_columns(self.sender_kind, self.sender_guid, self.sender_entry)
    }
    pub(crate) fn is_delivered(&self, now: Timestamp) -> bool {
        self.deliver_micros <= now.to_micros_since_unix_epoch()
    }
    pub(crate) fn snapshot(&self) -> ItemSnapshot {
        ItemSnapshot {
            entry: self.item_entry,
            stack_count: self.item_stack_count,
            durability: self.item_durability,
            enchant_id: self.item_enchant_id,
            soulbound: self.item_soulbound,
            random_property_id: self.random_property_id,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Attach {
    Nothing,
    NotYours,
    Soulbound,
    Detach,
}
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
    let snapshot = ItemSnapshot::from(&inst);
    items.guid().delete(item_guid);
    Ok(snapshot)
}
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
    crate::items::store_instance_state(ctx, payee_guid, payee.owner_identity, &tmpl, None, snapshot)
}
/// Everything a mail row is created from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Letter {
    pub recipient_guid: u64,
    pub sender: MailSender,
    pub subject: String,
    pub body: String,
    pub money: u32,
    pub cod: u32,
    pub item: ItemSnapshot,
    pub mail_template_id: u32,
    /// `CHECK_MASK_*` bits other than READ.
    pub check_flags: u32,
    /// When the recipient can first see the mail. 0 means from creation.
    pub deliver_micros: i64,
}

impl Letter {
    /// A letter from a character. It has HAS_BODY when it has a body and COPIED when it has none,
    /// so the client offers no copy of an empty letter (cmangos `MailHandler.cpp:264`).
    pub(crate) fn from_character(
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item: ItemSnapshot,
    ) -> Self {
        let check_flags = if body.is_empty() {
            lyracore_shared::mail::CHECK_MASK_COPIED
        } else {
            lyracore_shared::mail::CHECK_MASK_HAS_BODY
        };
        Self {
            recipient_guid,
            sender: MailSender::Character(sender_guid),
            subject,
            body,
            money,
            cod,
            item,
            mail_template_id: 0,
            check_flags,
            deliver_micros: 0,
        }
    }
    /// The same letter as the payment for a cash on delivery take. It keeps the priced letter's
    /// subject, and the client shows "COD Payment: <subject>" (cmangos `MailHandler.cpp:475-477`).
    pub(crate) fn into_cod_payment(self) -> Self {
        Self {
            check_flags: lyracore_shared::mail::CHECK_MASK_COD_PAYMENT,
            ..self
        }
    }
}
/// The one way a mail row is created.
pub(crate) fn insert_letter(ctx: &ReducerContext, letter: Letter) -> u64 {
    let (sender_kind, sender_guid, sender_entry) = letter.sender.columns();
    ctx.db
        .game_mail()
        .insert(Mail {
            id: 0,
            recipient_guid: letter.recipient_guid,
            sender_guid,
            subject: letter.subject,
            body: letter.body,
            item_entry: letter.item.entry,
            item_stack_count: letter.item.stack_count,
            item_durability: letter.item.durability,
            item_enchant_id: letter.item.enchant_id,
            item_soulbound: letter.item.soulbound,
            random_property_id: letter.item.random_property_id,
            money: letter.money,
            cod: letter.cod,
            was_read: false,
            created_at: ctx.timestamp,
            sender_kind,
            sender_entry,
            check_flags: letter.check_flags,
            mail_template_id: letter.mail_template_id,
            deliver_micros: letter.deliver_micros,
        })
        .id
}
/// The mail as its recipient can act on it. Before its delivery instant it does not exist for
/// them yet (cmangos `MailHandler.cpp:359,417,516`).
pub(crate) fn delivered_mail(ctx: &ReducerContext, mail_id: u64) -> Option<Mail> {
    ctx.db
        .game_mail()
        .id()
        .find(mail_id)
        .filter(|m| m.is_delivered(ctx.timestamp))
}
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn has_unread(ctx: &ReducerContext, recipient_guid: u64) -> bool {
    ctx.db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .any(|m| !m.was_read && m.is_delivered(ctx.timestamp))
}
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
    let id = insert_letter(
        ctx,
        Letter::from_character(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            0,
            ItemSnapshot::default(),
        ),
    );
    spacetimedb::log::info!(
        "debug_seed_mail: mail {id} to {recipient_guid} from {sender_guid} (unread now: {})",
        has_unread(ctx, recipient_guid)
    );
    Ok(())
}
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
    insert_letter(
        ctx,
        Letter::from_character(sender_guid, recipient_guid, subject, body, money, cod, item),
    );
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TakeMoney {
    NotYours,
    NothingToTake,
    Take(u32),
}
pub(crate) fn plan_take_money(row: Option<(u64, u32)>, caller_guid: u64) -> TakeMoney {
    match row {
        Some((recipient_guid, _)) if recipient_guid != caller_guid => TakeMoney::NotYours,
        Some((_, 0)) => TakeMoney::NothingToTake,
        Some((_, money)) => TakeMoney::Take(money),
        None => TakeMoney::NotYours,
    }
}
pub(crate) fn mail_money(ctx: &ReducerContext, mail_id: u64) -> Option<(u64, u32)> {
    delivered_mail(ctx, mail_id).map(|m| (m.recipient_guid, m.money))
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TakeItem {
    NotYours,
    NothingToTake,
    Take,
}
pub(crate) fn plan_take_item(row: Option<(u64, u32)>, caller_guid: u64) -> TakeItem {
    match row {
        Some((recipient_guid, _)) if recipient_guid != caller_guid => TakeItem::NotYours,
        Some((_, 0)) => TakeItem::NothingToTake,
        Some(_) => TakeItem::Take,
        None => TakeItem::NotYours,
    }
}
pub(crate) fn mail_item(ctx: &ReducerContext, mail_id: u64) -> Option<(u64, ItemSnapshot)> {
    delivered_mail(ctx, mail_id).map(|m| (m.recipient_guid, m.snapshot()))
}
pub(crate) fn clear_mail_item(ctx: &ReducerContext, mail_id: u64) {
    let mails = ctx.db.game_mail();
    if let Some(row) = mails.id().find(mail_id) {
        mails.id().update(Mail {
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            random_property_id: 0,
            ..row
        });
    }
}
pub(crate) fn clear_mail_cod(ctx: &ReducerContext, mail_id: u64) {
    let mails = ctx.db.game_mail();
    if let Some(mut row) = mails.id().find(mail_id) {
        row.cod = 0;
        mails.id().update(row);
    }
}
pub(crate) fn clear_mail_money(ctx: &ReducerContext, mail_id: u64) {
    let mails = ctx.db.game_mail();
    if let Some(mut row) = mails.id().find(mail_id) {
        row.money = 0;
        mails.id().update(row);
    }
}
pub(crate) fn credited(purse: u32, amount: u32) -> u32 {
    purse.saturating_add(amount)
}
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
pub(crate) fn apply_take_item(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let row = delivered_mail(ctx, mail_id);
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
        insert_letter(
            ctx,
            Letter::from_character(
                s.payer_guid,
                s.payee_guid,
                s.subject,
                String::new(),
                s.copper,
                0,
                ItemSnapshot::default(),
            )
            .into_cod_payment(),
        );
    }
    Ok(())
}
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeletePlan {
    NotYours,
    CodPriced,
    Delete,
}
/// The client offers no Delete on a priced mail, so the Gate refuses a crafted one
/// (cmangos `MailHandler.cpp:327-331`).
pub(crate) fn plan_delete(row: Option<&Mail>, caller_guid: u64) -> DeletePlan {
    match row {
        Some(m) if m.recipient_guid != caller_guid => DeletePlan::NotYours,
        Some(m) if m.cod > 0 => DeletePlan::CodPriced,
        Some(_) => DeletePlan::Delete,
        None => DeletePlan::NotYours,
    }
}
pub(crate) fn apply_delete(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let mails = ctx.db.game_mail();
    match plan_delete(mails.id().find(mail_id).as_ref(), recipient_guid) {
        DeletePlan::NotYours => Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        DeletePlan::CodPriced => Err(lyracore_shared::mail::COD_MAIL_UNDELETABLE.to_string()),
        DeletePlan::Delete => {
            mails.id().delete(mail_id);
            Ok(())
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReturnPlan {
    NotYours,
    NoCharacterSender,
    AlreadyReturned,
    Return,
}
/// Only a Character's mail goes back, and only once. These are LyraCore Gates, not cmangos server
/// behavior: cmangos accepts every return and silently drops a mail with no Character sender
/// (`MailHandler.cpp:375`), and it relies on the client hiding Return on a returned mail. The
/// client hides Return on each mail these Gates refuse, so a refusal answers a crafted packet.
pub(crate) fn plan_return(row: Option<&Mail>, caller_guid: u64) -> ReturnPlan {
    let Some(row) = row.filter(|m| m.recipient_guid == caller_guid) else {
        return ReturnPlan::NotYours;
    };
    if !matches!(row.sender(), MailSender::Character(guid) if guid != 0) {
        return ReturnPlan::NoCharacterSender;
    }
    if row.check_flags & CHECK_MASK_RETURNED != 0 {
        return ReturnPlan::AlreadyReturned;
    }
    ReturnPlan::Return
}
pub(crate) fn apply_return(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    let row = delivered_mail(ctx, mail_id);
    match plan_return(row.as_ref(), recipient_guid) {
        ReturnPlan::NotYours => return Err(lyracore_shared::mail::NOT_YOUR_MAIL.to_string()),
        ReturnPlan::NoCharacterSender => {
            return Err(lyracore_shared::mail::NO_SENDER_TO_RETURN_TO.to_string())
        }
        ReturnPlan::AlreadyReturned => {
            return Err(lyracore_shared::mail::ALREADY_RETURNED.to_string())
        }
        ReturnPlan::Return => {}
    }
    let row = row.expect("Return is only reachable with a row");
    ctx.db.game_mail().id().update(returned(row, ctx.timestamp));
    Ok(())
}
/// `row` sent back to its sender at `now`. It carries only RETURNED, loses its price and read
/// state, and arrives now, which restarts its expiry clock (cmangos `Mail.cpp:264,299-313`).
pub(crate) fn returned(row: Mail, now: Timestamp) -> Mail {
    Mail {
        recipient_guid: row.sender_guid,
        sender_guid: row.recipient_guid,
        was_read: false,
        cod: 0,
        check_flags: CHECK_MASK_RETURNED,
        deliver_micros: now.to_micros_since_unix_epoch(),
        ..row
    }
}
#[reducer]
pub fn realm_mail_mark_read(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_mark_read(ctx, recipient_guid, mail_id)
}
#[reducer]
#[allow(clippy::too_many_arguments)] // a reducer's arguments are the wire
pub fn realm_mail_send(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    cod: u32,
    item_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let sender_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
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
#[reducer]
pub fn realm_mail_take_item(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_take_item(ctx, recipient_guid, mail_id)
}
#[reducer]
pub fn realm_mail_take_money(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_take_money(ctx, recipient_guid, mail_id)
}
#[reducer]
pub fn realm_mail_item_room(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let payee_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    if crate::helpers::acting_entity_by_guid(ctx, payee_guid).is_none() {
        return Err(lyracore_shared::mail::NOT_IN_WORLD.to_string());
    }
    if !crate::items::has_free_slot(ctx, payee_guid) {
        return Err(lyracore_shared::mail::INVENTORY_FULL.to_string());
    }
    Ok(())
}
#[reducer]
pub fn realm_mail_delete(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_delete(ctx, recipient_guid, mail_id)
}
#[reducer]
pub fn realm_mail_return(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let recipient_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_return(ctx, recipient_guid, mail_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::read_scanned;

    #[test]
    fn taking_money_is_refused_for_a_mail_the_caller_is_not_the_recipient_of() {
        assert_eq!(plan_take_money(Some((7, 100)), 7), TakeMoney::Take(100));
        assert_eq!(plan_take_money(Some((7, 100)), 8), TakeMoney::NotYours);
        assert_eq!(plan_take_money(None, 7), TakeMoney::NotYours);
    }

    #[test]
    fn a_mail_already_emptied_of_money_has_nothing_left_to_take() {
        assert_eq!(plan_take_money(Some((7, 0)), 7), TakeMoney::NothingToTake);
    }

    #[test]
    fn attaching_an_item_the_sender_does_not_own_is_refused() {
        assert_eq!(plan_attach(4, Some((7, false)), 7), Attach::Detach);
        assert_eq!(plan_attach(4, Some((8, false)), 7), Attach::NotYours);
        assert_eq!(plan_attach(4, None, 7), Attach::NotYours);
    }

    #[test]
    fn a_soulbound_instance_is_refused_and_an_unworn_bind_on_equip_item_is_mailable() {
        assert_eq!(plan_attach(4, Some((7, true)), 7), Attach::Soulbound);
        assert_eq!(plan_attach(4, Some((7, false)), 7), Attach::Detach);
    }

    #[test]
    fn item_guid_zero_means_no_attachment_rather_than_a_missing_item() {
        assert_eq!(plan_attach(0, None, 7), Attach::Nothing);
        assert_eq!(plan_attach(0, Some((8, true)), 7), Attach::Nothing);
    }

    #[test]
    fn taking_an_item_is_refused_for_a_mail_the_caller_is_not_the_recipient_of() {
        assert_eq!(plan_take_item(Some((7, 509_0001)), 7), TakeItem::Take);
        assert_eq!(plan_take_item(Some((7, 509_0001)), 8), TakeItem::NotYours);
        assert_eq!(plan_take_item(None, 7), TakeItem::NotYours);
        assert_eq!(plan_take_item(Some((7, 0)), 7), TakeItem::NothingToTake);
    }

    #[test]
    fn an_attachment_with_no_entry_is_empty() {
        assert!(ItemSnapshot::default().is_empty());
        assert!(!ItemSnapshot {
            entry: 509_0001,
            ..Default::default()
        }
        .is_empty());
    }

    fn row(recipient_guid: u64, sender: MailSender) -> Mail {
        let (sender_kind, sender_guid, sender_entry) = sender.columns();
        Mail {
            id: 1,
            recipient_guid,
            sender_guid,
            subject: "Your sword".into(),
            body: String::new(),
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            money: 0,
            cod: 0,
            was_read: false,
            created_at: Timestamp::from_micros_since_unix_epoch(1_000_000),
            random_property_id: 0,
            sender_kind,
            sender_entry,
            check_flags: 0,
            mail_template_id: 0,
            deliver_micros: 0,
        }
    }

    #[test]
    fn returning_a_mail_is_refused_for_a_caller_who_is_not_the_recipient() {
        let mail = row(7, MailSender::Character(9));
        assert_eq!(plan_return(Some(&mail), 7), ReturnPlan::Return);
        assert_eq!(plan_return(Some(&mail), 8), ReturnPlan::NotYours);
        assert_eq!(plan_return(None, 7), ReturnPlan::NotYours);
    }

    #[test]
    fn only_a_mail_from_a_character_with_a_guid_can_be_returned() {
        for sender in [
            MailSender::Character(0),
            MailSender::AuctionHouse(7),
            MailSender::Creature(11_811),
            MailSender::Gameobject(176_582),
        ] {
            assert_eq!(
                plan_return(Some(&row(7, sender)), 7),
                ReturnPlan::NoCharacterSender,
                "{sender:?} has nobody to take the mail back"
            );
        }
    }

    #[test]
    fn a_returned_mail_cannot_be_returned_again() {
        let returned = Mail {
            check_flags: lyracore_shared::mail::CHECK_MASK_RETURNED,
            ..row(7, MailSender::Character(9))
        };
        assert_eq!(plan_return(Some(&returned), 7), ReturnPlan::AlreadyReturned);
        let with_body = Mail {
            check_flags: lyracore_shared::mail::CHECK_MASK_HAS_BODY,
            ..row(7, MailSender::Character(9))
        };
        assert_eq!(plan_return(Some(&with_body), 7), ReturnPlan::Return);
    }

    #[test]
    fn a_mail_with_a_cash_on_delivery_price_cannot_be_deleted() {
        let priced = Mail {
            cod: 250,
            ..row(7, MailSender::Character(9))
        };
        assert_eq!(plan_delete(Some(&priced), 7), DeletePlan::CodPriced);
        let paid = row(7, MailSender::Character(9));
        assert_eq!(plan_delete(Some(&paid), 7), DeletePlan::Delete);
        assert_eq!(plan_delete(Some(&paid), 8), DeletePlan::NotYours);
        assert_eq!(plan_delete(None, 7), DeletePlan::NotYours);
    }

    #[test]
    fn a_mail_is_hidden_from_its_recipient_until_its_delivery_instant() {
        let now = Timestamp::from_micros_since_unix_epoch(5_000_000);
        let at = |deliver_micros| Mail {
            deliver_micros,
            ..row(7, MailSender::Character(9))
        };
        assert!(at(0).is_delivered(now), "0 means delivered at creation");
        assert!(at(5_000_000).is_delivered(now));
        assert!(!at(5_000_001).is_delivered(now));
    }

    #[test]
    fn a_row_written_before_the_sender_columns_is_a_character_mail() {
        assert_eq!(
            row(7, MailSender::Character(9)).sender(),
            MailSender::Character(9)
        );
    }

    #[test]
    fn a_character_letter_is_marked_as_having_a_body_or_as_already_copied() {
        let letter = |body: &str| {
            Letter::from_character(
                9,
                7,
                "Hi".into(),
                body.into(),
                0,
                0,
                ItemSnapshot::default(),
            )
        };
        assert_eq!(letter("meet me at the gate").check_flags, 0x10);
        assert_eq!(letter("").check_flags, 0x04);
        assert_eq!(letter("").sender, MailSender::Character(9));
        assert_eq!(
            letter("").deliver_micros,
            0,
            "a character letter arrives at once"
        );
    }

    #[test]
    fn a_cod_payment_keeps_the_letters_subject_and_carries_only_the_payment_bit() {
        let payment = Letter::from_character(
            22,
            11,
            "Your sword".into(),
            String::new(),
            250,
            0,
            ItemSnapshot::default(),
        )
        .into_cod_payment();
        assert_eq!(payment.subject, "Your sword");
        assert_eq!(payment.check_flags, 0x08);
        assert_eq!(payment.money, 250);
    }

    /// `game_mail` inserts in `src`: the table call or a handle bound from it, followed by
    /// `.insert(` across any whitespace, so an insert split over lines still counts.
    fn game_mail_inserts(src: &str) -> usize {
        crate::test_scan::raw_table_reads(src, &["game_mail"], |code, at| {
            code[at..].trim_start().starts_with(".insert(")
        })
        .len()
    }

    #[test]
    fn the_mail_insert_scan_sees_split_and_handle_inserts_and_skips_comments() {
        assert_eq!(
            game_mail_inserts(
                "fn f() {\n    ctx.db\n        .game_mail()\n        .insert(row);\n}"
            ),
            1
        );
        assert_eq!(
            game_mail_inserts("let mails = ctx.db.game_mail();\nmails.insert(row);"),
            1
        );
        assert_eq!(game_mail_inserts("// ctx.db.game_mail().insert(row);"), 0);
        assert_eq!(game_mail_inserts("ctx.db.game_mail().id().update(row);"), 0);
        assert_eq!(
            game_mail_inserts("ctx.db.game_mail_escrow().insert(row);"),
            0
        );
    }

    /// Every mail row goes through `insert_letter`, so no letter skips the header it stamps. The
    /// one other writer is `character_owned!`'s Transfer remint, which inserts through
    /// `ctx.db.$table()` and so never spells the table name.
    #[test]
    fn every_mail_row_is_created_by_insert_letter() {
        let mut elsewhere = Vec::new();
        let mut total = 0;
        for file in crate::tripwires::character_owned_tripwire::scanned_files() {
            let src = std::fs::read_to_string(&file).expect("a scanned file is readable");
            let found = game_mail_inserts(&src);
            total += found;
            if found > 0 && !file.ends_with("module/src/mail.rs") {
                elsewhere.push(file.display().to_string());
            }
        }
        assert!(
            elsewhere.is_empty(),
            "these files insert game_mail rows directly: {elsewhere:?}. Build a `Letter` and call \
             `mail::insert_letter` instead"
        );
        let own = game_mail_inserts(&code_of(
            include_str!("mail.rs"),
            "pub(crate) fn insert_letter(",
        ));
        assert_eq!(
            (own, total),
            (1, 1),
            "`insert_letter` must hold the only game_mail insert"
        );
    }

    #[test]
    fn a_credited_purse_saturates_rather_than_wrapping() {
        assert_eq!(credited(100, 30), 130);
        assert_eq!(credited(u32::MAX, 1), u32::MAX);
    }

    #[test]
    fn both_planes_charge_the_same_total_for_the_same_letter() {
        for money in [0, 1, 100, u32::MAX - 1, u32::MAX] {
            let draft = crate::mail_escrow::Draft {
                recipient_guid: 1,
                subject: String::new(),
                body: String::new(),
                money,
                postage: lyracore_shared::mail::postage(),
                cod: 0,
            };
            assert_eq!(
                lyracore_shared::mail::total_cost(money),
                draft.fenced_copper(),
                "the two planes must charge the same for {money} copper attached"
            );
        }
    }

    #[test]
    fn no_mail_code_path_iterates_the_spatial_gameobject_table() {
        for path in [
            "module/src/mail.rs",
            "gateway/src/stdb/reads/mail.rs",
            "gateway/src/world/mail.rs",
            "gateway/src/world/handlers/mail.rs",
        ] {
            let table = format!("{}{}", "game_gameobject", "()");
            let scan = format!("{}{}", ".iter", "()");
            let src = read_scanned(path).expect("module/ and gateway/ ship in every checkout");
            for (n, line) in src.lines().enumerate() {
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

    #[test]
    fn a_returned_mail_goes_back_unread_unpriced_and_marked_returned_from_now() {
        let priced = Mail {
            money: 100,
            cod: 250,
            item_entry: 509_0001,
            item_stack_count: 1,
            was_read: true,
            check_flags: lyracore_shared::mail::CHECK_MASK_HAS_BODY,
            ..row(7, MailSender::Character(9))
        };
        let now = Timestamp::from_micros_since_unix_epoch(5_000_000);

        let back = returned(priced, now);

        assert_eq!((back.recipient_guid, back.sender_guid), (9, 7));
        assert!(!back.was_read);
        assert_eq!(
            back.cod, 0,
            "a surviving price would charge the seller for their own item"
        );
        assert_eq!(
            back.check_flags, 0x02,
            "RETURNED only (cmangos Mail.cpp:264)"
        );
        assert_eq!(
            (back.money, back.item_entry, back.item_stack_count),
            (100, 509_0001, 1),
            "the copper and the item travel back"
        );
        assert_eq!(
            lyracore_shared::mail::expires_at_secs(
                back.created_at.to_micros_since_unix_epoch() / 1_000_000,
                back.deliver_micros / 1_000_000,
                back.cod,
            ),
            5 + 30 * 86_400,
            "30 days from the return (cmangos Mail.cpp:299-313)"
        );
    }

    #[test]
    fn every_action_a_recipient_takes_looks_the_mail_up_by_its_delivery() {
        for signature in [
            "pub(crate) fn mail_money(",
            "pub(crate) fn mail_item(",
            "pub(crate) fn apply_take_item(",
            "pub(crate) fn apply_return(",
        ] {
            let body = crate::test_scan::shape_of(include_str!("mail.rs"), signature);
            assert!(
                body.contains("delivered_mail(ctx, mail_id)") && !body.contains(".find("),
                "`{signature}` must find the mail through `delivered_mail`, so an undelivered mail \
                 cannot be taken or returned. Body was:\n{body}"
            );
        }
    }

    use crate::test_scan::code_of;

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
