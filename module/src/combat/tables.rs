use crate::WorldEntity;

pub(crate) const MELEE_RANGE_SQ: f32 = 25.0;
pub(crate) const MELEE_RANGE_LEEWAY_SQ: f32 =
    (5.0 + 4.0 / 3.0 + 8.0 / 3.0) * (5.0 + 4.0 / 3.0 + 8.0 / 3.0);
pub(crate) const MELEE_RANGE_LEEWAY_CREATURE_SQ: f32 = (5.0 + 8.0 / 3.0) * (5.0 + 8.0 / 3.0);
pub(crate) const MELEE_LEEWAY_WINDOW_MS: u32 = 1200;
pub(crate) const RANGED_RANGE_SQ: f32 = 1225.0;
pub(crate) const RANGED_INITIAL_SHOT_MS: u32 = 500;
// Projectile speeds are in yd/s from Spell.dbc.
pub(crate) const AUTO_SHOT_PROJECTILE_SPEED: f32 = 40.0;
pub(crate) const WAND_PROJECTILE_SPEED: f32 = 20.0;
pub(crate) const fn is_translating(movement_flags: u32) -> bool {
    movement_flags & lyracore_shared::movement_flags::MOVING != 0
}
pub(crate) const PURSUIT_WINDOW_MS: u32 = 15_000;
pub(crate) const LEASH_RADIUS_SQ: f32 = 900.0;
pub(crate) const COMBAT_BACKSTOP_SQ: f32 = 8100.0;
pub const DEFAULT_ATTACK_TIME_MS: u32 = 2000;

pub(crate) const CREATURE_MELEE_MIN: u32 = 1;
pub(crate) const CREATURE_MELEE_MAX: u32 = 3;
pub(crate) const CRIT_MULT: u32 = 2;

const BASE_MISS_BP: u32 = 500;
const MAX_MISS_BP: u32 = 6000;
const PARRY_BP: u32 = 500;
const PARRY_BP_PER_SKILL: u32 = 10;
const MAX_PARRY_BP: u32 = 3000;
const BLOCK_BP: u32 = 500;
const BLOCK_BP_PER_SKILL: u32 = 10;
const MAX_BLOCK_BP: u32 = 3000;
const CRIT_BP: u32 = 1000;
const BASE_DODGE_BP: u32 = 500;
const DODGE_BP_PER_AGILITY: u32 = 5;
const MAX_DODGE_BP: u32 = 3000;
#[cfg(test)]
const SKILL_BP_PER_LEVEL: u32 = 5;
const GLANCING_BP_PER_LEVEL: u32 = 1000;
const MAX_GLANCING_BP: u32 = 4000;
pub(crate) const GLANCING_DAMAGE_PCT: u32 = 70;

const CRUSH_MIN_LEVEL_DIFF: u32 = 3;
const CRUSH_BP_PER_LEVEL: u32 = 1000;
const MAX_CRUSH_BP: u32 = 5000;
pub(crate) const CRUSH_DAMAGE_PCT: u32 = 150;

const UNARMED_BASE_MIN: u32 = 1;
const UNARMED_BASE_MAX: u32 = 3;
const AP_PER_DPS: u32 = 14;

pub(crate) const HIT_NORMAL: u8 = 0;
pub(crate) const HIT_CRIT: u8 = 1;
pub(crate) const HIT_MISS: u8 = 2;
pub(crate) const HIT_DODGE: u8 = 3;
pub(crate) const HIT_PARRY: u8 = 4;
pub(crate) const HIT_GLANCING: u8 = 5;
pub(crate) const HIT_CRUSHING: u8 = 6;
pub(crate) const HIT_BLOCK: u8 = 7;

pub(crate) fn miss_chance_bp(skill_diff: u32) -> u32 {
    let raw = if skill_diff > 10 {
        700 + (skill_diff - 10) * 40
    } else {
        BASE_MISS_BP + skill_diff * 10
    };
    raw.min(MAX_MISS_BP)
}

pub(crate) fn dodge_chance_bp(agility: u32, skill_diff: u32) -> u32 {
    (BASE_DODGE_BP + agility * DODGE_BP_PER_AGILITY + skill_diff * 10).min(MAX_DODGE_BP)
}

pub(crate) fn parry_chance_bp(skill_diff: u32) -> u32 {
    (PARRY_BP + skill_diff * PARRY_BP_PER_SKILL).min(MAX_PARRY_BP)
}

pub(crate) fn block_chance_bp(skill_diff: u32) -> u32 {
    (BLOCK_BP + skill_diff * BLOCK_BP_PER_SKILL).min(MAX_BLOCK_BP)
}

pub(crate) fn glancing_chance_bp(attacker_level: u32, target_level: u32) -> u32 {
    (target_level.saturating_sub(attacker_level) * GLANCING_BP_PER_LEVEL).min(MAX_GLANCING_BP)
}

pub(crate) fn crushing_chance_bp(attacker_level: u32, target_level: u32) -> u32 {
    let diff = attacker_level.saturating_sub(target_level);
    if diff < CRUSH_MIN_LEVEL_DIFF {
        return 0;
    }
    ((diff - CRUSH_MIN_LEVEL_DIFF + 1) * CRUSH_BP_PER_LEVEL).min(MAX_CRUSH_BP)
}

pub(crate) fn melee_attack_power(strength: u32, level: u32) -> u32 {
    (level * 3 + strength * 2).saturating_sub(20)
}

const CLASS_HUNTER: u8 = 3;
const CLASS_ROGUE: u8 = 4;

pub(crate) fn melee_attack_power_for(class: u8, strength: u32, agility: u32, level: u32) -> u32 {
    match class {
        CLASS_ROGUE | CLASS_HUNTER => (level * 2 + strength + agility).saturating_sub(20),
        _ => melee_attack_power(strength, level),
    }
}

pub(crate) fn ranged_attack_power(agility: u32, level: u32) -> u32 {
    (level * 2 + agility).saturating_sub(10)
}

pub(crate) fn ranged_sheet_values(
    agility: u32,
    level: u32,
    weapon: Option<(u32, u32, u32, u8)>,
) -> (u32, u32, u32) {
    use crate::items::weapon_subclass as ws;

    let Some((dmg_min, dmg_max, delay_ms, subclass)) = weapon else {
        return (0, 0, 0);
    };
    if matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW) {
        let attack_power = ranged_attack_power(agility, level);
        let (dmg_min, dmg_max) = weapon_swing_range_ap(attack_power, dmg_min, dmg_max, delay_ms);
        (attack_power, dmg_min, dmg_max)
    } else {
        (0, dmg_min, dmg_max)
    }
}

pub(crate) fn player_swing_range_ap(ap: u32, attack_time_ms: u32) -> (u32, u32) {
    let bonus = ap * attack_time_ms / (AP_PER_DPS * 1000);
    (UNARMED_BASE_MIN + bonus, UNARMED_BASE_MAX + bonus)
}

pub(crate) fn weapon_swing_range_ap(
    ap: u32,
    dmg_min: u32,
    dmg_max: u32,
    delay_ms: u32,
) -> (u32, u32) {
    let bonus = ap * delay_ms / (AP_PER_DPS * 1000);
    (dmg_min + bonus, dmg_max + bonus)
}

pub(crate) fn apply_offhand_penalty(min: u32, max: u32) -> (u32, u32) {
    ((min / 2).max(1), (max / 2).max(1))
}

pub(crate) fn effective_stat(base: i32, aura: i32, gear: i32) -> u32 {
    (i64::from(base) + i64::from(aura) + i64::from(gear)).clamp(0, i64::from(u32::MAX)) as u32
}

pub(crate) fn crit_bp_with_bonus(bonus: i32) -> u32 {
    (CRIT_BP as i32 + bonus).clamp(0, 10_000) as u32
}

pub(crate) fn agility_crit_bp(agility: u32, level: u32) -> u32 {
    agility * 100 / (level + 4)
}

pub(crate) fn miss_bp_with_bonus(miss: u32, bonus: i32) -> u32 {
    (miss as i32 - bonus).max(0) as u32
}

pub(crate) fn swing_time_with_haste(base_ms: u32, haste_pct: i32) -> u32 {
    let denom = (100 + haste_pct).max(1) as u32;
    base_ms.saturating_mul(100) / denom
}

#[cfg(test)]
use lyracore_shared::aura::move_speed_with_pct;

pub(crate) fn rank_damage_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000,
        1 => 1400,
        2 => 1800,
        3 => 2500,
        4 => 1500,
        _ => 1000,
    }
}

pub(crate) fn scale_swing_for_rank(min: u32, max: u32, rank: u8) -> (u32, u32) {
    let permille = rank_damage_multiplier_permille(rank);
    let scale = |d: u32| ((d as u64 * permille as u64) / 1000) as u32;
    (scale(min), scale(max))
}

pub(crate) fn armor_mitigation_pct(armor: u32, attacker_level: u32) -> u32 {
    if armor == 0 {
        return 0;
    }
    let denom = armor + 400 + 85 * attacker_level;
    (armor * 100 / denom).min(75)
}

pub(crate) fn resist_mitigation_pct(resistance: u32, caster_level: u32) -> u32 {
    if resistance == 0 || caster_level == 0 {
        return 0;
    }
    let cap = caster_level * 5;
    let r = resistance.min(cap);
    (75 * r / cap).min(75)
}

#[cfg(any(test, feature = "debug_reducers"))]
pub struct SwingProfile {
    pub base_min: u32,
    pub base_max: u32,
    pub mitigation_pct: u32,
    pub final_min: u32,
    pub final_max: u32,
}

#[cfg(any(test, feature = "debug_reducers"))]
pub(crate) fn swing_profile(
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

pub(crate) struct AttackTable {
    pub(crate) miss: u32,
    pub(crate) dodge: u32,
    pub(crate) parry: u32,
    pub(crate) glancing: u32,
    pub(crate) block: u32,
    pub(crate) crit: u32,
    pub(crate) crushing: u32,
}

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

pub(crate) fn is_rage_user(e: &WorldEntity) -> bool {
    ((e.unit_bytes_0 >> 24) & 0xFF) as u8 == lyracore_shared::packing::power_type::RAGE
}

// Rage uses tenths; 100 displayed rage is 1000 internal units.
pub(crate) fn rage_from_damage(damage: u32, dealt: bool) -> u32 {
    damage * if dealt { 8 } else { 3 }
}

// Vanilla credits 20 energy every 2 s, in and out of combat (cmangos `Player::Regenerate`).
const ENERGY_TICK_MS: u64 = 2_000;
const ENERGY_PER_TICK: u32 = 20;

const RAGE_DECAY_PER_TICK: u32 = 50;

fn mana_regen_per_tick(spirit: u32, level: u32) -> u32 {
    spirit / 2 + level / 2 + 1
}

fn health_regen_per_tick(spirit: u32, level: u32) -> u32 {
    spirit + level + 1
}

#[cfg(test)]
pub(crate) fn regen_power(
    power_type: u8,
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    in_combat: bool,
    mana_paused: bool,
) -> u32 {
    regen_power_with_flat(
        power_type,
        current,
        max,
        spirit,
        level,
        PowerRegenTick {
            in_combat,
            mana_paused,
            mana_flat_tick: 0,
            regen_rate: 1.0,
        },
    )
}

struct PowerRegenTick {
    in_combat: bool,
    mana_paused: bool,
    mana_flat_tick: i32,
    regen_rate: f64,
}

fn scale_regen(base: u32, regen_rate: f64) -> u32 {
    (f64::from(base) * regen_rate) as u32
}

fn regen_power_with_flat(
    power_type: u8,
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    tick: PowerRegenTick,
) -> u32 {
    use lyracore_shared::packing::power_type as pt;
    if max == 0 {
        return current;
    }
    match power_type {
        pt::MANA => apply_regen(
            current,
            max,
            if tick.mana_paused {
                0
            } else {
                scale_regen(mana_regen_per_tick(spirit, level), tick.regen_rate)
            },
            tick.mana_flat_tick,
        ),
        pt::RAGE => {
            if tick.in_combat {
                current
            } else {
                current.saturating_sub(scale_regen(RAGE_DECAY_PER_TICK, tick.regen_rate))
            }
        }
        _ => current,
    }
}

#[cfg(test)]
pub(crate) fn regen_health(current: u32, max: u32, spirit: u32, level: u32) -> u32 {
    regen_health_with_flat(current, max, spirit, level, 0)
}

fn regen_health_with_flat(
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    health_flat_tick: i32,
) -> u32 {
    if max == 0 {
        return current;
    }
    apply_regen(
        current,
        max,
        health_regen_per_tick(spirit, level),
        health_flat_tick,
    )
}

#[derive(Clone, Copy)]
pub(crate) struct RegenWindow {
    now_ms: u64,
    elapsed_ms: u64,
}

impl RegenWindow {
    pub(crate) fn new(now_ms: u64, elapsed_ms: u64) -> Self {
        Self { now_ms, elapsed_ms }
    }
}

fn per_five_in_window(per_five: i32, window: RegenWindow) -> i32 {
    let previous_ms = window.now_ms.saturating_sub(window.elapsed_ms);
    let accrued = |at_ms: u64| (i128::from(per_five) * i128::from(at_ms)).div_euclid(5_000);
    let points = accrued(window.now_ms) - accrued(previous_ms);
    points.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
}

fn apply_regen(current: u32, max: u32, base_tick: u32, flat_tick: i32) -> u32 {
    let tick =
        (i128::from(base_tick) + i128::from(flat_tick)).clamp(0, i128::from(u32::MAX)) as u32;
    current.saturating_add(tick).min(max)
}

pub(crate) fn regen_entity_power(
    e: &WorldEntity,
    in_combat: bool,
    window: RegenWindow,
    mana_per_five: i32,
    regen_rate: f64,
) -> u32 {
    let power_type = (e.unit_bytes_0 >> 24) as u8;
    let mana_paused = window.now_ms < e.mana_regen_paused_until_ms;
    regen_power_with_flat(
        power_type,
        e.power,
        e.max_power,
        e.spirit,
        e.level,
        PowerRegenTick {
            in_combat,
            mana_paused,
            mana_flat_tick: per_five_in_window(mana_per_five, window),
            regen_rate,
        },
    )
}

pub(crate) fn regen_entity_health(
    e: &WorldEntity,
    health_per_five: i32,
    window: RegenWindow,
) -> u32 {
    regen_health_with_flat(
        e.health,
        e.max_health,
        e.spirit,
        e.level,
        per_five_in_window(health_per_five, window),
    )
}

// Energy ticks fall on a fixed 2 s grid, so a late or early firing neither drops nor repeats one.
pub(crate) fn energy_ticks_between(credited_through_ms: u64, now_ms: u64) -> u64 {
    (now_ms / ENERGY_TICK_MS).saturating_sub(credited_through_ms / ENERGY_TICK_MS)
}

pub(crate) fn regen_energy(current: u32, max: u32, ticks: u64, regen_rate: f64) -> u32 {
    let gained = ticks.saturating_mul(u64::from(scale_regen(ENERGY_PER_TICK, regen_rate)));
    u64::from(current)
        .saturating_add(gained)
        .min(u64::from(max)) as u32
}

#[cfg(any(test, feature = "debug_reducers"))]
pub(crate) fn regen_health_in_combat(
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    combat_regen_pct: u32,
) -> u32 {
    if max == 0 || combat_regen_pct == 0 {
        return current;
    }
    let partial = (i128::from(health_regen_per_tick(spirit, level)) * i128::from(combat_regen_pct)
        / 100)
        .clamp(0, i128::from(u32::MAX)) as u32;
    apply_regen(current, max, partial, 0)
}

pub(crate) fn regen_entity_health_in_combat(
    e: &WorldEntity,
    combat_regen_pct: u32,
    health_per_five: i32,
    window: RegenWindow,
) -> u32 {
    let partial =
        (i128::from(health_regen_per_tick(e.spirit, e.level)) * i128::from(combat_regen_pct) / 100)
            .clamp(0, i128::from(u32::MAX)) as u32;
    apply_regen(
        e.health,
        e.max_health,
        partial,
        per_five_in_window(health_per_five, window),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sd(attacker_level: u32, target_level: u32) -> u32 {
        target_level.saturating_sub(attacker_level) * SKILL_BP_PER_LEVEL
    }

    #[test]
    fn melee_leeway_covers_the_post_flee_tick_gap_for_a_pinned_chaser() {
        use crate::creatures::MOVE_TICK_SECS;
        use lyracore_shared::constants::speeds::RUN;
        let worst_case_gap = 5.0 + RUN * MOVE_TICK_SECS;
        let leeway = MELEE_RANGE_LEEWAY_SQ.sqrt();
        assert!(
            leeway >= worst_case_gap,
            "leeway {leeway}yd must cover the worst-case post-flee-tick gap {worst_case_gap}yd"
        );
        assert!(
            (leeway - 9.0).abs() < 0.001,
            "leeway {leeway}yd expected exactly 9yd"
        );
    }

    #[test]
    fn attack_power_uses_the_strength_class_formula() {
        assert_eq!(melee_attack_power(23, 1), 29);
        assert_eq!(melee_attack_power(40, 10), 30 + 80 - 20);
        assert!(melee_attack_power(40, 10) > melee_attack_power(23, 1));
        assert_eq!(melee_attack_power(0, 1), 0);
        assert_eq!(melee_attack_power(5, 1), 0);
    }

    #[test]
    fn agility_crit_rises_with_agility_and_is_suppressed_by_level() {
        assert!(agility_crit_bp(40, 5) > agility_crit_bp(20, 5));
        assert!(agility_crit_bp(20, 10) < agility_crit_bp(20, 1));
        assert_eq!(agility_crit_bp(0, 5), 0);
        assert_eq!(crit_bp_with_bonus(agility_crit_bp(0, 5) as i32), CRIT_BP);
    }

    #[test]
    fn rogue_melee_ap_folds_in_agility_others_keep_the_str_curve() {
        let lo_agi = melee_attack_power_for(CLASS_ROGUE, 20, 20, 5);
        let hi_agi = melee_attack_power_for(CLASS_ROGUE, 20, 40, 5);
        assert_eq!(lo_agi, 30);
        assert!(
            hi_agi > lo_agi,
            "more Agility → strictly higher Rogue AP (Str/level held constant)"
        );
        assert_eq!(melee_attack_power_for(CLASS_HUNTER, 20, 40, 5), 50);
        assert_eq!(
            melee_attack_power_for(1, 23, 99, 1),
            melee_attack_power(23, 1)
        );
        assert_eq!(
            melee_attack_power_for(8, 23, 99, 1),
            melee_attack_power(23, 1)
        );
    }

    #[test]
    fn ranged_attack_power_scales_with_level_and_agility_and_floors_at_zero() {
        assert_eq!(ranged_attack_power(20, 10), 30);
        assert_eq!(ranged_attack_power(0, 5), 0);
        assert_eq!(ranged_attack_power(0, 1), 0);
        assert!(ranged_attack_power(40, 10) > ranged_attack_power(20, 10));
        assert!(ranged_attack_power(20, 20) > ranged_attack_power(20, 10));
    }

    #[test]
    fn ranged_sheet_launcher_uses_attack_power_and_weapon_speed() {
        let values = ranged_sheet_values(
            20,
            10,
            Some((8, 12, 2_800, crate::items::weapon_subclass::BOW)),
        );

        assert_eq!(values, (30, 14, 18));
    }

    #[test]
    fn ranged_sheet_wand_keeps_flat_weapon_damage() {
        let values = ranged_sheet_values(
            20,
            10,
            Some((8, 12, 2_800, crate::items::weapon_subclass::WAND)),
        );

        assert_eq!(values, (0, 8, 12));
    }

    #[test]
    fn effective_stat_sums_base_aura_gear_and_clamps_at_zero() {
        assert_eq!(effective_stat(23, 0, 0), 23);
        assert_eq!(effective_stat(23, 5, 7), 35);
        assert_eq!(effective_stat(10, -25, 3), 0);
        assert_eq!(effective_stat(10, -4, 2), 8);
    }

    #[test]
    fn player_swing_range_scales_with_stats_and_stays_sane() {
        let unarmed =
            |str: u32, level: u32, t: u32| player_swing_range_ap(melee_attack_power(str, level), t);
        assert_eq!(unarmed(23, 1, 2000), (5, 7));
        let (lo, hi) = unarmed(23, 1, 2000);
        assert_eq!(hi - lo, UNARMED_BASE_MAX - UNARMED_BASE_MIN);
        let (slo, shi) = unarmed(60, 20, 2000);
        assert!(slo > lo && shi > hi);
        let (flo, _) = unarmed(23, 1, 1000);
        assert!(flo <= lo);
        assert!(unarmed(0, 1, 2000).0 >= UNARMED_BASE_MIN);
    }

    #[test]
    fn ap_explicit_cores_take_stat_derived_ap_and_scale_with_ap() {
        assert_eq!(
            player_swing_range_ap(melee_attack_power(23, 1), 2000),
            (5, 7)
        );
        assert_eq!(
            weapon_swing_range_ap(melee_attack_power(23, 1), 1, 3, 1900),
            (4, 6)
        );
        let base_ap = melee_attack_power(23, 1);
        let (lo, hi) = player_swing_range_ap(base_ap, 2000);
        let (blo, bhi) = player_swing_range_ap(base_ap + 30, 2000);
        assert!(blo > lo && bhi > hi);
        let (wlo, _) = weapon_swing_range_ap(base_ap, 8, 12, 1900);
        let (wblo, _) = weapon_swing_range_ap(base_ap + 30, 8, 12, 1900);
        assert!(wblo > wlo);
    }

    #[test]
    fn weapon_swing_range_uses_weapon_base_plus_ap_bonus() {
        let weapon = |str: u32, level: u32, dmin: u32, dmax: u32, delay: u32| {
            weapon_swing_range_ap(melee_attack_power(str, level), dmin, dmax, delay)
        };
        assert_eq!(weapon(23, 1, 1, 3, 1900), (4, 6));
        let (wlo, whi) = weapon(23, 1, 8, 12, 1900);
        assert!(wlo > 4 && whi > 6);
        assert_eq!(whi - wlo, 12 - 8);
        let (slow_lo, _) = weapon(23, 1, 8, 12, 2600);
        let (fast_lo, _) = weapon(23, 1, 8, 12, 1300);
        assert!(slow_lo > fast_lo);
    }

    #[test]
    fn dodge_scales_with_agility_level_and_caps() {
        assert_eq!(dodge_chance_bp(0, sd(10, 10)), BASE_DODGE_BP);
        assert_eq!(
            dodge_chance_bp(20, sd(10, 10)),
            BASE_DODGE_BP + 20 * DODGE_BP_PER_AGILITY
        );
        assert!(dodge_chance_bp(100, sd(10, 10)) > dodge_chance_bp(20, sd(10, 10)));
        assert!(dodge_chance_bp(0, sd(10, 13)) > dodge_chance_bp(0, sd(10, 10)));
        assert_eq!(dodge_chance_bp(0, sd(10, 5)), BASE_DODGE_BP);
        assert_eq!(dodge_chance_bp(1_000_000, sd(10, 10)), MAX_DODGE_BP);
    }

    #[test]
    fn parry_scales_with_defense_skill_not_agility_and_caps() {
        assert_eq!(parry_chance_bp(sd(10, 10)), PARRY_BP);
        assert_eq!(parry_chance_bp(sd(10, 5)), PARRY_BP);
        assert_eq!(
            parry_chance_bp(sd(10, 11)),
            PARRY_BP + 5 * PARRY_BP_PER_SKILL
        );
        assert!(parry_chance_bp(sd(10, 13)) > parry_chance_bp(sd(10, 11)));
        assert_eq!(parry_chance_bp(sd(1, 1_000_000)), MAX_PARRY_BP);
    }

    #[test]
    fn swing_time_haste_shortens_interval_and_is_baseline_safe_at_zero() {
        assert_eq!(swing_time_with_haste(2000, 0), 2000);
        assert_eq!(swing_time_with_haste(2000, 50), 1333);
        assert_eq!(swing_time_with_haste(2000, 100), 1000);
        assert!(swing_time_with_haste(2600, 30) < 2600);
        assert_eq!(swing_time_with_haste(2000, -10), 2222);
        assert!(swing_time_with_haste(2000, -50) > 2000);
        assert_eq!(swing_time_with_haste(2000, -100), 200_000);
        assert_eq!(swing_time_with_haste(2000, -150), 200_000);
    }

    #[test]
    fn move_speed_snare_slows_and_is_baseline_safe_at_zero() {
        assert_eq!(move_speed_with_pct(7.0, 0), 7.0);
        assert!((move_speed_with_pct(7.0, -40) - 4.2).abs() < 1e-4);
        assert!(move_speed_with_pct(7.0, 50) > 7.0);
        assert!((move_speed_with_pct(7.0, -95) - 0.7).abs() < 1e-4);
        assert!((move_speed_with_pct(7.0, -200) - 0.7).abs() < 1e-4);
    }

    #[test]
    fn rank_damage_multiplier_scales_swing_range_and_rank0_is_identity() {
        assert_eq!(rank_damage_multiplier_permille(0), 1000);
        assert_eq!(rank_damage_multiplier_permille(1), 1400);
        assert_eq!(rank_damage_multiplier_permille(2), 1800);
        assert_eq!(rank_damage_multiplier_permille(3), 2500);
        assert_eq!(rank_damage_multiplier_permille(4), 1500);
        assert_eq!(rank_damage_multiplier_permille(7), 1000);

        assert_eq!(
            scale_swing_for_rank(CREATURE_MELEE_MIN, CREATURE_MELEE_MAX, 0),
            (CREATURE_MELEE_MIN, CREATURE_MELEE_MAX)
        );
        assert_eq!(scale_swing_for_rank(1, 3, 1), (1, 4));
        assert_eq!(scale_swing_for_rank(10, 20, 3), (25, 50));
    }

    #[test]
    fn resist_mitigation_scales_and_caps_at_75() {
        assert_eq!(resist_mitigation_pct(0, 60), 0);
        assert_eq!(resist_mitigation_pct(0, 2), 0);
        assert_eq!(resist_mitigation_pct(100, 0), 0);
        assert_eq!(resist_mitigation_pct(300, 60), 75);
        assert_eq!(resist_mitigation_pct(10, 2), 75);
        assert_eq!(resist_mitigation_pct(10_000, 60), 75);
        assert_eq!(resist_mitigation_pct(5, 2), 37);
        assert_eq!(resist_mitigation_pct(6, 2), 45);
        assert!(resist_mitigation_pct(150, 60) > resist_mitigation_pct(75, 60));
    }

    #[test]
    fn miss_chance_follows_the_level_hit_curve() {
        assert_eq!(miss_chance_bp(sd(10, 10)), BASE_MISS_BP);
        assert_eq!(miss_chance_bp(sd(10, 5)), BASE_MISS_BP);
        assert_eq!(miss_chance_bp(sd(10, 12)), 600);
        assert_eq!(miss_chance_bp(sd(10, 13)), 900);
        assert!(miss_chance_bp(sd(10, 14)) > miss_chance_bp(sd(10, 13)));
        assert_eq!(miss_chance_bp(sd(1, 60)), MAX_MISS_BP);
    }

    #[test]
    fn crit_and_hit_ratings_fold_into_the_attack_table_bands() {
        assert_eq!(crit_bp_with_bonus(0), CRIT_BP);
        assert_eq!(
            miss_bp_with_bonus(miss_chance_bp(sd(10, 10)), 0),
            miss_chance_bp(sd(10, 10))
        );

        assert_eq!(crit_bp_with_bonus(1000), CRIT_BP + 1000);
        assert!(crit_bp_with_bonus(1000) > crit_bp_with_bonus(0));

        let base_miss = miss_chance_bp(sd(10, 12));
        assert_eq!(miss_bp_with_bonus(base_miss, 500), base_miss - 500);
        assert!(miss_bp_with_bonus(base_miss, 500) < base_miss);

        assert_eq!(
            miss_bp_with_bonus(BASE_MISS_BP, BASE_MISS_BP as i32 + 1000),
            0
        );
        assert_eq!(miss_bp_with_bonus(0, 100), 0);

        assert_eq!(crit_bp_with_bonus(100_000), 10_000);
        assert_eq!(crit_bp_with_bonus(-(CRIT_BP as i32) - 500), 0);
    }

    #[test]
    fn glancing_only_vs_higher_level_and_caps() {
        assert_eq!(glancing_chance_bp(10, 10), 0);
        assert_eq!(glancing_chance_bp(10, 5), 0);
        assert_eq!(glancing_chance_bp(10, 11), GLANCING_BP_PER_LEVEL);
        assert_eq!(glancing_chance_bp(10, 13), 3 * GLANCING_BP_PER_LEVEL);
        assert_eq!(glancing_chance_bp(10, 60), MAX_GLANCING_BP);
    }

    #[test]
    fn attack_table_walks_miss_dodge_parry_glancing_crit_hit_in_order() {
        let t = AttackTable {
            miss: miss_chance_bp(sd(10, 13)),
            dodge: dodge_chance_bp(20, sd(10, 13)),
            parry: PARRY_BP,
            glancing: glancing_chance_bp(10, 13),
            block: 0,
            crit: CRIT_BP,
            crushing: 0,
        };
        let (m, d, p, g, c) = (t.miss, t.dodge, t.parry, t.glancing, t.crit);
        assert_eq!(attack_outcome(0, &t), HIT_MISS);
        assert_eq!(attack_outcome(m - 1, &t), HIT_MISS);
        assert_eq!(attack_outcome(m, &t), HIT_DODGE);
        assert_eq!(attack_outcome(m + d - 1, &t), HIT_DODGE);
        assert_eq!(attack_outcome(m + d, &t), HIT_PARRY);
        assert_eq!(attack_outcome(m + d + p, &t), HIT_GLANCING);
        assert_eq!(attack_outcome(m + d + p + g - 1, &t), HIT_GLANCING);
        assert_eq!(attack_outcome(m + d + p + g, &t), HIT_CRIT);
        assert_eq!(attack_outcome(m + d + p + g + c, &t), HIT_NORMAL);
        assert_eq!(attack_outcome(9_999, &t), HIT_NORMAL);

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
        assert_eq!(crushing_chance_bp(10, 10), 0);
        assert_eq!(crushing_chance_bp(10, 13), 0);
        assert_eq!(crushing_chance_bp(12, 10), 0);
        assert_eq!(crushing_chance_bp(13, 10), CRUSH_BP_PER_LEVEL);
        assert_eq!(crushing_chance_bp(14, 10), 2 * CRUSH_BP_PER_LEVEL);
        assert!(crushing_chance_bp(15, 10) > crushing_chance_bp(14, 10));
        assert_eq!(crushing_chance_bp(60, 1), MAX_CRUSH_BP);

        let t = AttackTable {
            miss: 0,
            dodge: 0,
            parry: 0,
            glancing: 0,
            block: 0,
            crit: 1000,
            crushing: 2000,
        };
        assert_eq!(attack_outcome(999, &t), HIT_CRIT);
        assert_eq!(attack_outcome(1000, &t), HIT_CRUSHING);
        assert_eq!(attack_outcome(2999, &t), HIT_CRUSHING);
        assert_eq!(attack_outcome(3000, &t), HIT_NORMAL);
    }

    #[test]
    fn block_chance_mirrors_parry_structure_and_caps() {
        assert_eq!(block_chance_bp(sd(10, 10)), BLOCK_BP);
        assert_eq!(block_chance_bp(sd(13, 10)), BLOCK_BP);
        assert_eq!(
            block_chance_bp(sd(10, 11)),
            BLOCK_BP + 5 * BLOCK_BP_PER_SKILL
        );
        assert_eq!(
            block_chance_bp(sd(10, 13)),
            BLOCK_BP + 15 * BLOCK_BP_PER_SKILL
        );
        assert!(block_chance_bp(sd(10, 14)) > block_chance_bp(sd(10, 13)));
        assert_eq!(block_chance_bp(sd(1, 1_000_000)), MAX_BLOCK_BP);
        assert_eq!(block_chance_bp(sd(10, 13)), parry_chance_bp(sd(10, 13)));
    }

    #[test]
    fn attack_table_inserts_block_after_glancing_before_crit() {
        let t = AttackTable {
            miss: 900,
            dodge: 500,
            parry: 500,
            glancing: 3000,
            block: 600,
            crit: 1000,
            crushing: 0,
        };
        let pre = t.miss + t.dodge + t.parry + t.glancing;
        assert_eq!(attack_outcome(pre - 1, &t), HIT_GLANCING);
        assert_eq!(attack_outcome(pre, &t), HIT_BLOCK);
        assert_eq!(attack_outcome(pre + t.block - 1, &t), HIT_BLOCK);
        assert_eq!(attack_outcome(pre + t.block, &t), HIT_CRIT);
        assert_eq!(attack_outcome(pre + t.block + t.crit, &t), HIT_NORMAL);
    }

    #[test]
    fn block_zero_band_is_baseline_identical() {
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
        assert_eq!(attack_outcome(after_glancing, &t), HIT_CRIT);
        assert_eq!(attack_outcome(after_glancing + t.crit, &t), HIT_NORMAL);
    }

    #[allow(clippy::too_many_arguments)]
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
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
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
            unit_bytes_2: 0,
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
            death_expire_micros: 0,
            instance_id: 0,
            run_speed_mult_bp: 10_000,
            godmode: false,
            resting: false,
            sheet_str_bonus: 0,
            sheet_agi_bonus: 0,
            sheet_sta_bonus: 0,
            sheet_int_bonus: 0,
            sheet_spi_bonus: 0,
            sheet_ap_base: 0,
            sheet_ap_mods: 0,
            sheet_dmg_min: 0,
            sheet_dmg_max: 0,
            sheet_crit_bp: 0,
            bank_bag_slots: 0,
            mount_display_id: 0,
            zone_id: 0,
            sheet_ranged_ap: 0,
            sheet_ranged_dmg_min: 0,
            sheet_ranged_dmg_max: 0,
            regeneration_pending: false,
            combat_active: false,
            simulation_indexed: false,
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
        let creature = entity_for_regen(0x0101, 42, 42, 0, 0, 0, 1, 0);
        assert!(is_rage_user(&warrior));
        assert!(!is_rage_user(&mage));
        assert!(!is_rage_user(&rogue));
        assert!(!is_rage_user(&creature));
    }

    #[test]
    fn rage_from_damage_is_asymmetric_dealt_vs_taken() {
        assert_eq!(rage_from_damage(10, true), 80);
        assert_eq!(rage_from_damage(10, false), 30);
        assert_eq!(rage_from_damage(0, true), 0);
        assert!(rage_from_damage(10, true) > rage_from_damage(10, false));
    }

    #[test]
    fn regen_entity_power_wrapper_reads_fields_and_straddles_the_fsr_window() {
        use lyracore_shared::packing::{power_type, unit_bytes_0};
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
        assert_eq!(
            regen_entity_power(&mage, false, RegenWindow::new(4_999, 4_000), 0, 1.0),
            100
        );
        assert_eq!(
            regen_entity_power(&mage, false, RegenWindow::new(5_000, 4_000), 0, 1.0),
            100 + 26
        );
        assert_eq!(
            regen_entity_power(&mage, true, RegenWindow::new(6_000, 4_000), 0, 1.0),
            100 + 26
        );

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
        assert_eq!(
            regen_entity_power(&warrior, false, RegenWindow::new(0, 4_000), 0, 1.0),
            1000 - 50
        );
        assert_eq!(
            regen_entity_power(&warrior, true, RegenWindow::new(0, 4_000), 0, 1.0),
            1000
        );
    }

    #[test]
    fn regen_entity_health_wrapper_reads_spirit_and_level_off_the_row() {
        let e = entity_for_regen(0, 100, 1000, 0, 0, 30, 10, 0);
        assert_eq!(
            regen_entity_health(&e, 0, RegenWindow::new(4_000, 4_000)),
            100 + (30 + 10 + 1)
        );
    }

    #[test]
    fn points_per_five_are_converted_to_the_four_second_regen_tick() {
        use lyracore_shared::packing::{power_type, unit_bytes_0};

        let mage = entity_for_regen(
            unit_bytes_0(1, 8, 0, power_type::MANA),
            100,
            1000,
            100,
            1000,
            40,
            10,
            0,
        );
        let window = RegenWindow::new(8_000, 4_000);
        assert_eq!(regen_entity_power(&mage, false, window, 10, 1.0), 134);
        assert_eq!(regen_entity_health(&mage, 15, window), 163);
        assert_eq!(regen_entity_power(&mage, false, window, -40, 1.0), 100);
        assert_eq!(regen_entity_health(&mage, -100, window), 100);

        let low_rate_total: i32 = (4_000..=20_000)
            .step_by(4_000)
            .map(|now_ms| per_five_in_window(1, RegenWindow::new(now_ms, 4_000)))
            .sum();
        assert_eq!(low_rate_total, 4);
    }

    #[test]
    fn flat_mana_regen_continues_during_the_five_second_rule() {
        use lyracore_shared::packing::{power_type, unit_bytes_0};

        let mage = entity_for_regen(
            unit_bytes_0(1, 8, 0, power_type::MANA),
            100,
            1000,
            100,
            1000,
            40,
            10,
            10_000,
        );
        assert_eq!(
            regen_entity_power(&mage, false, RegenWindow::new(8_000, 4_000), 10, 1.0),
            108
        );
    }

    #[test]
    fn regen_power_default_branch_leaves_focus_and_happiness_unchanged() {
        use lyracore_shared::packing::power_type as pt;
        assert_eq!(regen_power(pt::FOCUS, 50, 100, 40, 10, false, false), 50);
        assert_eq!(regen_power(pt::HAPPINESS, 50, 100, 40, 10, true, false), 50);
    }

    #[test]
    #[allow(clippy::identity_op)]
    fn power_regen_by_type_and_combat_state() {
        use lyracore_shared::packing::power_type as pt;
        const FSR_ACTIVE: bool = true;
        const FSR_CLEAR: bool = false;
        const IN_COMBAT: bool = true;
        const OUT_OF_COMBAT: bool = false;

        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, OUT_OF_COMBAT, FSR_CLEAR),
            100 + 26
        );
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, OUT_OF_COMBAT, FSR_ACTIVE),
            100
        );
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, IN_COMBAT, FSR_ACTIVE),
            100
        );
        assert_eq!(
            regen_power(pt::MANA, 100, 500, 40, 10, IN_COMBAT, FSR_CLEAR),
            100 + 26
        );
        assert_eq!(
            regen_power(pt::MANA, 490, 500, 40, 10, OUT_OF_COMBAT, FSR_CLEAR),
            500
        );
        let lo = regen_power(pt::MANA, 0, 5000, 20, 10, OUT_OF_COMBAT, FSR_CLEAR);
        let hi = regen_power(pt::MANA, 0, 5000, 200, 10, OUT_OF_COMBAT, FSR_CLEAR);
        assert!(hi > lo, "more spirit must regen more mana ({hi} !> {lo})");
        assert_eq!(
            regen_power(pt::MANA, 0, 500, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            0 + (10 / 2 + 1)
        );
        assert!(regen_power(pt::MANA, 0, 500, 0, 1, OUT_OF_COMBAT, FSR_CLEAR) > 0);
        assert_eq!(
            regen_power(pt::ENERGY, 20, 100, 0, 10, IN_COMBAT, FSR_CLEAR),
            20,
            "energy has its own 2 s pass"
        );
        assert_eq!(
            regen_power(pt::RAGE, 1000, 1000, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            1000 - 50
        );
        assert_eq!(
            regen_power(pt::RAGE, 1000, 1000, 0, 10, IN_COMBAT, FSR_CLEAR),
            1000
        );
        assert_eq!(
            regen_power(pt::RAGE, 30, 1000, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            0
        );
        assert_eq!(
            regen_power(pt::RAGE, 80, 1000, 0, 10, OUT_OF_COMBAT, FSR_CLEAR),
            30
        );
        assert_eq!(
            regen_power(pt::MANA, 0, 0, 40, 10, OUT_OF_COMBAT, FSR_CLEAR),
            0
        );
    }

    #[test]
    fn energy_ticks_count_each_two_second_boundary_once() {
        assert_eq!(energy_ticks_between(10_000, 11_999), 0);
        assert_eq!(energy_ticks_between(10_000, 12_000), 1);
        assert_eq!(energy_ticks_between(11_999, 12_001), 1);
        assert_eq!(
            energy_ticks_between(10_500, 14_600),
            2,
            "a late firing keeps both ticks"
        );
        assert_eq!(
            energy_ticks_between(12_000, 12_400),
            0,
            "an early firing adds none"
        );
    }

    #[test]
    fn energy_regen_is_twenty_per_tick_and_adrenaline_rush_doubles_it() {
        assert_eq!(regen_energy(0, 100, 1, 1.0), 20);
        assert_eq!(regen_energy(0, 100, 2, 1.0), 40);
        assert_eq!(regen_energy(0, 100, 1, 2.0), 40);
        assert_eq!(regen_energy(90, 100, 1, 1.0), 100);
        assert_eq!(regen_energy(0, 100, u64::MAX, 2.0), 100);
    }

    #[test]
    fn power_regen_percent_scales_spirit_mana_and_rage_decay() {
        use lyracore_shared::packing::{power_type, unit_bytes_0};
        let mage = entity_for_regen(
            unit_bytes_0(1, 8, 0, power_type::MANA),
            100,
            1000,
            100,
            1000,
            40,
            10,
            0,
        );
        assert_eq!(
            regen_entity_power(&mage, false, RegenWindow::new(8_000, 4_000), 0, 2.0),
            100 + 52
        );
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
        assert_eq!(
            regen_entity_power(&warrior, false, RegenWindow::new(0, 4_000), 0, 2.0),
            1000 - 100
        );
    }

    #[test]
    fn fsr_gate_pins_mana_regen_window() {
        use lyracore_shared::packing::power_type as pt;
        let spirit = 40u32;
        let level = 10u32;
        assert_eq!(
            regen_power(pt::MANA, 100, 500, spirit, level, false, true),
            100
        );
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
    #[allow(clippy::identity_op)]
    fn health_regen_is_spirit_and_level_scaled_and_clamped() {
        assert_eq!(regen_health(100, 1000, 30, 10), 100 + 41);
        let lo = regen_health(0, 100_000, 10, 10);
        let hi = regen_health(0, 100_000, 100, 10);
        assert!(hi > lo, "more spirit must regen more health ({hi} !> {lo})");
        assert_eq!(regen_health(995, 1000, 30, 10), 1000);
        assert_eq!(regen_health(0, 1000, 0, 9), 0 + 10);
        assert!(regen_health(0, 1000, 0, 1) > 0);
        assert_eq!(regen_health(0, 0, 30, 10), 0);
    }

    #[test]
    fn combat_regen_is_partial_fraction_of_normal_tick() {
        assert_eq!(regen_health_in_combat(100, 1000, 30, 10, 0), 100);
        assert_eq!(
            regen_health_in_combat(100, 1000, 30, 10, 100),
            regen_health(100, 1000, 30, 10)
        );
        assert_eq!(regen_health_in_combat(100, 1000, 0, 5, 10), 100);
        assert_eq!(regen_health_in_combat(100, 1000, 50, 20, 10), 100 + 7);
        assert_eq!(
            regen_health_in_combat(100, 1000, 50, 20, 40),
            100 + 71 * 40 / 100
        );
        assert!(
            regen_health_in_combat(0, 10000, 50, 20, 50)
                >= regen_health_in_combat(0, 10000, 50, 20, 10)
        );
        assert_eq!(regen_health_in_combat(995, 1000, 50, 20, 100), 1000);
        assert_eq!(regen_health_in_combat(0, 0, 30, 10, 10), 0);
    }

    #[test]
    fn flat_health_regen_continues_in_combat_without_a_percentage_aura() {
        let e = entity_for_regen(0, 100, 1000, 0, 0, 30, 10, 0);
        assert_eq!(
            regen_entity_health_in_combat(&e, 0, 15, RegenWindow::new(8_000, 4_000)),
            112
        );
    }

    #[test]
    fn armor_mitigation_follows_the_vanilla_curve() {
        assert_eq!(armor_mitigation_pct(0, 1), 0);
        assert_eq!(armor_mitigation_pct(42, 1), 7);
        assert!(armor_mitigation_pct(42, 60) < armor_mitigation_pct(42, 1));
        assert_eq!(armor_mitigation_pct(1_000_000, 1), 75);
    }

    #[test]
    #[allow(clippy::unnecessary_min_or_max)]
    fn swing_profile_applies_mitigation_to_the_range() {
        let p = swing_profile(5, 7, 0, 1);
        assert_eq!((p.base_min, p.base_max, p.mitigation_pct), (5, 7, 0));
        assert_eq!((p.final_min, p.final_max), (5, 7));
        let q = swing_profile(1, 3, 42, 1);
        assert_eq!(q.mitigation_pct, 7);
        assert_eq!(q.final_min, 1);
        assert_eq!(q.final_max, (3 * 93 / 100).max(1));
    }

    mod offhand_tests {
        use super::apply_offhand_penalty;

        #[test]
        fn apply_offhand_penalty_halves_the_range() {
            assert_eq!(apply_offhand_penalty(10, 20), (5, 10));
            assert_eq!(apply_offhand_penalty(2, 4), (1, 2));
        }

        #[test]
        fn apply_offhand_penalty_floors_at_one() {
            assert_eq!(apply_offhand_penalty(1, 1), (1, 1));
            assert_eq!(apply_offhand_penalty(0, 1), (1, 1));
            assert_eq!(apply_offhand_penalty(0, 0), (1, 1));
        }
    }
}
