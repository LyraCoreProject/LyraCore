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

use lyracore_shared::constants;
use spacetimedb::log;

use super::ai::{
    finite_point, leg_in_flight, leg_toward, nearest_waypoint_idx, next_waypoint_idx, spline_t,
    wander_point, TickScope, MOVE_TICK_SECS, RETURN_LEASH_SQ, SENSE_EVERY_N_TICKS,
    WANDER_CHANCE_PCT, WANDER_RADIUS,
};
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

/// A creature the idle movers may consider this firing: awake in an active cell, alive, and not a
/// player. `patrols` picks the anchor — a creature with a route walks it, one without falls back to
/// its spawn post.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct IdleCreature {
    pub guid: u64,
    pub at: Point,
    /// The route segment or wander hop still animating; no new idle leg starts until it lands.
    pub leg_ends_ms: u32,
    /// Route cursor — the waypoint this creature walks TO. Unset or stale re-acquires the nearest.
    pub wp_target: u64,
    pub patrols: bool,
}

/// One waypoint of a patrol route. `id` is the cursor's identity and the route's traversal order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Waypoint {
    pub id: u64,
    pub at: Point,
}

/// A creature's spawn post: where it belongs, and whether it free-wanders around it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Home {
    pub at: Point,
    pub wanders: bool,
}

/// How a creature travels a leg. Idle movement walks; going home runs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Gait {
    Walk,
    Run,
}

/// One movement leg a behavior decided, ready to send.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Leg {
    pub to: (f32, f32),
    /// Landing height where terrain is unimported — the waypoint's or the post's own z.
    pub z_fallback: f32,
    pub dur_ms: u32,
    pub gait: Gait,
    /// Hold the leg to completion (the idle ETA gate) instead of re-deciding it next firing.
    pub hold_until_landed: bool,
}

/// Idle movement's surface: walk a route, walk home, loiter near the post.
pub(crate) trait IdleSink {
    /// The awake creatures idle movement may move at all: alive, not players. Read fresh per phase —
    /// the passes between patrol and the return/wander phase engage creatures and move them.
    fn idle_creatures(&self, active: &HashSet<u64>) -> Vec<IdleCreature>;
    /// This creature's patrol route; empty for a creature that does not patrol.
    fn route_of(&self, guid: u64) -> Vec<Waypoint>;
    /// Where this creature spawned; `None` for one with no post to return to.
    fn home_of(&self, guid: u64) -> Option<Home>;
    /// Is this creature in a fight? Chase and rout own its movement if so.
    fn engaged(&self, guid: u64) -> bool;
    /// Yards per second at `gait`, after snares.
    fn speed_of(&self, guid: u64, gait: Gait) -> f32;
    /// Aim from where the creature stands toward `to`, travelling at most `max_step` yards and
    /// stepping around whatever blocks the straight line.
    fn navigate(&self, guid: u64, to: (f32, f32), max_step: f32) -> (f32, f32);
    /// One roll from the world's random stream.
    fn roll(&self) -> u32;
    /// Move the route cursor to `waypoint_id` without moving the creature.
    fn aim_at_waypoint(&mut self, guid: u64, waypoint_id: u64);
    /// Send the creature on `leg`: ground-snap the landing point, relay the leg to the clients that
    /// can see it, and stamp the move clock (plus the ETA gate for a held leg).
    fn commit_leg(&mut self, guid: u64, leg: Leg, now_ms: u32);
}

/// The whole world one cycle touches.
pub(crate) trait CreatureWorld: MotionSink + IdleSink + LegacyPasses {
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
    fn legacy_aggro_assist(&mut self, active: &HashSet<u64>) -> usize;
    fn legacy_pet(&mut self, scope: &TickScope, now_ms: u32, pets: &[u64]) -> usize;
    fn legacy_cast(&mut self, scope: &TickScope) -> usize;
    fn legacy_threat_retarget(&mut self, scope: &TickScope) -> usize;
    fn legacy_chase(&mut self, scope: &TickScope) -> usize;
    fn legacy_combat_enter(&mut self, scope: &TickScope) -> usize;
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
///   4. walking home before loitering — one idle leg per creature per firing, home wins.
///   5. rout and fear movement LAST, after regen.
///   6. decay before respawn (inside world maintenance — decay arms a future respawn).
///   7. package passes after every core pass.
///
/// The active-cell sweep runs once, before all passes, and its candidate set is shared: a creature
/// absent from it is dormant this firing for every pass that scopes to it (patrol, aggro/assist,
/// idle movement). The engaged, table-driven passes ignore the sweep and gate on `scope.covers`
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
    rows.push(("patrol", patrol(w, &tick, &active) as u64));
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
    rows.push(("idle", idle_movement(w, &tick, &active) as u64));
    if tick.sense && global {
        rows.push(("regen*", w.legacy_regen() as u64));
        rows.push(("combat_drop*", w.legacy_combat_drop(&in_combat) as u64));
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

/// How close to a waypoint counts as standing on it. Below this the segment leg would be
/// zero-length and the client rejects it, so the cursor advance is the whole move.
const WAYPOINT_ARRIVE_YD: f32 = 0.5;

/// The hop a wander stroll is sized for: this phase only rolls on a sense firing, so the leg must
/// span the whole ~4s sense cadence, not one movement firing.
/// ponytail: a schedule row slower than half the sense period senses on EVERY firing, so its
/// wanderers hop a leg longer than the gap to the next roll. Verbatim pre-cycle behavior.
const WANDER_HOP_SECS: f32 = MOVE_TICK_SECS * SENSE_EVERY_N_TICKS as f32;

/// The idle phases visit candidates in guid order, so one firing's legs are emitted in the same
/// order every time no matter how the sweep's set enumerates.
fn idle_order(w: &impl IdleSink, active: &HashSet<u64>) -> Vec<IdleCreature> {
    let mut candidates = w.idle_creatures(active);
    candidates.sort_unstable_by_key(|c| c.guid);
    candidates
}

/// May this creature move itself at all? A fight hands it to chase and rout; crowd control leaves
/// fear as its only mover.
fn moves_itself<W: IdleSink + MotionSink>(w: &W, guid: u64) -> bool {
    !w.engaged(guid) && !w.movement_suppressed(guid)
}

/// PATROL — an idle creature with a route walks it IN ORDER, one segment per leg. The route cursor
/// (`wp_target`) names the waypoint it walks TO; an unset or stale cursor re-acquires the nearest
/// waypoint first, so a fresh spawn joins its route at the closest point instead of the last one.
///
/// The ETA gate is what makes a segment play to completion: while the current leg animates the
/// creature is skipped, so a fast schedule row cannot re-throw the leg every firing.
fn patrol<W: IdleSink + MotionSink>(w: &mut W, tick: &TickContext, active: &HashSet<u64>) -> usize {
    let mut visited = 0usize;
    for c in idle_order(w, active) {
        if !c.patrols {
            continue;
        }
        visited += 1;
        if !moves_itself(w, c.guid) || leg_in_flight(tick.now_ms, c.leg_ends_ms) {
            continue;
        }
        let mut route = w.route_of(c.guid);
        if route.len() < 2 {
            continue; // a single waypoint is a post, not a route
        }
        route.sort_unstable_by_key(|wp| wp.id);
        let points: Vec<(f32, f32)> = route.iter().map(|wp| (wp.at.x, wp.at.y)).collect();
        let next = match route.iter().position(|wp| wp.id == c.wp_target) {
            Some(i) => next_waypoint_idx(i, route.len()),
            None => nearest_waypoint_idx(c.at.x, c.at.y, &points),
        };
        let wp = route[next];
        w.aim_at_waypoint(c.guid, wp.id);
        let (dx, dy, dz) = (wp.at.x - c.at.x, wp.at.y - c.at.y, wp.at.z - c.at.z);
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        if dist < WAYPOINT_ARRIVE_YD {
            continue; // already there — next firing walks to the one after it
        }
        // ponytail: the flat WALK speed, alone among the movers — a snare should slow a patroller
        // and does not. Pre-cycle behavior, carried verbatim.
        let leg = Leg {
            to: (wp.at.x, wp.at.y),
            z_fallback: wp.at.z,
            dur_ms: (dist / constants::speeds::WALK * 1000.0) as u32,
            gait: Gait::Walk,
            hold_until_landed: true,
        };
        w.commit_leg(c.guid, leg, tick.now_ms);
    }
    visited
}

/// IDLE MOVEMENT — walking home and loitering, as ONE decision per creature. A creature displaced
/// past the leash walks back to its post; one that is home-enough may hop around it. The two were
/// separate passes sharing a "moved already this firing" set; making them one branch is what keeps
/// the rule true by construction — two legs in a firing share a `spline_id` and the client rejects
/// the second.
///
/// Patrollers are the patrol phase's, engaged creatures are chase's, and only a RANDOM-movement
/// creature loiters (an IDLE one holds its post: quest givers, vendors, guards).
fn idle_movement<W: IdleSink + MotionSink>(
    w: &mut W,
    tick: &TickContext,
    active: &HashSet<u64>,
) -> usize {
    let mut visited = 0usize;
    for c in idle_order(w, active) {
        visited += 1;
        if c.patrols || !moves_itself(w, c.guid) {
            continue;
        }
        let Some(home) = w.home_of(c.guid) else {
            continue;
        };
        let (hdx, hdy) = (home.at.x - c.at.x, home.at.y - c.at.y);
        if hdx * hdx + hdy * hdy > RETURN_LEASH_SQ {
            walk_home(w, tick, &c, home);
        } else if tick.sense {
            loiter(w, tick, &c, home);
        }
    }
    visited
}

/// One firing's worth of run toward the post, landing ON it rather than short of it. Re-stepped
/// every firing from the CURRENT position, so any number of dormant firings in between only pauses
/// the walk home.
fn walk_home<W: IdleSink>(w: &mut W, tick: &TickContext, c: &IdleCreature, home: Home) {
    let run = w.speed_of(c.guid, Gait::Run);
    let step = w.navigate(c.guid, (home.at.x, home.at.y), run * tick.tick_secs);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, run) else {
        return; // nothing to close — the client rejects a zero-length leg
    };
    let leg = Leg {
        to: (x, y),
        z_fallback: home.at.z,
        dur_ms,
        gait: Gait::Run,
        hold_until_landed: false,
    };
    w.commit_leg(c.guid, leg, tick.now_ms);
}

/// A stroll to a random point near the post on about a third of sense firings — the other two
/// thirds the creature stands still, which is what reads as loitering instead of a constant jog.
/// The hop is anchored on HOME, not on the creature, so a loiterer provably never drifts off its
/// post and never trips the walk-home leash.
fn loiter<W: IdleSink>(w: &mut W, tick: &TickContext, c: &IdleCreature, home: Home) {
    if !home.wanders || leg_in_flight(tick.now_ms, c.leg_ends_ms) {
        return;
    }
    if w.roll() % 100 >= WANDER_CHANCE_PCT {
        return;
    }
    let (hx, hy) = wander_point(home.at.x, home.at.y, w.roll(), w.roll(), WANDER_RADIUS);
    let walk = w.speed_of(c.guid, Gait::Walk);
    let step = w.navigate(c.guid, (hx, hy), walk * WANDER_HOP_SECS);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, walk) else {
        return;
    };
    let leg = Leg {
        to: (x, y),
        z_fallback: home.at.z,
        dur_ms,
        gait: Gait::Walk,
        hold_until_landed: true,
    };
    w.commit_leg(c.guid, leg, tick.now_ms);
}
