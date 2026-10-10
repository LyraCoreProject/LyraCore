//! `Coordinator`'s [`NpcStore`] adapter.

use anyhow::Result;
use spacetimedb_sdk::Table;

use crate::codec;
use crate::codec::PetNameView;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, NpcStore};

impl NpcStore for Coordinator {
    /// Standing-derived reaction gate. The inherent read has other callers (vendor, guild, quest).
    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool> {
        Coordinator::npc_refuses_interaction(self, npc_guid, player_guid)
    }

    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>> {
        Coordinator::gameobject_template(self, entry)
    }

    /// Resolve the active title text for one NPC, with a per-creature menu override before the
    /// entry-owned default menu.
    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32 {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let entity = match db.game_world_entity().guid().find(&npc_guid) {
            Some(entity) => entity,
            None => return crate::codec::GOSSIP_GREETING_TEXT_ID,
        };
        if let Some(menu_id) = active_gossip_menu_id(db, &entity) {
            if let Some(menu) = db.game_gossip_menu_profile().menu_id().find(&menu_id) {
                return menu.text_id;
            }
        }
        match db.game_gossip_menu().entry().find(&entity.entry) {
            Some(row) => row.text_id,
            None => crate::codec::GOSSIP_GREETING_TEXT_ID,
        }
    }

    /// Look up the full weighted greeting for a `text_id`: `game_npc_text` (slot 0
    /// male, back-compat) + any `game_npc_text_slot` rows for the same id. Returns `None` when the
    /// text_id has no `game_npc_text` row at all (the gateway falls back to the generic greeting).
    ///
    /// Back-compat normalization: a `text_id` with NO `game_npc_text_slot` rows (never imported by the
    /// weighted-greeting importer — either a pre-existing row, or the legacy `npc_text[entry]`
    /// fallback path, which only ever populates `game_npc_text`) is read as slot 0 = `(text, text,
    /// 1.0)` and every other slot silent, matching the old single-slot behavior byte-for-byte. When
    /// slot rows DO exist, they
    /// are used verbatim (real per-slot probabilities from the dump) and slot 0's base-row `text` is
    /// NOT separately re-applied (the slot-0 row, always emitted by the importer alongside the others,
    /// is the source of truth once any slot row exists).
    fn npc_text_for_id(&self, text_id: u32) -> Option<crate::codec::NpcTextView> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let base = db.game_npc_text().text_id().find(&text_id)?;
        let mut slot_rows: Vec<_> = db
            .game_npc_text_slot()
            .iter()
            .filter(|s| s.text_id == text_id)
            .collect();
        if slot_rows.is_empty() {
            let mut view = crate::codec::NpcTextView::default();
            view.slots[0] = (base.text.clone(), base.text, 1.0);
            return Some(view);
        }
        slot_rows.sort_by_key(|s| s.slot_index); // stable slot order (SQL has no ORDER BY in 2.5)
        let mut view = crate::codec::NpcTextView::default();
        for row in slot_rows {
            if (row.slot_index as usize) < 8 {
                view.slots[row.slot_index as usize] =
                    (row.text_male, row.text_female, row.probability);
            }
        }
        Some(view)
    }

    /// Return the active imported options for one NPC. A live per-creature override replaces the
    /// entry-owned default options. The dispatcher applies conditions at both HELLO and SELECT.
    fn gossip_options(&self, npc_guid: u64) -> Result<Vec<crate::codec::GossipOptionView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entity) = db.game_world_entity().guid().find(&npc_guid) else {
            return Ok(Vec::new());
        };
        if let Some(menu_id) = active_gossip_menu_id(db, &entity) {
            if db
                .game_gossip_menu_profile()
                .menu_id()
                .find(&menu_id)
                .is_some()
            {
                let mut rows: Vec<_> = db
                    .game_gossip_menu_profile_option()
                    .iter()
                    .filter(|option| option.menu_id == menu_id)
                    .collect();
                rows.sort_by_key(|option| option.option_index);
                return Ok(rows
                    .into_iter()
                    .map(|option| crate::codec::GossipOptionView {
                        row_id: option.row_id,
                        icon: option.icon,
                        text: option.text,
                        action: option.action,
                        action_menu_id: option.action_menu_id,
                        cond_type: option.cond_type,
                        cond_value1: option.cond_value1,
                        cond_value2: option.cond_value2,
                    })
                    .collect());
            }
        }
        let mut rows: Vec<_> = db
            .game_gossip_option()
            .iter()
            .filter(|option| option.entry == entity.entry)
            .collect();
        rows.sort_by_key(|option| option.option_index);
        Ok(rows
            .into_iter()
            .map(|option| crate::codec::GossipOptionView {
                row_id: option.row_id,
                icon: option.icon,
                text: option.text,
                action: option.action,
                action_menu_id: option.action_menu_id,
                cond_type: option.cond_type,
                cond_value1: option.cond_value1,
                cond_value2: option.cond_value2,
            })
            .collect())
    }

    /// Does the NPC at `guid` carry the innkeeper flag? Gates the "Make this inn your home." gossip
    /// option + the `bind_home` select. Reads `npc_flags` off the entity (privileged cache); absent → false.
    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_world_entity()
            .guid()
            .find(&guid)
            .is_some_and(|e| e.npc_flags & lyracore_shared::constants::npc_flags::INNKEEPER != 0))
    }

    /// Resolve a live pet visible to the world. Pet names are public unit presentation: Hunter
    /// names come from the bounded durable projection, while summoned-pet names remain authored
    /// creature-template data. The requester must be in world, but need not own the observed pet.
    fn pet_name(
        &self,
        _requester: Actor,
        pet_number: u32,
        pet_guid: u64,
    ) -> Result<Option<PetNameView>, anyhow::Error> {
        if pet_guid == 0 {
            return Ok(None);
        }
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entity) = db.game_world_entity().guid().find(&pet_guid) else {
            return Ok(None);
        };
        if entity.owner_guid == 0 {
            return Ok(None);
        }
        if let Some(pet) = db
            .game_hunter_pet_protocol()
            .iter()
            .find(|pet| pet.live_pet_guid == pet_guid)
        {
            return Ok(resolve_pet_name(
                pet_number,
                pet_guid,
                Some(super::super::views::hunter_pet_protocol_view(pet)),
                None,
            ));
        }
        let summoned = db
            .game_creature_template()
            .entry()
            .find(&entity.entry)
            .map(|template| template.name);
        Ok(resolve_pet_name(pet_number, pet_guid, None, summoned))
    }

    /// Read a creature template by entry for a `CMSG_CREATURE_QUERY` reply (Tier 2 / NPCs).
    fn creature_template(&self, entry: u32) -> Result<Option<crate::codec::CreatureView>> {
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

    /// The `type_id` of a SPAWNED gameobject, by its live guid (join `game_gameobject` →
    /// `game_gameobject_template`). Feeds the `CMSG_GAMEOBJ_USE` dispatch: a
    /// `lyracore_shared::constants::go_type::QUESTGIVER` GO (the Wanted Poster, the Lost Guards corpses)
    /// opens the quest window instead of rolling loot / toggling state — that is what a questgiver
    /// gameobject does in vanilla. `None` for an unspawned/unknown guid (the caller falls back to the
    /// ordinary use-reducer path, which itself no-ops on an unknown guid).
    fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>> {
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

    /// Validate a `CMSG_INSPECT` request (target is a real in-world player, on the caller's map, in
    /// range, friendly) over the coordinator connection so the module resolves the caller from
    /// `ctx.sender`. `Err` (out of range / hostile / no such target) → the caller ignores it.
    fn inspect(&self, actor: Actor, target_guid: u64) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_inspect",
            gw_inspect_then(self.session_actor(actor), target_guid)
        )
    }

    /// Enter an area trigger (`CMSG_AREATRIGGER`) — credit any active explore quest tied to `trigger_id`.
    fn enter_areatrigger(&self, actor: Actor, trigger_id: u32) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_enter_areatrigger",
            gw_enter_areatrigger_then(self.session_actor(actor), trigger_id)
        )
    }

    /// `CMSG_GOSSIP_SELECT_OPTION` — the NOTIFY-ONLY module chokepoint. Fired
    /// best-effort BEFORE the gateway's own gossip behavior; a failure never blocks the reply.
    fn gossip_select(
        &self,
        actor: Actor,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_gossip_select",
            gw_gossip_select_then(
                self.session_actor(actor),
                npc_guid,
                option_id,
                option_row_id
            )
        )
    }

    /// Bind the caller's hearthstone home to their current position (`CMSG_GOSSIP_SELECT_OPTION` on an
    /// innkeeper's "Make this inn your home.") over the coordinator connection so the module attributes
    /// it to the caller's entity. No args — `bind_home` resolves the caller via `ctx.sender`.
    fn bind_home(&self, actor: Actor) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_bind_home",
            gw_bind_home_then(self.session_actor(actor))
        )
    }
}

fn active_gossip_menu_id(db: &RemoteTables, entity: &WorldEntity) -> Option<u32> {
    let menu = db
        .game_creature_gossip_menu_override()
        .creature_guid()
        .find(&entity.guid);
    matching_gossip_menu_id(
        entity.guid,
        entity.map_id,
        entity.instance_id,
        menu.as_ref(),
    )
}

fn matching_gossip_menu_id(
    creature_guid: u64,
    map_id: u32,
    instance_id: u64,
    menu: Option<&CreatureGossipMenuOverride>,
) -> Option<u32> {
    menu.filter(|menu| {
        menu.creature_guid == creature_guid
            && menu.map_id == map_id
            && menu.instance_id == instance_id
    })
    .map(|menu| menu.menu_id)
}

fn resolve_pet_name(
    pet_number: u32,
    pet_guid: u64,
    hunter: Option<crate::codec::HunterPetProtocolView>,
    summoned_name: Option<String>,
) -> Option<PetNameView> {
    if let Some(pet) = hunter {
        return (pet.live_pet_guid == pet_guid && pet.pet_id as u32 == pet_number).then_some(
            PetNameView {
                pet_number,
                name: pet.name,
                name_timestamp: pet.name_timestamp,
            },
        );
    }
    Some(PetNameView {
        pet_number,
        name: summoned_name?,
        name_timestamp: 0,
    })
}

#[cfg(test)]
mod gossip_tests {
    use super::*;

    #[test]
    fn gossip_override_is_scoped_to_the_exact_creature_instance() {
        let menu = CreatureGossipMenuOverride {
            creature_guid: 41,
            menu_id: 6687,
            map_id: 43,
            instance_id: 9,
        };

        assert_eq!(matching_gossip_menu_id(41, 43, 9, Some(&menu)), Some(6687));
        assert_eq!(matching_gossip_menu_id(42, 43, 9, Some(&menu)), None);
        assert_eq!(matching_gossip_menu_id(41, 43, 10, Some(&menu)), None);
        assert_eq!(matching_gossip_menu_id(41, 44, 9, Some(&menu)), None);
    }
}

#[cfg(test)]
mod pet_name_tests {
    use super::*;

    fn hunter(owner_guid: u64) -> crate::codec::HunterPetProtocolView {
        crate::codec::HunterPetProtocolView {
            pet_id: 77,
            owner_guid,
            live_pet_guid: 99,
            creature_entry: 3098,
            name: "Mottled Boar".into(),
            name_timestamp: 123,
            level: 8,
            pet_xp: 0,
            next_level_xp: 4_500,
            happiness: 166_500,
            loyalty_level: 1,
        }
    }

    #[test]
    fn hunter_name_requires_pet_guid_and_number_but_not_the_observers_guid() {
        let got = resolve_pet_name(77, 99, Some(hunter(7)), None).unwrap();
        assert_eq!(got.name, "Mottled Boar");
        assert_eq!(got.name_timestamp, 123);
        assert!(resolve_pet_name(78, 99, Some(hunter(7)), None).is_none());
        assert!(resolve_pet_name(77, 100, Some(hunter(7)), None).is_none());
    }

    #[test]
    fn summoned_pet_uses_authored_name_without_hunter_state() {
        let got = resolve_pet_name(0, 99, None, Some("Imp".into())).unwrap();
        assert_eq!(got.name, "Imp");
        assert_eq!(got.name_timestamp, 0);
    }
}
