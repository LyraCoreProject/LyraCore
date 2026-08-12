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
//! Here now: the table and its sweeps, the read path's seeder, mark-read, delete, and sending a
//! TEXT-ONLY letter. Attachments, take, return and COD are later slices — the attachment columns
//! exist because the wire has exactly one item block per mail and a later slice must not migrate
//! them in, not because anything writes them yet.
//!
//! The MECHANISM those slices move value with lives next door in [`crate::mail_escrow`]: on a
//! sharded realm the purse is on one database and the mail row on another, so a fence, a commit
//! keyed by a caller-chosen id, and a delete-last settle stand in for the transaction that cannot
//! span them. Nothing in this file goes through it — the single-database plane genuinely has that
//! transaction, and the postage debit #145 shipped is still the two-call path below.
//!
//! The wire-facing rules (the unread-mail float, the `item_text_id`, the expiry stamp) live in
//! `lyracore_shared::mail`: the gateway builds `SMSG_MAIL_LIST_RESULT` from a cache read, so a rule
//! kept here alone would have a second copy over there within one slice.

use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

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
/// one item block. All zero until the send slice writes them. [entity]
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
    /// Attached copper, and the cash-on-delivery price the taker owes. Both slices later.
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

/// Insert one mail. The shared insert core: whichever plane holds the rows, this is the shape they
/// take, so a seeded mail and a sent one can never differ in the columns the list read projects.
pub(crate) fn insert_mail(
    ctx: &ReducerContext,
    recipient_guid: u64,
    sender_guid: u64,
    subject: String,
    body: String,
    money: u32,
) -> u64 {
    ctx.db
        .game_mail()
        .insert(Mail {
            id: 0,
            recipient_guid,
            sender_guid,
            subject,
            body,
            item_entry: 0,
            item_stack_count: 0,
            item_durability: 0,
            item_enchant_id: 0,
            item_soulbound: false,
            money,
            cod: 0,
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
    let id = insert_mail(ctx, recipient_guid, sender_guid, subject, body, money);
    spacetimedb::log::info!(
        "debug_seed_mail: mail {id} to {recipient_guid} from {sender_guid} (unread now: {})",
        has_unread(ctx, recipient_guid)
    );
    Ok(())
}

/// Take `copper` out of `sender_guid`'s purse, or refuse and take nothing.
///
/// Reads the LIVE entity (`game_world_entity.money`), the way `trainer::buy` does: the durable
/// `game_character.money` is a mirror the logout persist writes, so debiting the row instead would
/// be overwritten by the sender's own session. Through `acting_entity_by_guid`, so a character
/// mid-transfer reads as absent rather than paying out of a purse that is in flight.
///
/// The refusal is the whole "a sender who cannot afford postage is charged nothing" guarantee: it
/// happens before any write, in the same transaction as the debit that would follow.
pub(crate) fn charge_postage(
    ctx: &ReducerContext,
    sender_guid: u64,
    copper: u32,
) -> Result<(), String> {
    let mut sender = crate::helpers::acting_entity_by_guid(ctx, sender_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_IN_WORLD.to_string())?;
    if sender.money < copper {
        return Err(lyracore_shared::mail::NOT_ENOUGH_MONEY.to_string());
    }
    sender.money -= copper;
    ctx.db.game_world_entity().guid().update(sender);
    Ok(())
}

/// Write one sent letter, charging `postage` first when this database holds the sender's purse.
///
/// `postage` is passed rather than derived because it decides WHERE the debit happens, and that is
/// the caller's knowledge, not this transaction's:
///
/// - **Single-database gateway** — the purse and the mail row are on one database, so the gateway
///   passes the real postage and the debit and the insert are ONE transaction. That is the shape
///   #9 asks for, and it is what `lyracore dev up` runs.
/// - **Sharded realm** — realm-core holds no characters, so the purse is on the sender's own shard
///   and one transaction is not reachable at all. The gateway charges there first
///   ([`realm_mail_charge_postage`]) and passes 0 here. Debit-first is deliberate: the failure it
///   leaves is a sender who paid postage and got no letter, which costs 30 copper and is logged,
///   against a letter delivered for free if the order were reversed.
///
/// Both planes reach [`insert_mail`], so the ROW is identical either way — a sent mail and a
/// seeded one cannot differ in a column the list read projects.
///
/// **This path must never route through [`crate::mail_escrow`], and the difference is not an
/// oversight to tidy up.** The escrow exists because two databases cannot share a transaction;
/// where they can, it is strictly worse — four transactions instead of one atomic write, plus a
/// fence that can be interrupted where nothing could be. Pinned by
/// `tests::the_single_database_send_path_never_reaches_the_escrow`.
pub(crate) fn apply_send(
    ctx: &ReducerContext,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    postage: u32,
) -> Result<(), String> {
    if postage > 0 {
        charge_postage(ctx, sender_guid, postage)?;
    }
    // No attached copper and no attachment: the money and item slices are later, and a text-only
    // letter is worth sending because the body rides `item_text_id` to the client's own query.
    insert_mail(ctx, recipient_guid, sender_guid, subject, body, 0);
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

/// Post one letter, as `sender_guid`, to `recipient_guid`. The write half of the send path.
///
/// **Operator-gated, and it has to be** — harder than the two above. Every gate that decides WHO may
/// write to whom (the recipient exists, the sender is in world, same faction, not yourself, standing
/// at a mailbox) runs in the gateway, because realm-core holds no characters and no gameobjects to
/// answer them with. So `sender_guid` here is not a claim this transaction can check: a client that
/// could call this would post mail as anybody on the realm, and the gateway passes only the guid it
/// authenticated for that socket.
///
/// `postage` is 0 on a sharded realm — see [`apply_send`] for why the debit is a separate call
/// there, and why it happens first.
#[reducer]
pub fn realm_mail_send(
    ctx: &ReducerContext,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    postage: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_send(ctx, sender_guid, recipient_guid, subject, body, postage)
}

/// Charge `sender_guid` the postage for a letter whose ROW is being written on another database.
///
/// The one reducer of this family that does NOT run on the mail plane: it runs on the shard holding
/// the sender's live entity, because that is where the copper is. Operator-gated for the same reason
/// as its siblings — it takes the payer's guid as an argument, so without the gate any connection
/// could empty a stranger's purse.
#[reducer]
pub fn realm_mail_charge_postage(
    ctx: &ReducerContext,
    sender_guid: u64,
    copper: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    charge_postage(ctx, sender_guid, copper)
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

#[cfg(test)]
mod tests {
    use crate::test_scan::read_scanned;

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
    fn the_single_database_send_path_never_reaches_the_escrow() {
        let body = code_of(include_str!("mail.rs"), "pub(crate) fn apply_send(");
        assert!(
            !body.contains("escrow"),
            "`apply_send` is the ONE-TRANSACTION plane. The escrow is the mechanism for the case \
             where a transaction cannot span the two databases; here one can. Body was:\n{body}"
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
            "pub fn realm_mail_send(",
            "pub fn realm_mail_charge_postage(",
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
