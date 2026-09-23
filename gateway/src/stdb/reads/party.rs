//! Party/group + loot-roll cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;

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

    /// Read only the bounded roster projection accepted by companion-command authority.
    pub fn party_command_group_roster(
        &self,
        character_guid: u64,
    ) -> anyhow::Result<Option<crate::world::party::GroupRoster>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some((group_id, members)) = guard
            .party_memberships
            .read()
            .unwrap()
            .bounded_roster(character_guid, lyracore_shared::group::GROUP_MAX_MEMBERS)?
        else {
            return Ok(None);
        };
        let Some(group) = db.game_group().group_id().find(&group_id) else {
            return Ok(None);
        };
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
        let mut rows: Vec<(u64, u64)> = db
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
                (membership_revision, m.character_guid)
            })
            .collect();
        rows.sort_unstable();
        let partitions = rows
            .iter()
            .map(|(membership_revision, character_guid)| {
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
            members: rows.into_iter().map(|(_, guid)| guid).collect(),
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

/// The raid cap (cm:Group.h:41). A longer member list is a damaged cache, not a group.
const MEMBER_STATS_ROSTER_LIMIT: usize = 40;

impl Coordinator {
    /// Every other member of `self_guid`'s group, from the party authority's membership index:
    /// Realm-core on a sharded Realm, this database otherwise.
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
            .bounded_roster(self_guid, MEMBER_STATS_ROSTER_LIMIT)?;
        Ok(roster
            .map(|(_, members)| members.into_iter().filter(|m| *m != self_guid).collect())
            .unwrap_or_default())
    }

    /// Find `guid` for Member Stats with the shared [`crate::world::locate_member`] decision. A
    /// live entity may come from any connected World Shard. Offline follows the absence rule:
    /// every configured World Shard must be healthy.
    ///
    /// Each connected shard is paired with its position in `all_shards()` before the scan, the
    /// same `enumerate()` order `arm_shared_world_view` assigned when it built `WorldView`'s
    /// `AuraIndex` — that position is the `ShardId` the index keys its rows by, so a live match
    /// can read the right shard's auras and pet.
    pub(crate) fn member_presence(&self, guid: u64) -> Result<crate::world::MemberPresence> {
        let connected: Vec<MemberShard> = self
            .all_shards()
            .into_iter()
            .enumerate()
            .map(|(id, coord)| MemberShard { id, coord })
            .collect();
        crate::world::locate_member(
            guid,
            &connected,
            || {
                if !self.is_sharded() {
                    return Ok(false);
                }
                let locator = self.realm_core()?.realm_character_partition(guid)?;
                Ok(locator.is_some_and(|partition| partition.transfer_pending))
            },
            || {
                let shards = self.world_shards_for_absence()?;
                Ok(shards
                    .into_iter()
                    .enumerate()
                    .map(|(id, (_, coord))| MemberShard { id, coord })
                    .collect())
            },
        )
    }

    /// The member's live `game_world_entity` row, its occupied aura slots, and its live pet —
    /// everything [`crate::world::MemberShardCache::member_entity`] needs, read from this one
    /// shard. `shard` is this connection's `ShardId` in `WorldView`'s `AuraIndex`.
    fn member_entity(
        &self,
        shard: crate::stdb::world_index::ShardId,
        guid: u64,
    ) -> Option<crate::codec::MemberEntity> {
        let live = self.0.coord();
        let entity = live.conn.db.game_world_entity().guid().find(&guid)?;
        Some(crate::codec::MemberEntity {
            health: entity.health,
            max_health: entity.max_health,
            power: entity.power,
            max_power: entity.max_power,
            unit_bytes_0: entity.unit_bytes_0,
            level: entity.level,
            zone_id: entity.zone_id,
            x: entity.x,
            y: entity.y,
            dead: entity.dead,
            player_flags: entity.player_flags,
            auras: self.member_aura_slots(shard, guid),
            pet: self.member_pet(shard, guid),
        })
    }

    /// The Transfer Intent table is bounded by the Module's writer Gate, so the scan is short.
    fn member_between_places(&self, guid: u64) -> bool {
        let live = self.0.coord();
        let db = &live.conn.db;
        let session_online = db
            .game_character()
            .guid()
            .find(&guid)
            .is_some_and(|character| character.online);
        session_online
            || db
                .game_bot_transfer_intent()
                .iter()
                .any(|intent| intent.bot_guid == guid)
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
    fn member_pet(
        &self,
        shard: crate::stdb::world_index::ShardId,
        owner_guid: u64,
    ) -> Option<crate::codec::MemberPetEntity> {
        let live = self.0.coord();
        let db = &live.conn.db;
        let pet = db
            .game_world_entity()
            .iter()
            .find(|entity| entity.owner_guid == owner_guid)?;
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

/// One connected World Shard paired with its `ShardId`, so [`crate::world::MemberShardCache`] can
/// read the right shard's `AuraIndex` rows without widening that trait with a storage-layer
/// index every other implementor (the Store Fake, the pure unit tests) would have to carry too.
struct MemberShard {
    id: crate::stdb::world_index::ShardId,
    coord: Coordinator,
}

impl crate::world::MemberShardCache for MemberShard {
    fn member_entity(&self, guid: u64) -> Option<crate::codec::MemberEntity> {
        self.coord.member_entity(self.id, guid)
    }

    fn member_between_places(&self, guid: u64) -> bool {
        self.coord.member_between_places(guid)
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

    /// The Coordinator reads the caches and leaves every decision to `locate_member`, which the
    /// Member Stats tests drive. Offline must come from the absence rule, and a bot's Transfer
    /// Intent must count as a crossing, because a sessionless bot is never online.
    #[test]
    fn the_coordinator_feeds_the_shared_decision_with_the_absence_rule() {
        let presence = flat("pub(crate) fn member_presence(");
        assert!(presence.contains("crate::world::locate_member(guid,&connected,"));
        assert!(presence.contains("self.world_shards_for_absence()?"));
        assert!(presence.contains(".realm_character_partition(guid)?"));

        let between = flat("fn member_between_places(");
        assert!(between.contains(".is_some_and(|character|character.online)"));
        assert!(between.contains(
            "session_online||db.game_bot_transfer_intent().iter().any(|intent|intent.bot_guid==guid)"
        ));
    }

    /// `member_entity` and `member_pet` must read auras from the SAME `ShardId` the live match
    /// came from, not a fixed or default one — a cross-shard aura read would silently show
    /// nothing outside the Gateway process that owns that shard's connection.
    #[test]
    fn member_entity_and_pet_read_auras_from_the_matched_shard() {
        let entity = flat("fn member_entity(");
        assert!(entity.contains("self.member_aura_slots(shard,guid)"));
        assert!(entity.contains("self.member_pet(shard,guid)"));

        let pet = flat("fn member_pet(");
        assert!(pet.contains("self.member_aura_slots(shard,pet.guid)"));
    }
}
