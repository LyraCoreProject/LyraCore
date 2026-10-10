//! Mail routing and cross-database escrow driving.
//! Sharded moves are fence → commit → attest → settle; local moves stay one transaction. The
//! turn-in files a Reward Letter as Escrow on every plane, and it is driven the same way.

use crate::world::{CharacterStore, SessionStore, ShardRoutingStore, SocialStore};
use anyhow::Result;

use super::{presence, WorldStore};
use crate::codec::MailView;
use lyracore_shared::mail as mail_rules;

/// Mail rows, Letter Copy text, and the cross-shard mail Escrow steps.
pub(crate) trait MailStore: Send + Sync {
    /// Every mail addressed to `recipient_guid`, on the database THIS handle names.
    ///
    /// Called on the realm-core handle when there is one and on the session's own handle when there
    /// is not — the two-plane read, which is why this is one method rather than a realm-only twin.
    /// The real refusals are the gates in `world::mail`, which run before this.
    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<MailView>>;

    /// The mail `mail_id`, delivered or not, on the database THIS handle names: the same two-plane
    /// routing as [`mail_list`](Self::mail_list), as one primary key read.
    fn mail_by_id(&self, mail_id: u64) -> Result<Option<MailView>>;

    /// The name of the Realm Account that owns `character_guid`, read on THIS handle only. The
    /// Account Character Owner names it when this Shard retains one. Otherwise the Character's
    /// local Account names it, unless that Account is a shadow Account, whose name is not a Realm
    /// Account's. `None` when this handle cannot name it. `world::mail` asks every World Shard,
    /// because the Delivery Delay compares the Realm Accounts of two Characters on any Shards.
    fn realm_account_name(&self, character_guid: u64) -> Result<Option<String>>;

    /// Is `player_guid` in range of the gameobject `mailbox_guid` names, and is it a mailbox at all?
    ///
    /// Always asked of the session's OWN handle: the mailbox is a gameobject on the shard the player
    /// is standing on, and realm-core holds none. A PK lookup plus a map/instance/range check —
    /// never a scan of the spatial gameobject table.
    fn mailbox_in_range(&self, mailbox_guid: u64, player_guid: u64) -> Result<bool>;

    /// Flip `mail_id`'s read state for `recipient_guid`, on the database THIS handle names.
    ///
    /// Called on the realm-core handle when there is one and on the session's own handle when there
    /// is not — the SAME two-plane routing [`mail_list`](Self::mail_list) takes, because the write
    /// and the read must never disagree about which database owns the rows. `Err` when `mail_id`
    /// does not exist, is not `recipient_guid`'s or has not arrived yet — the gates ran in
    /// `world::mail` before this is ever called, so a refusal here means a crafted id or a Gateway
    /// clock that runs ahead of the Module's.
    fn mail_mark_read(&self, recipient_guid: u64, mail_id: u64) -> Result<()>;

    /// Delete `mail_id` for `recipient_guid`, on the database THIS handle names — same two-plane
    /// routing as [`mail_mark_read`](Self::mail_mark_read). Destroys any attachment the row still
    /// carries, as vanilla does after its (client-side) confirmation prompt. `Err` for a mail with
    /// a cash on delivery price or one that has not arrived yet.
    fn mail_delete(&self, recipient_guid: u64, mail_id: u64) -> Result<()>;

    /// Return `mail_id` to whoever sent it, on the database THIS handle names — same two-plane
    /// routing as [`mail_delete`](Self::mail_delete). The row is re-addressed IN PLACE: it never
    /// leaves the plane that already holds it, so there is no sharded variant and no escrow, unlike
    /// [`mail_send`](Self::mail_send) and the takes below. `Err` when `mail_id` does not exist, is
    /// not `recipient_guid`'s, is not delivered yet, has no Character sender, or was returned
    /// already. `same_account` says whether `recipient_guid` and the mail's sender belong to one
    /// Realm Account; the Module turns it into the return's Delivery Delay.
    fn mail_return(&self, recipient_guid: u64, mail_id: u64, same_account: bool) -> Result<()>;

    /// Write one sent letter on the database THIS handle names, charging the sender the postage
    /// plus the attached `money` in the SAME transaction.
    ///
    /// **The single-database gateway only**, where the purse and the row are on one database. A
    /// sharded realm cannot have that transaction and drives [`mail_fence`](Self::mail_fence) and
    /// friends instead.
    ///
    /// Every gate that decides who may write to whom has already run in `world::mail` — realm-core
    /// can answer none of them — so `sender_guid` must be the guid the socket authenticated.
    ///
    /// `cod` is the price the RECIPIENT will owe for the attachment. It costs the sender nothing
    /// and is not part of the debit; it only rides the row until somebody takes the item.
    /// `same_account` says whether the sender and the recipient belong to one Realm Account; the
    /// Module turns it into the letter's Delivery Delay.
    #[allow(clippy::too_many_arguments)]
    fn mail_send(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item_guid: u64,
        same_account: bool,
    ) -> Result<()>;

    /// Credit `mail_id`'s copper to `recipient_guid` and empty the row, in one transaction. The
    /// single-database twin of [`mail_send`](Self::mail_send), and refused for a mail that is not
    /// the caller's, is not delivered yet, or has nothing left in it.
    fn mail_take_money(&self, recipient_guid: u64, mail_id: u64) -> Result<()>;

    /// Re-create `mail_id`'s attached item in `recipient_guid`'s bags and empty the row's
    /// attachment columns, in one transaction. [`mail_take_money`](Self::mail_take_money)'s twin,
    /// and refused for a mail that is not the caller's or not delivered yet, one with no
    /// attachment, or a full bag —
    /// where the refusal rolls the clear back, so the item stays in the letter.
    fn mail_take_item(&self, recipient_guid: u64, mail_id: u64) -> Result<()>;

    /// Has `payee_guid` room in their bags here for one more item?
    ///
    /// Asked of the TAKER's own handle, before a sharded item take fences anything: the fence is a
    /// one-way move, so a full bag found afterwards would strand the item in an escrow instead of
    /// leaving it in the letter. `Err` is the refusal.
    fn mail_item_room(&self, payee_guid: u64) -> Result<()>;

    /// `CMSG_MAIL_CREATE_TEXT_ITEM` step 1 (Letter Copy) — set COPIED on `mail_id` and file its
    /// body as durable item text, on the database that OWNS THE MAIL ROW (realm-core when sharded,
    /// this shard's own database otherwise — the same two-plane routing `mail_take_item_fence`
    /// takes). `Err` for a mail that is not the caller's, is not delivered, has no body, or is
    /// already GRANTED. A replay before GRANTED is set is `Ok` (a no-op on the mail plane), so a
    /// retry can still reach the Home Shard grant.
    fn mail_copy_text(&self, recipient_guid: u64, mail_id: u64) -> Result<()>;

    /// `CMSG_MAIL_CREATE_TEXT_ITEM` step 2 — store one Plain Letter carrying `item_text_id`, on the
    /// PAYEE's own handle. [`mail_item_room`](Self::mail_item_room)'s real Gate: a full bag found
    /// here refuses and leaves the mail COPIED with no letter granted. The Plain Letter sells for
    /// 0, so a grant lost to that race costs nothing — this is deliberately not an escrow. Also a
    /// no-op `Ok` when the payee already holds an item carrying `item_text_id`: the crash-window
    /// guard between this call landing and [`mail_mark_letter_granted`](Self::mail_mark_letter_granted)
    /// recording that it did.
    fn mail_grant_letter(&self, payee_guid: u64, item_text_id: u32) -> Result<()>;

    /// `CMSG_MAIL_CREATE_TEXT_ITEM` step 3 — the durable record that the grant landed, on the same
    /// database `mail_copy_text` wrote to. Called once [`mail_grant_letter`](Self::mail_grant_letter)
    /// returns `Ok`. Unlike the item itself, this bit cannot be destroyed, mailed away, or traded,
    /// so it is what refuses a second grant for good.
    fn mail_mark_letter_granted(&self, recipient_guid: u64, mail_id: u64) -> Result<()>;

    /// The durable text behind `item_text_id`, read from `game_item_text` on the database that
    /// OWNS THE MAIL PLANE (same two-plane routing as [`mail_copy_text`](Self::mail_copy_text)). A
    /// copied letter's text outlives the mail row that created it, so this answers even after that
    /// mail is deleted.
    ///
    /// The mail plane holds every copied letter's text keyed by a small, sequential id, so this
    /// must never be read for a caller who has not proven they may see it — see
    /// [`owns_item_with_text`](Self::owns_item_with_text).
    fn item_text(&self, item_text_id: u32) -> Result<Option<String>>;

    /// Does `owner_guid` hold an item carrying `item_text_id` in their own bags, on THIS handle?
    /// The ownership Gate `mail::item_text` checks before it answers from `game_item_text`: a
    /// client walking `item_text_id` values must not read another player's Letter Copy that way.
    /// `hint_item_guid` is `CMSG_ITEM_TEXT_QUERY`'s overloaded second field — often the queried
    /// item's own guid when it names an item rather than a mail — so an implementation can try a
    /// cheap PK lookup before falling back to a scan of `owner_guid`'s rows.
    fn owns_item_with_text(
        &self,
        owner_guid: u64,
        item_text_id: u32,
        hint_item_guid: u64,
    ) -> Result<bool>;

    /// **Escrow step 1 (send)** — take the postage plus the attached coin out of `sender_guid`'s
    /// purse into a fence keyed by the caller-chosen `escrow_id`, on the database THIS handle names.
    ///
    /// Always the SENDER's own handle: the purse is `game_world_entity.money`, on the shard they
    /// are standing on. `Err` is the atomic affordability refusal — a refused send costs nothing.
    ///
    /// A COD PAYMENT is fenced through here too, because it is a letter out of a purse like any
    /// other: `cod_source_mail_id` names the mail whose price it pays (0 for an ordinary letter),
    /// and it rides the fence so a re-drive can settle that price without re-deriving anything.
    ///
    /// `same_account` says whether the sender and the recipient belong to one Realm Account. The
    /// fence resolves the letter's Delivery Delay from it and stores it for the commit.
    #[allow(clippy::too_many_arguments)]
    fn mail_fence(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        postage: u32,
        item_guid: u64,
        cod: u32,
        cod_source_mail_id: u64,
        same_account: bool,
    ) -> Result<()>;

    /// **Escrow step 2 (send)** — write the mail row and its receipt under `escrow_id`, on the
    /// database THIS handle names (realm-core). Idempotent: a replay writes nothing.
    ///
    /// `cod_source_mail_id` (0 for an ordinary letter) is the mail this one PAYS FOR: its price is
    /// settled in the same transaction as the payout row, which is what makes a COD take charge
    /// once however the drive is interrupted.
    ///
    /// `delivery_delay_secs` is the Delivery Delay the fence stored. The letter arrives that long
    /// after this commit. `reward` names a Reward Letter's quest giver and Mail Template; `None` is
    /// a Character's letter from `sender_guid`.
    #[allow(clippy::too_many_arguments)]
    fn mail_commit(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        item: AttachedItem,
        cod: u32,
        cod_source_mail_id: u64,
        delivery_delay_secs: u32,
        reward: Option<lyracore_shared::mail::RewardHeader>,
    ) -> Result<()>;

    /// **Escrow step 1 (take)** — take `mail_id`'s copper out of the row into a fence, on the
    /// database that OWNS THE ROW. `expect_money` is the amount the caller is about to pay out; a
    /// mismatch is refused rather than fenced, because the gateway carries that number across.
    fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()>;

    /// **Escrow step 2 (take)** — credit `amount` to `payee_guid` and file a receipt under
    /// `escrow_id`, on the PAYEE's own handle. Idempotent: a replay credits nothing.
    fn mail_payout(&self, escrow_id: u64, payee_guid: u64, mail_id: u64, amount: u32)
        -> Result<()>;

    /// **Escrow step 1 (item take)** — take `mail_id`'s attachment out of the row into a fence, on
    /// the database that OWNS THE ROW. `expect_entry` is the item the caller is about to grant; a
    /// mismatch is refused rather than fenced, because the gateway carries the snapshot across.
    fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()>;

    /// **Escrow step 2 (item take)** — re-create the fenced item in `payee_guid`'s bags and file a
    /// receipt under `escrow_id`, on the PAYEE's own handle. Idempotent: a replay grants nothing.
    /// `Err` on a full bag, which leaves the fence holding the item for the next re-drive.
    fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        item: AttachedItem,
    ) -> Result<()>;

    /// **Escrow step 3** — attest, on the handle HOLDING the fence, that the other database
    /// committed. The only thing that licenses step 4.
    fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()>;

    /// **Escrow step 4** — destroy the fence, on the handle holding it. Delete-last: it refuses
    /// while unattested.
    fn mail_settle(&self, escrow_id: u64) -> Result<()>;

    /// Every unfinished mail escrow this database holds for `sender_guid` (the payee, on a payout).
    ///
    /// The read that makes re-driving possible at all: a fence carries its whole letter, so a drive
    /// abandoned by a dead gateway is resumable from the row.
    fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<HeldEscrow>>;
}

struct EscrowIdRange {
    next: std::sync::atomic::AtomicU64,
    start: u64,
    end: u64,
}

static ESCROW_ID_RANGE: std::sync::OnceLock<EscrowIdRange> = std::sync::OnceLock::new();
const NO_COD_SOURCE: u64 = 0;
/// A COD payment carries copper only, so it arrives at once (cmangos `MailHandler.cpp:475-477`).
const NO_DELIVERY_DELAY: u32 = 0;
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
/// The mailbox as its owner sees it, on whichever plane holds it. A mail whose delivery instant is
/// still ahead is absent, so the list, the unread poll, the body read and every take skip it
/// (cmangos `MailHandler.cpp:561`, `Player.cpp:3079-3096`).
pub(crate) fn mail_of<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<Vec<MailView>> {
    let mails = match store.realm_store() {
        Some(realm) => realm.mail_list(self_guid)?,
        None => store.mail_list(self_guid)?,
    };
    let now = now_secs();
    Ok(mails.into_iter().filter(|m| m.is_delivered(now)).collect())
}
/// Wall-clock seconds: the delivery filter's clock and the base of the list's expiry countdown.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
pub(crate) fn open_mailbox<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
) -> Result<Vec<MailView>> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    redrive(store, self_guid);
    mail_of(store, self_guid)
}
pub(crate) fn has_unread<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
) -> Result<bool> {
    let self_guid =
        self_guid.ok_or_else(|| anyhow::anyhow!(lyracore_shared::mail::NOT_IN_WORLD))?;
    Ok(mail_of(store, self_guid)?.iter().any(|m| !m.was_read))
}
pub(crate) fn letter_body<St: MailStore + ShardRoutingStore + ?Sized>(
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
pub(crate) fn mark_read<St: MailStore + ShardRoutingStore + ?Sized>(
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
pub(crate) fn delete<St: MailStore + ShardRoutingStore + ?Sized>(
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
pub(crate) fn return_to_sender<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> Result<()> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    let realm = store.realm_store();
    let mail = match &realm {
        Some(realm) => realm.mail_by_id(mail_id)?,
        None => store.mail_by_id(mail_id)?,
    };
    // A mail that is not there is refused by the Module, so its Accounts do not matter.
    let same_account = match mail {
        Some(mail) => same_realm_account(store, self_guid, mail.sender_guid)?,
        None => false,
    };
    match realm {
        Some(realm) => realm.mail_return(self_guid, mail_id, same_account),
        None => store.mail_return(self_guid, mail_id, same_account),
    }
}
/// Do two Characters belong to one Realm Account? The Module turns the answer into the Delivery
/// Delay, but it cannot read a Character's Account on another Shard, so the Gateway reads both
/// realm-wide (`docs/architecture.md` §2.3). An Account that no Shard can name counts as another
/// Account, so an item waits.
fn same_realm_account<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    a: u64,
    b: u64,
) -> Result<bool> {
    Ok(
        match (
            realm_account_anywhere(store, a)?,
            realm_account_anywhere(store, b)?,
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
    )
}
/// The Realm Account name the first handle that can name one holds for `guid`: this handle, then
/// every World Shard. Admission refuses two Shards that name different Accounts for one Character,
/// so the first name is the name.
fn realm_account_anywhere<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    guid: u64,
) -> Result<Option<String>> {
    if let Some(name) = store.realm_account_name(guid)? {
        return Ok(Some(name));
    }
    for shard in store.world_stores() {
        if let Some(name) = shard.realm_account_name(guid)? {
            return Ok(Some(name));
        }
    }
    Ok(None)
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn send<
    St: CharacterStore + MailStore + SessionStore + ShardRoutingStore + SocialStore + ?Sized,
>(
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
    if !presence::live_anywhere(store, sender_guid) {
        return Err(SendRefusal::NoMailbox(mail_rules::NOT_IN_WORLD.to_string()));
    }
    let candidates =
        presence::resolve_all_by_name(store, recipient_name).map_err(refusal_from_module)?;
    if candidates.is_empty() {
        return Err(SendRefusal::RecipientNotFound(
            mail_rules::no_recipient_named(recipient_name),
        ));
    }
    if candidates.contains(&sender_guid) {
        return Err(SendRefusal::CannotSendToSelf);
    }
    let sender = presence::character_anywhere(store, sender_guid)
        .map_err(refusal_from_module)?
        .ok_or_else(|| SendRefusal::Internal(mail_rules::NOT_IN_WORLD.to_string()))?;
    let mut reachable = Vec::new();
    for guid in candidates {
        let Some(candidate) =
            presence::character_anywhere(store, guid).map_err(refusal_from_module)?
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
    let same_account =
        same_realm_account(store, sender_guid, recipient_guid).map_err(refusal_from_module)?;
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
                same_account,
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
                    same_account,
                )
                .map_err(refusal_from_module)?;
            let held = held_fence(store, sender_guid, escrow_id)
                .map_err(|e| SendRefusal::Internal(format!("{e:#}")))?;
            drive(store, escrow_id, || {
                realm.mail_commit(
                    escrow_id,
                    sender_guid,
                    recipient_guid,
                    subject,
                    body,
                    money,
                    held.item.clone(),
                    cod,
                    NO_COD_SOURCE,
                    held.delivery_delay_secs,
                    held.reward,
                )
            })
            .map_err(|e| SendRefusal::Internal(format!("{e:#}")))
        }
    }
}
fn drive<St, F>(source: &St, escrow_id: u64, commit: F) -> Result<()>
where
    St: MailStore + ?Sized,
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
    pub random_property_id: u32,
    /// A Plain Letter's `ITEM_FIELD_ITEM_TEXT_ID`. 0 for every other item.
    pub item_text_id: u32,
}

impl AttachedItem {
    pub fn is_empty(&self) -> bool {
        self.entry == 0
    }
}
/// How long a fence's escrow row may take to reach the coordinator cache after the fence reducer
/// returned. The call pipe and the subscription travel different connections (`transfer.rs`'s
/// `escrow_after_begin` carries the same lag for a Transfer), so an immediate read can lose that
/// race even though the fence landed.
const ESCROW_VISIBLE_WITHIN: std::time::Duration = std::time::Duration::from_secs(2);
const ESCROW_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// The fence a send or a take just filed. The next step takes the attachment, and a send's Delivery
/// Delay, from it, exactly as a re-drive does. Waits out the coordinator cache's lag behind the
/// fence's own call-pipe commit instead of refusing on the first miss. Each read re-acquires the
/// store's own cache guard, so the wait holds none of it while it sleeps.
fn held_fence<St: MailStore + ?Sized>(
    store: &St,
    sender_guid: u64,
    escrow_id: u64,
) -> Result<HeldEscrow> {
    let deadline = std::time::Instant::now() + ESCROW_VISIBLE_WITHIN;
    loop {
        if let Some(held) = store
            .mail_escrows_of(sender_guid)?
            .into_iter()
            .find(|e| e.escrow_id == escrow_id)
        {
            return Ok(held);
        }
        if std::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "mail escrow {escrow_id}: the fence reported success but no row is readable within \
                 {ESCROW_VISIBLE_WITHIN:?}, so the letter's attachment and Delivery Delay cannot be \
                 confirmed"
            ));
        }
        std::thread::sleep(ESCROW_POLL);
    }
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
    /// The Delivery Delay the fence resolved, so a re-driven commit keeps it.
    pub delivery_delay_secs: u32,
    /// A Reward Letter's quest giver and Mail Template. `None` for a Character's letter.
    pub reward: Option<mail_rules::RewardHeader>,
}
/// Drive every letter `self_guid` holds as Escrow on its Home Shard to the mail plane, and every
/// take Realm-core holds for them into their purse or bags. That rescues a send a Gateway
/// abandoned, and it is the only thing that delivers a Reward Letter, which the Module files at
/// turn-in. On a single-database realm the mail plane is the same database: a Character's send files
/// no Escrow there, but a Reward Letter does.
pub(crate) fn redrive<St: MailStore + ShardRoutingStore + ?Sized>(store: &St, self_guid: u64) {
    let realm = store.realm_store();
    for held in store.mail_escrows_of(self_guid).unwrap_or_default() {
        if held.payout {
            continue; // A payout fence never lives on a shard; ignore a stray rather than mis-drive it.
        }
        let outcome = drive(store, held.escrow_id, || match &realm {
            Some(realm) => commit_held(realm.as_ref(), self_guid, &held),
            None => commit_held(store, self_guid, &held),
        });
        let kind = if held.reward.is_some() {
            "Reward Letter"
        } else {
            "send"
        };
        log_redrive(kind, held.escrow_id, outcome);
    }
    let Some(realm) = realm else {
        return; // One database: a take is one transaction and files no Escrow.
    };
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

/// Commit the letter `held` describes on the mail plane `plane`, fenced by `sender_guid`.
fn commit_held<P: MailStore + ?Sized>(
    plane: &P,
    sender_guid: u64,
    held: &HeldEscrow,
) -> Result<()> {
    plane.mail_commit(
        held.escrow_id,
        sender_guid,
        held.recipient_guid,
        held.subject.clone(),
        held.body.clone(),
        held.money,
        held.item.clone(),
        held.cod,
        held.mail_id,
        held.delivery_delay_secs,
        held.reward,
    )
}

fn log_redrive(kind: &str, escrow_id: u64, outcome: Result<()>) {
    match outcome {
        Ok(()) => log::info!("mail escrow {escrow_id}: held {kind} driven to completion"),
        Err(e) => log::warn!(
            "mail escrow {escrow_id}: {kind} re-drive failed, the fence is still HELD: {e:#}"
        ),
    }
}
pub(crate) fn take_money<St: MailStore + ShardRoutingStore + ?Sized>(
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
pub(crate) fn take_item<St: MailStore + ShardRoutingStore + ?Sized>(
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
    if row.item_entry == 0 {
        return Err(TakeItemRefusal::Other(
            mail_rules::NOTHING_TO_TAKE.to_string(),
        ));
    }
    let taken = (row.item_entry, row.item_stack_count);
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
        .mail_take_item_fence(escrow_id, self_guid, mail_id, row.item_entry)
        .map_err(take_item_refusal)?;
    let item = held_fence(realm.as_ref(), self_guid, escrow_id)
        .map_err(take_item_refusal)?
        .item;
    drive(realm.as_ref(), escrow_id, || {
        store.mail_item_payout(escrow_id, self_guid, mail_id, item)
    })
    .map_err(take_item_refusal)?;
    Ok(taken)
}
fn pay_cod<St: MailStore + ?Sized>(
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
        // Copper only: the fence resolves no Delivery Delay whatever the Accounts.
        false,
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
            NO_DELIVERY_DELAY,
            None,
        )
    })
}
fn next_escrow_id() -> Result<u64> {
    use std::sync::atomic::Ordering;
    #[cfg(test)]
    if ESCROW_ID_RANGE.get().is_none() {
        install_escrow_id_range(1, u64::MAX)?;
    }
    let range = ESCROW_ID_RANGE.get().ok_or_else(|| {
        anyhow::anyhow!(
        "mail escrow id range was not claimed; refusing instead of falling back to colliding ids"
    )
    })?;
    let id = range.next.fetch_add(1, Ordering::Relaxed);
    if id >= range.end {
        anyhow::bail!("mail escrow id range is exhausted")
    }
    Ok(id)
}
pub(crate) fn install_escrow_id_range(next: u64, end: u64) -> Result<()> {
    use std::sync::atomic::AtomicU64;
    let requested_start = next.max(1);
    let range = ESCROW_ID_RANGE.get_or_init(|| EscrowIdRange {
        next: AtomicU64::new(requested_start),
        start: requested_start,
        end,
    });
    if range.start != requested_start || range.end != end {
        anyhow::bail!("a different mail escrow id range is already installed")
    }
    Ok(())
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
fn at_mailbox<St: MailStore + ?Sized>(
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CopyLetterRefusal {
    NoMailbox(String),
    BagsFull(String),
    Other(String),
}

impl std::fmt::Display for CopyLetterRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMailbox(e) | Self::BagsFull(e) | Self::Other(e) => f.write_str(e),
        }
    }
}

fn copy_letter_refusal(e: anyhow::Error) -> CopyLetterRefusal {
    let text = format!("{e:#}");
    if text.contains(mail_rules::INVENTORY_FULL) {
        CopyLetterRefusal::BagsFull(text)
    } else {
        CopyLetterRefusal::Other(text)
    }
}
/// `CMSG_MAIL_CREATE_TEXT_ITEM`: turn a delivered letter's body into a Plain Letter in the bags.
/// Gates in order — the mailbox, bag room, the Realm-core copy, the Home Shard grant, the Realm-core
/// mark — so a full bag never touches the mail row, matching `take_item`'s ordering. Not an escrow:
/// the Plain Letter sells for 0, so a grant lost to bags filling between the room check and the
/// grant costs nothing. Every durable step is replay-safe, so a retry after an interrupted grant
/// reaches the Home Shard again instead of leaving the mail COPIED with nothing to show for it. The
/// mark is what makes a completed grant refuse a second one for good, even after the player
/// destroys, mails away, or trades the letter — `mail_grant_letter`'s own held-item check only
/// covers the narrow window before this call lands.
pub(crate) fn copy_letter<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> std::result::Result<(), CopyLetterRefusal> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)
        .map_err(|e| CopyLetterRefusal::NoMailbox(e.to_string()))?;
    store
        .mail_item_room(self_guid)
        .map_err(copy_letter_refusal)?;
    match store.realm_store() {
        Some(realm) => realm.mail_copy_text(self_guid, mail_id),
        None => store.mail_copy_text(self_guid, mail_id),
    }
    .map_err(copy_letter_refusal)?;
    // The text id is the mail id narrowed to u32 (`lyracore_shared::mail::item_text_id_for`'s
    // non-empty-body case) — the copy above just proved the body is non-empty.
    let item_text_id = u32::try_from(mail_id).unwrap_or(0);
    store
        .mail_grant_letter(self_guid, item_text_id)
        .map_err(copy_letter_refusal)?;
    match store.realm_store() {
        Some(realm) => realm.mail_mark_letter_granted(self_guid, mail_id),
        None => store.mail_mark_letter_granted(self_guid, mail_id),
    }
    .map_err(copy_letter_refusal)
}
/// `CMSG_ITEM_TEXT_QUERY`: the text behind `item_text_id`, for a caller who has PROVEN they may see
/// it — either they hold an item carrying that id, or they own the mail it names. `game_item_text`
/// ids are the mail's own id, small and sequential, so answering it for anyone who merely asks
/// would let a crafted query walk every copied letter on the realm. A caller who proves neither
/// gets empty text, the same answer a stale or foreign id has always produced.
///
/// `hint_item_guid` is the wire's own overloaded second field, forwarded to
/// [`MailStore::owns_item_with_text`] so it can try a cheap PK lookup before scanning. The
/// ownership scan runs only when the mail check does not already answer the question — most
/// queries are either "read my own undeleted mail" or "reread my own bagged letter," so one lookup
/// usually settles it.
///
/// A copied letter's text lives in `game_item_text` on the mail plane and outlives the mail that
/// held it; anything else falls back to the caller's own mail body under the same id, which is
/// what a letter still sitting in the mailbox resolves through today.
pub(crate) fn item_text<St: MailStore + ShardRoutingStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    item_text_id: u32,
    hint_item_guid: u64,
) -> Result<Option<String>> {
    let own_mail_body = letter_body(store, self_guid, u64::from(item_text_id))?;
    let owns_item = if own_mail_body.is_some() {
        false
    } else {
        match self_guid {
            Some(guid) => store.owns_item_with_text(guid, item_text_id, hint_item_guid)?,
            None => false,
        }
    };
    if !owns_item && own_mail_body.is_none() {
        return Ok(None);
    }
    let copied = match store.realm_store() {
        Some(realm) => realm.item_text(item_text_id),
        None => store.item_text(item_text_id),
    }?;
    Ok(copied.or(own_mail_body))
}
