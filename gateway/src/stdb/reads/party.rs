//! Party/group + loot-roll cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;

impl Coordinator {
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
