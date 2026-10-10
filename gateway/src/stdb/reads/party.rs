//! Party/group + loot-roll cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use super::super::bindings::*;
use super::super::connection::Coordinator;
use lyracore_shared::group::{GroupKind, RaidSlot};

impl Coordinator {
    /// The party `character_guid` belongs to, read from THIS handle's database.
    ///
    /// Which database the handle points at is the whole meaning of the answer: on the **realm-core**
    /// handle this is the AUTHORITATIVE roster, and on a world shard it is that shard's mirror of it
    /// (`group::sync_group_mirror`). Nothing here knows or cares which — routing is the caller's job,
    /// exactly as it is for every other read in this file.
    ///
    /// A cache read through the membership index, so it is cheap enough to run inside an SDK
    /// callback (which the realm-core group relay does): no reducer call, no round trip, no table
    /// scan.
    pub fn group_roster(&self, character_guid: u64) -> Option<crate::world::party::GroupRoster> {
        let group_id = self
            .0
            .coord()
            .party_memberships
            .read()
            .unwrap()
            .group_of(character_guid)?;
        self.group_roster_by_id(group_id)
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
        let group = db.game_group().group_id().find(&group_id)?;
        let row_ids = guard
            .party_memberships
            .read()
            .unwrap()
            .member_row_ids(group_id);
        let mut rows: Vec<(u64, u64, u8)> = row_ids
            .into_iter()
            .filter_map(|row_id| db.game_group_member().id().find(&row_id))
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
            kind: GroupKind::from_wire_or_default(group.group_type),
            members: rows
                .into_iter()
                .map(|(_, guid, slot)| crate::world::party::GroupRosterMember {
                    guid,
                    slot: RaidSlot::from_wire_or_default(slot),
                })
                .collect(),
            partitions,
        })
    }
}
