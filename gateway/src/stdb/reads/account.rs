//! Account / character / session cache-accessor methods (pure code-motion split of the
//! former `reads.rs`). See `stdb::reads` for the domain split's overview.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};
use spacetimedb_sdk::{Table, TableWithPrimaryKey};

use super::super::bindings::*;
use super::super::connection::Coordinator;
use super::super::views::{AccountRow, RealmRow};
use crate::realm_core::SessionKey;

impl Coordinator {
    /// Read the single realm row for the realm-list reply.
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

    /// Read an account's SRP6 salt/verifier for the logon challenge.
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
                alpha_test_tools: a.alpha_test_tools,
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

    pub(crate) fn realm_character_partition(
        &self,
        guid: u64,
    ) -> Result<Option<crate::world::party::RealmCharacterPartition>> {
        let live = self.0.coord();
        if !live.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for the Realm locator",
                self.shard_name()
            );
        }
        Ok(live
            .conn
            .db
            .game_character_shard()
            .character_guid()
            .find(&guid)
            .map(|row| crate::world::party::RealmCharacterPartition {
                map_id: row.map_id,
                instance_id: row.instance_id,
                revision: row.revision,
                transfer_pending: row.transfer_pending,
                pending_destination_map: row.pending_destination_map,
                pending_destination_instance: row.pending_destination_instance,
                bot_source_identity: row.bot_source_identity,
                bot_transfer_intent_id: row.bot_transfer_intent_id,
                bot_controller_generation: row.bot_controller_generation,
            }))
    }

    /// The account's bound 32-byte SpacetimeDB identity — DERIVED, not minted by a connection
    /// (the per-player-connection build this replaced was the ~850/process wall; see
    /// `synthetic_owner_identity`'s contract). `establish_session` writes it into
    /// `game_account.identity`, which `gw_player_login` fail-closes on.
    pub fn bound_identity(&self, account_id: u64) -> Result<[u8; 32]> {
        Ok(crate::config::synthetic_owner_identity(account_id))
    }

    /// Read the shared session key K, with its expiry, for the world handshake.
    pub fn session_key(&self, account_id: u64) -> Result<Option<SessionKey>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_session()
            .account_id()
            .find(&account_id)
            .and_then(|s| {
                Some(SessionKey {
                    key: <[u8; 40]>::try_from(s.session_key).ok()?,
                    expires_at_micros: s.expires_at.to_micros_since_unix_epoch(),
                })
            }))
    }
}

/// The Gateway-side index over `game_character_contact`, keyed by owner and kept current by the
/// cache's insert, update and delete callbacks. The SDK bindings expose no finder for the table's
/// `by_owner` index, and a whole-table scan per whisper or guild invite copies every contact row on
/// the Shard under the lock the pump needs.
#[derive(Default)]
pub(crate) struct ContactIndex {
    /// Owner guid to its contact rows by row id: `(target guid, is_ignore)`.
    by_owner: HashMap<u64, BTreeMap<u64, (u64, bool)>>,
}

impl ContactIndex {
    fn insert(&mut self, row: &ContactEntry) {
        self.by_owner
            .entry(row.owner_guid)
            .or_default()
            .insert(row.id, (row.target_guid, row.is_ignore));
    }

    fn remove(&mut self, row: &ContactEntry) {
        if let Some(rows) = self.by_owner.get_mut(&row.owner_guid) {
            rows.remove(&row.id);
            if rows.is_empty() {
                self.by_owner.remove(&row.owner_guid);
            }
        }
    }

    fn targets(&self, owner_guid: u64, ignore: bool) -> impl Iterator<Item = u64> + '_ {
        self.by_owner
            .get(&owner_guid)
            .into_iter()
            .flat_map(|rows| rows.values())
            .filter(move |(_, is_ignore)| *is_ignore == ignore)
            .map(|(target_guid, _)| *target_guid)
    }

    /// The guids `owner_guid` ignores.
    pub(crate) fn ignored_by(&self, owner_guid: u64) -> Vec<u64> {
        self.targets(owner_guid, true).collect()
    }

    /// The guids `owner_guid` lists as friends.
    pub(crate) fn friends_of(&self, owner_guid: u64) -> Vec<u64> {
        self.targets(owner_guid, false).collect()
    }
}

/// The Gateway-side index from lowercase Character name to guid on one Shard, kept current by the
/// `game_character` callbacks. The bindings' unique `name` finder is case-sensitive, and a typed
/// name is not; a whole-table scan per whisper and per Shard copies every Character row under the
/// lock the pump needs.
#[derive(Default)]
pub(crate) struct CharacterNameIndex {
    by_name: HashMap<String, BTreeSet<u64>>,
}

impl CharacterNameIndex {
    fn insert(&mut self, guid: u64, name: &str) {
        self.by_name
            .entry(name.to_ascii_lowercase())
            .or_default()
            .insert(guid);
    }

    fn remove(&mut self, guid: u64, name: &str) {
        let key = name.to_ascii_lowercase();
        if let Some(guids) = self.by_name.get_mut(&key) {
            guids.remove(&guid);
            if guids.is_empty() {
                self.by_name.remove(&key);
            }
        }
    }

    /// The Character `name` names, ASCII case-insensitive. Names are unique per Shard only up to
    /// case, so two rows that differ only in case answer the lower guid, the same one every time.
    pub(crate) fn guid_named(&self, name: &str) -> Option<u64> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|guids| guids.first().copied())
    }
}

/// Keep a [`CharacterNameIndex`] current from the Character callbacks. Initial subscription rows
/// arrive as inserts.
pub(crate) fn watch_character_names(conn: &DbConnection) -> Arc<RwLock<CharacterNameIndex>> {
    let index = Arc::new(RwLock::new(CharacterNameIndex::default()));
    let inserted = index.clone();
    conn.db.game_character().on_insert(move |_ctx, row| {
        let mut names = inserted.write().unwrap_or_else(|p| p.into_inner());
        names.insert(row.guid, &row.name);
    });
    let updated = index.clone();
    conn.db.game_character().on_update(move |_ctx, old, new| {
        let mut names = updated.write().unwrap_or_else(|p| p.into_inner());
        names.remove(old.guid, &old.name);
        names.insert(new.guid, &new.name);
    });
    let deleted = index.clone();
    conn.db.game_character().on_delete(move |_ctx, row| {
        let mut names = deleted.write().unwrap_or_else(|p| p.into_inner());
        names.remove(row.guid, &row.name);
    });
    index
}

/// Keep a [`ContactIndex`] current from the contact callbacks. Initial subscription rows arrive as
/// inserts.
pub(crate) fn watch_contacts(conn: &DbConnection) -> Arc<RwLock<ContactIndex>> {
    let index = Arc::new(RwLock::new(ContactIndex::default()));
    let inserted = index.clone();
    conn.db
        .game_character_contact()
        .on_insert(move |_ctx, row| {
            inserted
                .write()
                .unwrap_or_else(|p| p.into_inner())
                .insert(row);
        });
    let updated = index.clone();
    conn.db
        .game_character_contact()
        .on_update(move |_ctx, old, new| {
            let mut contacts = updated.write().unwrap_or_else(|p| p.into_inner());
            contacts.remove(old);
            contacts.insert(new);
        });
    let deleted = index.clone();
    conn.db
        .game_character_contact()
        .on_delete(move |_ctx, row| {
            deleted
                .write()
                .unwrap_or_else(|p| p.into_inner())
                .remove(row);
        });
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: u64, owner_guid: u64, target_guid: u64, is_ignore: bool) -> ContactEntry {
        ContactEntry {
            id,
            owner_guid,
            owner_identity: spacetimedb_sdk::Identity::ZERO,
            target_guid,
            is_ignore,
        }
    }

    #[test]
    fn the_contact_index_answers_each_owners_own_lists() {
        let mut index = ContactIndex::default();
        index.insert(&contact(1, 10, 20, true));
        index.insert(&contact(2, 10, 30, false));
        index.insert(&contact(3, 11, 40, true));
        assert_eq!(index.ignored_by(10), vec![20]);
        assert_eq!(index.friends_of(10), vec![30]);
        assert_eq!(index.ignored_by(11), vec![40]);
        assert!(index.friends_of(11).is_empty());
        assert!(index.ignored_by(12).is_empty());
    }

    #[test]
    fn a_typed_name_finds_its_character_in_any_case() {
        let mut index = CharacterNameIndex::default();
        index.insert(7, "Thrall");
        assert_eq!(index.guid_named("thrall"), Some(7));
        assert_eq!(index.guid_named("THRALL"), Some(7));
        assert_eq!(index.guid_named("Thral"), None);
    }

    #[test]
    fn a_renamed_or_deleted_character_leaves_the_name_index() {
        let mut index = CharacterNameIndex::default();
        index.insert(7, "Thrall");
        index.insert(8, "thrall");
        assert_eq!(
            index.guid_named("Thrall"),
            Some(7),
            "the lower guid, every time"
        );
        index.remove(7, "Thrall");
        assert_eq!(index.guid_named("Thrall"), Some(8));
        index.remove(8, "thrall");
        assert_eq!(
            index.guid_named("Thrall"),
            None,
            "no Character named it now"
        );
    }

    /// `watch_character_names`'s `on_update` callback does exactly this: remove the old name, then
    /// insert the new one. A rename must not answer to both names, nor to neither.
    #[test]
    fn a_renamed_character_answers_only_to_its_new_name() {
        let mut index = CharacterNameIndex::default();
        index.insert(7, "Thrall");
        index.remove(7, "Thrall");
        index.insert(7, "Go'el");
        assert_eq!(index.guid_named("Thrall"), None, "the old name is gone");
        assert_eq!(index.guid_named("go'el"), Some(7), "the new name resolves");
    }

    #[test]
    fn a_deleted_contact_row_leaves_the_index() {
        let mut index = ContactIndex::default();
        index.insert(&contact(1, 10, 20, true));
        index.insert(&contact(2, 10, 21, true));
        index.remove(&contact(1, 10, 20, true));
        assert_eq!(index.ignored_by(10), vec![21]);
        index.remove(&contact(2, 10, 21, true));
        assert!(
            index.ignored_by(10).is_empty(),
            "an owner with no rows left ignores nobody"
        );
    }
}
