//! `WorldStore`: the storage/coordination seam `world/mod.rs` calls through so the handshake
//! + crypto (and, via `handlers/`, every per-domain reducer call) can be unit-tested without a
//! database or a real socket. Pure code-motion out of `mod.rs` (behind the `pub use` facade) —
//! see `mod.rs`'s module doc for the full picture. Kept as ONE trait (only two implementors);
//! the section markers below are load-bearing navigation, not a split.

use super::*;

pub trait WorldStore: Send + Sync {
    /// Look up the shared session key K (+ account id) for an (already uppercased) account
    /// name. `None` when no live session exists for that account (reject the handshake).
    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>>;

    /// Multi-shard routing: the handle for the shard that OWNS `character_guid`'s location,
    /// resolved once per world entry and then used for EVERY player-scoped call and subscription of
    /// that session (see `on_home_shard!`). `None` means "you are already on the right shard" —
    /// which is what a single-entry shard map, and every mock, always answer, so the session keeps
    /// the handle it was given and behaves byte-identically to the pre-sharding gateway.
    fn home_shard(&self, _character_guid: u64) -> Option<std::sync::Arc<dyn WorldStore>> {
        None
    }

    /// The database this handle targets — routing identity, for logs and for the tests that assert
    /// no call ever escapes the player's home shard. `""` for mocks that don't model shards.
    fn shard_name(&self) -> &str {
        ""
    }

    // --- Cross-database transfer. Every one defaults to the single-database posture, so a
    // --- store that does not shard (and every mock that does not exercise transfers) is unchanged.

    /// Put `character_guid` on the shard that owns its location, running the escrowed transfer if
    /// it is somewhere else, then answer the same question [`home_shard`](Self::home_shard) does.
    /// Called at every world entry; `Err` fails the login rather than letting a half-moved
    /// character into the world on either side.
    fn settle_home_shard(
        &self,
        character_guid: u64,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        Ok(self.home_shard(character_guid))
    }

    /// The escrow row this shard holds for `character_guid`, if any — the transfer's identity, its
    /// destination and the serialized character. `None` = not mid-transfer here.
    fn escrowed_transfer(&self, _character_guid: u64) -> Option<EscrowedTransfer> {
        None
    }

    /// Where this shard's durable row says the character is going (`world::teleport_player` wrote
    /// the destination there before despawning the entity). `None` = this shard has no row for it.
    fn character_destination(&self, _character_guid: u64) -> Option<TransferPlan> {
        None
    }

    /// `begin_transfer` — freeze + serialize + delete the live entity, in one transaction.
    fn begin_transfer(&self, _plan: &TransferPlan) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `import_character_blob` — materialise the arrival copy from the carried blob.
    fn import_character_blob(&self, _transfer_id: u64, _blob: &[u8]) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `confirm_import` — attest, on the SOURCE, that the destination copy is durable.
    fn confirm_import(&self, _transfer_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `finish_transfer` — delete-last: destroy the source copy and clear the escrow.
    fn finish_transfer(&self, _transfer_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `release_transfer` — drop the arrival copy's fence. Replay-safe: `Ok` when there is nothing
    /// filed under this id, which is why it can be called speculatively at world entry.
    fn release_transfer(&self, _transfer_id: u64) -> Result<()> {
        Ok(())
    }

    /// `set_character_shard` on the REALM-CORE handle — publish where a settled transfer put the
    /// character. Called by `transfer::run_transfer` immediately after
    /// `finish_transfer` commits, so it can only ever name a destination the escrow actually
    /// reached; see `crate::realm_core::publish_shard_index` for why that is the strongest form of
    /// "transactional" available across two databases.
    ///
    /// The default is a no-op so a store that does not shard is unchanged — the same posture as
    /// every other transfer method. Production overrides it in `stdb::world_store`.
    fn publish_shard_index(
        &self,
        _character_guid: u64,
        _map_id: u32,
        _instance_id: u64,
    ) -> Result<()> {
        Ok(())
    }

    /// `ensure_instance` — mirror an instance id onto this shard, spawning its population once.
    fn ensure_instance(&self, _instance_id: u64, _map_id: u32, _party_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// `evict_instance_population` — drop an instance's population here, keeping the lease row.
    fn evict_instance_population(&self, _instance_id: u64) -> Result<()> {
        Err(anyhow!(
            "this store does not implement cross-database transfers"
        ))
    }

    /// Bind this shard's per-player connection identity to the account (`establish_session`), so
    /// `player_login` can resolve the caller here. A no-op on the realm shard, where the logon tier
    /// already did it. Called at world entry whenever the session's home shard is not the realm.
    fn bind_shard_session(&self, _account_id: u64, _session_key: &[u8; 40]) -> Result<()> {
        Ok(())
    }

    // --- Realm-wide party state (the realm-core group slice). Every one defaults to the single-database
    // --- posture, so an unsharded store — and every mock that does not model a realm — is
    // --- unchanged: `realm_store()` answering `None` is what routes every party op back onto the
    // --- player's own shard through the pre-realm-core reducers.

    /// The **realm-core** handle: the database that owns party membership realm-wide.
    ///
    /// `None` is not "no realm-core configured" — it is "this gateway runs against ONE database", in
    /// which case that database already is the authority and there is nothing to route. `world::party`
    /// branches on exactly this, so the single-database path never reads a row it did not read before.
    fn realm_store(&self) -> Option<std::sync::Arc<dyn WorldStore>> {
        None
    }

    /// Every connected WORLD shard's handle (realm-core excluded — it owns no gameplay reads). The
    /// fan-out set for the roster mirror; empty on a single-database gateway, which is what makes the
    /// mirror push a no-op there.
    fn world_stores(&self) -> Vec<std::sync::Arc<dyn WorldStore>> {
        Vec::new()
    }

    /// `realm_group_op` — run one party op against the database this handle names. Called on
    /// the realm-core handle; the op byte and argument slots are `lyracore_shared::group::realm_op`.
    fn realm_group_op(
        &self,
        _op: u8,
        _actor_guid: u64,
        _target_guid: u64,
        _arg_a: u8,
        _arg_b: u8,
    ) -> Result<()> {
        Err(anyhow!("this store does not host realm-wide party state"))
    }

    /// The party `character_guid` is in, as THIS handle's database sees it: authoritative on
    /// realm-core, a mirror on a world shard. `None` = not in a party there.
    fn group_roster(&self, _character_guid: u64) -> Result<Option<party::GroupRoster>> {
        Ok(None)
    }

    /// [`group_roster`](Self::group_roster) keyed by the group — the read the mirror push needs for a
    /// party the acting character has just left.
    fn group_roster_by_id(&self, _group_id: u64) -> Result<Option<party::GroupRoster>> {
        Ok(None)
    }

    /// `sync_group_mirror` — replace this shard's mirror of one party with realm-core's roster.
    /// An empty `roster.members` is the disband tombstone.
    fn sync_group_mirror(&self, _roster: &party::GroupRoster) -> Result<()> {
        Ok(())
    }

    /// `realm_whisper` — deliver one whisper against the database this handle names. Called on the
    /// realm-core handle, which is the only one that can address BOTH parties of
    /// a cross-shard whisper: `recipient_guid` is realm-wide, a bound identity is per-database.
    ///
    /// `sender_is_ignored` is the target's ignore-list verdict, resolved by the gateway from the shard
    /// that holds the target's contact rows — realm-core has none. The default errors rather than
    /// silently succeeding: a store that does not host the realm plane must never be *asked*, and
    /// `world::whisper` only asks the handle `realm_store()` handed it.
    fn realm_whisper(
        &self,
        _sender_guid: u64,
        _target_guid: u64,
        _message: String,
        _sender_is_ignored: bool,
    ) -> Result<()> {
        Err(anyhow!("this store does not host realm-wide whispers"))
    }

    // --- Realm-wide loot rolls. Every one defaults to the single-database posture, so an
    // --- unsharded store — and every mock that does not model a realm — is unchanged: `realm_store()`
    // --- answering `None` is what routes `CMSG_LOOT_ROLL` back onto the player's own shard through
    // --- the original `loot_roll` reducer, and leaves the relay (`loot::relay_tick`) with nothing to do.

    // Mirrors the `realm_loot_op` REDUCER's parameter list 1:1 — this trait is the seam between them, so the shapes have to match.
    #[allow(clippy::too_many_arguments)]
    /// `realm_loot_op` — run one loot-roll op against the database THIS handle names. Called on
    /// the **realm-core** handle: START promotes a world shard's staging roll, VOTE casts a vote.
    fn realm_loot_op(
        &self,
        _op: u8,
        _corpse_guid: u64,
        _slot: u8,
        _item_entry: u32,
        _actor_guid: u64,
        _vote: u8,
        _deadline_micros: i64,
        _recipients: Vec<u64>,
    ) -> Result<()> {
        Err(anyhow!("this store does not host realm-wide loot rolls"))
    }

    /// Every UNRESOLVED loot roll this WORLD SHARD has created but not yet had promoted onto
    /// realm-core — the relay's promotion queue. Empty by default, which is what makes the
    /// relay a no-op on an unsharded store and on realm-core's own handle (nothing is ever created
    /// there directly — only `realm_loot_op`'s START arm writes it, and that is not this method).
    fn pending_local_rolls(&self) -> Result<Vec<loot::PendingLootRoll>> {
        Ok(Vec::new())
    }

    /// `settle_loot_roll` — grant a resolved roll's item on THIS world shard, if it holds the
    /// matching corpse row. A no-op default so an unsharded store, and every shard that does
    /// not hold the corpse, are unaffected; the module's own `withheld` guard is what makes a
    /// wrong-shard call harmless in production too.
    fn settle_loot_roll(&self, _corpse_guid: u64, _slot: u8, _winner_guid: u64) -> Result<()> {
        Ok(())
    }

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the relay
    /// has promoted it onto realm-core. A no-op default, matching `sync_group_mirror`'s shape.
    fn clear_promoted_loot_roll(&self, _roll_id: u64) -> Result<()> {
        Ok(())
    }

    // Same shape as `Coordinator::loot_won_since` (watermark + `(corpse, slot, winner)` triples) — the trait mirrors the read it fronts.
    #[allow(clippy::type_complexity)]
    /// Every `ROLL_WON` `game_group_event` row realm-core has pushed with an id greater than
    /// `after_id` — `(corpse_guid, slot, winner_guid)` triples, plus the new high-water mark to
    /// poll from next. Called on the **realm-core** handle. `(after_id, [])` by default, so the relay
    /// never advances its watermark and never settles anything on an unsharded/mock store.
    fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        Ok((after_id, Vec::new()))
    }

    /// The account's characters for the character-select screen (Phase 3). In production this
    /// reads the per-player `game_character` subscription (RLS-restricted to the owner).
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>>;

    /// Create a character for the account (`CMSG_CHAR_CREATE`). Returns the game outcome
    /// (success / name-in-use / failed); `Err` only for an unrecoverable transport failure.
    fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome>;

    /// Delete a character for the account (`CMSG_CHAR_DELETE`). Returns the game
    /// outcome (success/failed); `Err` only for an unrecoverable transport failure. Ownership is
    /// enforced module-side (the character must belong to `account_id`).
    fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<codec::CharDeleteOutcome>;

    /// Enter the world with `character_guid` (Phase 4): calls the `player_login` reducer and
    /// returns the live entity to spawn (from the resulting `game_world_entity` row). Errors if
    /// the character isn't the caller's.
    fn player_login(&self, account_id: u64, character_guid: u64) -> Result<codec::EntityView>;

    /// Persist + relay an inbound movement (Phase 5): calls the `movement_update` reducer with
    /// the mover (= `ctx.sender` on the per-player path; named by `self_guid` on the
    /// `LYRACORE_SHARED_CALLS` path), the opcode to relay, and the
    /// `MovementInfo`. Relayed peer events arrive back on the per-player subscription (Phase 6).
    fn movement_update(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()>;

    /// Movement, submitted WITHOUT waiting for the module's completion.
    ///
    /// The outcome lands in `feedback` instead, and the session applies it on its next packet. The
    /// default implementation forwards to the blocking `movement_update`, so mock stores and any
    /// future `WorldStore` keep working unchanged — only the live `Coordinator` overrides it.
    fn movement_update_nowait(
        &self,
        account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
        _feedback: &std::sync::Arc<MovementFeedback>,
    ) -> Result<()> {
        // Returns the error INLINE and deliberately does not also record it in `feedback` — doing
        // both would count one failure twice (once here, once when the caller drains the slot on the
        // next packet), which is exactly what
        // `a_movement_packet_for_a_despawned_entity_never_kills_the_session` caught. A store that
        // answers synchronously has no deferred verdict to report.
        self.movement_update(account_id, self_guid, opcode, info)
    }

    /// Subscribe this player's connection to its per-player views (nearby `game_world_entity`,
    /// addressed `game_movement_event`) and push the resulting peer-spawn / movement-relay / destroy
    /// SMSG onto `tx` (Phase 6/7). The returned guard tears the subscription + callbacks down on
    /// drop. Called once, at `CMSG_PLAYER_LOGIN`, when `self_guid` is known.
    fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        login_instance: u64,
        login_map: u32,
        login_x: f32,
        login_y: f32,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions>;

    /// Remove the player from the world (Phase 7): calls the `logout` reducer so the live
    /// `game_world_entity` row is deleted and observers see the peer vanish. Called on disconnect.
    fn logout(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Look up a character by guid (any owner) to answer `CMSG_NAME_QUERY` — the queried guid is
    /// usually a peer, so this is not account-scoped.
    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>>;

    /// Look up a creature template by entry to answer `CMSG_CREATURE_QUERY` (Tier 2 / NPCs).
    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>>;

    /// Look up an item template by entry to answer `CMSG_ITEM_QUERY_SINGLE` (items slice-1).
    fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>>;

    /// Look up a gameobject template by entry to answer `CMSG_GAMEOBJECT_QUERY`.
    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>>;

    /// The `type_id` of a SPAWNED gameobject by its live guid — lets
    /// `CMSG_GAMEOBJ_USE` route a `go_type::QUESTGIVER` GO (the Wanted Poster, the Lost Guards
    /// corpses) to the quest window instead of the loot/toggle reducer path. Defaulted to `Ok(None)`
    /// (never a questgiver) so existing `WorldStore` implementors (test mocks) that don't override it
    /// keep their prior CMSG_GAMEOBJ_USE behavior unchanged; only the production `Coordinator` impl
    /// (`stdb::world_store`) overrides it with a real read.
    fn gameobject_type(&self, _go_guid: u64) -> Result<Option<u8>> {
        Ok(None)
    }

    /// Use a gameobject (`CMSG_GAMEOBJ_USE`): a chest rolls its loot, a quest-object grants credit.
    fn use_gameobject(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()>;

    /// Enter an area trigger (`CMSG_AREATRIGGER`): credit any active "explore" quest tied to it.
    fn enter_areatrigger(&self, account_id: u64, self_guid: u64, trigger_id: u32) -> Result<()>;

    /// Forward a parsed addon-bridge command to the module's `client_command` reducer ON
    /// THE PLAYER'S CONNECTION — the handler runs with exactly the player's reducer authority.
    fn client_command(&self, account_id: u64, self_guid: u64, cmd: String, payload: String) -> Result<()>;

    /// Read every item a character owns, for the login item spawns + inventory slots (items slice-1).
    fn player_items(&self, owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>>;
    /// The character's learned skill lines as `(skill_line, current, max_rank)` — feeds the self
    /// CREATE's SkillInfo block. Empty when no `game_player_skill` rows exist.
    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>>;

    /// The EFFECTIVE armor for `guid` (base + worn gear armor) for the self-login CREATE's
    /// `UNIT_FIELD_RESISTANCES[0]` — so the character sheet shows real worn armor on relog. Auras aren't
    /// folded here (they self-correct via the on_aura relay). Mirrors the module's combat `effective_armor`.
    fn effective_armor(&self, guid: u64) -> u32;

    /// Read a corpse's item loot for the loot window (items slice-4): `(slot, id, count, display)`,
    /// filtered for `viewer_guid` (a `quest_only` row is shown only to a
    /// viewer who currently needs it, or who already owns a per-member reserved clone of it).
    fn corpse_loot(&self, corpse_guid: u64, viewer_guid: u64) -> Result<Vec<codec::LootItemView>>;

    /// Read a vendor's stock for `SMSG_LIST_INVENTORY` (Tier 2 / vendors): resolve the vendor's
    /// creature entry from its entity row, join `game_npc_vendor` × `game_item_template`.
    fn vendor_items(&self, vendor_guid: u64) -> Result<Vec<codec::VendorItemView>>;

    /// Standing-derived reaction gate: does this NPC refuse `player_guid` its
    /// interaction WINDOW (gossip/vendor/trainer/questgiver)? Rep-bar factions refuse at
    /// Unfriendly-or-below standing; bar-less factions fall back to the FactionTemplate hostility
    /// masks. Fail-open on missing data.
    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool>;

    /// Buy `count` of `item_entry` from `vendor_guid` (`CMSG_BUY_ITEM`, Tier 2). The module gates
    /// the purchase on the vendor (stock / range / copper); a gameplay `Err` is per-action, not fatal.
    fn buy_item(
        &self,
        account_id: u64,
        self_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()>;

    /// Sell the item in inventory `slot` back to `vendor_guid` (`CMSG_SELL_ITEM`, Tier 2). The gateway
    /// resolves the client's item-instance guid to its slot first (the reducer takes the slot) and
    /// passes the vendor guid the client named so the module can range-gate the sale (like buy).
    fn sell_item(&self, account_id: u64, self_guid: u64, vendor_guid: u64, slot: u8) -> Result<()>;

    /// Re-purchase item in buyback ring slot `slot` (0-based) from vendor (`CMSG_BUYBACK_ITEM`). The
    /// gateway maps `BuybackSlot.as_int() - 69` before calling; the module gates range + copper.
    fn buyback_item(&self, account_id: u64, self_guid: u64, vendor_guid: u64, slot: u8) -> Result<()>;

    /// Repair the item with the given inventory `slot` at REPAIR-NPC `npc_guid` (`CMSG_REPAIR_ITEM`).
    /// The gateway resolves the client's item-instance guid to its slot first (the reducer takes the
    /// slot); the module gates the NPC (REPAIR flag / range) and charges copper. `slot == u8::MAX`
    /// repairs the whole body. A gameplay `Err` (out of range / too poor) is per-action, not fatal.
    fn repair_item(&self, account_id: u64, self_guid: u64, npc_guid: u64, slot: u8) -> Result<()>;

    /// The spells a class trainer (`trainer_guid`) teaches, each pre-folded with the player's level +
    /// known-state for the `SMSG_TRAINER_LIST` Green/Red/Gray rendering (`CMSG_TRAINER_LIST`).
    fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>>;

    /// Buy/learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`). The module gates it
    /// (range / level / cost / not-already-known); `Err` carries a `[N]` gtker failure-reason tag.
    fn buy_trainer_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<()>;

    /// Skin a beast corpse that has been fully looted (no items, no money left). The module gates it
    /// (dead beast, in range, not already skinned); on success the leather lands in the bag via the
    /// item-subscription relay. `Err` = not applicable (not a beast, out of range, already skinned,
    /// or dead player) — the caller falls through to the empty loot window and the player sees nothing.
    fn skin_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()>;

    /// Given an item-instance GUID from a client spell-target, return the bag slot for that item
    /// (so the disenchant / enchant_item reducer can receive a slot, not a GUID).
    fn item_slot_by_guid(&self, account_id: u64, item_guid: u64) -> Option<u8>;

    /// Disenchant the item in `slot` (`CMSG_CAST_SPELL` spell 13262). The module validates skill +
    /// item disenchantability and yields Strange Dust into the bag.
    fn disenchant_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()>;

    /// Apply `enchant_id` to the item in `slot` (`CMSG_CAST_SPELL` for enchant spell). The module
    /// validates skill, consumes reagent dust, and stamps enchant_id on the item instance.
    fn enchant_item_on_slot(&self, account_id: u64, self_guid: u64, slot: u8, enchant_id: u32) -> Result<()>;

    /// Return the `grant_spell_id` for `talent_id` (0 = passive, no ability granted), so the gateway
    /// can push `SMSG_LEARNED_SPELL` for ability talents after a successful `learn_talent`.
    fn talent_grant_spell(&self, talent_id: u32) -> u32;

    /// True iff `spell_id` spawns a ground area (E_PERSISTENT_AREA) — its GO carries no hit list.
    fn spell_is_ground_area(&self, spell_id: u32) -> bool;

    /// True iff `spell_id` is a Fishing cast (E_FISH) — routed to the `fish` reducer.
    fn spell_is_fishing(&self, spell_id: u32) -> bool;

    /// The instant-resolve Fishing catch.
    fn fish(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// True iff `spell_id` is an Open-Lock cast (E_OPEN_LOCK) — routed to the `pick_lock` reducer.
    fn spell_is_open_lock(&self, spell_id: u32) -> bool;

    /// Pick the lock on GameObject `go_guid` (`CMSG_CAST_SPELL` for Pick Lock). The module gates it
    /// (range / lock requirement / caster's Lockpicking skill); `Err` = refused (out of range, not
    /// locked, or skill too low) → the gateway answers SMSG_CAST_RESULT::Failure.
    fn pick_lock(&self, account_id: u64, self_guid: u64, go_guid: u64) -> Result<()>;

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`); action 0 clears the slot.
    fn set_action_button(
        &self,
        account_id: u64, self_guid: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()>;

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`, 195 slice B).
    /// `reputation_index` is the client's 0..63 rep-array slot, NOT a faction id.
    fn set_faction_at_war(
        &self,
        account_id: u64, self_guid: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()>;

    /// Talent-pane sync after a successful `learn_talent`: `(teach_spell, superseded_prev,
    /// points_remaining)` — the rank-spell to relay as LEARNED/SUPERCEDED (the 1.12 TalentFrame
    /// derives shown ranks from known rank-spells) and the live PLAYER_CHARACTER_POINTS1 value
    /// (earned − spent). `talent_id = 0` → just the points.
    fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32);

    /// Sum of the character's spent talent ranks — non-zero gates the login points correction.
    fn talent_points_spent(&self, character_guid: u64) -> u32;

    /// The character's active spell-modifier auras as raw (family_mask, op, amount, is_pct) rows —
    /// the SMSG_SET_FLAT/PCT_SPELL_MODIFIER mirror source.
    fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)>;

    /// Spend a talent point on `talent_id` (`CMSG_LEARN_TALENT`). The module gates it (points available
    /// / max rank / prerequisites); a gameplay `Err` is per-action, not session-fatal.
    fn learn_talent(&self, account_id: u64, self_guid: u64, talent_id: u32) -> Result<()>;

    /// Equip the item in main-inventory `from_slot` into its matching equipment slot
    /// (`CMSG_AUTOEQUIP_ITEM`). The module resolves the target slot from the item's `inventory_type`
    /// and validates the required-level gate; a gameplay `Err` is per-action, not session-fatal.
    fn equip_item(&self, account_id: u64, self_guid: u64, from_slot: u8) -> Result<()>;

    /// Unequip the item in equipment `from_slot` (0..=18) into a free backpack slot (right-click an
    /// equipped item → `CMSG_AUTOSTORE_BAG_ITEM`). Errors (not equipped / backpack full) are per-action.
    fn unequip_item(&self, account_id: u64, self_guid: u64, from_slot: u8) -> Result<()>;

    /// Use the consumable in main-inventory `slot` (`CMSG_USE_ITEM`) — eat/drink/potion/bandage. The
    /// module applies the item's on-use effect (flat heal for slice food) and decrements the stack.
    /// (Using a Hearthstone routes through here too — the module recalls to the bound home.)
    fn use_item(&self, account_id: u64, self_guid: u64, slot: u8) -> Result<()>;

    /// Bind the caller's hearthstone home to their current position (innkeeper gossip "Make this inn
    /// your home."). No args — the module resolves the caller via `ctx.sender`.
    fn bind_home(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Does the NPC at `guid` carry the innkeeper flag? Gates the "Make this inn your home." gossip
    /// option + the bind select.
    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool>;

    /// Resolve the `title_text_id` to embed in `SMSG_GOSSIP_MESSAGE` for the NPC at `guid`.
    /// Looks up `game_gossip_menu` by creature entry; falls back to 1 (generic greeting).
    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32;

    /// Look up the full weighted greeting (all 8 `npc_text` slots) for a `text_id`.
    /// Returns `None` when no imported `game_npc_text` row exists (the gateway falls back to the
    /// generic greeting string).
    fn npc_text_for_id(&self, text_id: u32) -> Option<codec::NpcTextView>;

    /// The imported gossip menu options for the NPC at `guid`, sorted by
    /// `option_index`, RAW/unfiltered by condition. Empty when nothing is imported for this creature
    /// (the gateway falls back to the flag-derived vendor/innkeeper synthesis).
    fn gossip_options(&self, npc_guid: u64) -> Result<Vec<codec::GossipOptionView>>;

    /// `(taken, rewarded)` for `quest_id` in `guid`'s quest log — feeds the QUEST_TAKEN/QUEST_REWARDED
    /// gossip option conditions (`codec::option_condition_holds`).
    fn quest_status(&self, guid: u64, quest_id: u32) -> (bool, bool);

    /// Respec at `trainer_guid` (the "I wish to unlearn my talents." gossip option, gated to level
    /// 10+ by `filtered_gossip_options` — #516). Errors (out of range / not enough gold) are
    /// per-action; the caller just closes the gossip window either way.
    fn reset_talents(&self, account_id: u64, self_guid: u64, trainer_guid: u64) -> Result<()>;

    /// Move (or swap) the item in main-inventory `from_slot` to `to_slot` (`CMSG_SWAP_INV_ITEM`/
    /// `CMSG_SWAP_ITEM`). The module's move primitive validates equip-slot transitions, so this also
    /// covers drag-to-equip and drag-to-unequip. A gameplay `Err` is per-action, not session-fatal.
    fn move_item(&self, account_id: u64, self_guid: u64, from_slot: u8, to_slot: u8) -> Result<()>;

    /// Evaluate a quest giver's quests against the player for the overhead status icon + the quest
    /// menu (quests gateway slice). See `stdb::reads::quest_giver_evals`.
    fn quest_giver_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<codec::GiverQuestEval>>;

    /// Build a quest's detail view (accept / offer-reward / completion screens). `None` if unloaded.
    fn quest_detail(&self, quest_id: u32) -> Result<Option<codec::QuestDetailView>>;

    /// Accept a quest from a giver (`CMSG_QUESTGIVER_ACCEPT_QUEST`). The module gates it; a gameplay
    /// `Err` is per-action, not session-fatal.
    fn accept_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()>;

    /// Turn a completed quest in for its rewards (`CMSG_QUESTGIVER_CHOOSE_REWARD`). The module
    /// validates completion + grants money/XP/items. `reward_index` is the player's pick-1-of-N choice
    /// reward slot (the CMSG `reward` field); ignored by quests with no choice rewards.
    fn turn_in_quest(
        &self,
        account_id: u64,
        self_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()>;

    /// Abandon an active quest (`CMSG_QUESTLOG_REMOVE_QUEST`). The module deletes the quest-log row;
    /// the relay clears the slot. The gateway resolves the client's log SLOT to the quest id first.
    fn abandon_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()>;

    /// Item-starts-quest: does the item in `owner_guid`'s inventory `slot` carry a
    /// non-zero `start_quest`? `Some((item_guid, quest_id))` if so — `CMSG_USE_ITEM` opens the quest
    /// details screen (item guid as giver) instead of consuming it. `None` for an ordinary item.
    fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)>;

    /// Quest sharing: share `quest_id` with the caller's party (`CMSG_PUSHQUESTTOPARTY`).
    /// The module validates the sender is grouped + actively on the quest and pushes per-member
    /// `QUEST_SHARE`/`QUEST_PUSH_RESULT` events; a gameplay `Err` (not grouped / not on the quest) is
    /// per-action, not session-fatal.
    fn push_quest(&self, account_id: u64, self_guid: u64, quest_id: u32) -> Result<()>;

    /// The player's active quests as quest-log descriptor slots (Phase 2 — the L window). Empty if
    /// none. Encoded into the `PLAYER_QUEST_LOG_*` fields + sent via the raw VALUES path.
    fn player_quest_log(&self, player_guid: u64) -> Result<Vec<codec::update_mask::QuestLogSlot>>;

    /// The player's LEARNED spells (`game_player_spell`, beyond the class kit) — chained into the
    /// login SMSG_INITIAL_SPELLS so a taught ability (e.g. Auto Shot) reaches the client spellbook.
    fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>>;

    /// The player's persisted reputation standings (`game_player_reputation`) as `(reputation_index,
    /// standing)` pairs — folded into the login `SMSG_INITIALIZE_FACTIONS` so a relog shows
    /// the real standing instead of the all-neutral stub.
    fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>>;

    /// The player's IMPORTED action-bar rows (`game_player_action`) as `(button,
    /// action, action_type)` triples — empty pre-import (the common case today), in which case the
    /// login codec falls back to synthesizing the bar from the spellbook (byte-identical to before
    /// this method existed).
    fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>>;

    /// The player's buyback ring, newest-first: `(item_entry, stack_count, price)` per entry
    /// (≤12). Read by the gateway to rebuild the client's buyback-tab view after
    /// sell/buyback and at login — the table itself is private (coordinator-only).
    fn buyback_ring(&self, player_guid: u64) -> Vec<(u32, u32, u32)>;

    /// The rank a trainer offering actually teaches (LearnSpell wrapper → its trigger; a
    /// self-contained rank resolves to itself). Mirrors the module's buy-time resolution so
    /// SMSG_LEARNED_SPELL books the granted spell, never the wrapper.
    fn resolve_learn_target(&self, spell_id: u32) -> u32;

    /// The KNOWN rank `new_spell` supersedes — Some(prev) drives SMSG_SUPERCEDED_SPELL on a buy.
    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32>;

    /// Is `guid`'s live entity currently in the world? The WORLDPORT_ACK gate: a cross-map
    /// transfer despawns the entity until the ack rebuilds it, so
    /// ABSENT = a transfer is genuinely pending; PRESENT = the ack is spurious (double-send or
    /// crafted) and must be ignored — honoring it would tear down and rebuild a live player
    /// (visible blink, gateway combat-bookkeeping reset) at zero cost to the client.
    fn entity_in_world(&self, guid: u64) -> bool;

    /// Record the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3). 0 clears it.
    fn set_target(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// Validate a `CMSG_INSPECT` request: `target_guid` must be a real in-world player, on the
    /// caller's map, in range, and friendly. `Ok(())` → the gateway replies `SMSG_INSPECT(target_guid)`;
    /// `Err` (out of range / hostile / no such target) → silently ignored, matching the other
    /// stateless-gate reducers (`enter_areatrigger`, `use_gameobject`).
    fn inspect(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1).
    fn start_attack(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// Relay a pet command-bar action (`CMSG_PET_ACTION`). `data` is the raw packed action
    /// (flag<<24 | id): flag 0x07 = command (Stay/Follow/Attack/Dismiss), flag 0x06 = react state
    /// (Passive/Defensive/Aggressive). The module decodes + validates (all pet policy lives there).
    fn pet_command(&self, account_id: u64, self_guid: u64, data: u32, target_guid: u64) -> Result<()>;

    /// Start the player's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot / 5019 wand
    /// Shoot), from `CMSG_CAST_SPELL`. Requires a ranged weapon equipped (the module enforces it).
    fn start_ranged_attack(
        &self,
        account_id: u64,
        self_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()>;

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    fn stop_attack(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Draw or stow the player's weapons (`CMSG_SETSHEATHED`, the `Z` key). `state` is 0 stowed /
    /// 1 melee / 2 ranged; the module range-checks it. Writes `UNIT_FIELD_BYTES_2` byte 0, which is
    /// what makes a drawn or stowed weapon visible to OTHER players. [#101]
    fn set_sheathed(&self, account_id: u64, self_guid: u64, state: u8) -> Result<()>;

    /// Cast a spell (`CMSG_CAST_SPELL`, aura tracer). Self-cast; target ignored.
    fn cast_spell(&self, account_id: u64, self_guid: u64, spell_id: u32, target_guid: u64) -> Result<()>;

    /// Cast a GROUND-TARGETED spell at a clicked world point (`CMSG_CAST_SPELL` with a DEST_LOCATION
    /// target block — Flamestrike/Blizzard/Rain of Fire). `(x,y,z)` is the ground click; the module
    /// anchors the AoE/patch there.
    fn cast_spell_at(
        &self,
        account_id: u64, self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()>;

    /// Cancel one of the caller's own auras by spell id (`CMSG_CANCEL_AURA` — the player right-clicks a
    /// buff icon to remove it). The module deletes the matching aura on the caller; the aura relay then
    /// re-syncs the buff bar.
    fn cancel_aura(&self, account_id: u64, self_guid: u64, spell_id: u32) -> Result<()>;

    /// Cancel the caller's in-progress cast (`CMSG_CANCEL_CAST` — the player pressed Esc, moved, or
    /// recast). The module deletes the caller's pending cast so the scheduled completion never fires a
    /// phantom `SMSG_SPELL_GO` that wedges the client in "Another action is in progress".
    fn cancel_cast(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// The spell's cast time (ms) from the static game_spell header — 0 = instant, None = unknown.
    /// The CMSG_CAST_SPELL handler uses it to clear instant casts synchronously.
    fn spell_cast_time(&self, spell_id: u32) -> Option<u32>;

    /// The live entity's max health (0 if not in world) — the fall-damage flavor line folds
    /// the shared curve against it.
    fn entity_max_health(&self, guid: u64) -> u32;

    /// True iff `spell_id` queues on the caster's next melee swing (Heroic Strike/Cleave). The
    /// CMSG_CAST_SPELL handler then sends NO synchronous START/CAST_RESULT/GO — the swing-fire
    /// emits them.
    fn spell_queues_next_swing(&self, spell_id: u32) -> bool;

    /// True iff `spell_id` is an auto-repeat ranged attack (Auto Shot / wand Shoot) — the
    /// `RANGED_AUTO_REPEAT` cast_flags bit. The CMSG_CAST_SPELL handler routes on this instead of a
    /// hardcoded `spell == 75 || 5019` id list; a new ranged auto-repeat ability onboards as data.
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool;

    /// Enchant/disenchant routing for `spell_id` from its effect rows — `None` for a normal cast. Lets
    /// the CMSG_CAST_SPELL handler route ITEM-target enchanting by effect KIND (enchant id in the effect
    /// data) instead of a hardcoded spell-id list; a new enchant is a data row, no gateway change.
    fn enchant_route(&self, spell_id: u32) -> Option<EnchantRoute>;

    /// Join a chat channel — the client auto-sends CMSG_JOIN_CHANNEL on zone-in.
    fn join_channel(&self, account_id: u64, self_guid: u64, channel: String) -> Result<()>;

    /// Leave a chat channel (`CMSG_LEAVE_CHANNEL`).
    fn leave_channel(&self, account_id: u64, self_guid: u64, channel: String) -> Result<()>;

    /// Speak into a joined channel (the CMSG_MESSAGECHAT Channel arm).
    fn send_channel_message(&self, account_id: u64, self_guid: u64, channel: String, message: String)
        -> Result<()>;

    /// Speak (`CMSG_MESSAGECHAT`, social tier): broadcast a say/yell line. `chat_type` 0 = say, 1 = yell.
    fn send_chat(
        &self,
        account_id: u64,
        self_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()>;

    /// Perform an emote (`CMSG_TEXT_EMOTE`, social tier): broadcast the "X dances." line + animation.
    /// `target_guid` (0 = untargeted) is the client's selected target — the gateway resolves it to a
    /// name so the chat line reads "X waves at <target>."
    fn send_emote(
        &self,
        account_id: u64,
        self_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()>;

    /// Broadcast a `/roll` result (`MSG_RANDOM_ROLL_Client`): pick a server-side random in
    /// `[min_roll, max_roll]` and fan the result to all nearby players as `MSG_RANDOM_ROLL_Server`.
    fn send_roll(&self, account_id: u64, self_guid: u64, min_roll: u32, max_roll: u32) -> Result<()>;

    /// Whisper `message` privately to the player named `target_player` (`CMSG_MESSAGECHAT` Whisper).
    fn send_whisper(&self, account_id: u64, self_guid: u64, target_player: String, message: String) -> Result<()>;

    /// Party chat (`CMSG_MESSAGECHAT` Party, `/p`): deliver `message` to every OTHER
    /// current group member plus an echo to the caller, over the `game_group_event` relay (no
    /// gateway-subscribed table — see `module/src/chat.rs::party_chat`'s doc). `Err` when the caller
    /// isn't in a group ([`lyracore_shared::group::err::NOT_IN_GROUP`] — the gateway maps it to
    /// `SMSG_PARTY_COMMAND_RESULT(NotInGroup)`, "You aren't in a party") or on the other
    /// `send_chat`-style rejections (not in world / empty message), which are silently dropped like
    /// say/yell.
    fn party_chat(&self, account_id: u64, self_guid: u64, message: String) -> Result<()>;

    /// GM playtest dot-command: `text` is the raw Say line, STILL carrying its
    /// leading `.` — the Say handler intercepts it BEFORE any chat relay/insert and forwards it here
    /// verbatim (module-side parsing keeps the command set data-free). `Err`'s message is relayed back
    /// to the SENDER ONLY as a system chat line (never broadcast, never a `game_chat_event` row).
    fn gm_command(&self, account_id: u64, self_guid: u64, text: String) -> Result<()>;

    /// Read a corpse's lootable copper for `SMSG_LOOT_RESPONSE` (slice 3). 0 if the target is gone
    /// or not a corpse. Read-only — the actual take is `loot_money`.
    fn loot_target_money(&self, target_guid: u64) -> Result<u32>;

    /// Take the money from a corpse the player has open (`CMSG_LOOT_MONEY`, slice 3): the module
    /// validates dead+range+has-money, moves the copper to the looter, and clears the lootable flag.
    fn loot_money(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    /// Take one item from the open corpse's loot into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`, slice
    /// 4): the module moves the corpse-loot item in `loot_slot` into a free inventory slot and deletes
    /// the loot row. The item then appears in the bag via the inventory live-relay.
    fn take_loot(
        &self,
        account_id: u64,
        self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
    ) -> Result<()>;

    /// Revive the caller after death (`CMSG_REPOP_REQUEST` / Release Spirit, slice 4): the module
    /// restores full health in place and clears the dead state (the client leaves the death screen
    /// once the restored health replicates).
    fn repop(&self, account_id: u64, self_guid: u64) -> Result<()>;

    /// Claim a fresh in-world session epoch (at player_login) so a stale socket's late logout can't
    /// delete a newer session's entity. The caller presents the returned epoch at teardown.
    fn claim_session(&self, account_id: u64) -> u64;

    /// Release a session epoch at teardown; returns true iff it was still current — i.e. the caller
    /// still owns the entity and may delete it. False means a newer login superseded this session.
    fn release_session(&self, account_id: u64, epoch: u64) -> bool;

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`, slice 5): the module validates the caller
    /// is a ghost owning the corpse, in range, past the reclaim delay, then resurrects at 50%.
    fn reclaim_corpse(&self, account_id: u64, self_guid: u64, corpse_guid: u64) -> Result<()>;

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`): `accept=true` revives the
    /// caller at the offer's frozen `%`; either way the offer is consumed. A failure (no pending offer
    /// for the caller) is expected when the offer already lapsed/was answered — per-action, log + ignore.
    fn resurrect_response(&self, account_id: u64, self_guid: u64, accept: bool) -> Result<()>;

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`): a ghost activates the graveyard Spirit
    /// Healer to res IN PLACE at 50% health/mana + a Resurrection Sickness debuff. `healer_guid` is the
    /// activated healer's guid (passed through to the confirm echo). The module gates on ghost state.
    fn spirit_healer_res(&self, account_id: u64, self_guid: u64, healer_guid: u64) -> Result<()>;

    /// Find `owner_guid`'s corpse location `(map_id, x, y, z)` for `MSG_CORPSE_QUERY` (slice 5).
    fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>>;

    /// Return the `combat_until_ms` timestamp for `player_guid`'s entity row (0 if the entity is not
    /// found). Used by the logout handler to deny `CMSG_LOGOUT_REQUEST` while the player is in combat.
    fn player_combat_until_ms(&self, player_guid: u64) -> u64;

    /// All currently-online player characters for `CMSG_WHO → SMSG_WHO`. A player is "online" iff
    /// their guid appears in `game_world_entity` with `entry == 0` (player entity). Joined with
    /// `game_character` for name/race/class/zone; dead players are included (ghosts are online).
    fn online_players(&self) -> Result<Vec<codec::WhoPlayerView>>;

    /// `self_guid`'s friend list + ignore list (guids only) for `CMSG_FRIEND_LIST → SMSG_FRIEND_LIST`
    /// + `SMSG_IGNORE_LIST`. Online friends carry live presence (level/class/zone).
    fn contact_lists(&self, self_guid: u64) -> Result<(Vec<codec::FriendView>, Vec<u64>)>;

    /// Resolve a typed contact name to a character guid (case-insensitive, like `send_whisper`'s
    /// target match), for `CMSG_ADD_FRIEND`/`CMSG_ADD_IGNORE`. `None` if no character has that name.
    fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>>;

    /// A character's live presence `(online, level, class, zone_id)` for `SMSG_FRIEND_STATUS`'s
    /// Added-Online-vs-Offline split. `None` if the guid doesn't resolve to any character.
    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>>;

    /// `CMSG_ADD_FRIEND` (the name is already resolved to `target_guid` by the gateway).
    fn add_friend(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_DEL_FRIEND`.
    fn del_friend(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_ADD_IGNORE` (the name is already resolved to `target_guid` by the gateway).
    fn add_ignore(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_DEL_IGNORE`.
    fn del_ignore(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;

    // The SINGLE-DATABASE party path (`world::party::run`'s `None` arm). Each takes the caller's
    // `self_guid` as well as its account: the account is what identifies the player CONNECTION these
    // reducers run on, and the guid is what identifies the CHARACTER to realm-core on the other arm.
    // Both are threaded through one call site (`world::social`), so the two planes take the same
    // arguments and a mock sees which character the op was for either way.

    /// `CMSG_GROUP_INVITE` (name gateway-resolved). Module Err strings map to PartyResult codes.
    fn group_invite(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_ACCEPT`.
    fn group_accept(&self, account_id: u64, self_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_DECLINE`.
    fn group_decline(&self, account_id: u64, self_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_DISBAND` (the client's "Leave Party").
    fn group_leave(&self, account_id: u64, self_guid: u64) -> Result<()>;
    /// `CMSG_GROUP_UNINVITE` (name gateway-resolved) — the leader kicks a member.
    fn group_uninvite(&self, account_id: u64, self_guid: u64, target_guid: u64) -> Result<()>;
    /// `CMSG_LOOT_METHOD` — the leader sets the party's loot method/
    /// threshold/master. `loot_setting`/`loot_threshold` are the gateway-decoded `GroupLootSetting`/
    /// `ItemQuality` wire bytes, passed straight through (the module adopted the wire ordering
    /// verbatim — zero translation).
    fn group_loot_method(
        &self,
        account_id: u64,
        self_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()>;
    /// `CMSG_LOOT_ROLL` — record the caller's need/greed/pass vote.
    fn loot_roll(&self, account_id: u64, self_guid: u64, corpse_guid: u64, loot_slot: u32, vote: u8) -> Result<()>;
    /// `CMSG_LOOT_MASTER_GIVE` — the master looter assigns an above-
    /// threshold row to `target_guid`.
    fn loot_master_give(
        &self,
        account_id: u64, self_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<()>;
    /// NOTIFY-ONLY module chokepoint for a gossip-option click — fired best-effort
    /// before the gateway's own gossip handling; failure never blocks the gossip reply.
    fn gossip_select(
        &self,
        account_id: u64, self_guid: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()>;
}
