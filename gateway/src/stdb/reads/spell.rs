//! Spell/spellbook cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use super::super::bindings::*;

// --- quest read helpers (free fns over the cache, shared by the methods above) ---------------------
// NOTE: game_creature_quest / game_quest_objective / game_quest_reward_item / game_character_quest only
// expose their `id` PK index via the SDK, so these scan with iter().filter() (not a secondary-index
// find). game_quest_template / game_quest_text / game_item_template / game_creature_template DO have a
// unique-PK index, so those use .entry()/.quest_entry().find() (point lookups).

/// cmangos `canStackSpellRanksInSpellBook`: MANA spells (power_type 0) keep EVERY rank
/// visible in the client book — downranking is a real caster mechanic; PASSIVES and
/// non-mana-cost spells (rage/energy/health) SUPERSEDE their old rank. Unknown spell → stack
/// (never hide a rank on missing data). LOCKSTEP: SPELL_ATTR_PASSIVE = 0x40 (module taxonomy).
pub(in crate::stdb) fn spell_ranks_stack_in_book(db: &RemoteTables, spell_id: u32) -> bool {
    const SPELL_ATTR_PASSIVE: u32 = 0x40;
    const POWER_MANA: u8 = 0;
    match db.game_spell().spell_id().find(&spell_id) {
        Some(h) => h.attributes & SPELL_ATTR_PASSIVE == 0 && h.power_type == POWER_MANA,
        None => true,
    }
}
