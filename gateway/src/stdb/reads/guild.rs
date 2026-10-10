//! Guild cache reads. The guild tables are read on the Realm-core handle; Character facts are read
//! across every World Shard. A Fee Hold and a Guild Charter item are read on the payer's Home
//! Shard.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use spacetimedb_sdk::{Table, TableWithPrimaryKey};

use super::super::bindings::*;
use super::super::connection::Coordinator;
use crate::world::CharacterFacts;

/// The Guild Ranks and members of each Guild, and the Characters that Petitions and Signatures
/// name, kept from the row callbacks. The SDK cache has no index on these columns, so a guild read
/// finds its rows here by key instead of scanning the tables.
#[derive(Default)]
pub(crate) struct GuildIndex {
    rank_ids: HashMap<u32, BTreeSet<u64>>,
    member_guids: HashMap<u32, BTreeSet<u64>>,
    /// Petition owner or signer guid to the number of Petition and Signature rows naming it.
    pub(in crate::stdb) petition_characters: HashMap<u64, usize>,
}

impl GuildIndex {
    fn insert_rank(&mut self, row: &GuildRank) {
        self.rank_ids
            .entry(row.guild_id)
            .or_default()
            .insert(row.id);
    }

    fn remove_rank(&mut self, row: &GuildRank) {
        remove_key(&mut self.rank_ids, row.guild_id, row.id);
    }

    fn insert_member(&mut self, row: &GuildMember) {
        self.member_guids
            .entry(row.guild_id)
            .or_default()
            .insert(row.character_guid);
    }

    fn remove_member(&mut self, row: &GuildMember) {
        remove_key(&mut self.member_guids, row.guild_id, row.character_guid);
    }

    pub(in crate::stdb) fn rank_ids(&self, guild_id: u32) -> Vec<u64> {
        self.rank_ids
            .get(&guild_id)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The members of `guild_id`, lowest guid first.
    pub(in crate::stdb) fn member_guids(&self, guild_id: u32) -> Vec<u64> {
        self.member_guids
            .get(&guild_id)
            .map(|guids| guids.iter().copied().collect())
            .unwrap_or_default()
    }

    fn name_petition_character(&mut self, guid: u64) {
        *self.petition_characters.entry(guid).or_default() += 1;
    }

    fn drop_petition_character(&mut self, guid: u64) {
        if let Some(rows) = self.petition_characters.get_mut(&guid) {
            *rows -= 1;
            if *rows == 0 {
                self.petition_characters.remove(&guid);
            }
        }
    }

    /// Every member of every Guild, every Petition owner and every signer.
    pub(in crate::stdb) fn named_characters(&self) -> BTreeSet<u64> {
        self.member_guids
            .values()
            .flatten()
            .copied()
            .chain(self.petition_characters.keys().copied())
            .collect()
    }
}

fn remove_key(index: &mut HashMap<u32, BTreeSet<u64>>, guild_id: u32, key: u64) {
    if let Some(keys) = index.get_mut(&guild_id) {
        keys.remove(&key);
        if keys.is_empty() {
            index.remove(&guild_id);
        }
    }
}

/// Keep a [`GuildIndex`] current from the Guild Rank and member row callbacks. Register it before
/// the subscription applies: the initial rows arrive as inserts.
pub(crate) fn watch_guilds(conn: &DbConnection) -> Arc<RwLock<GuildIndex>> {
    let index = Arc::new(RwLock::new(GuildIndex::default()));
    let inserted = index.clone();
    conn.db.game_guild_rank().on_insert(move |_ctx, row| {
        inserted.write().unwrap().insert_rank(row);
    });
    let updated = index.clone();
    conn.db.game_guild_rank().on_update(move |_ctx, old, new| {
        let mut guilds = updated.write().unwrap();
        guilds.remove_rank(old);
        guilds.insert_rank(new);
    });
    let deleted = index.clone();
    conn.db.game_guild_rank().on_delete(move |_ctx, row| {
        deleted.write().unwrap().remove_rank(row);
    });
    let inserted = index.clone();
    conn.db.game_guild_member().on_insert(move |_ctx, row| {
        inserted.write().unwrap().insert_member(row);
    });
    let updated = index.clone();
    conn.db
        .game_guild_member()
        .on_update(move |_ctx, old, new| {
            let mut guilds = updated.write().unwrap();
            guilds.remove_member(old);
            guilds.insert_member(new);
        });
    let deleted = index.clone();
    conn.db.game_guild_member().on_delete(move |_ctx, row| {
        deleted.write().unwrap().remove_member(row);
    });
    let inserted = index.clone();
    conn.db.game_guild_petition().on_insert(move |_ctx, row| {
        inserted
            .write()
            .unwrap()
            .name_petition_character(row.owner_guid);
    });
    let updated = index.clone();
    conn.db
        .game_guild_petition()
        .on_update(move |_ctx, old, new| {
            let mut guilds = updated.write().unwrap();
            guilds.drop_petition_character(old.owner_guid);
            guilds.name_petition_character(new.owner_guid);
        });
    let deleted = index.clone();
    conn.db.game_guild_petition().on_delete(move |_ctx, row| {
        deleted
            .write()
            .unwrap()
            .drop_petition_character(row.owner_guid);
    });
    let inserted = index.clone();
    conn.db
        .game_guild_petition_signature()
        .on_insert(move |_ctx, row| {
            inserted
                .write()
                .unwrap()
                .name_petition_character(row.signer_guid);
        });
    let updated = index.clone();
    conn.db
        .game_guild_petition_signature()
        .on_update(move |_ctx, old, new| {
            let mut guilds = updated.write().unwrap();
            guilds.drop_petition_character(old.signer_guid);
            guilds.name_petition_character(new.signer_guid);
        });
    let deleted = index.clone();
    conn.db
        .game_guild_petition_signature()
        .on_delete(move |_ctx, row| {
            deleted
                .write()
                .unwrap()
                .drop_petition_character(row.signer_guid);
        });
    index
}

/// A Petition with its signers, found by key: the SDK cache has no index on the Petition column.
pub(in crate::stdb) fn petition_view(
    db: &RemoteTables,
    row: GuildPetition,
) -> crate::codec::PetitionView {
    use lyracore_shared::guild::{petition_signature_key, MAX_PETITION_SIGNATURES};
    let signers = (0..MAX_PETITION_SIGNATURES)
        .filter_map(|slot| {
            db.game_guild_petition_signature()
                .signature_key()
                .find(&petition_signature_key(row.petition_id, slot))
        })
        .map(|signature| signature.signer_guid)
        .collect();
    crate::codec::PetitionView {
        petition_id: row.petition_id,
        charter_item_guid: row.charter_item_guid,
        owner_guid: row.owner_guid,
        name: row.name,
        signers,
    }
}

impl Coordinator {
    /// The Guild Projection `(guild_id, rank_id)` of `character_guid` in THIS handle's cache.
    /// `(0, 0)` outside a Guild. Cheap enough for a relay job.
    pub(crate) fn guild_projection(&self, character_guid: u64) -> (u32, u32) {
        self.0
            .coord()
            .conn
            .db
            .game_guild_member()
            .character_guid()
            .find(&character_guid)
            .map_or((0, 0), |member| (member.guild_id, member.rank_id))
    }

    /// Facts for one Character: the last logout and Realm Account from whichever World Shard holds
    /// its row, and its Realm Presence. `None` when no World Shard holds a Character row, so a
    /// creature's live entity never passes for a Character. `Err` when the World Shards cannot
    /// vouch for an absence.
    pub(crate) fn guild_character_facts(
        &self,
        character_guid: u64,
    ) -> Result<Option<CharacterFacts>> {
        let durable = self.all_shards().iter().find_map(|shard| {
            let guard = shard.0.coord();
            let db = &guard.conn.db;
            let character = db.game_character().guid().find(&character_guid)?;
            let realm_account_id = db
                .game_account_character_owner()
                .character_guid()
                .find(&character_guid)
                .map_or(0, |owner| owner.account_id);
            Some(crate::world::DurableCharacterFacts {
                last_logout_micros: character.last_logout_micros,
                realm_account_id,
            })
        });
        crate::world::character_facts(durable, || crate::world::presence::of(self, character_guid))
    }

    /// The GM level on THIS handle's own Character row, 0 when it holds none. The guild Gates call
    /// it on the actor's Home Shard, so a copy an interrupted Transfer left elsewhere never counts.
    pub(crate) fn home_gm_level(&self, character_guid: u64) -> u8 {
        self.0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid)
            .map_or(0, |character| character.gm_level)
    }

    /// The open Petition of the Guild Charter `charter_item_guid` in THIS handle's cache. Call it
    /// on the Realm-core handle.
    pub(crate) fn guild_petition_of_charter(
        &self,
        charter_item_guid: u64,
    ) -> Option<crate::codec::PetitionView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let row = db
            .game_guild_petition()
            .charter_item_guid()
            .find(&charter_item_guid)?;
        Some(petition_view(db, row))
    }

    /// The Petition id of the Guild Charter `charter_item_guid` in THIS handle's cache. Call it on
    /// the Realm-core handle.
    pub(crate) fn charter_petition_id(&self, charter_item_guid: u64) -> Option<u32> {
        self.0
            .coord()
            .conn
            .db
            .game_guild_petition()
            .charter_item_guid()
            .find(&charter_item_guid)
            .map(|petition| petition.petition_id)
    }

    /// Fill each Guild Charter's ITEM_FIELD_ENCHANTMENT with its Petition id from the Realm-core
    /// cache. Call it on the owner's Home Shard, with no cache guard held. A Charter whose
    /// Petition is unreadable shows 0; the client then asks about no Petition.
    pub(crate) fn project_charter_petitions(&self, items: &mut [crate::codec::ItemInstanceView]) {
        use lyracore_shared::guild::GUILD_CHARTER_ENTRY;
        if !items.iter().any(|item| item.entry == GUILD_CHARTER_ENTRY) {
            return;
        }
        let Ok(realm) = self.realm_core() else {
            return;
        };
        for item in items
            .iter_mut()
            .filter(|item| item.entry == GUILD_CHARTER_ENTRY)
        {
            item.enchantment = realm.charter_petition_id(item.guid).unwrap_or(0);
        }
    }
}
