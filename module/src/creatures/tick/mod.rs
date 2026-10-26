use lyracore_shared::spatial;
use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::world::MoveClock;
use crate::{
    game_creature_ai_state, game_entity_motion, game_melee_attack, game_sessionless_action_consent,
    game_world_entity, WorldEntity,
};

use super::*;

mod lifecycle;

pub(crate) use lifecycle::{pass_decay, pass_gameobject_respawn, pass_respawn};

pub(crate) use lifecycle::despawn_creature_entity;

#[table(accessor = game_creature_move_event, public)]
pub struct CreatureMoveEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub mover_guid: u64,
    pub start_x: f32,
    pub start_y: f32,
    pub start_z: f32,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub duration_ms: u32,
    pub spline_id: u32,
    pub created_at: Timestamp,
    #[default(false)]
    pub run: bool,
}

#[table(
    accessor = game_creature_spline,
    public,
    index(accessor = by_instance, btree(columns = [instance_id])),
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    // AOI subscriptions require three equality columns in SpacetimeDB 2.7.1.
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
}

impl CreatureSpline {
    #[cfg(feature = "debug_reducers")]
    pub(crate) fn repair_cell(mut self, ctx: &ReducerContext) {
        self.cell = lyracore_shared::spatial::grid_cell_id(self.grid_x, self.grid_y);
        // Repairing an index must not restart the client's current spline.
        ctx.db.game_creature_spline().guid().update(self);
    }

    fn replace(self, ctx: &ReducerContext) {
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

#[reducer]
pub fn tick_creatures(ctx: &ReducerContext, schedule: CreatureMoveSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let scope = TickScope::from_rows(
        schedule.instance_id,
        ctx.db
            .game_creature_move_schedule()
            .iter()
            .map(|r| r.instance_id),
    );
    let interval_micros = match &schedule.scheduled_at {
        ScheduleAt::Interval(d) => d.to_micros(),
        ScheduleAt::Time(_) => MOVE_TICK_MICROS,
    };
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let tick = crate::creatures::cycle::TickContext {
        now_micros: now_micros as u64,
        now_ms: (now_micros / 1000) as u32,
        tick_secs: tick_secs_for_interval(interval_micros),
        sense: is_sense_tick_for_interval(now_micros, interval_micros),
        sense_secs: sense_period_secs_for_interval(interval_micros),
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
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let radius = active_cell_radius(ctx);
    let mut out = std::collections::HashSet::new();
    let mut pets: Vec<u64> = Vec::new();
    let mut in_combat: Vec<u64> = Vec::new();

    if sense {
        pets.extend(
            entities
                .by_owner_guid()
                .filter(1u64..)
                .filter(|entity| scope.covers(entity.instance_id))
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
            out.insert(e.guid);
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
                out.insert(creature.guid);
            }
        }
    }
    TickSweep {
        active: out,
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
    pub(crate) active: std::collections::HashSet<u64>,
    pub(crate) pets: Vec<u64>,
    pub(crate) in_combat: Vec<u64>,
}

fn scope_label(scope: &TickScope) -> String {
    match scope {
        TickScope::CatchAll { dedicated } => {
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
    let us = ctx.timestamp.to_micros_since_unix_epoch();
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_move_spline(
    ctx: &ReducerContext,
    guid: u64,
    start: (f32, f32, f32),
    dest: (f32, f32, f32),
    dur_ms: u32,
    run: bool,
    spline_id: u32,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
    path: Option<CreaturePath>,
) {
    let row = CreatureSpline {
        guid,
        start_micros: ctx.timestamp.to_micros_since_unix_epoch() as u64,
        dur_ms,
        sx: start.0,
        sy: start.1,
        sz: start.2,
        dx: dest.0,
        dy: dest.1,
        dz: dest.2,
        map_id,
        instance_id,
        grid_x: grid.0,
        grid_y: grid.1,
        cell: lyracore_shared::spatial::grid_cell_id(grid.0, grid.1),
        spline_id,
        run,
        facing: false,
        facing_angle: 0.0,
        path,
    };
    row.replace(ctx);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_facing_spline(
    ctx: &ReducerContext,
    guid: u64,
    pos: (f32, f32, f32),
    angle_rad: f32,
    spline_id: u32,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
) {
    let row = CreatureSpline {
        guid,
        start_micros: ctx.timestamp.to_micros_since_unix_epoch() as u64,
        dur_ms: 0,
        sx: pos.0,
        sy: pos.1,
        sz: pos.2,
        dx: pos.0,
        dy: pos.1,
        dz: pos.2,
        map_id,
        instance_id,
        grid_x: grid.0,
        grid_y: grid.1,
        cell: lyracore_shared::spatial::grid_cell_id(grid.0, grid.1),
        spline_id,
        run: false,
        facing: true,
        facing_angle: angle_rad,
        path: None,
    };
    row.replace(ctx);
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

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_creature_path(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    points: Vec<(f32, f32, f32)>,
    run: bool,
) {
    use lyracore_shared::{constants::speeds, movement_path};
    let Some(&destination) = points.last() else {
        return;
    };
    let spline_id = begin_leg(ctx, &mut mover);
    let start = (mover.x, mover.y, mover.z);
    let length = movement_path::length(start, &points);
    if !length.is_finite() {
        return;
    }
    if length <= 0.0 {
        stop_where_rendered(ctx, &mut mover);
        mover.update(ctx);
        return;
    }
    let speed = if run { speeds::RUN } else { speeds::WALK };
    let duration = (length / speed * 1000.0).ceil().max(1.0) as u32;
    let path = CreaturePath {
        navigation: crate::nav::inputs(ctx, mover.map_id),
        points: points
            .iter()
            .map(|&(x, y, z)| CreaturePathPoint { x, y, z })
            .collect(),
    };
    emit_move_spline(
        ctx,
        mover.guid,
        start,
        destination,
        duration,
        run,
        spline_id,
        mover.map_id,
        mover.instance_id,
        (mover.grid_x, mover.grid_y),
        Some(path),
    );
    mover.orientation = (points[0].1 - mover.y).atan2(points[0].0 - mover.x);
    mover.last_move_ms = now_ms(ctx);
    mover.update(ctx);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_creature_leg(
    ctx: &ReducerContext,
    mut e: WorldEntity,
    to: (f32, f32),
    z_fallback: f32,
    duration_ms: u32,
    run: bool,
    now_ms: u32,
    set_leg_ends: bool,
) {
    let nz = crate::terrain::snap_z(ctx, e.map_id, e.instance_id, to.0, to.1, z_fallback);
    if !crate::creatures::ai::finite_point(to.0, to.1, nz) {
        spacetimedb::log::error!(
            "refused a non-finite leg for guid {} -> ({}, {}, {}) — creature left at its last good \
             position; this means some movement maths produced inf/NaN upstream",
            e.guid, to.0, to.1, nz
        );
        return;
    }
    emit_move_spline(
        ctx,
        e.guid,
        (e.x, e.y, e.z),
        (to.0, to.1, nz),
        duration_ms.max(1),
        run,
        now_ms,
        e.map_id,
        e.instance_id,
        (e.grid_x, e.grid_y),
        None,
    );
    // A normal spline leaves the client facing along the leg.
    if to != (e.x, e.y) {
        e.orientation = (to.1 - e.y).atan2(to.0 - e.x);
    }
    e.last_move_ms = now_ms;
    if set_leg_ends {
        e.leg_ends_ms = now_ms + duration_ms;
    }
    e.update(ctx);
}

pub(crate) fn stop_where_rendered(ctx: &ReducerContext, mover: &mut WorldEntity) {
    let Some(previous) = place_where_rendered(ctx, mover) else {
        return;
    };
    let spline_id = next_spline_id(now_ms(ctx), previous);
    let at = (mover.x, mover.y, mover.z);
    emit_move_spline(
        ctx,
        mover.guid,
        at,
        at,
        0,
        false,
        spline_id,
        mover.map_id,
        mover.instance_id,
        (mover.grid_x, mover.grid_y),
        None,
    );
}

pub(crate) fn stop_facing(ctx: &ReducerContext, mover: &mut WorldEntity, point: (f32, f32)) {
    stop_where_rendered(ctx, mover);
    mover.leg_ends_ms = 0;
    mover.orientation = (point.1 - mover.y).atan2(point.0 - mover.x);
    let replaced = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(mover.guid)
        .map_or(0, |leg| leg.spline_id);
    emit_facing_spline(
        ctx,
        mover.guid,
        (mover.x, mover.y, mover.z),
        mover.orientation,
        next_spline_id(now_ms(ctx), replaced),
        mover.map_id,
        mover.instance_id,
        (mover.grid_x, mover.grid_y),
    );
}

pub(crate) fn begin_leg(ctx: &ReducerContext, mover: &mut WorldEntity) -> u32 {
    let now_ms = now_ms(ctx);
    place_where_rendered(ctx, mover).map_or(now_ms, |previous| next_spline_id(now_ms, previous))
}

fn place_where_rendered(ctx: &ReducerContext, mover: &mut WorldEntity) -> Option<u32> {
    let leg = ctx.db.game_creature_spline().guid().find(mover.guid)?;
    let spline_id = leg.spline_id;
    if let Some(stop) = super::cycle::stop_on_stored_leg(ctx, leg) {
        place_stopped(mover, stop, MoveClock::Unchanged);
    }
    Some(spline_id)
}

// The client ignores a replacement id that does not exceed the previous spline id.
pub(crate) fn next_spline_id(now_ms: u32, previous: u32) -> u32 {
    now_ms.max(previous.wrapping_add(1))
}

pub(crate) fn now_ms(ctx: &ReducerContext) -> u32 {
    (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32
}

pub(crate) fn place_stopped(mover: &mut WorldEntity, stop: super::cycle::Stop, clock: MoveClock) {
    mover
        .place(stop.at.x, stop.at.y, stop.at.z, clock)
        .unexplored();
    if let Some(heading) = stop.heading {
        mover.orientation = heading;
    }
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
    if c.is_player() || c.dead || !scope.covers(c.instance_id) {
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
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
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
mod stop_between_firings {
    use super::{next_spline_id, place_stopped};
    use crate::creatures::cycle::{Point, Stop};
    use crate::world::MoveClock;
    use crate::WorldEntity;
    use lyracore_shared::spatial;

    fn creature_stored_at(x: f32) -> WorldEntity {
        let (grid_x, grid_y) = spatial::grid_cell(x, 0.0);
        let mut creature = crate::helpers::tests::entity(42, 0, 0, grid_x, grid_y);
        creature.x = x;
        creature.cell = spatial::grid_cell_id(grid_x, grid_y);
        creature
    }

    #[test]
    fn a_stop_past_a_cell_edge_moves_the_creature_into_the_next_cell() {
        let mut creature = creature_stored_at(15.0);
        let before = (creature.grid_x, creature.cell);
        place_stopped(
            &mut creature,
            Stop {
                at: Point {
                    x: 20.0,
                    y: 0.0,
                    z: 0.0,
                },
                heading: Some(1.5),
            },
            MoveClock::Unchanged,
        );
        let (grid_x, grid_y) = spatial::grid_cell(20.0, 0.0);
        assert_ne!(before.0, grid_x, "the stop must cross the cell edge");
        assert_eq!((creature.x, creature.y, creature.z), (20.0, 0.0, 0.0));
        assert_eq!((creature.grid_x, creature.grid_y), (grid_x, grid_y));
        assert_eq!(creature.cell, spatial::grid_cell_id(grid_x, grid_y));
        assert_ne!(creature.cell, before.1);
        assert_eq!(creature.orientation, 1.5);
    }

    #[test]
    fn a_stop_without_a_heading_keeps_the_creatures_facing() {
        let mut creature = creature_stored_at(15.0);
        creature.orientation = 0.7;
        place_stopped(
            &mut creature,
            Stop {
                at: Point {
                    x: 15.0,
                    y: 0.0,
                    z: 0.0,
                },
                heading: None,
            },
            MoveClock::Unchanged,
        );
        assert_eq!(creature.orientation, 0.7);
    }

    #[test]
    fn a_stop_in_the_same_millisecond_as_its_leg_still_gets_a_newer_spline_id() {
        assert_eq!(next_spline_id(60_000, 60_000), 60_001);
        assert_eq!(next_spline_id(60_000, 59_000), 60_000);
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
    fn a_bot_on_a_movement_leg_is_not_idle() {
        assert!(!is_idle_bot(&character_still_for(500), NOW_MS, || true));
    }

    #[test]
    fn a_bot_that_moved_within_thirty_seconds_is_not_idle() {
        assert!(!is_idle_bot(&character_still_for(29_999), NOW_MS, || true));
        let mut across_the_wrap = character_still_for(0);
        across_the_wrap.last_move_ms = u32::MAX - 4_999;
        assert!(!is_idle_bot(&across_the_wrap, 5_000, || true));
    }
}
