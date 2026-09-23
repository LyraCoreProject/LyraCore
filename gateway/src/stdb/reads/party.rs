//! Party/group + loot-roll cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use lyracore_shared::group::{GroupKind, RaidSlot};

/// A cached `game_group.group_type`. The Module writes only valid bytes, so the Party fallback is
/// unreachable.
fn group_kind_of(byte: u8) -> GroupKind {
    GroupKind::from_wire(byte).unwrap_or_default()
}

/// A cached `game_group_member.raid_slot`. The Module writes only valid bytes, so the fallback to
/// Subgroup 0 without the Assistant flag is unreachable.
fn raid_slot_of(byte: u8) -> RaidSlot {
    RaidSlot::from_wire(byte).unwrap_or_default()
}

impl Coordinator {
    pub(crate) fn stable_party_holder_observation(
        &self,
        character_guid: u64,
        serves_locator: bool,
    ) -> Result<crate::world::party::PartyHolderObservation> {
        let guard = self.0.coord();
        if !guard.is_healthy() {
            anyhow::bail!(
                "{} has no healthy Coordinator subscription for party partition certification",
                self.shard_name()
            );
        }
        let has_escrow = guard
            .conn
            .db
            .game_transfer_out()
            .transfer_id()
            .find(&crate::world::transfer::transfer_id_for(character_guid))
            .is_some_and(|row| row.character_guid == character_guid);
        let character_partition = guard
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid)
            .map(|character| (character.map_id, character.pending_instance_id));
        Ok(crate::world::party::PartyHolderObservation {
            serves_locator,
            has_escrow,
            character_partition,
        })
    }

    /// Every party in this Realm-core or World Shard cache.
    pub fn party_group_ids(&self) -> Vec<u64> {
        self.0
            .coord()
            .conn
            .db
            .game_group()
            .iter()
            .map(|group| group.group_id)
            .collect()
    }

    /// Every Character with party membership in this Realm-core cache.
    pub fn party_member_guids(&self) -> Vec<u64> {
        self.0
            .coord()
            .conn
            .db
            .game_group_member()
            .iter()
            .map(|member| member.character_guid)
            .collect()
    }

    /// The party `character_guid` belongs to, read from THIS handle's database.
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

    /// Read only the bounded roster projection accepted by companion-command authority. The bound
    /// is the Raid cap: a longer list is a damaged cache, and the caller refuses a Raid above five.
    pub fn party_command_group_roster(
        &self,
        character_guid: u64,
    ) -> anyhow::Result<Option<crate::world::party::GroupRoster>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some((group_id, rows)) = guard
            .party_memberships
            .read()
            .unwrap()
            .bounded_member_rows(character_guid, lyracore_shared::group::RAID_MAX_MEMBERS)?
        else {
            return Ok(None);
        };
        let Some(group) = db.game_group().group_id().find(&group_id) else {
            return Ok(None);
        };
        let members = rows
            .into_iter()
            .map(|(row_id, guid)| crate::world::party::GroupRosterMember {
                guid,
                slot: db
                    .game_group_member()
                    .id()
                    .find(&row_id)
                    .map_or_else(Default::default, |row| raid_slot_of(row.raid_slot)),
            })
            .collect();
        Ok(Some(crate::world::party::GroupRoster {
            group_id,
            roster_revision: db
                .game_group_roster_revision()
                .group_id()
                .find(&group_id)
                .map_or(1, |row| row.revision),
            leader_guid: group.leader_guid,
            loot_method: group.loot_method,
            loot_threshold: group.loot_threshold,
            master_looter_guid: group.master_looter_guid,
            kind: group_kind_of(group.group_type),
            members,
            partitions: Vec::new(),
        }))
    }

    /// [`group_roster`](Self::group_roster) keyed by the group itself — the read the mirror push
    /// needs for a party the acting character has just LEFT (their own membership row is gone, but
    /// the remaining members' rows still have to reach every shard).
    ///
    /// Realm membership revisions retain join order when Transfer replaces a World mirror row.
    /// Realm-core and unmirrored local parties use their own member-row ids.
    pub fn group_roster_by_id(&self, group_id: u64) -> Option<crate::world::party::GroupRoster> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let group = db.game_group().iter().find(|g| g.group_id == group_id)?;
        let mut rows: Vec<(u64, u64, u8)> = db
            .game_group_member()
            .iter()
            .filter(|m| m.group_id == group_id)
            .map(|m| {
                let membership_revision = db
                    .game_group_member_partition()
                    .character_guid()
                    .find(&m.character_guid)
                    .filter(|partition| partition.group_id == group_id && partition.member_active)
                    .map_or(m.id, |partition| partition.membership_revision);
                (membership_revision, m.character_guid, m.raid_slot)
            })
            .collect();
        rows.sort_unstable();
        let partitions = rows
            .iter()
            .map(|(membership_revision, character_guid, _)| {
                crate::world::party::GroupMemberPartition {
                    character_guid: *character_guid,
                    group_id,
                    membership_revision: *membership_revision,
                    member_active: true,
                    map_id: 0,
                    instance_id: 0,
                    locator_revision: 0,
                    state: crate::world::party::PartyPartitionState::Unknown,
                }
            })
            .collect();
        Some(crate::world::party::GroupRoster {
            group_id,
            roster_revision: db
                .game_group_roster_revision()
                .group_id()
                .find(&group_id)
                .map_or(1, |row| row.revision),
            leader_guid: group.leader_guid,
            loot_method: group.loot_method,
            loot_threshold: group.loot_threshold,
            master_looter_guid: group.master_looter_guid,
            kind: group_kind_of(group.group_type),
            members: rows
                .into_iter()
                .map(|(_, guid, slot)| crate::world::party::GroupRosterMember {
                    guid,
                    slot: raid_slot_of(slot),
                })
                .collect(),
            partitions,
        })
    }

    /// Realm-core's roster order survives disband in `game_group_roster_revision`.
    pub fn group_roster_revision(&self, group_id: u64) -> u64 {
        self.0
            .coord()
            .conn
            .db
            .game_group_roster_revision()
            .group_id()
            .find(&group_id)
            .map_or(1, |row| row.revision)
    }

    /// Every UNRESOLVED `game_loot_roll` row on THIS handle's database, joined with its votes.
    ///
    /// A cache read, like [`group_roster`](Self::group_roster) — this table is now part of the
    /// coordinator subscription list, so no reducer call is
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
                    random_property_id: r.random_property_id,
                    promotion_source: r.promotion_source,
                }
            })
            .collect())
    }

    // The return tuple is the poll result: the new watermark plus the `(corpse, slot, winner)` triples read since the old one.
    #[allow(clippy::type_complexity)]
    /// Every `ROLL_WON` `game_group_event` row on THIS handle's database with `id > after_id`,
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

#[cfg(test)]
mod member_stats_adapter_tests {
    use crate::test_scan::code_of;

    fn flat(signature: &str) -> String {
        code_of(include_str!("party.rs"), signature)
            .split_whitespace()
            .collect()
    }

    /// Member Stats reads Realm Presence rather than running its own discovery — no second
    /// live-entity/in-transit/absence decision left to duplicate `presence::of`'s.
    #[test]
    fn the_coordinator_converts_realm_presence_into_member_presence() {
        let presence = flat("pub(crate) fn member_presence(");
        assert!(presence.contains("crate::world::presence::of(self,guid)?"));
        assert!(presence.contains("Whereabouts::InWorld{entity,shard_name,..}"));
        assert!(presence.contains("self.with_member_shard_stats(&shard_name,guid,entity)"));
        assert!(presence.contains("crate::world::MemberPresence::Live"));
        assert!(
            presence.contains("Whereabouts::InTransit)=>crate::world::MemberPresence::InTransit")
        );
        assert!(
            presence.contains("Whereabouts::Offline)|None=>crate::world::MemberPresence::Offline")
        );
    }

    /// The aura and pet overlay must read from the SAME `ShardId` the live match came from, not a
    /// fixed or default one — a cross-shard aura read would silently show nothing outside the
    /// Gateway process that owns that shard's connection.
    ///
    /// A source scan, not a behavior test through a real `Coordinator`: `Coordinator` wraps the
    /// spacetimedb-sdk `DbConnection`, which only a live SpacetimeDB node can populate, so this
    /// method's actual database reads are outside what a unit test can drive. `crate::test_scan`'s
    /// own module doc names that as the same untestable-by-mock shape `ReducerContext` is on the
    /// Module side. This scan stays narrow: one substring per call it must make, not a copy of the
    /// method body.
    #[test]
    fn the_shard_overlay_reads_auras_and_pet_from_the_matched_shard() {
        let overlay = flat("fn with_member_shard_stats(");
        assert!(overlay.contains("self.member_aura_slots(shard,guid)"));
        assert!(overlay.contains("self.member_pet(shard,guid)"));

        let pet = flat("fn member_pet(");
        assert!(pet.contains("self.member_aura_slots(shard,pet.guid)"));
    }

    /// The pet read must stay a keyed lookup by the deterministic pet guid, never a table scan:
    /// `game_world_entity().iter()` clones every cached row while holding the client cache's own
    /// lock, and `member_pet` runs for every live group mate on every Relay tick, which can stall
    /// the shard pump that lock also guards. Also a source scan; see the test above for why.
    #[test]
    fn member_pet_looks_up_the_deterministic_guid_instead_of_scanning() {
        let pet = flat("fn member_pet(");
        assert!(pet.contains("lyracore_shared::pet::pet_guid_for(owner_guid)"));
        assert!(pet.contains(".guid().find(&pet_guid)"));
        assert!(
            !pet.contains("game_world_entity().iter()"),
            "member_pet must not scan every live entity to find one pet"
        );
    }
}
