//! The ONE production adapter: a real `ReducerContext` wearing the whole cycle world.
//!
//! **This layer is the seam's own blind spot.** The harness substitutes an in-memory `Scenario` for
//! every line of it, so a no-op'd method here is invisible to every test in the crate. Keep every
//! method a pass-through; ticket 09 pins its exact shape the way `transfer::tests::
//! the_production_adapter_is_the_pass_through_the_harness_assumes` pins `CtxShard`.

use std::collections::HashSet;

use lyracore_shared::spatial;
use spacetimedb::{ReducerContext, Table};

use super::{
    run_cycle, CreatureWorld, CycleOutcome, LegInFlight, LegacyPasses, MotionSink, Point,
    TickContext,
};
use crate::creatures::ai::TickScope;
use crate::creatures::tick::{self, TickSweep};
use crate::{game_creature_spline, game_world_entity};

/// `tick_creatures`' one call into the cycle. The adapter never leaves this module.
pub(crate) fn run(ctx: &ReducerContext, tick: TickContext, interval_micros: i64) -> CycleOutcome {
    run_cycle(
        &mut CtxWorld {
            ctx,
            interval_micros,
            moved_this_tick: HashSet::new(),
        },
        tick,
    )
}

struct CtxWorld<'a> {
    ctx: &'a ReducerContext,
    /// The firing row's own cadence — only `pass_pet` still reads it.
    // ponytail: migration scaffolding, deleted with `legacy_pet` in ticket 07.
    interval_micros: i64,
    /// Guids the return pass already moved this firing, which the wander pass must skip: one leg per
    /// creature per firing, because both would share a `spline_id` and the client rejects the second.
    // ponytail: migration scaffolding, deleted when ticket 02 migrates return and wander.
    moved_this_tick: HashSet<u64>,
}

impl CtxWorld<'_> {
    /// Move the creature's authoritative row to `at`, writing grid address and packed cell in the
    /// SAME statement (a stale `cell` puts the row in the wrong AOI cell). `moved_ms` stamps the
    /// move clock; a halted creature passes `None` because it did not travel.
    fn place(&self, guid: u64, at: Point, moved_ms: Option<u32>) -> Option<(i32, i32)> {
        let entities = self.ctx.db.game_world_entity();
        let mut e = entities.guid().find(guid)?;
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        e.x = at.x;
        e.y = at.y;
        e.z = at.z;
        e.grid_x = gx;
        e.grid_y = gy;
        e.cell = spatial::grid_cell_id(gx, gy);
        if let Some(ms) = moved_ms {
            e.last_move_ms = ms;
        }
        entities.guid().update(e);
        Some((gx, gy))
    }
}

impl MotionSink for CtxWorld<'_> {
    fn legs_in_flight(&self) -> Vec<LegInFlight> {
        let entities = self.ctx.db.game_world_entity();
        self.ctx
            .db
            .game_creature_spline()
            .iter()
            .map(|s| LegInFlight {
                guid: s.guid,
                start: Point {
                    x: s.sx,
                    y: s.sy,
                    z: s.sz,
                },
                dest: Point {
                    x: s.dx,
                    y: s.dy,
                    z: s.dz,
                },
                started_micros: s.start_micros,
                dur_ms: s.dur_ms,
                map_id: s.map_id,
                instance_id: s.instance_id,
                mover_gone: entities.guid().find(s.guid).is_none(),
            })
            .collect()
    }
    fn movement_suppressed(&self, guid: u64) -> bool {
        crate::spell::is_self_movement_suppressed(self.ctx, guid)
    }
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) {
        self.place(guid, at, Some(moved_ms));
    }
    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32) {
        if let Some(grid) = self.place(leg.guid, at, None) {
            tick::emit_move_spline(
                self.ctx,
                leg.guid,
                (at.x, at.y, at.z),
                (at.x, at.y, at.z),
                0,
                false,
                spline_id,
                leg.map_id,
                leg.instance_id,
                grid,
            );
        }
    }
    fn drop_leg(&mut self, guid: u64) {
        self.ctx.db.game_creature_spline().guid().delete(guid);
    }
}

impl CreatureWorld for CtxWorld<'_> {
    fn awake_creatures(&self, scope: &TickScope) -> TickSweep {
        tick::active_cell_creatures(self.ctx, scope)
    }
    fn run_due_world_maintenance(&mut self) -> Vec<(&'static str, u64)> {
        vec![
            ("decay*", tick::pass_decay(self.ctx) as u64),
            ("respawn*", tick::pass_respawn(self.ctx) as u64),
            (
                "go_respawn*",
                tick::pass_gameobject_respawn(self.ctx) as u64,
            ),
        ]
    }
    fn run_package_passes(&mut self) {
        crate::hooks::run_package_tick_passes(self.ctx);
    }
}

impl LegacyPasses for CtxWorld<'_> {
    fn legacy_patrol(&mut self, active: &HashSet<u64>) -> usize {
        tick::pass_patrol(self.ctx, active)
    }
    fn legacy_aggro_assist(&mut self, active: &HashSet<u64>) -> usize {
        tick::pass_aggro_assist(self.ctx, active)
    }
    fn legacy_pet(&mut self, scope: &TickScope, now_ms: u32, pets: &[u64]) -> usize {
        crate::creatures::pass_pet(self.ctx, now_ms, scope, self.interval_micros, pets)
    }
    fn legacy_cast(&mut self, scope: &TickScope) -> usize {
        tick::pass_cast(self.ctx, scope)
    }
    fn legacy_threat_retarget(&mut self, scope: &TickScope) -> usize {
        tick::pass_threat_retarget(self.ctx, scope)
    }
    fn legacy_chase(&mut self, scope: &TickScope) -> usize {
        tick::pass_chase(self.ctx, scope)
    }
    fn legacy_combat_enter(&mut self, scope: &TickScope) -> usize {
        tick::pass_combat_enter(self.ctx, scope)
    }
    fn legacy_return(&mut self, active: &HashSet<u64>, tick_secs: f32) -> usize {
        tick::pass_return(self.ctx, &mut self.moved_this_tick, active, tick_secs)
    }
    fn legacy_wander(&mut self, active: &HashSet<u64>) -> usize {
        tick::pass_wander(self.ctx, &self.moved_this_tick, active)
    }
    fn legacy_regen(&mut self) -> usize {
        tick::pass_regen(self.ctx)
    }
    fn legacy_combat_drop(&mut self, in_combat: &[u64]) -> usize {
        tick::pass_combat_drop(self.ctx, in_combat)
    }
    fn legacy_flee(&mut self, scope: &TickScope) -> usize {
        tick::pass_flee(self.ctx, scope)
    }
    fn legacy_fear_flee(&mut self, scope: &TickScope, tick_secs: f32) -> usize {
        tick::pass_fear_flee(self.ctx, scope, tick_secs)
    }
}
