pub(crate) const MOVE_TICK_MICROS: i64 = 500_000;

pub(crate) const MOVE_TICK_SECS: f32 = MOVE_TICK_MICROS as f32 / 1_000_000.0;

pub(crate) const SENSE_EVERY_N_TICKS: i64 = 8;

pub(crate) const SENSE_PERIOD_MICROS: i64 = MOVE_TICK_MICROS * SENSE_EVERY_N_TICKS;

// A schedule row without a positive interval runs at the default cadence.
fn interval_or_default(interval_micros: i64) -> i64 {
    if interval_micros > 0 {
        interval_micros
    } else {
        MOVE_TICK_MICROS
    }
}

pub(crate) fn is_sense_tick_for_interval(now_micros: i64, interval_micros: i64) -> bool {
    let interval = interval_or_default(interval_micros);
    let every_n = (SENSE_PERIOD_MICROS / interval).max(1);
    (now_micros / interval) % every_n == 0
}

pub(crate) fn tick_secs_for_interval(interval_micros: i64) -> f32 {
    let interval = interval_or_default(interval_micros);
    interval as f32 / 1_000_000.0
}

pub(crate) fn sense_period_secs_for_interval(interval_micros: i64) -> f32 {
    let interval = interval_or_default(interval_micros);
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

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::constants;

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
}
