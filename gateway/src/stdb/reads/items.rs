//! Item-instance / loot cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;
use lyracore_shared::item_property::client_enchantment_id;

impl Coordinator {
    /// Read every item a character owns, joined with its template for the CREATE
    /// descriptors (max-durability). Read from the privileged cache (the coordinator bypasses RLS),
    /// filtered by `owner_guid` — the SDK exposes only the PK index, so iterate+filter like the other
    /// row queries. Returns the instance views ready for `build_item_create_object` + inventory slots.
    /// The character's learned skills as `(skill_line, current, max_rank)` — the self-CREATE
    /// SkillInfo block's live source. RLS-bypassed cache read like the sibling item read.
    pub fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_skill()
            .iter()
            .filter(|s| s.character_guid == character_guid)
            .map(|s| (s.skill_line, s.current, s.max_rank))
            .collect())
    }

    pub fn player_items(&self, owner_guid: u64) -> Result<Vec<crate::codec::ItemInstanceView>> {
        let mut items = self.player_item_rows(owner_guid);
        self.project_charter_petitions(&mut items);
        Ok(items)
    }

    fn player_item_rows(&self, owner_guid: u64) -> Vec<crate::codec::ItemInstanceView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        db.game_item_instance()
            .iter()
            .filter(|i| i.owner_guid == owner_guid)
            // Skip an instance whose template is absent: a delete-template migration can leave orphaned rows
            // (e.g. synthetic ids deleted by the real-item remap), and sending a phantom item — blank/unknown
            // to the client, faulting on use/equip — is worse than dropping it. Defense-in-depth so a future
            // template delete can never push a broken item to a 5875 client.
            .filter_map(|i| {
                let tmpl = db.game_item_template().entry().find(&i.entry)?;
                Some(crate::codec::ItemInstanceView {
                    max_durability: tmpl.max_durability,
                    container_slots: tmpl.container_slots,
                    random_property_enchant_ids: property_enchant_ids(db, i.random_property_id),
                    ..view_of_item_row(&i)
                })
            })
            .collect()
    }

    /// Bag slot of the item instance with `item_guid`. Reads from the coordinator's privileged cache
    /// (same source as `player_items`). Item GUIDs are globally unique so account_id isn't needed
    /// for the lookup — ownership is enforced by the module reducer on the call.
    pub fn item_slot_by_guid(&self, _account_id: u64, item_guid: u64) -> Option<u8> {
        // `collect()` forces the iterator to complete (and drop its borrow from `guard`) before `guard`
        // itself drops — same pattern as `player_items`.
        let guard = self.0.coord();
        let slots: Vec<u8> = guard
            .conn
            .db
            .game_item_instance()
            .iter()
            .filter(|i| i.guid == item_guid)
            .map(|i| i.slot)
            .collect();
        slots.into_iter().next()
    }
}

/// The view fields an item row holds itself, without template or catalogue reads. The enchantment
/// is the ID the client resolves, so no send path ships a stored compatibility ID.
pub(crate) fn view_of_item_row(row: &ItemInstance) -> crate::codec::ItemInstanceView {
    crate::codec::ItemInstanceView {
        guid: row.guid,
        entry: row.entry,
        owner_guid: row.owner_guid,
        slot: row.slot,
        stack_count: row.stack_count,
        durability: row.durability,
        random_property_id: row.random_property_id,
        item_text_id: row.item_text_id,
        enchantment: client_enchantment_id(row.enchant_id),
        ..Default::default()
    }
}

/// A Random Property's enchant ids from the cached catalogue; `[0; 3]` for property 0 or an
/// unknown property.
pub(crate) fn property_enchant_ids(db: &RemoteTables, random_property_id: u32) -> [u32; 3] {
    db.game_item_random_property()
        .property_id()
        .find(&random_property_id)
        .map_or([0; 3], |property| {
            [
                property.enchant_id_1,
                property.enchant_id_2,
                property.enchant_id_3,
            ]
        })
}
