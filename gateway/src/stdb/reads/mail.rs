//! Mail cache-accessor methods: the mailbox list, and the mailbox-proximity gate.
//!
//! Both read the privileged coordinator cache. Which DATABASE that cache belongs to is the caller's
//! decision, not this file's — `world::mail` asks the realm-core handle when there is one and the
//! session's own handle when there is not, and both reach the same body here.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;

impl Coordinator {
    /// Every mail addressed to `recipient_guid`, oldest first — the order the mailbox window lists
    /// them in. The SDK exposes only the PK index, so this iterates and filters like every other
    /// per-owner read here (`player_items`, `player_skills`).
    pub fn mail_list(&self, recipient_guid: u64) -> Result<Vec<crate::codec::MailView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut mails: Vec<crate::codec::MailView> = db
            .game_mail()
            .iter()
            .filter(|m| m.recipient_guid == recipient_guid)
            .map(|m| crate::codec::MailView {
                id: m.id,
                sender_guid: m.sender_guid,
                subject: m.subject,
                body: m.body,
                item_entry: m.item_entry,
                item_stack_count: m.item_stack_count,
                item_durability: m.item_durability,
                // The row only ever snapshots CURRENT durability (mail.rs's `ItemSnapshot`); the
                // true max lives on the attachment's own template, the same read `player_items`
                // joins for. 0 for no attachment — `entry().find(0)` finds nothing.
                max_durability: db
                    .game_item_template()
                    .entry()
                    .find(&m.item_entry)
                    .map(|t| t.max_durability)
                    .unwrap_or(0),
                item_enchant_id: m.item_enchant_id,
                item_soulbound: m.item_soulbound,
                money: m.money,
                cod: m.cod,
                was_read: m.was_read,
                created_at_secs: m.created_at.to_micros_since_unix_epoch() / 1_000_000,
            })
            .collect();
        mails.sort_by_key(|m| m.id);
        Ok(mails)
    }

    /// Every mail escrow this database is holding for `sender_guid` — the fences a drive filed and
    /// never finished.
    ///
    /// The one module→gateway data flow the escrow adds, and it exists for the same reason
    /// `escrowed_transfer` does: the gateway is the only component that can see both databases, so
    /// re-driving a stalled fence means re-deriving the whole letter from its row. A fresh drive
    /// reads nothing. Private table, read through the owner token.
    ///
    /// Keyed by `sender_guid`, which on a payout escrow is the PAYEE — the character owed the
    /// copper either way, and the one whose session is about to re-drive it.
    pub fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<crate::world::mail::HeldEscrow>> {
        let guard = self.0.coord();
        Ok(guard
            .conn
            .db
            .game_mail_escrow()
            .iter()
            .filter(|e| e.sender_guid == sender_guid)
            .map(|e| crate::world::mail::HeldEscrow {
                escrow_id: e.escrow_id,
                recipient_guid: e.recipient_guid,
                subject: e.subject,
                body: e.body,
                money: e.money,
                postage: e.postage,
                payout: e.payout,
                mail_id: e.mail_id,
                item: crate::world::mail::AttachedItem {
                    entry: e.item_entry,
                    stack_count: e.item_stack_count,
                    durability: e.item_durability,
                    enchant_id: e.item_enchant_id,
                    soulbound: e.item_soulbound,
                },
                cod: e.cod,
            })
            .collect())
    }

    /// Is `player_guid` standing at the mailbox `mailbox_guid` names?
    ///
    /// A PK lookup on `game_gameobject`, then the same map/instance/range check
    /// `module/src/gameobject.rs::usable_go` applies to a chest. It must STAY a PK lookup:
    /// `game_gameobject` is spatial, and a scan over a sharded table returns a silent subset, so a
    /// mailbox would work or not depending on which database the session happens to read
    /// (`module/src/mail.rs` pins this by source scan).
    ///
    /// `false` for an unknown guid, a non-mailbox gameobject, another map or instance, and anything
    /// out of range — the gate answers the question, and the caller decides what a refusal costs.
    pub fn mailbox_in_range(&self, mailbox_guid: u64, player_guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let (Some(go), Some(player)) = (
            db.game_gameobject().guid().find(&mailbox_guid),
            db.game_world_entity().guid().find(&player_guid),
        ) else {
            return Ok(false);
        };
        let is_mailbox = db
            .game_gameobject_template()
            .entry()
            .find(&go.template_entry)
            .is_some_and(|t| t.type_id == lyracore_shared::mail::MAILBOX_GO_TYPE);
        if !is_mailbox || go.map_id != player.map_id || go.instance_id != player.instance_id {
            return Ok(false);
        }
        let (dx, dy, dz) = (go.x - player.x, go.y - player.y, go.z - player.z);
        Ok(dx * dx + dy * dy + dz * dz <= lyracore_shared::mail::MAILBOX_RANGE_SQ)
    }
}
