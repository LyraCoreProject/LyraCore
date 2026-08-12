//! NPC interaction cache-accessor methods (gossip/vendor/trainer) — pure code-motion split
//! of the former `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;

impl Coordinator {
    /// Look up the `title_text_id` to put in `SMSG_GOSSIP_MESSAGE` for the given NPC. Resolves:
    /// entity.guid → entity.entry → `game_gossip_menu.entry` → `game_gossip_menu.text_id`.
    /// Falls back to `GOSSIP_GREETING_TEXT_ID = 1` when no row is found (generic greeting).
    pub fn npc_gossip_text_id(&self, npc_guid: u64) -> u32 {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let entry = match db.game_world_entity().guid().find(&npc_guid) {
            Some(e) => e.entry,
            None => return crate::codec::GOSSIP_GREETING_TEXT_ID,
        };
        match db.game_gossip_menu().entry().find(&entry) {
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
    pub fn npc_text_for_id(&self, text_id: u32) -> Option<crate::codec::NpcTextView> {
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

    /// The imported gossip menu options for the NPC at `guid`, sorted by
    /// `option_index` (the render/select order) — RAW, unfiltered by condition; the dispatcher applies
    /// [`crate::codec::option_condition_holds`] identically at HELLO and SELECT_OPTION. Empty when the
    /// creature has no imported options (the gateway falls back to the flag-derived synthesis).
    pub fn gossip_options(&self, npc_guid: u64) -> Result<Vec<crate::codec::GossipOptionView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entry) = db
            .game_world_entity()
            .guid()
            .find(&npc_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let mut rows: Vec<_> = db
            .game_gossip_option()
            .iter()
            .filter(|o| o.entry == entry)
            .collect();
        rows.sort_by_key(|o| o.option_index);
        Ok(rows
            .into_iter()
            .map(|o| crate::codec::GossipOptionView {
                row_id: o.row_id,
                icon: o.icon,
                text: o.text,
                action: o.action,
                action_menu_id: o.action_menu_id,
                cond_type: o.cond_type,
                cond_value1: o.cond_value1,
                cond_value2: o.cond_value2,
            })
            .collect())
    }

    /// `(taken, rewarded)` for `quest_id` in `guid`'s quest log (privileged read — RLS bypassed, like
    /// `quest_giver_evals`) — feeds `option_condition_holds` for the QUEST_TAKEN/QUEST_REWARDED gossip
    /// option conditions. `taken` is true whenever a log row exists at all (accepted,
    /// whether or not yet turned in); a quest never seen by this player is `(false, false)`.
    pub fn quest_status(&self, guid: u64, quest_id: u32) -> (bool, bool) {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let found = db
            .game_character_quest()
            .iter()
            .find(|q| q.character_guid == guid && q.quest_entry == quest_id)
            .map(|row| row.rewarded);
        match found {
            Some(rewarded) => (true, rewarded),
            None => (false, false),
        }
    }

    /// Does the NPC at `guid` carry the innkeeper flag? Gates the "Make this inn your home." gossip
    /// option + the `bind_home` select. Reads `npc_flags` off the entity (privileged cache); absent → false.
    pub fn npc_is_innkeeper(&self, guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_world_entity()
            .guid()
            .find(&guid)
            .is_some_and(|e| e.npc_flags & lyracore_shared::constants::npc_flags::INNKEEPER != 0))
    }

    /// Read a vendor's stock for `SMSG_LIST_INVENTORY` (Tier 2 / vendors). Resolve the vendor's
    /// creature entry from its `game_world_entity` row, filter `game_npc_vendor` to that entry, and
    /// join each `game_item_template` for the display id / buy price / max durability (carrying the
    /// npc_vendor row's `max_count`). Read from the privileged cache (the coordinator bypasses RLS);
    /// the SDK exposes only the PK index, so iterate+filter like the other row queries. Items whose
    /// template isn't loaded are skipped (we can't price/display them). Ordered by the vendor `slot`.
    pub fn vendor_items(&self, vendor_guid: u64) -> Result<Vec<crate::codec::VendorItemView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        // The vendor's creature entry drives the stock lookup; a missing entity → no stock.
        let Some(creature_entry) = db
            .game_world_entity()
            .guid()
            .find(&vendor_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let mut rows: Vec<_> = db
            .game_npc_vendor()
            .iter()
            .filter(|v| v.creature_entry == creature_entry)
            .collect();
        rows.sort_by_key(|v| v.slot); // stable vendor-slot order (SQL has no ORDER BY in 2.5)
        let items = rows
            .into_iter()
            .filter_map(|v| {
                // Skip a stock line whose template isn't loaded — we can't price or display it.
                db.game_item_template()
                    .entry()
                    .find(&v.item_entry)
                    .map(|t| crate::codec::VendorItemView {
                        item_entry: v.item_entry,
                        display_id: t.display_id,
                        buy_price: t.buy_price,
                        max_durability: t.max_durability,
                        max_count: v.max_count,
                        buy_count: t.buy_count,
                    })
            })
            .collect();
        Ok(items)
    }

    /// Does this trainer serve `player_guid`'s class? Fail-open on any missing read (trainer,
    /// character, template), matching `npc_refuses_interaction`.
    pub fn trainer_serves(&self, player_guid: u64, trainer_guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let (Some(trainer), Some(player)) = (
            db.game_world_entity().guid().find(&trainer_guid),
            db.game_character().guid().find(&player_guid),
        ) else {
            return Ok(true);
        };
        Ok(db
            .game_creature_template()
            .entry()
            .find(&trainer.entry)
            .is_none_or(|t| {
                lyracore_shared::trainer::serves(player.class, t.trainer_type, t.trainer_class)
            }))
    }

    /// The spells trainer `trainer_guid` teaches, folded with `player_guid`'s level + known-state so the
    /// codec can render each Green/Red/Gray. The trainer's creature-template `entry` keys the list (like
    /// the vendor stock); `known` = a `game_player_spell` row (the one castability source). A missing
    /// trainer/player → empty list. Read from the privileged cache (coordinator bypasses RLS).
    pub fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<crate::codec::TrainerSpellView>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(trainer_entry) = db
            .game_world_entity()
            .guid()
            .find(&trainer_guid)
            .map(|e| e.entry)
        else {
            return Ok(Vec::new());
        };
        let Some(player) = db.game_world_entity().guid().find(&player_guid) else {
            return Ok(Vec::new());
        };
        let learned: std::collections::HashSet<u32> = db
            .game_player_spell()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| s.spell_id)
            .collect();
        // The player's trained skill caps, by line — for the per-row "known" of a PROFESSION offering
        // (game_player_spell never holds a profession id, so we gray a tier on the skill cap, not the
        // spellbook). RLS-bypassed via the coordinator cache, same as the reads above.
        let skill_caps: std::collections::HashMap<u32, u32> = db
            .game_player_skill()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| (s.skill_line, s.max_rank as u32))
            .collect();
        let mut rows: Vec<_> = db
            .game_trainer_spell()
            .iter()
            // Profession-learn offerings (learn_skill_line != 0) are now SHOWN: the importer synthesizes
            // them with REAL Spell.dbc learn ids (2575 Mining, 2366 Herbalism, …), so the 5875 client
            // resolves the name/icon + emits a valid SMSG_LEARNED_SPELL — unlike the old 50080-88 markers
            // this filter used to hide. The buy still routes to crate::skill::learn_profession (module
            // unchanged). The per-row `known` below grays tiers the player has already trained past.
            .filter(|t| t.trainer_entry == trainer_entry)
            .collect();
        rows.sort_by_key(|t| t.spell_id); // stable order (SQL has no ORDER BY in 2.5)
                                          // A class-spell offering id is a LearnSpell WRAPPER (1873 teaches 639); game_player_spell holds
                                          // the RESOLVED rank, so `known` must compare the resolved id or every known spell reads Green.
                                          // Same first-qualifying-trigger rule as resolve_learn_target (can't call it: coord() re-lock);
                                          // one pass over the effect table for the whole list instead of a scan per row.
        const EXCLUDED: [u8; 4] = [0x93, 0xBE, 0xAB, 0x05];
        let wrapper_ids: std::collections::HashSet<u32> = rows
            .iter()
            .filter(|t| t.learn_skill_line == 0)
            .map(|t| t.spell_id)
            .collect();
        let mut resolved: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for e in db.game_spell_effect().iter() {
            if wrapper_ids.contains(&e.spell_id)
                && e.trigger_spell != 0
                && !EXCLUDED.contains(&e.kind)
            {
                resolved.entry(e.spell_id).or_insert(e.trigger_spell);
            }
        }
        Ok(rows
            .into_iter()
            .map(|t| crate::codec::TrainerSpellView {
                spell_id: t.spell_id,
                cost: t.cost,
                required_level: t.required_level,
                player_level: player.level,
                // PROFESSION row: "known" iff the player's skill cap for this line already meets the
                // offering's cap (mirrors module trainer_buy_check's gate → the client grays the tiers
                // already trained past). CLASS-SPELL row (line 0): known iff a game_player_spell row
                // for the RESOLVED rank behind the wrapper (mirrors the module buy gate's knows_spell).
                known: if t.learn_skill_line != 0 {
                    skill_caps
                        .get(&t.learn_skill_line)
                        .is_some_and(|&c| c >= t.learn_skill_cap)
                } else {
                    learned.contains(resolved.get(&t.spell_id).unwrap_or(&t.spell_id))
                },
                profession: t.learn_skill_line != 0,
            })
            .collect())
    }
}
