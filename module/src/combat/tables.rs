//! Combat — the **ctx-free** melee math: the hit-table / damage / regen formulas and their
//! constants, with no `&ReducerContext` and no DB access. Pure functions only, so the whole region is
//! unit-testable in isolation (the tests live at the bottom of this file). The ctx/db layer that wires
//! these into live combat — `roll_swing`, `swing_range_ctx`, the `effective_*` aura family,
//! `kill_creature`, the reducers — lives in the parent `mod.rs` and re-exports this module
//! (`pub use tables::*`), so every `crate::combat::<sym>` path resolves regardless of which submodule
//! actually defines the symbol.

use crate::WorldEntity;

pub(crate) const MELEE_RANGE_SQ: f32 = 25.0; // (5 yd)²
                                             // Melee LEEWAY (vanilla): a MOVING attacker swinging a MOVING target gets the combat-reach term (+4/3 yd)
                                             // PLUS the famous +8/3yd running bonus — the full formula is MAX(5, reachA+reachB+4/3) + 8/3 when both are
                                             // running, i.e. 5 + 4/3 + 8/3 = 9yd. This matters because the creature's flee leg snaps its AUTHORITATIVE
                                             // position the full per-tick run distance (3.5yd @ RUN*0.5s) the instant the tick fires (the client only
                                             // animates the glide), so right after a flee tick a run-speed-parity chaser reads up to ~8.5yd away for
                                             // the brief window until it closes again — anything short of the full 9yd would miss that swing and read
                                             // the mob as uncatchable. The full 9yd leeway covers that gap for a chaser already pinned at the 5yd
                                             // boundary going into the tick (vanilla's exact formula value); it reduces, but doesn't eliminate, misses
                                             // for a chaser who's settled farther back. A standstill fight stays at 5yd.
pub(crate) const MELEE_RANGE_LEEWAY_SQ: f32 =
    (5.0 + 4.0 / 3.0 + 8.0 / 3.0) * (5.0 + 4.0 / 3.0 + 8.0 / 3.0); // (9 yd)² = 81
                                                                   // CREATURE-attacker leeway: the exact classic both-moving formula (5 + 8/3 ≈ 7.67 yd) WITHOUT the
                                                                   // +4/3 leg-quantization pad above — that pad compensates for a leg-quantized TARGET (a fleeing
                                                                   // creature's server position jumps a half-second leg at once), and a creature's target is a PLAYER
                                                                   // whose position streams continuously (no jump to cover). With the full 9 yd a chasing mob tagged a
                                                                   // fleeing player from visibly farther than classic (live report 2026-07-10; render-lead makes it
                                                                   // look even longer than the server range).
                                                                   // "Moving" = the unit's `last_move_ms` is within this window of now. Covers the 500ms creature flee/chase
                                                                   // tick + the player's movement-packet cadence with margin, so a genuine chase reads as both-running while a
                                                                   // standstill does not (no leeway leak into stationary combat).
pub(crate) const MELEE_RANGE_LEEWAY_CREATURE_SQ: f32 = (5.0 + 8.0 / 3.0) * (5.0 + 8.0 / 3.0); // (7.67 yd)²
pub(crate) const MELEE_LEEWAY_WINDOW_MS: u32 = 1200;
pub(crate) const RANGED_RANGE_SQ: f32 = 1225.0; // (35 yd)² — ranged auto-attack max reach; inside MELEE_RANGE_SQ is "too close" (shot hard-fails, 097)
pub(crate) const RANGED_INITIAL_SHOT_MS: u32 = 500; // Auto Shot wind-up before the FIRST shot (vanilla's 0.5s RANGED_ATTACK timer re-arm), vs firing instantly on activation (097)
                                                    // Projectile speeds from the real 1.12 Spell.dbc (speed column, calibrated on Fireball=24.0):
                                                    // spell 75 Auto Shot = 40 yd/s, spell 5019 wand Shoot = 20 yd/s. Drive the shot's damage-at-impact
                                                    // delay (dist/speed) so the hit lands with the client's arrow, not at the muzzle (097).
pub(crate) const AUTO_SHOT_PROJECTILE_SPEED: f32 = 40.0;
pub(crate) const WAND_PROJECTILE_SPEED: f32 = 20.0;
// The 1.12 MovementFlags bits that mean "actually translating" (vmangos movementFlagsMask minus the
// turn/pitch bits): forward|backward|strafe L/R|jumping|falling-far. Turning in place must NOT
// count — vanilla keeps Auto Shot firing while you spin, but DEFERS shots while you move (097).
pub(crate) const MOVE_MASK_MOVING: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x2000 | 0x4000;
// INVARIANT (097): must exceed RANGED_RANGE_SQ — with leash < ranged reach, an Auto Shot pull from the
// outer band armed the wolf's retaliation and the very next 100ms tick leash-evaded it, deleting the
// player's own engagement row before the 500ms first shot ever fired ("enters combat, leaves instantly,
// re-press dead"). 45 yd > the 35 yd max pull; the 40 yd HOME tether below still stops kiting.
pub(crate) const LEASH_RADIUS_SQ: f32 = 2025.0; // (45 yd)² — target beyond this → the creature evades
                                                // Home tether (work-item 046): a creature dragged farther than this from its OWN spawn point evades
                                                // even with the target glued to melee range (pass_chase keeps the target-distance leash from ever
                                                // opening, so without this a mob chased a kiting player across the whole zone into Goldshire).
pub(crate) const HOME_TETHER_RADIUS_SQ: f32 = 1600.0; // (40 yd)²
/// Default melee interval (ms) for an unarmed player — vanilla's unarmed `UNIT_FIELD_BASEATTACKTIME`.
/// Per-unit speed lives on `WorldEntity::base_attack_time_ms`; this is just the player default.
pub const DEFAULT_ATTACK_TIME_MS: u32 = 2000;

// Combat-depth rolls (real hit/crit/miss + variance). Damage is a [min, max] range; the swing rolls
// 0..100 for the outcome. A player's range is derived from the stat curve (attack power, below); a
// creature uses a flat low range (creatures carry no attributes — `strength == 0`).
pub(crate) const CREATURE_MELEE_MIN: u32 = 1; // creature swing 1..=3 (avg 2)
pub(crate) const CREATURE_MELEE_MAX: u32 = 3;
pub(crate) const CRIT_MULT: u32 = 2; // crits hit for 2x

// The vanilla melee **attack table** is a single 0..10000 roll (basis points: 100 bp = 1%) walked in
// order miss → dodge → parry → glancing → block → crit → crushing → hit, each outcome claiming its slice
// of the line. A
// high-avoid defender therefore also crowds out crit, exactly as in vanilla. The bands scale with the
// LEVEL DIFFERENCE: since weapon skill / defense aren't tracked yet, weapon skill = attacker.level*5
// and defense = target.level*5, so `skill_diff = (target.level - attacker.level) * 5` drives the
// vanilla hit/dodge/glancing curves (yellow/orange mobs are harder to hit, and tougher mobs glance).
const BASE_MISS_BP: u32 = 500; // 5% miss vs an equal/lower-level target
const MAX_MISS_BP: u32 = 6000; // cap miss at 60% — symmetry with the dodge/glancing caps + keeps the
                               // table from being swamped by miss alone at absurd level deltas
const PARRY_BP: u32 = 500; // 5% parry floor (defender vs an equal/lower-level attacker)
const PARRY_BP_PER_SKILL: u32 = 10; // +0.1%/skill point the defender's defense exceeds the attacker's
                                    // weapon skill — mirrors the dodge/miss skill term (defense = lvl*5)
const MAX_PARRY_BP: u32 = 3000; // cap parry at 30% (mirrors the dodge cap; keeps the table under 100%)
                                // Block CHANCE (a shielded defender). Mirrors parry's scaling (defense-vs-weapon-skill, level-derived);
                                // the block AMOUNT is a flat per-shield `block_value` subtracted in `roll_swing`, NOT a percent here.
const BLOCK_BP: u32 = 500; // 5% block floor (a shielded defender vs an equal/lower-level attacker)
const BLOCK_BP_PER_SKILL: u32 = 10; // +0.1%/skill point the defender's defense exceeds the attacker's weapon skill
const MAX_BLOCK_BP: u32 = 3000; // cap block chance at 30% (mirrors the parry cap; keeps the table under 100%)
const CRIT_BP: u32 = 1000; // 10% crit (attacker)
const BASE_DODGE_BP: u32 = 500; // 5% dodge floor (creatures, zero-agility, equal level)
const DODGE_BP_PER_AGILITY: u32 = 5; // +0.05%/agi → ~+1% dodge per 20 agility (low-level vanilla-ish)
const MAX_DODGE_BP: u32 = 3000; // cap dodge at 30% so the table never overflows 100%
#[cfg(test)]
const SKILL_BP_PER_LEVEL: u32 = 5; // a level of skill difference = 5 skill points; tests derive skill_diff
                                   // from levels via this (production gets skill_diff from crate::skill)
const GLANCING_BP_PER_LEVEL: u32 = 1000; // +10% glancing per level the target is above the attacker
const MAX_GLANCING_BP: u32 = 4000; // cap glancing at 40% (vanilla's rough ceiling vs +3 targets)
pub(crate) const GLANCING_DAMAGE_PCT: u32 = 70; // a glancing blow lands 70% damage

// Crushing blows — the CREATURE-side mirror of glancing. A creature whose level is `CRUSH_MIN_LEVEL_DIFF`+
// above its target lands occasional CRUSHING blows for `CRUSH_DAMAGE_PCT` damage (vanilla: a mob whose
// weapon skill exceeds your defense by 15+, i.e. ~3 levels, can crush for 150%). Only NPC→player swings
// crush (players never crush, and PvP doesn't), so the band is armed only on that path; a same/lower-level
// creature has `crushing_chance_bp == 0`, so the table is unchanged (baseline-safe). The damage twin of
// glancing's reduced hit — here an AMPLIFIED hit.
const CRUSH_MIN_LEVEL_DIFF: u32 = 3; // a creature must be 3+ levels above to crush (the vanilla threshold)
const CRUSH_BP_PER_LEVEL: u32 = 1000; // +10% crushing per level above the threshold floor
const MAX_CRUSH_BP: u32 = 5000; // cap crushing at 50% (a deep level gap is brutal but never certain)
pub(crate) const CRUSH_DAMAGE_PCT: u32 = 150; // a crushing blow lands 150% damage

// Player melee damage from the stat curve (combat-depth). Unarmed base damage + a contribution from
// attack power, the vanilla way: bonus = AP / 14 per second of weapon speed. With no weapons yet
// every player is unarmed (2.0s `base_attack_time_ms`); when items land, weapon speed/damage just
// feed the same formula. AP uses the Strength-based-class formula (`level*3 + Str*2 - 20`) — exact
// for Warrior/Paladin (the only fully set-up class); Agility-based classes (Rogue/Hunter) get a
// class-specific AP curve in a later pass.
const UNARMED_BASE_MIN: u32 = 1; // unarmed base swing before the AP bonus
const UNARMED_BASE_MAX: u32 = 3;
const AP_PER_DPS: u32 = 14; // 14 attack power == +1 damage per second of weapon speed

// `game_combat_event.hit_info` discriminant → mapped to the `HitInfo`/VictimState SMSG fields by the
// gateway. Dodge/parry are zero-damage avoidance outcomes (VictimState in vanilla, not HitInfo flags).
pub(crate) const HIT_NORMAL: u8 = 0;
pub(crate) const HIT_CRIT: u8 = 1;
pub(crate) const HIT_MISS: u8 = 2;
pub(crate) const HIT_DODGE: u8 = 3;
pub(crate) const HIT_PARRY: u8 = 4;
pub(crate) const HIT_GLANCING: u8 = 5;
pub(crate) const HIT_CRUSHING: u8 = 6; // a landed amplified hit (150%); the gateway renders it as a normal hit
                                       // (graceful `_` default) until the Crushing HitInfo wire flag is mapped
pub(crate) const HIT_BLOCK: u8 = 7; // a LANDED hit a shield reduced by a flat block_value (partial mitigation,
                                    // NOT avoidance). Rendered as an AffectsVictim hit + VictimState BLOCK(4)
                                    // + blocked_amount (gtker's HitInfo has no Block variant). This DISCRIMINANT
                                    // value (7) is independent of the WALK position (after glancing, before crit).
                                    // NB: the value 7 must match the gateway's hit_info match arm in
                                    // gateway/src/codec/combat.rs (the `7 => (HitInfo::AffectsVictim, BLOCK)` case).

// Section note (deliberately `//`, not `///`: it documents the block below, not the next item).
// The attack-table SKILL DIFFERENCE (defense skill − weapon skill, in skill points, clamped ≥0) is
// supplied by the CALLER — `crate::skill::skill_diff_ctx` derives it from the real tracked weapon/defense
// skill, falling back to `level*5` when untracked (so an all-untracked fight reproduces the flat
// level-derived `(target.level − attacker.level)*5` formula exactly — baseline-safe). The four
// skill-based bands below (miss/dodge/parry/block) take it as a parameter; glancing/crushing stay
// raw-level-based.

/// Miss chance in basis points (vanilla hit table): 5% at `skill_diff` 0, rising with the skill
/// difference — `+0.1%`/point up to +10, then `+0.4%`/point beyond (the classic 5%→7%→9% curve vs
/// +1/+2/+3 skill tiers). Pure — unit-tested.
pub fn miss_chance_bp(skill_diff: u32) -> u32 {
    let raw = if skill_diff > 10 {
        700 + (skill_diff - 10) * 40
    } else {
        BASE_MISS_BP + skill_diff * 10
    };
    raw.min(MAX_MISS_BP)
}

/// Defender dodge chance in basis points: a `BASE_DODGE_BP` floor, plus an agility term, plus the
/// skill-difference term (a higher-defense target dodges more), capped at `MAX_DODGE_BP`. Pure. A
/// same-skill creature (agility 0) dodges at the floor.
pub fn dodge_chance_bp(agility: u32, skill_diff: u32) -> u32 {
    (BASE_DODGE_BP + agility * DODGE_BP_PER_AGILITY + skill_diff * 10).min(MAX_DODGE_BP)
}

/// Defender parry chance in basis points: a `PARRY_BP` floor plus the skill-difference term — a
/// higher-defense defender parries more — capped at `MAX_PARRY_BP`. Unlike dodge, parry does NOT scale
/// with agility; it tracks defense-vs-weapon-skill only (vanilla). Pure — unit-tested. `skill_diff` 0 →
/// the flat 5% floor (baseline-safe).
pub fn parry_chance_bp(skill_diff: u32) -> u32 {
    (PARRY_BP + skill_diff * PARRY_BP_PER_SKILL).min(MAX_PARRY_BP)
}

/// Defender block CHANCE in basis points (NOT the blocked amount): a `BLOCK_BP` floor plus the
/// skill-difference term, capped at `MAX_BLOCK_BP`. Mirrors parry in scaling, but block differs in
/// OUTCOME — it lands a hit whose damage a flat shield `block_value` was subtracted from, not the
/// zero-damage avoidance of dodge/parry. Pure — unit-tested. The CALLER (`roll_swing`) gates this to a
/// defender that actually has a shield equipped; an unshielded unit's band is 0 (baseline-safe).
pub fn block_chance_bp(skill_diff: u32) -> u32 {
    (BLOCK_BP + skill_diff * BLOCK_BP_PER_SKILL).min(MAX_BLOCK_BP)
}

/// Glancing chance in basis points: 0 against an equal/lower-level target, else `+10%` per level the
/// target is above the attacker, capped at `MAX_GLANCING_BP`. Glancing blows can't crit and land
/// reduced damage — the leveling-combat "tougher mob shrugs part of it off". Pure — unit-tested.
pub fn glancing_chance_bp(attacker_level: u32, target_level: u32) -> u32 {
    (target_level.saturating_sub(attacker_level) * GLANCING_BP_PER_LEVEL).min(MAX_GLANCING_BP)
}

/// Crushing chance in basis points: 0 unless the ATTACKER is `CRUSH_MIN_LEVEL_DIFF` (3) or more levels
/// ABOVE the target, then `+10%` per level past the threshold floor, capped at `MAX_CRUSH_BP`. The
/// creature-side mirror of glancing (which keys on the target being above the attacker) — a deep
/// level gap lets a mob shrug the player around for amplified hits. Pure — unit-tested. The CALLER gates
/// this to NPC→player swings (players never crush); an equal/lower-level attacker gets 0 here regardless.
pub fn crushing_chance_bp(attacker_level: u32, target_level: u32) -> u32 {
    let diff = attacker_level.saturating_sub(target_level);
    if diff < CRUSH_MIN_LEVEL_DIFF {
        return 0;
    }
    ((diff - CRUSH_MIN_LEVEL_DIFF + 1) * CRUSH_BP_PER_LEVEL).min(MAX_CRUSH_BP)
}

/// Melee attack power from the Strength-based-class formula: `level*3 + Str*2 - 20`, clamped at 0
/// (a tiny-stat unit has no negative AP). Exact for Warrior/Paladin; the general str-class curve.
pub fn melee_attack_power(strength: u32, level: u32) -> u32 {
    (level * 3 + strength * 2).saturating_sub(20)
}

// Vanilla class ids (the subset whose melee AP curve diverges from the Str-class default).
const CLASS_HUNTER: u8 = 3;
const CLASS_ROGUE: u8 = 4;

/// Class-specific melee attack power. Agility-based classes (Rogue/Hunter) use the
/// vanilla `level*2 + Str + Agi - 20` curve so AGILITY feeds the swing — without this a leveling Rogue
/// gained nothing from their primary stat. Every other class (Warrior/Paladin + the default) keeps the
/// Str-class `melee_attack_power` curve, so their AP is byte-identical to before (baseline-safe). Clamped
/// ≥0. Known limitation: pure casters technically use `Str-10` in vanilla, but that's an
/// unrequested balance change — left on the Str-class default until a caster-melee pass needs it.
pub fn melee_attack_power_for(class: u8, strength: u32, agility: u32, level: u32) -> u32 {
    match class {
        CLASS_ROGUE | CLASS_HUNTER => (level * 2 + strength + agility).saturating_sub(20),
        _ => melee_attack_power(strength, level),
    }
}

/// Ranged attack power from the Agility-based (Hunter-style) curve: `level*2 + Agi - 10`, clamped at 0.
/// An approximation of the vanilla ranged-AP formula — close enough at low level; the only consumer is
/// Auto Shot (spell 75, bows/guns). Wands (Shoot, 5019) use NO ranged AP (flat weapon damage). Folded into
/// the ranged weapon's `[min,max]` by `weapon_swing_range_ap` so it scales with the weapon's speed. Pure.
pub fn ranged_attack_power(agility: u32, level: u32) -> u32 {
    (level * 2 + agility).saturating_sub(10)
}

/// The unarmed `[min, max]` swing range from an ALREADY-COMPUTED attack power: unarmed base plus the
/// `AP / 14`-per-second bonus (`ap * attack_time_ms / 14000`). The AP-explicit core so a caller can
/// fold in extra AP (e.g. a Battle Shout aura) BEFORE the bonus is derived — the multiply is on the
/// summed AP, the vanilla way (the buff scales with weapon speed too). Pure — unit-tested.
pub fn player_swing_range_ap(ap: u32, attack_time_ms: u32) -> (u32, u32) {
    let bonus = ap * attack_time_ms / (AP_PER_DPS * 1000);
    (UNARMED_BASE_MIN + bonus, UNARMED_BASE_MAX + bonus)
}

/// The weapon `[min, max]` swing range from an ALREADY-COMPUTED attack power: the weapon's tooltip
/// damage plus the `AP * delay_ms / 14000` bonus. The AP-explicit weapon core (mirrors
/// `player_swing_range_ap`) so an aura's extra AP folds into the summed AP before the per-swing
/// bonus is scaled by the weapon's speed. Pure — unit-tested.
pub fn weapon_swing_range_ap(ap: u32, dmg_min: u32, dmg_max: u32, delay_ms: u32) -> (u32, u32) {
    let bonus = ap * delay_ms / (AP_PER_DPS * 1000);
    (dmg_min + bonus, dmg_max + bonus)
}

/// The vanilla dual-wield OFF-HAND penalty: an off-hand swing's `[min, max]` (already the AP-scaled
/// off-hand weapon range from `weapon_swing_range_ap`) lands at 50% of its unpenalized damage, floored
/// at 1 so a very low-roll off-hander never swings for 0. Pure — unit-tested. `resolve_swing`'s off-hand
/// roll applies this to the off-hand's own AP-scaled range BEFORE the shared
/// attack-table roll (`roll_swing_with_range`), so crit/glancing/armor still multiply off the already-
/// halved base — the same shape as `weapon_swing_range_ap`'s main-hand range feeding `roll_swing`.
pub fn apply_offhand_penalty(min: u32, max: u32) -> (u32, u32) {
    ((min / 2).max(1), (max / 2).max(1))
}

/// Fold a base attribute together with its aura and gear bonuses into the EFFECTIVE value: the three
/// terms summed and clamped ≥0 so a debuff exceeding base never underflows the `u32`. The shared core of
/// `effective_strength`/`effective_agility`/`effective_armor` (each still computes its own base/aura/gear
/// from the unit, which are semantically distinct) so the "base + aura + gear, floored at 0" shape lives
/// in ONE place. Pure — unit-tested.
pub(crate) fn effective_stat(base: i32, aura: i32, gear: i32) -> u32 {
    (base + aura + gear).max(0) as u32
}

/// Apply a crit-rating bonus to the flat `CRIT_BP` base: `CRIT_BP + bonus`, clamped to `[0, 10000]`
/// so the attack table's crit band never goes negative or overflows the 0..10000 line. The AP-explicit
/// twin (mirrors `player_swing_range_ap`): the pure core so `effective_crit_bp` folds in the aura sum
/// and the unit tests cover the math without a context. `bonus == 0` → exactly `CRIT_BP`. Pure. [entity]
pub fn crit_bp_with_bonus(bonus: i32) -> u32 {
    (CRIT_BP as i32 + bonus).clamp(0, 10_000) as u32
}

/// Agility-derived melee crit chance in basis points: `agi * 100 / (level + 4)` — crit
/// rises with the attacker's Agility and is SUPPRESSED by level (the per-agi value shrinks as the
/// attacker levels, the vanilla "more Agility per 1% crit at higher level" shape). Folded into the crit
/// band ALONGSIDE the flat `CRIT_BP` base + the aura/gear bonus (so this is an ADD, not a replace — a
/// zero-Agility unit, e.g. every creature, contributes 0 here and keeps the flat base, baseline-safe).
/// Low-level approximation, NOT a byte-exact vanilla curve — `100`/`+4` are tuning knobs. Pure.
pub fn agility_crit_bp(agility: u32, level: u32) -> u32 {
    agility * 100 / (level + 4)
}

/// Apply a hit-rating bonus to a level-derived miss base: `miss - bonus`, clamped ≥0 (hit rating
/// REDUCES the chance to miss, floored at 0 rather than underflowing). The pure core so
/// `effective_miss_bp` folds in the aura sum and the unit tests cover the math without a context.
/// `bonus == 0` → exactly `miss`. Pure. [entity]
pub fn miss_bp_with_bonus(miss: u32, bonus: i32) -> u32 {
    (miss as i32 - bonus).max(0) as u32
}

/// Swing interval (ms) after a SIGNED melee haste/slow PERCENT: `base * 100 / (100 + pct)`. Positive =
/// faster (shorter interval, a haste); negative = slower (longer, an attack-speed slow like Thunder Clap).
/// The denominator is floored at 1 so a ≥100% slow can't divide-by-zero — it just becomes a very slow
/// swing. Pure — unit-tested. `pct == 0` → exactly `base_ms` (baseline-safe).
pub fn swing_time_with_haste(base_ms: u32, haste_pct: i32) -> u32 {
    let denom = (100 + haste_pct).max(1) as u32;
    base_ms.saturating_mul(100) / denom
}

/// Movement speed (yd/s) after a SIGNED move-speed PERCENT: `base * (100 + pct) / 100`. Positive =
/// faster; negative = slower (a snare like Hamstring). Floored at 10% of base so a heavy snare slows but
/// never freezes a unit dead (a zero-length step breaks the movement passes' no-op-leg skip and would
/// look frozen). Pure — unit-tested. `pct == 0` → exactly `base` (baseline-safe). [pure]
pub fn move_speed_with_pct(base: f32, pct: i32) -> f32 {
    base * (100 + pct).max(10) as f32 / 100.0
}

/// Per-1000 melee-damage multiplier for a creature's classification `rank` (the swing twin of
/// `creatures::rank_hp_multiplier_permille`). cmangos `creature_template.rank`: `0`=normal, `1`=elite,
/// `2`=rare-elite, `3`=boss, `4`=rare. An elite/rare/boss hits harder as well as having more HP, but
/// the damage bump is GENTLER than the HP bump (vanilla elites are tanky more than they are bursty):
/// normal 1.0×, elite 1.4×, rare-elite 1.8×, boss 2.5×, rare 1.5×. PERMILLE (×1000) keeps the scaling
/// exact integer math (`dmg * permille / 1000`); rank 0 (`1000`) is the identity so every current
/// rank-0 creature swing stays BYTE-IDENTICAL. Unknown rank → normal (never amplify). Pure — unit-tested.
pub fn rank_damage_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000, // normal — identity (baseline)
        1 => 1400, // elite
        2 => 1800, // rare-elite
        3 => 2500, // boss
        4 => 1500, // rare
        _ => 1000, // unknown rank → treat as normal
    }
}

/// Scale a creature swing `[min, max]` range by its `rank` (integer permille math, saturating). Shared
/// by `swing_range_ctx` and its unit test so the live scaling and the asserted numbers never drift.
/// Rank 0 → unchanged (`(CREATURE_MELEE_MIN, CREATURE_MELEE_MAX)` verbatim). Pure.
pub fn scale_swing_for_rank(min: u32, max: u32, rank: u8) -> (u32, u32) {
    let permille = rank_damage_multiplier_permille(rank);
    let scale = |d: u32| ((d as u64 * permille as u64) / 1000) as u32;
    (scale(min), scale(max))
}

/// Physical damage reduction from the target's armor, as a whole-number percent (vanilla formula):
/// `armor / (armor + 400 + 85*attacker_level)`, capped at 75%. Pure — unit-tested. A 0-armor target
/// (every creature today) takes full damage; a player's agility-derived armor shaves a few percent
/// off incoming creature swings. The single mitigation source shared by `roll_swing` (applies it to
/// the rolled hit) and `swing_profile`/`debug_compute_swing` (surfaces it) — so they never drift.
pub fn armor_mitigation_pct(armor: u32, attacker_level: u32) -> u32 {
    if armor == 0 {
        return 0;
    }
    let denom = armor + 400 + 85 * attacker_level;
    (armor * 100 / denom).min(75)
}

/// Magic-school resistance reduction as a whole-number percent (vanilla average-resist): `75 ×
/// resistance / (caster_level × 5)`, capped at 75%. Resistance beyond the per-level cap (`caster_level ×
/// 5`) doesn't help — that's the 75% ceiling. `resistance == 0` (no resist aura — every unit today) or a
/// 0-level caster → 0%, so a magic hit is byte-identical to before (baseline-safe). The MAGIC twin of
/// `armor_mitigation_pct` (physical) — both live here as the canonical mitigation formulas; the
/// spell-damage path folds this in (`spell::apply_resistance`). Pure — unit-tested.
///
/// PROVENANCE: the linear `75 × resist / (level×5)` average-resist with a 75% cap is the widely-used
/// 1.12 community approximation (the real engine rolls partial-resist bands whose *average* this models)
/// and is empirically live-verified here (a +6 fire ward vs an L2 caster mitigates Fireball 20→11 = 45%).
/// It is NOT cross-checked byte-for-byte against retail; a band-rolled refinement is a later option.
pub fn resist_mitigation_pct(resistance: u32, caster_level: u32) -> u32 {
    if resistance == 0 || caster_level == 0 {
        return 0;
    }
    let cap = caster_level * 5;
    let r = resistance.min(cap); // resistance past the cap is wasted (the 75% ceiling)
    (75 * r / cap).min(75)
}

/// The deterministic profile of one swing, for the combat-readback debug readout: the base `[min,max]`
/// range, the armor `mitigation_pct`, and the resulting mitigated normal-hit `[final_min,final_max]`.
/// Pure. `roll_swing` rolls within the base range then applies the SAME `armor_mitigation_pct`, so a
/// `debug_compute_swing` readout reflects exactly what live combat does (no drift). A landed hit is
/// floored at 1 damage.
pub struct SwingProfile {
    pub base_min: u32,
    pub base_max: u32,
    pub mitigation_pct: u32,
    pub final_min: u32,
    pub final_max: u32,
}

pub fn swing_profile(
    base_min: u32,
    base_max: u32,
    target_armor: u32,
    attacker_level: u32,
) -> SwingProfile {
    let mitigation_pct = armor_mitigation_pct(target_armor, attacker_level);
    let mit = |d: u32| (d * (100 - mitigation_pct) / 100).max(1);
    SwingProfile {
        base_min,
        base_max,
        mitigation_pct,
        final_min: mit(base_min),
        final_max: mit(base_max),
    }
}

/// The attack-table band widths in basis points, in roll order — the full composition of one swing's
/// 0..10000 outcome line. Built per swing from the attacker/target levels + the defender's agility.
pub(crate) struct AttackTable {
    pub(crate) miss: u32,
    pub(crate) dodge: u32,
    pub(crate) parry: u32,
    pub(crate) glancing: u32,
    pub(crate) block: u32, // a shielded defender — walks after glancing, before crit (vanilla order)
    pub(crate) crit: u32,
    pub(crate) crushing: u32,
}

/// Classify one attack-table roll (0..10000) against `t`'s bands: walks the vanilla single-roll table
/// miss → dodge → parry → glancing → block → crit → crushing → hit and returns the `hit_info`
/// discriminant. Block sits AFTER glancing and BEFORE crit (the vanilla single-roll order) and is 0
/// except for a shielded defender; crushing sits AFTER crit and is 0 except on the NPC→player path — so a
/// shieldless player→mob / PvP swing walks the identical bands it did before. Pure — the slice boundaries
/// are unit-tested.
pub(crate) fn attack_outcome(roll: u32, t: &AttackTable) -> u8 {
    let mut floor = t.miss;
    if roll < floor {
        return HIT_MISS;
    }
    floor += t.dodge;
    if roll < floor {
        return HIT_DODGE;
    }
    floor += t.parry;
    if roll < floor {
        return HIT_PARRY;
    }
    floor += t.glancing;
    if roll < floor {
        return HIT_GLANCING;
    }
    floor += t.block;
    if roll < floor {
        return HIT_BLOCK;
    }
    floor += t.crit;
    if roll < floor {
        return HIT_CRIT;
    }
    floor += t.crushing;
    if roll < floor {
        return HIT_CRUSHING;
    }
    HIT_NORMAL
}

/// Does this unit use rage (warriors)? The power type is byte 3 of `unit_bytes_0`. Only rage-users
/// generate rage in combat; mana/energy classes and creatures (power byte 0) gain none here.
pub(crate) fn is_rage_user(e: &WorldEntity) -> bool {
    ((e.unit_bytes_0 >> 24) & 0xFF) as u8 == lyracore_shared::packing::power_type::RAGE
}

/// Simplified vanilla-ish rage from one damage event, in INTERNAL units (rage is stored ×10, so
/// `max_power` 1000 = 100 rage). The attacker (dealing) gains more than the defender (taking) —
/// generous enough to afford a 10-rage shout in ~2 swings, roughly matching low-level vanilla. (The
/// real curve is a level-scaled conversion; this is the slice approximation until a stats pass.)
pub(crate) fn rage_from_damage(damage: u32, dealt: bool) -> u32 {
    damage * if dealt { 8 } else { 3 }
}

/// Flat energy gained per regen tick (rogues / energy creatures), in AND out of combat. Vanilla
/// energy ticks 20 per 2s tick; our regen tick fires once per 4s sense tick, so the correct
/// equivalent is 20 × 2 = 40. Independent of spirit/level — energy regen is flat in vanilla.
const ENERGY_TICK: u32 = 40;

/// Internal rage units removed per regen tick when OUT of combat. Rage is stored ×10 internally
/// (100 displayed = 1000 internal). Vanilla drains approximately 1.25 displayed rage/sec; our
/// regen tick fires once per ~4s, so the target decrement is 1.25 × 4 × 10 = 50 internal units.
/// This is a tuning knob — the operator should eyeball the live drain speed and tweak this value.
const RAGE_DECAY_PER_TICK: u32 = 50;

/// Spirit-derived MANA gained per regen tick, OUT of combat. Vanilla mana regen is a spirit-scaled
/// per-tick amount (the classic low-level approximation is ~`Spirit/4 + 5` mana per 2s tick); our
/// tick is ~4s, so we use roughly double that and fold in a small level term. The `+ level/2 + 1`
/// floor means a mana unit with spirit 0 (a stat-less mana creature) still trickles a tiny amount — a
/// small "regens a little out of combat" trickle without overshooting, and never zero so a drained pool
/// always recovers. Pure — unit-tested.
fn mana_regen_per_tick(spirit: u32, level: u32) -> u32 {
    spirit / 2 + level / 2 + 1
}

/// Spirit-derived HEALTH gained per regen tick, OUT of combat. Vanilla out-of-combat health regen is
/// spirit-scaled (plus a level term); our tick is ~4s. We use `spirit + level` per tick. The `+1`
/// floor keeps a spirit-0 unit (every creature today — creatures carry spirit 0) recovering at
/// `level + 1` per tick, which for a low-level mob is a handful of HP — sane and close to "heals a
/// bit out of combat", while NEVER exceeding max. Pure — unit-tested.
fn health_regen_per_tick(spirit: u32, level: u32) -> u32 {
    spirit + level + 1
}

/// Per-tick passive POWER adjustment by power type, returning the new power value (the regen pass in
/// `tick_creatures` applies it). Mana regenerates toward max at a SPIRIT+level scaled rate
/// (`mana_regen_per_tick`) when NOT in the FSR window (`mana_paused == false` — the caller derives
/// this from `now_ms < entity.mana_regen_paused_until_ms`); energy regenerates toward max ALWAYS (in
/// or out of combat, like a rogue's) at a flat `ENERGY_TICK`, INDEPENDENT of spirit; rage DECAYS
/// toward 0 out of combat at `RAGE_DECAY_PER_TICK` internal units/tick (~1.25 displayed rage/sec,
/// close to vanilla) and holds during a fight; other power types are unchanged. Pure — unit-tested.
/// `max == 0` (creatures, which carry no power bar) yields no change. Power never exceeds `max` (the
/// `.min(max)` cap) and never underflows (the `saturating_sub` on rage).
///
/// FSR (Five-Second Rule): `mana_paused` is true for 5s after the caster last spent mana — regen is
/// suppressed during that window IN OR OUT OF COMBAT (vanilla parity). After the window, mana regens
/// regardless of combat state, matching vanilla's "wait 5s and regen mid-fight" mechanic.
///
/// BASELINE-SAFETY: with `spirit == 0` a mana unit still regens `level/2 + 1` per tick when not
/// paused; energy is unchanged by spirit; rage decays at the vanilla-approximate rate.
pub fn regen_power(
    power_type: u8,
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    in_combat: bool,
    mana_paused: bool,
) -> u32 {
    use lyracore_shared::packing::power_type as pt;
    if max == 0 {
        return current;
    }
    match power_type {
        pt::MANA => {
            if mana_paused {
                current
            } else {
                (current + mana_regen_per_tick(spirit, level)).min(max)
            }
        }
        pt::ENERGY => (current + ENERGY_TICK).min(max),
        pt::RAGE => {
            if in_combat {
                current
            } else {
                current.saturating_sub(RAGE_DECAY_PER_TICK)
            }
        }
        _ => current, // focus/happiness: unchanged
    }
}

/// Per-tick passive HEALTH regen OUT of combat, returning the new health value. Spirit+level scaled
/// (`health_regen_per_tick`); applied by the health-regen pass in `tick_creatures` to any unit below
/// max HP that is NOT engaged. Pure — unit-tested. Never exceeds `max` (the `.min(max)` cap);
/// `max == 0` yields no change. A spirit-0 unit (every creature today) still heals `level + 1` per
/// tick — a small, sane amount that keeps the prior "recovers a bit out of combat" behavior.
pub fn regen_health(current: u32, max: u32, spirit: u32, level: u32) -> u32 {
    if max == 0 {
        return current;
    }
    (current + health_regen_per_tick(spirit, level)).min(max)
}

/// Entity-aware POWER-regen wrapper: reads `spirit`/`level`/power bar off the row so the `tick_creatures`
/// caller passes ONE entity + the in-combat flag + the current tick's `now_ms` (the spirit/level wiring
/// and FSR derivation live here, not inline in the tick). Power type is byte 3 of `unit_bytes_0`.
/// FSR gate: mana regen is paused when `now_ms < e.mana_regen_paused_until_ms` (stamped on every
/// mana-spend in the cast path). Returns the new power value (the caller writes it back only if it changed).
pub fn regen_entity_power(e: &WorldEntity, in_combat: bool, now_ms: u64) -> u32 {
    let power_type = (e.unit_bytes_0 >> 24) as u8;
    let mana_paused = now_ms < e.mana_regen_paused_until_ms;
    regen_power(
        power_type,
        e.power,
        e.max_power,
        e.spirit,
        e.level,
        in_combat,
        mana_paused,
    )
}

/// Entity-aware HEALTH-regen wrapper: reads `spirit`/`level` off the row so the `tick_creatures` health
/// pass passes ONE entity. Returns the new health value (the caller writes it back). Callers gate on
/// `health < max_health && !in_combat` before calling, as before.
pub fn regen_entity_health(e: &WorldEntity) -> u32 {
    regen_health(e.health, e.max_health, e.spirit, e.level)
}

/// Per-tick PARTIAL health regen DURING COMBAT: `combat_regen_pct%` of the normal out-of-combat
/// regen tick amount. Used when the entity carries one or more `A_COMBAT_HEALTH_REGEN_PCT` auras —
/// the caller sums their `amount × stacks` and passes the total here. `combat_regen_pct == 0` →
/// no change (caller should skip). `combat_regen_pct >= 100` → the full tick (uncapped at 100 so
/// multiple auras can stack to 100). Never exceeds `max`; `max == 0` yields no change. Pure —
/// unit-tested. `spirit == 0` (every creature today) still heals `(level+1) * pct / 100` which
/// for a L5 troll is (6) * 10 / 100 = 0 (floored) — tiny but not 0 once `level` is meaningful.
pub fn regen_health_in_combat(
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    combat_regen_pct: u32,
) -> u32 {
    if max == 0 || combat_regen_pct == 0 {
        return current;
    }
    let full_tick = health_regen_per_tick(spirit, level);
    // Integer multiply then divide; the full_tick is already small (spirit+level+1 at low levels),
    // so no overflow risk. A fractional result floors toward 0 (a 10% tick at `full_tick == 6` → 0
    // rather than rounding up — that is expected: low-level troll regen is very slow in combat).
    let partial = full_tick * combat_regen_pct / 100;
    (current + partial).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test oracle for the level-derived skill difference the four skill-based bands take as input —
    /// `(target_level − attacker_level) * 5`, clamped ≥0. Lets these tests keep their readable "+N
    /// levels" form while feeding the `skill_diff: u32` parameter; production derives the same number
    /// from real tracked skill via `crate::skill::skill_diff_ctx` (the equivalence to this form is
    /// brute-forced in `skill::tests::skill_diff_from_caps_equals_old_level_derived_form`).
    fn sd(attacker_level: u32, target_level: u32) -> u32 {
        target_level.saturating_sub(attacker_level) * SKILL_BP_PER_LEVEL
    }

    #[test]
    fn melee_leeway_covers_the_post_flee_tick_gap_for_a_pinned_chaser() {
        // A fleeing creature's authoritative position snaps the full per-tick run distance
        // (RUN * MOVE_TICK_SECS) the instant its flee leg fires — the client only animates the glide, the
        // swing-range check reads the post-snap DB position immediately. Worst case: a run-speed-parity
        // chaser is sitting right at the base melee boundary (5yd) the instant the mob's leg fires, so the
        // gap jumps to 5 + one flee leg. The leeway must cover that gap for a chaser already pinned at the
        // boundary, or the swing whiffs and the mob reads as uncatchable; a chaser who's settled farther
        // back can still miss occasionally, but the leeway keeps the pinned case solid.
        use crate::creatures::MOVE_TICK_SECS;
        use lyracore_shared::constants::speeds::RUN;
        let worst_case_gap = 5.0 + RUN * MOVE_TICK_SECS; // 8.5 yd
        let leeway = MELEE_RANGE_LEEWAY_SQ.sqrt();
        assert!(
            leeway >= worst_case_gap,
            "leeway {leeway}yd must cover the worst-case post-flee-tick gap {worst_case_gap}yd"
        );
        // Pin the exact vanilla-formula value (5 base + 4/3 reach + 8/3 running = 9yd) so a future edit
        // can't silently shrink it back below the worst-case gap.
        assert!(
            (leeway - 9.0).abs() < 0.001,
            "leeway {leeway}yd expected exactly 9yd"
        );
    }

    #[test]
    fn attack_power_uses_the_strength_class_formula() {
        // L1 Human Warrior: Str 23 → 1*3 + 23*2 - 20 = 29.
        assert_eq!(melee_attack_power(23, 1), 29);
        // Scales with both level and strength.
        assert_eq!(melee_attack_power(40, 10), 30 + 80 - 20);
        assert!(melee_attack_power(40, 10) > melee_attack_power(23, 1));
        // Clamps at 0 — a near-zero-stat unit never gets negative AP.
        assert_eq!(melee_attack_power(0, 1), 0);
        assert_eq!(melee_attack_power(5, 1), 0); // 3 + 10 - 20 saturates to 0
    }

    #[test]
    fn agility_crit_rises_with_agility_and_is_suppressed_by_level() {
        // More Agility → strictly more crit, level held constant.
        assert!(agility_crit_bp(40, 5) > agility_crit_bp(20, 5));
        // Same Agility at a HIGHER level → less crit (per-agi suppression).
        assert!(agility_crit_bp(20, 10) < agility_crit_bp(20, 1));
        // Zero Agility (every creature) → 0, so the crit band stays the flat CRIT_BP base (baseline-safe).
        assert_eq!(agility_crit_bp(0, 5), 0);
        assert_eq!(crit_bp_with_bonus(agility_crit_bp(0, 5) as i32), CRIT_BP);
    }

    #[test]
    fn rogue_melee_ap_folds_in_agility_others_keep_the_str_curve() {
        // Rogue (class 4): level*2 + Str + Agi - 20 → Agility raises AP.
        let lo_agi = melee_attack_power_for(CLASS_ROGUE, 20, 20, 5); // 10 + 20 + 20 - 20 = 30
        let hi_agi = melee_attack_power_for(CLASS_ROGUE, 20, 40, 5); // 10 + 20 + 40 - 20 = 50
        assert_eq!(lo_agi, 30);
        assert!(
            hi_agi > lo_agi,
            "more Agility → strictly higher Rogue AP (Str/level held constant)"
        );
        // Hunter (class 3) shares the Agi curve.
        assert_eq!(melee_attack_power_for(CLASS_HUNTER, 20, 40, 5), 50);
        // Warrior (class 1) + any non-Agi class keep the Str-class curve byte-identical, Agility inert.
        assert_eq!(
            melee_attack_power_for(1, 23, 99, 1),
            melee_attack_power(23, 1)
        );
        assert_eq!(
            melee_attack_power_for(8, 23, 99, 1),
            melee_attack_power(23, 1)
        ); // Mage: no agi term
    }

    #[test]
    fn ranged_attack_power_scales_with_level_and_agility_and_floors_at_zero() {
        // level*2 + agi - 10, clamped at 0.
        assert_eq!(ranged_attack_power(20, 10), 30); // 10*2 + 20 - 10
        assert_eq!(ranged_attack_power(0, 5), 0); // 5*2 + 0 - 10 == 0, exactly at the floor
        assert_eq!(ranged_attack_power(0, 1), 0); // 1*2 + 0 - 10 saturates to 0 (would be negative)
                                                  // More Agility → strictly more ranged AP (level held constant).
        assert!(ranged_attack_power(40, 10) > ranged_attack_power(20, 10));
        // Higher level → strictly more ranged AP (agility held constant).
        assert!(ranged_attack_power(20, 20) > ranged_attack_power(20, 10));
    }

    #[test]
    fn effective_stat_sums_base_aura_gear_and_clamps_at_zero() {
        // base + aura + gear, the no-buff/no-gear case is exactly the base (baseline-safe).
        assert_eq!(effective_stat(23, 0, 0), 23);
        assert_eq!(effective_stat(23, 5, 7), 35);
        // A debuff (negative aura) that exceeds base+gear clamps to 0 — never underflows the u32.
        assert_eq!(effective_stat(10, -25, 3), 0);
        // A negative aura partially offset by gear stays non-negative.
        assert_eq!(effective_stat(10, -4, 2), 8);
    }

    #[test]
    fn player_swing_range_scales_with_stats_and_stays_sane() {
        // The unarmed range from the stat-derived AP (the AP core is fed `melee_attack_power(str, level)`,
        // exactly as `swing_range_ctx` does for an unarmed player).
        let unarmed =
            |str: u32, level: u32, t: u32| player_swing_range_ap(melee_attack_power(str, level), t);
        // L1 Str 23 unarmed (2.0s): AP 29 → bonus 29*2000/14000 = 4 → range (5, 7).
        assert_eq!(unarmed(23, 1, 2000), (5, 7));
        // The window width is constant (max - min == base width); only the floor shifts up.
        let (lo, hi) = unarmed(23, 1, 2000);
        assert_eq!(hi - lo, UNARMED_BASE_MAX - UNARMED_BASE_MIN);
        // A stronger character hits strictly harder (both ends of the range rise).
        let (slo, shi) = unarmed(60, 20, 2000);
        assert!(slo > lo && shi > hi);
        // A faster weapon (lower attack time) yields a smaller per-swing bonus (DPS preserved).
        let (flo, _) = unarmed(23, 1, 1000);
        assert!(flo <= lo);
        // Never zero damage from the floor (a real hit always lands ≥ unarmed base).
        assert!(unarmed(0, 1, 2000).0 >= UNARMED_BASE_MIN);
    }

    #[test]
    fn ap_explicit_cores_take_stat_derived_ap_and_scale_with_ap() {
        // The AP-explicit cores fed the stat-derived AP (`melee_attack_power(str, level)`) — exactly how
        // `swing_range_ctx` drives an unarmed/weapon swing. L1 Str 23 → AP 29; unarmed 2.0s → (5, 7).
        assert_eq!(
            player_swing_range_ap(melee_attack_power(23, 1), 2000),
            (5, 7)
        );
        // A 1–3 / 1900ms Worn Shortsword with that same AP: bonus 29*1900/14000 = 3 → (4, 6).
        assert_eq!(
            weapon_swing_range_ap(melee_attack_power(23, 1), 1, 3, 1900),
            (4, 6)
        );
        // A Battle Shout-style AP buff (extra AP on top of the base) hits strictly harder — both
        // ends of the range rise — exactly what an active self-buff aura grants the attacker.
        let base_ap = melee_attack_power(23, 1);
        let (lo, hi) = player_swing_range_ap(base_ap, 2000);
        let (blo, bhi) = player_swing_range_ap(base_ap + 30, 2000); // +30 AP (Battle Shout)
        assert!(blo > lo && bhi > hi);
        // Same for the weapon core: more AP → strictly higher floor at the same weapon/speed.
        let (wlo, _) = weapon_swing_range_ap(base_ap, 8, 12, 1900);
        let (wblo, _) = weapon_swing_range_ap(base_ap + 30, 8, 12, 1900);
        assert!(wblo > wlo);
        // Zero extra AP yields exactly the base range (baseline-safe).
        assert_eq!(player_swing_range_ap(base_ap, 2000), (lo, hi));
    }

    #[test]
    fn weapon_swing_range_uses_weapon_base_plus_ap_bonus() {
        // The weapon range from the stat-derived AP (the weapon AP core fed `melee_attack_power(str, level)`,
        // exactly as `swing_range_ctx` does for a player wielding a main-hand weapon).
        let weapon = |str: u32, level: u32, dmin: u32, dmax: u32, delay: u32| {
            weapon_swing_range_ap(melee_attack_power(str, level), dmin, dmax, delay)
        };
        // L1 Str 23 (AP 29) wielding a 1–3 / 1900ms Worn Shortsword: bonus = 29*1900/14000 = 3 →
        // range (4, 6). The weapon's own damage range replaces the unarmed (1,3) base.
        assert_eq!(weapon(23, 1, 1, 3, 1900), (4, 6));
        // A stronger weapon (8–12) hits clearly harder than the worn sword at the same stats/speed.
        let (wlo, whi) = weapon(23, 1, 8, 12, 1900);
        assert!(wlo > 4 && whi > 6);
        // The window width tracks the WEAPON's spread, not the unarmed one.
        assert_eq!(whi - wlo, 12 - 8);
        // A slower weapon yields a bigger per-swing AP bonus (DPS preserved), so its floor is higher
        // than the same weapon swung faster.
        let (slow_lo, _) = weapon(23, 1, 8, 12, 2600);
        let (fast_lo, _) = weapon(23, 1, 8, 12, 1300);
        assert!(slow_lo > fast_lo);
    }

    #[test]
    fn dodge_scales_with_agility_level_and_caps() {
        // Equal level (no skill diff): floor + agility term.
        assert_eq!(dodge_chance_bp(0, sd(10, 10)), BASE_DODGE_BP); // creature, no agility → floor
        assert_eq!(
            dodge_chance_bp(20, sd(10, 10)),
            BASE_DODGE_BP + 20 * DODGE_BP_PER_AGILITY
        ); // +1% at 20 agi
        assert!(dodge_chance_bp(100, sd(10, 10)) > dodge_chance_bp(20, sd(10, 10))); // monotonic in agility
                                                                                     // A higher-level target dodges more (skill-diff term); lower-level adds nothing.
        assert!(dodge_chance_bp(0, sd(10, 13)) > dodge_chance_bp(0, sd(10, 10)));
        assert_eq!(dodge_chance_bp(0, sd(10, 5)), BASE_DODGE_BP);
        assert_eq!(dodge_chance_bp(1_000_000, sd(10, 10)), MAX_DODGE_BP); // capped, never overflows the table
    }

    #[test]
    fn parry_scales_with_defense_skill_not_agility_and_caps() {
        // Equal/lower-level attacker → the flat 5% floor (baseline-safe).
        assert_eq!(parry_chance_bp(sd(10, 10)), PARRY_BP);
        assert_eq!(parry_chance_bp(sd(10, 5)), PARRY_BP); // lower-level defender still floors at 5% (skill_diff=0)
                                                          // A higher-level defender's greater defense skill parries more: +1 level = 5 skill pts = +0.5%.
        assert_eq!(
            parry_chance_bp(sd(10, 11)),
            PARRY_BP + 5 * PARRY_BP_PER_SKILL
        ); // +1 → 5.5%
        assert!(parry_chance_bp(sd(10, 13)) > parry_chance_bp(sd(10, 11))); // monotonic in level gap
        assert_eq!(parry_chance_bp(sd(1, 1_000_000)), MAX_PARRY_BP); // capped, never overflows the table
    }

    #[test]
    fn swing_time_haste_shortens_interval_and_is_baseline_safe_at_zero() {
        assert_eq!(swing_time_with_haste(2000, 0), 2000); // no haste → base time (baseline-safe)
        assert_eq!(swing_time_with_haste(2000, 50), 1333); // +50% → 2000*100/150 (swings faster)
        assert_eq!(swing_time_with_haste(2000, 100), 1000); // +100% → half the interval
        assert!(swing_time_with_haste(2600, 30) < 2600); // any positive haste shortens the interval
                                                         // A NEGATIVE percent is an attack-speed SLOW (Thunder Clap): LENGTHENS the interval.
        assert_eq!(swing_time_with_haste(2000, -10), 2222); // −10% → 2000*100/90 (swings slower)
        assert!(swing_time_with_haste(2000, -50) > 2000); // any slow lengthens the interval
                                                          // A ≥100% slow can't divide-by-zero — the denominator floors at 1 (a very slow, bounded swing).
        assert_eq!(swing_time_with_haste(2000, -100), 200_000);
        assert_eq!(swing_time_with_haste(2000, -150), 200_000); // clamped, not negative/overflow
    }

    #[test]
    fn move_speed_snare_slows_and_is_baseline_safe_at_zero() {
        assert_eq!(move_speed_with_pct(7.0, 0), 7.0); // no aura → base (baseline-safe)
        assert!((move_speed_with_pct(7.0, -40) - 4.2).abs() < 1e-4); // −40% snare → 4.2 yd/s
        assert!(move_speed_with_pct(7.0, 50) > 7.0); // a speed buff quickens
                                                     // A heavy snare floors at 10% of base (never freezes dead → no zero-length step).
        assert!((move_speed_with_pct(7.0, -95) - 0.7).abs() < 1e-4); // floored at 10%
        assert!((move_speed_with_pct(7.0, -200) - 0.7).abs() < 1e-4); // clamped, not 0/negative
    }

    #[test]
    fn rank_damage_multiplier_scales_swing_range_and_rank0_is_identity() {
        // The six documented permille multipliers, in `rank` order (unknown ranks fall through to normal).
        assert_eq!(rank_damage_multiplier_permille(0), 1000); // normal — identity
        assert_eq!(rank_damage_multiplier_permille(1), 1400); // elite
        assert_eq!(rank_damage_multiplier_permille(2), 1800); // rare-elite
        assert_eq!(rank_damage_multiplier_permille(3), 2500); // boss
        assert_eq!(rank_damage_multiplier_permille(4), 1500); // rare
        assert_eq!(rank_damage_multiplier_permille(7), 1000); // unknown rank → normal, never amplify

        // Rank 0 is the IDENTITY for the swing range — every rank-0 creature (all current spawns) swings
        // for exactly its base range.
        assert_eq!(
            scale_swing_for_rank(CREATURE_MELEE_MIN, CREATURE_MELEE_MAX, 0),
            (CREATURE_MELEE_MIN, CREATURE_MELEE_MAX)
        );
        // Elite (×1.4): 1..3 → (1, 4) (1*1400/1000 = 1, 3*1400/1000 = 4).
        assert_eq!(scale_swing_for_rank(1, 3, 1), (1, 4));
        // Boss (×2.5): 10..20 → (25, 50).
        assert_eq!(scale_swing_for_rank(10, 20, 3), (25, 50));
    }

    #[test]
    fn resist_mitigation_scales_and_caps_at_75() {
        // No resistance (every unit today) → 0% → a magic hit is byte-identical (baseline-safe).
        assert_eq!(resist_mitigation_pct(0, 60), 0);
        assert_eq!(resist_mitigation_pct(0, 2), 0);
        // A 0-level caster → 0% (guards the divide-by-zero).
        assert_eq!(resist_mitigation_pct(100, 0), 0);
        // At the per-level cap (caster_level*5) → the full 75% ceiling.
        assert_eq!(resist_mitigation_pct(300, 60), 75); // cap 300 → 75%
        assert_eq!(resist_mitigation_pct(10, 2), 75); // cap 10 at L2 → 75%
                                                      // Resistance PAST the cap is wasted — still 75%, never more (never overflows the band).
        assert_eq!(resist_mitigation_pct(10_000, 60), 75);
        // Partial: half the cap → half of 75% (37%, integer-floored). L2 cap 10, resist 5 → 37%.
        assert_eq!(resist_mitigation_pct(5, 2), 37); // 75*5/10 = 37 (floored)
        assert_eq!(resist_mitigation_pct(6, 2), 45); // 75*6/10 = 45 (the live-verified value)
                                                     // Monotonic up to the cap.
        assert!(resist_mitigation_pct(150, 60) > resist_mitigation_pct(75, 60));
    }

    #[test]
    fn miss_chance_follows_the_level_hit_curve() {
        assert_eq!(miss_chance_bp(sd(10, 10)), BASE_MISS_BP); // equal level → 5%
        assert_eq!(miss_chance_bp(sd(10, 5)), BASE_MISS_BP); // lower-level target → still base
        assert_eq!(miss_chance_bp(sd(10, 12)), 600); // +2 (skill diff 10) → 6%
        assert_eq!(miss_chance_bp(sd(10, 13)), 900); // +3 (skill diff 15) → 7% + 5*0.4% = 9%
        assert!(miss_chance_bp(sd(10, 14)) > miss_chance_bp(sd(10, 13))); // keeps rising past +3
        assert_eq!(miss_chance_bp(sd(1, 60)), MAX_MISS_BP); // capped at an absurd level delta (no overflow)
    }

    #[test]
    fn crit_and_hit_ratings_fold_into_the_attack_table_bands() {
        // No-aura path is byte-identical: zero bonus → exactly the flat base bands.
        assert_eq!(crit_bp_with_bonus(0), CRIT_BP); // crit band unchanged with no A_MOD_COMBAT(CRIT)
        assert_eq!(
            miss_bp_with_bonus(miss_chance_bp(sd(10, 10)), 0),
            miss_chance_bp(sd(10, 10))
        ); // miss unchanged

        // A positive crit bonus ADDS to the crit band (the "Combat Insight" +1000 = +10% crit).
        assert_eq!(crit_bp_with_bonus(1000), CRIT_BP + 1000);
        assert!(crit_bp_with_bonus(1000) > crit_bp_with_bonus(0)); // monotonic in the bonus

        // A hit bonus SUBTRACTS from the miss band (hit rating reduces the chance to miss).
        let base_miss = miss_chance_bp(sd(10, 12)); // 600 vs a +2 target
        assert_eq!(miss_bp_with_bonus(base_miss, 500), base_miss - 500); // +500 hit → -5% miss
        assert!(miss_bp_with_bonus(base_miss, 500) < base_miss); // monotonic down in the bonus

        // The miss band FLOORS at 0 — a hit bonus larger than the base miss never underflows the u32.
        assert_eq!(
            miss_bp_with_bonus(BASE_MISS_BP, BASE_MISS_BP as i32 + 1000),
            0
        );
        assert_eq!(miss_bp_with_bonus(0, 100), 0);

        // The crit band CAPS at 10000 (never overflows the 0..10000 attack-table line) and floors at 0
        // for a (hypothetical) crit DEBUFF whose magnitude exceeds the base.
        assert_eq!(crit_bp_with_bonus(100_000), 10_000);
        assert_eq!(crit_bp_with_bonus(-(CRIT_BP as i32) - 500), 0);
    }

    #[test]
    fn glancing_only_vs_higher_level_and_caps() {
        assert_eq!(glancing_chance_bp(10, 10), 0); // equal level → no glancing
        assert_eq!(glancing_chance_bp(10, 5), 0); // lower-level target → none
        assert_eq!(glancing_chance_bp(10, 11), GLANCING_BP_PER_LEVEL); // +1 → 10%
        assert_eq!(glancing_chance_bp(10, 13), 3 * GLANCING_BP_PER_LEVEL); // +3 → 30%
        assert_eq!(glancing_chance_bp(10, 60), MAX_GLANCING_BP); // capped at 40%
    }

    #[test]
    fn attack_table_walks_miss_dodge_parry_glancing_crit_hit_in_order() {
        // +3 target: miss 9%, agi-20 dodge incl. skill term, parry 5%, glancing 30%, crit 10%.
        let t = AttackTable {
            miss: miss_chance_bp(sd(10, 13)),
            dodge: dodge_chance_bp(20, sd(10, 13)),
            parry: PARRY_BP,
            glancing: glancing_chance_bp(10, 13),
            block: 0, // an unshielded defender has no block band
            crit: CRIT_BP,
            crushing: 0, // a player→higher-mob swing never crushes
        };
        let (m, d, p, g, c) = (t.miss, t.dodge, t.parry, t.glancing, t.crit);
        // Boundaries are half-open [floor, next): the last bp of each band still belongs to it.
        assert_eq!(attack_outcome(0, &t), HIT_MISS);
        assert_eq!(attack_outcome(m - 1, &t), HIT_MISS);
        assert_eq!(attack_outcome(m, &t), HIT_DODGE);
        assert_eq!(attack_outcome(m + d - 1, &t), HIT_DODGE);
        assert_eq!(attack_outcome(m + d, &t), HIT_PARRY);
        assert_eq!(attack_outcome(m + d + p, &t), HIT_GLANCING);
        assert_eq!(attack_outcome(m + d + p + g - 1, &t), HIT_GLANCING);
        assert_eq!(attack_outcome(m + d + p + g, &t), HIT_CRIT);
        assert_eq!(attack_outcome(m + d + p + g + c, &t), HIT_NORMAL);
        assert_eq!(attack_outcome(9_999, &t), HIT_NORMAL); // the remaining line is a normal hit

        // An equal-level fight has no glancing band: parry abuts crit directly.
        let eq = AttackTable {
            miss: miss_chance_bp(sd(10, 10)),
            dodge: dodge_chance_bp(0, sd(10, 10)),
            parry: PARRY_BP,
            glancing: glancing_chance_bp(10, 10),
            block: 0,
            crit: CRIT_BP,
            crushing: 0,
        };
        assert_eq!(eq.glancing, 0);
        assert_eq!(attack_outcome(eq.miss + eq.dodge + eq.parry, &eq), HIT_CRIT);
    }

    #[test]
    fn crushing_blows_arm_only_for_higher_level_creatures_and_amplify() {
        // 0 below the +3 threshold (so player→mob, PvP, and small-gap fights are byte-identical).
        assert_eq!(crushing_chance_bp(10, 10), 0); // equal level
        assert_eq!(crushing_chance_bp(10, 13), 0); // attacker LOWER (the glancing case) → no crush
        assert_eq!(crushing_chance_bp(12, 10), 0); // +2 → still below the 3-level threshold
                                                   // At/above the threshold: +10% per level past the floor.
        assert_eq!(crushing_chance_bp(13, 10), CRUSH_BP_PER_LEVEL); // +3 → 10%
        assert_eq!(crushing_chance_bp(14, 10), 2 * CRUSH_BP_PER_LEVEL); // +4 → 20%
        assert!(crushing_chance_bp(15, 10) > crushing_chance_bp(14, 10)); // monotonic in the gap
        assert_eq!(crushing_chance_bp(60, 1), MAX_CRUSH_BP); // capped at 50%, never overflows the table

        // The crushing band sits AFTER crit in the walk, and a crushing outcome amplifies to 150%.
        let t = AttackTable {
            miss: 0,
            dodge: 0,
            parry: 0,
            glancing: 0,
            block: 0,
            crit: 1000,
            crushing: 2000,
        };
        assert_eq!(attack_outcome(999, &t), HIT_CRIT); // last bp of the crit band
        assert_eq!(attack_outcome(1000, &t), HIT_CRUSHING); // first bp past crit → crushing
        assert_eq!(attack_outcome(2999, &t), HIT_CRUSHING); // last bp of the crushing band
        assert_eq!(attack_outcome(3000, &t), HIT_NORMAL); // remainder is a normal hit
    }

    #[test]
    fn block_chance_mirrors_parry_structure_and_caps() {
        // Flat floor vs an equal/lower-level attacker (the shielded baseline).
        assert_eq!(block_chance_bp(sd(10, 10)), BLOCK_BP);
        assert_eq!(block_chance_bp(sd(13, 10)), BLOCK_BP); // attacker HIGHER → still the floor (skill_diff 0)
                                                           // +1 target level = 5 skill points → +0.5% (mirrors parry exactly).
        assert_eq!(
            block_chance_bp(sd(10, 11)),
            BLOCK_BP + 5 * BLOCK_BP_PER_SKILL
        );
        assert_eq!(
            block_chance_bp(sd(10, 13)),
            BLOCK_BP + 15 * BLOCK_BP_PER_SKILL
        ); // +3 target
        assert!(block_chance_bp(sd(10, 14)) > block_chance_bp(sd(10, 13))); // monotonic in the level gap
        assert_eq!(block_chance_bp(sd(1, 1_000_000)), MAX_BLOCK_BP); // capped, never overflows the table
                                                                     // Block scales identically to parry (both track defense-vs-weapon-skill, agility-independent).
        assert_eq!(block_chance_bp(sd(10, 13)), parry_chance_bp(sd(10, 13)));
    }

    #[test]
    fn attack_table_inserts_block_after_glancing_before_crit() {
        // A non-zero block band must sit exactly between glancing and crit in the walk.
        let t = AttackTable {
            miss: 900,
            dodge: 500,
            parry: 500,
            glancing: 3000,
            block: 600,
            crit: 1000,
            crushing: 0,
        };
        let pre = t.miss + t.dodge + t.parry + t.glancing; // start of the block band
        assert_eq!(attack_outcome(pre - 1, &t), HIT_GLANCING); // last bp of glancing
        assert_eq!(attack_outcome(pre, &t), HIT_BLOCK); // first bp of block
        assert_eq!(attack_outcome(pre + t.block - 1, &t), HIT_BLOCK); // last bp of block
        assert_eq!(attack_outcome(pre + t.block, &t), HIT_CRIT); // first bp past block → crit
        assert_eq!(attack_outcome(pre + t.block + t.crit, &t), HIT_NORMAL); // remainder is a hit
    }

    #[test]
    fn block_zero_band_is_baseline_identical() {
        // With block=0 the walk is byte-identical to a no-block table: glancing abuts crit directly.
        let t = AttackTable {
            miss: 900,
            dodge: 500,
            parry: 500,
            glancing: 3000,
            block: 0,
            crit: 1000,
            crushing: 0,
        };
        let after_glancing = t.miss + t.dodge + t.parry + t.glancing;
        assert_eq!(attack_outcome(after_glancing, &t), HIT_CRIT); // no zero-width block band fires
        assert_eq!(attack_outcome(after_glancing + t.crit, &t), HIT_NORMAL);
    }

    // Test-fixture builder: one argument per `WorldEntity` column the regen vectors vary.
    #[allow(clippy::too_many_arguments)]
    /// A hand-built `WorldEntity` for the pure-fn-adjacent wrapper tests below (`is_rage_user`,
    /// `regen_entity_power`, `regen_entity_health`) — every field except the ones the test cares about
    /// is a neutral zero/default, mirroring `build_creature_entity`'s field order.
    fn entity_for_regen(
        unit_bytes_0: u32,
        health: u32,
        max_health: u32,
        power: u32,
        max_power: u32,
        spirit: u32,
        level: u32,
        mana_regen_paused_until_ms: u64,
    ) -> WorldEntity {
        WorldEntity {
            guid: 1,
            owner_identity: spacetimedb::Identity::ZERO,
            account_id: 0,
            map_id: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            grid_x: 0,
            grid_y: 0,
            last_move_ms: 0,
            type_mask: 0,
            entry: 0,
            scale_x: 1.0,
            health,
            max_health,
            power,
            max_power,
            level,
            faction_template: 0,
            unit_bytes_0,
            display_id: 0,
            native_display_id: 0,
            unit_flags: 0,
            base_attack_time_ms: 2000,
            dynamic_flags: 0,
            dead: false,
            player_bytes: 0,
            player_bytes_2: 0,
            player_bytes_3: 0,
            player_flags: 0,
            xp: 0,
            next_level_xp: 0,
            target_guid: 0,
            money: 0,
            unit_bytes_1: 0,
            strength: 0,
            agility: 0,
            stamina: 0,
            intellect: 0,
            spirit,
            npc_flags: 0,
            armor: 0,
            leg_ends_ms: 0,
            wp_target: 0,
            movement_flags: 0,
            combat_until_ms: 0,
            pickpocketed: false,
            next_swing_spell: 0,
            overpower_until_ms: 0,
            revenge_until_ms: 0,
            stance: 0,
            owner_guid: 0,
            skinned: false,
            mana_regen_paused_until_ms,
            death_expire_micros: 0, // not exercised by these regen/rage tests
            instance_id: 0,
            run_speed_mult_bp: 10_000,
            godmode: false,
            resting: false,
        }
    }

    #[test]
    fn is_rage_user_reads_the_power_type_byte_from_unit_bytes_0() {
        use lyracore_shared::packing::{power_type, unit_bytes_0};
        let warrior = entity_for_regen(
            unit_bytes_0(1, 1, 0, power_type::RAGE),
            100,
            100,
            0,
            0,
            0,
            1,
            0,
        );
        let mage = entity_for_regen(
            unit_bytes_0(1, 8, 0, power_type::MANA),
            100,
            100,
            100,
            100,
            0,
            1,
            0,
        );
        let rogue = entity_for_regen(
            unit_bytes_0(1, 4, 0, power_type::ENERGY),
            100,
            100,
            100,
            100,
            0,
            1,
            0,
        );
        // A creature's placeholder bytes (`0x0101` from `build_creature_entity`) pack power type 0 (MANA).
        let creature = entity_for_regen(0x0101, 42, 42, 0, 0, 0, 1, 0);
        assert!(is_rage_user(&warrior));
        assert!(!is_rage_user(&mage));
        assert!(!is_rage_user(&rogue));
        assert!(!is_rage_user(&creature));
    }

    #[test]
    fn rage_from_damage_is_asymmetric_dealt_vs_taken() {
        // The attacker (dealing) gains more rage than the defender (taking) for the same damage.
        assert_eq!(rage_from_damage(10, true), 80); // dealt: 10 * 8
        assert_eq!(rage_from_damage(10, false), 30); // taken: 10 * 3
        assert_eq!(rage_from_damage(0, true), 0);
        assert!(rage_from_damage(10, true) > rage_from_damage(10, false));
    }

    #[test]
    fn regen_entity_power_wrapper_reads_fields_and_straddles_the_fsr_window() {
        use lyracore_shared::packing::{power_type, unit_bytes_0};
        // A mana entity with an FSR window ending at ms 5000, spirit 40 / level 10 (26/tick when clear).
        let mage = entity_for_regen(
            unit_bytes_0(1, 8, 0, power_type::MANA),
            100,
            500,
            100,
            500,
            40,
            10,
            5_000,
        );
        // Strictly inside the window → paused, no regen.
        assert_eq!(regen_entity_power(&mage, false, 4_999), 100);
        // Straddling the boundary: `now_ms == paused_until_ms` is NOT `<`, so the window has just cleared.
        assert_eq!(regen_entity_power(&mage, false, 5_000), 100 + 26);
        // Past the window, mid-combat → still regens (the FSR gate, not combat state, controls mana).
        assert_eq!(regen_entity_power(&mage, true, 6_000), 100 + 26);

        // A rage (warrior) entity ignores the FSR field entirely — reads its own power-type byte.
        let warrior = entity_for_regen(
            unit_bytes_0(1, 1, 0, power_type::RAGE),
            1,
            1,
            1000,
            1000,
            0,
            10,
            0,
        );
        assert_eq!(regen_entity_power(&warrior, false, 0), 1000 - 50); // out of combat → decays
        assert_eq!(regen_entity_power(&warrior, true, 0), 1000); // in combat → holds
    }

    #[test]
    fn regen_entity_health_wrapper_reads_spirit_and_level_off_the_row() {
        let e = entity_for_regen(0, 100, 1000, 0, 0, 30, 10, 0);
        assert_eq!(regen_entity_health(&e), 100 + (30 + 10 + 1)); // spirit + level + 1 per tick
    }

    #[test]
    fn regen_power_default_branch_leaves_focus_and_happiness_unchanged() {
        use lyracore_shared::packing::power_type as pt;
        // FOCUS (hunter) and HAPPINESS (pet) aren't ticked by this pass — the catch-all `_` arm holds
        // the value steady regardless of combat state.
        assert_eq!(regen_power(pt::FOCUS, 50, 100, 40, 10, false, false), 50);
        assert_eq!(regen_power(pt::HAPPINESS, 50, 100, 40, 10, true, false), 50);
    }

    #[test]
    // The expectations are written in the SHAPE of the formula they pin (`0 + (level/2 + 1)` = base
    // plus the spirit-0 trickle). Folding the `0 +` away would make the vector unreadable against
    // `mana_regen_per_tick`, which is the only reason these numbers are checkable by eye.
    #[allow(clippy::identity_op)]
    fn power_regen_by_type_and_combat_state() {
        use lyracore_shared::packing::power_type as pt;
        // Helper aliases for readability: FSR window states.
        const FSR_ACTIVE: bool = true; // within 5s of last mana spend → no mana regen
        const FSR_CLEAR: bool = false; // 5s elapsed (or never cast) → regen proceeds
        const IN_COMBAT: bool = true;
        const OUT_OF_COMBAT: bool = false;

        // Mana: SPIRIT+level scaled toward max when FSR window is clear — in OR out of combat.
        // spirit 40, level 10 → 40/2 + 10/2 + 1 = 26 per tick.
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, OUT_OF_COMBAT, FSR_CLEAR),
            100 + 26
        );
        // FSR active (just cast): mana paused regardless of combat state.
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, OUT_OF_COMBAT, FSR_ACTIVE),
            100
        );
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, IN_COMBAT, FSR_ACTIVE),
            100
        );
        // FSR clear AND in combat: mana REGENS (vanilla 5-second rule — mid-fight regen after 5s).
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, IN_COMBAT, FSR_CLEAR),
            100 + 26
        );
        // Never overshoots max.
        assert_eq!(
            regen_power(pt::MANA, 490, 500, 40, 10, OUT_OF_COMBAT, FSR_CLEAR),
            500
        );
        // More spirit → faster mana regen (the headline invariant).
        let lo = regen_power(pt::MANA, 0, 5000, 20, 10, OUT_OF_COMBAT, FSR_CLEAR);
        let hi = regen_power(pt::MANA, 0, 5000, 200, 10, OUT_OF_COMBAT, FSR_CLEAR);
        assert!(hi > lo, "more spirit must regen more mana ({hi} !> {lo})");
        // Spirit 0 (a stat-less mana creature) still trickles a small, non-zero amount when FSR clear.
        assert_eq!(
            regen_power(pt::MANA, 0, 500, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            0 + (10 / 2 + 1)
        ); // level/2 + 1 = 6
        assert!(regen_power(pt::MANA, 0, 500, 0, 1, OUT_OF_COMBAT, FSR_CLEAR) > 0);
        // Energy: flat ENERGY_TICK (40) in AND out of combat (rogue), INDEPENDENT of spirit, capped.
        assert_eq!(
            regen_power(pt::ENERGY, 20, 100, 0, 10, IN_COMBAT, FSR_CLEAR),
            60
        );
        assert_eq!(
            regen_power(pt::ENERGY, 20, 100, 999, 10, OUT_OF_COMBAT, FSR_CLEAR),
            60
        ); // spirit doesn't affect energy
        assert_eq!(
            regen_power(pt::ENERGY, 95, 100, 0, 10, IN_COMBAT, FSR_CLEAR),
            100
        ); // capped at max
           // Rage: decays at RAGE_DECAY_PER_TICK (50 internal = 5 displayed per ~4s tick ≈ 1.25 rage/sec)
           // out of combat, holds in combat, never underflows (saturating_sub).
        assert_eq!(
            regen_power(pt::RAGE, 1000, 1000, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            1000 - 50
        ); // -RAGE_DECAY_PER_TICK
        assert_eq!(
            regen_power(pt::RAGE, 1000, 1000, 0, 10, IN_COMBAT, FSR_CLEAR),
            1000
        ); // held during a fight
        assert_eq!(
            regen_power(pt::RAGE, 30, 1000, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            0
        ); // saturating, no underflow
           // Partial step: 80 rage internal (8 displayed) decays by 50 → 30, not 0 (gradual, not a cliff).
        assert_eq!(
            regen_power(pt::RAGE, 80, 1000, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            30
        );
        // A creature with no power bar (max 0) never changes, regardless of spirit/level.
        assert_eq!(
            regen_power(pt::MANA, 0, 0, 40, 10, OUT_OF_COMBAT, FSR_CLEAR),
            0
        );
    }

    #[test]
    fn fsr_gate_pins_mana_regen_window() {
        use lyracore_shared::packing::power_type as pt;
        // spirit 40, level 10 → 26 per tick.
        let spirit = 40u32;
        let level = 10u32;
        // FSR active (now_ms < paused_until_ms): no regen.
        assert_eq!(
            regen_power(pt::MANA, 100, 500, spirit, level, false, true),
            100
        );
        // FSR expired (now_ms >= paused_until_ms): regen proceeds regardless of combat flag.
        assert_eq!(
            regen_power(pt::MANA, 100, 500, spirit, level, true, false),
            100 + 26
        );
        assert_eq!(
            regen_power(pt::MANA, 100, 500, spirit, level, false, false),
            100 + 26
        );
    }

    #[test]
    // `0 + 10` is the formula's shape (spirit contribution + `level + 1`), kept verbatim so the
    // vector reads against `health_regen_per_tick`. See the sibling power-regen test.
    #[allow(clippy::identity_op)]
    fn health_regen_is_spirit_and_level_scaled_and_clamped() {
        // SPIRIT+level scaled: spirit 30, level 10 → 30 + 10 + 1 = 41 per tick.
        assert_eq!(regen_health(100, 1000, 30, 10), 100 + 41);
        // More spirit → faster health regen.
        let lo = regen_health(0, 100_000, 10, 10);
        let hi = regen_health(0, 100_000, 100, 10);
        assert!(hi > lo, "more spirit must regen more health ({hi} !> {lo})");
        // Capped at max (no overshoot).
        assert_eq!(regen_health(995, 1000, 30, 10), 1000);
        // Spirit 0 (every creature today) still heals a small, non-zero amount: level + 1.
        assert_eq!(regen_health(0, 1000, 0, 9), 0 + 10);
        assert!(regen_health(0, 1000, 0, 1) > 0);
        // A unit with no HP bar (max 0) never changes.
        assert_eq!(regen_health(0, 0, 30, 10), 0);
    }

    #[test]
    fn combat_regen_is_partial_fraction_of_normal_tick() {
        // Zero pct → no change (aura absent → today's behaviour).
        assert_eq!(regen_health_in_combat(100, 1000, 30, 10, 0), 100);
        // 100% → identical to the full out-of-combat tick.
        assert_eq!(
            regen_health_in_combat(100, 1000, 30, 10, 100),
            regen_health(100, 1000, 30, 10)
        );
        // Troll Regeneration typical case: spirit 0, level 5 → full_tick = 5+0+1 = 6; 10% → 6*10/100 = 0
        // (integer floor — a tiny troll gets zero ticks at low level, expected at L5 with spirit=0).
        assert_eq!(regen_health_in_combat(100, 1000, 0, 5, 10), 100);
        // Spirit 50, level 20: full_tick = 50+20+1 = 71; 10% → 71*10/100 = 7.
        assert_eq!(regen_health_in_combat(100, 1000, 50, 20, 10), 100 + 7);
        // Two stacked auras (20% each = 40%) — caller sums and passes total pct.
        assert_eq!(
            regen_health_in_combat(100, 1000, 50, 20, 40),
            100 + 71 * 40 / 100
        );
        // Always monotonic: more pct → more or equal regen.
        assert!(
            regen_health_in_combat(0, 10000, 50, 20, 50)
                >= regen_health_in_combat(0, 10000, 50, 20, 10)
        );
        // Capped at max (no overshoot even for pct=100 with health close to max).
        assert_eq!(regen_health_in_combat(995, 1000, 50, 20, 100), 1000);
        // max=0 (no HP bar) → unchanged.
        assert_eq!(regen_health_in_combat(0, 0, 30, 10, 10), 0);
    }

    #[test]
    fn armor_mitigation_follows_the_vanilla_curve() {
        // 0 armor → no reduction (every creature target today).
        assert_eq!(armor_mitigation_pct(0, 1), 0);
        // 42 armor (a naked L2 warrior, agi 21*2) vs an L1 attacker: 42/(42+400+85) = 7.97% → 7%.
        assert_eq!(armor_mitigation_pct(42, 1), 7);
        // Higher attacker level → the same armor mitigates LESS (denominator grows).
        assert!(armor_mitigation_pct(42, 60) < armor_mitigation_pct(42, 1));
        // Enormous armor is capped at 75% (the vanilla physical ceiling).
        assert_eq!(armor_mitigation_pct(1_000_000, 1), 75);
    }

    #[test]
    // `.max(1)` is provably a no-op for THIS input (3*93/100 = 2), and that is the point: the
    // expectation restates the production floor so the vector still reads correctly if the base
    // range is ever retuned to a value the floor does bite on.
    #[allow(clippy::unnecessary_min_or_max)]
    fn swing_profile_applies_mitigation_to_the_range() {
        // 0-armor target: profile is the base range untouched (mitigation 0).
        let p = swing_profile(5, 7, 0, 1);
        assert_eq!((p.base_min, p.base_max, p.mitigation_pct), (5, 7, 0));
        assert_eq!((p.final_min, p.final_max), (5, 7));
        // 42-armor target (7%): a creature's 1–3 base shrinks but floors at 1 (a landed hit always lands).
        let q = swing_profile(1, 3, 42, 1);
        assert_eq!(q.mitigation_pct, 7);
        assert_eq!(q.final_min, 1); // 1*93/100 = 0 → floored to 1
        assert_eq!(q.final_max, (3 * 93 / 100).max(1)); // 2
    }

    mod offhand_tests {
        use super::apply_offhand_penalty;

        #[test]
        fn apply_offhand_penalty_halves_the_range() {
            // A plain even range halves cleanly.
            assert_eq!(apply_offhand_penalty(10, 20), (5, 10));
            // Vanilla's 50% dual-wield penalty on a weapon-scaled range.
            assert_eq!(apply_offhand_penalty(2, 4), (1, 2));
        }

        #[test]
        fn apply_offhand_penalty_floors_at_one() {
            // A very low min/max never halves to 0 — an off-hand swing always lands at least 1.
            assert_eq!(apply_offhand_penalty(1, 1), (1, 1));
            assert_eq!(apply_offhand_penalty(0, 1), (1, 1));
            assert_eq!(apply_offhand_penalty(0, 0), (1, 1));
        }
    }
}
