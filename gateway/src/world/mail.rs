//! The mailbox READ path — the routing half of the mail slice, and the plane decision made once.
//!
//! # The two planes, and the one core between them
//!
//! `game_mail` is authoritative on **realm-core**: a mail is addressed to a character who may be
//! offline or homed on another database, which is the read that made a cross-boundary whisper
//! impossible. A gateway with no realm handle ([`WorldStore::realm_store`] → `None`) reads its own
//! database instead — whisper's shape verbatim.
//!
//! What is deliberately NOT duplicated is the read itself. [`mail_of`] picks the plane and then
//! every answer this module gives — the list, the unread poll, the letter body — is derived from
//! that one `Vec<MailView>`. Two planes cannot drift into two different mailbox views because there
//! is only one projection, and a mail cannot appear in the list but be missing from the poll.
//!
//! # The gates, and where each runs
//!
//! Realm-core has no gameobjects and no live entities, so the gates run here, before the read:
//!
//! - **in world** — a session at character select has no mailbox (`self_guid` is `None`);
//! - **at the named mailbox** — the client passes the mailbox guid in every mail packet, so this is
//!   a PK lookup plus a map/instance/range check on the session's OWN shard, where the gameobject
//!   is. It must never scan `game_gameobject` (sharded → silent subset).
//!
//! Every refusal is per-action: the caller logs it and answers the packet the client is waiting on,
//! or nothing at all. Nothing here is session-fatal.
//!
//! # Moving copper across the boundary
//!
//! A sharded send and a sharded take each move value between two databases that share no
//! transaction, so both go through the module's escrow and both are DRIVEN from here: fence,
//! commit, attest, settle, in that order and no other ([`drive`]). The gateway is the only
//! component that can see both databases, which is also why the recovery lives here — the module's
//! reaper settles an attested fence and holds everything else forever, and [`redrive`] is what
//! turns a held fence back into a delivered letter.
//!
//! A single-database gateway does none of this: the purse and the row are on one database, so the
//! send and the take are each ONE transaction. Keeping that path genuinely atomic is deliberate.
//!
//! # Poll, not push
//!
//! There is no relay and no event table. The client asks (`CMSG_GET_MAIL_LIST`,
//! `MSG_QUERY_NEXT_MAIL_TIME`), the gateway reads on demand. `SMSG_RECEIVED_MAIL` is out of scope
//! for the whole feature — do not add a push path here.

use anyhow::Result;

use super::{party, WorldStore};
use crate::codec::MailView;
use lyracore_shared::mail as mail_rules;

/// Why a send was refused, in the client's own vocabulary.
///
/// One variant per gate, because `SMSG_SEND_MAIL_RESULT` renders each as its own on-screen line and
/// that text is the whole diagnosability story for a refused send — a generic `ErrInternalError`
/// leaves the player staring at a letter that did not go, with no idea which rule stopped it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SendRefusal {
    /// Not in world, or not standing at the mailbox the client named.
    NoMailbox(String),
    /// Nobody on the realm answers to the typed name — or several do, and mail cannot pick.
    RecipientNotFound(String),
    CannotSendToSelf,
    NotYourTeam,
    /// The postage debit refused. The gates above already established the sender is in world with a
    /// live entity, so this is affordability and nothing else.
    NotEnoughMoney(String),
    /// The attached item is not the sender's, or does not exist. One variant for both, because the
    /// module answers one verdict for both: an item guid is client-supplied.
    AttachmentInvalid(String),
    /// The attached instance has bound to its owner. Its own variant so the client renders its own
    /// line — "soulbound" and "not yours" are different mistakes and a player must be able to tell.
    AttachmentSoulbound(String),
    /// The row insert failed after the gates passed — an unreachable database, not a rule. The one
    /// refusal that is genuinely internal.
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

/// Every mail addressed to `self_guid`, read from whichever plane owns the rows.
///
/// The ONE read both planes take. Sharded → realm-core, which is the only database that can address
/// a recipient who is offline or standing elsewhere. Unsharded → this handle, which already is the
/// authority; there is nothing to route.
pub(crate) fn mail_of<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: u64,
) -> Result<Vec<MailView>> {
    match store.realm_store() {
        Some(realm) => realm.mail_list(self_guid),
        None => store.mail_list(self_guid),
    }
}

/// The mailbox window's contents for a session standing at `mailbox_guid`.
///
/// `Err` is a refusal to answer at all — not in world, or not at that mailbox. An empty `Ok` is a
/// player with no mail, which is a real answer and must still be sent.
pub(crate) fn open_mailbox<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
) -> Result<Vec<MailView>> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    // Before the list, finish anything a dead gateway left in flight — this is the moment the
    // player would otherwise see a letter that never arrived or copper that never landed. Never
    // fatal: a failed re-drive leaves the fence for the next visit.
    redrive(store, self_guid);
    mail_of(store, self_guid)
}

/// Does the caller have unread mail? The answer behind the minimap envelope.
///
/// `MSG_QUERY_NEXT_MAIL_TIME` carries no mailbox guid — the client polls it from anywhere — so the
/// only gate is being in world. Derived from the same list read, so the envelope can never disagree
/// with what the mailbox window shows.
pub(crate) fn has_unread<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
) -> Result<bool> {
    let self_guid =
        self_guid.ok_or_else(|| anyhow::anyhow!(lyracore_shared::mail::NOT_IN_WORLD))?;
    Ok(mail_of(store, self_guid)?.iter().any(|m| !m.was_read))
}

/// The body of the caller's mail `mail_id` — the answer to `CMSG_ITEM_TEXT_QUERY`.
///
/// `None` for a mail that is not the caller's: the list read is already scoped to the recipient, so
/// a crafted packet naming someone else's mail finds nothing rather than reading their letter.
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

/// Flip mail `mail_id`'s read state for the session standing at `mailbox_guid`.
///
/// Same gate as [`open_mailbox`] (in world, at the named mailbox — vanilla's `CMSG_MAIL_MARK_AS_READ`
/// carries a mailbox guid too), then the SAME plane routing [`mail_of`] takes: whichever handle owns
/// the rows gets the write, so the write can never target a different database than the read that
/// will show it next. Authorization is the row lookup itself, on the plane that runs it — no
/// separate ownership check here, because a mail addressed to someone else simply isn't found there.
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

/// Delete mail `mail_id` for the session standing at `mailbox_guid`. Same gate and plane routing as
/// [`mark_read`]; see that function's doc for both.
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

/// Post one letter from the session standing at `mailbox_guid` to the character called
/// `recipient_name`.
///
/// # The gates, and why they are here
///
/// Realm-core holds no characters, no live entities and no gameobjects, so every question about WHO
/// may write to WHOM is answered here, before the reducer runs — whisper's split verbatim. In order:
/// in world and at the named mailbox; the recipient exists realm-wide; not yourself; same faction.
/// The module answers affordability, which is the one question it has the state for.
///
/// # The copper and the item
///
/// `money` travels WITH the letter and the recipient takes it out; the postage is what the post
/// office keeps. Both leave the purse in ONE debit at send — the sender cannot afford one without
/// the other, and a refused send costs nothing.
///
/// `item_guid` (0 for none) is the ONE attachment the vanilla wire carries. It is client-supplied,
/// so nothing here trusts it: the module resolves it, refuses it if it is not the sender's or if it
/// has bound, and otherwise DELETES the instance into a snapshot on the letter — in the same
/// transaction as the debit, so a refused send costs the sender neither the coin nor the item.
///
/// # Homonyms: refused, not guessed
///
/// A name can resolve to several characters — the uniqueness constraint is a per-DATABASE index
/// ([`party::resolve_all_by_name`]). Whisper disambiguates by picking the candidate that is ONLINE,
/// and mail cannot borrow that rule: reaching a character who is offline is the entire point of a
/// mailbox. So after the faction filter, a name that still names more than one person is REFUSED.
/// The rejected alternative is first-hit-wins, which silently posts an item and a purse of gold to
/// a stranger who can simply take it. A refusal costs the sender a retry; a misdelivery is
/// unrecoverable.
// The gates and the letter are one call deliberately: splitting them into a struct would put the
// item guid somewhere a caller could forget to fill in.
#[allow(clippy::too_many_arguments)]
pub(crate) fn send<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    recipient_name: &str,
    subject: String,
    body: String,
    money: u32,
    item_guid: u64,
) -> std::result::Result<(), SendRefusal> {
    let sender_guid = at_mailbox(store, self_guid, mailbox_guid)
        .map_err(|e| SendRefusal::NoMailbox(e.to_string()))?;
    // The sender's own liveness, unioned across the shards — the same read whisper gates on, and
    // the reason a character mid-hop cannot post mail out of a purse that is in flight.
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
    // Self BEFORE faction: a player who types their own name gets vanilla's own message, not a
    // faction line that would be nonsense about themselves.
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
    match store.realm_store() {
        // One database: the purse and the row are both on it, so the debit and the insert are one
        // transaction — the property the design asks for, and the only plane that can have it.
        None => store
            .mail_send(sender_guid, recipient_guid, subject, body, money, item_guid)
            .map_err(refusal_from_module),
        // Sharded: realm-core holds no purse, so no transaction covers both. The escrow does
        // instead — fence the whole cost on the sender's shard, commit the row on realm-core,
        // settle last.
        Some(realm) => {
            let escrow_id = next_escrow_id();
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
                )
                .map_err(refusal_from_module)?;
            // The fence is where the item stopped being a row and became a snapshot, so the
            // attachment the commit writes is read back off the fence rather than re-derived — one
            // source, and the same one `redrive` reads.
            let item = held_attachment(store, sender_guid, escrow_id)
                .map_err(|e| SendRefusal::Internal(format!("{e:#}")))?;
            // Past the fence the sender has PAID. Every failure from here is recoverable and none
            // is refundable, so it is reported as internal and the fence is left for a re-drive.
            drive(store, escrow_id, || {
                realm.mail_commit(
                    escrow_id,
                    sender_guid,
                    recipient_guid,
                    subject,
                    body,
                    money,
                    item.clone(),
                )
            })
            .map_err(|e| SendRefusal::Internal(format!("{e:#}")))
        }
    }
}

/// **Steps 2–4 of an escrow drive: commit, attest, settle.** One function for both directions and
/// for the re-drive, because the ordering is the whole safety argument and a second copy of it is a
/// second chance to get it wrong.
///
/// `source` is the handle HOLDING the fence — the sender's shard for a letter, the mail plane for a
/// take. `commit` is the step on the other database, whichever that is. The attestation may only
/// happen once `commit` has RETURNED: attesting first licenses destroying a fence whose value never
/// arrived, which is the one unrecoverable outcome.
///
/// Re-entrant. Every step is idempotent on the escrow id, so calling this again after any of them
/// replays the landed ones into no-ops — which is how [`redrive`] rescues an abandoned fence.
fn drive<St, F>(source: &St, escrow_id: u64, commit: F) -> Result<()>
where
    St: WorldStore + ?Sized,
    F: FnOnce() -> Result<()>,
{
    commit()?;
    source.mail_confirm_delivery(escrow_id)?;
    source.mail_settle(escrow_id)
}

/// The ONE item a letter carries, as the gateway moves it between databases. The mail row's own
/// attachment columns and the escrow row's, which are the same five columns: entry, stack,
/// durability, enchant and bind state. `entry` 0 is "no attachment".
///
/// Every column is here because mailing must not launder value — a re-grant from the template alone
/// would hand back a repaired, unenchanted item.
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

/// The attachment a fence is holding, read back off the escrow ROW rather than re-derived from what
/// the caller thought it was sending — `transfer`'s "read the escrow back rather than trusting the
/// plan", for the identical reason: the row on disk is what actually landed.
///
/// A missing row is an `Err`, never an empty attachment: committing a letter with no item while the
/// fence holds one would strand the item and tell the sender it went.
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

/// One escrow row the gateway is holding, flattened for the drive. Everything a re-drive needs is
/// here, because a fence carries its own letter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeldEscrow {
    pub escrow_id: u64,
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    pub money: u32,
    pub postage: u32,
    /// `true` when this fence is a mail row's copper on its way into a purse, `false` when it is a
    /// purse's copper on its way into a mail row. It decides which database is the source and which
    /// is the destination, and nothing else about the row does.
    pub payout: bool,
    pub mail_id: u64,
    /// The attachment in flight. Empty when the fence carries only coin.
    pub item: AttachedItem,
}

/// Finish every escrow left standing for `self_guid`, on both planes.
///
/// **This is the recovery path, and without it a gateway that died mid-send strands a player's gold
/// forever.** The module's reaper settles an ATTESTED fence and holds everything else — correctly,
/// because a shard cannot ask realm-core whether the letter arrived. Only the gateway can, by
/// re-driving the sequence forward: the commit replays into its receipt, the attestation lands, the
/// settle releases the fence.
///
/// Called when the player opens a mailbox — the moment they would notice the missing copper, and a
/// point where the two handles are both to hand. The rejected alternative was a background sweep
/// over every escrow on the realm: it would need its own scheduler and would re-drive fences for
/// characters this gateway has no session for.
///
/// Never fatal. A re-drive that fails leaves the fence exactly as it was, for the next attempt.
pub(crate) fn redrive<St: WorldStore + ?Sized>(store: &St, self_guid: u64) {
    let Some(realm) = store.realm_store() else {
        return; // One database, one transaction, no fences to rescue.
    };
    // The letters this player sent: fenced on their own shard, destined for a mail row on realm.
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
            )
        });
        log_redrive("send", held.escrow_id, outcome);
    }
    // The takes this player started: fenced on realm-core, destined for their purse on this shard.
    for held in realm.mail_escrows_of(self_guid).unwrap_or_default() {
        if !held.payout {
            continue;
        }
        // Which payout depends on WHAT the fence is holding, and nothing else: a take fence carries
        // either a mail row's copper or its item, never both — the two opcodes are separate clicks.
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
        // Loud: the copper is still fenced and still recoverable, but somebody has to know it is
        // sitting there.
        Err(e) => log::warn!(
            "mail escrow {escrow_id}: {kind} re-drive failed, the fence is still HELD: {e:#}"
        ),
    }
}

/// Take mail `mail_id`'s copper into the purse of the session standing at `mailbox_guid`.
///
/// The authorization is the row lookup scoped to the caller, run on whichever plane owns the row —
/// a mail id arrives from the client, so nothing here may trust it. Same gate as [`open_mailbox`]
/// first, for the same reason.
///
/// The mail itself survives; only its `money` is cleared, so a letter stays readable until the
/// recipient deletes it.
pub(crate) fn take_money<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> Result<()> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid)?;
    let Some(realm) = store.realm_store() else {
        // One database: the credit and the clear are one transaction, so "taking twice credits
        // once" needs no protocol at all.
        return store.mail_take_money(self_guid, mail_id);
    };
    // Sharded: the row is on realm-core and the purse is on this shard. The amount has to travel
    // with the drive, so it is read here and the fence refuses if the row disagrees — a stale read
    // that fenced less than the payout credits would mint the difference.
    let amount = mail_of(store, self_guid)?
        .into_iter()
        .find(|m| m.id == mail_id)
        .map(|m| m.money)
        .ok_or_else(|| anyhow::anyhow!(mail_rules::NOT_YOUR_MAIL))?;
    if amount == 0 {
        anyhow::bail!(mail_rules::NOTHING_TO_TAKE);
    }
    let escrow_id = next_escrow_id();
    realm.mail_take_money_fence(escrow_id, self_guid, mail_id, amount)?;
    // Past the fence the copper has left the row. A failure here is reported — the player's click
    // did not land — but the fence keeps the copper, and the next mailbox visit re-drives it.
    drive(realm.as_ref(), escrow_id, || {
        store.mail_payout(escrow_id, self_guid, mail_id, amount)
    })
}

/// Why an item take was refused, in the client's own vocabulary.
///
/// Only the full-bag arm gets its own variant: `MailResultTwo::ErrEquipError` is the one the client
/// renders as "make room", and it is the refusal a player can actually act on. Everything else —
/// not your mail, nothing in it, an unreachable database — reads the same on the wire, so a crafted
/// mail id learns nothing from the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TakeItemRefusal {
    BagsFull(String),
    Other(String),
}

impl std::fmt::Display for TakeItemRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BagsFull(e) | Self::Other(e) => f.write_str(e),
        }
    }
}

fn take_item_refusal(e: anyhow::Error) -> TakeItemRefusal {
    let text = format!("{e:#}");
    if text.contains(mail_rules::INVENTORY_FULL) {
        TakeItemRefusal::BagsFull(text)
    } else {
        TakeItemRefusal::Other(text)
    }
}

/// Take mail `mail_id`'s attached item into the bags of the session standing at `mailbox_guid`.
///
/// [`take_money`]'s twin, and the same authorization: the row lookup scoped to the caller, run on
/// whichever plane owns the row. A mail id arrives from the client, so nothing here may trust it.
///
/// # The full bag, and why the sharded plane probes first
///
/// One database does the whole take in one transaction, so a full bag rolls the clear back and the
/// item simply stays in the letter. Two databases cannot have that: the fence empties the letter
/// before the taker's shard is asked, so a full bag found afterwards would leave the item held in
/// an escrow rather than in the mail. So the sharded path asks the taker's shard for room BEFORE it
/// fences ([`WorldStore::mail_item_room`]). The probe is advisory — a bag filled in between leaves
/// the item held and re-driven at the next mailbox visit, never destroyed — but it is what makes
/// the ordinary full-bag click a clean refusal with the item still in the mail.
pub(crate) fn take_item<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    mail_id: u64,
) -> std::result::Result<(u32, u32), TakeItemRefusal> {
    let self_guid = at_mailbox(store, self_guid, mailbox_guid).map_err(take_item_refusal)?;
    // The snapshot is read here on BOTH planes: the wire's success arm names the item and stack the
    // client just gained, and the sharded drive has to carry the snapshot to the other database
    // anyway. The read is already scoped to the reader, so a crafted mail id finds nothing.
    let item = mail_of(store, self_guid)
        .map_err(take_item_refusal)?
        .into_iter()
        .find(|m| m.id == mail_id)
        .map(|m| AttachedItem {
            entry: m.item_entry,
            stack_count: m.item_stack_count,
            durability: m.item_durability,
            enchant_id: m.item_enchant_id,
            soulbound: m.item_soulbound,
        })
        .ok_or_else(|| TakeItemRefusal::Other(mail_rules::NOT_YOUR_MAIL.to_string()))?;
    if item.is_empty() {
        return Err(TakeItemRefusal::Other(
            mail_rules::NOTHING_TO_TAKE.to_string(),
        ));
    }
    let taken = (item.entry, item.stack_count);
    let Some(realm) = store.realm_store() else {
        // One database: the grant and the clear are one transaction, so a full bag refuses itself
        // and the item stays in the letter with no protocol at all.
        store
            .mail_take_item(self_guid, mail_id)
            .map_err(take_item_refusal)?;
        return Ok(taken);
    };
    // Sharded: ask for room BEFORE fencing. Past the fence the letter is empty, so a full bag found
    // afterwards would leave the item held in an escrow rather than in the mail.
    store.mail_item_room(self_guid).map_err(take_item_refusal)?;
    let escrow_id = next_escrow_id();
    // The fence refuses if the row disagrees with what was read — a stale read that fenced one item
    // and granted another would mint the difference.
    realm
        .mail_take_item_fence(escrow_id, self_guid, mail_id, item.entry)
        .map_err(take_item_refusal)?;
    // Past the fence the item has left the letter. A failure here is reported — the player's click
    // did not land — but the fence keeps the item, and the next mailbox visit re-drives it.
    drive(realm.as_ref(), escrow_id, || {
        store.mail_item_payout(escrow_id, self_guid, mail_id, item.clone())
    })
    .map_err(take_item_refusal)?;
    Ok(taken)
}

/// The next caller-chosen escrow id.
///
/// Caller-chosen is the requirement: it is the idempotency key on both databases, so it must be
/// stable across a retry of one drive — which a local holds — and distinct from every OTHER fence
/// in flight. Seeding the counter from the process start time and bumping it makes two gateways
/// collide only if they started in the same microsecond; if they ever did, the module's
/// `IdCollision` guards refuse the second rather than paying it, so the failure is a refused send
/// and not a lost purse. `auto_inc` cannot serve: it is per-database, and this key spans two.
fn next_escrow_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    NEXT.get_or_init(|| {
        AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                // 0 is the module's "no escrow" sentinel, so never start there.
                .unwrap_or(1)
                .max(1),
        )
    })
    .fetch_add(1, Ordering::Relaxed)
}

/// Read a module-side failure into the refusal the client is shown.
///
/// Affordability and the attachment verdict are the rules the module owns on this path, and each
/// answers with the shared text both halves compile against (`ERR_ATTACK_TARGET_DEAD`'s
/// convention), so the mapping cannot drift. Anything else really is internal — an unreachable
/// database, not a rule.
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

/// The gate every mailbox-addressed opcode opens with: in world, and standing at the gameobject the
/// client named. Answers the caller's own guid so the read below cannot accidentally use another.
fn at_mailbox<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
) -> Result<u64> {
    let self_guid =
        self_guid.ok_or_else(|| anyhow::anyhow!(lyracore_shared::mail::NOT_IN_WORLD))?;
    // The session's OWN handle, never the realm one: the mailbox is a gameobject, and realm-core
    // holds none — asking it would refuse every mailbox on a sharded realm.
    if !store.mailbox_in_range(mailbox_guid, self_guid)? {
        anyhow::bail!(lyracore_shared::mail::not_at_mailbox(mailbox_guid));
    }
    Ok(self_guid)
}
