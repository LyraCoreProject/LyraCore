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
    aggro_radius, finite_point, hp_pct_below, is_aggro_candidate, leg_in_flight, leg_toward,
    nearest_waypoint_idx, next_waypoint_idx, spline_t, stealth_detect_range, wander_point,
    within_assist_radius, TickScope, CHASE_LEAD_YD, CHASE_LEASH_SQ, CHASE_MELEE_SQ,
    CHASE_REPATH_COS, CHASE_TARGET_MOVING_MS, MOVE_TICK_SECS, RETURN_LEASH_SQ, SENSE_EVERY_N_TICKS,
    WANDER_CHANCE_PCT, WANDER_RADIUS,
};
use super::cast_condition;
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
    /// Stop this mover at `at` and tell the client to halt there: the position moves, the move clock
    /// does not, and the emitted leg has zero duration, REPLACING the leg it interrupts so the next
    /// cycle reads it as landed. Both a suppressed mover frozen mid-leg and a chaser that reached
    /// its victim end this way.
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

/// A player a creature could notice this firing. `dead` and `godmode` travel on the view rather
/// than being filtered away by the adapter, because "who is worth noticing" is a behavior rule the
/// cycle owns — see [`is_aggro_candidate`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct AggroTarget {
    pub guid: u64,
    pub at: Point,
    pub level: u32,
    pub faction_template: u32,
    pub map_id: u32,
    pub instance_id: u64,
    pub dead: bool,
    pub godmode: bool,
    /// Stealthed: seen only from inside this creature's own, much shorter, detection range.
    pub stealthed: bool,
}

/// A creature that may start a fight this firing: awake in an active cell, alive, not a pet, and
/// not already swinging at someone.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Sensor {
    pub guid: u64,
    pub at: Point,
    pub level: u32,
    pub faction_template: u32,
    pub map_id: u32,
    pub instance_id: u64,
    /// The template's hand-tuned aggro range (0 = the level-scaled default), or `None` when the
    /// template row is missing: such a creature notices nobody, but still answers a pack call.
    pub aggro_range: Option<u32>,
    /// Mind Soothe and friends — a signed yard change to this creature's own detection radius.
    pub detect_range_mod: f32,
    /// Near death and of a kind that runs. Engaging it would only drag it back into the fight it
    /// is about to flee.
    pub would_rout: bool,
    /// Stunned, polymorphed or feared. It cannot act, so it neither pulls nor answers a call.
    pub cannot_act: bool,
}

/// Why a creature entered a fight, as the world hook reports it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Pull {
    /// It noticed the player itself.
    Noticed,
    /// It answered a pack mate's call.
    Assisted,
}

/// One live melee engagement, as combat entry reads it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Engagement {
    pub attacker: u64,
    pub victim: u64,
    /// The pair's instance. Both sides share one by construction, so either side resolves it.
    pub instance_id: u64,
    /// A player's auto-attack toggle that has never landed a swing. Aiming at something out of
    /// range is not combat, and stopping the toggle leaves the player as un-engaged as before.
    pub player_never_swung: bool,
}

/// Engagement's surface: who can notice whom, and the fight that follows.
pub(crate) trait EngageSink {
    /// Every player in the world, with stealth already resolved.
    fn players(&self) -> Vec<AggroTarget>;
    /// The awake creatures that could start a fight. Read FRESH per phase: a creature that pulled
    /// during the aggro phase is now swinging, so it must not also answer a pack call.
    fn sensing_creatures(&self, active: &HashSet<u64>) -> Vec<Sensor>;
    /// Do these two faction templates count each other as enemies? Missing data is never hostile.
    fn hostile(&self, faction_template: u32, other: u32) -> bool;
    /// Can `looker` see the point `at`? A wall between them stops a pull and a pack call alike.
    fn line_of_sight(&self, looker: u64, at: Point) -> bool;
    /// Arm the fight: the creature swings at `victim`, faces it, and the world is told.
    fn engage(&mut self, creature: u64, victim: u64, pull: Pull);
    /// Every live melee engagement, player-started ones included.
    fn engagements(&self) -> Vec<Engagement>;
    /// Flag the unit in combat and push its combat-drop deadline out.
    fn enter_combat(&mut self, guid: u64);
}

/// An engaged creature the cast phase considers, with everything a rotation condition reads.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Caster {
    pub guid: u64,
    /// The unit it is swinging at — every offensive line of the rotation aims here.
    pub victim: u64,
    /// Where the victim stands, for the one line-of-sight verdict cast and chase share. `None` when
    /// the victim row is gone: nothing blocks a line to a unit that is not there.
    pub victim_at: Option<Point>,
    /// Spell rank comes from the caster's level.
    pub level: u8,
    pub health: u32,
    pub max_health: u32,
    /// Stunned, polymorphed or feared. A ROOTED caster still casts — ranged needs no movement.
    pub cannot_act: bool,
    /// Already mid-cast. Beginning a second cast deletes and restarts the first, so a creature that
    /// re-entered every firing would never finish one.
    pub casting: bool,
}

/// What one authored rotation line waits for before it fires.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CastWhen {
    /// A nuke or debuff at the victim: eligible whenever the spell itself is ready.
    Always,
    /// A heal on itself, below this percentage of its own health.
    Hurt(u32),
    /// A debuff the victim does not carry yet.
    VictimLacksIt,
    /// A buff the caster has let lapse.
    SelfLacksIt,
    /// A condition this server does not understand. The line is authored but never fires, and it
    /// still counts as a rotation — a creature that has one does not fall back to its lone spell.
    Never,
}

impl CastWhen {
    /// Read an authored rotation condition. Kept here rather than in the adapter so the whole
    /// rotation grammar, unknown discriminants included, is one cycle-owned decision.
    fn of(condition: u8, condition_value: u8) -> CastWhen {
        match condition {
            cast_condition::ALWAYS => CastWhen::Always,
            cast_condition::SELF_HP_BELOW_PCT => CastWhen::Hurt(condition_value as u32),
            cast_condition::TARGET_MISSING_AURA => CastWhen::VictimLacksIt,
            cast_condition::SELF_MISSING_AURA => CastWhen::SelfLacksIt,
            _ => CastWhen::Never,
        }
    }
}

/// One line of a creature's spell rotation: which spell, and what it waits for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SpellOption {
    pub spell_id: u32,
    pub when: CastWhen,
    /// Tried highest first.
    pub priority: u8,
    /// Authoring order — the tie-break that keeps a same-priority pair deterministic.
    pub authored: u64,
}

/// Casting's surface: who is engaged, what it may cast, and "begin this cast". The cycle picks WHICH
/// spell and WHETHER to cast; resolution, cooldown, power cost and aura application stay behind
/// [`CastSink::begin_cast`], in the spell module.
pub(crate) trait CastSink {
    /// Every engaged creature this firing covers whose attacker could cast — the same engagement
    /// rows and the same gate ladder chase opens with.
    fn casters(&self, scope: &TickScope) -> Vec<Caster>;
    /// This creature's authored rotation, in no particular order; empty for one with no rotation.
    fn rotation_of(&self, guid: u64) -> Vec<SpellOption>;
    /// The single spell a creature with no rotation casts at its victim.
    fn lone_spell(&self, guid: u64) -> Option<u32>;
    /// Does this unit already carry `spell_id`'s aura? The missing-aura conditions read it.
    fn carries(&self, guid: u64, spell_id: u32) -> bool;
    /// Begin `spell_id` at `target`. `false` means the spell was not ready — global cooldown, its own
    /// cooldown, power cost or range — so the next line of the rotation gets its turn.
    fn begin_cast(&mut self, caster: &Caster, spell_id: u32, target: u64) -> bool;
}

/// One engaged creature and the unit it currently fights — the pairing threat may overturn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Fighter {
    pub guid: u64,
    pub victim: u64,
}

/// Threat's surface. The authority is `crate::threat`: who counts as a valid source, how ties break,
/// and when a taunt lock is still live all stay there — the cycle only composes them.
pub(crate) trait ThreatSink {
    /// Every engaged creature this firing covers, with the unit it is fighting now.
    fn fighters(&self, scope: &TickScope) -> Vec<Fighter>;
    /// The unit a live taunt lock pins this creature on, regardless of the threat table.
    fn taunted_onto(&self, guid: u64) -> Option<u64>;
    /// Its highest-threat valid source; `None` for an empty or all-invalid table.
    fn top_threat(&self, guid: u64) -> Option<u64>;
    /// How much threat `source` holds on `creature`; a source absent from the table holds none.
    fn threat_on(&self, creature: u64, source: u64) -> i64;
    /// Point the creature at `victim` — the engagement and the creature's own target together, and
    /// nothing else: the swing clock stays where it was, so a retarget does not refresh the swing.
    fn retarget(&mut self, creature: u64, victim: u64);
}

/// One creature closing on the unit it fights, with everything that decision reads: where both
/// stand, how recently the victim moved, and the leg the chaser is already running.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Pursuit {
    pub guid: u64,
    pub at: Point,
    /// Where the creature currently looks — the facing turn is idempotent against it.
    pub orientation: f32,
    pub victim_at: Point,
    /// The victim's move clock. A kiter is run down on the move; a planted one is stood next to.
    pub victim_last_move_ms: u32,
    /// Inside an open rout window: the rout leg is this creature's sole mover this firing.
    pub routing: bool,
    /// The leg it is already riding. Both the re-aim decision and the stop read it.
    pub leg: Option<LegInFlight>,
}

/// Chase's surface: the fights worth closing, how far a caster fights from, and where a creature
/// looks. It shares the movers the idle phases use — `speed_of`, `navigate`, `commit_leg` — and
/// `halt` with spline advance, so a chase leg is written by the one leg writer like every other.
pub(crate) trait PursuitSink {
    /// Every live engagement this firing covers whose attacker is a creature that could move and
    /// whose victim is still in the world. NOT scoped to the active-cell sweep: a fight dragged out
    /// of every active cell must keep running.
    fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit>;
    /// The longest range this creature can bring an offensive spell to bear from; 0 for anything
    /// that has to close to melee.
    fn caster_hold_range(&self, guid: u64) -> f32;
    /// Turn the creature to `orientation` and tell the clients that can see it. A spline is the only
    /// thing a client derives creature facing from, so this is a zero-duration facing packet.
    fn face(&mut self, guid: u64, orientation: f32, spline_id: u32);
}

/// The whole world one cycle touches.
pub(crate) trait CreatureWorld:
    MotionSink + IdleSink + EngageSink + CastSink + ThreatSink + PursuitSink + LegacyPasses
{
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
    fn legacy_pet(&mut self, scope: &TickScope, now_ms: u32, pets: &[u64]) -> usize;
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
///   3. aggro before assist before combat entry — a pack answers a call raised this firing, and
///      every creature that pulled carries its combat flag before the firing ends.
///   4. chase before regen — regen's in-combat gate must see the still-engaged chaser.
///   5. walking home before loitering — one idle leg per creature per firing, home wins.
///   6. rout and fear movement LAST, after regen.
///   7. decay before respawn (inside world maintenance — decay arms a future respawn).
///   8. package passes after every core pass.
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
        let (pulls, seen) = aggro(w, &active);
        rows.push(("aggro", seen as u64));
        rows.push(("assist", assist(w, &active, pulls) as u64));
        rows.push(("pet", w.legacy_pet(&tick.scope, tick.now_ms, &pets) as u64));
        rows.push(("cast", cast(w, &tick.scope) as u64));
        rows.push(("threat_retarget", threat_retarget(w, &tick.scope) as u64));
    }
    rows.push(("chase", chase(w, &tick) as u64));
    rows.push(("combat_enter", combat_entry(w, &tick.scope) as u64));
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

/// One creature's pull this firing: who it engaged, and where it stood when it called for help.
/// Internal cycle state — it never leaves the aggro-to-assist handoff.
#[derive(Clone, Copy)]
struct Called {
    guid: u64,
    at: Point,
    map_id: u32,
    instance_id: u64,
    faction_template: u32,
    victim: u64,
}

/// The engagement phases visit candidates in guid order, so one firing arms its fights in the same
/// order every time no matter how the sweep's set enumerates.
fn sensing_order(w: &impl EngageSink, active: &HashSet<u64>) -> Vec<Sensor> {
    let mut candidates = w.sensing_creatures(active);
    candidates.sort_unstable_by_key(|c| c.guid);
    candidates
}

/// AGGRO — a creature notices a hostile player and engages it on sight. Collect-then-arm: the
/// on-aggro hook runs package code, so nothing may fire while the scan is still deciding.
///
/// Returns this firing's pulls, which are the assist phase's whole input, plus the candidates
/// visited. With nobody worth noticing in the world the phase costs one player read and stops.
fn aggro<W: EngageSink>(w: &mut W, active: &HashSet<u64>) -> (Vec<Called>, usize) {
    let targets: Vec<AggroTarget> = w
        .players()
        .into_iter()
        .filter(|p| is_aggro_candidate(p.dead, p.godmode))
        .collect();
    if targets.is_empty() {
        return (Vec::new(), 0);
    }
    let sensors = sensing_order(w, active);
    let mut called = Vec::new();
    for s in &sensors {
        if s.would_rout || s.cannot_act {
            continue;
        }
        let Some(victim) = nearest_noticed(w, s, &targets) else {
            continue;
        };
        called.push(Called {
            guid: s.guid,
            at: s.at,
            map_id: s.map_id,
            instance_id: s.instance_id,
            faction_template: s.faction_template,
            victim,
        });
    }
    for c in &called {
        w.engage(c.guid, c.victim, Pull::Noticed);
    }
    (called, sensors.len())
}

/// The nearest player this creature notices: same map and instance, hostile, inside the
/// level-scaled aggro radius (shrunk by a soothe, graded down again for a stealthed target), and
/// in plain sight. `None` means nothing to pull.
fn nearest_noticed<W: EngageSink>(w: &W, s: &Sensor, targets: &[AggroTarget]) -> Option<u64> {
    let tuned = s.aggro_range?; // no template row, no proximity aggro
    targets
        .iter()
        .filter(|p| p.map_id == s.map_id && p.instance_id == s.instance_id)
        .filter_map(|p| {
            let radius = (aggro_radius(s.level, p.level, tuned) + s.detect_range_mod).max(0.0);
            if radius <= 0.0 {
                return None; // grey to this player, or soothed all the way down
            }
            let d2 = dist_sq(s.at, p.at);
            if d2 > radius * radius {
                return None;
            }
            if p.stealthed && d2 > stealth_detect_range(s.level, p.level).powi(2) {
                return None; // sneaking past, outside what this creature can see through
            }
            if !w.hostile(s.faction_template, p.faction_template) {
                return None;
            }
            if !w.line_of_sight(s.guid, p.at) {
                return None; // a hostile behind the abbey wall is not "seen"
            }
            Some((p.guid, d2))
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(guid, _)| guid)
}

fn dist_sq(a: Point, b: Point) -> f32 {
    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
    dx * dx + dy * dy + dz * dz
}

/// ASSIST — pack behavior: a same-faction neighbor of a creature that pulled THIS firing piles onto
/// the same player, even a passive one the player never got close enough to notice. Range is
/// measured from the CALLER, not from the player, so a far-flung pack mate is not dragged in, and
/// the neighbor's own `aggro_range` is deliberately not consulted.
///
/// Nobody pulled means no scan at all — the overwhelmingly common firing. Calls are answered lowest
/// caller guid first, so a neighbor standing between two of them always joins the same fight.
fn assist<W: EngageSink>(w: &mut W, active: &HashSet<u64>, mut calls: Vec<Called>) -> usize {
    if calls.is_empty() {
        return 0;
    }
    calls.sort_unstable_by_key(|c| c.guid);
    // Fresh read: every creature that pulled above is now swinging, so it is no longer a candidate
    // here — one creature answers at most one call per firing by construction.
    let neighbors = sensing_order(w, active);
    let mut answered = Vec::new();
    for n in &neighbors {
        if n.would_rout || n.cannot_act {
            continue;
        }
        let call = calls.iter().find(|c| {
            c.guid != n.guid
                && c.map_id == n.map_id
                && c.faction_template == n.faction_template
                && c.instance_id == n.instance_id
                && within_assist_radius(c.at.x, c.at.y, n.at.x, n.at.y)
                // The caller is sighted at the NEIGHBOR's own height: the two stand a pack's width
                // apart, and their feet are what the ray has to clear.
                && w.line_of_sight(
                    n.guid,
                    Point {
                        x: c.at.x,
                        y: c.at.y,
                        z: n.at.z,
                    },
                )
        });
        if let Some(call) = call {
            answered.push((n.guid, call.victim));
        }
    }
    for (neighbor, victim) in answered {
        w.engage(neighbor, victim, Pull::Assisted);
    }
    neighbors.len()
}

/// CAST — an engaged caster creature begins ONE action from its rotation. It runs before chase, so a
/// caster that can cast casts instead of merely closing, and both phases reach the SAME line-of-sight
/// verdict about the same line: with a wall in the way the offensive half of the rotation is dropped
/// here and chase closes rather than holding at spell range.
///
/// A creature with rotation lines uses them, highest priority first; one with none falls back to its
/// single authored spell; one with neither never casts. Eligibility is only the CONDITION half — the
/// spell module owns cooldown, power cost and range, and reports "not ready" by refusing the cast, at
/// which point the next line down gets its turn. At most one cast per creature per firing.
///
/// The engagement rows are the candidate set, so this stays O(fights running). Visited in guid order.
/// Returns the covered candidates considered.
fn cast<W: CastSink + EngageSink>(w: &mut W, scope: &TickScope) -> usize {
    let mut casters = w.casters(scope);
    casters.sort_unstable_by_key(|c| c.guid);
    let visited = casters.len();
    // Collect-then-cast: a cast applies auras and moves health, which the conditions above read. One
    // interleaved loop would let the first creature's cast decide the second creature's rotation.
    let mut ready: Vec<(Caster, Vec<(u32, u64)>)> = Vec::new();
    for c in casters {
        if c.cannot_act || c.casting {
            continue;
        }
        let mut rotation = w.rotation_of(c.guid);
        let mut candidates: Vec<(u32, u64)> = if rotation.is_empty() {
            match w.lone_spell(c.guid) {
                Some(spell_id) => vec![(spell_id, c.victim)],
                None => continue, // no rotation and no spell — it only ever melees
            }
        } else {
            rotation.sort_unstable_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then(a.authored.cmp(&b.authored))
            });
            rotation
                .iter()
                .filter_map(|o| eligible_cast(w, &c, o))
                .collect()
        };
        // A hostile cast needs a clear line to its victim. Blocked, only the self-cast half survives
        // and the creature melees instead — the verdict chase reads again to close the distance.
        if candidates.iter().any(|&(_, t)| t != c.guid) {
            let blocked = c.victim_at.is_some_and(|at| !w.line_of_sight(c.guid, at));
            if blocked {
                candidates.retain(|&(_, t)| t == c.guid);
            }
        }
        if !candidates.is_empty() {
            ready.push((c, candidates));
        }
    }
    for (c, candidates) in ready {
        for (spell_id, target) in candidates {
            if w.begin_cast(&c, spell_id, target) {
                break; // one action per firing; the rest of the rotation waits
            }
        }
    }
    visited
}

/// One rotation line's verdict: `Some((spell, target))` when the world matches what the line waits
/// for. A heal or buff is aimed at the caster, a nuke or debuff at its victim.
fn eligible_cast<W: CastSink>(w: &W, c: &Caster, option: &SpellOption) -> Option<(u32, u64)> {
    let target = match option.when {
        CastWhen::Always => c.victim,
        CastWhen::Hurt(pct) => hp_pct_below(c.health, c.max_health, pct).then_some(c.guid)?,
        CastWhen::VictimLacksIt => (!w.carries(c.victim, option.spell_id)).then_some(c.victim)?,
        CastWhen::SelfLacksIt => (!w.carries(c.guid, option.spell_id)).then_some(c.guid)?,
        CastWhen::Never => return None,
    };
    Some((option.spell_id, target))
}

/// THREAT RETARGET — an engaged creature re-points at whoever it hates most. It runs before chase, so
/// the movement leg in the same firing follows the newly chosen victim rather than trailing a firing
/// behind it. Choosing a target is neither moving nor acting, so crowd control does not gate it: a
/// stunned creature still tracks who it will swing at when the stun lifts.
///
/// A live taunt lock pins the creature outright. Otherwise the switch needs STRICTLY more threat than
/// the current victim holds, which is the hysteresis that stops a tie flapping the target: an empty
/// table, or one source, leaves a proximity pull exactly where it was.
///
/// ponytail: this walks the engagement rows the cast phase just walked, rather than sharing one
/// candidate list. Ceiling: a second pass over the engaged rows per sense firing. A shared snapshot
/// would be read before the casts above land, which is exactly what a fresh read exists to avoid.
fn threat_retarget<W: ThreatSink>(w: &mut W, scope: &TickScope) -> usize {
    let fighters = w.fighters(scope);
    let visited = fighters.len();
    // Collect-then-repoint: retargeting writes the engagement rows this list was read from.
    let mut switches: Vec<(u64, u64)> = Vec::new();
    for f in &fighters {
        if let Some(pinned) = w.taunted_onto(f.guid) {
            if pinned != f.victim {
                switches.push((f.guid, pinned));
            }
            continue; // pinned — the threat compare is suspended for the taunt window
        }
        let Some(top) = w.top_threat(f.guid) else {
            continue;
        };
        if top != f.victim && w.threat_on(f.guid, top) > w.threat_on(f.guid, f.victim) {
            switches.push((f.guid, top));
        }
    }
    for (creature, victim) in switches {
        w.retarget(creature, victim);
    }
    visited
}

/// How far short of a STANDING victim a closing leg lands: just inside the 5-yard melee band, so the
/// next swing connects and the leg never walks onto the victim.
const MELEE_STOP_SHORT_YD: f32 = 4.0;

/// A caster holds inside this fraction of its longest spell range, so a victim's small strafes do
/// not yo-yo it across the boundary.
const CASTER_HOLD_MARGIN: f32 = 0.9;

/// The least bearing drift (radians, ~17°) that is worth turning for. Below it the correction would
/// be imperceptible and would re-throw a facing packet every firing; the epsilon is what makes "have
/// I already turned to face it" idempotent firing over firing.
const FACING_EPSILON_RAD: f32 = 0.3;

/// Shortest signed angular distance from `from` to `to`, wrapped into `(-PI, PI]` — so a bearing that
/// crosses the ±PI seam (orientation 3.0, target bearing -3.0) reads as a small turn rather than a
/// near-full circle. Pure — unit-tested below.
fn angle_diff(from: f32, to: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    let mut d = (to - from) % two_pi;
    if d > std::f32::consts::PI {
        d -= two_pi;
    } else if d < -std::f32::consts::PI {
        d += two_pi;
    }
    d
}

/// CHASE — an engaged creature closes on the unit it fights. It commits to ONE long run leg and
/// rides it for several firings, re-aiming only when the leg lands or the victim veers off the
/// heading it was thrown along; an offensive caster holds at spell range instead of face-tanking;
/// and a creature that reaches a victim standing inside melee reach stops and turns to face it.
///
/// The engagements are the candidate set, so this phase ignores the active-cell sweep — a player
/// cannot freeze a fight by dragging it away — and stays O(fights running). A routing or
/// crowd-controlled creature is skipped, leaving the rout leg and fear as its sole movers, and every
/// chaser gets at most one leg per firing: a second would share the first's `spline_id` and the
/// client would throw it away. Visited in guid order. Returns the covered pursuits considered.
fn chase<W: PursuitSink + MotionSink + IdleSink + EngageSink>(
    w: &mut W,
    tick: &TickContext,
) -> usize {
    let mut pursuits = w.pursuits(&tick.scope);
    pursuits.sort_unstable_by_key(|c| c.guid);
    let visited = pursuits.len();
    for c in pursuits {
        if c.routing || w.movement_suppressed(c.guid) {
            continue;
        }
        let gap_sq = dist_sq(c.at, c.victim_at);
        // Past the pursuit cutoff the creature stops closing: the engagement's own timer ends the
        // fight, distance does not, and walking home is the return phase's job afterwards.
        if gap_sq > CHASE_LEASH_SQ {
            continue;
        }
        // A creature plants into attack stance only for a victim that STOPPED. It runs a kiter down
        // continuously — the swing pass hits on the move — which is what removes the
        // run/attack-stance/run flicker of chasing someone who never stands still.
        let victim_moving =
            tick.now_ms.wrapping_sub(c.victim_last_move_ms) < CHASE_TARGET_MOVING_MS;
        if gap_sq <= CHASE_MELEE_SQ && !victim_moving {
            stand_and_face(w, tick, &c);
            continue;
        }
        // An offensive caster stands and casts (the cast phase already fired this firing) while its
        // victim is inside spell range and in plain sight. A wall-blocked one closes instead, which
        // is the verdict the cast phase reached about that same line.
        let hold = w.caster_hold_range(c.guid) * CASTER_HOLD_MARGIN;
        if hold > 0.0 && gap_sq <= hold * hold && w.line_of_sight(c.guid, c.victim_at) {
            continue;
        }
        let gap = gap_sq.sqrt();
        let dir = (
            (c.victim_at.x - c.at.x) / gap,
            (c.victim_at.y - c.at.y) / gap,
        );
        // Aim PAST a moving victim, so one leg carries several firings of a straight-line kite; aim
        // at melee reach for a standing one, so the stand above ends the approach.
        let leg_len = if victim_moving {
            gap + CHASE_LEAD_YD
        } else {
            (gap - MELEE_STOP_SHORT_YD).max(0.0)
        };
        let aim = (c.at.x + dir.0 * leg_len, c.at.y + dir.1 * leg_len);
        let step = w.navigate(c.guid, aim, leg_len);
        if step == (c.at.x, c.at.y) || !needs_new_leg(c.leg.as_ref(), dir) {
            continue;
        }
        let run = w.speed_of(c.guid, Gait::Run);
        let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, run) else {
            continue; // nothing to close — the client rejects a zero-length leg
        };
        // The victim's height is only the FALLBACK: the writer snaps the landing point to the ground
        // under it, which matters on a slope and where the point is a detour corner nowhere near the
        // victim.
        let leg = Leg {
            to: (x, y),
            z_fallback: c.victim_at.z,
            dur_ms,
            gait: Gait::Run,
            hold_until_landed: false,
        };
        w.commit_leg(c.guid, leg, tick.now_ms);
    }
    visited
}

/// Does this chaser need a NEW leg? One whose leg has landed does. One still riding a leg keeps it
/// while the victim stays roughly on the heading the leg was thrown along, which is what stops the
/// client re-computing its path every firing.
fn needs_new_leg(leg: Option<&LegInFlight>, dir: (f32, f32)) -> bool {
    let Some(leg) = leg else {
        return true;
    };
    let (lx, ly) = (leg.dest.x - leg.start.x, leg.dest.y - leg.start.y);
    let len = (lx * lx + ly * ly).sqrt();
    len <= 0.001 || (lx / len) * dir.0 + (ly / len) * dir.1 < CHASE_REPATH_COS
}

/// The victim is inside melee reach and standing still. Stop the leg the chaser was riding, so a
/// lead aimed past a now-stopped victim does not carry it PAST them, and turn it to face what it is
/// swinging at — a stand-and-swing creature throws no further movement leg, and a spline is the only
/// thing a client derives creature facing from, so without this it keeps its pre-combat heading for
/// the whole fight. Both effects are idempotent — the stop reaps its own leg on the next advance,
/// the turn is epsilon-gated — so a settled fight goes silent.
fn stand_and_face<W: MotionSink + PursuitSink>(w: &mut W, tick: &TickContext, c: &Pursuit) {
    // ponytail: the stop reuses advance's `halt`, which also re-places the creature. Ceiling: one
    // row write of the position it already holds, each time a fight settles.
    if let Some(leg) = &c.leg {
        w.halt(leg, c.at, tick.now_ms);
    }
    let bearing = (c.victim_at.y - c.at.y).atan2(c.victim_at.x - c.at.x);
    if angle_diff(c.orientation, bearing).abs() > FACING_EPSILON_RAD {
        w.face(c.guid, bearing, tick.now_ms);
    }
}

/// COMBAT ENTRY — every unit in a live engagement this firing covers gets the in-combat flag and a
/// refreshed combat-drop deadline. It covers a pure caster too: casting at a creature makes the
/// creature retaliate, so the caster is the VICTIM of an engagement and is flagged from here.
///
/// The engaged set is small — it scales with the fights running, not with the world.
fn combat_entry<W: EngageSink>(w: &mut W, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let mut units: Vec<u64> = Vec::new();
    for e in w.engagements() {
        // Equivalence: with only the catch-all schedule row, every pair is covered.
        if !scope.covers(e.instance_id) || e.player_never_swung {
            continue;
        }
        visited += 1;
        units.push(e.attacker);
        units.push(e.victim);
    }
    units.sort_unstable();
    units.dedup();
    for guid in units {
        w.enter_combat(guid);
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

/// The facing correction's precise edges. A scenario test proves that a creature reaching a standing
/// victim turns to face it; these pin the seam-crossing and epsilon arithmetic that decides it,
/// which is cheaper and sharper to state directly than through a fight.
#[cfg(test)]
mod facing_tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    #[test]
    fn zero_drift_is_zero() {
        assert_eq!(angle_diff(1.0, 1.0), 0.0);
    }

    #[test]
    fn a_quarter_turn_reads_as_a_quarter_turn_either_direction() {
        assert!((angle_diff(0.0, FRAC_PI_2) - FRAC_PI_2).abs() < 1e-6);
        assert!((angle_diff(0.0, -FRAC_PI_2) - (-FRAC_PI_2)).abs() < 1e-6);
    }

    #[test]
    fn the_pi_seam_takes_the_short_way_round() {
        // orientation just past +PI, target bearing just past -PI: only ~0.2 rad apart going
        // "outward" across the seam, NOT the ~2*PI-0.2 rad the naive subtraction would give.
        let from = PI - 0.1;
        let to = -PI + 0.1;
        let d = angle_diff(from, to);
        assert!(
            d.abs() < 0.3,
            "expected a short turn across the seam, got {d}"
        );
    }

    #[test]
    fn a_full_turn_collapses_to_no_turn() {
        assert!(angle_diff(0.5, 0.5 + TAU).abs() < 1e-4);
    }

    #[test]
    fn the_epsilon_gate_is_silent_below_threshold_and_fires_above_it() {
        let orientation = 0.0_f32;
        let just_under = FACING_EPSILON_RAD - 0.01;
        let just_over = FACING_EPSILON_RAD + 0.01;
        assert!(angle_diff(orientation, just_under).abs() <= FACING_EPSILON_RAD);
        assert!(angle_diff(orientation, just_over).abs() > FACING_EPSILON_RAD);
    }
}
