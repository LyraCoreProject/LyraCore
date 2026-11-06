use spacetimedb::ReducerContext;

pub(super) fn target_draw(ctx: &ReducerContext, _target_guid: u64) -> u32 {
    #[cfg(feature = "conformance_test_fixture")]
    if let Some(draw) = crate::conformance_rolls::spell_target_draw(ctx, _target_guid) {
        return draw;
    }
    ctx.random::<u32>()
}

pub(super) fn integer_roll(ctx: &ReducerContext, target_guid: u64, maximum: u32) -> u32 {
    loop {
        if let Some(roll) = integer_candidate(target_draw(ctx, target_guid), maximum) {
            return roll;
        }
    }
}

pub(super) fn magic_miss_bp(
    caster_level: u32,
    target_level: u32,
    target_is_character: bool,
    binary_resistance: f32,
) -> u32 {
    let difference = i64::from(target_level) - i64::from(caster_level);
    let hit = if difference < 3 {
        96 - difference
    } else {
        94 - (difference - 2) * if target_is_character { 7 } else { 11 }
    };
    let hit = hit.max(22) as f32 * (1.0 - binary_resistance);
    10_000 - ((hit * 100.0) as u32).clamp(100, 9900)
}

pub(super) fn intellect_crit_chance(class: Option<u8>, intellect: u32, level: u32) -> f32 {
    let Some(class) = class else {
        return 5.0;
    };
    let (base, rate, per_level) = match class {
        2 | 8 => (3.70, 14.77, 0.65),
        5 => (2.97, 10.03, 0.82),
        7 => (3.54, 11.51, 0.80),
        9 => (3.18, 11.30, 0.82),
        11 => (3.33, 12.41, 0.79),
        _ => (0.0, 0.0, 10.0),
    };
    base + intellect as f32 / (rate + per_level * level.max(1) as f32)
}

pub(super) fn chance_succeeds(chance: f32, draw: u32) -> bool {
    f64::from(chance) > 100.0 * f64::from(draw) / 4_294_967_296.0
}

// Inclusive rejection sampling follows MTRand::randInt; see ADR 0007 for the roll contract.
pub(super) fn integer_candidate(draw: u32, maximum: u32) -> Option<u32> {
    if maximum == 0 {
        return Some(0);
    }
    let candidate = draw & (u32::MAX >> maximum.leading_zeros());
    (candidate <= maximum).then_some(candidate)
}

pub(super) fn resistance_chance(
    resistance: i32,
    caster_level: u32,
    target_level: u32,
    target_is_character: bool,
    innate: bool,
) -> f32 {
    if caster_level == 0 {
        return 0.0;
    }
    if resistance < 0 {
        return (resistance as f32 / (target_level as f32 * 5.0).max(100.0)).max(-0.75);
    }
    let innate = if innate && !target_is_character {
        let difference = i64::from(target_level) - i64::from(caster_level);
        ((8.0 * difference as f32) * caster_level as f32 / 63.0) as i32
    } else {
        0
    };
    (resistance.saturating_add(innate) as f32 * (0.15 / caster_level as f32)).clamp(0.0, 0.75)
}

// vmangos Unit::RollMagicResistanceMultiplierOutcomeAgainst, pinned by ADR 0007.
const RESISTANCE_ROWS: [(u8, [u8; 4]); 31] = [
    (0, [0, 0, 0, 0]),
    (3, [0, 0, 2, 6]),
    (5, [0, 1, 4, 12]),
    (8, [0, 1, 5, 18]),
    (10, [0, 1, 7, 23]),
    (13, [0, 2, 9, 28]),
    (15, [0, 2, 11, 33]),
    (18, [0, 2, 13, 37]),
    (20, [0, 3, 15, 41]),
    (23, [1, 3, 17, 46]),
    (25, [1, 4, 19, 47]),
    (28, [1, 5, 21, 48]),
    (30, [1, 6, 24, 49]),
    (33, [1, 8, 28, 47]),
    (35, [1, 9, 33, 43]),
    (38, [1, 11, 37, 39]),
    (40, [1, 13, 41, 35]),
    (43, [1, 16, 45, 30]),
    (45, [1, 18, 48, 26]),
    (48, [2, 20, 48, 24]),
    (50, [4, 23, 48, 21]),
    (53, [5, 25, 47, 19]),
    (55, [7, 28, 45, 17]),
    (58, [9, 31, 43, 16]),
    (60, [11, 34, 40, 14]),
    (62, [13, 37, 37, 12]),
    (65, [15, 41, 33, 10]),
    (68, [18, 44, 29, 8]),
    (70, [20, 48, 25, 7]),
    (73, [23, 51, 20, 5]),
    (75, [25, 55, 16, 3]),
];

pub(super) fn resistance_quarters(chance: f32, periodic: bool, roll: u32) -> i8 {
    let negative = chance < 0.0;
    let mut percent = chance.abs().min(0.75) * 100.0;
    if periodic {
        percent *= 0.1;
    }
    let pair = RESISTANCE_ROWS
        .windows(2)
        .find(|pair| f32::from(pair[1].0) >= percent)
        .expect("integer-derived resistance is finite and capped at 75 percent");
    let coefficient = (percent - f32::from(pair[0].0)) / f32::from(pair[1].0 - pair[0].0);
    let weights: [f32; 4] = std::array::from_fn(|index| {
        let previous = f32::from(pair[0].1[index]);
        previous + (f32::from(pair[1].1[index]) - previous) * coefficient
    });
    let roll = roll as f32;
    let quarters = if roll < weights[0] + weights[1] {
        3
    } else if roll < weights[0] + weights[1] + weights[2] {
        2
    } else if roll < weights[0] + weights[1] + weights[2] + weights[3] {
        1
    } else {
        0
    };
    if negative {
        -quarters
    } else {
        quarters
    }
}

pub(super) fn damage_after_resistance(damage: i32, quarters: i8, draw: u32) -> i32 {
    let numerator = i64::from(damage.max(0)) * i64::from(quarters.abs());
    let remainder = numerator % 4;
    let rounded_up = remainder != 0 && u64::from(draw) >= (4 - remainder) as u64 * 1_073_741_824;
    let resisted = (numerator / 4 + i64::from(rounded_up)) as i32;
    if quarters < 0 {
        damage.saturating_add(resisted)
    } else {
        damage.saturating_sub(resisted).max(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inclusive_magic_hit_rolls_have_the_source_interval_widths() {
        for (level, landed) in [(60, 9601), (63, 8301)] {
            let miss = magic_miss_bp(60, level, false, 0.0);
            let hits = (0..16_384)
                .filter_map(|draw| integer_candidate(draw, 10_000))
                .filter(|roll| *roll >= miss)
                .count();
            assert_eq!(hits, landed);
            assert_eq!(integer_candidate(miss - 1, 10_000), Some(miss - 1));
            assert_eq!(integer_candidate(10_000, 10_000), Some(10_000));
            assert_eq!(integer_candidate(10_001, 10_000), None);
        }
        assert_eq!(magic_miss_bp(60, 63, true, 0.0), 1300);
        assert_eq!(magic_miss_bp(60, 1, false, 0.0), 100);
        assert_eq!(magic_miss_bp(1, 60, false, 0.0), 7800);
        assert_eq!(magic_miss_bp(60, 63, false, 0.5), 5850);
    }

    #[test]
    fn mage_crit_keeps_single_precision_and_the_full_random_word() {
        let chance = intellect_crit_chance(Some(8), 125, 60);
        assert_eq!(chance.to_bits(), 0x40c0_ca7a);
        assert!(chance_succeeds(chance, 258_759_598));
        assert!(!chance_succeeds(chance, 258_759_599));
        assert!(!chance_succeeds(0.0, 0));
        assert!(chance_succeeds(100.0, u32::MAX));
        assert_eq!(intellect_crit_chance(None, 0, 60), 5.0);
        assert_eq!(intellect_crit_chance(Some(2), 125, 60), chance);
        assert_eq!(intellect_crit_chance(Some(1), 60, 60), 0.1);
        for class in [5, 7, 9, 11] {
            assert!(intellect_crit_chance(Some(class), 125, 60) > 5.0);
        }
    }

    fn intervals(chance: f32, periodic: bool) -> [usize; 4] {
        let mut counts = [0; 4];
        for roll in 0..100 {
            counts[resistance_quarters(chance, periodic, roll).unsigned_abs() as usize] += 1;
        }
        counts
    }

    #[test]
    fn resistance_interpolates_before_comparing_integer_rolls() {
        let chance = resistance_chance(0, 60, 63, false, true);
        assert_eq!(intervals(chance, false), [81, 13, 5, 1]);
        assert_eq!(resistance_quarters(chance, false, 0), 3);
        assert_eq!(resistance_quarters(chance, false, 1), 2);
        assert_eq!(resistance_quarters(chance, false, 5), 2);
        assert_eq!(resistance_quarters(chance, false, 6), 1);
        assert_eq!(resistance_quarters(chance, false, 18), 1);
        assert_eq!(resistance_quarters(chance, false, 19), 0);
        assert_eq!(
            intervals(resistance_chance(20, 60, 60, false, true), false),
            [83, 12, 4, 1]
        );
        assert_eq!(
            intervals(resistance_chance(0, 60, 60, false, true), false),
            [100, 0, 0, 0]
        );
        assert_eq!(
            intervals(resistance_chance(300, 60, 60, false, true), false),
            [1, 3, 16, 80]
        );
        assert_eq!(integer_candidate(99, 99), Some(99));
        assert_eq!(integer_candidate(100, 99), None);
    }

    #[test]
    fn creature_level_resistance_is_separate_from_gear_and_binary_resistance() {
        assert_eq!(resistance_chance(0, 60, 63, true, true), 0.0);
        assert_eq!(resistance_chance(0, 60, 63, false, false), 0.0);
        assert_eq!(resistance_chance(0, 63, 60, false, true), 0.0);
        assert_eq!(resistance_chance(3000, 60, 60, true, true), 0.75);
        assert_eq!(resistance_chance(-300, 60, 60, true, true), -0.75);
        assert_eq!(resistance_chance(-50, 60, 1, true, true), -0.5);
    }

    #[test]
    fn periodic_resistance_uses_the_source_reduced_distribution() {
        let chance = resistance_chance(0, 60, 63, false, true);
        assert_eq!(intervals(chance, true), [98, 1, 1, 0]);
        assert_eq!(resistance_quarters(-0.05, false, 0), -3);
    }

    #[test]
    fn quarter_damage_rounds_stochastically_and_can_resist_a_small_hit_completely() {
        assert_eq!(damage_after_resistance(400, 3, u32::MAX), 100);
        assert_eq!(damage_after_resistance(1, 1, 3_221_225_471), 1);
        assert_eq!(damage_after_resistance(1, 1, 3_221_225_472), 0);
        assert_eq!(damage_after_resistance(3, 1, 0), 3);
        assert_eq!(damage_after_resistance(3, 1, u32::MAX), 2);
        assert_eq!(damage_after_resistance(3, -1, u32::MAX), 4);
        assert_eq!(damage_after_resistance(400, 0, u32::MAX), 400);
    }
}
