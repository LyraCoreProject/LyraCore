use spacetimedb::ReducerContext;

use super::tick::{game_creature_spline, CreaturePath, CreaturePathPoint};
use crate::world::MoveClock;
use crate::{game_world_entity, CreatureSpline, WorldEntity};

pub(crate) struct Course {
    to: (f32, f32, f32),
    duration_ms: u32,
    run: bool,
    path: Option<CreaturePath>,
    back_off_from: u64,
    held_until_landed: bool,
}

impl Course {
    pub(crate) fn straight(to: (f32, f32, f32), duration_ms: u32, run: bool) -> Self {
        Course {
            to,
            duration_ms,
            run,
            path: None,
            back_off_from: 0,
            held_until_landed: false,
        }
    }

    /// A straight course whose destination snaps to the ground below `to`, or `z_fallback`.
    pub(crate) fn on_ground(
        ctx: &ReducerContext,
        mover: &WorldEntity,
        to: (f32, f32),
        z_fallback: f32,
        duration_ms: u32,
        run: bool,
    ) -> Self {
        let z =
            crate::terrain::snap_z(ctx, mover.map_id, mover.instance_id, to.0, to.1, z_fallback);
        Self::straight((to.0, to.1, z), duration_ms, run)
    }

    /// Stamps `leg_ends_ms`, so the creature's idle passes leave the leg alone until it lands.
    pub(crate) fn held_until_landed(mut self, held: bool) -> Self {
        self.held_until_landed = held;
        self
    }

    pub(crate) fn backing_off_from(mut self, victim: u64) -> Self {
        self.back_off_from = victim;
        self
    }
}

/// Sends the mover on `course` from its stored position and writes the mover.
pub(crate) fn start(ctx: &ReducerContext, mut mover: WorldEntity, course: Course) {
    if begin(ctx, &mut mover, course) {
        mover.update(ctx);
    }
}

/// Sends the mover on `course` from its stored position; the caller writes the mover. A course
/// with a non-finite destination is refused: false, and the mover is unchanged.
pub(crate) fn begin(ctx: &ReducerContext, mover: &mut WorldEntity, course: Course) -> bool {
    let (x, y, z) = course.to;
    if !super::ai::finite_point(x, y, z) {
        spacetimedb::log::error!(
            "refused a non-finite leg for guid {} -> ({x}, {y}, {z}); the creature stays at its \
             last good position",
            mover.guid
        );
        return false;
    }
    let now_ms = crate::time::now_ms_u32(ctx);
    let facing_point = course
        .path
        .as_ref()
        .and_then(|path| path.points.first())
        .map_or((x, y), |point| (point.x, point.y));
    CreatureSpline {
        run: course.run,
        path: course.path,
        back_off_from: course.back_off_from,
        ..Origin::of(mover).row(ctx, course.to, course.duration_ms.max(1))
    }
    .replace(ctx);
    // A normal spline leaves the client facing along the leg.
    if facing_point != (mover.x, mover.y) {
        mover.orientation = (facing_point.1 - mover.y).atan2(facing_point.0 - mover.x);
    }
    if let MoveClock::At(ms) = MoveClock::realm(ctx, mover, now_ms) {
        mover.last_move_ms = ms;
    }
    if course.held_until_landed {
        mover.leg_ends_ms = now_ms.wrapping_add(course.duration_ms);
    }
    true
}

/// Sends the mover along a Route Path from where the client draws it and writes the mover.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn start_path(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    points: Vec<(f32, f32, f32)>,
    run: bool,
) {
    use lyracore_shared::{constants::speeds, movement_path};
    let Some(&destination) = points.last() else {
        return;
    };
    place_where_rendered(ctx, &mut mover);
    let length = movement_path::length((mover.x, mover.y, mover.z), &points);
    if !length.is_finite() {
        return;
    }
    if length <= 0.0 {
        stop(ctx, &mut mover);
        mover.update(ctx);
        return;
    }
    let speed = if run { speeds::RUN } else { speeds::WALK };
    let duration_ms = (length / speed * 1000.0).ceil().max(1.0) as u32;
    let path = CreaturePath {
        navigation: crate::nav::inputs(ctx, mover.map_id),
        points: points
            .iter()
            .map(|&(x, y, z)| CreaturePathPoint { x, y, z })
            .collect(),
    };
    start(
        ctx,
        mover,
        Course {
            path: Some(path),
            ..Course::straight(destination, duration_ms, run)
        },
    );
}

/// Sends a living creature on one straight leg from where the client draws it to `(x, y, z)`.
pub(crate) fn move_to_point(
    ctx: &ReducerContext,
    creature_guid: u64,
    x: f32,
    y: f32,
    z: f32,
    run: bool,
) -> Result<(), String> {
    use lyracore_shared::constants::speeds;
    let Some(mut mover) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return Err(format!("no live entity {creature_guid}"));
    };
    if mover.is_player() {
        return Err("move_to_point targets creatures only".to_string());
    }
    if mover.dead {
        return Err("mover is dead".to_string());
    }
    let base = if run { speeds::RUN } else { speeds::WALK };
    let speed = crate::modifiers::effective_move_speed(ctx, creature_guid, base);
    if speed <= 0.0 {
        return Ok(());
    }
    place_where_rendered(ctx, &mut mover);
    let (dx, dy) = (x - mover.x, y - mover.y);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= f32::EPSILON {
        stop(ctx, &mut mover);
        mover.update(ctx);
        return Ok(());
    }
    let duration_ms = ((dist / speed) * 1000.0) as u32;
    start(ctx, mover, Course::straight((x, y, z), duration_ms, run));
    Ok(())
}

/// Ends the mover's leg where the client draws it. The caller writes the mover.
pub(crate) fn stop(ctx: &ReducerContext, mover: &mut WorldEntity) {
    if place_where_rendered(ctx, mover) {
        halt(ctx, mover);
    }
}

/// Stops the mover where the client draws it and turns it toward `point`. The caller writes the
/// mover.
pub(crate) fn stop_facing(ctx: &ReducerContext, mover: &mut WorldEntity, point: (f32, f32)) {
    stop(ctx, mover);
    mover.leg_ends_ms = 0;
    face(ctx, mover, (point.1 - mover.y).atan2(point.0 - mover.x));
}

/// Replaces a leg the mover has with a zero-length spline at its stored position.
pub(crate) fn halt(ctx: &ReducerContext, mover: &WorldEntity) {
    halt_at(
        ctx,
        mover.guid,
        mover.map_id,
        mover.instance_id,
        (mover.x, mover.y, mover.z),
    );
}

/// Replaces a leg the mover has with a zero-length spline at `at`, which may be in another
/// partition than the leg.
pub(crate) fn halt_at(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    instance_id: u64,
    at: (f32, f32, f32),
) {
    if ctx.db.game_creature_spline().guid().find(guid).is_some() {
        let origin = Origin {
            guid,
            map_id,
            instance_id,
            at,
            grid: lyracore_shared::spatial::grid_cell(at.0, at.1),
        };
        origin.row(ctx, at, 0).replace(ctx);
    }
}

/// Turns the mover in place with a facing spline. The caller writes the mover.
pub(crate) fn face(ctx: &ReducerContext, mover: &mut WorldEntity, orientation: f32) {
    mover.orientation = orientation;
    CreatureSpline {
        facing: true,
        facing_angle: orientation,
        ..Origin::of(mover).row(ctx, (mover.x, mover.y, mover.z), 0)
    }
    .replace(ctx);
}

/// Moves the mover to where the client draws it on its current leg; false when it has none.
pub(crate) fn place_where_rendered(ctx: &ReducerContext, mover: &mut WorldEntity) -> bool {
    let Some(leg) = ctx.db.game_creature_spline().guid().find(mover.guid) else {
        return false;
    };
    if let Some(stop) = super::cycle::stop_on_stored_leg(ctx, leg) {
        place_stopped(mover, stop, MoveClock::Unchanged);
    }
    true
}

pub(crate) fn place_stopped(mover: &mut WorldEntity, stop: super::cycle::Stop, clock: MoveClock) {
    mover
        .place(stop.at.x, stop.at.y, stop.at.z, clock)
        .unexplored();
    if let Some(heading) = stop.heading {
        mover.orientation = heading;
    }
}

// The client ignores a replacement spline whose id does not exceed the previous one.
pub(crate) fn next_spline_id(now_ms: u32, previous: u32) -> u32 {
    now_ms.max(previous.wrapping_add(1))
}

struct Origin {
    guid: u64,
    map_id: u32,
    instance_id: u64,
    at: (f32, f32, f32),
    grid: (i32, i32),
}

impl Origin {
    fn of(mover: &WorldEntity) -> Self {
        Origin {
            guid: mover.guid,
            map_id: mover.map_id,
            instance_id: mover.instance_id,
            at: (mover.x, mover.y, mover.z),
            grid: (mover.grid_x, mover.grid_y),
        }
    }

    fn row(&self, ctx: &ReducerContext, to: (f32, f32, f32), dur_ms: u32) -> CreatureSpline {
        let now_ms = crate::time::now_ms_u32(ctx);
        let spline_id = ctx
            .db
            .game_creature_spline()
            .guid()
            .find(self.guid)
            .map_or(now_ms, |previous| {
                next_spline_id(now_ms, previous.spline_id)
            });
        CreatureSpline {
            guid: self.guid,
            start_micros: crate::time::now(ctx).to_micros_since_unix_epoch() as u64,
            dur_ms,
            sx: self.at.0,
            sy: self.at.1,
            sz: self.at.2,
            dx: to.0,
            dy: to.1,
            dz: to.2,
            map_id: self.map_id,
            instance_id: self.instance_id,
            grid_x: self.grid.0,
            grid_y: self.grid.1,
            cell: lyracore_shared::spatial::grid_cell_id(self.grid.0, self.grid.1),
            spline_id,
            run: false,
            facing: false,
            facing_angle: 0.0,
            path: None,
            back_off_from: 0,
        }
    }
}

#[cfg(test)]
mod tests {
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
