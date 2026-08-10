//! **Weapon & defense skill** — a per-player skill store (`game_player_skill`) tracking each weapon-type
//! line + Defense, feeding the melee attack table's skill-vs-defense rolls (miss / dodge / parry / block)
//! and rising on use.
//!
//! BASELINE SAFETY: an untracked skill (no row) falls back to `level * SKILL_PER_LEVEL`, so with an empty
//! table the computed `skill_diff = defense_skill - weapon_skill` equals exactly the level-derived
//! `(target.level - attacker.level) * 5` — every attack-table band is byte-identical until a skill
//! actually diverges from cap. The skill-up hook caps at `level*5`, so a maxed skill is a no-op.
//!
//! SCOPE: server-authoritative skills + the combat consumption + skill-up-on-use, verified
//! server-side via `debug_compute_swing`. DEFERRED: the client skill-pane DISPLAY (the `PLAYER_SKILL_INFO`
//! descriptor array — blocked by the update-mask descriptor wall), class-specific weapon learnability, and
//! basing glancing/crushing on skill (they stay level-based). [entity]

use spacetimedb::{table, Identity, ReducerContext, Table};

use lyracore_shared::constants::starter_item;

// `game_player_skill` is defined in THIS module (the `#[table]` accessor is generated here, like
// `threat::game_threat`), so it's in scope without a `use`. `game_item_template` (items/) is read to
// resolve the equipped weapon's subclass; `game_world_entity` (world.rs) only by the debug reducer.
use crate::skilldata::game_skill_ability;
use crate::{game_item_template, game_world_entity, WorldEntity}; // accessor trait for the autolearn read (282)

// ===========================================================================================
//  Pure skill math + taxonomy (ctx-free, unit-tested) [server]
// ===========================================================================================

/// Skill points gained per character level — the cap on every skill line is `level * SKILL_PER_LEVEL`
/// (vanilla: weapon/defense skill maxes at level×5). Also the level→skill conversion the baseline
/// fallback uses, so an untracked skill reads as if perfectly trained for its level.
pub const SKILL_PER_LEVEL: u32 = 5;

/// Real vanilla `SkillLine.dbc` IDs (kept verbatim so the DEFERRED client skill-pane relay is a straight
/// read with no remap — a local enum would force a translation layer at the descriptor boundary). Only
/// the lines combat/professions can actually produce are defined; ranged lines come with their features.
///
/// DEPRECATION POINTER (work-item 208): `game_skill_line` (`skilldata.rs`), loaded from the operator's
/// real `SkillLine.dbc` by the importer, is now the AUTHORITATIVE source for "what skill lines exist" —
/// it carries every vanilla line (~135), not just the ~17 hand-picked here. These consts stay because
/// the wire protocol is still keyed on the same verbatim ids (no remap needed either way), but treat
/// `game_skill_line` as the source of truth for anything beyond this hardcoded combat/profession subset.
pub mod skill_line {
    pub const AXE_1H: u32 = 44;
    pub const AXE_2H: u32 = 172;
    pub const MACE_1H: u32 = 54;
    pub const MACE_2H: u32 = 160;
    pub const POLEARM: u32 = 229;
    pub const SWORD_1H: u32 = 43;
    pub const SWORD_2H: u32 = 55;
    pub const STAFF: u32 = 136;
    pub const FIST: u32 = 473;
    pub const DAGGER: u32 = 173;
    pub const UNARMED: u32 = 162;
    pub const DEFENSE: u32 = 95;
    // --- Profession lines (verbatim vanilla SkillLine.dbc IDs, same convention as the combat lines so the
    // client skill-pane relay is a straight read). Unlike combat skills these are NOT level-capped (cooking
    // can reach 75 at level 1) — the profession learn path uses `max_rank=75` directly, never `level*5`.
    pub const COOKING: u32 = 185;
    pub const SKINNING: u32 = 393; // skin→leather producer
    pub const LEATHERWORKING: u32 = 165; // skin→leather→CRAFT consumer
    pub const HERBALISM: u32 = 182; // gather: herb nodes -> herbs
    pub const MINING: u32 = 186; // gather: mining veins -> ore
                                 // --- Profession-breadth batch (verbatim vanilla SkillLine.dbc IDs). Four new LEARNABLE craft lines;
                                 // SMELTING is NOT a separate skill — it rides MINING (186), so a recipe under MINING with no learn marker.
    pub const ALCHEMY: u32 = 171;
    pub const FIRST_AID: u32 = 129;
    pub const TAILORING: u32 = 197;
    pub const BLACKSMITHING: u32 = 164;
    // --- The final three professions (completing the 13). Verbatim vanilla SkillLine.dbc IDs, same
    // convention. ENGINEERING is a craft line (recipes → products incl. on-use BOMBS); ENCHANTING is a
    // craft line driven by disenchant/enchant reducers (a server-side per-instance enchant overlay);
    // FISHING is a secondary gather line driven by the `fish` reducer (immediate-catch, no bobber).
    pub const ENGINEERING: u32 = 202;
    pub const ENCHANTING: u32 = 333;
    pub const FISHING: u32 = 356;
    // Work-item 211 (Lock.dbc): the SkillLine a `ty==LocktypeReference` Lock.dbc index resolves to when
    // its LockType.dbc id is 1 (Lockpicking). Not otherwise consumed this slice (no lockpicking reducer
    // yet — `game_lock` is data-only until work-item 119) but kept alongside the other verbatim SkillLine
    // IDs so the importer's LockType→SkillLine map (`importer/src/dbc.rs`) has ONE source of truth to
    // cite in its doc comment rather than a bare magic number.
    pub const LOCKPICKING: u32 = 633;
}

/// Is `line` one of the 13 player professions (primary/secondary craft + gather lines)? Used to
/// tell a CRAFT recipe's skill-line ability apart from a combat/class spell ability when the recipe
/// gate reads `game_skill_ability` (282) — a reagent-consuming BUFF (Arcane Intellect) pairs with a
/// magic-school line, not a profession, so it never counts as a craft.
pub fn is_profession_line(line: u32) -> bool {
    use skill_line::*;
    matches!(
        line,
        COOKING
            | FIRST_AID
            | TAILORING
            | LEATHERWORKING
            | BLACKSMITHING
            | ENGINEERING
            | ALCHEMY
            | ENCHANTING
            | MINING
            | HERBALISM
            | SKINNING
            | FISHING
    )
}

/// Apprentice rank cap for a freshly-learned primary/secondary profession (vanilla: the first training tier
/// caps at 75). A profession row is born `current: 1, max_rank: APPRENTICE_CAP` and climbs toward it on use —
/// NOT bounded by `level*5` (the combat invariant), so a level-1 character can train cooking to 75.
pub const APPRENTICE_CAP: u16 = 75;

/// Map a vanilla weapon `ItemTemplate.subclass` to its `skill_line`. Anything unrecognized (or no weapon)
/// resolves to `UNARMED` — the line an empty-handed swing trains, so a fist/unmapped weapon never errors.
pub fn weapon_subclass_to_skill_line(subclass: u8) -> u32 {
    use skill_line::*;
    match subclass {
        0 => AXE_1H,
        1 => AXE_2H,
        4 => MACE_1H,
        5 => MACE_2H,
        6 => POLEARM,
        7 => SWORD_1H,
        8 => SWORD_2H,
        10 => STAFF,
        13 => FIST,
        15 => DAGGER,
        _ => UNARMED, // unarmed, thrown, wands, and any unmapped subclass train Unarmed
    }
}

/// The skill cap for a unit of `level` — `level * SKILL_PER_LEVEL`. Both the max a tracked skill can reach
/// and the baseline value an UNtracked skill reads as (so combat against an equal-level foe nets 0).
pub fn skill_cap_for_level(level: u32) -> u32 {
    level * SKILL_PER_LEVEL
}

/// The attack-table skill difference (in skill points) driving miss/dodge/parry/block: how far the
/// defender's defense skill exceeds the attacker's weapon skill, clamped at 0 (a higher-skilled attacker
/// adds nothing — the band floors). The single definition both live combat and the readback use.
pub fn skill_diff(defense_skill: u32, weapon_skill: u32) -> u32 {
    defense_skill.saturating_sub(weapon_skill)
}

/// One skill-up step: `current + 1`, clamped to `cap`. A skill already at cap is unchanged.
pub fn next_skill(current: u32, cap: u32) -> u32 {
    (current + 1).min(cap)
}

// -------------------------------------------------------------------------------------------
//  COMBAT skill-up curve — the behavioural spec this section implements
// -------------------------------------------------------------------------------------------
//
// Vanilla weapon/Defense skill-up mechanics are publicly documented community knowledge. The
// observable contract, restated here as the thing the code below has to satisfy — it is written
// FROM this spec, not transcribed from any particular implementation of it:
//
//   * A qualifying event (a landed melee swing for a weapon line, a landed hit taken for Defense)
//     rolls once for a single +1. Nothing else moves a combat line.
//   * The only thing that drives the roll is HEADROOM — how far the line sits below ITS OWN cap.
//     There is no mob-level / grey-target gate on this path (that band belongs to the profession
//     curve, `skillup_chance_bp`, which is unrelated and stays separate). A maxed line never rolls.
//   * Wide headroom is a CATCH-UP regime: the chance grows with headroom, only logarithmically
//     (skilling up an untrained line is fast at first and flattens), and it grows faster for low
//     levels so a low-level character is not stuck grinding a line their cap already moved past.
//   * Narrow headroom is a TAPER regime: the chance falls off quadratically to exactly zero as the
//     line touches cap, so the last few points are the slow ones.
//   * The two regimes MEET, they do not step: at the seam the catch-up term contributes nothing and
//     both branches evaluate to the same floor. See `SEAM_HEADROOM`.
//   * Weapon lines get a small additive assist from intellect (a smart character picks a weapon up
//     faster), capped at +10 percentage points. Defense gets no such assist.
//   * Degenerate input (no cap, no level, at/over cap) is a hard 0 — never roll.
//
// Everything below is expressed in that vocabulary: headroom, seam, catch-up, taper, assist.

/// Probability scale: every skill chance in this module is basis points, 0..=10000.
const BP_SCALE: f64 = 10_000.0;

/// The headroom at which the combat curve changes regime (catch-up above, taper below), in skill
/// points. Chosen so the two branches AGREE there rather than stepping: the catch-up logarithm is
/// written to be zero at the seam, leaving the shared floor `SEAM_HEADROOM / cap`, and the taper
/// is written as `headroom² / (SEAM_HEADROOM * cap)`, which at `headroom == SEAM_HEADROOM` is the
/// same `SEAM_HEADROOM / cap`. Continuity is therefore a property of the algebra, not a tuned
/// coincidence — the `..._seam_is_continuous` test guards it.
const SEAM_HEADROOM: u32 = 7;

/// Additive offset inside the catch-up logarithm, in skill points: it is what makes the log
/// argument reach 1 (and so the whole term vanish) exactly at the seam.
const CATCH_UP_LOG_OFFSET: f64 = 5.0;

/// How steeply the catch-up regime rewards headroom, and how much steeper it gets at low level
/// (`slope = BASE * (1 + LOW_LEVEL_LIFT / level)` — at level 12 the slope has doubled).
const CATCH_UP_BASE_SLOPE: f64 = 0.22;
const CATCH_UP_LOW_LEVEL_LIFT: f64 = 12.0;

/// The most intellect can add to a WEAPON line's per-hit chance (as a fraction, so +10 points).
const INTELLECT_ASSIST_MAX: f64 = 0.10;

/// `(level ceiling, reference level, intellect worth the full assist)` — the assist is scaled both
/// by how much intellect the character has against the band's reference amount and by how far below
/// the band's reference level they are. A vanilla realm caps at 60 and so only ever reads the first
/// band; the rest keep the function total for any level a debug lever might set.
const INTELLECT_ASSIST_BANDS: [(u32, f64, f64); 4] = [
    (60, 60.0, 750.0),
    (70, 70.0, 1500.0),
    (80, 80.0, 3000.0),
    (u32::MAX, 255.0, 6000.0),
];

/// How far `current` sits below `cap`, or `None` when no roll may happen at all (no cap, no level,
/// or the line is already at/over its cap). The single gate the whole curve is guarded by.
fn combat_headroom(current: u32, cap: u32, level: u32) -> Option<u32> {
    if cap == 0 || level == 0 {
        return None;
    }
    cap.checked_sub(current).filter(|&headroom| headroom > 0)
}

/// The headroom-driven part of the chance, as a fraction: catch-up above the seam, taper below it.
/// Both regimes are anchored on the shared floor `SEAM_HEADROOM / cap` (see `SEAM_HEADROOM`).
fn combat_practice_fraction(headroom: u32, cap: u32, level: u32) -> f64 {
    let seam = SEAM_HEADROOM as f64;
    let headroom_f = headroom as f64;
    let cap_f = cap as f64;
    let seam_floor = seam / cap_f;
    if headroom >= SEAM_HEADROOM {
        // CATCH-UP. The log argument is normalised on the seam, so it is 1 (and the term 0) there
        // and grows from there with headroom; the slope steepens as level drops.
        let slope = CATCH_UP_BASE_SLOPE * (1.0 + CATCH_UP_LOW_LEVEL_LIFT / level as f64);
        seam_floor
            + slope * ((headroom_f + CATCH_UP_LOG_OFFSET) / (seam + CATCH_UP_LOG_OFFSET)).ln()
    } else {
        // TAPER. Quadratic in headroom, normalised so it hits `seam_floor` at the seam and 0 at cap.
        (headroom_f * headroom_f) / (seam * cap_f)
    }
}

/// The intellect assist for a WEAPON line, as a fraction (Defense never calls this). Bounded by
/// `INTELLECT_ASSIST_MAX` at both ends, so a wildly-statted character cannot run away with it.
fn intellect_assist(intellect: u32, level: u32) -> f64 {
    let (_, reference_level, full_assist_intellect) = INTELLECT_ASSIST_BANDS
        .iter()
        .copied()
        .find(|&(ceiling, _, _)| level <= ceiling)
        .unwrap_or(INTELLECT_ASSIST_BANDS[INTELLECT_ASSIST_BANDS.len() - 1]);
    (INTELLECT_ASSIST_MAX
        * (intellect as f64 / full_assist_intellect)
        * (reference_level / level as f64))
        .clamp(0.0, INTELLECT_ASSIST_MAX)
}

/// Chance (basis points, 0..=10000) that ONE qualifying hit trains a COMBAT (weapon or Defense) line
/// currently at `current`/`cap` for a unit of `level`, plus — for weapon lines only — the small
/// intellect assist. Implements the spec block above: headroom is the only driver, the catch-up and
/// taper regimes meet continuously at `SEAM_HEADROOM`, and a maxed/untracked/level-0 line is a hard 0.
///
/// BEHAVIOURAL REFERENCE (citation, not derivation): the mangos-family cores drive their weapon and
/// defence skill-ups off the line's own headroom on this path with no mob-level check, which is why
/// grinding a grey mob still trains a lagging weapon skill here. That is a statement about observable
/// behaviour we match — no implementation of it was copied. Pure → unit-tested at the seam, the
/// monotonicity contract, and the clamp edges.
pub fn combat_skillup_chance_bp(
    current: u32,
    cap: u32,
    level: u32,
    intellect: u32,
    weapon: bool,
) -> u32 {
    let Some(headroom) = combat_headroom(current, cap, level) else {
        return 0;
    };
    let practice = combat_practice_fraction(headroom, cap, level).clamp(0.0, 1.0);
    let assist = if weapon {
        intellect_assist(intellect, level)
    } else {
        0.0
    };
    ((practice + assist).clamp(0.0, 1.0) * BP_SCALE) as u32
}

/// Probability (in basis points, 0..=10000) that ONE use of a profession at `current` skill yields a +1
/// skill-up, given the recipe/node's `orange` (the required floor — always skills below it) and `gray`
/// (the ceiling — never skills at/above it) difficulty thresholds. A single linear band orange→gray
/// (vanilla's yellow/green nuance is collapsed into the one line — DEFERRED). Pure → unit-tested.
///
/// SENTINEL: `gray <= orange` (notably the default-band `orange=1, gray=0`) means "no difficulty / always
/// skill up" → returns 10000. The seeded recipes/nodes carry the default band, so every craft/gather
/// deterministically +1s.
pub fn skillup_chance_bp(current: u32, orange: u32, gray: u32) -> u32 {
    if gray <= orange {
        return 10_000; // degenerate/disabled band (incl. the default sentinel) => always skill up
    }
    if current < orange {
        return 10_000; // below the required floor => always (vanilla: grey-name recipes you can't pick)
    }
    if current >= gray {
        return 0; // at/above gray => never
    }
    // Linear interpolation across [orange, gray): (gray - current) / (gray - orange), in basis points.
    let span = gray - orange;
    (gray - current) * 10_000 / span
}

/// Pure rank/cap-raise decision (ctx-free, unit-tested): the `max_rank` an EXISTING profession row should
/// carry after a rank-up offering of ceiling `offered_cap` — `max(current_cap, offered_cap)`. A higher
/// tier (Journeyman 150 over Apprentice 75) lifts the ceiling; re-buying the same/lower tier is a no-op
/// (the idempotency the trainer's already-known gate also enforces). `learn_profession` uses exactly this
/// for the present-row case (a NEW row instead just inserts `1/offered_cap`).
pub fn raised_cap(current_cap: u16, offered_cap: u16) -> u16 {
    current_cap.max(offered_cap)
}

// ===========================================================================================
//  Table [entity] — per-player, owner-scoped
// ===========================================================================================

/// One trained skill line for one character: `(character_guid, skill_line)` logical key (an `#[auto_inc]`
/// PK + a `by_character` btree, mirroring `game_threat`'s `(creature, source)` idiom — no lossy guid pack).
/// `owner_identity` scopes the RLS like `game_item_instance`/`game_character`. `current`/`max_rank` are the
/// trained value and the level cap (both ≤ level×5 ≤ 300, so `u16`). No Timestamp → SQL-inspectable. [entity]
#[table(accessor = game_player_skill, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerSkill {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub skill_line: u32,
    pub current: u16,
    pub max_rank: u16,
}

/// A player connection sees only its own skills (mirrors the item/character RLS filters).
#[spacetimedb::client_visibility_filter]
const PLAYER_SKILL_RLS: spacetimedb::Filter =
    spacetimedb::Filter::Sql("SELECT * FROM game_player_skill WHERE owner_identity = :sender");

// Skills are deleted on character delete, re-owned (identity re-stamp) on a relog under a changed
// gateway identity.
crate::character_owned!(delete, fn sweep_delete_game_player_skill(ctx, character_guid) {
    let skills = ctx.db.game_player_skill();
    for r in skills.by_character().filter(&character_guid).collect::<Vec<_>>() {
        skills.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19), HOT: weapon/defense/profession skill values gate every
// swing the character makes on arrival. `id` is a surrogate PK, re-minted.
crate::character_owned!(transfer, fn sweep_transfer_game_player_skill(ctx, character_guid, io) {
    table = game_player_skill,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_player_skill(ctx, character_guid, identity) {
    let skills = ctx.db.game_player_skill();
    for mut r in skills.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            skills.id().update(r);
        }
    }
});

// ===========================================================================================
//  ctx readers — effective skills with the level*5 fallback [entity]
// ===========================================================================================

/// The `skill_line` a unit's currently-equipped main-hand weapon trains: the weapon's subclass → line, or
/// `UNARMED` when there's no main-hand item, a non-weapon/broken item in it, or a missing template — the
/// same gate `combat::equipped_weapon_damage` uses, so weapon damage and weapon skill agree on "unarmed".
fn equipped_weapon_skill_line(ctx: &ReducerContext, guid: u64) -> u32 {
    let Some(inst) = crate::items::item_in_slot(ctx, guid, starter_item::MAINHAND_SLOT) else {
        return skill_line::UNARMED;
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(inst.entry) else {
        return skill_line::UNARMED;
    };
    if tmpl.class != starter_item::CLASS_WEAPON || crate::items::item_is_broken(&tmpl, &inst) {
        return skill_line::UNARMED;
    }
    weapon_subclass_to_skill_line(tmpl.subclass)
}

/// The trained value of `guid`'s `line`, or the `level*5` baseline when untracked. The fallback is what
/// makes combat byte-identical before any skill diverges from cap.
fn skill_value_or_cap(ctx: &ReducerContext, guid: u64, line: u32, level: u32) -> u32 {
    ctx.db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)
        .map(|s| s.current as u32)
        .unwrap_or_else(|| skill_cap_for_level(level))
}

/// A unit's EFFECTIVE weapon skill: the trained value of its equipped weapon's line, else the `level*5`
/// baseline. A creature (no items) always reads the baseline → its swings are unchanged.
pub fn effective_weapon_skill(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let line = equipped_weapon_skill_line(ctx, attacker.guid);
    skill_value_or_cap(ctx, attacker.guid, line, attacker.level)
}

/// A unit's EFFECTIVE defense skill: the trained Defense value (else the `level*5` baseline) PLUS any
/// `A_MOD_COMBAT(COMBAT_DEFENSE)` aura (e.g. the Anticipation talent). More defense raises
/// `skill_diff` against an attacker, so it tightens the attacker's miss/dodge/parry/block bands — a
/// defender's avoidance goes up. No defense aura → exactly the trained/baseline value (baseline-safe). [entity]
pub fn effective_defense_skill(ctx: &ReducerContext, target: &WorldEntity) -> u32 {
    let base = skill_value_or_cap(ctx, target.guid, skill_line::DEFENSE, target.level);
    let bonus = crate::spell::combat_field_bonus(ctx, target.guid, crate::spell::COMBAT_DEFENSE);
    (base as i32 + bonus).max(0) as u32
}

/// The attack-table skill difference for a swing of `attacker` at `target`: the defender's defense skill
/// minus the attacker's weapon skill (clamped ≥0). With no tracked rows this is exactly
/// `(target.level - attacker.level) * 5` — the pure level-derived `skill_diff` (baseline-safe). [entity]
pub fn skill_diff_ctx(ctx: &ReducerContext, attacker: &WorldEntity, target: &WorldEntity) -> u32 {
    skill_diff(
        effective_defense_skill(ctx, target),
        effective_weapon_skill(ctx, attacker),
    )
}

// ===========================================================================================
//  ctx mutators — lazy init + skill-up on use [entity]
// ===========================================================================================

/// Raise an EXISTING `(character_guid, line)` row one step toward ITS OWN `max_rank`. A no-op once at
/// `max_rank` and a no-op when the row is ABSENT: an untracked line already reads as the `level*5`
/// baseline, so there's nothing to climb (rows are born at login / `debug_set_skill`, never lazily here —
/// which also keeps creatures, never seeded, free of skill rows). Capping on the row's stored `max_rank`
/// (not the live `level*5`) preserves the `current ≤ max_rank` invariant and naturally admits a future
/// >`level*5` cap from a +weapon-skill racial/item. The visible climb window is `raise_combat_caps` lifting
/// > `max_rank` above `current` on a ding — without that, every combat line would be born and stay at cap,
/// > making this a permanent no-op.
///
/// Returns the row's `current` AFTER the call (whether or not it actually climbed), or `None` if
/// the row is ABSENT — callers that need the post-raise value (`gain_profession_skill`'s autolearn
/// check) use this instead of re-reading the row they just wrote.
fn raise_skill(ctx: &ReducerContext, guid: u64, line: u32) -> Option<u32> {
    let skills = ctx.db.game_player_skill();
    let mut row = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)?;
    let next = next_skill(row.current as u32, row.max_rank as u32) as u16;
    if next != row.current {
        row.current = next;
        skills.id().update(row);
    }
    Some(next as u32)
}

/// Roll `combat_skillup_chance_bp` for `guid`'s EXISTING `(line)` row and, on success, step it via
/// `raise_skill`. A no-op when the row is ABSENT (untracked lines read the `level*5` baseline — nothing
/// to climb) — the same guard `raise_skill` itself uses. `gain_weapon_skill` / `gain_defense_skill` both
/// route through this shared entry point.
fn roll_and_raise_skill(
    ctx: &ReducerContext,
    guid: u64,
    line: u32,
    level: u32,
    intellect: u32,
    weapon: bool,
) {
    let Some(row) = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)
    else {
        return;
    };
    let chance = combat_skillup_chance_bp(
        row.current as u32,
        row.max_rank as u32,
        level,
        intellect,
        weapon,
    );
    if chance > 0 && ctx.random::<u32>() % 10_000 < chance {
        raise_skill(ctx, guid, line);
    }
}

/// Skill-up the attacker's equipped weapon line on a landed player→creature hit. Gated by the caller on
/// `attacker.is_player()` + a real hit; trains the line the swing actually used, at the
/// headroom/level/intellect chance (`combat_skillup_chance_bp`, `weapon = true`).
pub fn gain_weapon_skill(ctx: &ReducerContext, attacker: &WorldEntity) {
    let line = equipped_weapon_skill_line(ctx, attacker.guid);
    roll_and_raise_skill(
        ctx,
        attacker.guid,
        line,
        attacker.level,
        attacker.intellect,
        true,
    );
}

/// Skill-up the defender's Defense line on a landed hit it survived. Gated by the caller on the defender
/// being a player, at the headroom/level chance (`combat_skillup_chance_bp`, `weapon = false` — vanilla
/// gives the intellect assist to weapon lines only, so Defense never gets it).
pub fn gain_defense_skill(ctx: &ReducerContext, target: &WorldEntity) {
    roll_and_raise_skill(
        ctx,
        target.guid,
        skill_line::DEFENSE,
        target.level,
        0,
        false,
    );
}

/// The COMBAT (weapon + Defense) skill lines — the set whose `max_rank` tracks `level*5`, as opposed to
/// profession lines (Cooking, Mining, …) whose cap is an independent apprentice/journeyman/…/artisan tier
/// set by `learn_profession`. Used by `raise_combat_caps` to know which rows to lift on a ding/login and
/// never touch a profession row's cap. `pub(crate)` (weapon masters, work-item 202): `trainer::apply_trainer_buy`
/// forks its profession-learn branch on this same taxonomy — a weapon-master offering (`learn_skill_line` set
/// to a weapon line) needs a LEVEL-derived cap, not the offering's static tier column.
pub(crate) fn is_combat_skill_line(line: u32) -> bool {
    use skill_line::*;
    matches!(
        line,
        AXE_1H
            | AXE_2H
            | MACE_1H
            | MACE_2H
            | POLEARM
            | SWORD_1H
            | SWORD_2H
            | STAFF
            | FIST
            | DAGGER
            | UNARMED
            | DEFENSE
    )
}

/// Raise every COMBAT-line row's `max_rank` to `level*5` for `guid` — never LOWERS it (a future
/// +weapon-skill racial/item bonus could already exceed it) and never touches `current` (that lag IS the
/// visible skill-up window) or a profession row (guarded by `is_combat_skill_line`). Called from BOTH the
/// mid-session ding path (`xp::grant_xp`) and login (`ensure_player_skills`, reconciling an ALREADY-dinged
/// character whose caps were frozen at first-login level — so a relog alone un-sticks a stale character,
/// not just future dings). [entity]
///
/// Lockpicking (633, work-item 172) also tracks `level*5` in vanilla but isn't part of
/// `is_combat_skill_line` — that predicate is shared with `trainer::apply_trainer_buy`'s weapon-master
/// fork, and Lockpicking is never a trainer offering (it's granted by learning Pick Lock, see
/// `grant_lockpicking_on_learn`), so it's checked here as a second, dedicated clause instead of joining
/// the shared taxonomy.
pub(crate) fn raise_combat_caps(ctx: &ReducerContext, guid: u64, new_level: u32) {
    let cap = skill_cap_for_level(new_level) as u16;
    let skills = ctx.db.game_player_skill();
    for mut row in skills.by_character().filter(&guid).collect::<Vec<_>>() {
        let tracks_level =
            is_combat_skill_line(row.skill_line) || row.skill_line == skill_line::LOCKPICKING;
        if tracks_level && row.max_rank < cap {
            row.max_rank = cap;
            skills.id().update(row);
        }
    }
}

/// Skill-up a PROFESSION line (e.g. Cooking) after a successful craft/gather: ROLL the orange/gray
/// difficulty band (`skillup_chance_bp`) and, on success, climb one step toward the row's own `max_rank`
/// via `raise_skill` (caps on the row's stored cap — 75/150/…, NOT `level*5`, so a profession climbs
/// independent of level). A no-op when the profession isn't learned (no row) — the same ABSENT-row guard
/// `raise_skill` already uses for combat lines.
///
/// DEFAULT BAND: callers seeded with the sentinel band (`orange=1, gray=0`, or any `gray<=orange`)
/// get a 10000bp chance → a deterministic +1 every use. Real difficulty tables (`gray>0`) are a
/// DEFERRED data concern.
///
/// Combat skill-ups (`gain_weapon_skill`/`gain_defense_skill`) DO NOT route through here — they roll
/// their own headroom curve (`combat_skillup_chance_bp`), not the orange/gray band.
pub(crate) fn gain_profession_skill(
    ctx: &ReducerContext,
    guid: u64,
    line: u32,
    orange: u32,
    gray: u32,
) {
    let current = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)
        .map(|s| s.current as u32)
        .unwrap_or(0);
    if ctx.random::<u32>() % 10_000 < skillup_chance_bp(current, orange, gray) {
        let Some(new_current) = raise_skill(ctx, guid, line) else {
            return;
        };
        // AUTOLEARN at threshold (282): climbing may have unlocked acquire_method=2 abilities whose
        // min_skill the new rank now meets. Idempotent; owner from the live caster entity (a skill-up
        // only ever fires for an in-world crafter). Requires a REAL live entity — the "always
        // in-world" invariant broke once already, and minting a `game_player_spell` row under
        // `Identity::ZERO` would be invisible to its owner under RLS forever, not just skipped.
        let Some(owner) = ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .map(|e| e.owner_identity)
        else {
            return;
        };
        grant_autolearn_abilities(ctx, guid, owner, line, new_current);
    }
}

/// Insert the `(guid, skill_line)` profession row at apprentice rank (`current: 1, max_rank: 75`) if it's
/// absent — the core learn-profession write, mirroring `ensure_player_skills`'s presence check + insert but
/// NOT level-capped (a profession's max is the apprentice cap, not `level*5`). Idempotent: a re-learn of an
/// already-known profession is a no-op (never resets a climbed skill). Called by the trainer-buy path
/// (`trainer::apply_trainer_buy`) when an offering carries `learn_skill_line > 0`, and by the
/// `debug_learn_profession` direct-grant lever below — so it's live in ALL builds. [entity]
pub(crate) fn learn_profession(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    skill_line: u32,
    cap: u16,
) {
    let skills = ctx.db.game_player_skill();
    if let Some(mut row) = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == skill_line)
    {
        // RANK-UP: an EXISTING profession row whose ceiling the next rank LIFTS
        // (Apprentice 75 → Journeyman 150 → …). Only raises `max_rank` (never resets `current`), and only
        // when the offered `cap` exceeds the stored ceiling — re-learning the SAME (or a lower) rank is a
        // no-op, so a default `cap == APPRENTICE_CAP` re-learn stays idempotent.
        let new_cap = raised_cap(row.max_rank, cap);
        if new_cap != row.max_rank {
            row.max_rank = new_cap;
            skills.id().update(row);
        }
    } else {
        skills.insert(PlayerSkill {
            id: 0, // auto_inc
            character_guid: guid,
            owner_identity: owner,
            skill_line,
            current: 1,
            max_rank: cap,
        });
    }
    // AUTOLEARN (282): learning a profession instantly grants its base abilities — this is how
    // Charred Wolf Meat (2538) / Disenchant (13262) arrive in vanilla (NOT from a trainer). Grant
    // at the row's current skill; higher-threshold autolearn abilities land as it climbs (via
    // gain_profession_skill below).
    let current = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == skill_line)
        .map(|s| s.current as u32)
        .unwrap_or(1);
    grant_autolearn_abilities(ctx, guid, owner, skill_line, current);
}

/// AUTOLEARN grant (282): for `skill_line`, learn every `game_skill_ability` spell whose real
/// `acquire_method` is autolearn (1 = on skill learn, 2 = on reaching the skill rank) and whose
/// `min_skill <= current`. Idempotent (`learn_spell` dedups), so it's safe to call on every learn +
/// skill-up. Race/class masks on profession recipes are universal in vanilla, so they're not
/// filtered here — a class-gated autolearn line would need that check (none in the 13 professions).
pub(crate) fn grant_autolearn_abilities(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    skill_line: u32,
    current: u32,
) {
    let spells: Vec<u32> = ctx
        .db
        .game_skill_ability()
        .by_skill_line()
        .filter(&skill_line)
        .filter(|a| matches!(a.acquire_method, 1 | 2) && a.min_skill.max(0) as u32 <= current)
        .map(|a| a.spell_id)
        .collect();
    for spell_id in spells {
        crate::spell::learn_spell(ctx, guid, owner, spell_id);
    }
}

/// Pick Lock (Rogue) — the spell whose learn grants the Lockpicking (633) skill line (work-item 119).
/// Kept as ONE named constant (the isolated place a spell id is named, like the importer's name rescues)
/// because the skill GRANT keys on the id: the gateway routes the CAST by the E_OPEN_LOCK effect kind,
/// but a fresh Rogue must already own the 633 row when their first pick_lock lands — `apply_pick_lock`
/// reads `game_player_skill(633)`, and an absent row reads skill 0 and refuses every lock.
pub(crate) const PICK_LOCK_SPELL_ID: u32 = 1804;

/// Grant the Lockpicking (633) skill line when a character learns Pick Lock (work-item 119) — insert
/// `1 / level*5` if absent (mirrors `learn_profession`'s NEW-row insert, but LEVEL-capped like a weapon
/// line: vanilla Lockpicking maxes at level×5, not the 75 apprentice tier). Idempotent — a re-learn
/// never resets a climbed skill. A no-op for every other spell. Called from the single `learn_spell`
/// grant seam (`spell::spellbook`), so a trainer buy / quest reward / any grant path seeds it once. Owner
/// is passed through from the learn (the RLS stamp). [entity]
pub(crate) fn grant_lockpicking_on_learn(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    spell_id: u32,
) {
    if spell_id != PICK_LOCK_SPELL_ID {
        return;
    }
    let skills = ctx.db.game_player_skill();
    if skills
        .by_character()
        .filter(&guid)
        .any(|s| s.skill_line == skill_line::LOCKPICKING)
    {
        return; // already have the line — never reset a climbed skill
    }
    // Level from the live entity (a trainer-taught spell → the learner is in world); default 1 so the
    // cap is >= 5 and the born-at-1 `current` never exceeds `max_rank`.
    let level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .map(|e| e.level)
        .unwrap_or(1);
    let cap = skill_cap_for_level(level.max(1)) as u16;
    skills.insert(PlayerSkill {
        id: 0,
        character_guid: guid,
        owner_identity: owner,
        skill_line: skill_line::LOCKPICKING,
        current: 1,
        max_rank: cap,
    });
}

/// Learn a profession (e.g. Cooking = 185) on `character_guid` at rank-cap `cap`: insert its skill row at
/// `1/cap` if absent, else LIFT the existing ceiling to `cap` (the rank-up path — Apprentice 75 →
/// Journeyman 150 → …). Reads owner from the live entity (mirrors `debug_set_skill`). The headless learn
/// lever — lets a verify learn a profession AND rank it up without the trainer UI. `cap` is the new
/// max_rank (75 = Apprentice, 150 = Journeyman, 225 = Expert, 300 = Artisan). [entity]
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_profession(
    ctx: &ReducerContext,
    character_guid: u64,
    skill_line: u32,
    cap: u32,
) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    learn_profession(
        ctx,
        character_guid,
        entity.owner_identity,
        skill_line,
        cap as u16,
    );
    spacetimedb::log::info!(
        "debug_learn_profession: guid={character_guid} learned profession skill_line={skill_line} (cap {cap})"
    );
    Ok(())
}

/// The synthetic "profession-learn spell" marker ids the seeded trainer offers. They
/// are NEVER resolved as spells (no `game_spell` header / effects) — they exist ONLY as a
/// `game_trainer_spell.spell_id` whose `learn_skill_line` is set, and the trainer-buy branch grants the skill
/// directly. Picked in the seed's 500xx range to avoid colliding with any real imported spell id.
#[cfg(feature = "debug_reducers")]
const LEARN_COOKING_SPELL_ID: u32 = 50080; // -> game_trainer_spell row with learn_skill_line = COOKING (185)
#[cfg(feature = "debug_reducers")]
const LEARN_SKINNING_SPELL_ID: u32 = 50081; // -> learn_skill_line = SKINNING (393)
#[cfg(feature = "debug_reducers")]
const LEARN_LEATHERWORKING_SPELL_ID: u32 = 50082; // -> learn_skill_line = LEATHERWORKING (165)
#[cfg(feature = "debug_reducers")]
const LEARN_MINING_SPELL_ID: u32 = 50083; // -> learn_skill_line = MINING (186)
#[cfg(feature = "debug_reducers")]
const LEARN_HERBALISM_SPELL_ID: u32 = 50084; // -> learn_skill_line = HERBALISM (182)
                                             // --- Profession-breadth batch markers (same convention: a `game_trainer_spell.spell_id` whose
                                             // `learn_skill_line` is set; never resolved as a spell). SMELTING needs NO marker (it rides MINING).
#[cfg(feature = "debug_reducers")]
const LEARN_ALCHEMY_SPELL_ID: u32 = 50085; // -> learn_skill_line = ALCHEMY (171)
#[cfg(feature = "debug_reducers")]
const LEARN_FIRST_AID_SPELL_ID: u32 = 50086; // -> learn_skill_line = FIRST_AID (129)
#[cfg(feature = "debug_reducers")]
const LEARN_TAILORING_SPELL_ID: u32 = 50087; // -> learn_skill_line = TAILORING (197)
#[cfg(feature = "debug_reducers")]
const LEARN_BLACKSMITHING_SPELL_ID: u32 = 50088; // -> learn_skill_line = BLACKSMITHING (164)

/// DRIVE the real trainer-buy branch for a profession without the trainer-window UI —
/// the small-arg test lever (`character_guid` is small; the trainer's own guid is a u64 > 2^53 the CLI
/// `spacetime call` mangles, so the trainer is resolved SERVER-SIDE here, mirroring `debug_skin_nearest`'s
/// nearest-search). It finds the closest live TRAINER creature on the player's map, maps the requested
/// `profession` skill_line → its marker spell id (Cooking 185 → 50080, Skinning 393 → 50081), then funnels
/// through the shared `trainer::apply_trainer_buy` — exercising the SAME range/flag gate, `trainer_buy_check`
/// (charge/idempotency), and `learn_profession` grant the CMSG-routed buy would. The trainer must offer that
/// marker (parent SQL-seeds the two `game_trainer_spell` rows) or the buy returns "[0] does not teach".
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_profession_from_trainer(
    ctx: &ReducerContext,
    character_guid: u64,
    profession: u32,
) -> Result<(), String> {
    let spell_id = match profession {
        x if x == skill_line::COOKING => LEARN_COOKING_SPELL_ID,
        x if x == skill_line::SKINNING => LEARN_SKINNING_SPELL_ID,
        x if x == skill_line::LEATHERWORKING => LEARN_LEATHERWORKING_SPELL_ID,
        x if x == skill_line::MINING => LEARN_MINING_SPELL_ID,
        x if x == skill_line::HERBALISM => LEARN_HERBALISM_SPELL_ID,
        x if x == skill_line::ALCHEMY => LEARN_ALCHEMY_SPELL_ID,
        x if x == skill_line::FIRST_AID => LEARN_FIRST_AID_SPELL_ID,
        x if x == skill_line::TAILORING => LEARN_TAILORING_SPELL_ID,
        x if x == skill_line::BLACKSMITHING => LEARN_BLACKSMITHING_SPELL_ID,
        _ => {
            return Err(format!(
                "no profession-learn marker for skill_line {profession}"
            ))
        }
    };
    let player = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| format!("learner {character_guid} not in world"))?;
    // Nearest live TRAINER creature in the learner's OWN (map, instance) partition — same-partition
    // scan via `crate::helpers::nearest_entity` (the big trainer guid resolved here, not by the CLI).
    let trainer = crate::helpers::nearest_entity(ctx, &player, |e| {
        !e.is_player()
            && !e.dead
            && e.npc_flags & lyracore_shared::constants::npc_flags::TRAINER != 0
    })
    .ok_or_else(|| "no live trainer near learner".to_string())?;
    crate::trainer::apply_trainer_buy(ctx, character_guid, trainer.guid, spell_id)
}

/// The synthetic "weapon-learn spell" marker ids the seeded Weapon Master (51005) offers (work-item
/// 202) — same convention as the profession markers above: NEVER resolved as spells (no `game_spell`
/// header), they exist ONLY as a `game_trainer_spell.spell_id` whose `learn_skill_line` names a COMBAT
/// line (`is_combat_skill_line`), routing `trainer::apply_trainer_buy` onto the WEAPON fork (level-derived
/// cap + presence-known) instead of the profession fork (static-tier cap). Picked just past the
/// profession-breadth batch's high-water mark (50124) to avoid colliding with any seeded marker/recipe id.
#[cfg(feature = "debug_reducers")]
const LEARN_AXE_1H_SPELL_ID: u32 = 50130; // -> learn_skill_line = AXE_1H (44), required_level 1 (the buy-succeeds fixture)
#[cfg(feature = "debug_reducers")]
const LEARN_POLEARM_SPELL_ID: u32 = 50131; // -> learn_skill_line = POLEARM (229), required_level 60 (the level-refusal fixture)

/// DRIVE the real trainer-buy branch for a WEAPON PROFICIENCY (work-item 202) without the trainer-window
/// UI — the weapon-master twin of `debug_learn_profession_from_trainer` above, same shape: the requested
/// `skill_line` (Daggers/1H Axe/Polearm/…) maps to its marker spell id, the nearest live TRAINER creature on
/// the learner's map is resolved SERVER-SIDE (mirroring `debug_skin_nearest`'s nearest-search — the
/// trainer's own guid is a u64 > 2^53 the CLI `spacetime call` mangles), then funnels through the SAME
/// `trainer::apply_trainer_buy` — exercising the identical range/flag gate, `trainer_buy_check`, and (via
/// `is_combat_skill_line`) the WEAPON fork's level-derived cap + row-presence known-check the CMSG-routed
/// buy would. The trainer must offer that marker (the seeded fixture SQL) or the buy returns "[0] trainer
/// does not teach that spell".
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_weapon_from_trainer(
    ctx: &ReducerContext,
    character_guid: u64,
    skill_line: u32,
) -> Result<(), String> {
    let spell_id = match skill_line {
        x if x == self::skill_line::AXE_1H => LEARN_AXE_1H_SPELL_ID,
        x if x == self::skill_line::POLEARM => LEARN_POLEARM_SPELL_ID,
        _ => {
            return Err(format!(
                "no weapon-learn marker for skill_line {skill_line}"
            ))
        }
    };
    let player = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| format!("learner {character_guid} not in world"))?;
    // Nearest live TRAINER creature in the learner's OWN (map, instance) partition — same-partition
    // scan via `crate::helpers::nearest_entity` (the big trainer guid resolved here, not by the CLI).
    let trainer = crate::helpers::nearest_entity(ctx, &player, |e| {
        !e.is_player()
            && !e.dead
            && e.npc_flags & lyracore_shared::constants::npc_flags::TRAINER != 0
    })
    .ok_or_else(|| "no live trainer near learner".to_string())?;
    crate::trainer::apply_trainer_buy(ctx, character_guid, trainer.guid, spell_id)
}

/// The weapon skill lines a player of `class` is proficient with at character creation (vanilla
/// `playercreateinfo_spell` / SkillLine.dbc). Only returns melee/ranged lines that have a tracked
/// `skill_line::*` constant (UNARMED and DEFENSE are seeded unconditionally; wands/thrown/bow/gun/
/// crossbow fall to UNARMED in `weapon_subclass_to_skill_line` and are therefore NOT listed here).
/// Returns an empty slice for unknown classes (e.g. creatures).
///
/// DEPRECATION POINTER (work-item 208): this hand-authored per-class table could in principle be
/// derived from `game_skill_availability` (`SkillRaceClassInfo.dbc`'s `class_mask`, imported by
/// `skilldata.rs`) joined against `skill_line::*`'s known combat lines — that data-driven replacement
/// is a follow-up, not done here (this function's behavior is UNCHANGED by this item).
/// Vanilla STARTING weapon proficiencies (256) — what a FRESH character knows before visiting a
/// weapon master, matching vanilla's per-race/class proficiency spells (race-merged;
/// wand/thrown/bow/gun lines are unmodeled — ranged rides the UNARMED fallback). The FULL per-class
/// list below stays as the "ever learnable by this class" reference; seeding from THAT list gave every
/// fresh paladin swords/axes/polearms and made the weapon masters pointless (live find, faladin).
/// Existing characters are GRANDFATHERED: `ensure_player_skills` only inserts missing lines, never
/// deletes, so a pre-256 character keeps its wide set.
pub fn class_starting_weapon_skill_lines(class: u8) -> &'static [u32] {
    use skill_line::*;
    match class {
        // Warrior: 1H axes, 1H maces, 1H swords (createinfo grants exactly these three).
        1 => &[AXE_1H, MACE_1H, SWORD_1H],
        // Paladin: 1H + 2H maces.
        2 => &[MACE_1H, MACE_2H],
        // Hunter: 1H axes + daggers (bow/gun unmodeled).
        3 => &[AXE_1H, DAGGER],
        // Rogue: daggers (thrown unmodeled).
        4 => &[DAGGER],
        // Priest: 1H maces (wand unmodeled).
        5 => &[MACE_1H],
        // Shaman: 1H maces + staves.
        7 => &[MACE_1H, STAFF],
        // Mage: staves (wand unmodeled).
        8 => &[STAFF],
        // Warlock: daggers (wand unmodeled).
        9 => &[DAGGER],
        // Druid: daggers, 1H maces, staves.
        11 => &[DAGGER, MACE_1H, STAFF],
        _ => &[],
    }
}

pub fn class_weapon_skill_lines(class: u8) -> &'static [u32] {
    use skill_line::*;
    match class {
        // Warrior: axes, maces, polearms, swords, fist, daggers (no staves; ranged → UNARMED fallback, not listed).
        1 => &[
            AXE_1H, AXE_2H, MACE_1H, MACE_2H, POLEARM, SWORD_1H, SWORD_2H, FIST, DAGGER,
        ],
        // Paladin: 1H/2H axes, maces, polearms, 1H/2H swords.
        2 => &[
            AXE_1H, AXE_2H, MACE_1H, MACE_2H, POLEARM, SWORD_1H, SWORD_2H,
        ],
        // Hunter: axes (1H/2H), daggers, fist, polearms, staves, swords (1H/2H).
        3 => &[
            AXE_1H, AXE_2H, DAGGER, FIST, POLEARM, STAFF, SWORD_1H, SWORD_2H,
        ],
        // Rogue: daggers, fist, 1H maces, 1H swords.
        4 => &[DAGGER, FIST, MACE_1H, SWORD_1H],
        // Priest: daggers, 1H maces, staves (wand → UNARMED, not listed).
        5 => &[DAGGER, MACE_1H, STAFF],
        // Shaman: 1H/2H axes, daggers, fist, 1H/2H maces, staves.
        7 => &[AXE_1H, AXE_2H, DAGGER, FIST, MACE_1H, MACE_2H, STAFF],
        // Mage: daggers, staves, 1H swords (wand → UNARMED, not listed).
        8 => &[DAGGER, STAFF, SWORD_1H],
        // Warlock: daggers, staves, 1H swords (wand → UNARMED, not listed).
        9 => &[DAGGER, STAFF, SWORD_1H],
        // Druid: daggers, fist, 1H/2H maces, staves.
        11 => &[DAGGER, FIST, MACE_1H, MACE_2H, STAFF],
        _ => &[],
    }
}

/// Lazily seed a logging-in player's skill rows (Defense + Unarmed + all class weapon proficiency
/// lines) at the level cap, owner-scoped for the RLS. Idempotent — only inserts lines the
/// character lacks, so a relog never duplicates (mirrors `grant_starter_item`). Seeding at cap
/// keeps a fresh login baseline (defense = weapon = level*5 → 0 skill_diff vs an equal-level foe).
/// `class` is the vanilla class id (1=Warrior … 11=Druid) from `Character.class`. [entity]
pub fn ensure_player_skills(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    level: u32,
    class: u8,
) {
    let cap = skill_cap_for_level(level) as u16;
    let equipped_weapon = equipped_weapon_skill_line(ctx, guid);
    let skills = ctx.db.game_player_skill();

    // Build the full set of lines to seed: always Defense + Unarmed, plus every class proficiency
    // line, plus the currently-equipped weapon (covers classes with no matching proficiency row,
    // and ensures the equipped slot is seeded even for odd starting gear).
    let mut lines: std::collections::HashSet<u32> = std::collections::HashSet::new();
    lines.insert(skill_line::DEFENSE);
    lines.insert(skill_line::UNARMED);
    lines.insert(equipped_weapon);
    // 256: seed only the vanilla STARTING subset — the wide class_weapon_skill_lines list made
    // every weapon-master purchase moot. The equipped-weapon insert above still covers odd
    // starting gear, and the equip gate (row presence) now genuinely refuses unlearned types.
    for &l in class_starting_weapon_skill_lines(class) {
        lines.insert(l);
    }

    for line in lines {
        let present = skills
            .by_character()
            .filter(&guid)
            .any(|s| s.skill_line == line);
        if !present {
            skills.insert(PlayerSkill {
                id: 0, // auto_inc
                character_guid: guid,
                owner_identity: owner,
                skill_line: line,
                current: cap,
                max_rank: cap,
            });
        }
    }

    // Reconcile PRE-EXISTING rows too: a character's combat lines can lag the current level's cap
    // (frozen at an earlier login's cap) — lift them here so a relog alone un-sticks a stale
    // character, not just future dings.
    raise_combat_caps(ctx, guid, level);
}

/// Set (find-or-insert) `character_guid`'s `skill_line` to `value`, clamped to the level cap — the
/// server-side verification lever (drop a weapon skill below cap, then `debug_compute_swing` /
/// fight to see the bands shift + the skill climb). Reads level + owner from the live entity.
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_set_skill(
    ctx: &ReducerContext,
    character_guid: u64,
    skill_line: u32,
    value: u32,
) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    let cap = skill_cap_for_level(entity.level);
    let current = value.min(cap) as u16;
    let max_rank = cap as u16;
    let skills = ctx.db.game_player_skill();
    if let Some(mut row) = skills
        .by_character()
        .filter(&character_guid)
        .find(|s| s.skill_line == skill_line)
    {
        row.current = current;
        row.max_rank = max_rank;
        skills.id().update(row);
    } else {
        skills.insert(PlayerSkill {
            id: 0,
            character_guid,
            owner_identity: entity.owner_identity,
            skill_line,
            current,
            max_rank,
        });
    }
    spacetimedb::log::info!(
        "debug_set_skill: guid={character_guid} line={skill_line} -> {current}/{max_rank}"
    );
    Ok(())
}

/// Force-run `ensure_player_skills` for an (offline) character from the CLI — the headless
/// verification path for the login skill seed. Reads level + class from `game_character`; owner is
/// set to `ctx.sender()` (operator identity, fine for a verify-only run). All missing weapon
/// proficiency rows for the character's class are inserted idempotently.
#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_reseed_skills(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    // REFUSE verdict (issue #30): `game_player_skill` is a HOT manifest table, so a post-begin
    // reseed is a lost write cross-database.
    let ch = crate::helpers::require_character(ctx, character_guid)
        .map_err(|_| format!("no game_character row for guid {character_guid}"))?;
    ensure_player_skills(ctx, ch.guid, ctx.sender(), ch.level as u32, ch.class);
    spacetimedb::log::info!(
        "debug_reseed_skills: guid={character_guid} class={} level={}",
        ch.class,
        ch.level
    );
    Ok(())
}

// ===========================================================================================
//  Tests [server]
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_line_ids_are_the_verbatim_vanilla_dbc_values() {
        // These are wire-facing (the deferred client skill-pane reads them verbatim) — guard the literals
        // against a silent edit so the mapping test below can't pass with a wrong-but-consistent ID.
        assert_eq!(skill_line::SWORD_1H, 43);
        assert_eq!(skill_line::AXE_1H, 44);
        assert_eq!(skill_line::MACE_1H, 54);
        assert_eq!(skill_line::SWORD_2H, 55);
        assert_eq!(skill_line::DEFENSE, 95);
        assert_eq!(skill_line::STAFF, 136);
        assert_eq!(skill_line::MACE_2H, 160);
        assert_eq!(skill_line::UNARMED, 162);
        assert_eq!(skill_line::AXE_2H, 172);
        assert_eq!(skill_line::DAGGER, 173);
        assert_eq!(skill_line::POLEARM, 229);
        assert_eq!(skill_line::FIST, 473);
    }

    #[test]
    fn weapon_subclass_maps_to_vanilla_skill_lines() {
        assert_eq!(weapon_subclass_to_skill_line(7), skill_line::SWORD_1H);
        assert_eq!(weapon_subclass_to_skill_line(8), skill_line::SWORD_2H);
        assert_eq!(weapon_subclass_to_skill_line(0), skill_line::AXE_1H);
        assert_eq!(weapon_subclass_to_skill_line(15), skill_line::DAGGER);
        assert_eq!(weapon_subclass_to_skill_line(10), skill_line::STAFF);
        // Thrown/wand/unknown all train Unarmed (the fallback line).
        assert_eq!(weapon_subclass_to_skill_line(16), skill_line::UNARMED);
        assert_eq!(weapon_subclass_to_skill_line(19), skill_line::UNARMED);
        assert_eq!(weapon_subclass_to_skill_line(255), skill_line::UNARMED);
    }

    #[test]
    fn skill_cap_is_level_times_five() {
        assert_eq!(skill_cap_for_level(1), 5);
        assert_eq!(skill_cap_for_level(60), 300);
        assert_eq!(skill_cap_for_level(0), 0);
    }

    #[test]
    fn next_skill_steps_one_and_caps() {
        assert_eq!(next_skill(0, 300), 1);
        assert_eq!(next_skill(299, 300), 300);
        assert_eq!(next_skill(300, 300), 300); // already capped → unchanged
        assert_eq!(next_skill(305, 300), 300); // never exceeds cap even if over
    }

    /// COMBAT skill-up chance — the headroom curve specified above `combat_skillup_chance_bp`.
    /// Guards, one per spec bullet: degenerate inputs → 0 (never rolls); the seam is continuous
    /// (catch-up side meets taper side); chance is non-increasing as `current` climbs toward `cap`;
    /// Defense (weapon=false) gets no intellect assist while an otherwise-identical weapon line does.
    #[test]
    fn combat_skillup_chance_zero_on_degenerate_inputs() {
        assert_eq!(
            combat_skillup_chance_bp(10, 0, 10, 0, false),
            0,
            "cap=0 never rolls"
        );
        assert_eq!(
            combat_skillup_chance_bp(10, 300, 0, 0, false),
            0,
            "level=0 never rolls"
        );
        assert_eq!(
            combat_skillup_chance_bp(300, 300, 60, 0, false),
            0,
            "at cap never rolls"
        );
        assert_eq!(
            combat_skillup_chance_bp(305, 300, 60, 0, false),
            0,
            "over cap (shouldn't happen) never rolls"
        );
    }

    #[test]
    fn combat_skillup_chance_seam_is_continuous() {
        // Across the seam (headroom 6 = taper side, headroom 7 = catch-up side): no discontinuous
        // jump — the chance rises with headroom and only slightly at the crossing.
        let level = 60;
        let cap = 300u32;
        let at_headroom_6 = combat_skillup_chance_bp(cap - 6, cap, level, 0, false);
        let at_headroom_7 = combat_skillup_chance_bp(cap - 7, cap, level, 0, false);
        assert!(
            at_headroom_7 >= at_headroom_6,
            "chance must not drop when headroom grows (further from cap)"
        );
        let gap = at_headroom_7 - at_headroom_6;
        assert!(
            gap < 1000,
            "the seam should be near-continuous, got a {gap}bp jump"
        );
    }

    /// The seam is continuous BY CONSTRUCTION, not by tuning (see `SEAM_HEADROOM`): evaluated at
    /// exactly `SEAM_HEADROOM`, the catch-up branch's logarithm is zero, so it returns the shared
    /// floor `SEAM_HEADROOM / cap` — which is precisely what the taper branch's
    /// `headroom² / (SEAM_HEADROOM * cap)` evaluates to there, at every cap and every level.
    #[test]
    fn combat_practice_fraction_branches_agree_exactly_at_the_seam() {
        let seam = SEAM_HEADROOM as f64;
        for cap in [5u32, 60, 145, 300] {
            let taper_at_seam = (seam * seam) / (seam * cap as f64);
            for level in [1u32, 12, 30, 60] {
                let catch_up_at_seam = combat_practice_fraction(SEAM_HEADROOM, cap, level);
                assert_eq!(
                    catch_up_at_seam, taper_at_seam,
                    "branches must meet at the seam (cap={cap} level={level})"
                );
                // …and the shared value IS the floor the spec names.
                assert_eq!(catch_up_at_seam, seam / cap as f64);
            }
        }
    }

    /// The catch-up regime's low-level lift: at equal headroom, a lower-level character trains a
    /// lagging line strictly faster (that is the whole point of the `CATCH_UP_LOW_LEVEL_LIFT` term),
    /// and the effect vanishes as level grows. Headroom must be above the seam for the term to exist.
    #[test]
    fn combat_skillup_catch_up_is_faster_at_low_level() {
        let cap = 300u32;
        let headroom = 60u32; // well above the seam → catch-up regime
        let current = cap - headroom;
        let low = combat_skillup_chance_bp(current, cap, 10, 0, false);
        let mid = combat_skillup_chance_bp(current, cap, 30, 0, false);
        let high = combat_skillup_chance_bp(current, cap, 60, 0, false);
        assert!(
            low > mid,
            "level 10 must catch up faster than level 30 ({low} vs {mid})"
        );
        assert!(
            mid > high,
            "level 30 must catch up faster than level 60 ({mid} vs {high})"
        );
    }

    /// The intellect assist is bounded at `INTELLECT_ASSIST_MAX` (+10 points = 1000bp) no matter how
    /// absurd the intellect, and is exactly zero at zero intellect.
    #[test]
    fn intellect_assist_is_clamped_and_zero_without_intellect() {
        for level in [1u32, 30, 60] {
            assert_eq!(intellect_assist(0, level), 0.0, "no intellect => no assist");
            assert!(
                intellect_assist(u32::MAX, level) <= INTELLECT_ASSIST_MAX,
                "assist must clamp at {INTELLECT_ASSIST_MAX}"
            );
        }
        // The clamp is reachable, not vacuous: enough intellect saturates it.
        assert_eq!(intellect_assist(100_000, 60), INTELLECT_ASSIST_MAX);
    }

    #[test]
    fn combat_skillup_chance_is_non_increasing_as_current_climbs() {
        let cap = 300u32;
        let level = 60;
        let mut prev = 10_001u32;
        for current in 0..=cap {
            let c = combat_skillup_chance_bp(current, cap, level, 0, false);
            assert!(
                c <= prev,
                "chance must not rise: at {current}/{cap} got {c} after {prev}"
            );
            prev = c;
        }
    }

    #[test]
    fn combat_skillup_chance_weapon_intellect_bonus_only_applies_to_weapon_lines() {
        // Same room/level/cap, only `weapon` flips: the weapon line's chance must be >= the defense
        // line's (the intellect bonus only ever ADDS), and strictly greater with nonzero intellect.
        let (current, cap, level, intellect) = (100u32, 300u32, 60u32, 500u32);
        let defense = combat_skillup_chance_bp(current, cap, level, intellect, false);
        let weapon = combat_skillup_chance_bp(current, cap, level, intellect, true);
        assert!(
            weapon > defense,
            "weapon line should get a strictly higher chance from intellect > 0"
        );
        // Zero intellect: the bonus term is 0, so weapon and defense agree exactly.
        let weapon_no_int = combat_skillup_chance_bp(current, cap, level, 0, true);
        assert_eq!(
            weapon_no_int, defense,
            "zero intellect => no bonus => weapon matches defense"
        );
    }

    #[test]
    fn combat_skillup_chance_never_exceeds_10000bp() {
        for cap in [5u32, 60, 300] {
            for current in (0..cap).step_by(3) {
                for level in [1u32, 30, 60] {
                    let c = combat_skillup_chance_bp(current, cap, level, 750, true);
                    assert!(
                        c <= 10_000,
                        "chance {c} exceeds 10000bp at current={current} cap={cap} level={level}"
                    );
                }
            }
        }
    }

    #[test]
    fn skill_diff_is_defense_minus_weapon_clamped() {
        assert_eq!(skill_diff(300, 300), 0); // equal skill
        assert_eq!(skill_diff(315, 300), 15); // +3 levels of defense over weapon
        assert_eq!(skill_diff(300, 315), 0); // higher weapon skill floors at 0
    }

    /// The combat/profession taxonomy `apply_trainer_buy` forks the weapon-master branch on (work-item 202):
    /// a weapon line (DAGGER) is combat (level-capped); a profession line (COOKING) is not (static-tier capped).
    #[test]
    fn is_combat_skill_line_true_for_dagger_false_for_cooking() {
        assert!(
            is_combat_skill_line(skill_line::DAGGER),
            "Dagger is a combat (weapon) line"
        );
        assert!(
            !is_combat_skill_line(skill_line::COOKING),
            "Cooking is a profession line, not combat"
        );
    }

    #[test]
    fn profession_skill_lines_are_verbatim_vanilla_dbc_values() {
        // Wire-facing (the client skill-pane reads these verbatim) — guard the literals against a silent edit.
        assert_eq!(skill_line::COOKING, 185);
        assert_eq!(skill_line::SKINNING, 393);
        assert_eq!(skill_line::LEATHERWORKING, 165);
        assert_eq!(skill_line::HERBALISM, 182);
        assert_eq!(skill_line::MINING, 186);
        // Profession-breadth batch (vanilla SkillLine.dbc ids — DO NOT change).
        assert_eq!(skill_line::ALCHEMY, 171);
        assert_eq!(skill_line::FIRST_AID, 129);
        assert_eq!(skill_line::TAILORING, 197);
        assert_eq!(skill_line::BLACKSMITHING, 164);
        // The final three (completing the 13) — verbatim vanilla SkillLine.dbc ids, DO NOT change.
        assert_eq!(skill_line::ENGINEERING, 202);
        assert_eq!(skill_line::ENCHANTING, 333);
        assert_eq!(skill_line::FISHING, 356);
    }

    /// A learned profession is born at apprentice rank (1/75) and climbs ONE step per craft toward 75,
    /// INDEPENDENT of character level (the `next_skill` cap is the row's own `max_rank`, never `level*5`).
    /// This is the skill-add piece the cooking loop's skill-up reuses (`raise_skill` → `next_skill`).
    #[test]
    fn profession_skill_adds_one_per_craft_capped_at_apprentice_75() {
        let cap = APPRENTICE_CAP as u32; // 75 — NOT level*5
        assert_eq!(cap, 75);
        // Born at 1; the first craft climbs to 2 (the loop's observable skill-up).
        assert_eq!(next_skill(1, cap), 2);
        // Climbs one at a time toward the apprentice cap, independent of level (level 1 can reach 75).
        assert_eq!(next_skill(74, cap), 75);
        assert_eq!(next_skill(75, cap), 75); // at apprentice cap → unchanged (further ranks need a new tier)
                                             // A profession is NOT level-capped: skill 1 at level 1 (cap level*5 == 5) still climbs past 5 toward 75.
        assert!(
            skill_cap_for_level(1) < cap,
            "profession cap 75 must exceed the level-1 combat cap (5)"
        );
        assert_eq!(next_skill(5, cap), 6); // would be frozen at 5 under the combat cap; the profession path isn't
    }

    /// The skill-up DIFFICULTY curve (orange/yellow/green/gray, collapsed to one linear band): a use at/below
    /// the orange floor ALWAYS skills (10000bp); at/above gray NEVER (0bp); between, linearly interpolated
    /// `(gray - current)/(gray - orange)`; and the `gray<=orange` SENTINEL (incl. the default `1,0` band)
    /// is always-skill. This is the regression guard for the seeded recipes/nodes (their default band must
    /// stay at 100%) AND the curve shape for future difficulty tables.
    #[test]
    fn skillup_chance_curve_orange_full_gray_zero_and_monotone() {
        // GRAY: at/above the ceiling → 0% (orange<gray real band).
        assert_eq!(skillup_chance_bp(80, 60, 80), 0); // exactly at gray
        assert_eq!(skillup_chance_bp(90, 60, 80), 0); // above gray
                                                      // ORANGE floor: at or below → always 100% (the required-level "always skills" floor).
        assert_eq!(skillup_chance_bp(60, 60, 120), 10_000); // exactly at orange
        assert_eq!(skillup_chance_bp(40, 60, 120), 10_000); // below orange
                                                            // SENTINEL (gray <= orange), incl. the default-band 1,0 → always 100% (the legacy byte-identical path).
        assert_eq!(skillup_chance_bp(50, 1, 0), 10_000);
        assert_eq!(skillup_chance_bp(50, 60, 60), 10_000); // gray == orange degenerate
        assert_eq!(skillup_chance_bp(50, 60, 30), 10_000); // gray < orange degenerate
                                                           // INTERPOLATION across [orange, gray): the midpoint is ~50%.
        assert_eq!(skillup_chance_bp(70, 60, 80), 5_000); // (80-70)/(80-60) = 1/2
        assert_eq!(skillup_chance_bp(65, 60, 80), 7_500); // (80-65)/20 = 3/4
                                                          // MONOTONE non-increasing across the band — never goes UP as skill rises.
        let mut prev = 10_001u32;
        for current in 0u32..=130 {
            let c = skillup_chance_bp(current, 60, 120);
            assert!(
                c <= prev,
                "chance must not rise: at {current} got {c} after {prev}"
            );
            prev = c;
        }
    }

    /// RANK/CAP RAISE for an EXISTING profession row: the ceiling only LIFTS when the offered cap
    /// is higher (Journeyman 150 over Apprentice 75); re-buying the same/lower tier is a no-op
    /// (`raised_cap` == the stored cap → no update). This is the pure decision `learn_profession`
    /// branches on for the present-row case. (The NEW-row insert path takes the offered cap
    /// verbatim — no raise math — so it isn't asserted here.)
    #[test]
    fn raised_cap_lifts_only_a_higher_tier() {
        // EXISTING row: Apprentice 75 → Journeyman 150 lifts the ceiling.
        assert_eq!(raised_cap(75, 150), 150);
        assert_eq!(raised_cap(150, 225), 225); // Journeyman → Expert
        assert_eq!(raised_cap(225, 300), 300); // Expert → Artisan
                                               // Re-buying the SAME tier is a no-op (75 stays 75) — the idempotent already-known path.
        assert_eq!(raised_cap(75, 75), 75);
        // A LOWER-tier "re-buy" never lowers the ceiling (max keeps the higher stored cap).
        assert_eq!(raised_cap(150, 75), 150);
    }

    /// The load-bearing baseline-safety equivalence: the NEW skill-derived diff
    /// (`skill_diff(cap(tl), cap(al))`) must equal the OLD level-derived diff
    /// (`(tl - al).saturating_sub * 5`) for EVERY level pair, since the refactor moved the
    /// saturating-subtract from before the ×5 to after it. Brute-forced over 0..=100².
    #[test]
    fn skill_diff_from_caps_equals_old_level_derived_form() {
        for al in 0u32..=100 {
            for tl in 0u32..=100 {
                let new = skill_diff(skill_cap_for_level(tl), skill_cap_for_level(al));
                let old = tl.saturating_sub(al) * SKILL_PER_LEVEL;
                assert_eq!(
                    new, old,
                    "diverged at attacker_level={al} target_level={tl}"
                );
            }
        }
    }

    /// Mage createinfo weapon proficiency lines: Daggers (173), Staves (136),
    /// 1H Swords (43). These are the three lines the vanilla `playercreateinfo_spell` table seeds
    /// for class 8. Verified against cmangos `playercreateinfo_spell race=1 class=8`.
    #[test]
    fn mage_weapon_proficiency_lines_are_dagger_staff_sword_1h() {
        let lines = class_weapon_skill_lines(8 /* Mage */);
        let mut set: std::collections::HashSet<u32> = lines.iter().copied().collect();
        assert!(
            set.remove(&skill_line::DAGGER),
            "Mage must have DAGGER (173)"
        );
        assert!(set.remove(&skill_line::STAFF), "Mage must have STAFF (136)");
        assert!(
            set.remove(&skill_line::SWORD_1H),
            "Mage must have SWORD_1H (43)"
        );
        assert!(
            set.is_empty(),
            "Mage has unexpected extra weapon proficiency lines: {:?}",
            set
        );
    }

    /// The `class_weapon_skill_lines` contract `ensure_player_skills` seeds from: every playable
    /// class returns at least one weapon proficiency line, and unknown/creature classes return an
    /// empty slice. (ensure_player_skills itself needs a ReducerContext and is NOT exercised here.)
    #[test]
    fn class_weapon_skill_lines_nonempty_for_playable_classes_empty_otherwise() {
        // Every playable human class must return at least one weapon proficiency line.
        for &class in &[1u8, 2, 3, 4, 5, 7, 8, 9, 11] {
            let lines = class_weapon_skill_lines(class);
            assert!(
                !lines.is_empty(),
                "class {class} returned zero weapon proficiency lines"
            );
        }
        // Unknown / creature class returns empty (guards the caller's loop from stale entries).
        assert!(class_weapon_skill_lines(0).is_empty());
        assert!(class_weapon_skill_lines(6).is_empty()); // no class 6 in vanilla
    }

    /// PINNED OUTPUTS — the observable contract this module's consumers see, as concrete basis
    /// points. These are the numbers the live realm rolls against; a change here is a rebalance,
    /// not a refactor, and must be argued as such. (Level 60 / cap 300 is the max-level line;
    /// level 12 / cap 60 is a mid-levelling one; cap 5 is the level-1 edge.)
    #[test]
    fn combat_skillup_chance_pinned_outputs() {
        // Max-level weapon line, far below cap → catch-up regime.
        assert_eq!(combat_skillup_chance_bp(100, 300, 60, 0, false), 7_725);
        // Same, with the intellect assist (weapon line, 750 int saturates the level-60 band at +1000bp).
        assert_eq!(combat_skillup_chance_bp(100, 300, 60, 750, true), 8_725);
        // Taper regime: the last few points before cap.
        assert_eq!(combat_skillup_chance_bp(299, 300, 60, 0, false), 4);
        assert_eq!(combat_skillup_chance_bp(294, 300, 60, 0, false), 171);
        // Exactly at the seam (headroom 7) → the shared floor 7/300.
        assert_eq!(combat_skillup_chance_bp(293, 300, 60, 0, false), 233);
        // Mid-level line: the low-level lift makes the same headroom train faster.
        assert_eq!(combat_skillup_chance_bp(20, 60, 12, 0, false), 6_982);
        // Level-1 edge (cap 5): every point is inside the taper.
        assert_eq!(combat_skillup_chance_bp(4, 5, 1, 0, false), 285);
    }
}
