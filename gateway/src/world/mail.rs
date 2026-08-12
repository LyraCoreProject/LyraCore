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

use super::WorldStore;
use crate::codec::MailView;

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
