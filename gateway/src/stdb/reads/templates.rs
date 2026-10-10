//! Static-template cache-accessor methods (creature/gameobject/item entries) — pure
//! code-motion split of the former `reads.rs`.

use anyhow::Result;

use super::super::bindings::*;
use super::super::connection::Coordinator;

impl Coordinator {
    /// Read a gameobject template by entry for a `CMSG_GAMEOBJECT_QUERY` reply. A Meeting Stone's
    /// `data0..data2` come from its stone row, the only copy of its level range and dungeon area,
    /// so the client tooltip shows the range.
    pub fn gameobject_template(
        &self,
        entry: u32,
    ) -> Result<Option<crate::codec::GameObjectTemplateView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db.game_gameobject_template().entry().find(&entry).map(|t| {
            let stone = (t.type_id == lyracore_shared::constants::go_type::MEETINGSTONE)
                .then(|| db.game_meeting_stone().entry().find(&entry))
                .flatten();
            let (data0, data1, data2) = match stone {
                Some(stone) => (stone.min_level, stone.max_level, stone.area_id),
                None => (t.data_0, t.data_1, 0),
            };
            crate::codec::GameObjectTemplateView {
                type_id: t.type_id,
                display_id: t.display_id,
                name: t.name,
                data0,
                data1,
                data2,
            }
        }))
    }
}
