//! Spell/spellbook cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;

impl Coordinator {
    /// The spell's cast time (ms) from the static `game_spell` header — 0 = instant. Used by the
    /// CMSG_CAST_SPELL handler to clear an instant cast SYNCHRONOUSLY (the async relay delivers START/GO
    /// after the aura effects, which wedges the 5875 client's cast slot). None = unknown spell.
    pub fn spell_cast_time(&self, spell_id: u32) -> Option<u32> {
        self.0
            .coord()
            .conn
            .db
            .game_spell()
            .spell_id()
            .find(&spell_id)
            .map(|s| s.cast_time_ms)
    }

    /// Enchant/disenchant routing for `spell_id`, read off the spell's effect rows — `None` if the spell
    /// has no item-target enchanting effect (a normal cast). The gateway uses this to route ITEM-target
    /// casts by effect KIND, with the enchant id carried in the effect's `p_0`, instead of a hardcoded
    /// spell-id list — a new enchant spell is a data row, no gateway change.
    pub fn enchant_route(&self, spell_id: u32) -> Option<crate::world::EnchantRoute> {
        use crate::world::EnchantRoute;
        const E_ENCHANT_ITEM: u8 = 0x17; // taxonomy E_ENCHANT_ITEM (p_0 = enchant_id)
        const E_DISENCHANT: u8 = 0x18; // taxonomy E_DISENCHANT
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .filter(|e| e.spell_id == spell_id)
            .find_map(|e| match e.kind {
                E_ENCHANT_ITEM => Some(EnchantRoute::Enchant(e.p_0 as u32)),
                E_DISENCHANT => Some(EnchantRoute::Disenchant),
                _ => None,
            })
    }

    /// True iff `spell_id` has a ground-area effect (taxonomy `E_PERSISTENT_AREA` 0x1B —
    /// Consecration etc.). The instant-cast GO for such a spell must carry an EMPTY hit list: the
    /// default self-cast fallback put the CASTER in `hits[]`, and the 5875 client rendered the
    /// spell's impact animation ON the paladin ("hit animation on the caster"). Kind-routed
    /// like `enchant_route` — a new ground spell is a data row.
    pub fn spell_is_ground_area(&self, spell_id: u32) -> bool {
        const E_PERSISTENT_AREA: u8 = 0x1B; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_PERSISTENT_AREA)
    }

    /// The character's active SPELL-MODIFIER auras as raw `(family_mask, op, amount, is_pct)`
    /// rows — the client-mirror source for SMSG_SET_FLAT/PCT_SPELL_MODIFIER (aggregation by
    /// (op, mask-bit) happens in the codec helper; mangos sends the TOTAL per bit).
    pub fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)> {
        const A_SPELLMOD_FLAT: u8 = 0xAC; // lockstep with module taxonomy
        const A_SPELLMOD_PCT: u8 = 0xAD;
        self.0
            .coord()
            .conn
            .db
            .game_aura()
            .iter()
            .filter(|a| {
                a.target_guid == character_guid
                    && (a.eff_kind == A_SPELLMOD_FLAT || a.eff_kind == A_SPELLMOD_PCT)
            })
            .map(|a| {
                (
                    a.eff_p1 as u32,
                    a.eff_p0 as u8,
                    a.amount,
                    a.eff_kind == A_SPELLMOD_PCT,
                )
            })
            .collect()
    }

    /// True iff `spell_id` is a FISHING cast (taxonomy `E_FISH` 0x1C — the three tier ids carry a
    /// synthesized marker effect row). Kind-routed like `enchant_route`/`spell_is_ground_area`.
    pub fn spell_is_fishing(&self, spell_id: u32) -> bool {
        const E_FISH: u8 = 0x1C; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_FISH)
    }

    /// True iff `spell_id` is an OPEN-LOCK cast (taxonomy `E_OPEN_LOCK` 0x1D — Pick Lock 1804). Routed to
    /// the `pick_lock` reducer, gateway-intercepted exactly like `spell_is_fishing`/`enchant_route`. The
    /// GO guid the pick targets rides the cast's SpellCastTargets (GAMEOBJECT flag).
    pub fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        const E_OPEN_LOCK: u8 = 0x1D; // lockstep with module taxonomy
        self.0
            .coord()
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_OPEN_LOCK)
    }

    /// The player's LEARNED spells — `game_player_spell` rows for this character (the coordinator
    /// bypasses RLS so it reads any player's). Chained into the login spellbook so a taught ability
    /// (Auto Shot) reaches the client and `CastSpellByName` can fire it.
    pub fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let known: Vec<u32> = db
            .game_player_spell()
            .iter()
            .filter(|s| s.character_guid == player_guid)
            .map(|s| s.spell_id)
            .collect();
        // 258 rank collapse: drop a known rank that another KNOWN spell supersedes (a game_spell_chain
        // row whose prev_spell is this id) — GATED on the same cmangos stacking rule as
        // superseded_old_rank (operator-corrected): MANA spells keep every rank in the book
        // (downranking Holy Light is a real thing); only non-mana/passive chains collapse
        // (Heroic Strike). One pass suffices: each superseded rank is prev of its own successor.
        let known_set: std::collections::HashSet<u32> = known.iter().copied().collect();
        let superseded: std::collections::HashSet<u32> = db
            .game_spell_chain()
            .iter()
            .filter(|c| {
                c.prev_spell != 0
                    && known_set.contains(&c.prev_spell)
                    && known_set.contains(&c.spell_id)
                    && !spell_ranks_stack_in_book(db, c.spell_id)
            })
            .map(|c| c.prev_spell)
            .collect();
        Ok(known
            .into_iter()
            .filter(|id| !superseded.contains(id))
            .collect())
    }

    /// The player's IMPORTED action-bar rows as `(button, action, action_type)` triples —
    /// `game_player_action` rows copied at character creation from `game_createinfo_action` (empty when
    /// no dump has been imported, the common case today). Chained into the login codec
    /// (`login_sequence_messages`), which builds the bar from these when non-empty and falls back to
    /// the spellbook synth otherwise. RLS-bypassed read, like `player_learned_spells`.
    pub fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_action()
            .iter()
            .filter(|a| a.character_guid == player_guid)
            .map(|a| (a.button, a.action, a.action_type))
            .collect())
    }

    /// Resolve a trainer offering's LEARN TARGET (the rank the buy actually granted) so
    /// SMSG_LEARNED_SPELL books the REAL spell, not the LearnSpell wrapper (live find 2026-07-11:
    /// "Devotion Aura appeared in my General tab as the spell that teaches Devotion Aura").
    /// LOCKSTEP with module trainer.rs::resolve_learn_target — the same first-qualifying-trigger
    /// rule; the excluded kinds are the module's taxonomy values (A_PERIODIC_TRIGGER 0x93,
    /// A_FLAG 0xBE, A_PROC_ON_HIT 0xAB, E_TRIGGER 0x05).
    /// True iff `spell_id` is an on-next-swing QUEUE spell (Heroic Strike/Cleave — any effect of kind
    /// E_NEXT_SWING). The CMSG_CAST_SPELL handler then sends NO synchronous START/CAST_RESULT/GO: the
    /// 5875 client lights the button locally on the press and holds it as a pending cast until the
    /// swing-fire GO arrives. Kind value LOCKSTEP with module taxonomy.rs::E_NEXT_SWING (0x13).
    pub fn spell_queues_next_swing(&self, spell_id: u32) -> bool {
        const E_NEXT_SWING: u8 = 0x13;
        let guard = self.0.coord();
        let queues = guard
            .conn
            .db
            .game_spell_effect()
            .iter()
            .any(|e| e.spell_id == spell_id && e.kind == E_NEXT_SWING);
        queues
    }

    /// True iff `spell_id` is an AUTO-REPEAT ranged attack (Auto Shot / wand Shoot) — the
    /// `SPELL_ATTR_RANGED_AUTO_REPEAT` cast_flags bit set by the importer (from the DBC AttributesEx2
    /// AUTOREPEAT bit / by name). The CMSG_CAST_SPELL handler routes on this instead of a hardcoded
    /// `spell == 75 || 5019` id list, so a new ranged auto-repeat ability onboards as data.
    pub fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        const SPELL_ATTR_RANGED_AUTO_REPEAT: u32 = 0x0200; // lockstep with importer::spell.rs
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_spell()
            .spell_id()
            .find(&spell_id)
            .is_some_and(|s| s.cast_flags & SPELL_ATTR_RANGED_AUTO_REPEAT != 0)
    }

    /// The KNOWN rank that learning `new_spell` supersedes: the game_spell_chain prev of
    /// `new_spell`, if the player knows it AND the spell's ranks don't stack in the book. Drives
    /// SMSG_SUPERCEDED_SPELL instead of LEARNED_SPELL on a trainer buy.
    ///
    /// The stacking rule is cmangos' canStackSpellRanksInSpellBook (operator-corrected live:
    /// downranking Holy Light is a real thing): MANA spells keep EVERY rank visible (casters
    /// downrank for mana efficiency); rage/energy/health-cost spells and PASSIVES supersede
    /// (Heroic Strike replaces its old rank — there is no "downranked HS").
    pub fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        if spell_ranks_stack_in_book(db, new_spell) {
            return None;
        }
        let prev = db
            .game_spell_chain()
            .spell_id()
            .find(&new_spell)
            .map(|c| c.prev_spell)?;
        if prev == 0 || prev == new_spell {
            return None;
        }
        let knows_prev = db
            .game_player_spell()
            .iter()
            .any(|s| s.character_guid == player_guid && s.spell_id == prev);
        knows_prev.then_some(prev)
    }

    pub fn resolve_learn_target(&self, spell_id: u32) -> u32 {
        const EXCLUDED: [u8; 4] = [0x93, 0xBE, 0xAB, 0x05];
        let guard = self.0.coord();
        let resolved = guard
            .conn
            .db
            .game_spell_effect()
            .iter()
            .filter(|e| e.spell_id == spell_id)
            .find_map(|e| {
                (e.trigger_spell != 0 && !EXCLUDED.contains(&e.kind)).then_some(e.trigger_spell)
            });
        resolved.unwrap_or(spell_id)
    }
}

// --- quest read helpers (free fns over the cache, shared by the methods above) ---------------------
// NOTE: game_creature_quest / game_quest_objective / game_quest_reward_item / game_character_quest only
// expose their `id` PK index via the SDK, so these scan with iter().filter() (not a secondary-index
// find). game_quest_template / game_quest_text / game_item_template / game_creature_template DO have a
// unique-PK index, so those use .entry()/.quest_entry().find() (point lookups).

/// Evaluate one giver↔quest relation against the player → a [`GiverQuestEval`], or `None` if the quest
/// template isn't loaded. PARITY: the `startable` predicate MUST stay identical to the module's
/// `apply_accept_quest` accept gate (`module/src/quest.rs`) — a fresh START relation with level + race +
/// class met and the prerequisite quest turned in — else the `!` icon shows a quest the reducer rejects.
/// Sum a player's held quantity of item `entry` over `game_item_instance` (the coordinator reads any
/// player's items — RLS-bypassed, like the quest log). The gateway twin of the module's `items::item_count`.
/// cmangos `canStackSpellRanksInSpellBook`: MANA spells (power_type 0) keep EVERY rank
/// visible in the client book — downranking is a real caster mechanic; PASSIVES and
/// non-mana-cost spells (rage/energy/health) SUPERSEDE their old rank. Unknown spell → stack
/// (never hide a rank on missing data). LOCKSTEP: SPELL_ATTR_PASSIVE = 0x40 (module taxonomy).
fn spell_ranks_stack_in_book(db: &RemoteTables, spell_id: u32) -> bool {
    const SPELL_ATTR_PASSIVE: u32 = 0x40;
    const POWER_MANA: u8 = 0;
    match db.game_spell().spell_id().find(&spell_id) {
        Some(h) => h.attributes & SPELL_ATTR_PASSIVE == 0 && h.power_type == POWER_MANA,
        None => true,
    }
}
