//! `Coordinator`'s [`MemberStatsStore`] adapter.

use anyhow::Result;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::Coordinator;
use crate::world::{MemberPresence, MemberStatsStore};

impl MemberStatsStore for crate::stdb::Coordinator {
    fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>> {
        crate::stdb::Coordinator::group_mates(self, self_guid)
    }

    fn member_presence(&self, guid: u64) -> Result<MemberPresence> {
        crate::stdb::Coordinator::member_presence(self, guid)
    }
}

impl Coordinator {
    /// Every other member of `self_guid`'s group, from the party authority's membership index:
    /// Realm-core on a sharded Realm, this database otherwise. A list longer than a Raid is a
    /// damaged cache, not a group.
    pub(crate) fn group_mates(&self, self_guid: u64) -> Result<Vec<u64>> {
        let authority = if self.is_sharded() {
            self.realm_core()?
        } else {
            self.clone()
        };
        let roster = authority
            .0
            .coord()
            .party_memberships
            .read()
            .unwrap()
            .bounded_member_rows(self_guid, lyracore_shared::group::RAID_MAX_MEMBERS)?;
        Ok(roster
            .map(|(_, rows)| {
                rows.into_iter()
                    .map(|(_, guid)| guid)
                    .filter(|guid| *guid != self_guid)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Find `guid` for Member Stats from its Realm Presence — the same live-entity / in-transit /
    /// absence-gated-offline decision every other realm-wide read now shares, so there is no
    /// second discovery left to disagree with it.
    pub(crate) fn member_presence(&self, guid: u64) -> Result<crate::world::MemberPresence> {
        use crate::world::presence::Whereabouts;
        Ok(
            match crate::world::presence::of(self, guid)?.map(|presence| presence.whereabouts) {
                Some(Whereabouts::InWorld {
                    entity, shard_name, ..
                }) => {
                    let entity = self.with_member_shard_stats(&shard_name, guid, entity);
                    crate::world::MemberPresence::Live(Box::new(
                        crate::codec::MemberStats::from_entity(&entity),
                    ))
                }
                Some(Whereabouts::InTransit) => crate::world::MemberPresence::InTransit,
                Some(Whereabouts::Offline) | None => crate::world::MemberPresence::Offline,
            },
        )
    }

    /// Overlay `guid`'s aura slots and live pet onto `entity` — the one piece of Member Stats a
    /// generic Realm Presence read cannot supply, because it does not know about `ShardId` or
    /// `WorldView`'s `AuraIndex`. Reads from the exact Shard named `shard_name`, the same one
    /// `presence::of` already searched to find the live entity `entity` itself came from, keyed by
    /// its position in `all_shards()` — the same `enumerate()` order `arm_shared_world_view`
    /// assigned when it built the `AuraIndex`. A name this handle's own `all_shards()` no longer
    /// lists (a Shard dropped between the two reads) leaves `entity` as `presence::of` built it,
    /// pet-less and aura-less rather than wrong.
    fn with_member_shard_stats(
        &self,
        shard_name: &str,
        guid: u64,
        entity: crate::codec::MemberEntity,
    ) -> crate::codec::MemberEntity {
        let Some(shard) = self
            .all_shards()
            .iter()
            .position(|coord| coord.shard_name() == shard_name)
        else {
            return entity;
        };
        crate::codec::MemberEntity {
            auras: self.member_aura_slots(shard, guid),
            pet: self.member_pet(shard, guid),
            ..entity
        }
    }

    /// `owner_guid`'s occupied `UNIT_FIELD_AURA` slots on `shard`, from `WorldView`'s `AuraIndex`.
    fn member_aura_slots(
        &self,
        shard: crate::stdb::world_index::ShardId,
        owner_guid: u64,
    ) -> Vec<crate::codec::MemberAuraSlot> {
        self.world_view()
            .auras
            .on_target(shard, owner_guid)
            .into_iter()
            .map(|aura| crate::codec::MemberAuraSlot {
                slot: aura.slot,
                spell_id: aura.spell_id,
            })
            .collect()
    }

    /// `owner_guid`'s live pet, resolved the way the pet bar resolves it: a Hunter's name comes
    /// from `game_hunter_pet_protocol`, a summoned pet's from its creature template, and every
    /// other field from the pet's own `game_world_entity` row (`pet_name`, `stdb/reads/pet.rs`).
    ///
    /// The pet guid is deterministic (`lyracore_shared::pet::pet_guid_for`, the same derivation
    /// the Module's `pet_of` uses), so this is one keyed lookup plus an owner check, never a scan.
    /// `game_world_entity().iter()` clones every cached row while holding the client cache's own
    /// lock, and this read runs for every live group mate on every Relay tick; a scan here would
    /// contend with the shard's coordinator pump, the busiest callback in the process.
    fn member_pet(
        &self,
        shard: crate::stdb::world_index::ShardId,
        owner_guid: u64,
    ) -> Option<crate::codec::MemberPetEntity> {
        let live = self.0.coord();
        let db = &live.conn.db;
        let pet_guid = lyracore_shared::pet::pet_guid_for(owner_guid);
        let pet = db
            .game_world_entity()
            .guid()
            .find(&pet_guid)
            .filter(|pet| pet.owner_guid == owner_guid)?;
        let name = db
            .game_hunter_pet_protocol()
            .iter()
            .find(|hunter| hunter.live_pet_guid == pet.guid)
            .map(|hunter| hunter.name)
            .or_else(|| {
                db.game_creature_template()
                    .entry()
                    .find(&pet.entry)
                    .map(|template| template.name)
            })
            .unwrap_or_default();
        Some(crate::codec::MemberPetEntity {
            guid: pet.guid,
            name,
            display_id: pet.display_id,
            health: pet.health,
            max_health: pet.max_health,
            power: pet.power,
            max_power: pet.max_power,
            unit_bytes_0: pet.unit_bytes_0,
            auras: self.member_aura_slots(shard, pet.guid),
        })
    }
}
