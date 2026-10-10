#![cfg_attr(not(has_packages), allow(unused_imports))]

//! Movement reads and legs for an Actor: navigation routes and coverage, terrain height and zone,
//! and the creature-leg operations a Package mover drives.

pub(crate) use crate::creatures::tick::{
    emit_creature_leg, emit_creature_path, stop_where_rendered,
};
pub(crate) use crate::nav::{
    coverage_generation, has_los, inputs, route_path, route_path_with_budget, route_segment_clear,
    route_step, walkable, CoverageEvidence, NavigationInputs, RoutePoint, RouteStatus, RouteStep,
    LEG_MAX_EXPANSIONS,
};
pub(crate) use crate::terrain::{ground_z, snap_z, zone_id_at};
