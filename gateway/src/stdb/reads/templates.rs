//! Static-template cache-accessor methods (creature/gameobject/item entries) — pure
//! code-motion split of the former `reads.rs`.

use anyhow::Result;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use super::super::views::item_template_view;

impl Coordinator {
    /// Read a creature template by entry for a `CMSG_CREATURE_QUERY` reply (Tier 2 / NPCs).
    pub fn creature_template(&self, entry: u32) -> Result<Option<crate::codec::CreatureView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_creature_template()
            .entry()
            .find(&entry)
            .map(|t| crate::codec::CreatureView {
                entry: t.entry,
                name: t.name,
                subname: t.subname,
                display_id: t.display_id,
                creature_type: t.creature_type as u32,
                creature_family: t.creature_family,
                type_flags: t.type_flags,
                rank: t.rank as u32,
            }))
    }

    /// Read a gameobject template by entry for a `CMSG_GAMEOBJECT_QUERY` reply.
    pub fn gameobject_template(
        &self,
        entry: u32,
    ) -> Result<Option<crate::codec::GameObjectTemplateView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_gameobject_template()
            .entry()
            .find(&entry)
            .map(|t| crate::codec::GameObjectTemplateView {
                type_id: t.type_id,
                display_id: t.display_id,
                name: t.name,
                data0: t.data_0,
                data1: t.data_1,
            }))
    }

    /// The `type_id` of a SPAWNED gameobject, by its live guid (join `game_gameobject` →
    /// `game_gameobject_template`). Feeds the `CMSG_GAMEOBJ_USE` dispatch: a
    /// `lyracore_shared::constants::go_type::QUESTGIVER` GO (the Wanted Poster, the Lost Guards corpses)
    /// opens the quest window instead of rolling loot / toggling state — that is what a questgiver
    /// gameobject does in vanilla. `None` for an unspawned/unknown guid (the caller falls back to the
    /// ordinary use-reducer path, which itself no-ops on an unknown guid).
    pub fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>> {
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
            .map(|t| t.type_id))
    }

    /// Read an item template by entry for a `CMSG_ITEM_QUERY_SINGLE` reply (items slice-1).
    pub fn item_template(&self, entry: u32) -> Result<Option<crate::codec::ItemTemplateView>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_item_template()
            .entry()
            .find(&entry)
            .map(item_template_view))
    }
}
