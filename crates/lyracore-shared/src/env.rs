use crate::movement_flags::{ON_TRANSPORT, SWIMMING};

// Collision height in yards.
pub const UNDERWATER_HEAD_HEIGHT: f32 = 2.0;

pub fn is_submerged(
    player_z: f32,
    liquid_level: f32,
    has_liquid: bool,
    movement_flags: u32,
) -> bool {
    has_liquid && movement_flags & SWIMMING != 0 && player_z + UNDERWATER_HEAD_HEIGHT < liquid_level
}

// Deep water still counts this many yards above its surface.
pub const DEEP_WATER_SURFACE_REACH: f32 = 2.0;

pub fn is_in_deep_water(player_z: f32, deep_water_level: Option<f32>, movement_flags: u32) -> bool {
    deep_water_level.is_some_and(|level| {
        movement_flags & ON_TRANSPORT == 0 && player_z <= level + DEEP_WATER_SURFACE_REACH
    })
}

pub const FULL_AIR_MICROS: i64 = 60_000_000;
pub const REFILL_MICROS: i64 = 10_000_000;
pub const DROWNING_INTERVAL_MICROS: i64 = 1_000_000;
pub const MAX_CATCHUP_TICKS: u32 = 5;

pub const FULL_FATIGUE_MICROS: i64 = 60_000_000;
// Fatigue recovers ten times faster than it drains.
pub const FATIGUE_RECOVERY_MICROS: i64 = FULL_FATIGUE_MICROS / 10;

#[derive(Debug, PartialEq, Eq)]
pub struct MirrorTimerAdvance {
    pub remaining_micros: i64,
    pub expired_ticks: u32,
}

pub fn advance_breath(
    remaining_micros: i64,
    draining: bool,
    elapsed_micros: i64,
) -> MirrorTimerAdvance {
    advance_mirror_timer(
        remaining_micros,
        FULL_AIR_MICROS,
        REFILL_MICROS,
        draining,
        elapsed_micros,
    )
}

pub fn advance_fatigue(
    remaining_micros: i64,
    in_deep_water: bool,
    elapsed_micros: i64,
) -> MirrorTimerAdvance {
    advance_mirror_timer(
        remaining_micros,
        FULL_FATIGUE_MICROS,
        FATIGUE_RECOVERY_MICROS,
        in_deep_water,
        elapsed_micros,
    )
}

fn advance_mirror_timer(
    remaining_micros: i64,
    full_micros: i64,
    refill_micros: i64,
    draining: bool,
    elapsed_micros: i64,
) -> MirrorTimerAdvance {
    let elapsed = elapsed_micros.max(0);
    let remaining = remaining_micros.clamp(0, full_micros);
    if !draining {
        let restored = elapsed.saturating_mul(full_micros) / refill_micros;
        return MirrorTimerAdvance {
            remaining_micros: remaining.saturating_add(restored).min(full_micros),
            expired_ticks: 0,
        };
    }
    if remaining >= elapsed {
        return MirrorTimerAdvance {
            remaining_micros: remaining - elapsed,
            expired_ticks: 0,
        };
    }
    let after_empty = elapsed - remaining;
    let ticks = if remaining == 0 {
        after_empty / DROWNING_INTERVAL_MICROS
    } else {
        1 + after_empty / DROWNING_INTERVAL_MICROS
    };
    MirrorTimerAdvance {
        remaining_micros: 0,
        expired_ticks: ticks.min(MAX_CATCHUP_TICKS as i64) as u32,
    }
}

pub fn drowning_damage(max_health: u32) -> u32 {
    max_health.saturating_mul(20).saturating_add(99) / 100
}

pub fn exhaustion_damage(max_health: u32, level: u32, roll: u32) -> u32 {
    (max_health / 5).saturating_add(roll % level.max(1))
}

// Client fall_time_ms includes jump ascent; gravity is about 19.29 yd/s^2.
pub fn fall_damage(fall_time_ms: u32, max_health: u32) -> u32 {
    let t = fall_time_ms as f32 / 1000.0;
    let height_yd = 0.5 * 19.29 * t * t;
    let perc = 0.018 * height_yd - 0.2426;
    if perc <= 0.0 {
        return 0;
    }
    (max_health as f32 * perc.min(1.0)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_curve() {
        assert_eq!(fall_damage(0, 100), 0);
        assert_eq!(fall_damage(800, 100), 0);
        assert_eq!(fall_damage(1_180, 100), 0);
        let d15 = fall_damage(1_500, 100);
        assert!((10..25).contains(&d15), "{d15}");
        assert_eq!(fall_damage(10_000, 100), 100);
    }

    #[test]
    fn submerged_requires_swimming_liquid_and_head_below_surface() {
        let surface = 10.0;
        let swimming = SWIMMING;

        assert!(!is_submerged(8.0, surface, true, swimming));
        assert!(is_submerged(7.99, surface, true, swimming));
        assert!(!is_submerged(8.01, surface, true, swimming));
        assert!(!is_submerged(0.0, surface, false, swimming));
        assert!(!is_submerged(0.0, surface, true, 0));
    }

    #[test]
    fn deep_water_counts_below_and_just_above_the_surface_but_not_on_a_transport() {
        assert!(is_in_deep_water(-30.0, Some(0.0), SWIMMING));
        assert!(is_in_deep_water(2.0, Some(0.0), 0));
        assert!(!is_in_deep_water(2.01, Some(0.0), 0));
        assert!(!is_in_deep_water(-1.0, None, SWIMMING));
        assert!(!is_in_deep_water(-1.0, Some(0.0), ON_TRANSPORT));
    }

    #[test]
    fn fatigue_drains_for_a_minute_then_exhausts_each_second() {
        assert_eq!(
            advance_fatigue(FULL_FATIGUE_MICROS, true, 59_000_000),
            MirrorTimerAdvance {
                remaining_micros: 1_000_000,
                expired_ticks: 0
            }
        );
        assert_eq!(
            advance_fatigue(1_000_000, true, 3_000_000),
            MirrorTimerAdvance {
                remaining_micros: 0,
                expired_ticks: 3
            }
        );
    }

    #[test]
    fn fatigue_recovers_in_six_seconds_outside_deep_water() {
        assert_eq!(
            advance_fatigue(0, false, 3_000_000).remaining_micros,
            30_000_000
        );
        assert_eq!(
            advance_fatigue(0, false, 6_000_000).remaining_micros,
            FULL_FATIGUE_MICROS
        );
    }

    #[test]
    fn exhaustion_damage_is_a_fifth_of_health_plus_a_level_roll() {
        assert_eq!(exhaustion_damage(1000, 60, 0), 200);
        assert_eq!(exhaustion_damage(1000, 60, 59), 259);
        assert_eq!(exhaustion_damage(1000, 60, 60), 200);
        assert_eq!(exhaustion_damage(1000, 1, 7), 200);
    }

    #[test]
    fn breath_state_machine_drains_refills_and_bounds_catchup() {
        assert_eq!(
            advance_breath(FULL_AIR_MICROS, true, 12_000_000),
            MirrorTimerAdvance {
                remaining_micros: 48_000_000,
                expired_ticks: 0
            }
        );
        assert_eq!(
            advance_breath(200_000, true, 300_000),
            MirrorTimerAdvance {
                remaining_micros: 0,
                expired_ticks: 1
            }
        );
        assert_eq!(
            advance_breath(0, true, 20_000_000),
            MirrorTimerAdvance {
                remaining_micros: 0,
                expired_ticks: MAX_CATCHUP_TICKS
            }
        );
        assert_eq!(
            advance_breath(54_000_000, false, 2_000_000),
            MirrorTimerAdvance {
                remaining_micros: FULL_AIR_MICROS,
                expired_ticks: 0
            }
        );
        assert_eq!(drowning_damage(100).saturating_mul(10).min(100), 100);
    }
}
