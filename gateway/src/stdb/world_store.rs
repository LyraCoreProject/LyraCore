//! `Coordinator` is the production [`WorldStore`]. Focused protocol-family supertraits are
//! implemented beside their dispatchers; this file adapts the remaining broad world-session seam.
//!
//! Most methods forward to `Coordinator`'s like-named INHERENT method (defined by concern in
//! `reads`/`reducers`/`subscriptions`/`connection`). Rust method resolution prefers inherent methods
//! over trait methods, so `self.characters(..)` here calls the inherent `Coordinator::characters`, not
//! this trait method — these are thin views, NOT recursion. The inherent methods can't simply *be* this
//! impl: a trait impl must be one contiguous block, but `subscribe_player_events` is a large relay that
//! lives in `subscriptions.rs` and the logon tier calls some inherent reads directly — so the real
//! bodies stay distributed by concern and this block is the unifying world-facing view. The two methods
//! that are NOT 1:1 forwards do real work: `lookup_session` composes two reads, and `movement_update`
//! serializes the `MovementInfo` before the inherent reducer call.

use anyhow::Result;
use wow_world_messages::vanilla::MovementInfo;

use crate::codec;
use crate::world::{SessionTx, WorldSession, WorldStore};

use super::connection::Coordinator;
use super::views::{AccountRow, RealmRow};
use super::PlayerSubscriptions;

impl WorldStore for Coordinator {
    /// Multi-shard routing, driven by the realm-core character→shard index.
    ///
    /// Finds where `character_guid` lives, resolves that location through the shard map, and returns
    /// the owning shard's handle. `None` = this handle already owns it, which is the only answer a
    /// single-entry shard map can give. The original routing answered "where does it live" by reading
    /// `game_character` on the default shard — true only until a transfer actually moves someone off
    /// it. Realm-core answers
    /// it from its index, CONFIRMED against the shard that holds the row, and repairs the
    /// index when the two disagree. The decision itself is the pure, unit-tested
    /// [`crate::config::resolve_home_shard`]; this method only supplies it with live probes.
    // Note: routing is all this does — it does NOT provision the player on the destination.
    // Ceiling: a session that actually routes to a second database opens a fresh player connection
    // there whose node-issued identity was never bound by `establish_session` on THAT database, so
    // `player_login` will be refused by the module's owner check. Realm-core narrowed that gap but
    // did not close it: accounts and sessions ARE now shared, but the per-shard player
    // connection's identity binding still has to be made on the shard the player lands on. Upgrade
    // path: the escrowed transfer moves the character rows, instance entry drives it,
    // and the arriving shard's `establish_session` (or `player_login`'s restamp) does the binding.
    fn home_shard(&self, character_guid: u64) -> Option<std::sync::Arc<dyn WorldStore>> {
        // The single-shard short-circuit, the realm-core index hint, the confirming
        // probe and the self-heal write-back — all of it in `realm_core::settle_shard_index`,
        // written against the `RealmDb` trait so the tests can execute THIS decision (index reads,
        // probe order, heal write and the "one database costs zero reads" property) without a live
        // node. It was unreachable by any test while it lived inline here, and a mutation review
        // left two surviving mutations in it for exactly that reason.
        let resolved = crate::realm_core::settle_shard_index(self, character_guid)?;
        // The handle comes from `shard_for`, unchanged: it resolves the same location
        // through the same map and returns `None` when that is already this handle's shard. All
        // the index lookup adds in front of it is a better answer to "what IS the location". This runs at
        // world ENTRY only: a session already in the world holds its pinned handle and is never
        // asked again (see `WorldConn::route_home`).
        let shard = self.shard_for(resolved.location.0, resolved.location.1);
        debug_assert!(
            resolved.db
                == shard.as_ref().map_or(
                    crate::stdb::Coordinator::shard_name(self),
                    Coordinator::shard_name
                ),
            "resolve_home_shard and shard_for must agree — they resolve the same location"
        );
        Some(std::sync::Arc::new(shard?) as std::sync::Arc<dyn WorldStore>)
    }

    fn shard_name(&self) -> &str {
        // Inherent method (see the module note): this is a view, not recursion.
        self.shard_name()
    }

    // -------------------------------------------------------------------------------------
    // Cross-database transfer — the production side of `world::transfer`.
    // -------------------------------------------------------------------------------------

    /// The transfer-aware upgrade of `home_shard`: find where the character actually LIVES, resolve who should
    /// own it, and run the escrowed transfer when those differ. Single-shard → the `is_sharded`
    /// short-circuit, so the unconfigured gateway does not even read a row.
    ///
    /// **This, not `home_shard`, is the production world-entry resolver.** `world::route_home`
    /// calls `settle_home_shard`, and this impl OVERRIDES it — so any routing rule that is
    /// applied only in `home_shard` is dead code in production while every suite stays green (every
    /// mock takes the trait default, which forwards to `home_shard`). That trap is why the
    /// tripwire at the bottom of this file scans THIS body.
    ///
    /// The HOLDER lookup below is `realm_core::locate_home_shard` — the realm-core
    /// index consulted FIRST, the shard scan only on a miss, self-heal write-back on either path —
    /// not `settle_shard_index`/`home_shard`'s copy of the same rule, which this override still
    /// never calls and which the login hot path therefore still never reaches.
    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        // The shard that currently holds the durable row (or the in-flight escrow). Realm-core's
        // character→shard index is consulted first; the scan (`Coordinator::all_shards`) is the
        // fallback, not the only path.
        let Some(holder) = crate::realm_core::locate_home_shard(self, character_guid) else {
            // Unknown character: leave the session where it is, exactly as `home_shard` does. The
            // module's own ownership check is what refuses the login.
            return Ok(None);
        };
        // The ESCROW's destination outranks the durable row's location: `settle_transfer` RESUMES
        // an existing escrow against the destination the escrow was opened for, so an owner
        // resolved from anything else would hand `run_transfer` a `dst` that is not the database the
        // blob is already filed against — importing the character into a third shard and orphaning
        // the fenced copy. `run_transfer` cannot catch that for itself: a `&dyn WorldStore` cannot be
        // asked which `(map, instance)` it owns, so the obligation is here, at the resolution site.
        // With no escrow (the fresh-transfer path) this is the durable row, exactly as before — the
        // same fields `character_destination` builds the fresh plan from.
        let escrow = holder.escrow_row(character_guid);
        let (map_id, instance_id) = escrow
            .as_ref()
            .map(|r| (r.dest_map_id, r.dest_instance_id))
            .or_else(|| holder.character_location(character_guid))
            .ok_or_else(|| anyhow::anyhow!("character {character_guid} vanished mid-routing"))?;
        // OWNER RESOLUTION. The shard map answers, in DATABASE NAMES resolved through
        // `shard_for`/`instance_shard_for`, so "the answer names the shard I already am" decodes to
        // `None` in both, and `self` is the handle that means "stay put". The NAME never depends on
        // which handle asked (`shard_for` and `world_shards` both read the shared `ShardSet`), so
        // this replaces the older "resolve from the default handle" without reintroducing
        // `realm_shard()`, which realm-core deleted.
        //
        // The instance-pool stickiness applies ONLY when there is no escrow in flight. With an
        // escrow the destination is already fixed by the row (see the paragraph above): being
        // sticky about the holder there would resolve an owner that differs from the database the
        // blob is filed against, which is the third-shard hazard this comment block exists to
        // prevent. Without one, the holder is the only durable evidence of which pool member a
        // live run is on, and it is what stops a pool resize from forking a party.
        let owner = if escrow.is_none() {
            self.instance_shard_for(map_id, instance_id, holder.shard_name())
        } else {
            self.shard_for(map_id, instance_id)
        }
        .unwrap_or_else(|| self.clone());
        // The three facts that decide whether a resume is DRIVEN or silently skipped. Nothing logged
        // them, so every diagnosis of a stranded copy (twice, live) had to infer the holder from what
        // did not happen — and `settle_transfer`'s `holder == owner` branch is silent by design.
        log::info!(
            "settle {character_guid}: holder={} owner={} escrow={} ({map_id}/{instance_id})",
            holder.shard_name(),
            owner.shard_name(),
            escrow.is_some()
        );
        crate::world::transfer::settle_transfer(&holder, &owner, character_guid)?;
        Ok(Some(
            std::sync::Arc::new(owner) as std::sync::Arc<dyn WorldStore>
        ))
    }

    fn escrowed_transfer(
        &self,
        character_guid: u64,
    ) -> Option<crate::world::transfer::EscrowedTransfer> {
        self.escrow_row(character_guid)
            .map(|r| crate::world::transfer::EscrowedTransfer {
                transfer_id: r.transfer_id,
                character_guid: r.character_guid,
                dest_map_id: r.dest_map_id,
                dest_instance_id: r.dest_instance_id,
                blob: r.blob,
            })
    }

    fn character_destination(
        &self,
        character_guid: u64,
    ) -> Option<crate::world::transfer::TransferPlan> {
        let c = self.character_row(character_guid)?;
        Some(crate::world::transfer::TransferPlan {
            transfer_id: crate::world::transfer::transfer_id_for(character_guid),
            character_guid,
            dest_map_id: c.map_id,
            dest_instance_id: c.pending_instance_id,
            dest_x: c.x,
            dest_y: c.y,
            dest_z: c.z,
            dest_o: c.orientation,
        })
    }

    fn begin_transfer(&self, plan: &crate::world::transfer::TransferPlan) -> Result<()> {
        self.begin_transfer(plan)
    }

    fn import_character_blob(&self, transfer_id: u64, blob: &[u8]) -> Result<()> {
        self.import_character_blob(transfer_id, blob)
    }

    fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        self.confirm_import(transfer_id)
    }

    fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        self.finish_transfer(transfer_id)
    }

    fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        self.release_transfer(transfer_id)
    }

    fn ensure_instance(&self, instance_id: u64, map_id: u32, party_id: u64) -> Result<()> {
        self.ensure_instance(instance_id, map_id, party_id)
    }

    fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        self.evict_instance_population(instance_id)
    }

    fn bind_shard_session(&self, account_id: u64, session_key: &[u8; 40]) -> Result<()> {
        // `bound_identity` DERIVES this shard's identity for the account
        // (`synthetic_owner_identity`); `establish_session` then binds it to the shadow account
        // row that `import_character_blob` created. On the realm shard this is the same binding
        // the logon tier already wrote, re-asserted — idempotent, and cheap next to a login.
        let identity = self.bound_identity(account_id)?;
        self.establish_session(account_id, session_key, identity)
    }

    /// The world handshake's account→K lookup, split across the two databases that own the two
    /// halves of the answer. The body is `realm_core::lookup_session`, written against the
    /// `RealmDb` trait so the split it performs — K from realm-core, the account id from THIS world
    /// shard — is executed by a test rather than only described by a comment. Both of those were
    /// surviving mutations in a mutation review.
    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>> {
        crate::realm_core::lookup_session(self, account_name)
    }

    /// Publish a settled transfer's destination into the REALM-CORE character→shard index.
    /// Step 5b of `world::transfer::run_transfer` — see `realm_core::publish_shard_index` for why
    /// this is a replication of `finish_transfer`'s own transactional write and not a best-effort
    /// side call.
    fn publish_shard_index(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<()> {
        crate::realm_core::publish_shard_index(self, character_guid, map_id, instance_id)
    }

    /// The character-select list, UNIONED across every connected shard.
    ///
    /// A character that logged out inside a dungeon lives on the instance shard, not the realm
    /// database — and Phase A has no realm-core character index to ask, so the gateway asks every
    /// shard. The escrow guarantees a character has exactly one durable copy once a transfer has
    /// settled, so the union is the list; the `guid` dedup covers the seconds-long window where an
    /// interrupted transfer left a frozen copy on both sides (first hit wins, and the source is
    /// listed first because `all_shards` puts the default database first).
    ///
    /// Single-shard → the `is_sharded` short-circuit, byte-identical to before.
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>> {
        if !self.is_sharded() {
            return self.characters(account_id);
        }
        let mut out: Vec<codec::CharacterView> = Vec::new();
        for shard in self.all_shards() {
            for c in shard.characters(account_id)? {
                if !out.iter().any(|existing| existing.guid == c.guid) {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    /// Create a character on `self` — the DEFAULT/realm shard — always, even when the race's start
    /// position (`module/src/auth.rs`'s per-race spawn table) routes to a DIFFERENT world shard
    /// under the configured `LYRACORE_SHARD_MAP` (e.g. an Orc/Tauren/Troll/Night Elf's Kalimdor
    /// start position on a two-continent split). The DECISION, made explicit:
    ///
    /// **Create-then-transfer-on-first-login, not create-directly-on-the-owning-shard.** The row
    /// lands on `self` and stays there until the character's very first `CMSG_PLAYER_LOGIN` runs
    /// `route_home`/`settle_home_shard` — the SAME resolve-and-transfer every OTHER
    /// login already goes through, freshly created or not. Creating directly on the owning shard
    /// instead would need its own routing resolved BEFORE the character exists (nothing to look up
    /// in the character→shard index yet), built from scratch for a path that runs exactly once per
    /// character and is not latency-sensitive the way login is.
    ///
    /// This costs nothing NEW: the escrowed transfer it rides is the one already proven against a
    /// full gateway-kill crash matrix
    /// (`a_gateway_kill_at_every_transfer_step_recovers_to_exactly_one_whole_copy`,
    /// `world/tests.rs`), and a fresh character has no live history to lose in transit — if
    /// anything the simplest case that machinery handles. What it did NOT have at first is a
    /// test that the first login of a freshly created character actually drives that transfer
    /// end-to-end rather than merely reusing already-tested machinery by assumption:
    /// `a_freshly_created_characters_first_login_transfers_off_the_default_shard` (`world/tests.rs`).
    fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome> {
        self.create_character(account_id, name, race, class, gender, appearance)
    }

    /// Delete a character wherever it actually LIVES, not only on `self`.
    ///
    /// `characters()` above is a cross-shard UNION: a character resident on ANY connected
    /// shard shows at char-select. `delete_character` did not match that — the reducer
    /// call only ever ran on `self`, so a character resident on a non-default shard was NOT_FOUND
    /// there (surfaced as `Failed`) even though the row was real and deletable.
    /// The resolution is `realm_core::resolve_delete_shard` — the SAME index-first
    /// lookup (`locate_home_shard`) the world-entry path already trusts, not a second routing
    /// mechanism — and delete runs EXACTLY ONCE, on the resolved owner: turning "the wrong shard"
    /// into "every shard" would trade a correctness bug for a data-loss footgun. A character caught
    /// mid-transfer (an in-flight escrow) is refused rather than raced — see
    /// `resolve_delete_shard`'s own doc for why.
    ///
    /// `account_id` is forwarded UNCHANGED to whichever shard `resolve_delete_shard` names — not
    /// re-resolved for that database. This is safe, not merely assumed: `game_character.account_id`
    /// is a per-database `#[auto_inc]` surrogate in general (`realm_core::lookup_session`'s doc
    /// covers that boundary — the REALM-CORE ↔ world-shard one, where ids legitimately differ), but
    /// a character can only ever REACH a non-default world shard through the escrowed transfer
    /// (`create_character` never targets one directly — see its own doc comment), and
    /// `ensure_shadow_account` (`module/src/auth.rs`) creates that shard's shadow `game_account` row
    /// with the EXACT numeric id carried in the import blob, bypassing `#[auto_inc]` — so
    /// `account_id` is preserved bit-for-bit, world-shard to world-shard, across every transfer.
    /// `characters()`'s cross-shard union above and `route_home`'s `bind_shard_session` already
    /// rely on this same invariant (`world/mod.rs`), so this is the established precedent, not a
    /// new assumption introduced here.
    fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<codec::CharDeleteOutcome> {
        match crate::realm_core::resolve_delete_shard(self, character_guid) {
            Ok(Some(owner)) => owner.delete_character(account_id, character_guid),
            Ok(None) => self.delete_character(account_id, character_guid),
            Err(e) => {
                log::warn!("world: {e:#}");
                Ok(codec::CharDeleteOutcome::Failed)
            }
        }
    }

    fn player_login(&self, account_id: u64, character_guid: u64) -> Result<codec::EntityView> {
        self.player_login(account_id, character_guid)
    }

    fn movement_update(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()> {
        // Carry the MovementInfo verbatim so the module can relay it to in-range observers, who
        // re-emit it under the same opcode (animation intent, not just position). The inherent
        // `movement_update` takes the pre-serialized body + a u16 opcode (different arity → visibly
        // not a recursive call).
        let body = codec::movement_info_to_bytes(info)?;
        self.movement_update(
            account_id,
            self_guid,
            opcode as u16,
            &body,
            info.position.x,
            info.position.y,
            info.position.z,
            info.orientation,
            info.timestamp,
        )
    }

    /// The non-blocking submit — this is the override that actually
    /// removes the ceiling; the trait default just forwards to the blocking call.
    fn movement_update_nowait(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
        feedback: &std::sync::Arc<crate::world::MovementFeedback>,
    ) -> Result<()> {
        let body = codec::movement_info_to_bytes(info)?;
        feedback.submitted();
        let r = self.movement_update_nowait(
            account_id,
            self_guid,
            opcode as u16,
            &body,
            info.position.x,
            info.position.y,
            info.position.z,
            info.orientation,
            info.timestamp,
        );
        // Queueing is the only completion observable on a shared batch; individual reducer
        // outcomes are intentionally unavailable to preserve one transaction per batch.
        feedback.completed();
        r
    }

    fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        login_instance: u64,
        login_map: u32,
        login_x: f32,
        login_y: f32,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        self.subscribe_player_events(
            account_id,
            self_guid,
            login_instance,
            login_map,
            login_x,
            login_y,
            tx,
        )
    }

    fn logout(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.logout(account_id, self_guid)
    }

    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>> {
        self.character_by_guid(guid)
    }

    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>> {
        self.creature_template(entry)
    }

    fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>> {
        self.item_template(entry)
    }

    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>> {
        self.gameobject_template(entry)
    }

    fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>> {
        self.gameobject_type(go_guid)
    }

    fn use_gameobject(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()> {
        self.use_gameobject(account_id, self_guid, go_guid)
    }

    fn enter_areatrigger(&self, account_id: u64, self_guid: u64, trigger_id: u32) -> Result<()> {
        self.enter_areatrigger(account_id, self_guid, trigger_id)
    }

    fn client_command(
        &self,
        account_id: u64,
        self_guid: u64,
        cmd: String,
        payload: String,
    ) -> Result<()> {
        self.client_command(account_id, self_guid, cmd, payload)
    }

    fn player_items(&self, owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
        self.player_items(owner_guid)
    }

    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        self.player_skills(character_guid)
    }

    fn effective_armor(&self, guid: u64) -> u32 {
        self.effective_armor(guid)
    }

    fn corpse_loot(&self, corpse_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>> {
        self.corpse_loot(corpse_guid, viewer_guid)
    }

    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool> {
        self.npc_refuses_interaction(npc_guid, player_guid)
    }

    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<codec::MailView>> {
        self.mail_list(recipient_guid)
    }

    fn mailbox_in_range(&self, mailbox_guid: u64, player_guid: u64) -> Result<bool> {
        self.mailbox_in_range(mailbox_guid, player_guid)
    }

    fn mail_mark_read(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_mark_read(recipient_guid, mail_id)
    }

    fn mail_delete(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_delete(recipient_guid, mail_id)
    }
    fn trainer_serves(&self, player_guid: u64, trainer_guid: u64) -> Result<bool> {
        self.trainer_serves(player_guid, trainer_guid)
    }

    fn mail_return(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_return(recipient_guid, mail_id)
    }

    fn mail_send(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item_guid: u64,
    ) -> Result<()> {
        self.mail_send(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            item_guid,
        )
    }

    fn mail_take_money(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_take_money(recipient_guid, mail_id)
    }

    fn mail_take_item(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_take_item(recipient_guid, mail_id)
    }

    fn mail_item_room(&self, payee_guid: u64) -> Result<()> {
        self.mail_item_room(payee_guid)
    }

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
    ) -> Result<()> {
        self.mail_fence(
            escrow_id,
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            postage,
            item_guid,
            cod,
            cod_source_mail_id,
        )
    }

    fn mail_commit(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        item: crate::world::mail::AttachedItem,
        cod: u32,
        cod_source_mail_id: u64,
    ) -> Result<()> {
        self.mail_commit(
            escrow_id,
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            item,
            cod,
            cod_source_mail_id,
        )
    }

    fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()> {
        self.mail_take_money_fence(escrow_id, payee_guid, mail_id, expect_money)
    }

    fn mail_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        amount: u32,
    ) -> Result<()> {
        self.mail_payout(escrow_id, payee_guid, mail_id, amount)
    }

    fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()> {
        self.mail_take_item_fence(escrow_id, payee_guid, mail_id, expect_entry)
    }

    fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        item: crate::world::mail::AttachedItem,
    ) -> Result<()> {
        self.mail_item_payout(escrow_id, payee_guid, mail_id, item)
    }

    fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()> {
        self.mail_confirm_delivery(escrow_id)
    }

    fn mail_settle(&self, escrow_id: u64) -> Result<()> {
        self.mail_settle(escrow_id)
    }

    fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<crate::world::mail::HeldEscrow>> {
        self.mail_escrows_of(sender_guid)
    }

    fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>> {
        self.trainer_list(player_guid, trainer_guid)
    }

    fn buy_trainer_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        self.buy_trainer_spell(account_id, self_guid, trainer_guid, spell_id)
    }

    fn skin_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.skin_corpse(account_id, self_guid, corpse_guid)
    }

    fn item_slot_by_guid(&self, account_id: u64, item_guid: u64) -> Option<u8> {
        self.item_slot_by_guid(account_id, item_guid)
    }

    fn disenchant_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()> {
        self.disenchant_item(account_id, self_guid, slot)
    }

    fn enchant_item_on_slot(
        &self,
        account_id: u64,
        self_guid: u64,
        slot: u8,
        enchant_id: u32,
    ) -> Result<()> {
        self.enchant_item_on_slot(account_id, self_guid, slot, enchant_id)
    }

    fn talent_grant_spell(&self, talent_id: u32) -> u32 {
        self.talent_by_id(talent_id)
            .map(|t| t.grant_spell_id)
            .unwrap_or(0)
    }

    fn spell_is_ground_area(&self, spell_id: u32) -> bool {
        self.spell_is_ground_area(spell_id)
    }

    fn spell_is_fishing(&self, spell_id: u32) -> bool {
        self.spell_is_fishing(spell_id)
    }

    fn fish(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.fish(account_id, self_guid)
    }

    fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        self.spell_is_open_lock(spell_id)
    }

    fn pick_lock(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()> {
        self.pick_lock(account_id, self_guid, go_guid)
    }

    fn set_faction_at_war(
        &self,
        account_id: u64,
        self_guid: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()> {
        self.set_faction_at_war(account_id, self_guid, reputation_index, at_war)
    }

    fn set_action_button(
        &self,
        account_id: u64,
        self_guid: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()> {
        self.set_action_button(account_id, self_guid, button, action, action_type)
    }

    fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32) {
        self.talent_pane_sync(character_guid, talent_id)
    }

    fn talent_points_spent(&self, character_guid: u64) -> u32 {
        self.talent_points_spent(character_guid)
    }

    fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        self.spell_modifiers(character_guid)
    }

    fn learn_talent(&self, account_id: u64, self_guid: u64, talent_id: u32) -> Result<()> {
        self.learn_talent(account_id, self_guid, talent_id)
    }

    fn bind_home(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.bind_home(account_id, self_guid)
    }

    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool> {
        self.npc_is_innkeeper(guid)
    }

    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32 {
        self.npc_gossip_text_id(npc_guid)
    }

    fn npc_text_for_id(&self, text_id: u32) -> Option<codec::NpcTextView> {
        self.npc_text_for_id(text_id)
    }

    fn gossip_options(&self, npc_guid: u64) -> Result<Vec<codec::GossipOptionView>> {
        self.gossip_options(npc_guid)
    }

    fn quest_status(&self, guid: u64, quest_id: u32) -> (bool, bool) {
        self.quest_status(guid, quest_id)
    }

    fn reset_talents(&self, account_id: u64, self_guid: u64, trainer_guid: u64) -> Result<()> {
        self.reset_talents(account_id, self_guid, trainer_guid)
    }

    fn auto_bank_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()> {
        self.auto_bank_item(account_id, self_guid, slot)
    }

    fn buy_bank_slot(&self, account_id: u64, self_guid: u64, banker_guid: u64) -> Result<()> {
        self.buy_bank_slot(account_id, self_guid, banker_guid)
    }

    fn quest_giver_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>> {
        self.quest_giver_evals(giver_guid, player_guid)
    }

    fn quest_detail(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>> {
        self.quest_detail(quest_id)
    }

    fn accept_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()> {
        self.accept_quest(account_id, self_guid, giver_guid, quest_id)
    }

    fn turn_in_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        self.turn_in_quest(account_id, self_guid, giver_guid, quest_id, reward_index)
    }

    fn abandon_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()> {
        self.abandon_quest(account_id, self_guid, quest_id)
    }

    fn push_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()> {
        self.push_quest(account_id, self_guid, quest_id)
    }

    fn player_quest_log(&self, player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>> {
        self.player_quest_log(player_guid)
    }
    fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>> {
        self.player_learned_spells(player_guid)
    }
    fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        self.player_reputations(player_guid)
    }
    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        self.resolve_learn_target(spell_id)
    }

    fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        self.player_actions(player_guid)
    }
    fn entity_in_world(&self, guid: u64) -> bool {
        self.entity_in_world(guid)
    }

    fn set_target(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.set_target(account_id, self_guid, target_guid)
    }

    fn inspect(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.inspect(account_id, self_guid, target_guid)
    }

    fn start_attack(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.start_attack(account_id, self_guid, target_guid)
    }
    fn pet_command(
        &self,
        account_id: u64,
        self_guid: u64,
        data: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.pet_command(account_id, self_guid, data, target_guid)
    }
    fn start_ranged_attack(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        self.start_ranged_attack(account_id, self_guid, target_guid, spell_id)
    }

    fn stop_attack(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.stop_attack(account_id, self_guid)
    }

    fn set_sheathed(&self, account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.set_sheathed(account_id, self_guid, state)
    }

    fn cast_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.cast_spell(account_id, self_guid, spell_id, target_guid)
    }

    fn cast_spell_at(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        self.cast_spell_at(account_id, self_guid, spell_id, target_guid, x, y, z)
    }

    fn cancel_aura(&self, account_id: u64, self_guid: u64, spell_id: u32) -> Result<()> {
        self.cancel_aura(account_id, self_guid, spell_id)
    }

    fn cancel_cast(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.cancel_cast(account_id, self_guid)
    }

    fn spell_cast_time(&self, spell_id: u32) -> Option<u32> {
        self.spell_cast_time(spell_id)
    }

    fn spell_queues_next_swing(&self, spell_id: u32) -> bool {
        self.spell_queues_next_swing(spell_id)
    }

    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        self.spell_is_ranged_auto_repeat(spell_id)
    }

    fn entity_max_health(&self, guid: u64) -> u32 {
        self.entity_max_health(guid)
    }

    fn join_channel(&self, account_id: u64, self_guid: u64, channel: String) -> Result<()> {
        self.join_channel(account_id, self_guid, channel)
    }

    fn leave_channel(&self, account_id: u64, self_guid: u64, channel: String) -> Result<()> {
        self.leave_channel(account_id, self_guid, channel)
    }

    fn send_channel_message(
        &self,
        account_id: u64,
        self_guid: u64,
        channel: String,
        message: String,
    ) -> Result<()> {
        self.send_channel_message(account_id, self_guid, channel, message)
    }

    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32> {
        self.superseded_old_rank(new_spell, player_guid)
    }

    fn enchant_route(&self, spell_id: u32) -> Option<crate::world::EnchantRoute> {
        self.enchant_route(spell_id)
    }

    fn send_chat(
        &self,
        account_id: u64,
        self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()> {
        self.send_chat(account_id, self_guid, chat_type, language, message)
    }

    fn send_emote(
        &self,
        account_id: u64,
        self_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.send_emote(account_id, self_guid, text_emote, emote_anim, target_guid)
    }

    fn send_roll(
        &self,
        account_id: u64,
        self_guid: u64,
        min_roll: u32,
        max_roll: u32,
    ) -> Result<()> {
        self.send_roll(account_id, self_guid, min_roll, max_roll)
    }

    fn send_whisper(
        &self,
        account_id: u64,
        self_guid: u64,
        target_player: String,
        message: String,
    ) -> Result<()> {
        self.send_whisper(account_id, self_guid, target_player, message)
    }

    fn party_chat(&self, account_id: u64, self_guid: u64, message: String) -> Result<()> {
        self.party_chat(account_id, self_guid, message)
    }

    fn gm_command(&self, account_id: u64, self_guid: u64, text: String) -> Result<()> {
        self.gm_command(account_id, self_guid, text)
    }

    fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        self.loot_target_money(target_guid)
    }

    fn loot_money(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.loot_money(account_id, self_guid, target_guid)
    }

    fn take_loot(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
    ) -> Result<()> {
        self.take_loot(account_id, self_guid, corpse_guid, loot_slot)
    }

    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.repop(account_id, self_guid)
    }

    fn claim_session(&self, account_id: u64) -> u64 {
        self.claim_session(account_id)
    }

    fn release_session(&self, account_id: u64, epoch: u64) -> bool {
        self.release_session(account_id, epoch)
    }

    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.reclaim_corpse(account_id, self_guid, corpse_guid)
    }

    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        self.resurrect_response(account_id, self_guid, accept)
    }

    fn spirit_healer_res(&self, account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()> {
        self.spirit_healer_res(account_id, self_guid, healer_guid)
    }

    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        self.corpse_location(owner_guid)
    }

    fn player_combat_until_ms(&self, player_guid: u64) -> u64 {
        self.player_combat_until_ms(player_guid)
    }

    fn online_players(&self) -> Result<Vec<codec::WhoPlayerView>> {
        self.online_players()
    }

    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<codec::FriendView>, Vec<u64>)> {
        self.contact_lists(self_guid)
    }

    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>> {
        self.character_guid_by_name(name)
    }

    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        self.character_presence(guid)
    }

    // The SINGLE-DATABASE party path: unchanged reducer calls on the player's own connection.
    // `self_guid` is unused here on purpose — the module resolves the actor from `ctx.sender()`'s
    // live entity, which is the whole reason this arm needs no operator gate. `world::party` is what
    // chooses between this and the realm-core arm below.
    fn group_invite(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.group_invite(account_id, self_guid, target_guid)
    }
    fn initiate_trade(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.initiate_trade(account_id, self_guid, target_guid)
    }
    fn begin_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.begin_trade(account_id, self_guid)
    }
    fn cancel_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.cancel_trade(account_id, self_guid)
    }
    fn set_trade_item(
        &self,
        account_id: u64,
        self_guid: u64,
        trade_slot: u8,
        inv_slot: u8,
    ) -> Result<()> {
        self.set_trade_item(account_id, self_guid, trade_slot, inv_slot)
    }
    fn clear_trade_item(&self, account_id: u64, self_guid: u64, trade_slot: u8) -> Result<()> {
        self.clear_trade_item(account_id, self_guid, trade_slot)
    }
    fn set_trade_gold(&self, account_id: u64, self_guid: u64, copper: u32) -> Result<()> {
        self.set_trade_gold(account_id, self_guid, copper)
    }
    fn accept_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.accept_trade(account_id, self_guid)
    }
    fn unaccept_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.unaccept_trade(account_id, self_guid)
    }
    fn busy_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.busy_trade(account_id, self_guid)
    }
    fn ignore_trade(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.ignore_trade(account_id, self_guid)
    }
    fn group_accept(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.group_accept(account_id, self_guid)
    }
    fn group_decline(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.group_decline(account_id, self_guid)
    }
    fn group_leave(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.group_leave(account_id, self_guid)
    }
    fn group_uninvite(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.group_uninvite(account_id, self_guid, target_guid)
    }
    fn group_loot_method(
        &self,
        account_id: u64,
        self_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()> {
        self.group_loot_method(
            account_id,
            self_guid,
            loot_setting,
            master_guid,
            loot_threshold,
        )
    }

    // --- Realm-wide party state (the realm-core group slice) ---

    /// The realm-core handle, but ONLY on a multi-database gateway.
    ///
    /// The `is_sharded()` short-circuit is the whole of "unset env vars ⇒ today's gateway" for the
    /// realm-wide party plane: with one database `realm_core()` would answer THIS handle, and routing
    /// party ops through the operator-gated realm reducers against the same database would be a
    /// different code path, a different gate, and a different set of writes for no gain. Answering
    /// `None` here sends `world::party` down the arm that calls exactly the reducers it called
    /// before realm-core existed.
    ///
    /// Configured-but-unreachable realm-core answers `None` as well (`realm_core()` is `Err`), so a
    /// realm-core outage degrades party ops to shard-local rather than failing them. That is a
    /// deliberate split from the AUTH paths, which fail CLOSED: a stale auth cache lets someone in
    /// with a revoked credential, while a shard-local party op is only ever a smaller party.
    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>> {
        if !self.is_sharded() {
            return None;
        }
        self.realm_core()
            .ok()
            .map(|rc| std::sync::Arc::new(rc) as std::sync::Arc<dyn WorldStore>)
    }

    /// Every connected WORLD shard (realm-core excluded by `ShardMap::shards`, as always) — the
    /// mirror fan-out set. Empty when unsharded, so the push costs a single-database gateway nothing.
    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>> {
        if !self.is_sharded() {
            return Vec::new();
        }
        self.all_shards()
            .into_iter()
            .map(|c| std::sync::Arc::new(c) as std::sync::Arc<dyn WorldStore>)
            .collect()
    }

    fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
    ) -> Result<()> {
        self.realm_group_op(op, actor_guid, target_guid, arg_a, arg_b)
    }

    fn group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster(character_guid))
    }

    fn group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster_by_id(group_id))
    }

    fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        self.sync_group_mirror(roster)
    }

    fn realm_whisper(
        &self,
        sender_guid: u64,
        target_guid: u64,
        message: String,
        sender_is_ignored: bool,
    ) -> Result<()> {
        self.realm_whisper(sender_guid, target_guid, message, sender_is_ignored)
    }
    fn loot_roll(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<()> {
        self.loot_roll(account_id, self_guid, corpse_guid, loot_slot, vote)
    }

    // --- Realm-wide loot rolls ---

    fn realm_loot_op(
        &self,
        op: u8,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        actor_guid: u64,
        vote: u8,
        deadline_micros: i64,
        recipients: Vec<u64>,
    ) -> Result<()> {
        self.realm_loot_op(
            op,
            corpse_guid,
            slot,
            item_entry,
            actor_guid,
            vote,
            deadline_micros,
            recipients,
        )
    }

    fn pending_local_rolls(&self) -> Result<Vec<crate::world::loot::PendingLootRoll>> {
        self.pending_local_rolls()
    }

    fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        self.settle_loot_roll(corpse_guid, slot, winner_guid)
    }

    fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        self.clear_promoted_loot_roll(roll_id)
    }

    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        self.loot_won_since(after_id)
    }
    fn loot_master_give(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<()> {
        self.loot_master_give(account_id, self_guid, corpse_guid, loot_slot, target_guid)
    }
    fn gossip_select(
        &self,
        account_id: u64,
        self_guid: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        self.gossip_select(account_id, self_guid, npc_guid, option_id, option_row_id)
    }
    fn add_friend(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.add_friend(account_id, self_guid, target_guid)
    }

    fn del_friend(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.del_friend(account_id, self_guid, target_guid)
    }

    fn add_ignore(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.add_ignore(account_id, self_guid, target_guid)
    }

    fn del_ignore(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()> {
        self.del_ignore(account_id, self_guid, target_guid)
    }
}

/// The realm-core split's store seam. Every method here is a view onto
/// `Coordinator`'s like-named INHERENT method — Rust resolves inherent methods first, so these are
/// forwards, not recursion, exactly like the `WorldStore` impl above.
///
/// This block is the ONE layer `realm_core.rs`'s fake substitutes for wholesale, so it is pinned by
/// exact-shape equality in `realm_core::tests::the_coordinator_forwards_are_views_not_logic`. Keep
/// it a block of forwards; any logic that grows here is untested by construction. `has_escrow` is
/// the one method that narrows its inherent counterpart (`escrow_row`'s `Option<TransferOut>`) to a
/// bool rather than forwarding it bare — see that test's doc for why this one is still safe.
impl crate::realm_core::RealmDb for Coordinator {
    fn shard_name(&self) -> &str {
        self.shard_name()
    }
    fn is_sharded(&self) -> bool {
        self.is_sharded()
    }
    fn shard_map(&self) -> &crate::config::ShardMap {
        self.shard_map()
    }
    fn realm_core(&self) -> Result<Coordinator> {
        self.realm_core()
    }
    fn world_shards(&self) -> Vec<(String, Coordinator)> {
        self.world_shards()
    }
    fn account_by_username(&self, username: &str) -> Result<Option<AccountRow>> {
        self.account_by_username(username)
    }
    fn session_key(&self, account_id: u64) -> Result<Option<[u8; 40]>> {
        self.session_key(account_id)
    }
    fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> {
        self.bound_identity(account_id)
    }
    fn character_count(&self, account_id: u64) -> Result<u8> {
        self.character_count(account_id)
    }
    fn realm(&self) -> Result<RealmRow> {
        self.realm()
    }
    fn establish_session(
        &self,
        account_id: u64,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()> {
        self.establish_session(account_id, session_key, bound_identity)
    }
    fn character_location(&self, guid: u64) -> Option<(u32, u64)> {
        self.character_location(guid)
    }
    fn character_shard(&self, guid: u64) -> Option<(u32, u64)> {
        self.character_shard(guid)
    }
    fn set_character_shard(&self, guid: u64, map_id: u32, instance_id: u64) -> Result<()> {
        self.set_character_shard(guid, map_id, instance_id)
    }
    fn has_escrow(&self, guid: u64) -> bool {
        self.escrow_row(guid).is_some()
    }
    fn session_count(&self) -> usize {
        self.session_count()
    }
    fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
        gateway_key: u64,
    ) -> Result<()> {
        self.record_shard_load(shard, writer_occupancy_pct, sessions, gateway_key)
    }
}

#[cfg(test)]
mod routing_call_site_tests {
    /// One method's body in this file, comments stripped. The routing decisions in this impl run
    /// against a live SpacetimeDB node and NOTHING ELSE, so no fake can reach
    /// them: the pure resolvers in `crate::config` are pinned by their own tests, and the trait-level
    /// behavior is pinned by the fakes in `world::tests` — but *whether the Coordinator's own
    /// override still calls them* is exactly the mutation that stays green in both. A source scan is
    /// what the module tier already does for its un-runnable call sites (`transfer.rs`'s fence
    /// tripwires), so the same instrument is used here. Comments are stripped because a scanner that
    /// matches prose pins prose (the `code_of` lesson) — the doc comment above each call site names
    /// every symbol asserted below.
    fn code_of(method: &str) -> String {
        let src = include_str!("world_store.rs");
        let needle = format!("fn {method}(");
        // The scan takes the FIRST match, so a second definition of the same name anywhere earlier
        // in this file — an inherent `impl Coordinator` block carrying the blessed text, say —
        // silently becomes the thing every assertion below is measured against while the real
        // override is free to be anything. That defeat was demonstrated during this PR's
        // adversarial review: an eight-line decoy above `impl WorldStore for Coordinator` left
        // `publish_shard_index` a bare `Ok(())` in production with all 403 tests green. A source
        // scan is only worth having if it cannot be aimed somewhere else, so: exactly one.
        let hits = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && l.contains(needle.as_str()))
            .count();
        assert_eq!(
            hits, 1,
            "`{needle}` appears {hits} times in world_store.rs — a source scan that takes the first \
             match cannot tell which one production calls. Either there is a decoy definition \
             shadowing the scanned call site, or a legitimate second one that this tripwire must be \
             re-aimed at explicitly."
        );
        let body = src
            .split_once(needle.as_str())
            .unwrap_or_else(|| panic!("`impl WorldStore for Coordinator` still defines {method}"))
            .1;
        let body = body.split_once("\n    fn ").map_or(body, |(b, _)| b);
        body.lines()
            .map(strip_line_comment)
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Strip a Rust line comment from `line`, respecting simple double-quoted string literals so a
    /// `//` inside one is never mistaken for a comment start. Also closes the trailing-comment
    /// defeat `code_of` above used to be open to (`.filter(|l|
    /// !l.trim_start().starts_with("//"))` only drops a comment that is the FIRST thing on its
    /// line, so a needle surviving in a trailing comment on an otherwise-live line still satisfied
    /// a `.contains()` scan). Module-crate copy: `module/src/test_scan.rs`, which this cannot import
    /// across the crate boundary — kept in sync by hand, and both are exercised by their own tests.
    fn strip_line_comment(line: &str) -> &str {
        let bytes = line.as_bytes();
        let mut in_string = false;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_string => i += 1,
                b'"' => in_string = !in_string,
                b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
                _ => {}
            }
            i += 1;
        }
        line
    }

    /// `Coordinator::realm_core` — the AUTH ROUTER, and the one edit that reverts the whole
    /// realm-core auth split.
    ///
    /// Found surviving by an adversarial review. `impl RealmDb for Coordinator` is
    /// pinned below, and every `realm_core.rs` body is executed against `fake::Handle` — but both
    /// stop at the forward. The inherent method the forward calls is one level lower, and replacing
    /// its body with `Ok(self.clone())` left ALL 403 gateway tests green while:
    ///
    /// * the SRP6 challenge is answered from the world shard's write-through `game_account` cache,
    ///   so a password rotation or a ban applied on realm-core stops being enforced (M1's live
    ///   consequence, reached one call below where M1 was pinned);
    /// * the world handshake reads K from the world shard's session cache (M5, ditto);
    /// * `establish_session` writes only the world shard, so the realm-wide session row — the whole
    ///   point of a shared session table — is never written;
    /// * fail-closed is gone: a dead realm-core no longer refuses anything, it is simply not asked.
    ///
    /// That is the exact regression the realm-core split was built to prevent, so it does not get
    /// to be the one line nothing watches. It cannot be executed (it reads
    /// a live websocket's liveness), so it gets the same instrument as the forwards: exact shape.
    #[test]
    fn the_auth_database_router_is_still_a_router_and_not_a_passthrough() {
        let src = include_str!("connection.rs");
        let needle = "pub(crate) fn realm_core(&self)";
        let hits = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && l.contains(needle))
            .count();
        assert_eq!(
            hits, 1,
            "`{needle}` is defined {hits} times in connection.rs — a first-match \
                   source scan cannot tell which one the auth paths call"
        );
        let body = &src[src.find(needle).expect("checked above")..];
        let end = body
            .find("\n    }\n")
            .expect("unterminated `realm_core` body");
        let shape = body[..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let want = "pub(crate) fn realm_core(&self) -> Result<Coordinator> { \
             let db = self .1 .map .auth_db(|d| { \
             self.1.conns.get(d).is_some_and(|inner| { \
             let live = inner.coord(); \
             live.conn.is_active() && live._sub.is_active() }) }) \
             .map_err(|db| { anyhow!( \
             \"realm-core database {db} is not connected — refusing to authenticate against \\ \
             the world database's stale auth cache\" ) })?; \
             let inner = self .1 .conns .get(db) \
             .ok_or_else(|| anyhow!(\"auth database {db} missing from the coordinator set\"))?; \
             Ok(Coordinator(inner.clone(), self.1.clone()))"
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            shape, want,
            "`Coordinator::realm_core` is no longer the auth ROUTER it is documented to be. It must \
             resolve the database through `ShardMap::auth_db` with a LIVE connection predicate and \
             hand back a handle on THAT database — never `self`, and never an `Ok` on a realm-core \
             that is configured but not connected. Everything that authenticates goes through this \
             one function and nothing else can observe it; a deliberate change must be re-blessed \
             here with the same care."
        );
    }

    /// The two `WorldStore` methods whose whole body is a delegation into `crate::realm_core` —
    /// pinned by EXACT SHAPE, the same instrument `module/src/transfer/mod.rs` uses for `CtxShard`.
    ///
    /// This is the seam's own blind spot, and it was measured, not assumed. `realm_core.rs`'s tests
    /// substitute `fake::Handle` for `Coordinator`, and `world::tests`' mocks substitute
    /// `InMemoryStore` for it — so BOTH suites are blind to this adapter, and two mutations proved
    /// it by staying green across all 402 tests:
    ///
    /// | edit | live consequence |
    /// |---|---|
    /// | `publish_shard_index` → `Ok(())` | every completed transfer stops telling realm-core where the character went — the shard index silently stops being written |
    /// | `lookup_session` → read the world DB directly | the world handshake authenticates off the world shard's session cache: realm-core's entire point, reverted |
    ///
    /// Equality rather than `contains`, because a `contains` scan is defeated by leaving its own
    /// text in a dead branch — which this repo has had done to it repeatedly. A deliberate change
    /// here must be re-blessed with the same care.
    #[test]
    fn the_realm_core_delegations_are_exactly_delegations() {
        for (method, want) in [
            (
                "lookup_session",
                "&self, account_name: &str) -> Result<Option<WorldSession>> { \
                 crate::realm_core::lookup_session(self, account_name) }",
            ),
            (
                "publish_shard_index",
                "&self, character_guid: u64, map_id: u32, instance_id: u64, ) -> Result<()> { \
                 crate::realm_core::publish_shard_index(self, character_guid, map_id, instance_id) }",
            ),
        ] {
            let got = code_of(method).split_whitespace().collect::<Vec<_>>().join(" ");
            let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(
                got.trim_end_matches('}').trim_end(),
                want.trim_end_matches('}').trim_end(),
                "`impl WorldStore for Coordinator::{method}` is no longer the bare delegation into \
                 `crate::realm_core` that both test suites assume it is. `realm_core.rs`'s tests run \
                 the shared body with a FAKE substituted for this handle and `world::tests`' mocks \
                 override this method entirely, so nothing either suite asserts covers an edit here \
                 — and each of these is one line whose damage is total and silent."
            );
        }
    }

    /// `delete_character`'s routing composition. `realm_core::resolve_delete_shard` is tested
    /// BEHAVIOURALLY (`realm_core.rs`, via `fake::Handle`) for the resolution and refusal decisions
    /// themselves, but — like `settle_home_shard` above — nothing can exercise *whether Coordinator's
    /// override actually dispatches through that answer* without a live node: `world::tests`' mocks
    /// implement `WorldStore` directly and never take this override. Pinned by EXACT SHAPE rather
    /// than `the_realm_core_delegations_are_exactly_delegations`'s bare-forward instrument just
    /// above, because this body is three branches, not one line — a `contains` scan of any single
    /// branch is defeated by a decoy left in a dead one (demonstrated live in this repo), and
    /// equality is what actually pins the composition: that delete runs on the RESOLVED owner (not
    /// unconditionally on `self`, which was the bug), and that a refusal (`Err`) reports
    /// `Failed` rather than falling through to delete anyway (which would race a resumed transfer).
    #[test]
    fn delete_character_dispatches_through_the_resolved_shard_not_always_self() {
        let got = code_of("delete_character")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let want =
            "&self, account_id: u64, character_guid: u64, ) -> Result<codec::CharDeleteOutcome> { \
             match crate::realm_core::resolve_delete_shard(self, character_guid) { \
             Ok(Some(owner)) => owner.delete_character(account_id, character_guid), \
             Ok(None) => self.delete_character(account_id, character_guid), \
             Err(e) => { \
             log::warn!(\"world: {e:#}\"); \
             Ok(codec::CharDeleteOutcome::Failed) \
             } \
             } \
             }"
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            got.trim_end_matches('}').trim_end(),
            want.trim_end_matches('}').trim_end(),
            "`impl WorldStore for Coordinator::delete_character` no longer resolves the owning \
             shard through `realm_core::resolve_delete_shard` before deleting. Either a character \
             resident on a non-default shard is undeletable again, or — the \
             opposite failure — delete stopped running on exactly ONE shard, which is the \
             data-loss footgun this design explicitly refuses to introduce."
        );
    }

    /// THE tripwire for the live world-entry path. `world::route_home` calls `settle_home_shard`,
    /// and this file overrides it — so a rule applied in `home_shard` alone is production-dead
    /// while both suites stay green (the mocks take the trait default, which forwards to
    /// `home_shard`). That is the merge-defect class this test exists to make impossible to
    /// reintroduce.
    #[test]
    fn settle_home_shard_keeps_the_short_circuit_and_the_escrow_first_destination() {
        let code = code_of("settle_home_shard");
        // It must short-circuit before anything else on a single-database gateway,
        // which is what "the env vars unset ⇒ today's gateway" means on the login hot path.
        // Pin the NEGATED form, not the bare substring: `code.contains("is_sharded()")`
        // still matched after the guard was inverted to `if self.is_sharded()`, which is the exact
        // source-scan defeat class this repo keeps hitting (a scan that a mutation walks straight
        // through). This body is unreachable from the suite — `Coordinator` cannot be constructed
        // offline, so every mock takes the trait default — so this scan is the only guard there is,
        // and it has to be precise enough to fail when the guard flips.
        assert!(
            code.contains("if !self.is_sharded() {"),
            "settle_home_shard lost the single-shard short-circuit (or inverted it): an \
             unconfigured gateway would pay the shard scan and the escrow read on every world entry",
        );
        // The owner handed to `run_transfer` must be resolved from the ESCROW's stated
        // destination, not from whatever the durable row says, or a resumed transfer imports the
        // blob into a database it was never filed against. `run_transfer` cannot check this itself.
        let escrow_at = code
            .find("escrow_row(character_guid)")
            .unwrap_or(usize::MAX);
        let location_at = code
            .find("character_location(character_guid)")
            .unwrap_or(usize::MAX);
        assert!(
            escrow_at < location_at,
            "settle_home_shard must resolve the destination from the escrow row FIRST and fall back \
             to `character_location` — the other order hands `run_transfer` a `dst` that can differ \
             from the destination the escrow was opened for"
        );
    }

    /// The same tripwire, for the instance-pool stickiness. `ShardMap::instance_owner` is pinned
    /// by its own unit tests, but *whether the production world-entry resolver still calls it* is
    /// unreachable from any fake: `world::tests`' mocks implement the TRAIT, and this rule lives in
    /// the `Coordinator` impl. Deleting the call site restores the pre-stickiness routing — a live dungeon
    /// run forks onto a newly added pool member on the next world entry — with every suite green.
    #[test]
    fn settle_home_shard_still_applies_the_instance_pool_stickiness() {
        let code = code_of("settle_home_shard");
        // The ARGUMENTS, not just the name. Adversarial review: `contains("instance_shard_for(")`
        // alone is defeated by every alias substitution that keeps the call — passing
        // `self.shard_name()` instead of `holder.shard_name()` (the session's handle, which on a
        // login is the DEFAULT shard and never a pool member, so stickiness can never fire) left
        // all 391 gateway tests green, as did passing a literal `0` for `instance_id`. Both delete
        // the feature outright while this scan stays satisfied.
        assert!(
            code.contains("instance_shard_for(map_id, instance_id, holder.shard_name())"),
            "settle_home_shard no longer resolves an instance's owner through \
             `instance_shard_for(map_id, instance_id, holder.shard_name())` (pure half: \
             `config::ShardMap::instance_owner`). The HOLDER is the whole rule — it is the only \
             durable evidence of which pool member a live run is on, and `self` is the session's \
             handle, not the character's. Adding a second instances database would then re-point \
             the buckets of LIVE runs, and each of those parties would be transferred into a \
             freshly mirrored, empty copy of their own dungeon."
        );
        // …and only on the no-escrow path. An in-flight escrow's destination is fixed by the row;
        // being sticky there hands `run_transfer` a `dst` the blob was never filed against.
        assert!(
            code.contains("if escrow.is_none() {"),
            "the stickiness arm lost its escrow guard — a resumed transfer would resolve an owner \
             that differs from the destination its escrow was opened for, orphaning the fenced copy"
        );
    }

    #[test]
    fn home_shard_still_resolves_through_the_settle_shard_index() {
        let code = code_of("home_shard");
        // The single-shard short-circuit, the index hint, the probe and the self-heal
        // live in `realm_core::settle_shard_index` — where they are BEHAVIOURALLY tested
        // (`a_single_database_gateway_resolves_the_home_shard_without_reading_anything`,
        // `a_deliberately_stale_index_entry_still_routes_correctly_and_is_healed_in_place`), which
        // they never were while they sat inline here. What this scan still has to pin is that
        // `home_shard` calls it at all: routing to `resolved` while nothing computed `resolved`
        // from the index would drop the whole realm-core routing with every suite green.
        assert!(
            code.contains("settle_shard_index(self, character_guid)"),
            "home_shard no longer resolves through `realm_core::settle_shard_index` — the realm-core \
             index hint, the confirming probe, the self-heal write-back and the single-database \
             short-circuit are all in it, so dropping the call drops realm-core routing entirely (and makes an \
             unconfigured gateway pay for routing it does not do)"
        );
    }

    /// The ONE line that decides whether a party op runs on realm-core or on the
    /// player's own shard, and the only one no fake can reach — `world::party`'s tests drive the
    /// trait, and this is the `Coordinator` impl that answers it in production.
    ///
    /// Both directions are damage. Dropping the `is_sharded()` short-circuit routes a
    /// SINGLE-DATABASE gateway's party ops through the operator-gated realm reducers against its own
    /// database: a different reducer, a different gate, and a mirror push per op, in a configuration
    /// the realm-core slice promises costs nothing. Dropping `realm_core()` — answering `self` — silently reinstates
    /// the shard-local party plane this whole slice exists to remove, with every suite green because
    /// every mock supplies its own realm handle.
    #[test]
    fn realm_store_is_the_realm_core_handle_and_only_on_a_multi_database_gateway() {
        let code = code_of("realm_store");
        assert!(
            code.contains("if !self.is_sharded() {") && code.contains("return None;"),
            "realm_store lost its single-database short-circuit: an unconfigured gateway would route \
             every party op through the operator-gated `realm_group_op` and mirror it back onto the \
             one database it already wrote. The byte-identical promise is exactly this line."
        );
        assert!(
            code.contains("self.realm_core()"),
            "realm_store no longer resolves the REALM-CORE handle. Answering any other database \
             puts party membership back on a world shard — a cross-shard invite stops resolving and \
             a split party stops seeing itself, which is the live defect this slice fixes."
        );
    }

    /// The mirror fan-out set. `world_stores()` naming only SOME shards (or none) leaves the others
    /// running a party's gameplay — kill-XP split, quest credit, loot rules, the dungeon binding —
    /// against a roster that no longer exists. Realm-core must not be in it: it owns no gameplay
    /// reads, and `ShardMap::shards()` is what excludes it by construction.
    #[test]
    fn world_stores_fans_out_to_every_world_shard_and_never_to_realm_core() {
        let code = code_of("world_stores");
        assert!(
            code.contains("self.all_shards()"),
            "world_stores no longer enumerates the connected world shards through `all_shards()` \
             (which excludes realm-core by construction). A narrower set silently stops mirroring \
             realm-core's roster onto the shards it left out."
        );
        assert!(
            code.contains("if !self.is_sharded() {"),
            "world_stores lost its single-database short-circuit — the mirror push must cost an \
             unconfigured gateway nothing"
        );
    }
}
