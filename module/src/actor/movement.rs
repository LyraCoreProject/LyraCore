#![allow(
    unused_imports,
    reason = "Packages select the named API operations they need"
)]

pub(crate) use crate::creatures::leg::start_path as emit_creature_path;
pub(crate) use crate::nav::{
    coverage_generation, has_los, inputs, route_path, route_path_with_budget, route_segment_clear,
    route_step, walkable, CoverageEvidence, NavigationInputs, RoutePoint, RouteStatus, RouteStep,
    LEG_MAX_EXPANSIONS,
};
pub(crate) use crate::terrain::{ground_z, snap_z, zone_id_at};

/// Sends the mover on a straight leg from its stored position and writes the mover. The Realm
/// Clock picks the spline id and stamps the leg; `now_ms` stays for Package API version 2.
#[cfg_attr(not(has_packages), allow(dead_code))]
#[allow(clippy::too_many_arguments, reason = "Package API version 2 signature")]
pub(crate) fn emit_creature_leg(
    ctx: &spacetimedb::ReducerContext,
    mover: crate::WorldEntity,
    to: (f32, f32),
    z_fallback: f32,
    duration_ms: u32,
    run: bool,
    _now_ms: u32,
    hold_until_landed: bool,
) {
    use crate::creatures::leg;
    let course = leg::Course::on_ground(ctx, &mover, to, z_fallback, duration_ms, run)
        .held_until_landed(hold_until_landed);
    leg::start(ctx, mover, course);
}
