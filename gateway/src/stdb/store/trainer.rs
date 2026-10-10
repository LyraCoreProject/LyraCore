//! `Coordinator`'s [`TrainerStore`] adapter.

use anyhow::Result;
use lyracore_shared::trainer::TrainerRefusal;
use spacetimedb_sdk::Table;

use crate::stdb::bindings::*;
use crate::stdb::connection::{call_reducer, reducer_refusal_reason};
use crate::stdb::reads::spell_ranks_stack_in_book;
use crate::stdb::Coordinator;
use crate::world::{Actor, InteractionOutcome, TrainerStore};

use super::interaction_outcome;

impl TrainerStore for Coordinator {
    /// The Module's current talent reset price for the confirmation dialog.
    fn talent_reset_cost(&self, character_guid: u64) -> Option<u32> {
        self.0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid)
            .map(|character| lyracore_shared::talent::respec_cost_copper(character.respec_count))
    }

    /// A character's live presence `(online, level, class, zone_id)` for `SMSG_FRIEND_STATUS`/
    /// `SMSG_FRIEND_LIST`. `None` if the guid doesn't resolve to any character (a stale reference).
    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>> {
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&guid)
            .map(|c| (c.online, c.level, c.class, c.zone_id)))
    }

    /// Does this trainer serve `player_guid`'s class? Fail-open on any missing read (trainer,
    /// character, template), matching `npc_refuses_interaction`.
    fn trainer_serves(&self, player_guid: u64, trainer_guid: u64) -> Result<bool> {
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

    /// The skill line offering `spell_id` teaches at trainer `trainer_guid`, or 0 for an ordinary spell
    /// offering (and for any missing trainer/offering row). The offering list is keyed by the trainer's
    /// creature-template `entry`, like the vendor stock. Read from the privileged cache.
    fn trainer_offer_skill_line(&self, trainer_guid: u64, spell_id: u32) -> u32 {
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entry) = db
            .game_world_entity()
            .guid()
            .find(&trainer_guid)
            .map(|e| e.entry)
        else {
            return 0;
        };
        let line = db
            .game_trainer_spell()
            .iter()
            .find(|t| t.trainer_entry == entry && t.spell_id == spell_id)
            .map(|t| t.learn_skill_line)
            .unwrap_or(0);
        line
    }

    /// The spells trainer `trainer_guid` teaches, folded with `player_guid`'s level + known-state so the
    /// codec can render each Green/Red/Gray. The trainer's creature-template `entry` keys the list (like
    /// the vendor stock); `known` = a `game_player_spell` row (the one castability source). A missing
    /// trainer/player → empty list. Read from the privileged cache (coordinator bypasses RLS).
    fn trainer_list(
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

    /// The KNOWN rank that learning `new_spell` supersedes: the game_spell_chain prev of
    /// `new_spell`, if the player knows it AND the spell's ranks don't stack in the book. Drives
    /// SMSG_SUPERCEDED_SPELL instead of LEARNED_SPELL on a trainer buy.
    ///
    /// The stacking rule is cmangos' canStackSpellRanksInSpellBook (operator-corrected live:
    /// downranking Holy Light is a real thing): MANA spells keep EVERY rank visible (casters
    /// downrank for mana efficiency); rage/energy/health-cost spells and PASSIVES supersede
    /// (Heroic Strike replaces its old rank — there is no "downranked HS").
    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32> {
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

    fn resolve_learn_target(&self, spell_id: u32) -> u32 {
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

    /// The `grant_spell_id` for `talent_id` from the coordinator's `game_talent` cache (0 = passive,
    /// no ability granted, or an unseeded talent).
    fn talent_grant_spell(&self, talent_id: u32) -> u32 {
        self.0
            .coord()
            .conn
            .db
            .game_talent()
            .iter()
            .find(|t| t.talent_id == talent_id)
            .map(|t| t.grant_spell_id)
            .unwrap_or(0)
    }

    /// Sum of the character's spent talent ranks (`game_character_talent`, coordinator RLS-bypassed).
    /// Non-zero gates the post-CREATE login correction of `PLAYER_CHARACTER_POINTS1` (the CREATE's
    /// formula counts points EARNED only — see `codec/entity.rs`).
    fn talent_points_spent(&self, character_guid: u64) -> u32 {
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
    fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32) {
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

    /// Learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`) over the coordinator
    /// connection. The module gates it (range / level / cost / not-already-known) and charges copper.
    /// A Refusal the Module tagged comes back as an outcome; anything else stays an error.
    /// Rides the coordinator connection as `gw_trainer_buy`.
    fn buy_trainer_spell(
        &self,
        actor: Actor,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<crate::world::TrainerBuyOutcome> {
        let coord = self.0.call_pipe();
        let result: Result<()> = call_reducer!(
            coord.conn.reducers,
            "gw_trainer_buy",
            gw_trainer_buy_then(self.session_actor(actor), trainer_guid, spell_id)
        );
        match result {
            Ok(()) => Ok(crate::world::TrainerBuyOutcome::Learned),
            Err(error) => match trainer_refusal(&error) {
                Some(refusal) => Ok(refusal.into()),
                None => Err(error),
            },
        }
    }

    fn learn_talent(&self, actor: Actor, talent_id: u32) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_learn_talent",
            gw_learn_talent_then(self.session_actor(actor), talent_id)
        )
    }

    /// Confirm a talent reset. The Module checks the trainer, range and price.
    fn reset_talents(&self, actor: Actor, trainer_guid: u64) -> Result<InteractionOutcome> {
        let coord = self.0.call_pipe();
        interaction_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_reset_talents",
            gw_reset_talents_then(self.session_actor(actor), trainer_guid)
        ))
    }

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`): upsert by (character, button);
    /// action 0 clears. Without this every bar drag was lost on relog (only creation seeds survived).
    fn set_action_button(
        &self,
        actor: Actor,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()> {
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_action_button",
            gw_set_action_button_then(self.session_actor(actor), button, action, action_type)
        )
    }

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`): the wire's
    /// u32 is the client's 0..63 rep-array slot (ReputationListID — the gtker `Faction` field name
    /// lies, same as SET_FACTION_STANDING); the module reverse-resolves the faction and upserts.
    fn set_faction_at_war(
        &self,
        actor: Actor,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<InteractionOutcome> {
        let coord = self.0.call_pipe();
        interaction_outcome(call_reducer!(
            coord.conn.reducers,
            "gw_set_faction_at_war",
            gw_set_faction_at_war_then(self.session_actor(actor), reputation_index, at_war)
        ))
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

/// The Module's typed trainer Refusal. Only a reducer the Module rejected carries a tag; a timeout,
/// transport, or SDK failure stays an error with an unknown outcome.
fn trainer_refusal(error: &anyhow::Error) -> Option<TrainerRefusal> {
    reducer_refusal_reason(error).and_then(TrainerRefusal::parse_tag)
}

#[cfg(test)]
mod trainer_reducer_tests {
    use super::*;
    use crate::stdb::connection::ReducerCallError;
    use anyhow::anyhow;

    #[test]
    fn only_a_rejected_reducer_carries_a_typed_refusal() {
        for refusal in TrainerRefusal::ALL {
            let error = anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_trainer_buy".to_string(),
                reason: refusal.as_tag().to_string(),
            })
            .context("trainer buy");
            assert_eq!(trainer_refusal(&error), Some(refusal));
        }

        let not_refusals = [
            anyhow::Error::from(ReducerCallError::fatal(
                "gw_trainer_buy reducer timed out after 10s".to_string(),
            )),
            anyhow::Error::from(ReducerCallError::Rejected {
                operation: "gw_trainer_buy".to_string(),
                reason: "operator only".to_string(),
            }),
            anyhow!(
                "wrapped text that mentions {}",
                TrainerRefusal::AlreadyKnown.as_tag()
            ),
        ];
        for error in not_refusals {
            assert_eq!(trainer_refusal(&error), None, "{error:#}");
        }
    }
}
