//! Account / character / session cache-accessor methods (pure code-motion split of the
//! former `reads.rs`). See `stdb::reads` for the domain split's overview.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use super::super::views::{character_view, AccountRow, RealmRow};

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
    /// routing. Prefers the LIVE entity and falls back to the durable `game_character` row,
    /// which is what a fresh login (and a mid-teleport character, whose entity was despawned)
    /// reads. `None` = this shard has no row for that guid, which is also how
    /// `realm_core::locate_home_shard` finds the shard that does.
    ///
    /// The durable fallback reads `pending_instance_id`, NOT a hardcoded 0: that column is
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

    /// This shard's raw durable `game_character` row for `guid` — the transfer driver needs
    /// the DESTINATION fields (`pending_instance_id`, position) that `CharacterView` does not carry.
    pub(crate) fn character_row(&self, guid: u64) -> Option<super::super::bindings::Character> {
        self.0.coord().conn.db.game_character().guid().find(&guid)
    }

    /// This shard's escrow row for `guid`, raw. `Coordinator`-level rather than
    /// `WorldStore`-level because it's needed (via `has_escrow`) before any handle has been chosen
    /// as the session's home — see `realm_core::locate_home_shard`.
    pub(crate) fn escrow_row(&self, guid: u64) -> Option<super::super::bindings::TransferOut> {
        use spacetimedb_sdk::Table as _;
        self.0
            .coord()
            .conn
            .db
            .game_transfer_out()
            .iter()
            .find(|r| r.character_guid == guid)
    }

    /// The realm-core character→shard index entry for `guid`: the `(map_id, instance_id)` the realm
    /// believes the character is at. A HINT — `config::resolve_home_shard` confirms it against
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

    /// The EFFECTIVE armor for `guid` for the character-sheet CREATE (`UNIT_FIELD_RESISTANCES[0]`),
    /// Presence check for the WORLDPORT_ACK gate: is the guid's live entity in the world?
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
        super::super::armor::effective_armor(&guard.conn.db, guid)
    }

    /// The live entity's `max_health` from the privileged cache — 0 if not in world. Feeds the
    /// fall-damage flavor line; the module applies the authoritative damage itself.
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

    /// The account's bound 32-byte SpacetimeDB identity: the node-issued identity of the
    /// per-account player connection. `establish_session` writes this into `game_account.identity`
    /// so the player connection's later `player_login`/`movement_update` calls pass the module's
    /// `ctx.sender == owner` checks. Opening the connection here (at logon) caches it for reuse in
    /// the world phase.
    pub fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> {
        // Under LYRACORE_SHARED_CALLS the bound identity is DERIVED, not minted by a connection —
        // this call was what opened the per-account connection at logon, i.e. the exact build the
        // ~850/process wall lives in. Viable once the remaining cold verbs got their `gw_*` twins
        // (the first attempt broke the then-unmigrated cold verbs and was reverted — see
        // a6ce00b2). See `synthetic_owner_identity`'s contract for the full story.
        if crate::config::shared_calls_enabled() {
            return Ok(crate::config::synthetic_owner_identity(account_id));
        }
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
    /// `SMSG_IGNORE_LIST`. Reads `game_character_contact` via the privileged cache
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
}
