//! Mail routing and cross-database escrow driving.
//! Sharded moves are fence → commit → attest → settle; local moves stay one transaction.

use anyhow::Result;

use super::{party, WorldStore};
use crate::codec::MailView;
use lyracore_shared::mail as mail_rules;

static NEXT_ESCROW_ID: std::sync::OnceLock<std::sync::atomic::AtomicU64> =
    std::sync::OnceLock::new();
static ESCROW_ID_END: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
const NO_COD_SOURCE: u64 = 0;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SendRefusal {
    NoMailbox(String),
    RecipientNotFound(String),
    CannotSendToSelf,
    NotYourTeam,
    NotEnoughMoney(String),
    AttachmentInvalid(String),
    AttachmentSoulbound(String),
    Internal(String),
}

impl std::fmt::Display for SendRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMailbox(e)
            | Self::RecipientNotFound(e)
            | Self::NotEnoughMoney(e)
            | Self::AttachmentInvalid(e)
            | Self::AttachmentSoulbound(e) => f.write_str(e),
            Self::CannotSendToSelf => f.write_str(mail_rules::CANNOT_SEND_TO_SELF),
            Self::NotYourTeam => f.write_str(mail_rules::NOT_YOUR_TEAM),
            Self::Internal(e) => f.write_str(e),
        }
    }
}
pub(crate) fn mail_of<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<Vec<MailView>> {
    match store.realm_store() {
        Some(realm) => realm.mail_list(self_guid),
        None => store.mail_list(self_guid),
    }
}
pub(crate) fn open_mailbox<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
) -> Result<Vec<MailView>> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    redrive(store, self_guid);
    mail_of(store, self_guid)
}
pub(crate) fn has_unread<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
) -> Result<bool> {
    let self_guid =
        self_guid.ok_or_else(|| anyhow::anyhow!(lyracore_shared::mail::NOT_IN_WORLD))?;
    Ok(mail_of(store, self_guid)?.iter().any(|m| !m.was_read))
}
pub(crate) fn letter_body<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mail_id: u64,
) -> Result<Option<String>> {
    let self_guid =
        self_guid.ok_or_else(|| anyhow::anyhow!(lyracore_shared::mail::NOT_IN_WORLD))?;
    Ok(mail_of(store, self_guid)?
        .into_iter()
        .find(|m| m.id == mail_id)
        .map(|m| m.body))
}
pub(crate) fn mark_read<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> Result<()> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    match store.realm_store() {
        Some(realm) => realm.mail_mark_read(self_guid, mail_id),
        None => store.mail_mark_read(self_guid, mail_id),
    }
}
pub(crate) fn delete<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> Result<()> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    match store.realm_store() {
        Some(realm) => realm.mail_delete(self_guid, mail_id),
        None => store.mail_delete(self_guid, mail_id),
    }
}
pub(crate) fn return_to_sender<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> Result<()> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    match store.realm_store() {
        Some(realm) => realm.mail_return(self_guid, mail_id),
        None => store.mail_return(self_guid, mail_id),
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn send<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    recipient_name: &str,
    subject: String,
    body: String,
    money: u32,
    cod: u32,
    item_guid: u64,
) -> std::result::Result<(), SendRefusal> {
    let sender_guid = at_mailbox(store, self_guid, mailbox_guid)
        .map_err(|e| SendRefusal::NoMailbox(e.to_string()))?;
    if !party::live_anywhere(store, sender_guid) {
        return Err(SendRefusal::NoMailbox(mail_rules::NOT_IN_WORLD.to_string()));
    }
    let candidates =
        party::resolve_all_by_name(store, recipient_name).map_err(refusal_from_module)?;
    if candidates.is_empty() {
        return Err(SendRefusal::RecipientNotFound(
            mail_rules::no_recipient_named(recipient_name),
        ));
    }
    if candidates.contains(&sender_guid) {
        return Err(SendRefusal::CannotSendToSelf);
    }
    let sender = party::character_anywhere(store, sender_guid)
        .map_err(refusal_from_module)?
        .ok_or_else(|| SendRefusal::Internal(mail_rules::NOT_IN_WORLD.to_string()))?;
    let mut reachable = Vec::new();
    for guid in candidates {
        let Some(candidate) =
            party::character_anywhere(store, guid).map_err(refusal_from_module)?
        else {
            continue;
        };
        if lyracore_shared::faction::same_team(sender.race, candidate.race) {
            reachable.push(guid);
        }
    }
    let recipient_guid = match reachable.as_slice() {
        [] => return Err(SendRefusal::NotYourTeam),
        [only] => *only,
        _ => {
            return Err(SendRefusal::RecipientNotFound(
                mail_rules::ambiguous_recipient(recipient_name),
            ))
        }
    };
    let cod = mail_rules::cod_at_send(cod, item_guid != 0);
    match store.realm_store() {
        None => store
            .mail_send(
                sender_guid,
                recipient_guid,
                subject,
                body,
                money,
                cod,
                item_guid,
            )
            .map_err(refusal_from_module),
        Some(realm) => {
            let escrow_id =
                next_escrow_id().map_err(|e| SendRefusal::Internal(format!("{e:#}")))?;
            store
                .mail_fence(
                    escrow_id,
                    sender_guid,
                    recipient_guid,
                    subject.clone(),
                    body.clone(),
                    money,
                    mail_rules::postage(),
                    item_guid,
                    cod,
                    NO_COD_SOURCE,
                )
                .map_err(refusal_from_module)?;
            let item = held_attachment(store, sender_guid, escrow_id)
                .map_err(|e| SendRefusal::Internal(format!("{e:#}")))?;
            drive(store, escrow_id, || {
                realm.mail_commit(
                    escrow_id,
                    sender_guid,
                    recipient_guid,
                    subject,
                    body,
                    money,
                    item.clone(),
                    cod,
                    NO_COD_SOURCE,
                )
            })
            .map_err(|e| SendRefusal::Internal(format!("{e:#}")))
        }
    }
}
fn drive<St, F>(source: &St, escrow_id: u64, commit: F) -> Result<()>
where
    St: WorldStore + ?Sized,
    F: FnOnce() -> Result<()>,
{
    commit()?;
    source.mail_confirm_delivery(escrow_id)?;
    source.mail_settle(escrow_id)
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachedItem {
    pub entry: u32,
    pub stack_count: u32,
    pub durability: u32,
    pub enchant_id: u32,
    pub soulbound: bool,
}

impl AttachedItem {
    pub fn is_empty(&self) -> bool {
        self.entry == 0
    }
}
fn held_attachment<St: WorldStore + ?Sized>(
    store: &St,
    sender_guid: u64,
    escrow_id: u64,
) -> Result<AttachedItem> {
    store
        .mail_escrows_of(sender_guid)?
        .into_iter()
        .find(|e| e.escrow_id == escrow_id)
        .map(|e| e.item)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "mail escrow {escrow_id}: the fence reported success but no row is readable —                  refusing to commit a letter whose attachment cannot be confirmed"
            )
        })
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeldEscrow {
    pub escrow_id: u64,
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    pub money: u32,
    pub postage: u32,
    pub payout: bool,
    pub mail_id: u64,
    pub item: AttachedItem,
    pub cod: u32,
}
pub(crate) fn redrive<St: WorldStore + ?Sized>(store: &St, self_guid: u64) {
    let Some(realm) = store.realm_store() else {
        return; // One database, one transaction, no fences to rescue.
    };
    for held in store.mail_escrows_of(self_guid).unwrap_or_default() {
        if held.payout {
            continue; // A payout fence never lives on a shard; ignore a stray rather than mis-drive it.
        }
        let outcome = drive(store, held.escrow_id, || {
            realm.mail_commit(
                held.escrow_id,
                self_guid,
                held.recipient_guid,
                held.subject.clone(),
                held.body.clone(),
                held.money,
                held.item.clone(),
                held.cod,
                held.mail_id,
            )
        });
        log_redrive("send", held.escrow_id, outcome);
    }
    for held in realm.mail_escrows_of(self_guid).unwrap_or_default() {
        if !held.payout {
            continue;
        }
        let outcome = drive(realm.as_ref(), held.escrow_id, || {
            if held.item.is_empty() {
                store.mail_payout(held.escrow_id, self_guid, held.mail_id, held.money)
            } else {
                store.mail_item_payout(held.escrow_id, self_guid, held.mail_id, held.item.clone())
            }
        });
        log_redrive("take", held.escrow_id, outcome);
    }
}

fn log_redrive(kind: &str, escrow_id: u64, outcome: Result<()>) {
    match outcome {
        Ok(()) => log::info!("mail escrow {escrow_id}: abandoned {kind} re-driven to completion"),
        Err(e) => log::warn!(
            "mail escrow {escrow_id}: {kind} re-drive failed, the fence is still HELD: {e:#}"
        ),
    }
}
pub(crate) fn take_money<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> Result<()> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    let Some(realm) = store.realm_store() else {
        return store.mail_take_money(self_guid, mail_id);
    };
    let amount = mail_of(store, self_guid)?
        .into_iter()
        .find(|m| m.id == mail_id)
        .map(|m| m.money)
        .ok_or_else(|| anyhow::anyhow!(mail_rules::NOT_YOUR_MAIL))?;
    if amount == 0 {
        anyhow::bail!(mail_rules::NOTHING_TO_TAKE);
    }
    let escrow_id = next_escrow_id()?;
    realm.mail_take_money_fence(escrow_id, self_guid, mail_id, amount)?;
    drive(realm.as_ref(), escrow_id, || {
        store.mail_payout(escrow_id, self_guid, mail_id, amount)
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TakeItemRefusal {
    BagsFull(String),
    CannotAffordCod(String),
    Other(String),
}

impl std::fmt::Display for TakeItemRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BagsFull(e) | Self::CannotAffordCod(e) | Self::Other(e) => f.write_str(e),
        }
    }
}

fn take_item_refusal(e: anyhow::Error) -> TakeItemRefusal {
    let text = format!("{e:#}");
    if text.contains(mail_rules::INVENTORY_FULL) {
        TakeItemRefusal::BagsFull(text)
    } else if text.contains(mail_rules::COD_NOT_AFFORDABLE)
        || text.contains(mail_rules::NOT_ENOUGH_MONEY)
    {
        TakeItemRefusal::CannotAffordCod(text)
    } else {
        TakeItemRefusal::Other(text)
    }
}
pub(crate) fn take_item<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> std::result::Result<(u32, u32), TakeItemRefusal> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid).map_err(take_item_refusal)?;
    let row = mail_of(store, self_guid)
        .map_err(take_item_refusal)?
        .into_iter()
        .find(|m| m.id == mail_id)
        .ok_or_else(|| TakeItemRefusal::Other(mail_rules::NOT_YOUR_MAIL.to_string()))?;
    let item = AttachedItem {
        entry: row.item_entry,
        stack_count: row.item_stack_count,
        durability: row.item_durability,
        enchant_id: row.item_enchant_id,
        soulbound: row.item_soulbound,
    };
    if item.is_empty() {
        return Err(TakeItemRefusal::Other(
            mail_rules::NOTHING_TO_TAKE.to_string(),
        ));
    }
    let taken = (item.entry, item.stack_count);
    let Some(realm) = store.realm_store() else {
        store
            .mail_take_item(self_guid, mail_id)
            .map_err(take_item_refusal)?;
        return Ok(taken);
    };
    store.mail_item_room(self_guid).map_err(take_item_refusal)?;
    pay_cod(store, realm.as_ref(), self_guid, &row).map_err(take_item_refusal)?;
    let escrow_id = next_escrow_id().map_err(take_item_refusal)?;
    realm
        .mail_take_item_fence(escrow_id, self_guid, mail_id, item.entry)
        .map_err(take_item_refusal)?;
    drive(realm.as_ref(), escrow_id, || {
        store.mail_item_payout(escrow_id, self_guid, mail_id, item.clone())
    })
    .map_err(take_item_refusal)?;
    Ok(taken)
}
fn pay_cod<St: WorldStore + ?Sized>(
    store: &St,
    realm: &dyn WorldStore,
    taker_guid: u64,
    row: &MailView,
) -> Result<()> {
    let Some(settlement) =
        mail_rules::cod_settlement(row.cod, row.sender_guid, &row.subject, taker_guid)
    else {
        return Ok(());
    };
    let escrow_id = store
        .mail_escrows_of(taker_guid)
        .unwrap_or_default()
        .into_iter()
        .find(|e| !e.payout && e.mail_id == row.id)
        .map(|e| e.escrow_id)
        .map(Ok)
        .unwrap_or_else(next_escrow_id)?;
    store.mail_fence(
        escrow_id,
        settlement.payer_guid,
        settlement.payee_guid,
        settlement.subject.clone(),
        String::new(),
        settlement.copper,
        0,
        0,
        0,
        row.id,
    )?;
    drive(store, escrow_id, || {
        realm.mail_commit(
            escrow_id,
            settlement.payer_guid,
            settlement.payee_guid,
            settlement.subject.clone(),
            String::new(),
            settlement.copper,
            AttachedItem::default(),
            0,
            row.id,
        )
    })
}
fn next_escrow_id() -> Result<u64> {
    use std::sync::atomic::Ordering;
    #[cfg(test)]
    if NEXT_ESCROW_ID.get().is_none() {
        install_escrow_id_range(1, u64::MAX);
    }
    let next = NEXT_ESCROW_ID.get().ok_or_else(|| {
        anyhow::anyhow!(
        "mail escrow id range was not claimed; refusing instead of falling back to colliding ids"
    )
    })?;
    let id = next.fetch_add(1, Ordering::Relaxed);
    if id >= *ESCROW_ID_END.get().unwrap_or(&0) {
        anyhow::bail!("mail escrow id range is exhausted")
    }
    Ok(id)
}
pub(crate) fn install_escrow_id_range(next: u64, end: u64) {
    use std::sync::atomic::AtomicU64;
    let _ = ESCROW_ID_END.set(end);
    let _ = NEXT_ESCROW_ID.set(AtomicU64::new(next.max(1)));
}
fn refusal_from_module(e: anyhow::Error) -> SendRefusal {
    let text = format!("{e:#}");
    if text.contains(mail_rules::NOT_ENOUGH_MONEY) {
        SendRefusal::NotEnoughMoney(text)
    } else if text.contains(mail_rules::ITEM_IS_SOULBOUND) {
        SendRefusal::AttachmentSoulbound(text)
    } else if text.contains(mail_rules::NOT_YOUR_ITEM) {
        SendRefusal::AttachmentInvalid(text)
    } else {
        SendRefusal::Internal(text)
    }
}
fn at_mailbox<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
) -> Result<u64> {
    let self_guid =
        self_guid.ok_or_else(|| anyhow::anyhow!(lyracore_shared::mail::NOT_IN_WORLD))?;
    if !store.mailbox_in_range(mailbox_guid, self_guid)? {
        anyhow::bail!(lyracore_shared::mail::not_at_mailbox(mailbox_guid));
    }
    Ok(self_guid)
}
