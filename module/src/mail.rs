//! Durable mail rows and single-database mailbox operations.
//! Attachments are item snapshots; cross-database moves live in `mail_escrow`.

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
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn has_unread(ctx: &ReducerContext, recipient_guid: u64) -> bool {
    ctx.db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .any(|m| !m.was_read)
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
    ctx.db
        .game_mail()
        .id()
        .find(mail_id)
        .map(|m| (m.recipient_guid, m.money))
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
    ctx.db
        .game_mail()
        .id()
        .find(mail_id)
        .map(|m| (m.recipient_guid, m.snapshot()))
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReturnPlan {
    NotYours,
    Return,
}
pub(crate) fn plan_return(recipient_guid: Option<u64>, caller_guid: u64) -> ReturnPlan {
    match recipient_guid {
        Some(r) if r == caller_guid => ReturnPlan::Return,
        _ => ReturnPlan::NotYours,
    }
}
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
#[reducer]
pub fn realm_mail_mark_read(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_mark_read(ctx, recipient_guid, mail_id)
}
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
#[reducer]
pub fn realm_mail_take_item(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_take_item(ctx, recipient_guid, mail_id)
}
#[reducer]
pub fn realm_mail_take_money(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_take_money(ctx, recipient_guid, mail_id)
}
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
#[reducer]
pub fn realm_mail_delete(
    ctx: &ReducerContext,
    recipient_guid: u64,
    mail_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_delete(ctx, recipient_guid, mail_id)
}
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

    #[test]
    fn returning_a_mail_is_refused_for_a_caller_who_is_not_the_recipient() {
        assert_eq!(plan_return(Some(7), 7), ReturnPlan::Return);
        assert_eq!(plan_return(Some(7), 8), ReturnPlan::NotYours);
        assert_eq!(plan_return(None, 7), ReturnPlan::NotYours);
    }

    #[test]
    fn a_credited_purse_saturates_rather_than_wrapping() {
        assert_eq!(credited(100, 30), 130);
        assert_eq!(credited(u32::MAX, 1), u32::MAX);
    }

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
