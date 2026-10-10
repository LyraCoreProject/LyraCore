//! `Coordinator`'s [`MailStore`] adapter.

use anyhow::Result;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, MailStore};

impl MailStore for Coordinator {
    /// Every mail addressed to `recipient_guid`, delivered or not, in no set order.
    /// `codec::build_mail_list` orders the inbox. The SDK exposes only the PK index, so this
    /// iterates and filters like every other per-owner read here (`player_items`, `player_skills`).
    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<crate::codec::MailView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_mail()
            .iter()
            .filter(|m| m.recipient_guid == recipient_guid)
            .map(|m| mail_view(db, m))
            .collect())
    }

    /// The mail `mail_id`, delivered or not, by its primary key.
    fn mail_by_id(&self, mail_id: u64) -> Result<Option<crate::codec::MailView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db.game_mail().id().find(&mail_id).map(|m| mail_view(db, m)))
    }

    fn realm_account_name(&self, character_guid: u64) -> Result<Option<String>> {
        Ok(self.realm_account_name(character_guid))
    }

    /// Is `player_guid` standing at the mailbox `mailbox_guid` names?
    ///
    /// A PK lookup on `game_gameobject`, then the same map/instance/range check
    /// `module/src/gameobject.rs::usable_go` applies to a chest. It must STAY a PK lookup:
    /// `game_gameobject` is spatial, and a scan over a sharded table returns a silent subset, so a
    /// mailbox availability would depend on which database the session reads.
    ///
    /// `false` for an unknown guid, a non-mailbox gameobject, another map or instance, and anything
    /// out of range — the gate answers the question, and the caller decides what a refusal costs.
    fn mailbox_in_range(&self, mailbox_guid: u64, player_guid: u64) -> Result<bool> {
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

    /// `realm_mail_mark_read` — flip a mail's read state against the database THIS handle points
    /// at. Operator-gated, `recipient` passed
    /// explicitly (the plane may hold no live entity), called on whichever handle `world::mail`
    /// picked — realm-core when sharded, this shard's own database when not.
    fn mail_mark_read(&self, recipient: Actor, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_mark_read",
            realm_mail_mark_read_then(self.session_actor(recipient), mail_id)
        )
    }

    /// `realm_mail_delete` — [`mail_mark_read`](Self::mail_mark_read)'s twin for delete.
    fn mail_delete(&self, recipient: Actor, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_delete",
            realm_mail_delete_then(self.session_actor(recipient), mail_id)
        )
    }

    /// `realm_mail_return` — [`mail_delete`](Self::mail_delete)'s twin for return-to-sender: the row
    /// is re-addressed in place, on the database THIS handle points at. No sharded variant — the row
    /// never leaves the plane that already holds it.
    fn mail_return(&self, recipient: Actor, mail_id: u64, same_account: bool) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_return",
            realm_mail_return_then(self.session_actor(recipient), mail_id, same_account)
        )
    }

    /// `realm_mail_send` — write one sent letter against the database THIS handle points at, and
    /// charge the sender for it in the same transaction. Same trust shape as the two above, and the
    /// guid it carries is the one the socket authenticated: every gate deciding who may write to
    /// whom ran in `world::mail`, because realm-core can answer none of them.
    ///
    /// **The single-database plane only.** A sharded realm cannot have that one transaction and
    /// drives the escrow below instead.
    #[allow(clippy::too_many_arguments)]
    fn mail_send(
        &self,
        sender: Actor,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item_guid: u64,
        same_account: bool,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_send",
            realm_mail_send_then(
                self.session_actor(sender),
                recipient_guid,
                subject,
                body,
                money,
                cod,
                item_guid,
                same_account
            )
        )
    }

    /// `realm_mail_take_money` — credit a mail's copper to the recipient and empty the row, in one
    /// transaction. The single-database plane's whole take, for the same reason.
    fn mail_take_money(&self, recipient: Actor, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_money",
            realm_mail_take_money_then(self.session_actor(recipient), mail_id)
        )
    }

    /// `realm_mail_take_item` — re-create a mail's attachment in the recipient's bags and empty the
    /// row, in one transaction. The single-database plane's whole item take.
    fn mail_take_item(&self, recipient: Actor, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_item",
            realm_mail_take_item_then(self.session_actor(recipient), mail_id)
        )
    }

    /// `realm_mail_item_room` — the bag-space probe a sharded item take runs BEFORE it fences, on
    /// the taker's own handle. A read dressed as a reducer, because only the module can answer it
    /// without a second copy of the bag search.
    fn mail_item_room(&self, payee: Actor) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_item_room",
            realm_mail_item_room_then(self.session_actor(payee))
        )
    }

    /// `realm_mail_copy_text` — Letter Copy step 1, against the database THIS handle points at.
    /// Same trust shape as `mail_mark_read`: operator-gated, `recipient` passed explicitly.
    fn mail_copy_text(&self, recipient: Actor, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_copy_text",
            realm_mail_copy_text_then(self.session_actor(recipient), mail_id)
        )
    }

    /// `gw_mail_grant_letter` — Letter Copy step 2, on the PAYEE's own handle: grants one Plain
    /// Letter carrying `item_text_id`.
    fn mail_grant_letter(&self, payee: Actor, item_text_id: u32) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_mail_grant_letter",
            gw_mail_grant_letter_then(self.session_actor(payee), item_text_id)
        )
    }

    /// `realm_mail_mark_letter_granted` — Letter Copy step 3, against the database THIS handle
    /// points at: the durable record that the Home Shard grant landed.
    fn mail_mark_letter_granted(&self, recipient: Actor, mail_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_mark_letter_granted",
            realm_mail_mark_letter_granted_then(self.session_actor(recipient), mail_id)
        )
    }

    /// A copied letter's text, read from `game_item_text` on THIS handle's database. Private table,
    /// read through the owner token, by its PK — the same shape `mailbox_in_range` uses to resolve
    /// a gameobject.
    fn item_text(&self, item_text_id: u32) -> Result<Option<String>> {
        let guard = self.0.coord();
        Ok(guard
            .conn
            .db
            .game_item_text()
            .id()
            .find(&item_text_id)
            .map(|t| t.text))
    }

    /// Does `owner_guid` hold an item carrying `item_text_id`? `CMSG_ITEM_TEXT_QUERY`'s ownership
    /// Gate: a client cannot use this to probe what anyone else holds, since the answer is a bare
    /// bool. Read from the privileged cache.
    ///
    /// `hint_item_guid` is the wire's own second field — a bag item guid when the client is
    /// reading that item's tooltip (`cm:MailHandler.cpp:630-646`), and vmangos always sends it for
    /// this shape of the query. A PK lookup answers in O(1); a miss (item deleted, wrong id, or a
    /// crafted query) answers `false` rather than falling back to a table-wide scan — this runs on
    /// every such query, including ones for an id that names nothing, and the SDK exposes no
    /// owner-keyed index to narrow it by.
    fn owns_item_with_text(
        &self,
        owner_guid: u64,
        item_text_id: u32,
        hint_item_guid: u64,
    ) -> Result<bool> {
        if item_text_id == 0 || hint_item_guid == 0 {
            return Ok(false);
        }
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let owns = db
            .game_item_instance()
            .guid()
            .find(&hint_item_guid)
            .is_some_and(|item| item.owner_guid == owner_guid && item.item_text_id == item_text_id);
        Ok(owns)
    }

    /// `realm_mail_fence` — step 1 of a sharded SEND, on the SENDER's own handle: the postage plus
    /// the attached coin leave the purse into an escrow row keyed by the caller-chosen `escrow_id`.
    #[allow(clippy::too_many_arguments)]
    fn mail_fence(
        &self,
        escrow_id: u64,
        sender: Actor,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        postage: u32,
        item_guid: u64,
        cod: u32,
        cod_source_mail_id: u64,
        same_account: bool,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_fence",
            realm_mail_fence_then(
                escrow_id,
                self.session_actor(sender),
                recipient_guid,
                subject,
                body,
                money,
                postage,
                item_guid,
                cod,
                cod_source_mail_id,
                same_account
            )
        )
    }

    /// `realm_mail_commit` — step 2 of a sharded send, on the REALM handle: the mail row plus a
    /// receipt under the same `escrow_id`, so a replay writes one letter and not two. A Reward
    /// Letter commits here on every plane, with its `reward` header.
    #[allow(clippy::too_many_arguments)]
    fn mail_commit(
        &self,
        escrow_id: u64,
        sender: Actor,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        item: crate::world::mail::AttachedItem,
        cod: u32,
        cod_source_mail_id: u64,
        delivery_delay_secs: u32,
        reward: Option<lyracore_shared::mail::RewardHeader>,
    ) -> Result<()> {
        let (sender_kind, sender_entry, mail_template_id) =
            lyracore_shared::mail::RewardHeader::columns(reward);
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_commit",
            realm_mail_commit_then(
                escrow_id,
                self.session_actor(sender),
                recipient_guid,
                subject,
                body,
                money,
                item.entry,
                item.stack_count,
                item.durability,
                item.enchant_id,
                item.soulbound,
                item.random_property_id,
                cod,
                cod_source_mail_id,
                delivery_delay_secs,
                sender_kind,
                sender_entry,
                mail_template_id,
                item.item_text_id
            )
        )
    }

    /// `realm_mail_take_money_fence` — step 1 of a sharded TAKE, on the handle that OWNS THE MAIL
    /// ROW: the copper leaves the row into an escrow there. The mirror of `mail_fence`.
    fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee: Actor,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_money_fence",
            realm_mail_take_money_fence_then(
                escrow_id,
                self.session_actor(payee),
                mail_id,
                expect_money
            )
        )
    }

    /// `realm_mail_payout` — step 2 of a sharded take, on the TAKER's own handle: the purse plus a
    /// receipt under the same `escrow_id`. `mail_commit`'s twin.
    fn mail_payout(&self, escrow_id: u64, payee: Actor, mail_id: u64, amount: u32) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_payout",
            realm_mail_payout_then(escrow_id, self.session_actor(payee), mail_id, amount)
        )
    }

    /// `realm_mail_take_item_fence` — step 1 of a sharded ITEM take, on the handle that OWNS THE
    /// MAIL ROW: the attachment leaves the row into an escrow there.
    fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee: Actor,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_take_item_fence",
            realm_mail_take_item_fence_then(
                escrow_id,
                self.session_actor(payee),
                mail_id,
                expect_entry
            )
        )
    }

    /// `realm_mail_item_payout` — step 2 of a sharded item take, on the TAKER's own handle: the
    /// item plus a receipt under the same `escrow_id`. `mail_payout`'s twin.
    fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee: Actor,
        mail_id: u64,
        item: crate::world::mail::AttachedItem,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_item_payout",
            realm_mail_item_payout_then(
                escrow_id,
                self.session_actor(payee),
                mail_id,
                item.entry,
                item.stack_count,
                item.durability,
                item.enchant_id,
                item.soulbound,
                item.random_property_id,
                item.item_text_id
            )
        )
    }

    /// `realm_mail_confirm_delivery` — step 3, on the handle that HOLDS THE FENCE. The attestation
    /// that the other database committed, and the only thing that licenses the settle.
    fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_confirm_delivery",
            realm_mail_confirm_delivery_then(escrow_id, self.owner_actor())
        )
    }

    /// `realm_mail_settle` — step 4, on the handle that holds the fence: destroy it. Delete-last.
    fn mail_settle(&self, escrow_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_mail_settle",
            realm_mail_settle_then(escrow_id, self.owner_actor())
        )
    }

    /// Every mail escrow this database is holding for `sender_guid` — the fences a drive filed and
    /// never finished.
    ///
    /// The one module→gateway data flow the escrow adds, and it exists for the same reason
    /// `escrowed_transfer` does: the gateway is the only component that can see both databases, so
    /// re-driving a stalled fence means re-deriving the whole letter from its row. A Character's fresh
    /// send reads nothing more than its own fence; a Reward Letter, which the Module files at
    /// turn-in, is always driven from its row. Private table, read through the owner token.
    ///
    /// Keyed by `sender_guid`, which on a payout escrow is the PAYEE — the character owed the
    /// copper either way, and the one whose session is about to re-drive it.
    fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<crate::world::mail::HeldEscrow>> {
        let guard = self.0.coord();
        let ids = guard.mail_escrows.read().unwrap().of(sender_guid);
        let escrows = guard.conn.db.game_mail_escrow();
        Ok(ids
            .into_iter()
            .filter_map(|id| escrows.escrow_id().find(&id))
            .filter_map(|e| {
                // A row no letter could have written stays held rather than commit as the wrong
                // letter.
                let reward = lyracore_shared::mail::RewardHeader::from_columns(
                    e.sender_kind,
                    e.sender_entry,
                    e.mail_template_id,
                )
                .map_err(|refusal| {
                    log::error!("mail escrow {}: not driven: {refusal}", e.escrow_id)
                })
                .ok()?;
                Some((e, reward))
            })
            .map(|(e, reward)| crate::world::mail::HeldEscrow {
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
                    random_property_id: e.random_property_id,
                    item_text_id: e.item_text_id,
                },
                cod: e.cod,
                delivery_delay_secs: e.delivery_delay_secs,
                reward,
            })
            .collect())
    }
}

fn mail_view(db: &RemoteTables, m: Mail) -> crate::codec::MailView {
    crate::codec::MailView {
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
        random_property_id: m.random_property_id,
        money: m.money,
        cod: m.cod,
        was_read: m.was_read,
        created_at_secs: m.created_at.to_micros_since_unix_epoch() / 1_000_000,
        sender_kind: m.sender_kind,
        sender_entry: m.sender_entry,
        check_flags: m.check_flags,
        mail_template_id: m.mail_template_id,
        // Rounded up to the second. The Gateway compares it with its own clock, which can run
        // ahead of the Module's, so the Module Gates mark-read, delete, every take, the return
        // and a COD payment against its own clock again.
        deliver_secs: m
            .deliver_micros
            .saturating_add(999_999)
            .div_euclid(1_000_000),
    }
}
