use lyracore_shared::spatial;
use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table};

use crate::{
    game_creature_ai_state, game_melee_attack, game_sessionless_action_consent, game_world_entity,
    WorldEntity,
};

use super::*;

pub(crate) mod cadence;
mod lifecycle;

pub(crate) use lifecycle::{pass_decay, pass_gameobject_respawn, pass_respawn};

pub(crate) use lifecycle::despawn_creature_entity;

#[table(
    accessor = game_creature_spline,
    public,
    index(accessor = by_instance, btree(columns = [instance_id])),
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    // AOI subscriptions require three equality columns in SpacetimeDB 2.14.0.
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct CreatureSpline {
    #[primary_key]
    pub guid: u64,
    pub start_micros: u64,
    pub dur_ms: u32,
    pub sx: f32,
    pub sy: f32,
    pub sz: f32,
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
    // The client keys interpolation by spline id; it must change for every replacement leg.
    #[default(0u32)]
    pub spline_id: u32,
    #[default(false)]
    pub run: bool,
    #[default(0i64)]
    pub cell: i64,
    #[default(false)]
    pub facing: bool,
    #[default(0.0f32)]
    pub facing_angle: f32,
    #[default(None::<CreaturePath>)]
    pub path: Option<CreaturePath>,
    // The victim a back-off steps away from; 0 on every other leg.
    #[default(0u64)]
    pub back_off_from: u64,
}

impl CreatureSpline {
    #[cfg(feature = "debug_reducers")]
    pub(crate) fn repair_cell(mut self, ctx: &ReducerContext) {
        self.cell = lyracore_shared::spatial::grid_cell_id(self.grid_x, self.grid_y);
        // Repairing an index must not restart the client's current spline.
        ctx.db.game_creature_spline().guid().update(self);
    }

    pub(crate) fn replace(self, ctx: &ReducerContext) {
        let table = ctx.db.game_creature_spline();
        let row = if table.guid().find(self.guid).is_some() {
            table.guid().update(self)
        } else {
            table.insert(self)
        };
        crate::native::creature_spline_changed(ctx, &row);
    }
}

#[table(accessor = game_creature_move_schedule, scheduled(tick_creatures))]
pub struct CreatureMoveSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[default(18_446_744_073_709_551_615u64)]
    pub instance_id: u64,
}

/// Replaces the catch-all creature tick with one that fires every `interval`.
pub(crate) fn arm_creature_tick(ctx: &ReducerContext, interval: spacetimedb::TimeDuration) {
    let schedules = ctx.db.game_creature_move_schedule();
    let catch_all: Vec<u64> = schedules
        .iter()
        .filter(|row| row.instance_id == cadence::GLOBAL_TICK_INSTANCE)
        .map(|row| row.scheduled_id)
        .collect();
    for scheduled_id in catch_all {
        schedules.scheduled_id().delete(scheduled_id);
    }
    schedules.insert(CreatureMoveSchedule {
        scheduled_id: 0,
        scheduled_at: crate::time::schedule_interval(ctx, interval),
        instance_id: cadence::GLOBAL_TICK_INSTANCE,
    });
}

#[reducer]
pub fn tick_creatures(ctx: &ReducerContext, schedule: CreatureMoveSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    creature_pass(ctx, schedule);
}

#[table(accessor = game_creature_tick_cursor)]
pub struct CreatureTickCursor {
    #[primary_key]
    pub id: u8,
    pub tick: u64,
}

fn next_tick(ctx: &ReducerContext) -> u64 {
    let cursors = ctx.db.game_creature_tick_cursor();
    match cursors.id().find(0) {
        Some(mut cursor) => {
            let tick = cursor.tick;
            cursor.tick = tick.wrapping_add(1);
            cursors.id().update(cursor);
            tick
        }
        None => {
            cursors.insert(CreatureTickCursor { id: 0, tick: 1 });
            0
        }
    }
}

fn creature_pass(ctx: &ReducerContext, schedule: CreatureMoveSchedule) {
    let interval_micros = match &schedule.scheduled_at {
        ScheduleAt::Interval(d) => d.to_micros(),
        ScheduleAt::Time(_) => MOVE_TICK_MICROS,
    };
    // A catch-all row armed at any other interval runs the whole world every firing.
    let sliced = schedule.instance_id == crate::creatures::GLOBAL_TICK_INSTANCE
        && interval_micros == crate::creatures::SLICED_TICK_MICROS;
    let tick_number = sliced.then(|| next_tick(ctx));
    let slice = tick_number.map_or(TickSlice::WHOLE, TickSlice::of_tick);
    let scope = TickScope::sliced(
        schedule.instance_id,
        ctx.db
            .game_creature_move_schedule()
            .iter()
            .map(|r| r.instance_id),
        slice,
    );
    let period_micros = interval_micros * i64::from(slice.count);
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let sense = match tick_number {
        Some(tick) => {
            let every_n = (cadence::SENSE_PERIOD_MICROS / period_micros).max(1) as u64;
            (tick / u64::from(slice.count)) % every_n == 0
        }
        None => is_sense_tick_for_interval(now_micros, interval_micros),
    };
    let tick = crate::creatures::cycle::TickContext {
        now_micros: now_micros as u64,
        now_ms: (now_micros / 1000) as u32,
        tick_secs: tick_secs_for_interval(period_micros),
        sense,
        sense_secs: sense_period_secs_for_interval(period_micros),
        scope,
    };
    let (sense, scope_label) = (tick.sense, scope_label(&tick.scope));

    let outcome = crate::creatures::cycle::run(ctx, tick);

    log_pass_stats(
        ctx,
        &scope_label,
        sense,
        outcome.awake,
        &outcome.rows_visited,
        interval_micros,
    );
}

const ACTIVE_CELL_LOG_PERIOD_MICROS: i64 = 60_000_000;

fn active_cell_radius(ctx: &ReducerContext) -> f32 {
    let visible = spatial::BOX_HALF_SPAN as f32 * spatial::GRID_CELL_SIZE;

    let templates = ctx.db.game_creature_template();
    let cutoff = crate::creatures::ai::aggro_override_cutoff(visible);
    let template_aggro_max = templates
        .by_aggro_range()
        .filter(cutoff..)
        .map(|t| t.aggro_range as f32)
        .fold(0.0_f32, f32::max);
    combat_active_radius(template_aggro_max).max(visible)
}

pub(crate) fn active_cell_creatures(
    ctx: &ReducerContext,
    scope: &TickScope,
    sense: bool,
) -> TickSweep {
    let entities = ctx.db.game_world_entity();
    let consents = ctx.db.game_sessionless_action_consent();
    let package_controlled = |guid: u64| consents.character_guid().find(guid).is_some();
    let now_ms = move_clock_ms(ctx);
    let radius = active_cell_radius(ctx);
    let mut awake = std::collections::HashSet::new();
    let mut pets: Vec<u64> = Vec::new();
    let mut in_combat: Vec<u64> = Vec::new();

    if sense {
        pets.extend(
            entities
                .by_owner_guid()
                .filter(1u64..)
                .filter(|entity| scope.owns(entity.guid, entity.instance_id))
                .map(|entity| entity.guid),
        );
        if scope.runs_global_passes() {
            in_combat.extend(
                entities
                    .by_combat_active()
                    .filter(true)
                    .map(|entity| entity.guid),
            );
        }
    }

    let players: Vec<WorldEntity> = entities
        .by_entry()
        .filter(&0u32)
        .filter(|e| e.is_player() && scope.covers(e.instance_id))
        .filter(|e| !is_idle_bot(e, now_ms, || package_controlled(e.guid)))
        .collect();
    for state in ctx
        .db
        .game_creature_ai_state()
        .by_active_object()
        .filter(&true)
    {
        let Some(e) = entities.guid().find(state.creature_guid) else {
            continue;
        };
        if active_object_enters_scope(e.is_player(), scope.covers(e.instance_id), true) {
            awake.insert(e.guid);
        }
    }
    let cells = active_cells(
        players
            .into_iter()
            .map(|p| (p.map_id, p.instance_id, p.x, p.y)),
        radius,
    );
    for cell in cells {
        for creature in entities.by_grid().filter(cell) {
            if !creature.is_player() {
                awake.insert(creature.guid);
            }
        }
    }
    let slice = scope.slice();
    TickSweep {
        active: awake.iter().copied().filter(|&g| slice.owns(g)).collect(),
        awake,
        pets,
        in_combat,
    }
}

fn active_object_enters_scope(is_player: bool, partition_covered: bool, active: bool) -> bool {
    !is_player && partition_covered && active
}

const IDLE_BOT_AFTER_MS: u32 = 30_000;

fn is_idle_bot(
    character: &WorldEntity,
    now_ms: u32,
    package_controlled: impl FnOnce() -> bool,
) -> bool {
    character.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT == 0
        && now_ms.wrapping_sub(character.last_move_ms) >= IDLE_BOT_AFTER_MS
        && package_controlled()
}

#[derive(Default)]
pub(crate) struct TickSweep {
    /// The awake creatures this tick's slice runs.
    pub(crate) active: std::collections::HashSet<u64>,
    /// Every awake creature in scope, whichever slice runs it; a neighbour can answer any call.
    pub(crate) awake: std::collections::HashSet<u64>,
    pub(crate) pets: Vec<u64>,
    pub(crate) in_combat: Vec<u64>,
}

fn scope_label(scope: &TickScope) -> String {
    match scope {
        TickScope::CatchAll { dedicated, slice } if slice.count > 1 => {
            format!(
                "global(slice {}/{}, skipping {} dedicated)",
                slice.index,
                slice.count,
                dedicated.len()
            )
        }
        TickScope::CatchAll { dedicated, .. } => {
            format!("global(skipping {} dedicated)", dedicated.len())
        }
        TickScope::Only(n) => format!("instance {n}"),
    }
}

fn log_pass_stats(
    ctx: &ReducerContext,
    scope_label: &str,
    sense: bool,
    awake: usize,
    rows_visited: &[(&'static str, u64)],
    interval_micros: i64,
) {
    let us = crate::time::now(ctx).to_micros_since_unix_epoch();
    if us.rem_euclid(ACTIVE_CELL_LOG_PERIOD_MICROS) >= interval_micros.max(MOVE_TICK_MICROS) {
        return;
    }
    let body = rows_visited
        .iter()
        .map(|(pass, rows)| format!("{pass}={rows}"))
        .collect::<Vec<_>>()
        .join(" ");
    log::info!(
        "tick_creatures pass rows-visited: scope={scope_label} sense={sense} awake={awake} {body} (*=global pass)"
    );
}

#[derive(spacetimedb::SpacetimeType, Clone)]
pub struct CreaturePath {
    pub points: Vec<CreaturePathPoint>,
    pub navigation: crate::nav::NavigationInputs,
}

#[derive(spacetimedb::SpacetimeType, Clone)]
pub struct CreaturePathPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// The Realm Clock in milliseconds, truncated to the u32 width of the creature move clock columns.
pub(crate) fn move_clock_ms(ctx: &ReducerContext) -> u32 {
    crate::time::now_ms(ctx) as u32
}

pub(crate) fn advance_needs_persist(
    stored: &WorldEntity,
    advanced: &WorldEntity,
    settled: &WorldEntity,
    leg: Option<&CreatureSpline>,
    now_micros: u64,
) -> bool {
    let max_drift = crate::world::PERSIST_MAX_DRIFT_YD;
    let drift_sq = (advanced.x - stored.x).powi(2)
        + (advanced.y - stored.y).powi(2)
        + (advanced.z - stored.z).powi(2);
    let same_leg_in_flight = leg.is_some_and(|leg| leg.start_micros < now_micros);
    !same_leg_in_flight
        || settled != advanced
        || stored.is_player()
        || stored.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
        || (stored.grid_x, stored.grid_y) != (advanced.grid_x, advanced.grid_y)
        || stored.orientation != advanced.orientation
        || drift_sq > max_drift * max_drift
}

pub(crate) fn movable_creature(
    ctx: &ReducerContext,
    guid: u64,
    scope: &TickScope,
) -> Option<WorldEntity> {
    let c = ctx.db.game_world_entity().guid().find(guid)?;
    if c.is_player() || c.dead || !scope.owns(c.guid, c.instance_id) {
        return None;
    }
    Some(c)
}

pub(crate) fn rout_eligible(ctx: &ReducerContext, c: &WorldEntity) -> bool {
    should_flee(c.health, c.max_health)
        && ctx
            .db
            .game_creature_template()
            .entry()
            .find(c.entry)
            .is_some_and(|t| flee_eligible(t.creature_type))
}

pub(crate) fn creature_is_routing(ctx: &ReducerContext, c: &WorldEntity) -> bool {
    let now_ms = move_clock_ms(ctx);
    ctx.db
        .game_melee_attack()
        .attacker_guid()
        .find(c.guid)
        .is_some_and(|row| rout_window_open(now_ms, row.rout_ends_ms))
        && (rout_eligible(ctx, c) || crate::creatures::eventai::authored_combat(ctx, c.guid).flee)
        && !crate::spell::is_self_movement_suppressed(ctx, c.guid)
}

fn active_cells(
    positions: impl IntoIterator<Item = (u32, u64, f32, f32)>,
    radius: f32,
) -> std::collections::BTreeSet<(u32, u64, i32, i32)> {
    let mut cells = std::collections::BTreeSet::new();
    for (map, instance, x, y) in positions {
        let (gx0, gx1, gy0, gy1) = spatial::covering_cell_box(x, y, radius);
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                cells.insert((map, instance, gx, gy));
            }
        }
    }
    cells
}

#[cfg(test)]
mod relay_tripwire {
    #[test]
    fn overlapping_characters_read_each_active_cell_once() {
        let cells = super::active_cells([(0, 0, 0.0, 0.0); 1_000], 100.0);
        assert_eq!(cells.len(), 25);
        assert!(cells.contains(&(0, 0, 339, 339)));
        assert!(cells.contains(&(0, 0, 343, 343)));
        assert!(!cells.contains(&(0, 0, 344, 343)));
    }

    #[test]
    fn active_cells_keep_maps_and_instances_separate() {
        let cells = super::active_cells(
            [(0, 0, 0.0, 0.0), (1, 0, 0.0, 0.0), (0, 7, 0.0, 0.0)],
            100.0,
        );
        assert_eq!(cells.len(), 75);
        for partition in [(0, 0), (1, 0), (0, 7)] {
            assert!(cells.contains(&(partition.0, partition.1, 339, 339)));
        }
        assert!(!cells.contains(&(1, 7, 339, 339)));
    }

    #[test]
    fn adjacent_neighborhoods_preserve_their_outer_cells() {
        let cells = super::active_cells([(0, 0, 0.0, 0.0), (0, 0, 50.0, 0.0)], 100.0);
        assert_eq!(cells.len(), 30);
        assert!(cells.contains(&(0, 0, 338, 339)));
        assert!(cells.contains(&(0, 0, 343, 343)));
    }

    #[test]
    fn active_objects_enter_only_their_creature_partition_sweep() {
        assert!(super::active_object_enters_scope(false, true, true));
        assert!(!super::active_object_enters_scope(true, true, true));
        assert!(!super::active_object_enters_scope(false, false, true));
        assert!(!super::active_object_enters_scope(false, true, false));
    }
}

#[cfg(test)]
mod advance_persist_gate {
    use super::{advance_needs_persist, CreatureSpline};
    use crate::WorldEntity;
    use lyracore_shared::constants::{type_mask, unit_flags};
    use lyracore_shared::spatial;

    const NOW_MICROS: u64 = 60_000_000;
    const NOW_MS: u32 = 60_000;

    fn creature_at(x: f32, moved_ms: u32) -> WorldEntity {
        let (gx, gy) = spatial::grid_cell(x, 0.0);
        let mut creature = crate::helpers::tests::entity(7, 0, 0, gx, gy);
        creature.x = x;
        creature.last_move_ms = moved_ms;
        creature
    }

    fn leg_started_at(start_micros: u64, dur_ms: u32) -> CreatureSpline {
        CreatureSpline {
            guid: 7,
            start_micros,
            dur_ms,
            sx: 0.0,
            sy: 0.0,
            sz: 0.0,
            dx: 20.0,
            dy: 0.0,
            dz: 0.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            spline_id: (start_micros / 1000) as u32,
            run: false,
            facing: false,
            facing_angle: 0.0,
            path: None,
            back_off_from: 0,
        }
    }

    struct Walk {
        stored: WorldEntity,
        advanced: WorldEntity,
        settled: WorldEntity,
        leg: Option<CreatureSpline>,
    }

    impl Walk {
        fn between(from_x: f32, to_x: f32) -> Self {
            Walk {
                stored: creature_at(from_x, NOW_MS - 1_000),
                advanced: creature_at(to_x, NOW_MS),
                settled: creature_at(to_x, NOW_MS),
                leg: Some(leg_started_at(NOW_MICROS - 1_000_000, 8_000)),
            }
        }

        fn rows(&mut self) -> [&mut WorldEntity; 3] {
            [&mut self.stored, &mut self.advanced, &mut self.settled]
        }

        fn persists(&self) -> bool {
            advance_needs_persist(
                &self.stored,
                &self.advanced,
                &self.settled,
                self.leg.as_ref(),
                NOW_MICROS,
            )
        }
    }

    #[test]
    fn a_walk_within_four_yards_leaves_the_stored_row_behind() {
        assert!(!Walk::between(1.0, 4.0).persists());
        assert!(!Walk::between(1.0, 5.0).persists(), "exactly four yards");
    }

    #[test]
    fn a_walk_past_four_yards_is_written() {
        assert!(Walk::between(1.0, 5.5).persists());
    }

    #[test]
    fn a_step_into_another_cell_is_written() {
        let walk = Walk::between(15.0, 18.0);
        assert_ne!(
            walk.stored.grid_x, walk.advanced.grid_x,
            "x=16.67 is a cell edge"
        );
        assert!(walk.persists());
    }

    #[test]
    fn arrival_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.leg = None;
        assert!(walk.persists());
    }

    #[test]
    fn a_leg_started_or_halted_this_firing_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.leg = Some(leg_started_at(NOW_MICROS, 3_000));
        assert!(walk.persists(), "a new leg");
        walk.leg = Some(leg_started_at(NOW_MICROS, 0));
        assert!(walk.persists(), "a halt");
    }

    #[test]
    fn another_write_to_the_row_this_firing_keeps_the_advance() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.settled.wp_target = 3;
        assert!(walk.persists());
    }

    #[test]
    fn a_turn_along_the_path_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.advanced.orientation = 1.5;
        walk.settled.orientation = 1.5;
        assert!(walk.persists());
    }

    #[test]
    fn a_creature_in_combat_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        for row in walk.rows() {
            row.unit_flags = unit_flags::IN_COMBAT;
        }
        assert!(walk.persists());
    }

    #[test]
    fn a_character_on_a_leg_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        for row in walk.rows() {
            row.type_mask = type_mask::PLAYER_BIT;
        }
        assert!(walk.persists());
    }
}

#[cfg(test)]
mod idle_bot {
    use super::is_idle_bot;
    use crate::WorldEntity;
    use lyracore_shared::constants::{type_mask, unit_flags};

    const NOW_MS: u32 = 3_600_000;

    fn character_still_for(still_ms: u32) -> WorldEntity {
        let mut character = crate::helpers::tests::entity(42, 0, 0, 0, 0);
        character.type_mask = type_mask::PLAYER_BIT;
        character.last_move_ms = NOW_MS - still_ms;
        character
    }

    #[test]
    fn a_package_controlled_character_still_for_thirty_seconds_is_an_idle_bot() {
        assert!(is_idle_bot(&character_still_for(30_000), NOW_MS, || true));
        assert!(is_idle_bot(&character_still_for(600_000), NOW_MS, || true));
    }

    #[test]
    fn a_character_without_consent_is_never_an_idle_bot() {
        let human = character_still_for(600_000);
        assert!(!is_idle_bot(&human, NOW_MS, || false));
    }

    #[test]
    fn a_bot_in_combat_is_not_idle() {
        let mut bot = character_still_for(600_000);
        bot.unit_flags = unit_flags::IN_COMBAT;
        assert!(!is_idle_bot(&bot, NOW_MS, || true));
    }

    #[test]
    fn a_bot_that_moved_within_thirty_seconds_is_not_idle() {
        assert!(!is_idle_bot(&character_still_for(29_999), NOW_MS, || true));
        let mut across_the_wrap = character_still_for(0);
        across_the_wrap.last_move_ms = u32::MAX - 4_999;
        assert!(!is_idle_bot(&across_the_wrap, 5_000, || true));
    }
}
