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

pub(super) const PARRY_BP: u32 = 500;
pub(super) const BLOCK_BP: u32 = 500;
const BASE_DODGE_BP: u32 = 500;
const DODGE_BP_PER_AGILITY: u32 = 5;
const MAX_DODGE_BP: u32 = 3000;
pub(crate) const GLANCING_DAMAGE_PCT: u32 = 70;

const CRUSH_MIN_LEVEL_DIFF: u32 = 3;
const CRUSH_BP_PER_LEVEL: u32 = 1000;
const MAX_CRUSH_BP: u32 = 5000;
pub(crate) const CRUSH_DAMAGE_PCT: u32 = 150;

const UNARMED_BASE_MIN: u32 = 1;
const UNARMED_BASE_MAX: u32 = 3;
const AP_PER_DPS: u32 = 14;

pub(crate) fn dodge_chance_bp(agility: u32) -> u32 {
    (BASE_DODGE_BP + agility * DODGE_BP_PER_AGILITY).min(MAX_DODGE_BP)
}

pub(crate) fn crushing_chance_bp(attacker_level: u32, target_level: u32) -> u32 {
    let diff = attacker_level.saturating_sub(target_level);
    if diff < CRUSH_MIN_LEVEL_DIFF {
        return 0;
    }
    ((diff - CRUSH_MIN_LEVEL_DIFF + 1) * CRUSH_BP_PER_LEVEL).min(MAX_CRUSH_BP)
}

pub(super) fn sheet_crit_bp(
    agility: u32,
    class: u8,
    level: u32,
    rate: Option<f32>,
    weapon: i32,
    maximum: i32,
    bonus: i32,
) -> u32 {
    let Some(rate) = rate.filter(|rate| rate.is_finite() && *rate > 0.0) else {
        let agility_bp = i64::from(agility) * 100 / (i64::from(level) + 4);
        return (1000 + agility_bp + i64::from(bonus)).clamp(0, 10_000) as u32;
    };
    let offset = match class {
        2 => 70,
        5 => 300,
        7 => 170,
        8 => 320,
        9 => 200,
        11 => 90,
        _ => 0,
    };
    let agility = agility as f32 / rate * 100.0;
    (agility + (offset + bonus + (weapon - maximum) * 4) as f32).clamp(0.0, 10_000.0) as u32
}

pub(crate) fn melee_attack_power(strength: u32, level: u32) -> u32 {
    (level * 3 + strength * 2).saturating_sub(20)
}

const CLASS_WARRIOR: u8 = 1;
const CLASS_HUNTER: u8 = 3;
const CLASS_ROGUE: u8 = 4;
pub(crate) const CLASS_PRIEST: u8 = 5;
pub(crate) const CLASS_MAGE: u8 = 8;
pub(crate) const CLASS_WARLOCK: u8 = 9;

pub(crate) fn melee_attack_power_for(class: u8, strength: u32, agility: u32, level: u32) -> u32 {
    match class {
        CLASS_ROGUE | CLASS_HUNTER => (level * 2 + strength + agility).saturating_sub(20),
        _ => melee_attack_power(strength, level),
    }
}

pub(crate) fn ranged_attack_power_for(class: u8, agility: u32, level: u32) -> u32 {
    let raw = match class {
        CLASS_WARRIOR | CLASS_ROGUE => level + agility,
        CLASS_HUNTER => level * 2 + agility * 2,
        _ => agility,
    };
    raw.saturating_sub(10)
}

pub(crate) fn ranged_sheet_values(
    class: u8,
    agility: u32,
    level: u32,
    weapon: Option<(u32, u32, u32, u8)>,
    bonus: i32,
) -> (u32, f32, f32) {
    use crate::items::weapon_subclass as ws;

    let Some((dmg_min, dmg_max, delay_ms, subclass)) = weapon else {
        return (0, 0.0, 0.0);
    };
    if matches!(subclass, ws::BOW | ws::GUN | ws::CROSSBOW | ws::THROWN) {
        let base = ranged_attack_power_for(class, agility, level);
        let attack_power = base.saturating_add_signed(bonus);
        let bonus = attack_power as f32 / AP_PER_DPS as f32 * (delay_ms as f32 / 1000.0);
        (base, dmg_min as f32 + bonus, dmg_max as f32 + bonus)
    } else {
        (0, dmg_min as f32, dmg_max as f32)
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

pub(crate) fn swing_time_with_haste(base_ms: u32, haste_pct: i32) -> u32 {
    let denom = (100 + haste_pct).max(1) as u32;
    base_ms.saturating_mul(100) / denom
}

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

pub(crate) fn armor_mitigated(damage: u32, mitigation_pct: u32) -> u32 {
    (u64::from(damage) * u64::from(100 - mitigation_pct) / 100).max(1) as u32
}

#[cfg(feature = "debug_reducers")]
pub struct SwingProfile {
    pub base_min: u32,
    pub base_max: u32,
    pub mitigation_pct: u32,
    pub final_min: u32,
    pub final_max: u32,
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn swing_profile(
    base_min: u32,
    base_max: u32,
    target_armor: u32,
    attacker_level: u32,
) -> SwingProfile {
    let mitigation_pct = armor_mitigation_pct(target_armor, attacker_level);
    SwingProfile {
        base_min,
        base_max,
        mitigation_pct,
        final_min: armor_mitigated(base_min, mitigation_pct),
        final_max: armor_mitigated(base_max, mitigation_pct),
    }
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

fn regen_health_in_combat_with_flat(
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    combat_regen_pct: u32,
    health_flat_tick: i32,
) -> u32 {
    let partial = (i128::from(health_regen_per_tick(spirit, level)) * i128::from(combat_regen_pct)
        / 100)
        .clamp(0, i128::from(u32::MAX)) as u32;
    apply_regen(current, max, partial, health_flat_tick)
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn regen_health_in_combat(
    current: u32,
    max: u32,
    spirit: u32,
    level: u32,
    combat_regen_pct: u32,
) -> u32 {
    regen_health_in_combat_with_flat(current, max, spirit, level, combat_regen_pct, 0)
}

pub(crate) fn regen_entity_health_in_combat(
    e: &WorldEntity,
    combat_regen_pct: u32,
    health_per_five: i32,
    window: RegenWindow,
) -> u32 {
    regen_health_in_combat_with_flat(
        e.health,
        e.max_health,
        e.spirit,
        e.level,
        combat_regen_pct,
        per_five_in_window(health_per_five, window),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ranged_attack_power_uses_each_class_formula() {
        for (class, expected) in [(1, 180), (4, 180), (3, 370), (7, 120)] {
            assert_eq!(ranged_attack_power_for(class, 130, 60), expected);
            assert_eq!(ranged_attack_power_for(class, 0, 1), 0);
        }
    }

    #[test]
    fn ranged_sheet_launcher_uses_attack_power_and_weapon_speed() {
        assert_eq!(
            ranged_sheet_values(
                3,
                20,
                10,
                Some((8, 12, 2800, crate::items::weapon_subclass::BOW)),
                0
            ),
            (50, 18.0, 22.0)
        );
    }

    #[test]
    fn spell_9329_adds_ranged_attack_power_to_thrown_damage_without_changing_its_base() {
        assert_eq!(
            ranged_sheet_values(
                4,
                0,
                1,
                Some((10, 10, 14000, crate::items::weapon_subclass::THROWN)),
                16
            ),
            (0, 26.0, 26.0)
        );
    }

    #[test]
    fn spell_9329_with_wicked_throwing_dagger_uses_rogue_agility() {
        for class in [1, 4, 3] {
            let (ap, min, max) = ranged_sheet_values(
                class,
                130,
                60,
                Some((23, 43, 1800, crate::items::weapon_subclass::THROWN)),
                16,
            );
            let (expected_ap, expected_min, expected_max) = if class == 3 {
                (370, 72.62857, 92.62857)
            } else {
                (180, 48.2, 68.2)
            };
            assert_eq!(ap, expected_ap);
            assert!((min - expected_min).abs() < 0.0001);
            assert!((max - expected_max).abs() < 0.0001);
        }
    }

    #[test]
    fn ranged_sheet_wand_keeps_flat_weapon_damage() {
        assert_eq!(
            ranged_sheet_values(
                5,
                130,
                60,
                Some((8, 12, 2800, crate::items::weapon_subclass::WAND)),
                16
            ),
            (0, 8.0, 12.0)
        );
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
    fn crit_uses_class_rates_offsets_and_actual_weapon_skill() {
        assert_eq!(sheet_crit_bp(100, 1, 60, Some(20.0), 300, 300, 0), 500);
        assert_eq!(sheet_crit_bp(20, 1, 1, Some(4.0), 5, 5, 0), 500);
        assert_eq!(sheet_crit_bp(100, 4, 60, Some(28.9855), 300, 300, 0), 345);
        assert_eq!(sheet_crit_bp(100, 8, 60, Some(19.4553), 300, 300, 0), 833);
        assert_eq!(sheet_crit_bp(100, 1, 60, Some(20.0), 295, 300, 0), 480);
        assert_eq!(sheet_crit_bp(100, 1, 60, Some(20.0), 305, 300, 0), 520);
    }

    #[test]
    fn missing_crit_rate_preserves_the_previous_formula() {
        assert_eq!(sheet_crit_bp(100, 8, 60, None, 300, 300, 0), 1156);
        assert_eq!(sheet_crit_bp(100, 8, 60, None, 295, 300, 200), 1356);
        assert_eq!(sheet_crit_bp(20, 4, 1, None, 5, 5, 0), 1400);
        assert_eq!(sheet_crit_bp(100, 8, 60, None, 300, 300, -1500), 0);
        assert_eq!(sheet_crit_bp(100, 8, 60, None, 300, 300, 10_000), 10_000);
    }

    #[test]
    fn dodge_scales_with_agility_and_caps() {
        assert_eq!(dodge_chance_bp(0), 500);
        assert_eq!(dodge_chance_bp(20), 600);
        assert_eq!(dodge_chance_bp(1_000_000), 3000);
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
    fn crushing_blows_arm_only_three_levels_above_and_cap() {
        assert_eq!(crushing_chance_bp(10, 10), 0);
        assert_eq!(crushing_chance_bp(10, 13), 0);
        assert_eq!(crushing_chance_bp(12, 10), 0);
        assert_eq!(crushing_chance_bp(13, 10), 1000);
        assert_eq!(crushing_chance_bp(14, 10), 2000);
        assert!(crushing_chance_bp(15, 10) > crushing_chance_bp(14, 10));
        assert_eq!(crushing_chance_bp(60, 1), 5000);
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
            health,
            max_health,
            power,
            max_power,
            level,
            unit_bytes_0,
            spirit,
            mana_regen_paused_until_ms,
            ..WorldEntity::blank(1, 0, 0, [0.0; 4])
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
    fn health_regen_is_spirit_and_level_scaled_and_clamped() {
        assert_eq!(regen_health_with_flat(100, 1000, 30, 10, 0), 141);
        assert_eq!(regen_health_with_flat(0, 100_000, 10, 10, 0), 21);
        assert_eq!(regen_health_with_flat(0, 100_000, 100, 10, 0), 111);
        assert_eq!(regen_health_with_flat(995, 1000, 30, 10, 0), 1000);
        assert_eq!(regen_health_with_flat(0, 1000, 0, 9, 0), 10);
        assert_eq!(regen_health_with_flat(0, 1000, 0, 1, 0), 2);
        assert_eq!(regen_health_with_flat(0, 0, 30, 10, 0), 0);
    }

    #[test]
    fn combat_regen_is_partial_fraction_of_normal_tick() {
        assert_eq!(
            regen_health_in_combat_with_flat(100, 1000, 30, 10, 0, 0),
            100
        );
        assert_eq!(
            regen_health_in_combat_with_flat(100, 1000, 30, 10, 100, 0),
            141
        );
        assert_eq!(
            regen_health_in_combat_with_flat(100, 1000, 0, 5, 10, 0),
            100
        );
        assert_eq!(
            regen_health_in_combat_with_flat(100, 1000, 50, 20, 10, 0),
            107
        );
        assert_eq!(
            regen_health_in_combat_with_flat(100, 1000, 50, 20, 40, 0),
            128
        );
        assert_eq!(
            regen_health_in_combat_with_flat(0, 10000, 50, 20, 50, 0),
            35
        );
        assert_eq!(
            regen_health_in_combat_with_flat(0, 10000, 300, 60, 50, 0),
            180
        );
        assert_eq!(
            regen_health_in_combat_with_flat(995, 1000, 50, 20, 100, 0),
            1000
        );
        assert_eq!(regen_health_in_combat_with_flat(0, 0, 30, 10, 10, 0), 0);
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
    fn armor_mitigation_scales_the_hit_and_never_drops_below_one() {
        assert_eq!(armor_mitigated(7, 0), 7);
        assert_eq!(armor_mitigated(3, 7), 2);
        assert_eq!(armor_mitigated(1, 7), 1);
        assert_eq!(armor_mitigated(1000, 75), 250);
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
