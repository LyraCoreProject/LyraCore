//! The creature BEHAVIOR CYCLE: one scheduled firing's complete ordered transition, from creature
//! world state to updated state plus emitted movement effects.
//!
//! [`run_cycle`] is the only place that knows the pass order. `tick_creatures` resolves
//! authorization, coverage and cadence, builds a [`TickContext`], calls this once, and logs the
//! [`CycleOutcome`]. Nothing outside this module may compose or reorder behavior passes.
//!
//! Two adapters wear the world trait and neither escapes: [`ctx::CtxWorld`] over a real
//! `ReducerContext`, and `harness::Scenario` in memory for the tests.

use std::collections::HashSet;

use spacetimedb::log;

use super::ai::{finite_point, spline_t, TickScope};
use super::tick::TickSweep;

mod ctx;
#[cfg(test)]
mod harness;

pub(crate) use ctx::run;

/// A world point a behavior decision moves a creature to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Point {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A creature's movement leg, currently playing on the client. `mover_gone` marks an ORPHAN: the
/// creature despawned mid-leg, so the leg is only waiting to be reaped.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct LegInFlight {
    pub guid: u64,
    pub start: Point,
    pub dest: Point,
    pub started_micros: u64,
    pub dur_ms: u32,
    pub map_id: u32,
    pub instance_id: u64,
    pub mover_gone: bool,
}

impl LegInFlight {
    /// Where the mover RENDERS at `now_micros` (the same lerp the client runs), and whether the leg
    /// has landed. Computed absolutely from the leg start, so skipped firings never drift it.
    fn rendered_at(&self, now_micros: u64) -> (Point, bool) {
        let t = spline_t(now_micros, self.started_micros, self.dur_ms);
        let at = Point {
            x: self.start.x + (self.dest.x - self.start.x) * t,
            y: self.start.y + (self.dest.y - self.start.y) * t,
            z: self.start.z + (self.dest.z - self.start.z) * t,
        };
        (at, t >= 1.0)
    }
}

/// Everything ONE firing knows before any behavior runs: when it fires, how far a creature travels
/// in it, whether it senses, and which instances it covers. It never names a pass.
pub(crate) struct TickContext {
    pub now_micros: u64,
    pub now_ms: u32,
    /// Movement step length for this firing, derived from the firing row's own interval.
    pub tick_secs: f32,
    /// Do the ~4s-quantized sensing passes run this firing?
    pub sense: bool,
    pub scope: TickScope,
}

/// What one cycle changed, as the operator log reads it: how many creatures the active-cell sweep
/// woke, and the rows each pass did real per-candidate work on, in pass order. A candidate-set
/// regression shows up here as a count that grows where it must not.
pub(crate) struct CycleOutcome {
    pub awake: usize,
    pub rows_visited: Vec<(&'static str, u64)>,
}

/// Spline advance's surface: read every leg in flight, then move, halt or forget it.
pub(crate) trait MotionSink {
    fn legs_in_flight(&self) -> Vec<LegInFlight>;
    /// Is this creature rooted, stunned, polymorphed or fear-frozen — unable to move itself?
    fn movement_suppressed(&self, guid: u64) -> bool;
    /// Move the creature to `at` — position, grid address and packed cell in one write — and stamp
    /// its move clock at `moved_ms`.
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32);
    /// Freeze a suppressed mover where it renders and tell the client to stop there: the position
    /// moves, the move clock does not, and the emitted leg has zero duration.
    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32);
    /// Forget this creature's leg — arrived, orphaned or refused.
    fn drop_leg(&mut self, guid: u64);
}

/// The whole world one cycle touches.
pub(crate) trait CreatureWorld: MotionSink + LegacyPasses {
    /// The creatures near a covered player this firing, plus the pet and in-combat candidate lists
    /// the same sweep harvests. Read ONCE per cycle and shared by every pass that scopes to it.
    fn awake_creatures(&self, scope: &TickScope) -> TickSweep;
    /// Corpse decay, creature respawn and gameobject respawn. Not behavior — the cycle only
    /// SEQUENCES them. Returns the rows each visited, for the operator log.
    fn run_due_world_maintenance(&mut self) -> Vec<(&'static str, u64)>;
    /// Every registered package tick pass, after all core behavior.
    fn run_package_passes(&mut self);
}

// ponytail: migration scaffolding. Every method here is deleted by the ticket that migrates its
// pass; ticket 09 deletes the trait. See .scratch/creature-behavior-cycle/.
/// Passes still living in `creatures::tick` — the cycle owns WHEN they run, not yet HOW.
pub(crate) trait LegacyPasses {
    fn legacy_patrol(&mut self, active: &HashSet<u64>) -> usize;
    fn legacy_aggro_assist(&mut self, active: &HashSet<u64>) -> usize;
    fn legacy_pet(&mut self, scope: &TickScope, now_ms: u32, pets: &[u64]) -> usize;
    fn legacy_cast(&mut self, scope: &TickScope) -> usize;
    fn legacy_threat_retarget(&mut self, scope: &TickScope) -> usize;
    fn legacy_chase(&mut self, scope: &TickScope) -> usize;
    fn legacy_combat_enter(&mut self, scope: &TickScope) -> usize;
    fn legacy_return(&mut self, active: &HashSet<u64>, tick_secs: f32) -> usize;
    fn legacy_wander(&mut self, active: &HashSet<u64>) -> usize;
    fn legacy_regen(&mut self) -> usize;
    fn legacy_combat_drop(&mut self, in_combat: &[u64]) -> usize;
    fn legacy_flee(&mut self, scope: &TickScope) -> usize;
    fn legacy_fear_flee(&mut self, scope: &TickScope, tick_secs: f32) -> usize;
}

/// ONE firing's complete behavior transition. The order below is load-bearing:
///   1. advance splines FIRST — every range read must see where a creature renders, not its leg end.
///   2. aggro and pet engagement before chase — a creature aggroed this sense tick closes the same
///      tick; cast and threat retarget also precede chase (cast instead of close; move at the newly
///      selected victim).
///   3. chase before regen — regen's in-combat gate must see the still-engaged chaser.
///   4. return before wander, sharing the one-leg-per-firing exclusion.
///   5. rout and fear movement LAST, after regen.
///   6. decay before respawn (inside world maintenance — decay arms a future respawn).
///   7. package passes after every core pass.
///
/// The active-cell sweep runs once, before all passes, and its candidate set is shared: a creature
/// absent from it is dormant this firing for every pass that scopes to it (patrol, aggro/assist,
/// return, wander). The engaged, table-driven passes ignore the sweep and gate on `scope.covers`
/// instead, so a player can drag a creature anywhere without freezing it. The due-time passes
/// (decay, respawn, gameobject respawn, regen, combat drop) and the package passes run on the
/// catch-all firing only, and still cover every instance.
pub(crate) fn run_cycle<W: CreatureWorld>(w: &mut W, tick: TickContext) -> CycleOutcome {
    let TickSweep {
        active,
        pets,
        in_combat,
    } = w.awake_creatures(&tick.scope);
    let global = tick.scope.runs_global_passes();
    let mut rows: Vec<(&'static str, u64)> = Vec::new();

    rows.push(("advance", advance_legs(w, &tick) as u64));
    rows.push(("patrol", w.legacy_patrol(&active) as u64));
    if tick.sense {
        if global {
            rows.extend(w.run_due_world_maintenance());
        }
        rows.push(("aggro_assist", w.legacy_aggro_assist(&active) as u64));
        rows.push(("pet", w.legacy_pet(&tick.scope, tick.now_ms, &pets) as u64));
        rows.push(("cast", w.legacy_cast(&tick.scope) as u64));
        rows.push((
            "threat_retarget",
            w.legacy_threat_retarget(&tick.scope) as u64,
        ));
    }
    rows.push(("chase", w.legacy_chase(&tick.scope) as u64));
    rows.push(("combat_enter", w.legacy_combat_enter(&tick.scope) as u64));
    rows.push(("return", w.legacy_return(&active, tick.tick_secs) as u64));
    if tick.sense {
        rows.push(("wander", w.legacy_wander(&active) as u64));
        if global {
            rows.push(("regen*", w.legacy_regen() as u64));
            rows.push(("combat_drop*", w.legacy_combat_drop(&in_combat) as u64));
        }
    }
    rows.push(("flee", w.legacy_flee(&tick.scope) as u64));
    rows.push((
        "fear_flee",
        w.legacy_fear_flee(&tick.scope, tick.tick_secs) as u64,
    ));
    if global {
        w.run_package_passes();
    }

    CycleOutcome {
        awake: active.len(),
        rows_visited: rows,
    }
}

/// SPLINE ADVANCE — the first phase of every cycle. Moves each in-flight leg to the point the client
/// renders it at, so the passes below read where the creature IS instead of leading by a whole leg.
/// A suppressed mover freezes and the client is told to stop (no rooted slide into melee); arrived,
/// orphaned and non-finite legs are forgotten. Returns the legs actually advanced.
fn advance_legs<W: MotionSink>(w: &mut W, tick: &TickContext) -> usize {
    let mut visited = 0usize;
    for leg in w.legs_in_flight() {
        if leg.mover_gone {
            w.drop_leg(leg.guid);
            continue;
        }
        let (at, arrived) = leg.rendered_at(tick.now_micros);
        // A leg whose endpoints went bad would write the corruption onto the creature every firing,
        // and a creature at an infinite grid cell is in no active cell ever again — unkillable while
        // its melee row keeps swinging. Refuse it and leave the creature where it was.
        if !finite_point(at.x, at.y, at.z) {
            log::error!(
                "refused a non-finite spline advance for guid {} — dropping the leg",
                leg.guid
            );
            w.drop_leg(leg.guid);
            continue;
        }
        // The halt does NOT forget the leg: the zero-duration stop replaces it and reaps itself on
        // the next cycle, when it reads as arrived.
        if w.movement_suppressed(leg.guid) {
            w.halt(&leg, at, tick.now_ms);
            continue;
        }
        w.commit_position(leg.guid, at, tick.now_ms);
        if arrived {
            w.drop_leg(leg.guid);
        }
        visited += 1;
    }
    visited
}
