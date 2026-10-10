//! `Coordinator` is the production [`WorldStore`]. Focused protocol-family supertraits are
//! implemented beside their dispatchers; this file adapts the remaining broad world-session seam.
//!
//! Most methods forward to `Coordinator`'s like-named INHERENT method (defined by concern in
//! `reads`/`reducers`/`subscriptions`/`connection`). Rust method resolution prefers inherent methods
//! over trait methods, so `self.characters(..)` here calls the inherent `Coordinator::characters`, not
//! this trait method — these are thin views, NOT recursion. The inherent methods can't simply *be* this
//! impl: a trait impl must be one contiguous block, but `subscribe_player_events` builds viewer state
//! in `subscriptions.rs` and the logon tier calls some inherent reads directly — so the real
//! bodies stay distributed by concern and this block is the unifying world-facing view. The two methods
//! that are NOT 1:1 forwards do real work: `lookup_session` composes two reads, and `movement_update`
//! serializes the `MovementInfo` before enqueueing it on the shard's shared batch.

use anyhow::{anyhow, Result};
use wow_world_messages::vanilla::MovementInfo;

use crate::codec;
use crate::realm_core::SessionKey;
use crate::world::{SessionTx, WorldSession, WorldStore, MOVE_SUBMITTED};

use super::bindings::game_instance_table::GameInstanceTableAccess;
use super::bindings::game_transfer_in_table::GameTransferInTableAccess;
use super::bindings::game_world_entity_table::GameWorldEntityTableAccess;
use super::bindings::GwMove;
use super::connection::{CharacterPresenceSnapshot, Coordinator};
use super::views::{AccountRow, RealmRow};
use super::PlayerSubscriptions;

fn stable_character_absence(
    first: &[CharacterPresenceSnapshot],
    second: &[CharacterPresenceSnapshot],
) -> bool {
    first.len() == second.len()
        && first.iter().zip(second).all(|(before, after)| {
            !before.present
                && !after.present
                && before.generation == after.generation
                && before.revision == after.revision
        })
}

fn durable_character_presence_snapshots(
    shards: &[(String, Coordinator)],
    guid: u64,
) -> Result<Vec<CharacterPresenceSnapshot>> {
    shards
        .iter()
        .map(|(name, shard)| {
            shard
                .0
                .coord()
                .durable_character_presence_snapshot(guid, name)
        })
        .collect()
}

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
    /// mock takes the trait default, which forwards to `home_shard`).
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

    /// `instance_shard_for`, not `shard_for`: this handle is the character's HOLDER, and the
    /// stickiness rule that keeps a live dungeon run on its pool member is written against exactly
    /// that pair. `settle_home_shard` above resolves a fresh (escrow-free) hop the same way.
    fn shard_for_location(
        &self,
        map_id: u32,
        instance_id: u64,
    ) -> Option<std::sync::Arc<dyn WorldStore>> {
        let shard = self.instance_shard_for(map_id, instance_id, Coordinator::shard_name(self))?;
        Some(std::sync::Arc::new(shard) as std::sync::Arc<dyn WorldStore>)
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

    fn import_character_blob(
        &self,
        transfer_id: u64,
        blob: &[u8],
        source: crate::world::transfer::RealmLocatorPredecessor,
        bot_arrival: Option<&crate::world::transfer::BotTransferIntent>,
    ) -> Result<()> {
        match bot_arrival {
            Some(intent) => {
                if (source.map_id, source.instance_id, source.revision)
                    != (
                        intent.source_map,
                        intent.source_instance,
                        intent.source_locator_revision,
                    )
                {
                    anyhow::bail!("bot Transfer Realm locator binding changed before import");
                }
                self.import_bot_character_blob(transfer_id, blob, intent)
            }
            None => self.import_player_character_blob(transfer_id, blob, source),
        }
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

    fn release_player_transfer_arrival(
        &self,
        transfer_id: u64,
        character_guid: u64,
        source: crate::world::transfer::RealmLocatorPredecessor,
    ) -> Result<()> {
        Coordinator::release_player_transfer_arrival(self, transfer_id, character_guid, source)
    }

    fn transfer_arrival(
        &self,
        transfer_id: u64,
    ) -> Option<crate::world::transfer::TransferArrival> {
        self.0
            .coord()
            .conn
            .db
            .game_transfer_in()
            .transfer_id()
            .find(&transfer_id)
            .map(|row| crate::world::transfer::TransferArrival {
                character_guid: row.character_guid,
                source_map: row.source_map_id,
                source_instance: row.source_instance_id,
                source_locator_revision: row.source_locator_revision,
                bot_source_identity: row.bot_intent_source,
                bot_transfer_intent_id: row.bot_intent_id,
                bot_controller_generation: row.bot_controller_generation,
            })
    }

    fn instance_partition(&self, instance_id: u64) -> Option<(u32, u64)> {
        self.0
            .coord()
            .conn
            .db
            .game_instance()
            .instance_id()
            .find(&instance_id)
            .map(|row| (row.map_id, row.party_id))
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

    fn begin_shard_index_transfer(
        &self,
        plan: &crate::world::transfer::TransferPlan,
        bot_intent: Option<(&crate::world::transfer::BotTransferIntent, u64)>,
    ) -> Result<crate::world::party::RealmCharacterPartition> {
        crate::realm_core::begin_shard_index_transfer(self, plan, bot_intent)
    }

    fn finish_player_shard_index_transfer(
        &self,
        plan: &crate::world::transfer::TransferPlan,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
    ) -> Result<()> {
        crate::realm_core::finish_player_shard_index_transfer(
            self,
            plan,
            source_map,
            source_instance,
            source_revision,
        )
    }

    fn finish_pending_shard_index_transfer(
        &self,
        character_guid: u64,
        destination_map: u32,
        destination_instance: u64,
        arrival: &crate::world::transfer::TransferArrival,
    ) -> Result<()> {
        crate::realm_core::finish_pending_shard_index_transfer(
            self,
            character_guid,
            destination_map,
            destination_instance,
            arrival,
        )
    }

    fn bind_bot_transfer_locator(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
        source_revision: u64,
        claim_token: u64,
    ) -> Result<()> {
        self.bind_bot_transfer_locator(intent, source_revision, claim_token)
    }

    fn publish_bot_shard_index(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        crate::realm_core::publish_bot_shard_index(self, intent)
    }

    /// Union the account's Characters across connected World Shards. Deduplicate by guid
    /// during interrupted Transfers, keeping the first copy in default-first probe order.
    /// Guild membership comes from Realm-core; an unavailable authority leaves guild names empty.
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>> {
        let mut out: Vec<codec::CharacterView> = if self.is_sharded() {
            let mut out: Vec<codec::CharacterView> = Vec::new();
            for shard in self.all_shards() {
                for c in shard.characters(account_id)? {
                    if !out.iter().any(|existing| existing.guid == c.guid) {
                        out.push(c);
                    }
                }
            }
            out
        } else {
            self.characters(account_id)?
        };
        match self.realm_core() {
            Ok(realm) => {
                for c in &mut out {
                    c.guild_id = realm.guild_projection(c.guid).0;
                }
            }
            Err(error) => log::warn!("world: character list without guild ids: {error:#}"),
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

    fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
        entry: codec::WorldEntry,
    ) -> Result<codec::EntityView> {
        self.player_login(account_id, character_guid, entry)
    }

    fn movement_update(
        &self,
        _account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()> {
        if self_guid == 0 {
            return Err(anyhow!("movement_update: actor_guid unresolved"));
        }
        // Carry the MovementInfo verbatim so the module can relay it to in-range observers under
        // the same opcode. Queueing is the only completion observable on the shared batch;
        // individual reducer outcomes are intentionally unavailable.
        let body = codec::movement_info_to_bytes(info)?;
        self.0.motion_batch.push(GwMove {
            actor: self.session_actor(self_guid),
            opcode: opcode as u16,
            movement_info: body,
            x: info.position.x,
            y: info.position.y,
            z: info.position.z,
            o: info.orientation,
            move_time_ms: info.timestamp,
        });
        MOVE_SUBMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        arrival: &crate::codec::EntityView,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        self.subscribe_player_events(account_id, self_guid, arrival, tx)
    }

    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>> {
        self.character_by_guid(guid)
    }

    fn character_exists_on_any_world_shard(&self, guid: u64) -> Result<bool> {
        let shards = self.world_shards_for_absence()?;
        let first = durable_character_presence_snapshots(&shards, guid)?;
        if first.iter().any(|snapshot| snapshot.present) {
            return Ok(true);
        }
        let second = durable_character_presence_snapshots(&shards, guid)?;
        if second.iter().any(|snapshot| snapshot.present) {
            return Ok(true);
        }
        if !stable_character_absence(&first, &second) {
            anyhow::bail!(
                "World Shard Character presence changed while checking {guid}; cleanup is deferred"
            );
        }
        Ok(false)
    }

    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>> {
        self.creature_template(entry)
    }

    fn pet_name(
        &self,
        requester_guid: u64,
        pet_number: u32,
        pet_guid: u64,
    ) -> Result<Option<codec::PetNameView>> {
        self.pet_name(requester_guid, pet_number, pet_guid)
    }

    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>> {
        self.gameobject_template(entry)
    }

    fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>> {
        self.gameobject_type(go_guid)
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

    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        self.player_skills(character_guid)
    }

    fn effective_armor(&self, guid: u64) -> u32 {
        self.effective_armor(guid)
    }

    fn effective_magic_resistances(&self, guid: u64) -> [u32; 6] {
        self.effective_magic_resistances(guid)
    }

    fn pending_system_messages(&self, self_guid: u64) -> Vec<String> {
        self.system_messages_for(self_guid)
    }

    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool> {
        self.npc_refuses_interaction(npc_guid, player_guid)
    }

    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<codec::MailView>> {
        self.mail_list(recipient_guid)
    }

    fn mail_by_id(&self, mail_id: u64) -> Result<Option<codec::MailView>> {
        Ok(self.mail_by_id(mail_id))
    }

    fn realm_account_name(&self, character_guid: u64) -> Result<Option<String>> {
        Ok(self.realm_account_name(character_guid))
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

    fn mail_return(&self, recipient_guid: u64, mail_id: u64, same_account: bool) -> Result<()> {
        self.mail_return(recipient_guid, mail_id, same_account)
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
        same_account: bool,
    ) -> Result<()> {
        self.mail_send(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            item_guid,
            same_account,
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

    fn mail_copy_text(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_copy_text(recipient_guid, mail_id)
    }

    fn mail_grant_letter(&self, payee_guid: u64, item_text_id: u32) -> Result<()> {
        self.mail_grant_letter(payee_guid, item_text_id)
    }

    fn mail_mark_letter_granted(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_mark_letter_granted(recipient_guid, mail_id)
    }

    fn item_text(&self, item_text_id: u32) -> Result<Option<String>> {
        self.item_text(item_text_id)
    }

    fn owns_item_with_text(
        &self,
        owner_guid: u64,
        item_text_id: u32,
        hint_item_guid: u64,
    ) -> Result<bool> {
        self.owns_item_with_text(owner_guid, item_text_id, hint_item_guid)
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
        same_account: bool,
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
            same_account,
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
        delivery_delay_secs: u32,
        reward: Option<lyracore_shared::mail::RewardHeader>,
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
            delivery_delay_secs,
            reward,
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
    ) -> Result<crate::world::TrainerBuyOutcome> {
        self.buy_trainer_spell(account_id, self_guid, trainer_guid, spell_id)
    }

    fn trainer_offer_skill_line(&self, trainer_guid: u64, spell_id: u32) -> u32 {
        self.trainer_offer_skill_line(trainer_guid, spell_id)
    }

    fn talent_grant_spell(&self, talent_id: u32) -> u32 {
        self.talent_by_id(talent_id)
            .map(|t| t.grant_spell_id)
            .unwrap_or(0)
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

    fn reset_talents(&self, account_id: u64, self_guid: u64, trainer_guid: u64) -> Result<()> {
        self.reset_talents(account_id, self_guid, trainer_guid)
    }

    fn auto_bank_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()> {
        self.auto_bank_item(account_id, self_guid, slot)
    }

    fn buy_bank_slot(&self, account_id: u64, self_guid: u64, banker_guid: u64) -> Result<()> {
        self.buy_bank_slot(account_id, self_guid, banker_guid)
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

    fn pet_command(
        &self,
        account_id: u64,
        self_guid: u64,
        data: u32,
        target_guid: u64,
    ) -> Result<()> {
        self.pet_command(account_id, self_guid, data, target_guid)
    }
    fn set_sheathed(&self, account_id: u64, self_guid: u64, state: u8) -> Result<()> {
        self.set_sheathed(account_id, self_guid, state)
    }

    fn entity_max_health(&self, guid: u64) -> u32 {
        self.entity_max_health(guid)
    }

    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32> {
        self.superseded_old_rank(new_spell, player_guid)
    }

    fn send_chat(
        &self,
        account_id: u64,
        self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<crate::world::ChatOutcome> {
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

    fn gm_command(&self, account_name: &str, self_guid: u64, text: String) -> Result<()> {
        self.gm_command(account_name, self_guid, text)
    }

    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.repop(account_id, self_guid)
    }

    fn claim_session(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::world::WorldSessionToken> {
        self.claim_session(account_id, character_guid)
    }

    fn bind_session(
        &self,
        token: crate::world::WorldSessionToken,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        Ok(Some(std::sync::Arc::new(self.bind_session(token)?)))
    }

    fn release_session(&self, token: crate::world::WorldSessionToken) -> Result<()> {
        self.release_session(token)
    }

    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()> {
        self.reclaim_corpse(account_id, self_guid, corpse_guid)
    }

    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()> {
        self.resurrect_response(account_id, self_guid, accept)
    }

    fn self_resurrect(&self, account_id: u64, self_guid: u64) -> Result<()> {
        self.self_resurrect(account_id, self_guid)
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

    fn character_identity(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::presence::CharacterIdentity>> {
        self.character_identity(guid)
    }

    fn live_entity(&self, guid: u64) -> Option<codec::MemberEntity> {
        self.live_entity(guid)
    }

    fn character_in_transit(&self, guid: u64) -> bool {
        self.character_in_transit(guid)
    }

    fn auto_reply_text(&self, guid: u64) -> Result<Option<String>> {
        Ok(self.auto_reply_text(guid))
    }

    fn every_shard_vouches_for_absence(&self) -> Result<()> {
        self.world_shards_for_absence()?;
        Ok(())
    }

    fn in_world_players(&self) -> Result<Vec<crate::world::presence::RealmPresence>> {
        self.in_world_players()
    }

    fn zone_name(&self, zone_id: u32) -> String {
        self.zone_name(zone_id)
    }

    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<u64>, Vec<u64>)> {
        self.contact_lists(self_guid)
    }

    fn ignored_guids(&self, owner_guid: u64) -> Result<Vec<u64>> {
        self.ignored_guids(owner_guid)
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
    fn group_invite(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
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
    fn group_accept(
        &self,
        account_id: u64,
        self_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_accept(account_id, self_guid)
    }
    fn group_decline(
        &self,
        account_id: u64,
        self_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_decline(account_id, self_guid)
    }
    fn group_leave(
        &self,
        account_id: u64,
        self_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_leave(account_id, self_guid)
    }
    fn group_uninvite(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.group_uninvite(account_id, self_guid, target_guid)
    }
    fn group_loot_method(
        &self,
        account_id: u64,
        self_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<crate::world::party::PartyOutcome> {
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

    fn party_cleanup_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        self.realm_core()
            .map(|realm| Some(std::sync::Arc::new(realm) as std::sync::Arc<dyn WorldStore>))
    }

    fn party_command_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        self.realm_core()
            .map(|realm| Some(std::sync::Arc::new(realm) as std::sync::Arc<dyn WorldStore>))
    }

    fn sync_transfer_arrival(&self, character_guid: u64) -> Result<()> {
        crate::world::party::sync_transfer_arrival_mirror(self, character_guid)
    }

    fn transfer_realm(&self) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(None);
        }
        self.realm_core()
            .map(|realm| Some(std::sync::Arc::new(realm) as std::sync::Arc<dyn WorldStore>))
    }

    fn sync_transfer_pending(&self, character_guid: u64) -> Result<()> {
        crate::world::party::sync_transfer_arrival_mirror(self, character_guid)
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

    fn party_command_worlds(&self) -> Result<Vec<std::sync::Arc<dyn WorldStore>>> {
        if !self.is_sharded() {
            return Ok(Vec::new());
        }
        Ok(self
            .configured_world_shards()?
            .into_iter()
            .map(|coordinator| std::sync::Arc::new(coordinator) as std::sync::Arc<dyn WorldStore>)
            .collect())
    }

    fn claim_bot_invite_intent(&self, intent_id: u64) -> Result<crate::world::party::PartyOutcome> {
        self.claim_bot_invite_intent(intent_id)
    }

    fn claim_party_command_intent(&self, intent_id: u64, claim_token: u64) -> Result<()> {
        Coordinator::claim_party_command_intent(self, intent_id, claim_token)
    }

    fn admit_party_command_authority(
        &self,
        group_id: u64,
        leader_guid: u64,
        bot_guid: u64,
        authority_member_guid: u64,
        expected_members: Vec<u64>,
    ) -> Result<crate::world::party::CompanionCommandOutcome> {
        Coordinator::admit_party_command_authority(
            self,
            group_id,
            leader_guid,
            bot_guid,
            authority_member_guid,
            expected_members,
        )
    }

    fn apply_admitted_party_command(
        &self,
        command: &crate::world::party::AdmittedCompanionCommand,
    ) -> Result<crate::world::party::CompanionCommandOutcome> {
        Coordinator::apply_admitted_party_command(self, command)
    }

    fn finish_party_command_intent(
        &self,
        intent_id: u64,
        claim_token: u64,
        outcome: crate::world::party::CompanionCommandOutcome,
    ) -> Result<()> {
        Coordinator::finish_party_command_intent(self, intent_id, claim_token, outcome)
    }

    fn confirm_party_command_receipt(
        &self,
        source_identity: spacetimedb_sdk::Identity,
        intent_id: u64,
    ) -> Result<Option<crate::world::party::CompanionCommandOutcome>> {
        Coordinator::confirm_party_command_receipt(self, source_identity, intent_id)
    }

    fn confirm_party_command_holder(
        &self,
        bot_guid: u64,
    ) -> Result<crate::world::party::PartyCommandHolder> {
        Coordinator::confirm_party_command_holder(self, bot_guid)
    }

    fn entity_partition(&self, guid: u64) -> Option<(u32, u64)> {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .map(|entity| (entity.map_id, entity.instance_id))
    }

    fn mark_bot_transfer_arrival_ready(
        &self,
        intent_id: u64,
        bot_guid: u64,
        controller_generation: u64,
        claim_token: u64,
    ) -> Result<()> {
        Coordinator::mark_bot_transfer_arrival_ready(
            self,
            intent_id,
            bot_guid,
            controller_generation,
            claim_token,
        )
    }

    fn bot_transfer_arrival_matches(
        &self,
        transfer_id: u64,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> bool {
        self.0
            .coord()
            .conn
            .db
            .game_transfer_in()
            .transfer_id()
            .find(&transfer_id)
            .is_some_and(|arrival| {
                arrival.character_guid == intent.bot_guid
                    && arrival.bot_intent_source == intent.source_module_identity
                    && arrival.bot_intent_id == intent.id
                    && arrival.bot_controller_generation == intent.controller_generation
                    && arrival.bot_intent_created_micros == intent.created_micros
                    && arrival.source_map_id == intent.source_map
                    && arrival.source_instance_id == intent.source_instance
                    && arrival.source_locator_revision == intent.source_locator_revision
            })
    }

    fn release_bot_transfer_arrival(
        &self,
        transfer_id: u64,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        Coordinator::release_bot_transfer_arrival(
            self,
            transfer_id,
            intent.bot_guid,
            intent.source_module_identity,
            intent.id,
            intent.controller_generation,
            intent.created_micros,
            intent.source_map,
            intent.source_instance,
            intent.source_locator_revision,
        )
    }

    fn admit_sessionless_group_action(
        &self,
        character_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.admit_sessionless_group_action(character_guid)
    }

    fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.realm_group_op(op, actor_guid, target_guid, arg_a, arg_b, arg_c)
    }

    fn realm_group_op_visible(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
        arg_c: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.realm_group_op_visible(op, actor_guid, target_guid, arg_a, arg_b, arg_c)
    }

    fn deleted_character_party_leave(
        &self,
        character_guid: u64,
    ) -> Result<crate::world::party::PartyOutcome> {
        self.deleted_character_party_leave(character_guid)
    }

    fn group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster(character_guid))
    }

    fn party_command_group_roster(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        self.party_command_group_roster(character_guid)
    }

    fn group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        Ok(self.group_roster_by_id(group_id))
    }

    fn group_roster_revision(&self, group_id: u64) -> Result<u64> {
        Ok(self.group_roster_revision(group_id))
    }

    fn held_roster_revision(&self, group_id: u64) -> Result<Option<u64>> {
        Ok(self.held_roster_revision(group_id))
    }

    fn realm_character_partition(
        &self,
        character_guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        self.realm_character_partition(character_guid)
    }

    fn party_holder_observation(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<crate::world::party::PartyHolderObservation> {
        let serves_locator = self.shard_for_location(map_id, instance_id).is_none();
        self.stable_party_holder_observation(character_guid, serves_locator)
    }

    fn party_cleanup_group_roster_by_id(
        &self,
        group_id: u64,
    ) -> Result<Option<crate::world::party::GroupRoster>> {
        let healthy = {
            let live = self.0.coord();
            live.is_healthy()
        };
        if !healthy {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self.group_roster_by_id(group_id))
    }

    fn party_member_guids(&self) -> Result<Vec<u64>> {
        if !self.0.coord().is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self.party_member_guids())
    }

    fn party_group_ids(&self) -> Result<Vec<u64>> {
        if !self.0.coord().is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party cleanup",
                self.shard_name()
            );
        }
        Ok(self.party_group_ids())
    }

    fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        self.sync_group_mirror(roster)
    }

    fn loot_roll(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<crate::world::LootActionStatus> {
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
        random_property_id: u32,
        promotion_source: spacetimedb_sdk::Identity,
        source_roll_id: u64,
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
            random_property_id,
            promotion_source,
            source_roll_id,
        )
    }

    fn realm_loot_vote(
        &self,
        corpse_guid: u64,
        slot: u8,
        actor_guid: u64,
        vote: u8,
    ) -> Result<crate::world::LootActionStatus> {
        self.realm_loot_vote(corpse_guid, slot, actor_guid, vote)
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
    ) -> Result<crate::world::LootActionStatus> {
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
    fn add_friend(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
        target_race: u8,
    ) -> Result<crate::world::ContactOutcome> {
        self.add_friend(account_id, self_guid, target_guid, target_race)
    }

    fn del_friend(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::ContactOutcome> {
        self.del_friend(account_id, self_guid, target_guid)
    }

    fn add_ignore(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::ContactOutcome> {
        self.add_ignore(account_id, self_guid, target_guid)
    }

    fn del_ignore(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
    ) -> Result<crate::world::ContactOutcome> {
        self.del_ignore(account_id, self_guid, target_guid)
    }
}

/// The realm-core split's store seam. Every method here is a view onto
/// `Coordinator`'s like-named INHERENT method — Rust resolves inherent methods first, so these are
/// forwards, not recursion, exactly like the `WorldStore` impl above.
///
/// This block is the ONE layer `realm_core.rs`'s fake substitutes for wholesale. Keep it a block of
/// forwards; any logic that grows here is untested by construction. `has_escrow` narrows
/// `Option<TransferOut>` to a bool.
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
    fn session_key(&self, account_id: u64) -> Result<Option<SessionKey>> {
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
    fn request_gm_command(
        &self,
        actor_guid: u64,
        alpha_test_tools: bool,
        text: String,
    ) -> Result<()> {
        self.request_gm_command(actor_guid, alpha_test_tools, text)
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
    fn realm_character_partition(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        self.realm_character_partition(guid)
    }
    fn begin_character_shard_transfer(
        &self,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
        source_module_identity: spacetimedb_sdk::Identity,
        intent_id: u64,
        controller_generation: u64,
        character_guid: u64,
    ) -> Result<()> {
        self.begin_character_shard_transfer(
            source_map,
            source_instance,
            source_revision,
            destination_map,
            destination_instance,
            source_module_identity,
            intent_id,
            controller_generation,
            character_guid,
        )
    }
    fn finish_character_shard_transfer(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        self.finish_character_shard_transfer(intent)
    }
    fn finish_player_character_shard_transfer(
        &self,
        character_guid: u64,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
    ) -> Result<()> {
        self.finish_player_character_shard_transfer(
            character_guid,
            source_map,
            source_instance,
            source_revision,
            destination_map,
            destination_instance,
        )
    }
    fn finish_pending_character_shard_transfer(
        &self,
        character_guid: u64,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
        source_module_identity: spacetimedb_sdk::Identity,
        transfer_intent_id: u64,
        controller_generation: u64,
    ) -> Result<()> {
        self.finish_pending_character_shard_transfer(
            character_guid,
            source_map,
            source_instance,
            source_revision,
            destination_map,
            destination_instance,
            source_module_identity,
            transfer_intent_id,
            controller_generation,
        )
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
mod stable_absence_tests {
    #[test]
    fn a_transfer_between_durable_scans_cannot_prove_stable_absence() {
        use super::{stable_character_absence, CharacterPresenceSnapshot};

        let snapshot = |generation, revision, present| CharacterPresenceSnapshot {
            generation,
            revision,
            present,
        };
        let first = [snapshot(1, 0, false), snapshot(2, 1, false)];
        let destination_visible = [snapshot(1, 1, true), snapshot(2, 1, false)];
        assert!(!stable_character_absence(&first, &destination_visible));

        let moved_again = [snapshot(1, 2, false), snapshot(2, 2, false)];
        assert!(!stable_character_absence(&first, &moved_again));

        let unchanged = [snapshot(1, 0, false), snapshot(2, 1, false)];
        assert!(stable_character_absence(&first, &unchanged));
    }
}
