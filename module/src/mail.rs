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
//! This slice is the READ path: the table, the sweeps, and a debug seeder so a mailbox is demoable
//! before anything can send. Sending, attachments, take, mark-read, delete, return and COD are
//! later slices — the attachment columns exist because the wire has exactly one item block
//! per mail and a later slice must not migrate them in, not because anything writes them yet.
//!
//! The wire-facing rules (the unread-mail float, the `item_text_id`, the expiry stamp) live in
//! `lyracore_shared::mail`: the gateway builds `SMSG_MAIL_LIST_RESULT` from a cache read, so a rule
//! kept here alone would have a second copy over there within one slice.

use spacetimedb::{table, ReducerContext, Table, Timestamp};

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
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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
}
