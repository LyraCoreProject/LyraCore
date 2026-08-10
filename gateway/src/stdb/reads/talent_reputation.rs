//! Talent-pane + faction/reputation cache-accessor methods — pure code-motion split of the
//! former `reads.rs`.

use anyhow::Result;
use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::super::connection::Coordinator;

impl Coordinator {
    /// The player's persisted reputation standings as `(reputation_index, standing,
    /// at_war)` triples — chained into the login `SMSG_INITIALIZE_FACTIONS` so a relog carries the
    /// real standing + the At-War checkbox instead of the all-neutral stub. Rows with
    /// `reputation_index < 0` (stale pre-migration filler) are skipped — there is no slot to
    /// address. RLS-bypassed read, like `player_learned_spells`.
    pub fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        Ok(db
            .game_player_reputation()
            .iter()
            .filter(|r| r.character_guid == player_guid && r.reputation_index >= 0)
            .map(|r| (r.reputation_index, r.standing, r.at_war))
            .collect())
    }

    /// Does `npc_guid` REFUSE to interact with `player_guid` (vanilla
    /// `Unit::GetReactionTo` for the gossip/vendor/trainer/questgiver windows)? The NPC's
    /// faction_template resolves to its parent faction:
    /// - parent has a REP BAR (`reputation_index >= 0`) → reaction = the player's standing RANK
    ///   (their `game_player_reputation` row, else the faction's `base_standing`); refuse at
    ///   Unfriendly (2) or below — Neutral+ interacts.
    /// - no rep bar → FactionTemplate mask fallback: refuse when the NPC is HOSTILE to the player.
    ///
    /// Missing data anywhere → do NOT refuse (fail-open: an unfactioned fixture NPC keeps working).
    pub fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool> {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let entities = db.game_world_entity();
        let (Some(npc), Some(player)) = (
            entities.guid().find(&npc_guid),
            entities.guid().find(&player_guid),
        ) else {
            return Ok(false);
        };
        let templates = db.game_faction_template();
        let Some(npc_ft) = templates.id().find(&npc.faction_template) else {
            return Ok(false);
        };
        if let Some(parent) = db.game_faction().faction_id().find(&npc_ft.faction) {
            if parent.reputation_index >= 0 {
                let row = db
                    .game_player_reputation()
                    .iter()
                    .find(|r| r.character_guid == player_guid && r.faction_id == parent.faction_id);
                // The At-War checkbox forces hostile regardless of standing (vanilla).
                if row.as_ref().map(|r| r.at_war).unwrap_or(false) {
                    return Ok(true);
                }
                let standing = row.map(|r| r.standing).unwrap_or(parent.base_standing);
                return Ok(reputation_rank(standing) <= RANK_UNFRIENDLY);
            }
        }
        let Some(player_ft) = templates.id().find(&player.faction_template) else {
            return Ok(false);
        };
        Ok(faction_template_hostile(&npc_ft, &player_ft))
    }

    /// Look up the static `Talent` metadata for `talent_id` (from the coordinator's `game_talent` cache).
    /// Returns `None` if the talent isn't seeded (the dispatch treats an unknown talent as a noop).
    pub fn talent_by_id(&self, talent_id: u32) -> Option<Talent> {
        self.0
            .coord()
            .conn
            .db
            .game_talent()
            .iter()
            .find(|t| t.talent_id == talent_id)
    }

    /// Sum of the character's spent talent ranks (`game_character_talent`, coordinator RLS-bypassed).
    /// Non-zero gates the post-CREATE login correction of `PLAYER_CHARACTER_POINTS1` (the CREATE's
    /// formula counts points EARNED only — see `codec/entity.rs`).
    pub fn talent_points_spent(&self, character_guid: u64) -> u32 {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_character_talent()
            .iter()
            .filter(|t| t.character_guid == character_guid)
            .map(|t| t.rank as u32)
            .sum()
    }

    /// Talent-pane sync data read AFTER a successful `learn_talent` (the blocking reducer call
    /// returns with the cache already consistent): `(teach_spell, superseded_prev, points_remaining)`.
    /// `teach_spell` = the rank-spell the module just put in the spellbook — the 1.12 TalentFrame
    /// derives a talent's shown rank from which rank-spell is KNOWN, so this must relay live as
    /// SMSG_LEARNED_SPELL or the pane freezes until relog; 0 when the module taught nothing (mirror
    /// of `talent::apply_talent_rank`'s game_spell-existence gate). `superseded_prev` = the previous
    /// rank's now-replaced spell (drives SMSG_SUPERCEDED_SPELL; 0 for rank 1 / same-spell demo
    /// trees). `points_remaining` = earned (level−9, floor 0) minus spent — the live
    /// `PLAYER_CHARACTER_POINTS1` value. Callers may pass `talent_id = 0` to get just the points.
    pub fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32) {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let level = db
            .game_world_entity()
            .guid()
            .find(&character_guid)
            .map(|e| e.level)
            .unwrap_or(0);
        let spent: u32 = db
            .game_character_talent()
            .iter()
            .filter(|t| t.character_guid == character_guid)
            .map(|t| t.rank as u32)
            .sum();
        let remaining = (level as i32 - 9).max(0) as u32;
        let remaining = remaining.saturating_sub(spent);
        let Some(t) = db.game_talent().iter().find(|t| t.talent_id == talent_id) else {
            return (0, 0, remaining);
        };
        let rank = db
            .game_character_talent()
            .iter()
            .find(|r| r.character_guid == character_guid && r.talent_id == talent_id)
            .map(|r| r.rank)
            .unwrap_or(0);
        let new_spell = pick_rank_spell(rank, &t);
        let prev_spell = if rank >= 2 {
            pick_rank_spell(rank - 1, &t)
        } else {
            0
        };
        // Mirror the module's teach gate: apply_talent_rank only puts the rank-spell in the book
        // when it exists in game_spell (an unimported rank-spell is skipped there — sending
        // LEARNED for it would desync the client book from the server).
        let teach = if new_spell != 0 && db.game_spell().spell_id().find(&new_spell).is_some() {
            new_spell
        } else {
            0
        };
        let superseded = if teach != 0 && prev_spell != 0 && prev_spell != teach {
            prev_spell
        } else {
            0
        };
        (teach, superseded, remaining)
    }
}

/// Mirror of the module's `talent::pick_rank_spell`: rank N's spell from the per-rank columns
/// (`rank_spell_2..5`), a 0 column falling back to `spell_id` (the demo tree scales ONE spell by
/// rank; the imported tree has a distinct spell per rank).
fn pick_rank_spell(rank: u8, t: &Talent) -> u32 {
    let s = match rank {
        2 => t.rank_spell_2,
        3 => t.rank_spell_3,
        4 => t.rank_spell_4,
        5 => t.rank_spell_5,
        _ => t.spell_id,
    };
    if s != 0 {
        s
    } else {
        t.spell_id
    }
}

/// Reputation RANK for a raw standing (0=Hated .. 7=Exalted, Neutral=3). KEEP IN LOCKSTEP with
/// `module/src/reputation.rs::reputation_rank` — same mangos thresholds, duplicated because the
/// module fn is table-crate-private (promote both into lyracore_shared if a third consumer appears).
fn reputation_rank(standing: i32) -> u8 {
    match standing {
        s if s >= 42000 => 7,
        s if s >= 21000 => 6,
        s if s >= 9000 => 5,
        s if s >= 3000 => 4,
        s if s >= 0 => 3,
        s if s >= -3000 => 2,
        s if s >= -6000 => 1,
        _ => 0,
    }
}
/// Refuse-interaction threshold: Unfriendly (2) and below refuse; Neutral (3)+ interacts.
const RANK_UNFRIENDLY: u8 = 2;

/// Is template `a` HOSTILE to template `b`? KEEP IN LOCKSTEP with
/// `module/src/faction.rs::compute_hostile` (the vanilla `FactionTemplate.dbc` relationship rule) — the
/// explicit enemy list wins, then the explicit friend list, else the group masks decide. Duplicated
/// over the gateway BINDING type for the interaction-window mask fallback.
fn faction_template_hostile(
    a: &super::super::bindings::FactionTemplate,
    b: &super::super::bindings::FactionTemplate,
) -> bool {
    if b.faction != 0 {
        if [a.enemy_0, a.enemy_1, a.enemy_2, a.enemy_3].contains(&b.faction) {
            return true;
        }
        if [a.friend_0, a.friend_1, a.friend_2, a.friend_3].contains(&b.faction) {
            return false;
        }
    }
    (a.enemy_group & b.faction_group) != 0
}

#[cfg(test)]
mod tests {
    // The two pure reaction helpers, vectors mirrored from module/src/reputation.rs +
    // module/src/faction.rs tests (the lockstep contract).
    #[test]
    fn reputation_rank_thresholds_match_mangos() {
        use super::reputation_rank;
        assert_eq!(reputation_rank(42000), 7); // Exalted
        assert_eq!(reputation_rank(9000), 5); // Honored
        assert_eq!(reputation_rank(0), 3); // Neutral
        assert_eq!(reputation_rank(-1), 2); // Unfriendly
        assert_eq!(reputation_rank(-3001), 1); // Hostile
        assert_eq!(reputation_rank(-42000), 0); // Hated
        assert!(
            reputation_rank(-1) <= super::RANK_UNFRIENDLY,
            "Unfriendly refuses"
        );
        assert!(
            reputation_rank(0) > super::RANK_UNFRIENDLY,
            "Neutral interacts"
        );
    }

    #[test]
    fn faction_template_hostility_mask_and_explicit_lists() {
        use super::super::super::bindings::FactionTemplate;
        let ft = |id, faction, fg, eg, enemy_0, friend_0| FactionTemplate {
            id,
            faction,
            faction_group: fg,
            friend_group: 0,
            enemy_group: eg,
            enemy_0,
            enemy_1: 0,
            enemy_2: 0,
            enemy_3: 0,
            friend_0,
            friend_1: 0,
            friend_2: 0,
            friend_3: 0,
        };
        // Monster (group mask): enemy_group intersects the player's faction_group → hostile.
        let monster = ft(14, 14, 8, 1, 0, 0);
        let player = ft(1, 1, 1, 0, 0, 0);
        assert!(super::faction_template_hostile(&monster, &player));
        assert!(
            !super::faction_template_hostile(&player, &monster),
            "player group 0 enemy mask"
        );
        // Explicit friend list beats the mask; explicit enemy list beats everything.
        let befriended = ft(2, 14, 8, 1, 0, 1);
        assert!(!super::faction_template_hostile(&befriended, &player));
        let sworn = ft(3, 14, 8, 0, 1, 0);
        assert!(super::faction_template_hostile(&sworn, &player));
    }

}
