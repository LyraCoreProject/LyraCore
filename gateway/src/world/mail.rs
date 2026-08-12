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
    /// The row insert failed after the gates passed — an unreachable database, not a rule. The one
    /// refusal that is genuinely internal.
    Internal(String),
}

impl std::fmt::Display for SendRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMailbox(e) | Self::RecipientNotFound(e) | Self::NotEnoughMoney(e) => {
                f.write_str(e)
            }
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
/// # Homonyms: refused, not guessed
///
/// A name can resolve to several characters — the uniqueness constraint is a per-DATABASE index
/// ([`party::resolve_all_by_name`]). Whisper disambiguates by picking the candidate that is ONLINE,
/// and mail cannot borrow that rule: reaching a character who is offline is the entire point of a
/// mailbox. So after the faction filter, a name that still names more than one person is REFUSED.
/// The rejected alternative is first-hit-wins, which silently posts a letter — and, once the
/// attachment slices land, an item and a purse of gold — to a stranger who can simply take it. A
/// refusal costs the sender a retry; a misdelivery is unrecoverable.
pub(crate) fn send<St: WorldStore + ?Sized>(
    store: &St,
    self_guid: Option<u64>,
    mailbox_guid: u64,
    recipient_name: &str,
    subject: String,
    body: String,
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
    // A text-only letter attaches no coin, so the total the sender pays is the bare postage. The
    // argument exists because the money slice must debit ONE total rather than charging twice.
    let cost = mail_rules::total_cost(0);
    match store.realm_store() {
        // One database: the purse and the row are both on it, so the debit and the insert are one
        // transaction — the property the design asks for, and the only plane that can have it.
        None => store
            .mail_send(sender_guid, recipient_guid, subject, body, cost)
            .map_err(refusal_from_module),
        // Sharded: realm-core holds no purse, so the debit runs on the sender's own shard and the
        // two writes cannot be one transaction. Debit FIRST — it is the atomic affordability check,
        // so a refused send is charged nothing. The residue in the other direction is 30 copper on
        // an unreachable realm-core, against a letter delivered free if the order were reversed.
        Some(realm) => {
            store
                .mail_charge_postage(sender_guid, cost)
                .map_err(refusal_from_module)?;
            realm
                .mail_send(sender_guid, recipient_guid, subject, body, 0)
                .map_err(refusal_from_module)
        }
    }
}

/// Read a module-side failure into the refusal the client is shown.
///
/// Affordability is the only rule the module owns on this path, and it answers with the shared text
/// both halves compile against (`ERR_ATTACK_TARGET_DEAD`'s convention), so the mapping cannot drift.
/// Anything else really is internal — an unreachable database, not a rule.
fn refusal_from_module(e: anyhow::Error) -> SendRefusal {
    let text = format!("{e:#}");
    if text.contains(mail_rules::NOT_ENOUGH_MONEY) {
        SendRefusal::NotEnoughMoney(text)
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
