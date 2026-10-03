//! Meeting Stone reads: stone rows on the Home Shard, Seeker rows on the party authority.

use anyhow::Result;
use lyracore_shared::constants::go_type;

use super::super::connection::Coordinator;
use super::*;

impl Coordinator {
    /// The dungeon area of the Meeting Stone spawned as `go_guid` on this database, if any.
    pub(crate) fn meeting_stone_area(&self, go_guid: u64) -> Result<Option<u32>> {
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
            .filter(|template| template.type_id == go_type::MEETINGSTONE)
            .and_then(|template| db.game_meeting_stone().entry().find(&template.entry))
            .map(|stone| stone.area_id))
    }

    /// The area `character_guid` is queued for, read from the party authority.
    pub(crate) fn queued_area(&self, character_guid: u64) -> Result<Option<u32>> {
        let realm = self.realm_core()?;
        let guard = realm.0.coord();
        Ok(guard
            .conn
            .db
            .game_meeting_stone_seeker()
            .character_guid()
            .find(&character_guid)
            .map(|seeker| seeker.area_id))
    }
}
