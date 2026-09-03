//! Class trainers — learn new spells/ranks while leveling (the leveling-spine system that lets the
//! per-character spellbook GROW past the login class kit). Mirrors the vendor shape: a static
//! `game_trainer_spell` list (trainer creature entry → teachable spells), a range/flag-gated buy that
//! charges copper and calls [`crate::spell::learn_spell`], and a debug twin for server verification.
//! The gateway turns `CMSG_TRAINER_LIST` into `SMSG_TRAINER_LIST` (each spell flagged Green/Red/Gray)
//! and `CMSG_TRAINER_BUY_SPELL` into this reducer, then pushes `SMSG_LEARNED_SPELL` so the ability
//! appears on the action bar live (no relog).
//!
//! Deliberate simplification: the `game_trainer_spell` row carries only spell_id/cost/required_level —
//! class spells need no required_skill / prerequisite-spell chain (those are the profession/talent
//! cases; add the columns when professions land). A refused buy names its reason with a
//! [`TrainerRefusal`]; the Gateway owns the mapping onto gtker vanilla's three
//! `TrainingFailureReason` codes. [entity]

use lyracore_shared::trainer::TrainerRefusal;
use spacetimedb::{table, ReducerContext};

use crate::{
    game_creature_template, game_player_skill, game_spell_chain, game_spell_effect,
    game_world_entity, WorldEntity,
};

/// Max distance to train at a trainer: (10 yd)² — same as the vendor/quest/loot interaction range. The
/// client walks into range before sending `CMSG_TRAINER_BUY_SPELL`, so this only rejects out-of-range abuse.
/// `pub(crate)` so `talent::do_reset_talents` (also a trainer-gated interaction) shares the ONE range
/// constant rather than forking a second magic number that could drift from this one.
pub(crate) const TRAINER_RANGE_SQ: f32 = 100.0;

/// Reducer edge: only the tag crosses to the gateway; the detail stays in module logs.
fn refused(refusal: TrainerRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("trainer refused {tag}: {detail}");
    tag.to_string()
}

/// One spell a trainer teaches. Static, public, SQL-loadable (no Timestamp), keyed by the trainer's
/// CREATURE TEMPLATE entry (every spawned trainer of that entry offers the same list — like vendors).
/// Logical key `(trainer_entry, spell_id)` via the `#[auto_inc]` PK + `by_trainer` btree. [static]
#[table(accessor = game_trainer_spell, public, index(accessor = by_trainer, btree(columns = [trainer_entry])))]
pub struct TrainerSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub trainer_entry: u32, // -> game_creature_template.entry (the trainer NPC's template)
    pub spell_id: u32,      // the spell taught
    pub cost: u32,          // copper charged on purchase
    pub required_level: u8, // minimum character level to learn it
    // END-APPENDED (professions slice 3), defaulted 0 → `publish` auto-migrates + every existing
    // class-spell row reads 0 (the unchanged spell path). 0 = a normal spell offering; >0 = this
    // offering TEACHES that skill_line as a profession (Cooking=185 / Skinning=393) — the buy branch
    // grants the skill via `crate::skill::learn_profession` instead of casting/learning a spell. The
    // `spell_id` of a profession offering is a synthetic marker id (50080/50081), never resolved.
    // `#[default(0u32)]` (typed, not bare 0 — the last_logout_micros lesson) so the column-add
    // auto-migrates the populated game_trainer_spell table instead of aborting the publish.
    #[default(0u32)]
    pub learn_skill_line: u32,
    // END-APPENDED (professions slice — rank/cap scaling), defaulted **75** (Apprentice). An auto-migrated
    // EXISTING profession offering learns at exactly the apprentice cap as before (byte-identical); a
    // distinct higher-tier offering (Journeyman=150, Expert=225, Artisan=300) marker LIFTS the ceiling on
    // buy. `#[default(75u32)]` (NOT 0 — a 0 cap would insert a 1/0 profession that can never climb). Only
    // meaningful when `learn_skill_line > 0`; a class-spell row (line 0) ignores it.
    #[default(75u32)]
    pub learn_skill_cap: u32,
}

/// What a successful trainer purchase GRANTS (professions slice 3) — `crate::skill::learn_profession` for
/// a flagged profession/weapon offering, `crate::spell::learn_spell_with_dependents` for a plain class
/// spell. Each `apply_trainer_buy` arm constructs its OWN variant directly at the point it already knows
/// which one applies — there is no longer a separate `grant_for` routing function: the old
/// shape threaded a `(to_learn, known, cap)` tuple with a `0u32` dead-sentinel `to_learn` on the two
/// profession/weapon arms (never read on those arms — only `grant_for` re-derived, from `profession_line`
/// ALONE, the exact branch each tuple was already built in) through to a `grant_for` call that just
/// re-decided the same fork the caller had already taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuyGrant {
    /// Teach a profession `(skill_line, cap)` (Cooking=185 / Skinning=393 at cap 75/150/…) →
    /// `crate::skill::learn_profession`. The cap is the rank ceiling the buy lifts the skill to.
    Profession(u32, u32),
    /// Teach RIDING at this tier (75 Apprentice, 150 Journeyman) → `crate::skill::learn_riding`. Its own
    /// variant because riding is granted WHOLE — current and max both become the tier — while a
    /// profession is born at 1 and climbs to its cap. The mount gate compares the CURRENT rank, so
    /// granting riding the profession way would leave a paying rider unable to mount.
    Riding(u32),
    /// Learn the resolved spell id (the LearnSpell wrapper's rank, or the spell itself) → `crate::spell::learn_spell`.
    Spell(u32),
}

/// Whether a profession offering at `cap` is ALREADY covered by the character's stored `max_rank` for
/// that skill line (professions slice 3's rank/cap scaling) — the pure comparison the buy path's
/// already-known gate keys on for a profession offering (a class-spell offering uses `knows_spell`
/// instead). Extracted from `apply_trainer_buy` (pure code-motion). `None` (no `game_player_skill` row
/// at all — never learned) is never capped. `stored_max_rank >= cap` is the boundary: exactly meeting
/// the cap is already-known (rejected); one below it is not (the buy proceeds and lifts the ceiling).
pub(crate) fn profession_already_capped(stored_max_rank: Option<u16>, cap: u32) -> bool {
    stored_max_rank.is_some_and(|rank| rank as u32 >= cap)
}

/// Whether a rank purchase's `game_spell_chain` prerequisite is satisfied (work-item 102, reduced scope):
/// `prev_spell == 0` — the family's first rank, or a spell with no chain concept at all — has no
/// prerequisite, so it is ALWAYS allowed regardless of `knows_prev`. Otherwise the caster must already
/// know `prev_spell` (vanilla refuses training "Fireball Rank 3" while only Rank 1 is known). Pure →
/// unit-tested without a `ReducerContext`.
pub(crate) fn rank_prereq_met(prev_spell: u32, knows_prev: bool) -> bool {
    prev_spell == 0 || knows_prev
}

/// Pure purchase gate — decides whether `level`/`money` may learn a spell costing `cost` at `required_level`.
/// Checked in priority order (already-known first, then level, then money) so the player gets the most
/// relevant message. Pure → unit-tested.
pub(crate) fn trainer_buy_check(
    known: bool,
    level: u32,
    required_level: u32,
    money: u32,
    cost: u32,
) -> Result<(), TrainerRefusal> {
    if known {
        return Err(TrainerRefusal::AlreadyKnown);
    }
    if level < required_level {
        return Err(TrainerRefusal::LevelTooLow);
    }
    if money < cost {
        return Err(TrainerRefusal::NotEnoughMoney);
    }
    Ok(())
}

/// The spell a trainer OFFERING actually teaches. A trainer offers a LearnSpell WRAPPER (Spell.dbc
/// effect 36 → E_SCRIPTED with the real rank in `trigger_spell`); the castable RANK is that trigger —
/// learn THAT so the spellbook + cast resolve a real game_spell row. A self-contained ability (no
/// trigger) is learned as-is; falls back to `spell_id` when the wrapper's effect rows aren't imported
/// (never worse than before). EXCEPTION — a CHANNELED spell (Arcane Missiles) is NOT a wrapper: its
/// `trigger_spell` is the per-tick MISSILE (an A_PERIODIC_TRIGGER effect), not a rank to learn; skip
/// that trigger so the learner gets the channel itself (5143), not the hidden bolt (7268). The same
/// logic excludes every other reactive/at-cast trigger kind — A_FLAG, both Proc kinds (Frost Armor's
/// chill 6136), plain E_TRIGGER (Bloodrage's trickle 29131): those triggers are effect PAYLOADS, not
/// ranks; treating them as wrappers taught the payload instead of the spell (156 review — a Frost
/// Armor R2 buy charged for and learned "Chilled"). A genuine LearnSpell wrapper's effect imports as
/// E_SCRIPTED (never one of the excluded kinds), so it still resolves. This exclusion list MUST stay
/// in lockstep with the importer's `wrapper_to_rank` heuristic (importer/src/spell.rs) — the two are
/// the same rule on the two sides of the wire. Generic over the kind. Shared by `apply_trainer_buy`
/// (the player buy) and the playerbots trainer-kit pass (work-item 156) — ONE wrapper-resolution
/// chokepoint, so a bot's spellbook and a trained player's can never drift. [entity]
pub(crate) fn resolve_learn_target(ctx: &ReducerContext, spell_id: u32) -> u32 {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .find_map(|e| {
            (e.trigger_spell != 0
                && e.kind != crate::spell::A_PERIODIC_TRIGGER
                && e.kind != crate::spell::A_FLAG
                && e.kind != crate::spell::A_PROC_TRIGGER
                && e.kind != crate::spell::A_PROC_DAMAGE
                && e.kind != crate::spell::E_TRIGGER)
                .then_some(e.trigger_spell)
        })
        .unwrap_or(spell_id)
}

/// Resolve + validate a trainer interaction: `trainer_guid` must be a real in-range TRAINER on `caster`'s
/// own map+instance, and one that SERVES `caster`'s class — a Paladin trainer refuses a Warrior
/// outright. Shared by [`apply_trainer_buy`] and `talent::do_reset_talents` (the respec path,
/// which is trainer-gated identically — its own comment used to read "Same gates as apply_trainer_buy").
/// Returns the resolved trainer entity.
///
/// The `Err` is a bare human detail, not a Refusal tag: `apply_trainer_buy` turns it into a
/// [`TrainerRefusal::Unavailable`] and logs the detail, while `do_reset_talents` keeps the plain
/// text its respec path already returns.
pub(crate) fn validate_trainer_interaction(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    trainer_guid: u64,
) -> Result<WorldEntity, String> {
    let trainer = ctx
        .db
        .game_world_entity()
        .guid()
        .find(trainer_guid)
        .ok_or_else(|| "no such trainer".to_string())?;
    if trainer.npc_flags & lyracore_shared::constants::npc_flags::TRAINER == 0 {
        return Err("target is not a trainer".to_string());
    }
    if trainer.map_id != caster.map_id || trainer.instance_id != caster.instance_id {
        return Err("trainer on another map".to_string());
    }
    if crate::helpers::dist_sq(caster, &trainer) > TRAINER_RANGE_SQ {
        return Err("trainer out of range".to_string());
    }
    // The class gate sits at this shared chokepoint so one guard closes both wrong-class training
    // and wrong-class respec. No template row means no gate: missing imported data must not block
    // an interaction that used to work.
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(trainer.entry)
        .is_some_and(|t| {
            !lyracore_shared::trainer::serves(caster.class(), t.trainer_type, t.trainer_class)
        })
    {
        return Err("trainer does not teach your class".to_string());
    }
    Ok(trainer)
}

/// Shared buy logic for the player + debug paths: validate `trainer` is a real in-range trainer, that it
/// teaches `spell_id`, run [`trainer_buy_check`], then charge copper + learn the spell. This is the
/// reducer edge, so a Refusal leaves only its [`TrainerRefusal`] tag in the `Err`. [entity]
pub(crate) fn apply_trainer_buy(
    ctx: &ReducerContext,
    caster_guid: u64,
    trainer_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut caster = entities
        .guid()
        .find(caster_guid)
        .ok_or_else(|| refused(TrainerRefusal::Unavailable, "buyer not in world"))?;
    let trainer = validate_trainer_interaction(ctx, &caster, trainer_guid)
        .map_err(|detail| refused(TrainerRefusal::Unavailable, &detail))?;
    // The trainer must actually teach this spell (its template's list). The spawned trainer carries its
    // creature-template `entry`; the list is keyed by that.
    let offered = ctx
        .db
        .game_trainer_spell()
        .by_trainer()
        .filter(&trainer.entry)
        .find(|s| s.spell_id == spell_id)
        .ok_or_else(|| {
            refused(
                TrainerRefusal::NotOffered,
                "trainer does not teach that spell",
            )
        })?;

    // PROFESSION-LEARN BRANCH (professions slice 3): a flagged offering teaches a SKILL, not a spell —
    // it never resolves a wrapper/rank, never casts, never touches `game_player_spell`. `known` mirrors
    // `learn_profession`'s presence check (a `game_player_skill` row for that line → a re-buy is the
    // idempotent already-known no-op), and on Ok it grants the skill at 1/75. The `learn_skill_line == 0`
    // arm below is the EXISTING spell path verbatim (byte-identical → no class-spell regression).
    //
    // WEAPON-MASTER FORK (work-item 202): the SAME `learn_skill_line` column shape also carries weapon
    // proficiency offerings (Daggers, Polearm, …) — no new row kind. `is_combat_skill_line` tells the two
    // apart and the cap/known computation diverges for each:
    //   - PROFESSION cap is the offering's STATIC tier column (`learn_skill_cap`: Apprentice 75, Journeyman
    //     150, …), author-set and independent of level; "known" is a CAP comparison (`profession_already_capped`)
    //     so a higher-tier re-buy still lifts the ceiling.
    //   - WEAPON cap is LEVEL-DERIVED (`skill::skill_cap_for_level(caster.level)`), exactly like every other
    //     combat line (`raise_combat_caps`) — the offering's `learn_skill_cap` column is ignored (seeded 0/don't-
    //     care) for a weapon row, since a static column can never track a climbing level and a stored 0 would
    //     be the struct comment's "1/0 stuck-row hazard" (a cap that can never be climbed) if read literally.
    //     "known" is mere ROW PRESENCE (any `game_player_skill` row for that line at all): weapon proficiency
    //     has no tiers to re-buy, so a class-seeded line (already present via `ensure_player_skills`) is fully
    //     known and refuses, while a lacked line proceeds — matching vanilla's "already knows Swords" refusal.
    //
    // RIDING FORK: the same column carries a riding offering (`learn_skill_line` = 762). It takes the cap
    // from the offering's static tier like a profession, but grants through `learn_riding` — riding is
    // trained WHOLE (current == max == the tier) because the mount cast gate reads the CURRENT rank.
    let profession_line = offered.learn_skill_line;
    let is_weapon_learn =
        profession_line != 0 && crate::skill::is_combat_skill_line(profession_line);
    let (grant, known): (BuyGrant, bool) = if crate::skill::is_riding_skill_line(profession_line) {
        // RIDING FORK: a riding offering only belongs on a MOUNTS trainer, so a mis-authored row on a
        // class/tradeskill trainer is refused rather than quietly teaching the mount skill. Fail-OPEN on a
        // missing creature template, the same posture `validate_trainer_interaction`'s class gate takes —
        // un-imported data must never block an interaction that used to work.
        if ctx
            .db
            .game_creature_template()
            .entry()
            .find(trainer.entry)
            .is_some_and(|t| t.trainer_type != lyracore_shared::trainer::trainer_type::MOUNTS)
        {
            return Err(refused(
                TrainerRefusal::Unavailable,
                "that trainer does not teach riding",
            ));
        }
        // "Known" is the same TIER comparison a profession uses: Apprentice-75 has not met Journeyman-150,
        // so the second tier is buyable exactly once and a re-buy of the tier you hold is refused.
        let stored_max_rank = ctx
            .db
            .game_player_skill()
            .by_character()
            .filter(&caster_guid)
            .find(|s| s.skill_line == profession_line)
            .map(|r| r.max_rank);
        (
            BuyGrant::Riding(offered.learn_skill_cap),
            profession_already_capped(stored_max_rank, offered.learn_skill_cap),
        )
    } else if is_weapon_learn {
        let cap = crate::skill::skill_cap_for_level(caster.level);
        let present = ctx
            .db
            .game_player_skill()
            .by_character()
            .filter(&caster_guid)
            .any(|s| s.skill_line == profession_line);
        (BuyGrant::Profession(profession_line, cap), present)
    } else if profession_line != 0 {
        // Already-known keys on the CAP, not mere row presence (rank/cap scaling): "known" iff a row exists
        // whose `max_rank` already meets-or-exceeds THIS offering's cap. So an Apprentice re-buy (cap 75 vs
        // stored 75) is rejected for the default cap, but buying Journeyman (cap 150) while at
        // apprentice-75 PASSES the gate and lifts the ceiling, and a 2nd Journeyman buy (150 >= 150) is
        // then rejected.
        let stored_max_rank = ctx
            .db
            .game_player_skill()
            .by_character()
            .filter(&caster_guid)
            .find(|s| s.skill_line == profession_line)
            .map(|r| r.max_rank);
        (
            BuyGrant::Profession(profession_line, offered.learn_skill_cap),
            profession_already_capped(stored_max_rank, offered.learn_skill_cap),
        )
    } else {
        // The castable RANK behind the offering — extracted to `resolve_learn_target` (see its doc) so
        // the playerbots trainer-kit pass (work-item 156) resolves wrappers IDENTICALLY to a real buy.
        // The already-known gate keys on the RANK so a re-buy of a known rank is rejected (not silently
        // re-charged).
        let to_learn = resolve_learn_target(ctx, spell_id);
        (
            BuyGrant::Spell(to_learn),
            crate::spell::knows_spell(ctx, caster_guid, to_learn),
        )
    };
    // RANK-PREREQ GATE (work-item 102, reduced scope): a plain class-spell offering (`grant` is
    // `BuyGrant::Spell` — professions/weapons have no chain concept) whose resolved rank carries a
    // `game_spell_chain` row must have the PREVIOUS rank already known (vanilla refuses training
    // "Fireball Rank 3" while only Rank 1 is known). NO chain row (an un-imported spell, or a rank with
    // nothing before it) → the gate passes, BYTE-IDENTICAL to before this work item (the 178/212
    // precedent: missing imported data never blocks a purchase that used to succeed). Skipped when the
    // spell is ALREADY KNOWN so `trainer_buy_check`'s "already-known first" message-priority contract
    // (see its doc) holds even for a rank granted out of order by a debug lever or bot kit — the buy is
    // rejected either way; only the message differs (102 review finding).
    if !known {
        if let BuyGrant::Spell(to_learn) = grant {
            if let Some(chain) = ctx.db.game_spell_chain().spell_id().find(to_learn) {
                let knows_prev = crate::spell::knows_spell(ctx, caster_guid, chain.prev_spell);
                if !rank_prereq_met(chain.prev_spell, knows_prev) {
                    return Err(refused(
                        TrainerRefusal::PreviousRankMissing,
                        "requires the previous rank",
                    ));
                }
            }
        }
    }
    match trainer_buy_check(
        known,
        caster.level,
        offered.required_level as u32,
        caster.money,
        offered.cost,
    ) {
        Ok(()) => {
            caster.money = caster.money.saturating_sub(offered.cost);
            let owner = caster.owner_identity;
            entities.guid().update(caster);
            match grant {
                BuyGrant::Profession(line, cap) => {
                    crate::skill::learn_profession(ctx, caster_guid, owner, line, cap as u16)
                }
                BuyGrant::Riding(tier) => {
                    crate::skill::learn_riding(ctx, caster_guid, owner, tier as u16)
                }
                // Trainer-path only: `learn_spell_with_dependents` also auto-teaches this rank's
                // ONE-LEVEL `game_spell_learn` dependents (see its doc for why NOT plain `learn_spell`
                // universally — it would change createinfo's spell COUNT and break 212's provenance
                // count-parity runbook).
                BuyGrant::Spell(id) => {
                    crate::spell::learn_spell_with_dependents(ctx, caster_guid, owner, id)
                }
            }
            Ok(())
        }
        Err(TrainerRefusal::AlreadyKnown) => {
            Err(refused(TrainerRefusal::AlreadyKnown, "already known"))
        }
        Err(TrainerRefusal::LevelTooLow) => Err(refused(
            TrainerRefusal::LevelTooLow,
            &format!("requires level {}", offered.required_level),
        )),
        Err(TrainerRefusal::NotEnoughMoney) => Err(refused(
            TrainerRefusal::NotEnoughMoney,
            &format!("not enough money (need {})", offered.cost),
        )),
        // `trainer_buy_check` returns no other Refusal; the arm keeps the match exhaustive.
        Err(refusal) => Err(refused(refusal, "purchase gate refused the buy")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- work-item 102 (reduced scope): rank-prereq gate ---------------------------------------------

    /// `rank_prereq_met` truth table: `prev_spell == 0` is ALWAYS allowed regardless of `knows_prev`
    /// (the family's first rank, or a spell with no chain concept — the fn's own NO-ROW-FALLBACK
    /// shape: `apply_trainer_buy` only calls this at all when a `game_spell_chain` row exists, and an
    /// absent row is handled at that ctx-bound call site by skipping the gate entirely — see its doc —
    /// which is behaviorally identical to this fn returning `true` for a `prev_spell == 0` row).
    /// Otherwise the caster must already know `prev_spell`.
    #[test]
    fn rank_prereq_met_truth_table() {
        assert!(
            rank_prereq_met(0, false),
            "no prerequisite (rank 1 / unchained) -> always allowed"
        );
        assert!(
            rank_prereq_met(0, true),
            "no prerequisite -> allowed even if knows_prev happens to be true"
        );
        assert!(rank_prereq_met(133, true), "prerequisite known -> allowed");
        assert!(
            !rank_prereq_met(133, false),
            "prerequisite NOT known -> rejected"
        );
    }

    #[test]
    fn trainer_buy_check_gates_in_priority_order() {
        // already known wins even when everything else is fine
        assert_eq!(
            trainer_buy_check(true, 10, 1, 1000, 10),
            Err(TrainerRefusal::AlreadyKnown)
        );
        // then the level gate
        assert_eq!(
            trainer_buy_check(false, 2, 6, 1000, 10),
            Err(TrainerRefusal::LevelTooLow)
        );
        // then the money gate
        assert_eq!(
            trainer_buy_check(false, 10, 1, 5, 10),
            Err(TrainerRefusal::NotEnoughMoney)
        );
        // all good → Ok
        assert_eq!(trainer_buy_check(false, 10, 6, 100, 10), Ok(()));
        // exact level + exact money → Ok (boundaries inclusive)
        assert_eq!(trainer_buy_check(false, 6, 6, 10, 10), Ok(()));
    }

    /// PROFESSION REBUY CAP: the already-known gate for a profession offering compares the character's
    /// STORED max_rank against THIS offering's cap. Exactly meeting the cap is already-known (rejected,
    /// no re-charge); one rank below it is not (the buy proceeds and lifts the ceiling); never having
    /// trained the line at all (`None`) is never capped.
    #[test]
    fn profession_already_capped_compares_stored_max_rank_against_the_offering_cap() {
        assert!(
            !profession_already_capped(None, 75),
            "never learned -> not capped"
        );
        assert!(
            profession_already_capped(Some(75), 75),
            "Apprentice re-buy at the same cap is rejected"
        );
        assert!(
            !profession_already_capped(Some(75), 150),
            "Apprentice-75 has NOT met the Journeyman-150 cap"
        );
        assert!(
            profession_already_capped(Some(150), 150),
            "a 2nd Journeyman buy (150 >= 150) is rejected"
        );
        assert!(
            !profession_already_capped(Some(74), 75),
            "one rank below the cap is not yet capped"
        );
    }

    /// WEAPON-LEARN PRESENCE-KNOWN semantics (work-item 202): unlike a profession offering (known = a CAP
    /// comparison), a weapon offering's "known" is mere ROW PRESENCE — feeding `trainer_buy_check` a bare
    /// `known` bool either way, so the SAME gate enforces both: a class-seeded weapon line (row present,
    /// regardless of its stored cap) refuses as already-known; a lacked line (no row at all) proceeds.
    #[test]
    fn weapon_learn_known_is_row_presence_not_a_cap_comparison() {
        // Present (seeded by ensure_player_skills at character creation) -> already-known, rejected.
        assert_eq!(
            trainer_buy_check(true, 40, 1, 1000, 100),
            Err(TrainerRefusal::AlreadyKnown)
        );
        // Absent (never seeded/learned) -> proceeds regardless of what a stale cap column might say.
        assert_eq!(trainer_buy_check(false, 40, 1, 1000, 100), Ok(()));
    }

    // --- the riding fork ------------------------------------------------------------------

    /// A riding purchase is TIER-gated the same way a profession is: Apprentice-75 has not met the
    /// Journeyman-150 offering, so the second tier is buyable exactly once, and re-buying the tier you
    /// already hold is refused with no re-charge. That is what stops a player paying twice for riding.
    #[test]
    fn a_riding_tier_is_buyable_once_and_the_next_tier_still_sells() {
        assert!(
            !profession_already_capped(None, 75),
            "an untrained rider can buy Apprentice"
        );
        assert!(
            profession_already_capped(Some(75), 75),
            "re-buying Apprentice is refused"
        );
        assert!(
            !profession_already_capped(Some(75), 150),
            "Apprentice-75 has not met the Journeyman-150 tier"
        );
        assert!(
            profession_already_capped(Some(150), 150),
            "re-buying Journeyman is refused"
        );
    }

    /// The riding fork's two decisions, pinned on the buy path because the crate has no
    /// `ReducerContext` harness: a riding offering is refused on a trainer that is not a
    /// `trainer_type::MOUNTS` trainer, and a successful buy grants through `learn_riding` — never
    /// `learn_profession`, whose born-at-1 row would leave a paying rider below every mount's
    /// `min_skill`.
    #[test]
    fn the_riding_fork_requires_a_mounts_trainer_and_grants_the_whole_tier() {
        let buy = crate::test_scan::code_of(
            include_str!("trainer.rs"),
            "pub(crate) fn apply_trainer_buy(",
        );
        assert!(
            buy.contains("crate::skill::is_riding_skill_line(profession_line)"),
            "the riding offering must take its own fork. Body was:\n{buy}"
        );
        assert!(
            buy.contains("t.trainer_type != lyracore_shared::trainer::trainer_type::MOUNTS"),
            "a riding offering must be refused on a non-MOUNTS trainer. Body was:\n{buy}"
        );
        assert!(
            buy.contains("crate::skill::learn_riding(ctx, caster_guid, owner, tier as u16)"),
            "the riding grant must go through `learn_riding`, not the profession grant. Body was:\n{buy}"
        );
    }

    /// THE IDEMPOTENT RE-LEARN (professions slice 3): the profession buy keys its already-known gate on the
    /// PRESENCE of a `game_player_skill` row for the line (the `known` flag the buy path derives), and feeds
    /// it through the SAME `trainer_buy_check`. So a re-buy of an already-learned profession is rejected with
    /// `TrainerRefusal::AlreadyKnown` (no re-charge, no duplicate/reset row) — identical to a re-bought known spell.
    #[test]
    fn profession_rebuy_is_rejected_as_already_known() {
        // First learn: no skill row yet (known=false), cost 0, level 1 → the gate ALLOWS the grant.
        assert_eq!(trainer_buy_check(false, 1, 1, 0, 0), Ok(()));
        // Re-buy: the skill row now exists (known=true) → rejected as already-known, even though cost is 0
        // and the level is fine (the idempotent no-op — the row is never reset or duplicated).
        assert_eq!(
            trainer_buy_check(true, 1, 1, 0, 0),
            Err(TrainerRefusal::AlreadyKnown)
        );
    }
}
