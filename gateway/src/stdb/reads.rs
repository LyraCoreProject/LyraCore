//! Pure cache-accessor methods on `Coordinator`: read the privileged subscription cache (RLS
//! bypass) and project rows into codec views. No reducer calls — those live in `reducers.rs`.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use super::bindings::*;
use super::connection::Coordinator;
use super::views::{character_view, item_template_view, AccountRow, RealmRow};

impl Coordinator {
    /// Read the single realm row for the realm-list reply (Phase 1).
    pub fn realm(&self) -> Result<RealmRow> {
        self.0
            .coord()
            .conn
            .db
            .game_realm()
            .iter()
            .next()
            .map(|r| RealmRow {
                id: r.id,
                name: r.name,
                address: r.address,
                realm_type: r.realm_type,
                flags: r.flags,
                population: r.population,
                timezone: r.timezone,
            })
            .ok_or_else(|| anyhow!("no game_realm row in the coordinator cache"))
    }

    /// Read an account's SRP6 salt/verifier for the logon challenge (Phase 1).
    pub fn account_by_username(&self, username: &str) -> Result<Option<AccountRow>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_account()
            .username()
            .find(&username.to_string())
            .map(|a| AccountRow {
                id: a.id,
                username: a.username,
                salt: a.salt,
                verifier: a.verifier,
                banned: a.banned,
            }))
    }

    /// Count characters on the realm for an account (realm-list character count).
    pub fn character_count(&self, account_id: u64) -> Result<u8> {
        let n = self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .iter()
            .filter(|c| c.account_id == account_id)
            .count();
        Ok(n.min(u8::MAX as usize) as u8)
    }

    /// Read an account's characters for the character-select screen (Phase 3). In production
    /// this reads the per-player `game_character` subscription cache (RLS-restricted to owner).
    /// Equipment slots (0..=18) are populated from `game_item_instance` + `game_item_template`
    /// so the client renders the character's gear on the select screen instead of all-naked.
    pub fn characters(&self, account_id: u64) -> Result<Vec<crate::codec::CharacterView>> {
        use wow_world_messages::vanilla::{CharacterGear, InventoryType};
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut views: Vec<crate::codec::CharacterView> = db
            .game_character()
            .iter()
            .filter(|c| c.account_id == account_id)
            .map(character_view)
            .collect();
        // Fill equipment slots 0..=18 from item instances.  Slots ≥ 19 are backpack/bag slots;
        // skip them.  An unknown inventory_type degrades to InventoryType::default (Non) which
        // the client treats the same as display_id=0 — no model shown, no crash.
        for view in &mut views {
            for item in db
                .game_item_instance()
                .iter()
                .filter(|i| i.owner_guid == view.guid && i.slot <= 18)
            {
                let Some(tmpl) = db.game_item_template().entry().find(&item.entry) else {
                    continue;
                };
                let inv_type =
                    InventoryType::try_from(u32::from(tmpl.inventory_type)).unwrap_or_default();
                view.equipment[item.slot as usize] = CharacterGear {
                    equipment_display_id: tmpl.display_id,
                    inventory_type: inv_type,
                };
            }
        }
        Ok(views)
    }

    /// Read a single character by guid (any owner) for a `CMSG_NAME_QUERY` reply. The queried guid
    /// is usually a *peer*, so this reads across owners via the privileged cache (no RLS on the
    /// owner connection), unlike `characters` which filters to one account.
    pub fn character_by_guid(&self, guid: u64) -> Result<Option<crate::codec::CharacterView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&guid)
            .map(character_view))
    }

    /// Where a character IS **on this shard**, as `(map_id, instance_id)` — the input to shard
    /// routing (#17). Prefers the LIVE entity and falls back to the durable `game_character` row,
    /// which is what a fresh login (and a mid-teleport character, whose entity was despawned)
    /// reads. `None` = this shard has no row for that guid, which is also how
    /// `realm_core::locate_home_shard` finds the shard that does (issue #47).
    ///
    /// The durable fallback reads `pending_instance_id`, NOT a hardcoded 0 (#19): that column is
    /// where `teleport_player` parks the DESTINATION instance for a cross-map hop, so it is the
    /// whole routing key for instance entry — reading 0 there would route a player walking into
    /// Deadmines by map alone, which is correct only as long as no shard-map rule ever names a
    /// bucket (`389:0=pool-a`, see `config::ShardMap`).
    pub fn character_location(&self, guid: u64) -> Option<(u32, u64)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        if let Some(e) = db.game_world_entity().guid().find(&guid) {
            return Some((e.map_id, e.instance_id));
        }
        db.game_character()
            .guid()
            .find(&guid)
            .map(|c| (c.map_id, c.pending_instance_id))
    }

    /// This shard's raw durable `game_character` row for `guid` (#19) — the transfer driver needs
    /// the DESTINATION fields (`pending_instance_id`, position) that `CharacterView` does not carry.
    pub(crate) fn character_row(&self, guid: u64) -> Option<super::bindings::Character> {
        self.0.coord().conn.db.game_character().guid().find(&guid)
    }

    /// This shard's escrow row for `guid`, raw (#19). `Coordinator`-level rather than
    /// `WorldStore`-level because it's needed (via `has_escrow`) before any handle has been chosen
    /// as the session's home — see `realm_core::locate_home_shard` (#47).
    pub(crate) fn escrow_row(&self, guid: u64) -> Option<super::bindings::TransferOut> {
        use spacetimedb_sdk::Table as _;
        self.0
            .coord()
            .conn
            .db
            .game_transfer_out()
            .iter()
            .find(|r| r.character_guid == guid)
    }

    /// Where a character is STANDING, on this handle's database, as `(x, y, instance_id)` — the
    /// region overlay's input (#23). `character_location` answers the PARTITION (`map_id`,
    /// `instance_id`); a region is finer than a partition, so routing by region needs the position
    /// too. Entity first (the live row), then the durable character row for someone who is logged
    /// out.
    ///
    /// The instance id comes back with the position **and is not the one `character_location`
    /// reports**: that read answers `0` for anyone without a live entity, while a character who is
    /// mid-instance-entry has no entity and carries the destination in `pending_instance_id` (the
    /// field `teleport_player` stamps and the escrow reads). Regions partition the OPEN WORLD only,
    /// so the carve-out has to see the pending id or it opens exactly at the moment — the
    /// WORLDPORT_ACK re-entry into an instance — it exists to cover.
    pub fn character_position(&self, guid: u64) -> Option<(f32, f32, u64)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        if let Some(e) = db.game_world_entity().guid().find(&guid) {
            return Some((e.x, e.y, e.instance_id));
        }
        db.game_character()
            .guid()
            .find(&guid)
            .map(|c| (c.x, c.y, c.pending_instance_id))
    }

    /// The baked region definitions held by this handle's database (#23) — content data imported by
    /// the world ETL alongside `grid_x`/`grid_y`. Rows that break the geometry rules (overlap, the
    /// ~10×10 cell floor) are DROPPED with a log line: their cells fall back to `DEFAULT_REGION` and
    /// therefore to the ordinary shard map, so a bad menu can only ever collapse toward today.
    ///
    /// #72 slice 2: memoized behind `CoordinatorInner::cached_map_regions` (invalidated by the
    /// `game_map_region` `on_insert`/`on_delete` callbacks `connect_blocking` registers). This was a
    /// once-per-world-entry read before the warm handoff existed; the seam check calls it on every
    /// cell crossing a moving player makes, and re-decoding + re-logging every rejected row on each
    /// call does not scale to that — see the field doc on `CoordinatorInner::map_regions_cache`.
    pub fn map_regions(&self) -> lyracore_shared::region::RegionMap {
        self.0.cached_map_regions(|| {
            let rows: Vec<lyracore_shared::region::Region> = self
                .0
                .coord()
                .conn
                .db
                .game_map_region()
                .iter()
                .map(|r| lyracore_shared::region::Region {
                    map_id: r.map_id,
                    region_id: r.region_id,
                    gx_min: r.gx_min,
                    gx_max: r.gx_max,
                    gy_min: r.gy_min,
                    gy_max: r.gy_max,
                })
                .collect();
            let (map, rejected) = lyracore_shared::region::RegionMap::build(rows);
            for why in rejected {
                log::error!("region definition dropped on {}: {why}", self.shard_name());
            }
            map
        })
    }

    /// The region→shard assignment rows this handle's database holds (#23). Authoritative on
    /// realm-core; empty everywhere else, which is why the caller asks the realm-core handle.
    pub fn region_assignments(&self) -> Vec<crate::config::RegionAssignment> {
        self.0
            .coord()
            .conn
            .db
            .game_region_assignment()
            .iter()
            .map(|a| crate::config::RegionAssignment {
                map_id: a.map_id,
                region_id: a.region_id,
                shard: a.shard,
                epoch: a.epoch,
            })
            .collect()
    }

    /// Open-world player population on THIS shard, bucketed by region (#78) — every LIVE player
    /// entity's cell, run through this shard's own baked region definitions ([`Coordinator::map_regions`]).
    /// Approximate by construction: a snapshot of wherever everyone happened to be at the sample
    /// instant, not a windowed average (`docs/region-sharding.md`'s staleness note). Instances are
    /// excluded (`instance_id == 0` only) — regions partition the open world, per `module/src/region.rs`'s
    /// doc — and creatures are excluded via the same `owner_identity == Identity::ZERO` rule
    /// `helpers.rs` documents on the module side.
    ///
    /// Positions are collected FIRST and the `coord()` read guard dropped before calling
    /// `map_regions()` (which takes its own guard internally) — nesting two guards from the same
    /// `RwLock` on one thread is the kind of thing that only deadlocks once a writer is queued, so
    /// it is not worth relying on read-lock reentrancy here.
    pub fn region_player_counts(&self) -> Vec<(u32, u32, u32)> {
        let positions: Vec<(u32, i32, i32)> = {
            let guard = self.0.coord();
            guard
                .conn
                .db
                .game_world_entity()
                .iter()
                .filter(|e| {
                    e.instance_id == 0 && e.owner_identity != spacetimedb_sdk::Identity::ZERO
                })
                .map(|e| (e.map_id, e.grid_x, e.grid_y))
                .collect()
        };
        self.map_regions().count_by_region(positions)
    }

    /// The realm-core character→shard index entry for `guid`: the `(map_id, instance_id)` the realm
    /// believes the character is at (#20). A HINT — `config::resolve_home_shard` confirms it against
    /// the shard that actually holds the row before routing anything to it.
    pub fn character_shard(&self, guid: u64) -> Option<(u32, u64)> {
        self.0
            .coord()
            .conn
            .db
            .game_character_shard()
            .character_guid()
            .find(&guid)
            .map(|s| (s.map_id, s.instance_id))
    }

    /// Read a creature template by entry for a `CMSG_CREATURE_QUERY` reply (Tier 2 / NPCs).
    pub fn creature_template(&self, entry: u32) -> Result<Option<crate::codec::CreatureView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_creature_template()
            .entry()
            .find(&entry)
            .map(|t| crate::codec::CreatureView {
                entry: t.entry,
                name: t.name,
                subname: t.subname,
                display_id: t.display_id,
                creature_type: t.creature_type as u32,
                creature_family: t.creature_family,
                type_flags: t.type_flags,
                rank: t.rank as u32,
            }))
    }

    /// Read a gameobject template by entry for a `CMSG_GAMEOBJECT_QUERY` reply.
    pub fn gameobject_template(
        &self,
        entry: u32,
    ) -> Result<Option<crate::codec::GameObjectTemplateView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_gameobject_template()
            .entry()
            .find(&entry)
            .map(|t| crate::codec::GameObjectTemplateView {
                type_id: t.type_id,
                display_id: t.display_id,
                name: t.name,
                data0: t.data_0,
                data1: t.data_1,
            }))
    }

    /// The `type_id` of a SPAWNED gameobject, by its live guid (join `game_gameobject` →
    /// `game_gameobject_template`). Feeds the `CMSG_GAMEOBJ_USE` dispatch (work-item 041): a
    /// `lyracore_shared::constants::go_type::QUESTGIVER` GO (the Wanted Poster, the Lost Guards corpses)
    /// opens the quest window instead of rolling loot / toggling state — that is what a questgiver
    /// gameobject does in vanilla. `None` for an unspawned/unknown guid (the caller falls back to the
    /// ordinary use-reducer path, which itself no-ops on an unknown guid).
    pub fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_gameobject()
            .guid()
            .find(&go_guid)
            .and_then(|go| {
                db.game_gameobject_template()
                    .entry()
                    .find(&go.template_entry)
            })
            .map(|t| t.type_id))
    }

    /// Read an item template by entry for a `CMSG_ITEM_QUERY_SINGLE` reply (items slice-1).
    pub fn item_template(&self, entry: u32) -> Result<Option<crate::codec::ItemTemplateView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_item_template()
            .entry()
            .find(&entry)
            .map(item_template_view))
    }

    /// The spell's cast time (ms) from the static `game_spell` header — 0 = instant. Used by the
    /// CMSG_CAST_SPELL handler to clear an instant cast SYNCHRONOUSLY (the async relay delivers START/GO
    /// after the aura effects, which wedges the 5875 client's cast slot). None = unknown spell. [083]
    pub fn spell_cast_time(&self, spell_id: u32) -> Option<u32> {
        self.0
            .coord()
            .conn
            .db
            .game_spell()
            .spell_id()
            .find(&spell_id)
            .map(|s| s.cast_time_ms)
    }

    /// Enchant/disenchant routing for `spell_id`, read off the spell's effect rows — `None` if the spell
    /// has no item-target enchanting effect (a normal cast). The gateway uses this to route ITEM-target
    /// casts by effect KIND, with the enchant id carried in the effect's `p_0`, instead of a hardcoded
    /// spell-id list — a new enchant spell is a data row, no gateway change. [094]
    pub fn enchant_route(&self, spell_id: u32) -> Option<crate::world::EnchantRoute> {
        use crate::world::EnchantRoute;
        const E_ENCHANT_ITEM: u8 = 0x17; // taxonomy E_ENCHANT_ITEM (p_0 = enchant_id)
        const E_DISENCHANT: u8 = 0x18; // taxonomy E_DISENCHANT
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .filter(|e| e.spell_id == spell_id)
            .find_map(|e| match e.kind {
                E_ENCHANT_ITEM => Some(EnchantRoute::Enchant(e.p_0 as u32)),
                E_DISENCHANT => Some(EnchantRoute::Disenchant),
                _ => None,
            })
    }

    /// True iff `spell_id` has a ground-area effect (taxonomy `E_PERSISTENT_AREA` 0x1B —
    /// Consecration etc.). The instant-cast GO for such a spell must carry an EMPTY hit list: the
    /// default self-cast fallback put the CASTER in `hits[]`, and the 5875 client rendered the
    /// spell's impact animation ON the paladin ("hit animation on the caster", 118). Kind-routed
    /// like `enchant_route` — a new ground spell is a data row.
    pub fn spell_is_ground_area(&self, spell_id: u32) -> bool {
        const E_PERSISTENT_AREA: u8 = 0x1B; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_PERSISTENT_AREA)
    }

    /// The character's active SPELL-MODIFIER auras (264) as raw `(family_mask, op, amount, is_pct)`
    /// rows — the client-mirror source for SMSG_SET_FLAT/PCT_SPELL_MODIFIER (aggregation by
    /// (op, mask-bit) happens in the codec helper; mangos sends the TOTAL per bit).
    pub fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        const A_SPELLMOD_FLAT: u8 = 0xAC; // lockstep with module taxonomy
        const A_SPELLMOD_PCT: u8 = 0xAD;
        self.0
            .coord()
            .conn
            .db
            .game_aura()
            .iter()
            .filter(|a| {
                a.target_guid == character_guid
                    && (a.eff_kind == A_SPELLMOD_FLAT || a.eff_kind == A_SPELLMOD_PCT)
            })
            .map(|a| {
                (
                    a.eff_p1 as u32,
                    a.eff_p0 as u8,
                    a.amount,
                    a.eff_kind == A_SPELLMOD_PCT,
                )
            })
            .collect()
    }

    /// True iff `spell_id` is a FISHING cast (taxonomy `E_FISH` 0x1C — the three tier ids carry a
    /// synthesized marker effect row). Kind-routed like `enchant_route`/`spell_is_ground_area`. [060]
    pub fn spell_is_fishing(&self, spell_id: u32) -> bool {
        const E_FISH: u8 = 0x1C; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_FISH)
    }

    /// True iff `spell_id` is an OPEN-LOCK cast (taxonomy `E_OPEN_LOCK` 0x1D — Pick Lock 1804). Routed to
    /// the `pick_lock` reducer, gateway-intercepted exactly like `spell_is_fishing`/`enchant_route`. The
    /// GO guid the pick targets rides the cast's SpellCastTargets (GAMEOBJECT flag). [119]
    pub fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        const E_OPEN_LOCK: u8 = 0x1D; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_OPEN_LOCK)
    }

    /// Read every item a character owns (items slice-1), joined with its template for the CREATE
    /// descriptors (max-durability). Read from the privileged cache (the coordinator bypasses RLS),
    /// filtered by `owner_guid` — the SDK exposes only the PK index, so iterate+filter like the other
    /// row queries. Returns the instance views ready for `build_item_create_object` + inventory slots.
    /// The character's learned skills as `(skill_line, current, max_rank)` — the self-CREATE
    /// SkillInfo block's live source (061). RLS-bypassed cache read like the sibling item read.
    pub fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_skill()
            .iter()
            .filter(|s| s.character_guid == character_guid)
            .map(|s| (s.skill_line, s.current, s.max_rank))
            .collect())
    }

    pub fn player_items(&self, owner_guid: u64) -> Result<Vec<crate::codec::ItemInstanceView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let items = db
            .game_item_instance()
            .iter()
            .filter(|i| i.owner_guid == owner_guid)
            // Skip an instance whose template is absent: a delete-template migration can leave orphaned rows
            // (e.g. synthetic ids deleted by the real-item remap), and sending a phantom item — blank/unknown
            // to the client, faulting on use/equip — is worse than dropping it. Defense-in-depth so a future
            // template delete can never push a broken item to a 5875 client.
            .filter_map(|i| {
                let tmpl = db.game_item_template().entry().find(&i.entry)?;
                Some(crate::codec::ItemInstanceView {
                    guid: i.guid,
                    entry: i.entry,
                    owner_guid: i.owner_guid,
                    slot: i.slot,
                    stack_count: i.stack_count,
                    durability: i.durability,
                    max_durability: tmpl.max_durability,
                    container_slots: tmpl.container_slots,
                })
            })
            .collect();
        Ok(items)
    }

    /// Bag slot of the item instance with `item_guid`. Reads from the coordinator's privileged cache
    /// (same source as `player_items`). Item GUIDs are globally unique so account_id isn't needed
    /// for the lookup — ownership is enforced by the module reducer on the call.
    pub fn item_slot_by_guid(&self, _account_id: u64, item_guid: u64) -> Option<u8> {
        // `collect()` forces the iterator to complete (and drop its borrow from `guard`) before `guard`
        // itself drops — same pattern as `player_items`.
        let guard = self.0.coord();
        let slots: Vec<u8> = guard
            .conn
            .db
            .game_item_instance()
            .iter()
            .filter(|i| i.guid == item_guid)
            .map(|i| i.slot)
            .collect();
        slots.into_iter().next()
    }

    /// Work-item 194 (item-starts-quest): does the item in `owner_guid`'s inventory `slot` carry a
    /// non-zero `item_template.start_quest`? Returns `(item_guid, quest_id)` if so — the gateway's
    /// `CMSG_USE_ITEM` handler uses this to open `SMSG_QUESTGIVER_QUEST_DETAILS` (item guid as giver)
    /// INSTEAD of calling `use_item` (never both — the item isn't consumed). `None` for every ordinary
    /// item (the ubiquitous case), which falls through to the normal use-item path unchanged.
    pub fn item_start_quest(&self, owner_guid: u64, slot: u8) -> Option<(u64, u32)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let hit: Option<(u64, u32)> = db
            .game_item_instance()
            .iter()
            .filter(|i| i.owner_guid == owner_guid && i.slot == slot)
            .find_map(|i| {
                db.game_item_template()
                    .entry()
                    .find(&i.entry)
                    .filter(|t| t.start_quest != 0)
                    .map(|t| (i.guid, t.start_quest))
            });
        hit
    }

    /// The EFFECTIVE armor for `guid` for the character-sheet CREATE (`UNIT_FIELD_RESISTANCES[0]`),
    /// Presence check for the WORLDPORT_ACK gate (224): is the guid's live entity in the world?
    pub fn entity_in_world(&self, guid: u64) -> bool {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .is_some()
    }

    /// folded from the coordinator's privileged cache: base (`game_world_entity.armor`) + worn gear armor.
    /// The coordinator isn't subscribed to `game_aura`, so the aura term is 0 here — login-present armor
    /// auras are pushed by the on_aura relay the instant they insert. Delegates to the shared
    /// `stdb::armor::effective_armor` so CREATE and the relays compute the IDENTICAL fold.
    pub fn effective_armor(&self, guid: u64) -> u32 {
        let guard = self.0.coord();
        super::armor::effective_armor(&guard.conn.db, guid)
    }

    /// The live entity's `max_health` from the privileged cache — 0 if not in world. Feeds the
    /// fall-damage flavor line (058); the module applies the authoritative damage itself.
    pub fn entity_max_health(&self, guid: u64) -> u32 {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .map(|e| e.max_health)
            .unwrap_or(0)
    }

    /// Return the `combat_until_ms` value for `player_guid`'s entity row (0 if the entity is not
    /// found or was never in combat). Read from the privileged coordinator cache.
    pub fn player_combat_until_ms(&self, player_guid: u64) -> u64 {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&player_guid)
            .map_or(0, |e| e.combat_until_ms)
    }

    /// Look up the `title_text_id` to put in `SMSG_GOSSIP_MESSAGE` for the given NPC. Resolves:
    /// entity.guid → entity.entry → `game_gossip_menu.entry` → `game_gossip_menu.text_id`.
    /// Falls back to `GOSSIP_GREETING_TEXT_ID = 1` when no row is found (generic greeting).
    pub fn npc_gossip_text_id(&self, npc_guid: u64) -> u32 {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let entry = match db.game_world_entity().guid().find(&npc_guid) {
            Some(e) => e.entry,
            None => return crate::codec::GOSSIP_GREETING_TEXT_ID,
        };
        match db.game_gossip_menu().entry().find(&entry) {
            Some(row) => row.text_id,
            None => crate::codec::GOSSIP_GREETING_TEXT_ID,
        }
    }

    /// Look up the full weighted greeting (work-item 217) for a `text_id`: `game_npc_text` (slot 0
    /// male, back-compat) + any `game_npc_text_slot` rows for the same id. Returns `None` when the
    /// text_id has no `game_npc_text` row at all (the gateway falls back to the generic greeting).
    ///
    /// Back-compat normalization: a `text_id` with NO `game_npc_text_slot` rows (never imported by the
    /// 217 importer — either a pre-217 row, or the legacy `npc_text[entry]` fallback path, which only
    /// ever populates `game_npc_text`) is read as slot 0 = `(text, text, 1.0)` and every other slot
    /// silent, matching the pre-217 single-slot behavior byte-for-byte. When slot rows DO exist, they
    /// are used verbatim (real per-slot probabilities from the dump) and slot 0's base-row `text` is
    /// NOT separately re-applied (the slot-0 row, always emitted by the importer alongside the others,
    /// is the source of truth once any slot row exists).
    pub fn npc_text_for_id(&self, text_id: u32) -> Option<crate::codec::NpcTextView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let base = db.game_npc_text().text_id().find(&text_id)?;
        let mut slot_rows: Vec<_> = db
            .game_npc_text_slot()
            .iter()
            .filter(|s| s.text_id == text_id)
            .collect();
        if slot_rows.is_empty() {
            let mut view = crate::codec::NpcTextView::default();
            view.slots[0] = (base.text.clone(), base.text, 1.0);
            return Some(view);
        }
        slot_rows.sort_by_key(|s| s.slot_index); // stable slot order (SQL has no ORDER BY in 2.5)
        let mut view = crate::codec::NpcTextView::default();
        for row in slot_rows {
            if (row.slot_index as usize) < 8 {
                view.slots[row.slot_index as usize] =
                    (row.text_male, row.text_female, row.probability);
            }
        }
        Some(view)
    }

    /// The imported gossip menu options for the NPC at `guid` (work-item 217), sorted by
    /// `option_index` (the render/select order) — RAW, unfiltered by condition; the dispatcher applies
    /// [`crate::codec::option_condition_holds`] identically at HELLO and SELECT_OPTION. Empty when the
    /// creature has no imported options (the gateway falls back to the flag-derived synthesis).
    pub fn gossip_options(&self, npc_guid: u64) -> Result<Vec<crate::codec::GossipOptionView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entry) = db
            .game_world_entity()
            .guid()
            .find(&npc_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let mut rows: Vec<_> = db
            .game_gossip_option()
            .iter()
            .filter(|o| o.entry == entry)
            .collect();
        rows.sort_by_key(|o| o.option_index);
        Ok(rows
            .into_iter()
            .map(|o| crate::codec::GossipOptionView {
                row_id: o.row_id,
                icon: o.icon,
                text: o.text,
                action: o.action,
                action_menu_id: o.action_menu_id,
                cond_type: o.cond_type,
                cond_value1: o.cond_value1,
                cond_value2: o.cond_value2,
            })
            .collect())
    }

    /// `(taken, rewarded)` for `quest_id` in `guid`'s quest log (privileged read — RLS bypassed, like
    /// `quest_giver_evals`) — feeds `option_condition_holds` for the QUEST_TAKEN/QUEST_REWARDED gossip
    /// option conditions (work-item 217). `taken` is true whenever a log row exists at all (accepted,
    /// whether or not yet turned in); a quest never seen by this player is `(false, false)`.
    pub fn quest_status(&self, guid: u64, quest_id: u32) -> (bool, bool) {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let found = db
            .game_character_quest()
            .iter()
            .find(|q| q.character_guid == guid && q.quest_entry == quest_id)
            .map(|row| row.rewarded);
        match found {
            Some(rewarded) => (true, rewarded),
            None => (false, false),
        }
    }

    /// Does the NPC at `guid` carry the innkeeper flag? Gates the "Make this inn your home." gossip
    /// option + the `bind_home` select. Reads `npc_flags` off the entity (privileged cache); absent → false.
    pub fn npc_is_innkeeper(&self, guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_world_entity()
            .guid()
            .find(&guid)
            .is_some_and(|e| e.npc_flags & lyracore_shared::constants::npc_flags::INNKEEPER != 0))
    }

    /// Read a vendor's stock for `SMSG_LIST_INVENTORY` (Tier 2 / vendors). Resolve the vendor's
    /// creature entry from its `game_world_entity` row, filter `game_npc_vendor` to that entry, and
    /// join each `game_item_template` for the display id / buy price / max durability (carrying the
    /// npc_vendor row's `max_count`). Read from the privileged cache (the coordinator bypasses RLS);
    /// the SDK exposes only the PK index, so iterate+filter like the other row queries. Items whose
    /// template isn't loaded are skipped (we can't price/display them). Ordered by the vendor `slot`.
    pub fn vendor_items(&self, vendor_guid: u64) -> Result<Vec<crate::codec::VendorItemView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        // The vendor's creature entry drives the stock lookup; a missing entity → no stock.
        let Some(creature_entry) = db
            .game_world_entity()
            .guid()
            .find(&vendor_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let mut rows: Vec<_> = db
            .game_npc_vendor()
            .iter()
            .filter(|v| v.creature_entry == creature_entry)
            .collect();
        rows.sort_by_key(|v| v.slot); // stable vendor-slot order (SQL has no ORDER BY in 2.5)
        let items = rows
            .into_iter()
            .filter_map(|v| {
                // Skip a stock line whose template isn't loaded — we can't price or display it.
                db.game_item_template()
                    .entry()
                    .find(&v.item_entry)
                    .map(|t| crate::codec::VendorItemView {
                        item_entry: v.item_entry,
                        display_id: t.display_id,
                        buy_price: t.buy_price,
                        max_durability: t.max_durability,
                        max_count: v.max_count,
                        buy_count: t.buy_count,
                    })
            })
            .collect();
        Ok(items)
    }

    /// Read a corpse's item loot for the loot window (items slice-4), joined with each item's
    /// template for the display id, then filtered PER VIEWER for `quest_only` rows (work-item 187
    /// slice 0 — fixes 210's recorded divergence: quest items are now per-looter, not gated on
    /// whoever got kill credit) AND group-loot rows (work-item 187 slices 2-4: a live NEED/GREED
    /// roll is withheld from EVERYONE; a round-robin/master-designated row is visible only to its
    /// designee). Read from the privileged cache (the coordinator bypasses RLS), filtered by
    /// `corpse_guid` (iterate+filter, like the other row queries). Returns `(slot, item_id, count,
    /// display_id)` triples for `build_loot_response_raw`. An item whose template isn't loaded falls
    /// back to display 0 (the client still resolves the name via query).
    pub fn corpse_loot(
        &self,
        corpse_guid: u64,
        viewer_guid: u64,
    ) -> Result<Vec<crate::codec::LootItemView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut items: Vec<crate::codec::LootItemView> = db
            .game_corpse_loot()
            .iter()
            .filter(|l| l.corpse_guid == corpse_guid)
            .filter(|l| {
                if l.quest_only {
                    let needs = viewer_needs_quest_item(db, viewer_guid, l.item_entry);
                    quest_row_visible_to_viewer(l.quest_only, l.reserved_for, viewer_guid, needs)
                } else {
                    group_loot_row_visible_to_viewer(
                        l.reserved_for,
                        l.withheld,
                        l.master_only,
                        l.designated_looter_guid,
                        viewer_guid,
                    )
                }
            })
            .map(|l| {
                let display_id = db
                    .game_item_template()
                    .entry()
                    .find(&l.item_entry)
                    .map(|t| t.display_id)
                    .unwrap_or(0);
                (l.slot, l.item_entry, l.count, display_id)
            })
            .collect();
        items.sort_by_key(|(slot, ..)| *slot); // stable loot-slot order (SQL has no ORDER BY in 2.5)
        Ok(items)
    }

    /// The account's bound 32-byte SpacetimeDB identity: the node-issued identity of the
    /// per-account player connection. `establish_session` writes this into `game_account.identity`
    /// so the player connection's later `player_login`/`movement_update` calls pass the module's
    /// `ctx.sender == owner` checks. Opening the connection here (at logon) caches it for reuse in
    /// the world phase.
    pub fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> {
        Ok(self.player_conn(account_id)?.identity.to_byte_array())
    }

    /// Read the shared session key K for the world handshake (Phase 2).
    pub fn session_key(&self, account_id: u64) -> Result<Option<[u8; 40]>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_session()
            .account_id()
            .find(&account_id)
            .and_then(|s| <[u8; 40]>::try_from(s.session_key).ok()))
    }

    /// Read a corpse's lootable copper for `SMSG_LOOT_RESPONSE` (slice 3) from the privileged cache.
    /// Returns 0 if the target is missing or not a corpse — the client only sends `CMSG_LOOT` on a
    /// lootable corpse, but we stay defensive (an empty loot window is harmless).
    pub fn loot_target_money(&self, target_guid: u64) -> Result<u32> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&target_guid)
            .filter(|e| e.dead)
            .map(|e| e.money)
            .unwrap_or(0))
    }

    /// Evaluate every quest a creature OR gameobject `giver_guid` starts/ends against `player_guid`,
    /// for the overhead status icon (`SMSG_QUESTGIVER_STATUS`) + the quest menu
    /// (`SMSG_QUESTGIVER_QUEST_LIST`). The giver is resolved EXACTLY like the module's
    /// `quest::validate_giver` fallback (work-item 041): a live `game_world_entity` first (a
    /// creature), else a spawned `game_gameobject` (GO 68 "Wanted Poster" starts q176 with NO creature
    /// giver at all; GO 55/56 "Lost Guards" corpses drive the q37/q45/q71 END chain) — never both, and
    /// never a live player (this reader doesn't special-case party-share givers, work-item 194; a
    /// player guid here matches the entity lookup (players share the table) but resolves to Giver::Creature(0) — a player entry is always 0, and no game_creature_quest row keys entry 0, so the result is empty, same as before). For each relation
    /// on the giver's entry (`game_creature_quest` or `game_gameobject_quest`): join its
    /// `game_quest_template`, look the quest up in the player's `game_character_quest` log (the
    /// coordinator bypasses RLS so it reads any player's log), and compute `startable` (a fresh,
    /// level-qualifying START), `active` (held + un-rewarded END), and `complete` (active + every
    /// objective's count met). One guard, all inline — no nested cache locks. The codec
    /// ([`crate::codec::quest_giver_status`]) folds these.
    pub fn quest_giver_evals(
        &self,
        giver_guid: u64,
        player_guid: u64,
    ) -> Result<Vec<crate::codec::GiverQuestEval>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        enum Giver {
            Creature(u32),
            GameObject(u32),
        }
        let giver = if let Some(e) = db.game_world_entity().guid().find(&giver_guid) {
            Giver::Creature(e.entry)
        } else if let Some(go) = db.game_gameobject().guid().find(&giver_guid) {
            Giver::GameObject(go.template_entry)
        } else {
            return Ok(Vec::new());
        };
        // No live player → no status (don't fabricate a level-0 phantom). A DEAD player (ghost) gets no
        // status either: the module rejects both accept AND turn-in while dead, so showing a `!`/`?` the
        // reducer would then reject is exactly the icon-vs-reducer mismatch we forbid.
        let Some(player) = db.game_world_entity().guid().find(&player_guid) else {
            return Ok(Vec::new());
        };
        if player.dead {
            return Ok(Vec::new());
        }
        let player_level = player.level;
        // Race/class packed in unit_bytes_0 (race | class<<8 | …), for the gates.
        let race = (player.unit_bytes_0 & 0xFF) as u8;
        let class = ((player.unit_bytes_0 >> 8) & 0xFF) as u8;
        // The player's quest log, guid-keyed (privileged read — RLS bypassed).
        let log: Vec<_> = db
            .game_character_quest()
            .iter()
            .filter(|q| q.character_guid == player_guid)
            .collect();
        let evals = match giver {
            Giver::Creature(entry) => db
                .game_creature_quest()
                .iter()
                .filter(|r| r.creature_entry == entry)
                .filter_map(|rel| {
                    eval_relation(
                        db,
                        rel.quest_entry,
                        rel.role,
                        &log,
                        player_guid,
                        player_level,
                        race,
                        class,
                    )
                })
                .collect(),
            Giver::GameObject(entry) => db
                .game_gameobject_quest()
                .iter()
                .filter(|r| r.go_entry == entry)
                .filter_map(|rel| {
                    eval_relation(
                        db,
                        rel.quest_entry,
                        rel.role,
                        &log,
                        player_guid,
                        player_level,
                        race,
                        class,
                    )
                })
                .collect(),
        };
        Ok(evals)
    }

    /// Build the detail view for one quest (the accept screen, the offer-reward screen, and the
    /// completion popup all read this): title, money, the RESOLVED XP reward (explicit, else the shared
    /// `lyracore_shared::quest::xp_reward` so it matches the module's grant), reward items joined with their
    /// display ids, and a synthesized objectives line per kill objective ("Creature slain: 0/N" from the
    /// creature-template name — quest body text isn't imported yet). `None` if the quest isn't loaded.
    pub fn quest_detail(&self, quest_id: u32) -> Result<Option<crate::codec::QuestDetailView>> {
        let guard = self.0.coord();
        Ok(quest_detail_view(&guard.conn.db, quest_id))
    }

    /// The player's active (un-rewarded) quests as quest-log descriptor slots (Phase 2: the L window).
    /// Deterministic slot assignment (sorted by quest_entry → slot 0..), capped at the 20 vanilla slots;
    /// each slot carries the quest id, per-objective counts, and a state byte (1 = all objectives met,
    /// else 0). The gateway encodes these into the `PLAYER_QUEST_LOG_*` fields. RLS-bypassed read.
    pub fn player_quest_log(
        &self,
        player_guid: u64,
    ) -> Result<Vec<crate::codec::update_mask::QuestLogSlot>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let quests: Vec<_> = db.game_character_quest().iter().collect();
        Ok(build_quest_log_slots(db, &quests, player_guid))
    }

    /// The player's LEARNED spells (#10) — `game_player_spell` rows for this character (the coordinator
    /// bypasses RLS so it reads any player's). Chained into the login spellbook so a taught ability
    /// (Auto Shot) reaches the client and `CastSpellByName` can fire it.
    pub fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let known: Vec<u32> = db
            .game_player_spell()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| s.spell_id)
            .collect();
        // 258 rank collapse: drop a known rank that another KNOWN spell supersedes (a game_spell_chain
        // row whose prev_spell is this id) — GATED on the same cmangos stacking rule as
        // superseded_old_rank (operator-corrected): MANA spells keep every rank in the book
        // (downranking Holy Light is a real thing); only non-mana/passive chains collapse
        // (Heroic Strike). One pass suffices: each superseded rank is prev of its own successor.
        let known_set: std::collections::HashSet<u32> = known.iter().copied().collect();
        let superseded: std::collections::HashSet<u32> = db
            .game_spell_chain()
            .iter()
            .filter(|c| {
                c.prev_spell != 0
                    && known_set.contains(&c.prev_spell)
                    && known_set.contains(&c.spell_id)
                    && !spell_ranks_stack_in_book(db, c.spell_id)
            })
            .map(|c| c.prev_spell)
            .collect();
        Ok(known
            .into_iter()
            .filter(|id| !superseded.contains(id))
            .collect())
    }

    /// The player's IMPORTED action-bar rows (work-item 212) as `(button, action, action_type)` triples —
    /// `game_player_action` rows copied at character creation from `game_createinfo_action` (empty when
    /// no dump has been imported, the common case today). Chained into the login codec
    /// (`login_sequence_messages`), which builds the bar from these when non-empty and falls back to
    /// the spellbook synth otherwise. RLS-bypassed read, like `player_learned_spells`.
    pub fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_action()
            .iter()
            .filter(|a| a.character_guid == player_guid)
            .map(|a| (a.button, a.action, a.action_type))
            .collect())
    }

    /// The player's persisted reputation standings (#13 slice 2) as `(reputation_index, standing,
    /// at_war)` triples — chained into the login `SMSG_INITIALIZE_FACTIONS` so a relog carries the
    /// real standing + the At-War checkbox (195 slice B) instead of the all-neutral stub. Rows with
    /// `reputation_index < 0` (stale pre-migration filler) are skipped — there is no slot to
    /// address. RLS-bypassed read, like `player_learned_spells`.
    pub fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_reputation()
            .iter()
            .filter(|r| r.character_guid == player_guid && r.reputation_index >= 0)
            .map(|r| (r.reputation_index, r.standing, r.at_war))
            .collect())
    }

    /// Does `npc_guid` REFUSE to interact with `player_guid` (195 slice A — vanilla
    /// `Unit::GetReactionTo` for the gossip/vendor/trainer/questgiver windows)? The NPC's
    /// faction_template resolves to its parent faction:
    /// - parent has a REP BAR (`reputation_index >= 0`) → reaction = the player's standing RANK
    ///   (their `game_player_reputation` row, else the faction's `base_standing`); refuse at
    ///   Unfriendly (2) or below — Neutral+ interacts.
    /// - no rep bar → FactionTemplate mask fallback: refuse when the NPC is HOSTILE to the player.
    ///
    /// Missing data anywhere → do NOT refuse (fail-open: an unfactioned fixture NPC keeps working).
    pub fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let entities = db.game_world_entity();
        let (Some(npc), Some(player)) = (
            entities.guid().find(&npc_guid),
            entities.guid().find(&player_guid),
        ) else {
            return Ok(false);
        };
        let templates = db.game_faction_template();
        let Some(npc_ft) = templates.id().find(&npc.faction_template) else {
            return Ok(false);
        };
        if let Some(parent) = db.game_faction().faction_id().find(&npc_ft.faction) {
            if parent.reputation_index >= 0 {
                let row = db
                    .game_player_reputation()
                    .iter()
                    .find(|r| r.character_guid == player_guid && r.faction_id == parent.faction_id);
                // 195 slice B: the At-War checkbox forces hostile regardless of standing (vanilla).
                if row.as_ref().map(|r| r.at_war).unwrap_or(false) {
                    return Ok(true);
                }
                let standing = row.map(|r| r.standing).unwrap_or(parent.base_standing);
                return Ok(reputation_rank(standing) <= RANK_UNFRIENDLY);
            }
        }
        let Some(player_ft) = templates.id().find(&player.faction_template) else {
            return Ok(false);
        };
        Ok(faction_template_hostile(&npc_ft, &player_ft))
    }

    /// The spells trainer `trainer_guid` teaches, folded with `player_guid`'s level + known-state so the
    /// codec can render each Green/Red/Gray. The trainer's creature-template `entry` keys the list (like
    /// the vendor stock); `known` = a `game_player_spell` row (the one castability source). A missing
    /// trainer/player → empty list. Read from the privileged cache (coordinator bypasses RLS).
    pub fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<crate::codec::TrainerSpellView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(trainer_entry) = db
            .game_world_entity()
            .guid()
            .find(&trainer_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let Some(player) = db.game_world_entity().guid().find(&player_guid) else {
            return Ok(Vec::new());
        };
        let learned: std::collections::HashSet<u32> = db
            .game_player_spell()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| s.spell_id)
            .collect();
        // The player's trained skill caps, by line — for the per-row "known" of a PROFESSION offering
        // (game_player_spell never holds a profession id, so we gray a tier on the skill cap, not the
        // spellbook). RLS-bypassed via the coordinator cache, same as the reads above.
        let skill_caps: std::collections::HashMap<u32, u32> = db
            .game_player_skill()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| (s.skill_line, s.max_rank as u32))
            .collect();
        let mut rows: Vec<_> = db
            .game_trainer_spell()
            .iter()
            // Profession-learn offerings (learn_skill_line != 0) are now SHOWN: the importer synthesizes
            // them with REAL Spell.dbc learn ids (2575 Mining, 2366 Herbalism, …), so the 5875 client
            // resolves the name/icon + emits a valid SMSG_LEARNED_SPELL — unlike the old 50080-88 markers
            // this filter used to hide. The buy still routes to crate::skill::learn_profession (module
            // unchanged). The per-row `known` below grays tiers the player has already trained past.
            .filter(|t| t.trainer_entry == trainer_entry)
            .collect();
        rows.sort_by_key(|t| t.spell_id); // stable order (SQL has no ORDER BY in 2.5)
                                          // A class-spell offering id is a LearnSpell WRAPPER (1873 teaches 639); game_player_spell holds
                                          // the RESOLVED rank, so `known` must compare the resolved id or every known spell reads Green.
                                          // Same first-qualifying-trigger rule as resolve_learn_target (can't call it: coord() re-lock);
                                          // one pass over the effect table for the whole list instead of a scan per row.
        const EXCLUDED: [u8; 4] = [0x93, 0xBE, 0xAB, 0x05];
        let wrapper_ids: std::collections::HashSet<u32> = rows
            .iter()
            .filter(|t| t.learn_skill_line == 0)
            .map(|t| t.spell_id)
            .collect();
        let mut resolved: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for e in db.game_spell_effect().iter() {
            if wrapper_ids.contains(&e.spell_id)
                && e.trigger_spell != 0
                && !EXCLUDED.contains(&e.kind)
            {
                resolved.entry(e.spell_id).or_insert(e.trigger_spell);
            }
        }
        Ok(rows
            .into_iter()
            .map(|t| crate::codec::TrainerSpellView {
                spell_id: t.spell_id,
                cost: t.cost,
                required_level: t.required_level,
                player_level: player.level,
                // PROFESSION row: "known" iff the player's skill cap for this line already meets the
                // offering's cap (mirrors module trainer_buy_check's gate → the client grays the tiers
                // already trained past). CLASS-SPELL row (line 0): known iff a game_player_spell row
                // for the RESOLVED rank behind the wrapper (mirrors the module buy gate's knows_spell).
                known: if t.learn_skill_line != 0 {
                    skill_caps
                        .get(&t.learn_skill_line)
                        .is_some_and(|&c| c >= t.learn_skill_cap)
                } else {
                    learned.contains(resolved.get(&t.spell_id).unwrap_or(&t.spell_id))
                },
                profession: t.learn_skill_line != 0,
            })
            .collect())
    }

    /// Find `owner_guid`'s corpse location `(map_id, x, y, z)` from the privileged cache, for the
    /// `MSG_CORPSE_QUERY` reply (slice 5). `None` if they have no corpse.
    pub fn corpse_location(&self, owner_guid: u64) -> Result<Option<(u32, f32, f32, f32)>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_corpse()
            .iter()
            .find(|c| c.owner_guid == owner_guid)
            .map(|c| (c.map_id, c.x, c.y, c.z)))
    }

    /// All currently-online player characters for `CMSG_WHO → SMSG_WHO`. Iterates
    /// `game_world_entity` for entries with `entry == 0` (player entities; creatures have a
    /// non-zero entry), then joins each against `game_character` for name/race/class/zone. The
    /// coordinator bypasses RLS so it sees every player's entity regardless of the caller's scope.
    pub fn online_players(&self) -> Result<Vec<crate::codec::WhoPlayerView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let views = db
            .game_world_entity()
            .iter()
            .filter(|e| e.entry == 0) // players have entry == 0; creatures have a template entry
            .filter_map(|e| {
                let ch = db.game_character().guid().find(&e.guid)?;
                Some(crate::codec::WhoPlayerView {
                    name: ch.name.clone(),
                    level: ch.level,
                    class: ch.class,
                    race: ch.race,
                    zone_id: ch.zone_id,
                })
            })
            .collect();
        Ok(views)
    }

    /// Resolve a typed contact name to a character guid (case-insensitive, mirroring the module's own
    /// `send_whisper` name match) via the privileged cache — the same RLS-bypass trick `online_players`
    /// uses. `None` if no character has that name.
    pub fn character_guid_by_name(&self, name: &str) -> Result<Option<u64>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .map(|c| c.guid))
    }

    /// The party `character_guid` belongs to, read from THIS handle's database (#22, group slice).
    ///
    /// Which database the handle points at is the whole meaning of the answer: on the **realm-core**
    /// handle this is the AUTHORITATIVE roster, and on a world shard it is that shard's mirror of it
    /// (`group::sync_group_mirror`). Nothing here knows or cares which — routing is the caller's job,
    /// exactly as it is for every other read in this file.
    ///
    /// A cache read, so it is cheap enough to run inside an SDK callback (which the realm-core group
    /// relay does): no reducer call, no round trip.
    pub fn group_roster(&self, character_guid: u64) -> Option<crate::world::party::GroupRoster> {
        let group_id = {
            let guard = self.0.coord();
            let found = guard
                .conn
                .db
                .game_group_member()
                .iter()
                .find(|m| m.character_guid == character_guid);
            found?.group_id
        };
        self.group_roster_by_id(group_id)
    }

    /// [`group_roster`](Self::group_roster) keyed by the group itself — the read the mirror push
    /// needs for a party the acting character has just LEFT (their own membership row is gone, but
    /// the remaining members' rows still have to reach every shard).
    ///
    /// Members come back in join order (member-row id), which is the order leadership succession
    /// uses (`group::leader_after_removal`) and therefore the order the party frame should render.
    pub fn group_roster_by_id(&self, group_id: u64) -> Option<crate::world::party::GroupRoster> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let group = db.game_group().iter().find(|g| g.group_id == group_id)?;
        let mut rows: Vec<(u64, u64)> = db
            .game_group_member()
            .iter()
            .filter(|m| m.group_id == group_id)
            .map(|m| (m.id, m.character_guid))
            .collect();
        rows.sort_unstable();
        Some(crate::world::party::GroupRoster {
            group_id,
            leader_guid: group.leader_guid,
            loot_method: group.loot_method,
            loot_threshold: group.loot_threshold,
            master_looter_guid: group.master_looter_guid,
            members: rows.into_iter().map(|(_, guid)| guid).collect(),
        })
    }

    /// Every UNRESOLVED `game_loot_roll` row on THIS handle's database, joined with its votes (#50).
    ///
    /// A cache read, like [`group_roster`](Self::group_roster) — this table is subscribed on every
    /// connection now (issue #50 extends the coordinator subscription list), so no reducer call is
    /// needed. Meaningful only on a WORLD SHARD in a sharded deployment: realm-core never has a row
    /// here that a world shard wrote (only its own `realm_loot_op` START arm inserts there), and on
    /// an unsharded gateway the relay that calls this never runs at all (`realm_store()` is `None`).
    pub fn pending_local_rolls(&self) -> Result<Vec<crate::world::loot::PendingLootRoll>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_loot_roll()
            .iter()
            .filter(|r| !r.resolved)
            .map(|r| {
                let recipients = db
                    .game_loot_roll_vote()
                    .iter()
                    .filter(|v| v.roll_id == r.id)
                    .map(|v| v.voter_guid)
                    .collect();
                crate::world::loot::PendingLootRoll {
                    roll_id: r.id,
                    corpse_guid: r.corpse_guid,
                    slot: r.slot,
                    item_entry: r.item_entry,
                    deadline_micros: r.deadline_micros,
                    recipients,
                }
            })
            .collect())
    }

    // The return tuple is the poll result: the new watermark plus the `(corpse, slot, winner)` triples read since the old one.
    #[allow(clippy::type_complexity)]
    /// Every `ROLL_WON` `game_group_event` row on THIS handle's database with `id > after_id` (#50),
    /// decoded to `(corpse_guid, slot, winner_guid)`, plus the new high-water mark (the max id seen,
    /// or `after_id` unchanged if none). Meaningful on the **realm-core** handle: that is the only
    /// database `resolve_roll`/`force_resolve_rolls_for_disband` push a `ROLL_WON` event on in a
    /// sharded deployment (voting is routed there exclusively — `world::loot::run_vote`).
    ///
    /// An unparseable payload is skipped + logged rather than failing the whole scan — the module
    /// writes this grammar, so a decode failure here means the two crates' `lyracore_shared::loot_roll`
    /// copies have drifted, not that this event is meaningless.
    pub fn loot_won_since(&self, after_id: u64) -> Result<(u64, Vec<(u64, u8, u64)>)> {
        use lyracore_shared::loot_roll::event_kind as roll_kind;
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut watermark = after_id;
        let mut wins = Vec::new();
        for row in db.game_group_event().iter() {
            if row.id <= after_id || row.kind != roll_kind::ROLL_WON {
                continue;
            }
            watermark = watermark.max(row.id);
            match lyracore_shared::loot_roll::decode_won(&row.payload) {
                Some((corpse_guid, slot, ..)) => wins.push((corpse_guid, slot, row.other_guid)),
                None => log::warn!(
                    "loot-roll relay: unparseable ROLL_WON payload {:?} (event {})",
                    row.payload,
                    row.id
                ),
            }
        }
        Ok((watermark, wins))
    }

    /// A character's live presence `(online, level, class, zone_id)` for `SMSG_FRIEND_STATUS`/
    /// `SMSG_FRIEND_LIST`. `None` if the guid doesn't resolve to any character (a stale reference).
    pub fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&guid)
            .map(|c| (c.online, c.level, c.class, c.zone_id)))
    }

    /// `owner_guid`'s friend list + ignore list for `CMSG_FRIEND_LIST → SMSG_FRIEND_LIST` +
    /// `SMSG_IGNORE_LIST` (work-item 130). Reads `game_character_contact` via the privileged cache
    /// (RLS-bypassed, same trick `online_players` uses) so an online friend's presence resolves
    /// regardless of whose connection is asking. A friend whose character has since been deleted
    /// (stale row, pre-sweep or a race) degrades to an offline/zero row rather than erroring.
    pub fn contact_lists(
        &self,
        owner_guid: u64,
    ) -> Result<(Vec<crate::codec::FriendView>, Vec<u64>)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut friends = Vec::new();
        let mut ignored = Vec::new();
        for c in db
            .game_character_contact()
            .iter()
            .filter(|c| c.owner_guid == owner_guid)
        {
            if c.is_ignore {
                ignored.push(c.target_guid);
            } else {
                let (online, level, class, zone_id) = db
                    .game_character()
                    .guid()
                    .find(&c.target_guid)
                    .map(|ch| (ch.online, ch.level, ch.class, ch.zone_id))
                    .unwrap_or((false, 0, 0, 0));
                friends.push(crate::codec::FriendView {
                    guid: c.target_guid,
                    online,
                    level,
                    class,
                    zone_id,
                });
            }
        }
        Ok((friends, ignored))
    }

    /// Look up the static `Talent` metadata for `talent_id` (from the coordinator's `game_talent` cache).
    /// Returns `None` if the talent isn't seeded (the dispatch treats an unknown talent as a noop).
    pub fn talent_by_id(&self, talent_id: u32) -> Option<Talent> {
        self.0
            .coord()
            .conn
            .db
            .game_talent()
            .iter()
            .find(|t| t.talent_id == talent_id)
    }

    /// Sum of the character's spent talent ranks (`game_character_talent`, coordinator RLS-bypassed).
    /// Non-zero gates the post-CREATE login correction of `PLAYER_CHARACTER_POINTS1` (the CREATE's
    /// formula counts points EARNED only — see `codec/entity.rs`).
    pub fn talent_points_spent(&self, character_guid: u64) -> u32 {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_character_talent()
            .iter()
            .filter(|t| t.character_guid == character_guid)
            .map(|t| t.rank as u32)
            .sum()
    }

    /// Talent-pane sync data read AFTER a successful `learn_talent` (the blocking reducer call
    /// returns with the cache already consistent): `(teach_spell, superseded_prev, points_remaining)`.
    /// `teach_spell` = the rank-spell the module just put in the spellbook — the 1.12 TalentFrame
    /// derives a talent's shown rank from which rank-spell is KNOWN, so this must relay live as
    /// SMSG_LEARNED_SPELL or the pane freezes until relog; 0 when the module taught nothing (mirror
    /// of `talent::apply_talent_rank`'s game_spell-existence gate). `superseded_prev` = the previous
    /// rank's now-replaced spell (drives SMSG_SUPERCEDED_SPELL; 0 for rank 1 / same-spell demo
    /// trees). `points_remaining` = earned (level−9, floor 0) minus spent — the live
    /// `PLAYER_CHARACTER_POINTS1` value. Callers may pass `talent_id = 0` to get just the points.
    pub fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32) {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let level = db
            .game_world_entity()
            .guid()
            .find(&character_guid)
            .map(|e| e.level)
            .unwrap_or(0);
        let spent: u32 = db
            .game_character_talent()
            .iter()
            .filter(|t| t.character_guid == character_guid)
            .map(|t| t.rank as u32)
            .sum();
        let remaining = (level as i32 - 9).max(0) as u32;
        let remaining = remaining.saturating_sub(spent);
        let Some(t) = db.game_talent().iter().find(|t| t.talent_id == talent_id) else {
            return (0, 0, remaining);
        };
        let rank = db
            .game_character_talent()
            .iter()
            .find(|r| r.character_guid == character_guid && r.talent_id == talent_id)
            .map(|r| r.rank)
            .unwrap_or(0);
        let new_spell = pick_rank_spell(rank, &t);
        let prev_spell = if rank >= 2 {
            pick_rank_spell(rank - 1, &t)
        } else {
            0
        };
        // Mirror the module's teach gate: apply_talent_rank only puts the rank-spell in the book
        // when it exists in game_spell (an unimported rank-spell is skipped there — sending
        // LEARNED for it would desync the client book from the server).
        let teach = if new_spell != 0 && db.game_spell().spell_id().find(&new_spell).is_some() {
            new_spell
        } else {
            0
        };
        let superseded = if teach != 0 && prev_spell != 0 && prev_spell != teach {
            prev_spell
        } else {
            0
        };
        (teach, superseded, remaining)
    }
}

/// Mirror of the module's `talent::pick_rank_spell`: rank N's spell from the per-rank columns
/// (`rank_spell_2..5`), a 0 column falling back to `spell_id` (the demo tree scales ONE spell by
/// rank; the imported tree has a distinct spell per rank).
fn pick_rank_spell(rank: u8, t: &Talent) -> u32 {
    let s = match rank {
        2 => t.rank_spell_2,
        3 => t.rank_spell_3,
        4 => t.rank_spell_4,
        5 => t.rank_spell_5,
        _ => t.spell_id,
    };
    if s != 0 {
        s
    } else {
        t.spell_id
    }
}

// --- quest read helpers (free fns over the cache, shared by the methods above) ---------------------
// NOTE: game_creature_quest / game_quest_objective / game_quest_reward_item / game_character_quest only
// expose their `id` PK index via the SDK, so these scan with iter().filter() (not a secondary-index
// find). game_quest_template / game_quest_text / game_item_template / game_creature_template DO have a
// unique-PK index, so those use .entry()/.quest_entry().find() (point lookups).

/// Evaluate one giver↔quest relation against the player → a [`GiverQuestEval`], or `None` if the quest
/// template isn't loaded. PARITY: the `startable` predicate MUST stay identical to the module's
/// `apply_accept_quest` accept gate (`module/src/quest.rs`) — a fresh START relation with level + race +
/// class met and the prerequisite quest turned in — else the `!` icon shows a quest the reducer rejects.
/// Sum a player's held quantity of item `entry` over `game_item_instance` (the coordinator reads any
/// player's items — RLS-bypassed, like the quest log). The gateway twin of the module's `items::item_count`.
/// cmangos `canStackSpellRanksInSpellBook` (258): MANA spells (power_type 0) keep EVERY rank
/// visible in the client book — downranking is a real caster mechanic; PASSIVES and
/// non-mana-cost spells (rage/energy/health) SUPERSEDE their old rank. Unknown spell → stack
/// (never hide a rank on missing data). LOCKSTEP: SPELL_ATTR_PASSIVE = 0x40 (module taxonomy).
fn spell_ranks_stack_in_book(db: &RemoteTables, spell_id: u32) -> bool {
    const SPELL_ATTR_PASSIVE: u32 = 0x40;
    const POWER_MANA: u8 = 0;
    match db.game_spell().spell_id().find(&spell_id) {
        Some(h) => h.attributes & SPELL_ATTR_PASSIVE == 0 && h.power_type == POWER_MANA,
        None => true,
    }
}

fn player_item_count(db: &RemoteTables, owner_guid: u64, entry: u32) -> u32 {
    db.game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == owner_guid && i.entry == entry)
        .map(|i| i.stack_count)
        .sum()
}

/// Work-item 187 slice 0: does `viewer_guid` currently need quest item `item_entry`? The gateway twin
/// of the module's `loot::killer_needs_item`/`needs_item_pure` (an ACTIVE — un-rewarded — quest with a
/// `COLLECT_ITEM` objective naming `item_entry`, held < required), applied to the VIEWER opening the
/// loot window rather than the credited killer. RLS-bypassed read (coordinator), same shape as
/// `quest_objectives_complete`/`player_item_count` above.
fn viewer_needs_quest_item(db: &RemoteTables, viewer_guid: u64, item_entry: u32) -> bool {
    const COLLECT_ITEM: u8 = 1; // == module objective_kind::COLLECT_ITEM
    let active: Vec<u32> = db
        .game_character_quest()
        .iter()
        .filter(|q| q.character_guid == viewer_guid && !q.rewarded)
        .map(|q| q.quest_entry)
        .collect();
    if active.is_empty() {
        return false;
    }
    db.game_quest_objective().iter().any(|o| {
        o.kind == COLLECT_ITEM
            && o.target_entry == item_entry
            && active.contains(&o.quest_entry)
            && player_item_count(db, viewer_guid, item_entry) < o.required_count.max(1)
    })
}

/// Work-item 187 slice 0 — the per-viewer loot-window visibility gate: is a `game_corpse_loot` row
/// visible to `viewer_guid`? A non-quest row is always visible (FFA, unconditional — byte-identical to
/// before this slice). A `quest_only` row is visible when EITHER it's still the UNRESERVED shared row
/// (`reserved_for == 0` — nobody has split it yet) and the viewer currently needs the item
/// (`viewer_needs_item`, resolved by the caller — mirrors the module's ctx/pure split so this decision
/// needs no live cache to unit-test), OR it's the viewer's OWN already-split reserved clone
/// (`reserved_for == viewer_guid`) — a clone reserved for someone ELSE is invisible regardless of need.
/// Mirrored module-side by `loot::quest_take_allowed` (same predicate shape) so a row a viewer's
/// window shows is always a row their take can actually succeed on. Pure.
pub(crate) fn quest_row_visible_to_viewer(
    quest_only: bool,
    reserved_for: u64,
    viewer_guid: u64,
    viewer_needs_item: bool,
) -> bool {
    if !quest_only {
        return true;
    }
    if reserved_for == viewer_guid {
        return true;
    }
    reserved_for == 0 && viewer_needs_item
}

/// Work-item 187 slices 2-4 — the per-viewer loot-window visibility gate for a NON-quest row (the
/// caller only calls this when `!quest_only`; `quest_row_visible_to_viewer` handles quest rows,
/// unchanged, above). `reserved_for` is GENERALIZED here beyond the quest clone: nonzero also covers
/// a NEED/GREED winner's inventory-full fallback row (`resolve_roll`'s module doc) — either way,
/// nonzero means "visible ONLY to that guid", full stop. A `withheld` row (a live roll in progress)
/// is invisible to EVERYONE (not just non-eligible viewers — matches the design's "withheld rows
/// invisible in EVERYONE's window until resolved", proving the AutoLoot-safety trap: a grey +
/// green drop on one corpse has the grey's row visible/FFA while the green's is withheld). A
/// `master_only` row is never shown via the plain window (the master acts through
/// `SMSG_LOOT_MASTER_LIST`/`CMSG_LOOT_MASTER_GIVE` instead). A `designated_looter_guid` row (round-
/// robin/below-threshold) is visible only to its designee. The all-zero/false baseline is plain FFA
/// (byte-identical to pre-187 behavior). Mirrored module-side by `loot::group_loot_take_allowed`
/// (same predicate shape) so a row a viewer's window shows is always a row their take can succeed
/// on. Pure.
pub(crate) fn group_loot_row_visible_to_viewer(
    reserved_for: u64,
    withheld: bool,
    master_only: bool,
    designated_looter_guid: u64,
    viewer_guid: u64,
) -> bool {
    if reserved_for != 0 {
        return reserved_for == viewer_guid;
    }
    if withheld || master_only {
        return false;
    }
    designated_looter_guid == 0 || designated_looter_guid == viewer_guid
}

/// Is every objective of `quest_entry` met for this player? MIRRORS the module's `quest_is_complete`
/// (module/src/quest.rs): a COLLECT_ITEM objective reads LIVE INVENTORY (its progress count is never bumped —
/// completion follows the bag), every other kind reads the per-objective progress count. THE FIX for
/// collect-quest turn-in: the gateway used to read `counts` ONLY, so the 121-of-174 collect objectives always
/// showed incomplete → the client's "Complete Quest" button stayed disabled and the zone's quest backbone was
/// un-turn-in-able. A quest with no objectives is trivially complete (a pure talk-to-giver quest).
fn quest_objectives_complete(
    db: &RemoteTables,
    quest_entry: u32,
    logrow: Option<&CharacterQuest>,
    owner_guid: u64,
) -> bool {
    const COLLECT_ITEM: u8 = 1; // == module objective_kind::COLLECT_ITEM
    db.game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_entry)
        .all(|o| {
            let have = if o.kind == COLLECT_ITEM {
                player_item_count(db, owner_guid, o.target_entry)
            } else {
                logrow
                    .and_then(|q| q.counts.get(o.obj_index as usize).copied())
                    .unwrap_or(0)
            };
            have >= o.required_count
        })
}

/// Build one quest's `QuestDetailView` off `db` (any connection's cache carrying the public quest
/// tables — the privileged coordinator cache via `Coordinator::quest_detail`, OR a per-player
/// subscription's own cache when it's subscribed to them too). `None` if the quest isn't loaded.
/// Extracted from `Coordinator::quest_detail` (work-item 194) so the QUEST_SHARE group-event relay
/// (`subscriptions.rs`) can build the SAME view over a cloned `Coordinator` handle without
/// duplicating this join logic — one chokepoint, not two copies to keep in sync.
pub(crate) fn quest_detail_view(
    db: &RemoteTables,
    quest_id: u32,
) -> Option<crate::codec::QuestDetailView> {
    use crate::codec::QuestDetailView;
    let tmpl = db.game_quest_template().entry().find(&quest_id)?;
    // MUST mirror module/src/quest.rs's award resolution exactly (explicit override → authentic
    // RewMoneyMaxLevel/0.6 → xp_reward placeholder) so the completion popup equals the grant. No
    // over-level penalty on either side yet (deferred — add to both together with the player's level).
    let reward_xp = if tmpl.reward_xp > 0 {
        tmpl.reward_xp
    } else if tmpl.reward_money_max_level > 0 {
        lyracore_shared::quest::quest_xp(tmpl.reward_money_max_level)
    } else {
        lyracore_shared::quest::xp_reward(tmpl.quest_level)
    };
    // Body text from the side table (cmangos Details/Objectives/OfferReward/RequestItems).
    let text = db.game_quest_text().quest_entry().find(&quest_id);
    let details = text.as_ref().map(|t| t.details.clone()).unwrap_or_default();
    let offer_reward_text = text
        .as_ref()
        .map(|t| t.offer_reward_text.clone())
        .unwrap_or_default();
    let request_items_text = text
        .as_ref()
        .map(|t| t.request_items_text.clone())
        .unwrap_or_default();
    let imported_objectives = text.map(|t| t.objectives).unwrap_or_default();
    // All objectives in obj_index order → wire (creature_or_go, count, req_item, req_item_count):
    //   KILL_CREATURE(0): (+creature_entry, count, 0, 0)
    //   COLLECT_ITEM(1):  (0, 0, item_entry, count)
    //   USE_GAMEOBJECT(2): (negative go_entry as u32, count, 0, 0) — client checks sign to tell apart
    //   EXPLORE_AREATRIGGER(3): no wire slot (area triggers have no entry in the objectives array)
    let mut raw_objs: Vec<(u8, u32, u32, u32, u32)> = db
        .game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_id)
        .filter_map(|o| {
            let slot = match o.kind {
                0 => (o.target_entry, o.required_count, 0u32, 0u32),
                1 => (0, 0, o.target_entry, o.required_count),
                2 => ((-(o.target_entry as i32)) as u32, o.required_count, 0, 0),
                _ => return None, // EXPLORE_AREATRIGGER and unknown: no wire slot
            };
            Some((o.obj_index, slot.0, slot.1, slot.2, slot.3))
        })
        .collect();
    raw_objs.sort_by_key(|(i, _, _, _, _)| *i);
    Some(QuestDetailView {
        quest_id,
        quest_level: tmpl.quest_level,
        zone_or_sort: tmpl.zone_or_sort,
        title: tmpl.title.clone(),
        details,
        objectives_text: synthesized_objectives(db, quest_id, imported_objectives),
        offer_reward_text,
        request_items_text,
        money_reward: tmpl.reward_money,
        reward_xp,
        // Work-item 194: threaded from the template — chains (successor auto-offer) + the level-cap
        // payout preview (always computed; the client only surfaces it when the VIEWER is capped).
        next_quest_id: tmpl.next_quest_id,
        max_level_money_reward: lyracore_shared::quest::max_level_money_reward(reward_xp),
        rewards: quest_rewards(db, quest_id),
        choice_rewards: quest_choice_rewards(db, quest_id),
        objectives: raw_objs
            .into_iter()
            .map(|(_, a, b, c, d)| (a, b, c, d))
            .collect(),
    })
}

// One argument per column of the condition row being evaluated; the SDK row type is not in scope on this side of the read.
#[allow(clippy::too_many_arguments)]
/// Evaluate ONE giver relation (a `game_creature_quest` OR `game_gameobject_quest` row, already
/// filtered to the giver's entry by the caller) against `player_guid`'s quest log. Takes the bare
/// `quest_entry`/`role` rather than a table row type — `game_creature_quest` and `game_gameobject_quest`
/// have the identical `(quest_entry, role)` shape (work-item 041), so `quest_giver_evals` calls this
/// same function for both, keyed only on which relation table it iterated.
fn eval_relation(
    db: &RemoteTables,
    quest_entry: u32,
    role: u8,
    log: &[CharacterQuest],
    player_guid: u64,
    player_level: u32,
    race: u8,
    class: u8,
) -> Option<crate::codec::GiverQuestEval> {
    use crate::codec::{GiverQuestEval, ROLE_START};
    let tmpl = db.game_quest_template().entry().find(&quest_entry)?;
    let logrow = log.iter().find(|q| q.quest_entry == quest_entry);
    let active = logrow.map(|q| !q.rewarded).unwrap_or(false);
    // Complete = active AND every objective met — INVENTORY-AWARE for COLLECT_ITEM (see the helper).
    let complete = active && quest_objectives_complete(db, quest_entry, logrow, player_guid);
    // A repeatable quest already rewarded keeps its CharacterQuest row (module/src/quest.rs no longer
    // deletes it — deleting would erase the only record it was ever completed, breaking any OTHER quest's
    // `prev_quest_id` check below), so "no row" is not the right re-offer test for it: a rewarded row on a
    // repeatable quest is startable again, mirroring apply_accept_quest's duplicate-guard exception.
    let no_active_or_done =
        logrow.is_none() || (tmpl.repeatable && logrow.is_some_and(|q| q.rewarded));
    let startable = role == ROLE_START
        && no_active_or_done
        && player_level >= tmpl.min_level
        && lyracore_shared::quest::race_allowed(tmpl.required_races, race)
        && lyracore_shared::quest::class_allowed(tmpl.required_classes, class)
        && (tmpl.prev_quest_id == 0
            || log
                .iter()
                .any(|q| q.quest_entry == tmpl.prev_quest_id && q.rewarded));
    Some(GiverQuestEval {
        quest_id: tmpl.entry,
        title: tmpl.title.clone(),
        level: tmpl.quest_level,
        role,
        startable,
        active,
        complete,
    })
}

/// Build the player's quest-log descriptor slots from raw quest + objective rows. Shared by the login
/// read ([`Coordinator::player_quest_log`], over the privileged coordinator cache) and the live relay
/// (`subscriptions.rs`, over the player connection's own RLS-scoped cache) so both produce identical
/// slot assignments. Active (un-rewarded) quests for `player_guid`, sorted by quest_entry → slot 0..,
/// capped at the 20 vanilla slots; `state` is 1 when every objective is met (INVENTORY-AWARE for
/// COLLECT_ITEM, via `quest_objectives_complete` — so the log shows "(Complete)" for a collect quest),
/// else 0. Reads `db` (game_quest_objective + game_item_instance); both callers' caches see this player.
pub(crate) fn build_quest_log_slots(
    db: &RemoteTables,
    quests: &[CharacterQuest],
    player_guid: u64,
) -> Vec<crate::codec::update_mask::QuestLogSlot> {
    let mut active: Vec<&CharacterQuest> = quests
        .iter()
        .filter(|q| q.character_guid == player_guid && !q.rewarded)
        .collect();
    active.sort_by_key(|q| q.quest_entry); // deterministic slot order
    active
        .into_iter()
        .take(crate::codec::update_mask::idx::QUEST_LOG_SLOTS as usize)
        .enumerate()
        .map(|(i, q)| {
            let complete = quest_objectives_complete(db, q.quest_entry, Some(q), player_guid);
            crate::codec::update_mask::QuestLogSlot {
                slot: i as u8,
                quest_id: q.quest_entry,
                counts: q.counts.clone(),
                state: u8::from(complete), // 1 = complete, 0 = incomplete
                timer: 0,
            }
        })
        .collect()
}

/// A quest's guaranteed reward items joined with each item's `display_id` (0 if the template isn't loaded).
fn quest_rewards(db: &RemoteTables, quest_id: u32) -> Vec<crate::codec::QuestRewardView> {
    db.game_quest_reward_item()
        .iter()
        .filter(|r| r.quest_entry == quest_id)
        .map(|r| {
            let display_id = db
                .game_item_template()
                .entry()
                .find(&r.item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            crate::codec::QuestRewardView {
                item_entry: r.item_entry,
                count: r.count,
                display_id,
            }
        })
        .collect()
}

/// A quest's CHOICE reward items (pick-1-of-N), joined to each item's `display_id` (0 if the template
/// isn't loaded) and ORDERED BY `choice_index` so the wire position matches the index the client sends
/// back as `CMSG_QUESTGIVER_CHOOSE_REWARD.reward`.
fn quest_choice_rewards(db: &RemoteTables, quest_id: u32) -> Vec<crate::codec::QuestRewardView> {
    let mut rows: Vec<_> = db
        .game_quest_reward_choice()
        .iter()
        .filter(|r| r.quest_entry == quest_id)
        .collect();
    rows.sort_by_key(|r| r.choice_index);
    rows.into_iter()
        .map(|r| {
            let display_id = db
                .game_item_template()
                .entry()
                .find(&r.item_entry)
                .map(|t| t.display_id)
                .unwrap_or(0);
            crate::codec::QuestRewardView {
                item_entry: r.item_entry,
                count: r.count,
                display_id,
            }
        })
        .collect()
}

/// The objectives text for the detail window: the imported cmangos `Objectives` if present, else a
/// synthesized line per objective (kind-aware: kill = "Name slain: 0/N", collect = "Name: 0/N").
fn synthesized_objectives(db: &RemoteTables, quest_id: u32, imported: String) -> String {
    if !imported.trim().is_empty() {
        return imported;
    }
    let mut objs: Vec<(u8, String)> = db
        .game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_id)
        .map(|o| {
            let text = match o.kind {
                0 => {
                    // KILL_CREATURE
                    let name = db
                        .game_creature_template()
                        .entry()
                        .find(&o.target_entry)
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    format!("{name} slain: 0/{}", o.required_count)
                }
                1 => {
                    // COLLECT_ITEM
                    let name = db
                        .game_item_template()
                        .entry()
                        .find(&o.target_entry)
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    format!("{name}: 0/{}", o.required_count)
                }
                _ => String::new(), // USE_GAMEOBJECT / EXPLORE: cmangos Objectives text covers these
            };
            (o.obj_index, text)
        })
        .filter(|(_, t)| !t.is_empty())
        .collect();
    objs.sort_by_key(|(i, _)| *i);
    objs.into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reputation RANK for a raw standing (0=Hated .. 7=Exalted, Neutral=3). KEEP IN LOCKSTEP with
/// `module/src/reputation.rs::reputation_rank` — same mangos thresholds, duplicated because the
/// module fn is table-crate-private (195; promote both into lyracore_shared if a third consumer appears).
fn reputation_rank(standing: i32) -> u8 {
    match standing {
        s if s >= 42000 => 7,
        s if s >= 21000 => 6,
        s if s >= 9000 => 5,
        s if s >= 3000 => 4,
        s if s >= 0 => 3,
        s if s >= -3000 => 2,
        s if s >= -6000 => 1,
        _ => 0,
    }
}
/// Refuse-interaction threshold: Unfriendly (2) and below refuse; Neutral (3)+ interacts.
const RANK_UNFRIENDLY: u8 = 2;

/// Is template `a` HOSTILE to template `b`? KEEP IN LOCKSTEP with
/// `module/src/faction.rs::compute_hostile` (the vanilla `FactionTemplate.dbc` relationship rule) — the
/// explicit enemy list wins, then the explicit friend list, else the group masks decide. Duplicated
/// over the gateway BINDING type for the interaction-window mask fallback (195).
fn faction_template_hostile(
    a: &super::bindings::FactionTemplate,
    b: &super::bindings::FactionTemplate,
) -> bool {
    if b.faction != 0 {
        if [a.enemy_0, a.enemy_1, a.enemy_2, a.enemy_3].contains(&b.faction) {
            return true;
        }
        if [a.friend_0, a.friend_1, a.friend_2, a.friend_3].contains(&b.faction) {
            return false;
        }
    }
    (a.enemy_group & b.faction_group) != 0
}

impl Coordinator {
    /// Resolve a trainer offering's LEARN TARGET (the rank the buy actually granted) so
    /// SMSG_LEARNED_SPELL books the REAL spell, not the LearnSpell wrapper (live find 2026-07-11:
    /// "Devotion Aura appeared in my General tab as the spell that teaches Devotion Aura").
    /// LOCKSTEP with module trainer.rs::resolve_learn_target — the same first-qualifying-trigger
    /// rule; the excluded kinds are the module's taxonomy values (A_PERIODIC_TRIGGER 0x93,
    /// A_FLAG 0xBE, A_PROC_ON_HIT 0xAB, E_TRIGGER 0x05).
    /// True iff `spell_id` is an on-next-swing QUEUE spell (Heroic Strike/Cleave — any effect of kind
    /// E_NEXT_SWING). The CMSG_CAST_SPELL handler then sends NO synchronous START/CAST_RESULT/GO: the
    /// 5875 client lights the button locally on the press and holds it as a pending cast until the
    /// swing-fire GO arrives (114). Kind value LOCKSTEP with module taxonomy.rs::E_NEXT_SWING (0x13).
    pub fn spell_queues_next_swing(&self, spell_id: u32) -> bool {
        const E_NEXT_SWING: u8 = 0x13;
        let guard = self.0.coord();
        let queues = guard
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_NEXT_SWING);
        queues
    }

    /// True iff `spell_id` is an AUTO-REPEAT ranged attack (Auto Shot / wand Shoot) — the
    /// `SPELL_ATTR_RANGED_AUTO_REPEAT` cast_flags bit set by the importer (from the DBC AttributesEx2
    /// AUTOREPEAT bit / by name). The CMSG_CAST_SPELL handler routes on this instead of a hardcoded
    /// `spell == 75 || 5019` id list, so a new ranged auto-repeat ability onboards as data (097).
    pub fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        const SPELL_ATTR_RANGED_AUTO_REPEAT: u32 = 0x0200; // lockstep with importer::spell.rs
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_spell()
            .spell_id()
            .find(&spell_id)
            .is_some_and(|s| s.cast_flags & SPELL_ATTR_RANGED_AUTO_REPEAT != 0)
    }

    /// The KNOWN rank that learning `new_spell` supersedes (258): the game_spell_chain prev of
    /// `new_spell`, if the player knows it AND the spell's ranks don't stack in the book. Drives
    /// SMSG_SUPERCEDED_SPELL instead of LEARNED_SPELL on a trainer buy.
    ///
    /// The stacking rule is cmangos' canStackSpellRanksInSpellBook (operator-corrected live:
    /// downranking Holy Light is a real thing): MANA spells keep EVERY rank visible (casters
    /// downrank for mana efficiency); rage/energy/health-cost spells and PASSIVES supersede
    /// (Heroic Strike replaces its old rank — there is no "downranked HS").
    pub fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        if spell_ranks_stack_in_book(db, new_spell) {
            return None;
        }
        let prev = db
            .game_spell_chain()
            .spell_id()
            .find(&new_spell)
            .map(|c| c.prev_spell)?;
        if prev == 0 || prev == new_spell {
            return None;
        }
        let knows_prev = db
            .game_player_spell()
            .iter()
            .any(|s| s.character_guid == player_guid && s.spell_id == prev);
        knows_prev.then_some(prev)
    }

    pub fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        const EXCLUDED: [u8; 4] = [0x93, 0xBE, 0xAB, 0x05];
        let guard = self.0.coord();
        let resolved = guard
            .conn
            .db
            .game_spell_effect()
            .iter()
            .filter(|e| e.spell_id == spell_id)
            .find_map(|e| {
                (e.trigger_spell != 0 && !EXCLUDED.contains(&e.kind)).then_some(e.trigger_spell)
            });
        resolved.unwrap_or(spell_id)
    }

    /// The player's buyback ring, newest-first (248): `(item_entry, stack_count, price)` ≤12.
    pub fn buyback_ring(&self, player_guid: u64) -> Vec<(u32, u32, u32)> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let mut rows: Vec<_> = db
            .game_character_buyback()
            .iter()
            .filter(|b| b.player_guid == player_guid)
            .collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        rows.into_iter()
            .map(|b| (b.item_entry, b.stack_count, b.price))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{group_loot_row_visible_to_viewer, quest_row_visible_to_viewer};

    // 195: the two pure reaction helpers, vectors mirrored from module/src/reputation.rs +
    // module/src/faction.rs tests (the lockstep contract).
    #[test]
    fn reputation_rank_thresholds_match_mangos() {
        use super::reputation_rank;
        assert_eq!(reputation_rank(42000), 7); // Exalted
        assert_eq!(reputation_rank(9000), 5); // Honored
        assert_eq!(reputation_rank(0), 3); // Neutral
        assert_eq!(reputation_rank(-1), 2); // Unfriendly
        assert_eq!(reputation_rank(-3001), 1); // Hostile
        assert_eq!(reputation_rank(-42000), 0); // Hated
        assert!(
            reputation_rank(-1) <= super::RANK_UNFRIENDLY,
            "Unfriendly refuses"
        );
        assert!(
            reputation_rank(0) > super::RANK_UNFRIENDLY,
            "Neutral interacts"
        );
    }

    #[test]
    fn faction_template_hostility_mask_and_explicit_lists() {
        use super::super::bindings::FactionTemplate;
        let ft = |id, faction, fg, eg, enemy_0, friend_0| FactionTemplate {
            id,
            faction,
            faction_group: fg,
            friend_group: 0,
            enemy_group: eg,
            enemy_0,
            enemy_1: 0,
            enemy_2: 0,
            enemy_3: 0,
            friend_0,
            friend_1: 0,
            friend_2: 0,
            friend_3: 0,
        };
        // Monster (group mask): enemy_group intersects the player's faction_group → hostile.
        let monster = ft(14, 14, 8, 1, 0, 0);
        let player = ft(1, 1, 1, 0, 0, 0);
        assert!(super::faction_template_hostile(&monster, &player));
        assert!(
            !super::faction_template_hostile(&player, &monster),
            "player group 0 enemy mask"
        );
        // Explicit friend list beats the mask; explicit enemy list beats everything.
        let befriended = ft(2, 14, 8, 1, 0, 1);
        assert!(!super::faction_template_hostile(&befriended, &player));
        let sworn = ft(3, 14, 8, 0, 1, 0);
        assert!(super::faction_template_hostile(&sworn, &player));
    }

    // Work-item 187 slice 0: the per-viewer loot-window visibility gate. Every other read in this
    // file goes through the coordinator's live cache (`RemoteTables`) and has no fake-cache harness to
    // unit-test against (the module crate's "never mock the ctx, extract + test pure fns" rule applies
    // here too) — `quest_row_visible_to_viewer` is the one decision worth pulling out pure so it's
    // directly testable without a live SpacetimeDB connection.

    #[test]
    fn non_quest_rows_are_always_visible_regardless_of_viewer_state() {
        // FFA, unconditional — byte-identical to pre-187 behavior. reserved_for/need are irrelevant.
        assert!(quest_row_visible_to_viewer(false, 0, 7, false));
        assert!(quest_row_visible_to_viewer(false, 99, 7, false));
        assert!(quest_row_visible_to_viewer(false, 7, 7, false));
    }

    #[test]
    fn an_unreserved_quest_row_is_visible_only_to_a_needing_viewer() {
        assert!(
            quest_row_visible_to_viewer(true, 0, 7, true),
            "needs it -> sees the shared row"
        );
        assert!(
            !quest_row_visible_to_viewer(true, 0, 7, false),
            "doesn't need it -> invisible"
        );
    }

    #[test]
    fn a_reserved_clone_is_visible_only_to_its_owner() {
        // The viewer's OWN clone is visible even if `viewer_needs_item` somehow reads false (a stale
        // held-count race) — the reservation itself is the authority once split.
        assert!(
            quest_row_visible_to_viewer(true, 7, 7, false),
            "the viewer's own reserved clone"
        );
        assert!(quest_row_visible_to_viewer(true, 7, 7, true));
        // Reserved for a DIFFERENT character — invisible to this viewer even if they also need it.
        assert!(
            !quest_row_visible_to_viewer(true, 7, 8, true),
            "reserved for someone else"
        );
        assert!(!quest_row_visible_to_viewer(true, 7, 8, false));
    }

    // ---- Group loot methods (work-item 187 slices 2-4) ----

    #[test]
    fn group_loot_baseline_ffa_is_visible_to_everyone() {
        assert!(group_loot_row_visible_to_viewer(0, false, false, 0, 7));
        assert!(group_loot_row_visible_to_viewer(0, false, false, 0, 8));
    }

    #[test]
    fn group_loot_withheld_row_is_invisible_to_everyone_including_the_eventual_winner() {
        // The AutoLoot-safety trap: a live roll's row must not appear in ANYONE's window, not even
        // the guid who will eventually win it.
        assert!(!group_loot_row_visible_to_viewer(0, true, false, 0, 7));
        assert!(!group_loot_row_visible_to_viewer(0, true, false, 7, 7));
    }

    #[test]
    fn group_loot_master_only_row_never_shows_in_the_plain_window() {
        // Not even the stamped master sees it here — they act via SMSG_LOOT_MASTER_LIST instead.
        assert!(!group_loot_row_visible_to_viewer(0, false, true, 42, 42));
        assert!(!group_loot_row_visible_to_viewer(0, false, true, 42, 8));
    }

    #[test]
    fn group_loot_designated_row_is_visible_only_to_its_designee() {
        assert!(group_loot_row_visible_to_viewer(0, false, false, 42, 42));
        assert!(!group_loot_row_visible_to_viewer(0, false, false, 42, 8));
    }

    #[test]
    fn group_loot_reserved_winner_locked_row_wins_over_every_other_flag() {
        // A nonzero reserved_for (the inventory-full winner fallback) is exclusive and unconditional
        // — it overrides withheld/master_only/designated entirely (those are all stale by then).
        assert!(group_loot_row_visible_to_viewer(7, true, true, 99, 7));
        assert!(!group_loot_row_visible_to_viewer(7, true, true, 99, 8));
    }

    /// The AutoLoot-addon trap (work-item 187), spelled out as ONE corpse's two rows: a grey
    /// (below-threshold, FFA-in-group) item autoloots normally while a green (at/above threshold,
    /// mid-roll) item on the SAME corpse is simultaneously withheld from every window — proving the
    /// two rows are decided independently and a live roll can never be swept up by autostore.
    #[test]
    fn a_grey_autoloots_while_a_green_on_the_same_corpse_is_withheld() {
        let grey = (0u64, false, false, 0u64); // FFA baseline: (reserved_for, withheld, master_only, designated)
        let green = (0u64, true, false, 0u64); // a live NEED/GREED roll in progress
        for viewer in [7u64, 8u64] {
            assert!(
                group_loot_row_visible_to_viewer(grey.0, grey.1, grey.2, grey.3, viewer),
                "the grey row autoloots for any viewer"
            );
            assert!(
                !group_loot_row_visible_to_viewer(green.0, green.1, green.2, green.3, viewer),
                "the green row is withheld from every viewer while its roll is live"
            );
        }
    }
}
