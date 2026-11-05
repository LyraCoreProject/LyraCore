const FLEE_HEALTH_PCT: u32 = 15;

const ASSIST_RADIUS: f32 = 10.0;

pub(crate) fn within_assist_radius(ax: f32, ay: f32, bx: f32, by: f32) -> bool {
    let (dx, dy) = (ax - bx, ay - by);
    dx * dx + dy * dy <= ASSIST_RADIUS * ASSIST_RADIUS
}

pub(crate) fn hp_pct_below(health: u32, max_health: u32, pct: u32) -> bool {
    health > 0 && (health as u64) * 100 < (max_health as u64) * (pct as u64)
}

pub(crate) fn should_flee(health: u32, max_health: u32) -> bool {
    hp_pct_below(health, max_health, FLEE_HEALTH_PCT)
}

pub(crate) fn flee_eligible(creature_type: u8) -> bool {
    creature_type == crate::spell::HUMANOID_TYPE
}

pub(crate) const ROUT_DURATION_MS: u32 = 10_000;

pub(crate) fn rout_window_open(now_ms: u32, rout_ends_ms: u32) -> bool {
    rout_ends_ms != 0 && now_ms < rout_ends_ms
}

pub(crate) fn may_start_rout(eligible: bool, rout_ends_ms: u32) -> bool {
    eligible && rout_ends_ms == 0
}

// Zero means never started; wrapping onto it would allow a second rout.
pub(crate) fn rout_close_ms(now_ms: u32) -> u32 {
    now_ms.wrapping_add(ROUT_DURATION_MS).max(1)
}

pub(crate) fn pursuit_deadline_ms(now_ms: u32) -> u32 {
    now_ms.wrapping_add(crate::combat::PURSUIT_WINDOW_MS).max(1)
}

pub(crate) fn beyond_combat_backstop(dist_sq: f32) -> bool {
    dist_sq > crate::combat::COMBAT_BACKSTOP_SQ
}

pub(crate) fn should_evade(now_ms: u32, pursuit_ends_ms: u32, target_dist_sq: f32) -> bool {
    pursuit_ends_ms != 0
        && now_ms >= pursuit_ends_ms
        && target_dist_sq > crate::combat::LEASH_RADIUS_SQ
}

pub(crate) fn wounded_slow_factor(health: u32, max_health: u32) -> f32 {
    if max_health == 0 {
        return 1.0;
    }
    let hp_pct = (health as f32 / max_health as f32) * 100.0;
    let capped = hp_pct.min(30.0);
    (1.0 - ((30.0 - capped) * 1.67) / 100.0).max(0.499)
}

const BASE_AGGRO_RADIUS: f32 = 20.0;
const MIN_AGGRO_RADIUS: f32 = 5.0;
const MAX_AGGRO_RADIUS: f32 = 45.0;
const GREY_LEVEL_DIFF: i32 = 8;

// Infinite x or y saturates the grid index outside normal active cells.
pub(crate) fn finite_point(x: f32, y: f32, z: f32) -> bool {
    x.is_finite() && y.is_finite() && z.is_finite()
}

pub(crate) fn is_aggro_candidate(dead: bool, godmode: bool) -> bool {
    !dead && !godmode
}

pub(crate) fn aggro_radius(
    creature_level: u32,
    player_level: u32,
    template_aggro_range: u32,
) -> f32 {
    if template_aggro_range > 0 {
        return template_aggro_range as f32;
    }
    let leveldiff = creature_level as i32 - player_level as i32;
    if -leveldiff >= GREY_LEVEL_DIFF {
        return 0.0;
    }
    (BASE_AGGRO_RADIUS + leveldiff as f32).clamp(MIN_AGGRO_RADIUS, MAX_AGGRO_RADIUS)
}

pub(crate) const fn combat_active_radius(template_aggro_max: f32) -> f32 {
    let ceiling = if template_aggro_max > MAX_AGGRO_RADIUS {
        template_aggro_max
    } else {
        MAX_AGGRO_RADIUS
    };
    ceiling + ASSIST_RADIUS
}

pub(crate) fn aggro_override_cutoff(visibility_floor: f32) -> u32 {
    let cutoff = visibility_floor - ASSIST_RADIUS;
    if cutoff < 0.0 {
        return 0;
    }
    cutoff.floor() as u32 + 1
}

const STEALTH_DETECT_BASE: f32 = 5.0;
const STEALTH_DETECT_PER_LEVEL: f32 = 1.0;
const STEALTH_DETECT_MAX: f32 = 30.0;

pub(crate) fn stealth_detect_range(creature_level: u32, target_level: u32) -> f32 {
    let advantage = (creature_level as i32 - target_level as i32).max(0) as f32;
    (STEALTH_DETECT_BASE + advantage * STEALTH_DETECT_PER_LEVEL).min(STEALTH_DETECT_MAX)
}

pub(crate) fn flee_step(cx: f32, cy: f32, tx: f32, ty: f32, dist: f32) -> (f32, f32) {
    let (dx, dy) = (cx - tx, cy - ty);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return (cx + dist, cy);
    }
    (cx + dx / len * dist, cy + dy / len * dist)
}

pub(crate) const FEAR_STEP: f32 = 15.0;

// Keep jitter below 90 degrees so every fear step moves away from the source.
const FEAR_JITTER: f32 = std::f32::consts::FRAC_PI_3;

pub(crate) fn feared_flee_step(
    cx: f32,
    cy: f32,
    sx: f32,
    sy: f32,
    dist: f32,
    rand: u32,
) -> (f32, f32) {
    let (dx, dy) = (cx - sx, cy - sy);
    let base = if dx == 0.0 && dy == 0.0 {
        0.0
    } else {
        dy.atan2(dx)
    };
    let jitter = ((rand as f32 / u32::MAX as f32) * 2.0 - 1.0) * FEAR_JITTER;
    let a = base + jitter;
    (cx + a.cos() * dist, cy + a.sin() * dist)
}

pub(crate) const CHASE_MELEE_SQ: f32 = crate::combat::MELEE_RANGE_SQ;

pub(crate) const CHASE_LEASH_SQ: f32 = combat_active_radius(0.0) * combat_active_radius(0.0);

pub(crate) const CHASE_REPATH_COS: f32 = 0.9;

pub(crate) const FLEE_LEG_YD: f32 = 28.0;

pub(crate) fn spline_t(now_us: u64, start_micros: u64, dur_ms: u32) -> f32 {
    let dur_us = (dur_ms as u64) * 1000;
    if dur_us == 0 {
        return 1.0;
    }
    (now_us.saturating_sub(start_micros) as f32 / dur_us as f32).clamp(0.0, 1.0)
}

pub(crate) fn chase_step(
    cx: f32,
    cy: f32,
    tx: f32,
    ty: f32,
    max_step: f32,
    stop_dist: f32,
) -> (f32, f32) {
    let (dx, dy) = (tx - cx, ty - cy);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= stop_dist {
        return (cx, cy);
    }
    let travel = (len - stop_dist).min(max_step);
    (cx + dx / len * travel, cy + dy / len * travel)
}

pub(crate) const RETURN_LEASH_SQ: f32 = 225.0;

pub(crate) const WANDER_RADIUS: f32 = 6.0;

pub(crate) const WANDER_CHANCE_PCT: u32 = 35;

pub(crate) fn wander_point(
    home_x: f32,
    home_y: f32,
    rand_a: u32,
    rand_b: u32,
    radius: f32,
) -> (f32, f32) {
    let angle = (rand_a as f32 / u32::MAX as f32) * std::f32::consts::TAU;
    let r = (rand_b as f32 / u32::MAX as f32) * radius;
    (home_x + r * angle.cos(), home_y + r * angle.sin())
}

pub(crate) const MOVE_TICK_SECS: f32 = 0.5;

pub(crate) const MOVE_TICK_MICROS: i64 = 500_000;

pub(crate) const SENSE_EVERY_N_TICKS: i64 = 8;

pub(crate) const SENSE_PERIOD_MICROS: i64 = MOVE_TICK_MICROS * SENSE_EVERY_N_TICKS;

pub(crate) fn is_sense_tick_for_interval(now_micros: i64, interval_micros: i64) -> bool {
    let interval = if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    };
    let every_n = (SENSE_PERIOD_MICROS / interval).max(1);
    (now_micros / interval) % every_n == 0
}

pub(crate) fn tick_secs_for_interval(interval_micros: i64) -> f32 {
    let interval = if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    };
    interval as f32 / 1_000_000.0
}

pub(crate) fn fear_step_for_tick(tick_secs: f32) -> f32 {
    FEAR_STEP * (tick_secs / MOVE_TICK_SECS)
}

pub(crate) fn sense_period_secs_for_interval(interval_micros: i64) -> f32 {
    let interval = if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    };
    let every_n = (SENSE_PERIOD_MICROS / interval).max(1);
    (every_n * interval) as f32 / 1_000_000.0
}

pub(crate) const GLOBAL_TICK_INSTANCE: u64 = u64::MAX;

pub(crate) const TICK_SLICES: u32 = 4;

/// Each firing runs one slice, so every creature still runs once per `MOVE_TICK_MICROS`.
pub(crate) const SLICED_TICK_MICROS: i64 = MOVE_TICK_MICROS / TICK_SLICES as i64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TickSlice {
    pub index: u32,
    pub count: u32,
}

impl TickSlice {
    pub(crate) const WHOLE: TickSlice = TickSlice { index: 0, count: 1 };

    pub(crate) fn of_tick(tick: u64) -> Self {
        TickSlice {
            index: (tick % u64::from(TICK_SLICES)) as u32,
            count: TICK_SLICES,
        }
    }

    pub(crate) fn owns(self, guid: u64) -> bool {
        self.count <= 1 || guid % u64::from(self.count) == u64::from(self.index)
    }
}

pub(crate) enum TickScope {
    CatchAll {
        dedicated: std::collections::HashSet<u64>,
        slice: TickSlice,
    },
    Only(u64),
}

impl TickScope {
    #[cfg(test)]
    pub(crate) fn from_rows(
        row_instance_id: u64,
        all_row_instance_ids: impl IntoIterator<Item = u64>,
    ) -> Self {
        Self::sliced(row_instance_id, all_row_instance_ids, TickSlice::WHOLE)
    }

    pub(crate) fn sliced(
        row_instance_id: u64,
        all_row_instance_ids: impl IntoIterator<Item = u64>,
        slice: TickSlice,
    ) -> Self {
        if row_instance_id == GLOBAL_TICK_INSTANCE {
            TickScope::CatchAll {
                dedicated: all_row_instance_ids
                    .into_iter()
                    .filter(|&i| i != GLOBAL_TICK_INSTANCE)
                    .collect(),
                slice,
            }
        } else {
            TickScope::Only(row_instance_id)
        }
    }

    pub(crate) fn covers(&self, instance_id: u64) -> bool {
        match self {
            TickScope::CatchAll { dedicated, .. } => !dedicated.contains(&instance_id),
            TickScope::Only(n) => instance_id == *n,
        }
    }

    /// Whether this tick runs the creature: its instance is covered and its guid is in the slice.
    pub(crate) fn owns(&self, guid: u64, instance_id: u64) -> bool {
        self.covers(instance_id) && self.slice().owns(guid)
    }

    pub(crate) fn slice(&self) -> TickSlice {
        match self {
            TickScope::CatchAll { slice, .. } => *slice,
            TickScope::Only(_) => TickSlice::WHOLE,
        }
    }

    pub(crate) fn runs_global_passes(&self) -> bool {
        matches!(self, TickScope::CatchAll { slice, .. } if slice.index == 0)
    }
}

pub(crate) fn leg_in_flight(now_ms: u32, leg_ends_ms: u32) -> bool {
    now_ms < leg_ends_ms
}

fn leg_duration_ms(from: (f32, f32), to: (f32, f32), speed: f32) -> u32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    ((dx * dx + dy * dy).sqrt() / speed * 1000.0) as u32
}

pub(crate) fn leg_toward(from: (f32, f32), to: (f32, f32), speed: f32) -> Option<(f32, f32, u32)> {
    if to == from {
        return None;
    }
    Some((to.0, to.1, leg_duration_ms(from, to, speed)))
}

pub(crate) fn next_waypoint_idx(cur_idx: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (cur_idx + 1) % len
    }
}

pub(crate) fn nearest_waypoint_idx(cx: f32, cy: f32, pts: &[(f32, f32)]) -> usize {
    pts.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (a.0 - cx).powi(2) + (a.1 - cy).powi(2);
            let db = (b.0 - cx).powi(2) + (b.1 - cy).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::constants;

    use crate::creatures::{
        rank_hp_multiplier_permille, rolled_creature_stats, scale_creature_damage_for_level,
        scale_health_for_rank,
    };

    #[test]
    fn creature_damage_scales_from_template_to_live_level_without_overflow() {
        assert_eq!(scale_creature_damage_for_level(5, 10, 10, 10), (5, 10));
        assert_eq!(scale_creature_damage_for_level(5, 10, 10, 12), (6, 12));
        assert_eq!(
            scale_creature_damage_for_level(u32::MAX, u32::MAX, 1, 60),
            (u32::MAX, u32::MAX)
        );
    }

    #[test]
    fn hp_pct_below_threshold_and_flee_delegation() {
        assert!(hp_pct_below(49, 100, 50));
        assert!(!hp_pct_below(50, 100, 50));
        assert!(!hp_pct_below(80, 100, 50));
        assert!(!hp_pct_below(0, 100, 50));
        assert!(should_flee(14, 100));
        assert!(!should_flee(15, 100));
        assert!(!should_flee(0, 100));
    }

    #[test]
    fn hp_pct_below_widens_products_past_u32_overflow_boundary() {
        const LAST_SAFE_TIMES_100: u32 = u32::MAX / 100;
        assert!(!hp_pct_below(
            LAST_SAFE_TIMES_100,
            LAST_SAFE_TIMES_100,
            FLEE_HEALTH_PCT,
        ));
        assert!(!hp_pct_below(
            LAST_SAFE_TIMES_100 + 1,
            LAST_SAFE_TIMES_100 + 1,
            FLEE_HEALTH_PCT,
        ));

        assert!(!hp_pct_below(u32::MAX, u32::MAX, FLEE_HEALTH_PCT));
        assert!(hp_pct_below(u32::MAX / 10, u32::MAX, FLEE_HEALTH_PCT));
    }

    #[test]
    fn aggro_radius_scales_with_level_and_greys_out() {
        assert_eq!(aggro_radius(10, 10, 0), BASE_AGGRO_RADIUS);
        assert_eq!(aggro_radius(13, 10, 0), 23.0);
        assert_eq!(aggro_radius(99, 10, 0), MAX_AGGRO_RADIUS);
        assert_eq!(aggro_radius(8, 14, 0), 14.0);
        assert_eq!(aggro_radius(8, 15, 0), 13.0);
        assert_eq!(aggro_radius(1, 1 + GREY_LEVEL_DIFF as u32, 0), 0.0);
        assert_eq!(aggro_radius(1, 60, 0), 0.0);
        assert_eq!(aggro_radius(1, 60, 8), 8.0);
        assert_eq!(aggro_radius(10, 10, 12), 12.0);
    }

    #[test]
    fn combat_active_radius_covers_the_vanilla_ceiling_and_scales_with_template_overrides() {
        assert_eq!(combat_active_radius(0.0), MAX_AGGRO_RADIUS + ASSIST_RADIUS);
        assert_eq!(combat_active_radius(8.0), MAX_AGGRO_RADIUS + ASSIST_RADIUS);
        assert_eq!(combat_active_radius(100.0), 100.0 + ASSIST_RADIUS);
    }

    #[test]
    fn aggro_override_cutoff_is_exactly_the_overrides_that_can_beat_the_visibility_floor() {
        let floor = lyracore_shared::spatial::BOX_HALF_SPAN as f32
            * lyracore_shared::spatial::GRID_CELL_SIZE;
        let cutoff = aggro_override_cutoff(floor);
        assert_eq!(
            cutoff, 91,
            "100yd floor − 10yd assist margin → overrides of 91+ matter"
        );
        for t in 0..200u32 {
            let beats_floor = combat_active_radius(t as f32) > floor;
            assert_eq!(
                beats_floor,
                t >= cutoff,
                "override {t}: cutoff must agree with the real radius comparison"
            );
        }
        assert_eq!(aggro_override_cutoff(ASSIST_RADIUS - 1.0), 0);
    }

    #[test]
    fn a_creature_displaced_far_from_home_steps_all_the_way_back() {
        let (home_x, home_y) = (-100.0_f32, 250.0_f32);
        let step = constants::speeds::RUN * MOVE_TICK_SECS;
        let start = 300.0_f32;
        let dist_sq = |p: (f32, f32)| (p.0 - home_x).powi(2) + (p.1 - home_y).powi(2);
        let mut pos = (home_x + start, home_y);
        let mut prev_sq = dist_sq(pos);
        let mut ticks = 0u32;
        while dist_sq(pos) > RETURN_LEASH_SQ {
            let next = chase_step(pos.0, pos.1, home_x, home_y, step, 0.0);
            assert_ne!(next, pos, "return stalled after {ticks} ticks");
            pos = next;
            let now_sq = dist_sq(pos);
            assert!(now_sq < prev_sq, "step {ticks} did not close the gap");
            prev_sq = now_sq;
            ticks += 1;
            assert!(ticks < 10_000, "no arrival from {start} yd out");
        }
        assert_eq!(ticks, 82, "(300 - 15) yd at 3.5 yd per tick");
    }

    #[test]
    fn stealth_detect_range_grows_with_level_advantage_and_clamps() {
        assert_eq!(stealth_detect_range(10, 10), 5.0);
        assert_eq!(stealth_detect_range(20, 10), 15.0);
        assert_eq!(stealth_detect_range(11, 10), 6.0);
        assert_eq!(stealth_detect_range(5, 10), 5.0);
        assert_eq!(stealth_detect_range(1, 60), 5.0);
        assert_eq!(stealth_detect_range(99, 1), STEALTH_DETECT_MAX);
        assert_eq!(stealth_detect_range(10, 10), MIN_AGGRO_RADIUS);
    }

    #[test]
    fn level_roll_respects_range_and_interpolates_health() {
        assert_eq!(rolled_creature_stats(3, 0, 42, 0, 12345), (3, 42));
        assert_eq!(rolled_creature_stats(3, 3, 42, 60, 999), (3, 42));
        for rand in 0..50u32 {
            let (lvl, _) = rolled_creature_stats(1, 3, 40, 80, rand);
            assert!(
                (1..=3).contains(&lvl),
                "level {lvl} out of [1,3] for rand {rand}"
            );
        }
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 0).0, 1);
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 2).0, 3);
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 0), (1, 40));
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 1), (2, 60));
        assert_eq!(rolled_creature_stats(1, 3, 40, 80, 2), (3, 80));
        assert_eq!(rolled_creature_stats(1, 3, 50, 0, 2), (3, 50));
    }

    #[test]
    fn rank_scales_health_and_rank0_is_identity() {
        for base in [0u32, 1, 42, 1000, 250_000, u32::MAX] {
            assert_eq!(
                scale_health_for_rank(base, 0),
                base,
                "rank-0 must be identity for base {base}"
            );
        }
        assert_eq!(scale_health_for_rank(100, 1), 150);
        assert_eq!(scale_health_for_rank(100, 2), 280);
        assert_eq!(scale_health_for_rank(100, 3), 400);
        assert_eq!(scale_health_for_rank(100, 4), 180);
        assert_eq!(scale_health_for_rank(42, 1), 63);
        assert_eq!(scale_health_for_rank(100, 7), 100);
        assert_eq!(scale_health_for_rank(100, 255), 100);
        assert_eq!(rank_hp_multiplier_permille(0), 1000);
        assert!(rank_hp_multiplier_permille(1) > rank_hp_multiplier_permille(0));
        assert!(rank_hp_multiplier_permille(3) > rank_hp_multiplier_permille(1));
        assert_eq!(scale_health_for_rank(u32::MAX, 1), u32::MAX);
        assert_eq!(scale_health_for_rank(u32::MAX, 2), u32::MAX);
        assert_eq!(scale_health_for_rank(u32::MAX, 3), u32::MAX);
        assert_eq!(scale_health_for_rank(u32::MAX, 4), u32::MAX);
        assert_eq!(scale_health_for_rank(2_000_000_000, 1), 3_000_000_000);
        assert_eq!(scale_health_for_rank(1_000_000_000, 2), 2_800_000_000);
        assert_eq!(scale_health_for_rank(1_000_000_000, 3), 4_000_000_000);
        assert_eq!(scale_health_for_rank(2_000_000_000, 4), 3_600_000_000);
    }

    #[test]
    fn flee_threshold_is_alive_and_low_health() {
        assert!(should_flee(5, 42));
        assert!(!should_flee(10, 42));
        assert!(!should_flee(0, 42));
        assert!(!should_flee(42, 42));
    }

    #[test]
    fn flee_eligible_is_humanoid_only() {
        assert!(
            flee_eligible(7),
            "humanoid (Defias militiaman) routs to call for help"
        );
        assert!(
            !flee_eligible(1),
            "BEAST (wolf/boar) fights to the death, never routs"
        );
        assert!(
            !flee_eligible(8),
            "critter (chicken) is not flee-eligible either"
        );
        assert!(
            !flee_eligible(0),
            "no/unknown type → not eligible (safe default)"
        );
    }

    #[test]
    fn flee_step_runs_away_from_target() {
        let (x, y) = flee_step(10.0, 0.0, 0.0, 0.0, 12.0);
        assert!((x - 22.0).abs() < 0.01, "x {x} expected ~22.0");
        assert!((y - 0.0).abs() < 0.01, "y {y} expected ~0.0");

        let (x, y) = flee_step(5.0, 5.0, 5.0, 5.0, 12.0);
        assert!(
            x.is_finite() && y.is_finite(),
            "fallback must be finite, got ({x},{y})"
        );
        let d = ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt();
        assert!(
            (d - 12.0).abs() < 0.01,
            "fallback distance {d} expected ~12.0"
        );
    }

    #[test]
    fn feared_flee_step_moves_away_from_source() {
        let (x, y) = feared_flee_step(10.0, 0.0, 0.0, 0.0, 12.0, u32::MAX / 2);
        assert!(
            (x - 22.0).abs() < 0.05,
            "zero-jitter leg goes straight away on +x, got x={x}"
        );
        assert!(
            y.abs() < 0.05,
            "zero-jitter leg stays on the away axis, got y={y}"
        );
    }

    #[test]
    fn feared_flee_step_always_nets_away() {
        let (cx, sx) = (10.0_f32, 0.0_f32);
        for r in [
            0u32,
            u32::MAX / 4,
            u32::MAX / 2,
            (u32::MAX / 4) * 3,
            u32::MAX,
        ] {
            let (x, _y) = feared_flee_step(cx, 0.0, sx, 0.0, 12.0, r);
            assert!(
                x > cx,
                "jitter roll {r}: must still net away from the source, got x={x}"
            );
        }
    }

    #[test]
    fn feared_flee_step_coincident_is_finite() {
        let (x, y) = feared_flee_step(5.0, 5.0, 5.0, 5.0, 12.0, 0);
        assert!(
            x.is_finite() && y.is_finite(),
            "coincident fallback must be finite, got ({x},{y})"
        );
        let d = ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt();
        assert!(
            (d - 12.0).abs() < 0.05,
            "coincident leg distance {d} expected ~12.0 (still moves)"
        );
    }

    #[test]
    fn chase_step_closes_to_just_inside_melee() {
        let (x, y) = chase_step(0.0, 0.0, 20.0, 0.0, constants::speeds::RUN * 4.0, 4.0);
        assert!(
            (x - 16.0).abs() < 0.01,
            "x {x} expected ~16.0 (4 yd short of the target)"
        );
        assert!(
            (y - 0.0).abs() < 0.01,
            "y {y} expected ~0.0 (straight-line approach)"
        );

        let (x, y) = chase_step(0.0, 0.0, 100.0, 0.0, constants::speeds::RUN * 4.0, 4.0);
        assert!(
            (x - 28.0).abs() < 0.01,
            "x {x} expected ~28.0 (one tick of run, capped)"
        );
        assert!((y - 0.0).abs() < 0.01, "y {y} expected ~0.0");

        let (x, y) = chase_step(0.0, 0.0, 3.0, 0.0, 28.0, 4.0);
        assert_eq!((x, y), (0.0, 0.0), "within stop_dist must hold position");

        let (x, y) = chase_step(0.0, 0.0, 4.0, 0.0, 28.0, 4.0);
        assert_eq!(
            (x, y),
            (0.0, 0.0),
            "at the stop_dist boundary must hold position"
        );

        let (x, y) = chase_step(5.0, 5.0, 5.0, 5.0, 28.0, 4.0);
        assert_eq!(
            (x, y),
            (5.0, 5.0),
            "coincident must hold position, never NaN"
        );
        assert!(x.is_finite() && y.is_finite());

        let (x, y) = chase_step(0.0, 0.0, 30.0, 40.0, 100.0, 5.0);
        assert!((x - 27.0).abs() < 0.01, "x {x} expected ~27.0");
        assert!((y - 36.0).abs() < 0.01, "y {y} expected ~36.0");
    }

    #[test]
    fn wander_point_stays_within_radius_of_home() {
        let (hx, hy) = (-100.0_f32, 250.0_f32);
        let radius = WANDER_RADIUS;
        let eps = 1e-3;
        let rolls = [
            (0u32, 0u32),
            (u32::MAX, u32::MAX),
            (u32::MAX / 2, u32::MAX / 4),
            (1, u32::MAX),
            (u32::MAX, 1),
            (123_456_789, 987_654_321),
            (0xDEAD_BEEF, 0x1234_5678),
            (u32::MAX / 3, u32::MAX / 7),
        ];
        for (ra, rb) in rolls {
            let (px, py) = wander_point(hx, hy, ra, rb, radius);
            let d = ((px - hx).powi(2) + (py - hy).powi(2)).sqrt();
            assert!(
                d <= radius + eps,
                "wander dist {d} exceeds radius {radius} for rolls ({ra},{rb}) → ({px},{py})"
            );
            assert!(
                px.is_finite() && py.is_finite(),
                "non-finite point for rolls ({ra},{rb})"
            );
        }
        let (px, py) = wander_point(hx, hy, 9999, 0, radius);
        assert!(
            (px - hx).abs() < eps && (py - hy).abs() < eps,
            "rand_b 0 must land on home"
        );
        let (px, py) = wander_point(hx, hy, 0, u32::MAX, radius);
        let d = ((px - hx).powi(2) + (py - hy).powi(2)).sqrt();
        assert!(
            (d - radius).abs() < eps,
            "max rand_b should sit on the rim, dist {d} vs {radius}"
        );
    }

    #[test]
    fn assist_radius_is_inclusive_boundary() {
        assert!(within_assist_radius(0.0, 0.0, 7.0, 0.0));
        assert!(within_assist_radius(0.0, 0.0, 10.0, 0.0));
        assert!(!within_assist_radius(0.0, 0.0, 12.0, 0.0));
        assert!(within_assist_radius(3.0, 4.0, 3.0, 4.0));
    }

    #[test]
    fn wounded_slow_factor_curve() {
        let approx = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(approx(wounded_slow_factor(30, 100), 1.0));
        assert!(approx(wounded_slow_factor(100, 100), 1.0));
        assert!(approx(wounded_slow_factor(15, 100), 0.7495));
        assert!(approx(wounded_slow_factor(10, 100), 0.666));
        assert!(approx(wounded_slow_factor(1, 100), 0.5157));
        assert!(wounded_slow_factor(14, 100) < 1.0);
        assert!(wounded_slow_factor(1, 100) >= 0.499);
        assert!(approx(wounded_slow_factor(0, 0), 1.0));
    }

    #[test]
    fn finite_point_rejects_every_non_finite_coordinate() {
        assert!(finite_point(-8949.9, -132.4, 83.5));
        assert!(!finite_point(f32::INFINITY, 0.0, 0.0));
        assert!(!finite_point(0.0, f32::NEG_INFINITY, 0.0));
        assert!(!finite_point(0.0, 0.0, f32::NAN));
        assert!(!finite_point(f32::NAN, f32::NAN, f32::NAN));
    }

    #[test]
    fn godmode_and_dead_players_are_not_aggro_candidates() {
        assert!(
            is_aggro_candidate(false, false),
            "an ordinary live player IS a candidate"
        );
        assert!(!is_aggro_candidate(true, false), "a corpse is not");
        assert!(!is_aggro_candidate(false, true), "a godmoded GM is not");
        assert!(!is_aggro_candidate(true, true));
    }

    #[test]
    fn spline_t_interpolates_and_clamps() {
        let approx = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(approx(spline_t(0, 0, 1000), 0.0));
        assert!(approx(spline_t(250_000, 0, 1000), 0.25));
        assert!(approx(spline_t(500_000, 0, 1000), 0.5));
        assert!(approx(spline_t(1_000_000, 0, 1000), 1.0));
        assert!(approx(spline_t(2_000_000, 0, 1000), 1.0));
        assert!(approx(spline_t(0, 500_000, 1000), 0.0));
        assert!(approx(spline_t(0, 0, 0), 1.0));
        let t = spline_t(250_000, 0, 1000);
        assert!(approx(0.0 + (10.0 - 0.0) * t, 2.5));
    }

    #[test]
    fn chase_step_at_tick_scale_glues_to_a_fleeing_target() {
        let run = constants::speeds::RUN * MOVE_TICK_SECS;
        let (x, _) = chase_step(0.0, 0.0, 20.0, 0.0, run, 4.0);
        assert!((x - 3.5).abs() < 0.01, "x {x} expected ~3.5 (RUN*0.5 step)");
        let (mut cx, mut tx) = (0.0f32, 20.0f32);
        for _ in 0..20 {
            cx = chase_step(cx, 0.0, tx, 0.0, run, 4.0).0;
            tx += 0.5;
        }
        assert!(
            tx - cx <= 5.0,
            "should stay glued within ~one leg, gap {}",
            tx - cx
        );
    }

    #[test]
    fn tick_scope_dedicated_row_covers_exactly_its_instance() {
        let rows = [GLOBAL_TICK_INSTANCE, 7, 9];
        let scope = TickScope::from_rows(7, rows);
        assert!(scope.covers(7), "a dedicated row covers its own instance");
        assert!(!scope.covers(0), "…and never the open world");
        assert!(!scope.covers(9), "…and never a SIBLING dedicated instance");
        assert!(
            !scope.covers(3),
            "…and never an unrelated instance (that's the catch-all's)"
        );
        assert!(
            !scope.runs_global_passes(),
            "due-time passes (regen/decay/respawn) run only on the catch-all firing"
        );
    }

    #[test]
    fn tick_scope_catch_all_skips_dedicated_rows_and_covers_everything_else() {
        let rows = [GLOBAL_TICK_INSTANCE, 7, 9];
        let scope = TickScope::from_rows(GLOBAL_TICK_INSTANCE, rows);
        assert!(
            scope.covers(0),
            "the catch-all covers instance 0 (the open world)"
        );

        assert!(
            scope.covers(3),
            "an instance with no row of its own is covered by the catch-all"
        );
        assert!(
            scope.covers(u64::MAX - 1),
            "…even a pathological huge instance id"
        );
        assert!(
            !scope.covers(7),
            "the catch-all SKIPS an instance with a dedicated row"
        );
        assert!(!scope.covers(9));
        assert!(scope.runs_global_passes());
    }

    #[test]
    fn tick_slices_run_every_creature_once_per_cycle_and_global_passes_once() {
        let rows = [GLOBAL_TICK_INSTANCE];
        let scopes: Vec<TickScope> = (0..u64::from(TICK_SLICES))
            .map(|tick| TickScope::sliced(GLOBAL_TICK_INSTANCE, rows, TickSlice::of_tick(tick)))
            .collect();
        for guid in [0u64, 1, 2, 3, 0xF130_0003_3601_3B72, u64::MAX] {
            let owners = scopes.iter().filter(|s| s.owns(guid, 0)).count();
            assert_eq!(owners, 1, "guid {guid:#x} must run in exactly one slice");
        }
        let global = scopes.iter().filter(|s| s.runs_global_passes()).count();
        assert_eq!(global, 1, "world-wide passes run once per cycle");
        assert_eq!(
            SLICED_TICK_MICROS * i64::from(TICK_SLICES),
            MOVE_TICK_MICROS
        );
    }

    #[test]
    fn tick_scope_partitions_every_instance_exactly_once() {
        let rows = [GLOBAL_TICK_INSTANCE, 7, 9];
        let scopes: Vec<TickScope> = rows
            .iter()
            .map(|&r| TickScope::from_rows(r, rows))
            .collect();
        for instance in [0u64, 1, 3, 7, 9, 42, u64::MAX - 1] {
            let covering = scopes.iter().filter(|s| s.covers(instance)).count();
            assert_eq!(
                covering, 1,
                "instance {instance} must be covered by exactly one scope"
            );
        }
    }

    #[test]
    fn tick_scope_default_config_visits_the_identical_candidate_set() {
        let scope = TickScope::from_rows(GLOBAL_TICK_INSTANCE, [GLOBAL_TICK_INSTANCE]);
        let candidates: Vec<(u64, u64)> = (0..50).map(|g| (g, 0u64)).collect();
        let visited: Vec<(u64, u64)> = candidates
            .iter()
            .copied()
            .filter(|&(_, inst)| scope.covers(inst))
            .collect();
        assert_eq!(
            visited, candidates,
            "default config must not drop a single candidate"
        );
        assert!(
            scope.runs_global_passes(),
            "…and the global passes still run every firing"
        );
    }

    #[test]
    fn sense_tick_for_interval_fires_on_the_documented_boundary_at_default_cadence() {
        for now in [0, 499_999, 4_000_000, 4_499_999, 8_000_000] {
            assert!(is_sense_tick_for_interval(now, 500_000), "{now}");
        }
        for now in [500_000, 1_234_567, 3_999_999, 4_500_000, 7_999_999] {
            assert!(!is_sense_tick_for_interval(now, 500_000), "{now}");
        }
    }

    #[test]
    fn sense_tick_for_interval_keeps_the_4s_cadence_for_fast_and_slow_rows() {
        let hits = (0..40i64)
            .filter(|t| is_sense_tick_for_interval(t * 100_000, 100_000))
            .count();
        assert_eq!(hits, 1, "a 100ms row senses once per ~4s, not per firing");
        assert!(is_sense_tick_for_interval(10_000_000, 10_000_000));
        assert!(is_sense_tick_for_interval(20_000_000, 10_000_000));
        assert_eq!(
            is_sense_tick_for_interval(0, 0),
            is_sense_tick_for_interval(0, MOVE_TICK_MICROS)
        );
        assert_eq!(
            is_sense_tick_for_interval(4_000_000, -5),
            is_sense_tick_for_interval(4_000_000, MOVE_TICK_MICROS)
        );
    }

    #[test]
    fn tick_secs_for_interval_is_exact_at_default_and_scales_linearly() {
        assert_eq!(tick_secs_for_interval(MOVE_TICK_MICROS), MOVE_TICK_SECS);
        assert_eq!(tick_secs_for_interval(100_000), 0.1);
        assert_eq!(tick_secs_for_interval(1_000_000), 1.0);
        let run = constants::speeds::RUN;
        let default_net = (1.0 / MOVE_TICK_SECS) * (run * tick_secs_for_interval(MOVE_TICK_MICROS));
        let fast_net = (1.0 / 0.1) * (run * tick_secs_for_interval(100_000));
        assert!(
            (default_net - fast_net).abs() < 1e-4,
            "net yd/s must be cadence-invariant"
        );
        assert_eq!(tick_secs_for_interval(0), MOVE_TICK_SECS);
        assert_eq!(tick_secs_for_interval(-1), MOVE_TICK_SECS);
    }

    #[test]
    fn fear_step_scales_with_tick_and_is_identity_at_default() {
        assert_eq!(
            fear_step_for_tick(MOVE_TICK_SECS),
            FEAR_STEP,
            "default tick → verbatim FEAR_STEP"
        );
        assert!(
            (fear_step_for_tick(0.1) - FEAR_STEP * 0.2).abs() < 1e-5,
            "100ms tick → 1/5 the dash"
        );
    }

    #[test]
    fn sense_period_matches_actual_sense_tick_spacing() {
        let spacing_micros = |interval: i64| {
            let mut firings = (0..).map(|n| n * interval);
            firings
                .find(|&now| now > 0 && is_sense_tick_for_interval(now, interval))
                .expect("every cadence senses again")
        };
        for (interval, period_secs) in [
            (500_000, 4.0),
            (100_000, 4.0),
            (200_000, 4.0),
            (2_500_000, 2.5),
            (3_000_000, 3.0),
            (10_000_000, 10.0),
        ] {
            assert_eq!(sense_period_secs_for_interval(interval), period_secs);
            assert_eq!(spacing_micros(interval) as f32 / 1_000_000.0, period_secs);
        }
        assert!((sense_period_secs_for_interval(707_000) - 3.535).abs() < 1e-4);
        assert_eq!(sense_period_secs_for_interval(0), 4.0);
    }

    #[test]
    fn sense_tick_is_one_in_eight() {
        let hits: Vec<bool> = (0..16)
            .map(|t| is_sense_tick_for_interval(t * MOVE_TICK_MICROS, MOVE_TICK_MICROS))
            .collect();
        assert_eq!(
            hits.iter().filter(|&&h| h).count(),
            2,
            "2 sense ticks in 16 (1-in-8)"
        );
        assert!(hits[0] && hits[8], "ticks 0 and 8 are sense ticks");
        assert!(!hits[1] && !hits[7], "the 7 between are movement-only");
    }

    #[test]
    fn leg_in_flight_gate() {
        assert!(leg_in_flight(100, 600));
        assert!(!leg_in_flight(600, 600));
        assert!(!leg_in_flight(700, 600));
        assert!(!leg_in_flight(100, 0));
    }

    #[test]
    fn rout_window_open_covers_the_whole_clock_lifecycle() {
        assert!(!rout_window_open(0, 0));
        assert!(!rout_window_open(50_000, 0));
        assert!(rout_window_open(50_000, 60_000));
        assert!(!rout_window_open(60_000, 60_000));
        assert!(!rout_window_open(600_000, 60_000));
    }

    #[test]
    fn a_rout_starts_once_per_engagement_and_only_when_eligible() {
        assert!(may_start_rout(true, 0));
        assert!(!may_start_rout(true, 60_000));
        assert!(!may_start_rout(false, 0));
        assert!(!may_start_rout(false, 60_000));
    }

    #[test]
    fn a_rout_window_never_stamps_the_never_started_sentinel() {
        assert_eq!(rout_close_ms(50_000), 50_000 + ROUT_DURATION_MS);
        assert_eq!(rout_close_ms(u32::MAX - ROUT_DURATION_MS + 1), 1);
    }

    #[test]
    fn evade_needs_both_an_expired_deadline_and_a_target_that_left_the_leash() {
        let near = crate::combat::LEASH_RADIUS_SQ - 1.0;
        let far = crate::combat::LEASH_RADIUS_SQ + 1.0;
        assert!(!should_evade(50_000, 60_000, near));
        assert!(!should_evade(50_000, 60_000, far));
        assert!(!should_evade(60_000, 60_000, near));
        assert!(should_evade(60_000, 60_000, far));
    }

    #[test]
    fn the_backstop_is_the_only_raw_distance_that_still_ends_a_fight() {
        let backstop = crate::combat::COMBAT_BACKSTOP_SQ;
        assert!(!beyond_combat_backstop(backstop));
        assert!(beyond_combat_backstop(backstop + 1.0));
        assert!(!should_evade(50_000, 60_000, backstop - 1.0));
    }

    #[test]
    fn a_pursuit_deadline_never_stamps_the_never_refreshed_sentinel() {
        assert_eq!(
            pursuit_deadline_ms(50_000),
            50_000 + crate::combat::PURSUIT_WINDOW_MS
        );
        assert_eq!(
            pursuit_deadline_ms(u32::MAX - crate::combat::PURSUIT_WINDOW_MS + 1),
            1
        );
        assert!(!should_evade(
            50_000,
            pursuit_deadline_ms(50_000),
            f32::INFINITY
        ));
    }

    #[test]
    fn the_leash_constants_hold_their_invariants() {
        const {
            assert!(ROUT_DURATION_MS < crate::combat::PURSUIT_WINDOW_MS);
            assert!(crate::combat::LEASH_RADIUS_SQ < crate::combat::COMBAT_BACKSTOP_SQ);
            assert!(crate::combat::RANGED_RANGE_SQ < crate::combat::COMBAT_BACKSTOP_SQ);
        }
    }

    #[test]
    fn waypoint_cursor_traverses_in_order() {
        assert_eq!(next_waypoint_idx(0, 4), 1);
        assert_eq!(next_waypoint_idx(2, 4), 3);
        assert_eq!(next_waypoint_idx(3, 4), 0);
        let pts = [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0), (30.0, 0.0)];
        assert_eq!(nearest_waypoint_idx(19.0, 0.0, &pts), 2);
        assert_eq!(nearest_waypoint_idx(-5.0, 0.0, &pts), 0);
        let mut idx = nearest_waypoint_idx(0.0, 0.0, &pts);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..4 {
            seen.insert(idx);
            idx = next_waypoint_idx(idx, pts.len());
        }
        assert_eq!(seen.len(), 4, "ordered traversal visits all waypoints");
    }
}
