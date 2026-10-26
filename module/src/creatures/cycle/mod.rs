use std::collections::HashSet;

use lyracore_shared::constants;
use spacetimedb::log;

use super::ai::{
    aggro_radius, fear_step_for_tick, feared_flee_step, finite_point, flee_step, hp_pct_below,
    is_aggro_candidate, leg_in_flight, leg_toward, may_start_rout, nearest_waypoint_idx,
    next_waypoint_idx, rout_close_ms, spline_t, stealth_detect_range, wander_point,
    within_assist_radius, wounded_slow_factor, TickScope, CHASE_LEASH_SQ, CHASE_MELEE_SQ,
    CHASE_REPATH_COS, FLEE_LEG_YD, MOVE_TICK_SECS, RETURN_LEASH_SQ, SENSE_EVERY_N_TICKS,
    WANDER_CHANCE_PCT, WANDER_RADIUS,
};
use super::cast_condition;
use super::eventai::EventAiRequest;
use super::pet;
use super::tick::TickSweep;

mod ctx;
#[cfg(test)]
mod harness;

pub(crate) use ctx::{run, stop_on_stored_leg};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Point {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

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
    pub back_off: bool,
    pub waypoints: Vec<(f32, f32, f32)>,
}

impl LegInFlight {
    fn rendered_at(&self, now_micros: u64) -> (Point, bool) {
        let t = spline_t(now_micros, self.started_micros, self.dur_ms);
        if !self.waypoints.is_empty() {
            let (at, _) = lyracore_shared::movement_path::sample(
                (self.start.x, self.start.y, self.start.z),
                &self.waypoints,
                t,
            );
            return (
                Point {
                    x: at.0,
                    y: at.1,
                    z: at.2,
                },
                t >= 1.0,
            );
        }
        let at = Point {
            x: self.start.x + (self.dest.x - self.start.x) * t,
            y: self.start.y + (self.dest.y - self.start.y) * t,
            z: self.start.z + (self.dest.z - self.start.z) * t,
        };
        (at, t >= 1.0)
    }

    fn heading_on_path(&self, at: Point, now_micros: u64) -> Option<f32> {
        let (_, next) = lyracore_shared::movement_path::sample(
            (self.start.x, self.start.y, self.start.z),
            &self.waypoints,
            spline_t(now_micros, self.started_micros, self.dur_ms),
        );
        self.waypoints
            .get(next)
            .map(|p| (p.1 - at.y).atan2(p.0 - at.x))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Stop {
    pub at: Point,
    pub heading: Option<f32>,
}

pub(crate) fn stop_on_leg(w: &impl MotionSink, leg: &LegInFlight, now_micros: u64) -> Option<Stop> {
    let (at, _) = leg.rendered_at(now_micros);
    if let Some(halted) = w.path_obstruction(leg, now_micros) {
        return Some(Stop {
            at: halted,
            heading: None,
        });
    }
    finite_point(at.x, at.y, at.z).then(|| Stop {
        at,
        heading: leg.heading_on_path(at, now_micros),
    })
}

pub(crate) struct TickContext {
    pub now_micros: u64,
    pub now_ms: u32,
    pub tick_secs: f32,
    pub sense: bool,
    pub sense_secs: f32,
    pub scope: TickScope,
}

pub(crate) struct CycleOutcome {
    pub awake: usize,
    pub rows_visited: Vec<(&'static str, u64)>,
}

pub(crate) trait MotionSink {
    fn legs_in_flight(&self, scope: &TickScope) -> Vec<LegInFlight>;
    fn path_obstruction(&self, leg: &LegInFlight, now_micros: u64) -> Option<Point>;
    fn movement_suppressed(&self, guid: u64) -> bool;
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32);
    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32);
    fn drop_leg(&mut self, guid: u64);
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct IdleCreature {
    pub guid: u64,
    pub at: Point,
    pub leg_ends_ms: u32,
    pub wp_target: u64,
    pub patrols: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Waypoint {
    pub id: u64,
    pub at: Point,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Home {
    pub at: Point,
    pub wanders: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Gait {
    Walk,
    Run,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Leg {
    pub to: (f32, f32),
    pub z_fallback: f32,
    pub dur_ms: u32,
    pub gait: Gait,
    pub hold_until_landed: bool,
}

pub(crate) trait IdleSink {
    fn idle_creatures(&self, active: &HashSet<u64>) -> Vec<IdleCreature>;
    fn route_of(&self, guid: u64) -> Vec<Waypoint>;
    fn patrol_paused(&self, _guid: u64) -> bool {
        false
    }
    fn idle_stationary(&self, _guid: u64) -> bool {
        false
    }
    fn returning_home(&self, _guid: u64) -> bool {
        false
    }
    fn follow_target_at(&self, _guid: u64) -> Option<Point> {
        None
    }
    fn pending_facing(&self, _guid: u64) -> Option<f32> {
        None
    }
    fn clear_pending_facing(&mut self, _guid: u64) {}
    fn idle_gait(&self, _guid: u64, default: Gait) -> Gait {
        default
    }
    fn wander_radius(&self, _guid: u64) -> f32 {
        WANDER_RADIUS
    }
    fn home_of(&self, guid: u64) -> Option<Home>;
    fn return_home_of(&self, guid: u64) -> Option<Home> {
        self.home_of(guid)
    }
    fn engaged(&self, guid: u64) -> bool;
    fn speed_of(&self, guid: u64, gait: Gait) -> f32;
    fn navigate(&self, guid: u64, to: (f32, f32), max_step: f32, stop_dist: f32) -> (f32, f32);
    fn roll(&self) -> u32;
    fn aim_at_waypoint(&mut self, guid: u64, waypoint_id: u64);
    fn reached_home(&mut self, guid: u64);
    fn commit_leg(&mut self, guid: u64, leg: Leg, now_ms: u32);
}

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
    pub stealthed: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Sensor {
    pub guid: u64,
    pub at: Point,
    pub level: u32,
    pub faction_template: u32,
    pub map_id: u32,
    pub instance_id: u64,
    pub aggro_range: Option<u32>,
    pub detect_range_mod: f32,
    pub would_rout: bool,
    pub cannot_act: bool,
    pub react_state: crate::creatures::CreatureReactState,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Pull {
    Noticed,
    Assisted,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Engagement {
    pub attacker: u64,
    pub victim: u64,
    pub instance_id: u64,
    pub player_never_swung: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Combatant {
    pub guid: u64,
    pub combat_until_ms: u64,
}

pub(crate) trait EngageSink {
    fn players(&self) -> Vec<AggroTarget>;
    fn sensing_creatures(&self, active: &HashSet<u64>) -> Vec<Sensor>;
    fn hostile(&self, faction_template: u32, other: u32) -> bool;
    fn line_of_sight(&self, looker: u64, at: Point) -> bool;
    fn engage(&mut self, creature: u64, victim: u64, pull: Pull);
    fn engagements(&self) -> Vec<Engagement>;
    fn enter_combat(&mut self, guid: u64);
    fn flagged_in_combat(&self, candidates: &[u64]) -> Vec<Combatant>;
    fn leave_combat(&mut self, guid: u64);
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Recovering {
    pub guid: u64,
    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    pub in_combat: bool,
}

pub(crate) trait RegenSink {
    fn recovering(&self, flagged: &[u64]) -> Vec<Recovering>;
    fn healed_to(&self, u: &Recovering) -> u32;
    fn combat_healed_to(&self, u: &Recovering) -> Option<u32>;
    fn powered_to(&self, u: &Recovering) -> u32;
    fn restore(&mut self, guid: u64, health: Option<u32>, power: Option<u32>);
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Caster {
    pub guid: u64,
    pub victim: u64,
    pub victim_at: Option<Point>,
    pub level: u8,
    pub health: u32,
    pub max_health: u32,
    pub cannot_act: bool,
    pub casting: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CastWhen {
    Always,
    Hurt(u32),
    VictimLacksIt,
    SelfLacksIt,
    Never,
}

impl CastWhen {
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

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SpellOption {
    pub spell_id: u32,
    pub when: CastWhen,
    pub priority: u8,
    pub authored: u64,
}

pub(crate) trait CastSink {
    fn casters(&self, scope: &TickScope) -> Vec<Caster>;
    fn rotation_of(&self, guid: u64) -> Vec<SpellOption>;
    fn creature_spell_list_of(&self, _guid: u64) -> Vec<SpellOption> {
        Vec::new()
    }
    fn lone_spell(&self, guid: u64) -> Option<u32>;
    fn carries(&self, guid: u64, spell_id: u32) -> bool;
    fn begin_cast(&mut self, caster: &Caster, spell_id: u32, target: u64) -> bool;
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Fighter {
    pub guid: u64,
    pub victim: u64,
}

pub(crate) trait ThreatSink {
    fn fighters(&self, scope: &TickScope) -> Vec<Fighter>;
    fn taunted_onto(&self, guid: u64) -> Option<u64>;
    fn top_threat(&self, guid: u64) -> Option<u64>;
    fn threat_on(&self, creature: u64, source: u64) -> i64;
    fn retarget(&mut self, creature: u64, victim: u64);
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Pursuit {
    pub guid: u64,
    pub at: Point,
    pub orientation: f32,
    pub victim_at: Point,
    pub victim_orientation: f32,
    pub victim_movement_flags: u32,
    pub victim_is_character: bool,
    pub last_move_ms: u32,
    pub routing: bool,
    pub leg: Option<LegInFlight>,
}

pub(crate) trait PursuitSink {
    fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit>;
    fn authored_ranged_posture(&self, guid: u64) -> Option<(f32, f32)>;
    fn combat_movement_enabled(&self, _guid: u64) -> bool {
        true
    }
    fn chase_gait(&self, _guid: u64) -> Gait {
        Gait::Run
    }
    fn target_not_reachable(&mut self, _guid: u64) {}
    fn caster_hold_range(&self, guid: u64) -> f32;
    fn casting(&self, guid: u64) -> bool;
    fn commit_back_off(&mut self, guid: u64, leg: Leg, now_ms: u32);
    fn face(&mut self, guid: u64, at: Point, orientation: f32, spline_id: u32);
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Router {
    pub guid: u64,
    pub at: Point,
    pub victim: u64,
    pub victim_at: Option<Point>,
    pub health: u32,
    pub max_health: u32,
    pub eligible: bool,
    pub rout_ends_ms: u32,
    pub routing: bool,
    pub committed: bool,
}

pub(crate) trait RoutSink {
    fn routers(&self, scope: &TickScope) -> Vec<Router>;
    fn start_rout(&mut self, guid: u64, ends_ms: u32);
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Panicked {
    pub guid: u64,
    pub at: Point,
    pub source_at: Option<Point>,
    pub frozen: bool,
}

pub(crate) trait FearSink {
    fn panicked(&self, scope: &TickScope) -> Vec<Panicked>;
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pet {
    pub guid: u64,
    pub at: Point,
    pub map_id: u32,
    pub instance_id: u64,
    pub owner: Option<PetOwner>,
    pub dead: bool,
    pub suppressed: bool,
    pub command: PetCommand,
    pub react: PetReact,
    pub fighting: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct PetOwner {
    pub guid: u64,
    pub at: Point,
    pub map_id: u32,
    pub instance_id: u64,
    pub combat_target: u64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum PetCommand {
    Stay,
    Follow,
    Attack(u64),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum PetReact {
    Passive,
    Defensive,
    Aggressive,
}

impl PetCommand {
    fn of(command: u8, command_target: u64) -> PetCommand {
        match command {
            pet::CMD_STAY => PetCommand::Stay,
            pet::CMD_ATTACK => PetCommand::Attack(command_target),
            _ => PetCommand::Follow,
        }
    }
}

impl PetReact {
    fn of(react: u8) -> PetReact {
        match react {
            pet::REACT_PASSIVE => PetReact::Passive,
            pet::REACT_AGGRESSIVE => PetReact::Aggressive,
            _ => PetReact::Defensive,
        }
    }
}

pub(crate) trait PetSink {
    fn pets(&self, scope: &TickScope, candidates: &[u64]) -> Vec<Pet>;
    fn pet_may_attack(&self, pet: &Pet, target: u64) -> bool;
    fn nearest_hostile_to(&self, pet: &Pet) -> Option<u64>;
    fn cancel_attack_order(&mut self, owner: u64);
    fn take_victim(&mut self, pet: u64, victim: u64);
    fn stand_down(&mut self, pet: u64);
    fn dismiss(&mut self, pet: u64);
    fn restage(&mut self, pet: u64, at: Point, map_id: u32, instance_id: u64, now_ms: u32);
}

pub(crate) trait CreatureWorld:
    MotionSink
    + IdleSink
    + EngageSink
    + CastSink
    + ThreatSink
    + PursuitSink
    + RoutSink
    + FearSink
    + PetSink
    + RegenSink
{
    fn awake_creatures(&self, scope: &TickScope, sense: bool) -> TickSweep;
    fn run_due_world_maintenance(&mut self) -> Vec<(&'static str, u64)>;
    fn evaluate_eventai(&mut self, request: EventAiRequest<'_>) -> u64;
    fn run_package_passes(&mut self);
}

pub(crate) fn run_cycle<W: CreatureWorld>(w: &mut W, tick: TickContext) -> CycleOutcome {
    let TickSweep {
        active,
        pets,
        in_combat,
    } = w.awake_creatures(&tick.scope, tick.sense);
    let global = tick.scope.runs_global_passes();
    let mut rows: Vec<(&'static str, u64)> = Vec::new();

    rows.push(("advance", advance_legs(w, &tick) as u64));
    rows.push(("patrol", patrol(w, &tick, &active) as u64));
    if global {
        rows.extend(w.run_due_world_maintenance());
    }
    if tick.sense {
        let (pulls, seen) = aggro(w, &active);
        rows.push(("aggro", seen as u64));
        rows.push(("assist", assist(w, &active, pulls) as u64));
        rows.push(("pet", pet_behavior(w, &tick, &pets) as u64));
        rows.push((
            "eventai",
            w.evaluate_eventai(EventAiRequest::Cycle {
                scope: &tick.scope,
                active: &active,
            }),
        ));
        rows.push(("cast", cast(w, &tick.scope) as u64));
        rows.push(("threat_retarget", threat_retarget(w, &tick.scope) as u64));
    }
    rows.push(("chase", chase(w, &tick) as u64));
    rows.push(("combat_enter", combat_entry(w, &tick.scope) as u64));
    rows.push(("idle", idle_movement(w, &tick, &active) as u64));
    if tick.sense && global {
        rows.push(("regen*", regenerate(w, &in_combat) as u64));
        rows.push(("combat_drop*", combat_exit(w, &tick, &in_combat) as u64));
    }
    rows.push(("rout", rout(w, &tick) as u64));
    rows.push(("fear", fear(w, &tick) as u64));
    if !tick.sense {
        rows.push((
            "eventai",
            w.evaluate_eventai(EventAiRequest::Cycle {
                scope: &tick.scope,
                active: &active,
            }),
        ));
    }
    if global {
        w.run_package_passes();
    }

    CycleOutcome {
        awake: active.len(),
        rows_visited: rows,
    }
}

fn advance_legs<W: MotionSink>(w: &mut W, tick: &TickContext) -> usize {
    let mut visited = 0usize;
    for leg in w.legs_in_flight(&tick.scope) {
        if leg.mover_gone {
            w.drop_leg(leg.guid);
            continue;
        }
        if leg.start == leg.dest && leg.waypoints.is_empty() {
            w.drop_leg(leg.guid);
            visited += 1;
            continue;
        }
        let (at, arrived) = leg.rendered_at(tick.now_micros);
        if let Some(stopped) = w.path_obstruction(&leg, tick.now_micros) {
            w.halt(&leg, stopped, tick.now_ms);
            continue;
        }
        if !finite_point(at.x, at.y, at.z) {
            log::error!(
                "refused a non-finite spline advance for guid {} — dropping the leg",
                leg.guid
            );
            w.drop_leg(leg.guid);
            continue;
        }
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

// The client refuses zero-length legs below the waypoint arrival threshold.
const WAYPOINT_ARRIVE_YD: f32 = 0.5;

const PATROL_DISPLACED_SQ: f32 = 1600.0;

// Wander legs span the sense cadence, rather than one movement firing.
const WANDER_HOP_SECS: f32 = MOVE_TICK_SECS * SENSE_EVERY_N_TICKS as f32;

fn idle_order(w: &impl IdleSink, active: &HashSet<u64>) -> Vec<IdleCreature> {
    let mut candidates = w.idle_creatures(active);
    candidates.sort_unstable_by_key(|c| c.guid);
    candidates
}

fn moves_itself<W: IdleSink + MotionSink>(w: &W, guid: u64) -> bool {
    !w.engaged(guid) && !w.movement_suppressed(guid)
}

fn patrol<W: IdleSink + MotionSink>(w: &mut W, tick: &TickContext, active: &HashSet<u64>) -> usize {
    let mut visited = 0usize;
    for c in idle_order(w, active) {
        if !c.patrols {
            continue;
        }
        visited += 1;
        if !moves_itself(w, c.guid)
            || w.patrol_paused(c.guid)
            || w.returning_home(c.guid)
            || w.pending_facing(c.guid).is_some()
            || w.follow_target_at(c.guid).is_some()
            || leg_in_flight(tick.now_ms, c.leg_ends_ms)
        {
            continue;
        }
        let mut route = w.route_of(c.guid);
        if route.len() < 2 {
            continue;
        }
        route.sort_unstable_by_key(|wp| wp.id);
        let points: Vec<(f32, f32)> = route.iter().map(|wp| (wp.at.x, wp.at.y)).collect();
        let cur = route.iter().position(|wp| wp.id == c.wp_target);
        let next = match cur {
            Some(i) if dist_sq(route[i].at, c.at) >= WAYPOINT_ARRIVE_YD * WAYPOINT_ARRIVE_YD => i,
            Some(i) => next_waypoint_idx(i, route.len()),
            None => nearest_waypoint_idx(c.at.x, c.at.y, &points),
        };
        let wp = route[next];
        w.aim_at_waypoint(c.guid, wp.id);
        let (dx, dy, dz) = (wp.at.x - c.at.x, wp.at.y - c.at.y, wp.at.z - c.at.z);
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq < WAYPOINT_ARRIVE_YD * WAYPOINT_ARRIVE_YD {
            continue;
        }
        let leg = if dist_sq <= PATROL_DISPLACED_SQ {
            let gait = w.idle_gait(c.guid, Gait::Walk);
            let speed = w.speed_of(c.guid, gait);
            Leg {
                to: (wp.at.x, wp.at.y),
                z_fallback: wp.at.z,
                dur_ms: (dist_sq.sqrt() / speed * 1000.0) as u32,
                gait,
                hold_until_landed: true,
            }
        } else {
            let gait = w.idle_gait(c.guid, Gait::Walk);
            let speed = w.speed_of(c.guid, gait);
            let step = w.navigate(c.guid, (wp.at.x, wp.at.y), speed * tick.tick_secs, 0.0);
            let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, speed) else {
                continue;
            };
            Leg {
                to: (x, y),
                z_fallback: wp.at.z,
                dur_ms,
                gait,
                hold_until_landed: false,
            }
        };
        w.commit_leg(c.guid, leg, tick.now_ms);
    }
    visited
}

#[derive(Clone, Copy)]
struct Called {
    guid: u64,
    at: Point,
    map_id: u32,
    instance_id: u64,
    faction_template: u32,
    victim: u64,
}

fn sensing_order(w: &impl EngageSink, active: &HashSet<u64>) -> Vec<Sensor> {
    let mut candidates = w.sensing_creatures(active);
    candidates.sort_unstable_by_key(|c| c.guid);
    candidates
}

// Decide every pull before hooks run Package code that can change the candidates.
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
        if s.would_rout
            || s.cannot_act
            || s.react_state != crate::creatures::CreatureReactState::Aggressive
        {
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

fn nearest_noticed<W: EngageSink>(w: &W, s: &Sensor, targets: &[AggroTarget]) -> Option<u64> {
    let tuned = s.aggro_range?;
    targets
        .iter()
        .filter(|p| p.map_id == s.map_id && p.instance_id == s.instance_id)
        .filter_map(|p| {
            let radius = (aggro_radius(s.level, p.level, tuned) + s.detect_range_mod).max(0.0);
            if radius <= 0.0 {
                return None;
            }
            let d2 = dist_sq(s.at, p.at);
            if d2 > radius * radius {
                return None;
            }
            if p.stealthed && d2 > stealth_detect_range(s.level, p.level).powi(2) {
                return None;
            }
            if !w.hostile(s.faction_template, p.faction_template) {
                return None;
            }
            if !w.line_of_sight(s.guid, p.at) {
                return None;
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

fn assist<W: EngageSink>(w: &mut W, active: &HashSet<u64>, mut calls: Vec<Called>) -> usize {
    if calls.is_empty() {
        return 0;
    }
    calls.sort_unstable_by_key(|c| c.guid);
    let neighbors = sensing_order(w, active);
    let mut answered = Vec::new();
    for n in &neighbors {
        if n.would_rout
            || n.cannot_act
            || n.react_state == crate::creatures::CreatureReactState::Passive
        {
            continue;
        }
        let call = calls.iter().find(|c| {
            c.guid != n.guid
                && c.map_id == n.map_id
                && c.faction_template == n.faction_template
                && c.instance_id == n.instance_id
                && within_assist_radius(c.at.x, c.at.y, n.at.x, n.at.y)
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

const PET_FOLLOW_BAND_SQ: f32 = 64.0;

const PET_TRAIL_YD: f32 = 4.0;

const _: () = assert!(
    PET_FOLLOW_BAND_SQ > CHASE_MELEE_SQ,
    "the follow band must sit outside melee range, or a trailing pet jitters at the boundary"
);

enum PetAction {
    Despawn,
    Engage(u64),
    StandDown,
    Restage {
        at: Point,
        map_id: u32,
        instance_id: u64,
    },
    Follow(Leg),
}

fn pet_behavior<W: PetSink + IdleSink>(w: &mut W, tick: &TickContext, candidates: &[u64]) -> usize {
    let pets = w.pets(&tick.scope, candidates);
    let visited = pets.len();
    let mut actions: Vec<(u64, PetAction)> = Vec::new();
    for pet in &pets {
        if pet.dead {
            actions.push((pet.guid, PetAction::Despawn));
            continue;
        }
        if pet.suppressed {
            continue;
        }
        let Some(owner) = pet.owner else {
            actions.push((pet.guid, PetAction::Despawn));
            continue;
        };
        if let Some(victim) = pet_victim(w, pet, &owner) {
            actions.push((pet.guid, PetAction::Engage(victim)));
            continue;
        }
        if pet.fighting {
            actions.push((pet.guid, PetAction::StandDown));
            continue;
        }
        if pet.command == PetCommand::Stay {
            continue;
        }
        if let Some(action) = follow(w, tick, pet, &owner) {
            actions.push((pet.guid, action));
        }
    }
    for (guid, action) in actions {
        match action {
            PetAction::Despawn => w.dismiss(guid),
            PetAction::Engage(victim) => w.take_victim(guid, victim),
            PetAction::StandDown => w.stand_down(guid),
            PetAction::Restage {
                at,
                map_id,
                instance_id,
            } => w.restage(guid, at, map_id, instance_id, tick.now_ms),
            PetAction::Follow(leg) => w.commit_leg(guid, leg, tick.now_ms),
        }
    }
    visited
}

fn pet_victim<W: PetSink>(w: &mut W, pet: &Pet, owner: &PetOwner) -> Option<u64> {
    if let PetCommand::Attack(target) = pet.command {
        if w.pet_may_attack(pet, target) {
            return Some(target);
        }
        w.cancel_attack_order(owner.guid);
    }
    if pet.react == PetReact::Passive {
        return None;
    }
    if w.pet_may_attack(pet, owner.combat_target) {
        return Some(owner.combat_target);
    }
    match pet.react {
        PetReact::Aggressive => w.nearest_hostile_to(pet),
        _ => None,
    }
}

fn follow<W: IdleSink>(
    w: &W,
    tick: &TickContext,
    pet: &Pet,
    owner: &PetOwner,
) -> Option<PetAction> {
    if pet.map_id != owner.map_id || pet.instance_id != owner.instance_id {
        return Some(PetAction::Restage {
            at: owner.at,
            map_id: owner.map_id,
            instance_id: owner.instance_id,
        });
    }
    if dist_sq(pet.at, owner.at) <= PET_FOLLOW_BAND_SQ {
        return None;
    }
    let (dx, dy) = (owner.at.x - pet.at.x, owner.at.y - pet.at.y);
    let gap = (dx * dx + dy * dy).sqrt();
    if gap <= PET_TRAIL_YD {
        return None;
    }
    let leg_len = gap - PET_TRAIL_YD;
    let aim = (pet.at.x + dx / gap * leg_len, pet.at.y + dy / gap * leg_len);
    let run = w.speed_of(pet.guid, Gait::Run);
    let step = w.navigate(pet.guid, aim, run * tick.sense_secs, 0.0);
    let (x, y, dur_ms) = leg_toward((pet.at.x, pet.at.y), step, run)?;
    Some(PetAction::Follow(Leg {
        to: (x, y),
        z_fallback: owner.at.z,
        dur_ms,
        gait: Gait::Run,
        hold_until_landed: false,
    }))
}

fn cast<W: CastSink + EngageSink>(w: &mut W, scope: &TickScope) -> usize {
    let mut casters = w.casters(scope);
    casters.sort_unstable_by_key(|c| c.guid);
    let visited = casters.len();
    // Choose every cast before applying one so earlier effects cannot change later choices.
    let mut ready: Vec<(Caster, Vec<(u32, u64)>)> = Vec::new();
    for c in casters {
        if c.cannot_act || c.casting {
            continue;
        }
        let mut rotation = w.rotation_of(c.guid);
        rotation.extend(w.creature_spell_list_of(c.guid));
        let mut candidates: Vec<(u32, u64)> = if rotation.is_empty() {
            match w.lone_spell(c.guid) {
                Some(spell_id) => vec![(spell_id, c.victim)],
                None => continue,
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
                break;
            }
        }
    }
    visited
}

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

fn threat_retarget<W: ThreatSink>(w: &mut W, scope: &TickScope) -> usize {
    let fighters = w.fighters(scope);
    let visited = fighters.len();
    let mut switches: Vec<(u64, u64)> = Vec::new();
    for f in &fighters {
        if let Some(pinned) = w.taunted_onto(f.guid) {
            if pinned != f.victim {
                switches.push((f.guid, pinned));
            }
            continue;
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

const BOUNDING_RADIUS_YD: f32 = 0.389;

// Two default bounding radii plus the 0.5 yd vanilla contact distance.
const CHASE_CONTACT_YD: f32 = 2.0 * BOUNDING_RADIUS_YD + 0.5;

const OVERLAP_YD: f32 = 2.0 * BOUNDING_RADIUS_YD;

// CHASE_CLOSENESS_TIMER in cmangos ChaseMovementGenerator::Backpedal.
const BACK_OFF_AFTER_STILL_MS: u32 = 2_000;

const _: () = assert!(
    CHASE_CONTACT_YD * CHASE_CONTACT_YD < CHASE_MELEE_SQ,
    "a chaser must stop inside the reach its own swing needs"
);

const CASTER_HOLD_MARGIN: f32 = 0.9;

const RANGED_HOLD_ARRIVED_SQ: f32 = 0.25;

const FACING_EPSILON_RAD: f32 = 0.3;

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

fn chase<W: PursuitSink + MotionSink + IdleSink + EngageSink>(
    w: &mut W,
    tick: &TickContext,
) -> usize {
    let mut pursuits = w.pursuits(&tick.scope);
    pursuits.sort_unstable_by_key(|c| c.guid);
    let visited = pursuits.len();
    for c in pursuits {
        if c.routing || w.movement_suppressed(c.guid) || !w.combat_movement_enabled(c.guid) {
            continue;
        }
        let gap_sq = dist_sq(c.at, c.victim_at);
        if gap_sq > CHASE_LEASH_SQ {
            continue;
        }
        let posture = w.authored_ranged_posture(c.guid);
        if let Some(aim) = posture.and_then(|(distance, angle)| {
            (distance > 0.0 && w.line_of_sight(c.guid, c.victim_at))
                .then(|| ranged_hold_point(&c, distance, angle))
        }) {
            if dist_sq(c.at, aim) <= RANGED_HOLD_ARRIVED_SQ {
                stand_and_face(w, tick, &c);
            } else {
                commit_chase_leg(w, &c, aim, 0.0, tick.now_ms);
            }
            continue;
        }
        let victim_moving = crate::combat::is_translating(c.victim_movement_flags);
        let in_contact = gap_sq <= CHASE_CONTACT_YD * CHASE_CONTACT_YD;
        if in_contact || (gap_sq <= CHASE_MELEE_SQ && !victim_moving) {
            let riding_back_off =
                c.leg.as_ref().is_some_and(|leg| leg.back_off) && !w.casting(c.guid);
            if !riding_back_off && !back_off(w, tick, &c, gap_sq) {
                stand_and_face(w, tick, &c);
            }
            continue;
        }
        if posture.is_none() {
            let hold = w.caster_hold_range(c.guid) * CASTER_HOLD_MARGIN;
            if hold > 0.0 && gap_sq <= hold * hold && w.line_of_sight(c.guid, c.victim_at) {
                continue;
            }
        }
        commit_chase_leg(w, &c, c.victim_at, CHASE_CONTACT_YD, tick.now_ms);
    }
    visited
}

fn commit_chase_leg<W: MotionSink + IdleSink + PursuitSink>(
    w: &mut W,
    c: &Pursuit,
    target: Point,
    stop_dist: f32,
    now_ms: u32,
) {
    let gap = dist_sq(c.at, target).sqrt();
    if gap <= stop_dist {
        return;
    }
    let dir = ((target.x - c.at.x) / gap, (target.y - c.at.y) / gap);
    let step = w.navigate(c.guid, (target.x, target.y), gap, stop_dist);
    if step == (c.at.x, c.at.y) {
        w.target_not_reachable(c.guid);
        return;
    }
    let stop_at = (target.x - dir.0 * stop_dist, target.y - dir.1 * stop_dist);
    if !needs_new_leg(c.leg.as_ref(), dir, stop_at) {
        return;
    }
    let gait = w.chase_gait(c.guid);
    let speed = w.speed_of(c.guid, gait);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, speed) else {
        return;
    };
    w.commit_leg(
        c.guid,
        Leg {
            to: (x, y),
            z_fallback: target.z,
            dur_ms,
            gait,
            hold_until_landed: false,
        },
        now_ms,
    );
}

// cmangos ChaseMovementGenerator::Backpedal, measured with our contact and overlap distances.
fn back_off<W: IdleSink + PursuitSink>(
    w: &mut W,
    tick: &TickContext,
    c: &Pursuit,
    gap_sq: f32,
) -> bool {
    let still_ms = tick.now_ms.wrapping_sub(c.last_move_ms);
    if !c.victim_is_character
        || c.leg.is_some()
        || still_ms < BACK_OFF_AFTER_STILL_MS
        || gap_sq >= OVERLAP_YD * OVERLAP_YD
        || w.casting(c.guid)
    {
        return false;
    }
    let (dx, dy) = (c.at.x - c.victim_at.x, c.at.y - c.victim_at.y);
    let len = (dx * dx + dy * dy).sqrt();
    let away = if len > 0.0 {
        (dx / len, dy / len)
    } else {
        (-c.orientation.cos(), -c.orientation.sin())
    };
    let to = (
        c.victim_at.x + CHASE_CONTACT_YD * away.0,
        c.victim_at.y + CHASE_CONTACT_YD * away.1,
    );
    let gap = ((to.0 - c.at.x).powi(2) + (to.1 - c.at.y).powi(2)).sqrt();
    let step = w.navigate(c.guid, to, gap, 0.0);
    let speed = w.speed_of(c.guid, Gait::Walk);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, speed) else {
        return false;
    };
    w.commit_back_off(
        c.guid,
        Leg {
            to: (x, y),
            z_fallback: c.at.z,
            dur_ms,
            gait: Gait::Walk,
            hold_until_landed: false,
        },
        tick.now_ms,
    );
    true
}

fn ranged_hold_point(pursuit: &Pursuit, distance: f32, angle: f32) -> Point {
    let radial = if angle == 0.0 {
        (pursuit.at.y - pursuit.victim_at.y).atan2(pursuit.at.x - pursuit.victim_at.x)
    } else {
        pursuit.victim_orientation + angle
    };
    Point {
        x: pursuit.victim_at.x + distance * radial.cos(),
        y: pursuit.victim_at.y + distance * radial.sin(),
        z: pursuit.victim_at.z,
    }
}

fn needs_new_leg(leg: Option<&LegInFlight>, dir: (f32, f32), stop_at: (f32, f32)) -> bool {
    let Some(leg) = leg else {
        return true;
    };
    if (leg.dest.x - stop_at.0) * dir.0 + (leg.dest.y - stop_at.1) * dir.1 > 0.0 {
        return true;
    }
    let (lx, ly) = (leg.dest.x - leg.start.x, leg.dest.y - leg.start.y);
    let len = (lx * lx + ly * ly).sqrt();
    len <= 0.001 || (lx / len) * dir.0 + (ly / len) * dir.1 < CHASE_REPATH_COS
}

fn stand_and_face<W: PursuitSink>(w: &mut W, tick: &TickContext, c: &Pursuit) {
    let bearing = (c.victim_at.y - c.at.y).atan2(c.victim_at.x - c.at.x);
    let turning = angle_diff(c.orientation, bearing).abs() > FACING_EPSILON_RAD;
    if c.leg.is_none() && !turning {
        return;
    }
    w.face(c.guid, c.at, bearing, tick.now_ms);
}

fn combat_entry<W: EngageSink>(w: &mut W, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let mut units: Vec<u64> = Vec::new();
    for e in w.engagements() {
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

fn idle_movement<W: IdleSink + MotionSink + PursuitSink>(
    w: &mut W,
    tick: &TickContext,
    active: &HashSet<u64>,
) -> usize {
    let mut visited = 0usize;
    for c in idle_order(w, active) {
        visited += 1;
        if !moves_itself(w, c.guid) {
            continue;
        }
        if w.returning_home(c.guid) {
            let Some(home) = w.return_home_of(c.guid) else {
                continue;
            };
            let (hdx, hdy) = (home.at.x - c.at.x, home.at.y - c.at.y);
            if hdx * hdx + hdy * hdy > RETURN_LEASH_SQ {
                walk_home(w, tick, &c, home);
            } else {
                w.reached_home(c.guid);
            }
            continue;
        }
        if let Some(orientation) = w.pending_facing(c.guid) {
            w.face(c.guid, c.at, orientation, tick.now_ms);
            w.clear_pending_facing(c.guid);
            continue;
        }
        if let Some(target) = w.follow_target_at(c.guid) {
            follow_authored(w, tick, &c, target);
            continue;
        }
        if c.patrols || w.idle_stationary(c.guid) {
            continue;
        }
        let Some(home) = w.home_of(c.guid) else {
            continue;
        };
        let (hdx, hdy) = (home.at.x - c.at.x, home.at.y - c.at.y);
        if hdx * hdx + hdy * hdy > RETURN_LEASH_SQ {
            walk_home(w, tick, &c, home);
            continue;
        }
        if tick.sense {
            w.reached_home(c.guid);
            loiter(w, tick, &c, home);
        }
    }
    visited
}

const FOLLOW_ARRIVE_YD: f32 = 3.0;

fn follow_authored<W: IdleSink>(w: &mut W, tick: &TickContext, c: &IdleCreature, target: Point) {
    if leg_in_flight(tick.now_ms, c.leg_ends_ms) {
        return;
    }
    let distance_sq = dist_sq(c.at, target);
    if distance_sq <= FOLLOW_ARRIVE_YD * FOLLOW_ARRIVE_YD {
        return;
    }
    let distance = distance_sq.sqrt();
    let gait = w.idle_gait(c.guid, Gait::Walk);
    let speed = w.speed_of(c.guid, gait);
    let max_step = (distance - FOLLOW_ARRIVE_YD).min(speed * tick.tick_secs);
    let step = w.navigate(c.guid, (target.x, target.y), max_step, 0.0);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, speed) else {
        return;
    };
    w.commit_leg(
        c.guid,
        Leg {
            to: (x, y),
            z_fallback: target.z,
            dur_ms,
            gait,
            hold_until_landed: false,
        },
        tick.now_ms,
    );
}

fn walk_home<W: IdleSink>(w: &mut W, tick: &TickContext, c: &IdleCreature, home: Home) {
    let run = w.speed_of(c.guid, Gait::Run);
    let step = w.navigate(c.guid, (home.at.x, home.at.y), run * tick.tick_secs, 0.0);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, run) else {
        return;
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

fn loiter<W: IdleSink>(w: &mut W, tick: &TickContext, c: &IdleCreature, home: Home) {
    if !home.wanders || leg_in_flight(tick.now_ms, c.leg_ends_ms) {
        return;
    }
    if w.roll() % 100 >= WANDER_CHANCE_PCT {
        return;
    }
    let radius = w.wander_radius(c.guid);
    let (hx, hy) = wander_point(home.at.x, home.at.y, w.roll(), w.roll(), radius);
    let gait = w.idle_gait(c.guid, Gait::Walk);
    let speed = w.speed_of(c.guid, gait);
    let step = w.navigate(c.guid, (hx, hy), speed * WANDER_HOP_SECS, 0.0);
    let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, speed) else {
        return;
    };
    let leg = Leg {
        to: (x, y),
        z_fallback: home.at.z,
        dur_ms,
        gait,
        hold_until_landed: true,
    };
    w.commit_leg(c.guid, leg, tick.now_ms);
}

fn regenerate<W: RegenSink>(w: &mut W, flagged: &[u64]) -> usize {
    let units = w.recovering(flagged);
    let visited = units.len();
    for u in &units {
        let health = match (u.health < u.max_health, u.in_combat) {
            (false, _) => None,
            (true, true) => w.combat_healed_to(u),
            (true, false) => Some(w.healed_to(u)),
        }
        .filter(|next| *next != u.health);
        let power = (u.max_power > 0)
            .then(|| w.powered_to(u))
            .filter(|next| *next != u.power);
        if health.is_some() || power.is_some() {
            w.restore(u.guid, health, power);
        }
    }
    visited
}

fn combat_exit<W: EngageSink>(w: &mut W, tick: &TickContext, flagged: &[u64]) -> usize {
    let now_ms = tick.now_micros / 1000;
    let expired: Vec<u64> = w
        .flagged_in_combat(flagged)
        .into_iter()
        .filter(|c| now_ms >= c.combat_until_ms)
        .map(|c| c.guid)
        .collect();
    for guid in expired {
        w.leave_combat(guid);
    }
    flagged.len()
}

fn rout<W: RoutSink + MotionSink + IdleSink + EngageSink>(w: &mut W, tick: &TickContext) -> usize {
    let mut routers = w.routers(&tick.scope);
    routers.sort_unstable_by_key(|c| c.guid);
    let visited = routers.len();
    for c in routers {
        // Crowd control owns this firing's movement; fear must not move the creature twice.
        if w.movement_suppressed(c.guid) {
            continue;
        }
        let starting = may_start_rout(c.eligible, c.rout_ends_ms);
        if !starting && !c.routing {
            continue;
        }
        let Some(victim_at) = c.victim_at else {
            continue;
        };
        if starting {
            w.start_rout(c.guid, rout_close_ms(tick.now_ms));
        }
        // Refresh both combat deadlines mid-leg so a long rout cannot end the engagement.
        w.enter_combat(c.guid);
        w.enter_combat(c.victim);
        if c.committed {
            continue;
        }
        let run = w.speed_of(c.guid, Gait::Run) * wounded_slow_factor(c.health, c.max_health);
        let away = flee_step(c.at.x, c.at.y, victim_at.x, victim_at.y, FLEE_LEG_YD);
        let step = w.navigate(c.guid, away, FLEE_LEG_YD, 0.0);
        let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), step, run) else {
            continue;
        };
        let leg = Leg {
            to: (x, y),
            z_fallback: c.at.z,
            dur_ms,
            gait: Gait::Run,
            hold_until_landed: false,
        };
        w.commit_leg(c.guid, leg, tick.now_ms);
    }
    visited
}

fn fear<W: FearSink + IdleSink>(w: &mut W, tick: &TickContext) -> usize {
    let mut panicked = w.panicked(&tick.scope);
    // Collapse multiple fear auras to one movement leg per creature per firing.
    panicked.sort_unstable_by_key(|c| c.guid);
    panicked.dedup_by_key(|c| c.guid);
    let visited = panicked.len();
    for c in panicked {
        if c.frozen {
            continue;
        }
        let from = c.source_at.unwrap_or(c.at);
        let (fx, fy) = feared_flee_step(
            c.at.x,
            c.at.y,
            from.x,
            from.y,
            fear_step_for_tick(tick.tick_secs),
            w.roll(),
        );
        let Some((x, y, dur_ms)) = leg_toward((c.at.x, c.at.y), (fx, fy), constants::speeds::RUN)
        else {
            continue;
        };
        let leg = Leg {
            to: (x, y),
            z_fallback: c.at.z,
            dur_ms,
            gait: Gait::Run,
            hold_until_landed: false,
        };
        w.commit_leg(c.guid, leg, tick.now_ms);
    }
    visited
}

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

#[cfg(test)]
mod chase_leg_tests {
    use super::*;

    const DIR: (f32, f32) = (1.0, 0.0);
    const STOP_AT: (f32, f32) = (19.0, 0.0);

    fn leg_to(dest_x: f32) -> LegInFlight {
        LegInFlight {
            guid: 1,
            start: Point {
                x: 0.0,
                y: 0.0,
                z: 10.0,
            },
            dest: Point {
                x: dest_x,
                y: 0.0,
                z: 10.0,
            },
            started_micros: 0,
            dur_ms: 4_000,
            map_id: 0,
            instance_id: 0,
            back_off: false,
            mover_gone: false,
            waypoints: Vec::new(),
        }
    }

    #[test]
    fn no_leg_in_flight_always_needs_one() {
        assert!(needs_new_leg(None, DIR, STOP_AT));
    }

    #[test]
    fn a_leg_that_lands_short_of_the_stop_point_is_kept() {
        assert!(!needs_new_leg(Some(&leg_to(10.0)), DIR, STOP_AT));
    }

    #[test]
    fn a_leg_landing_exactly_on_the_stop_point_is_kept() {
        assert!(!needs_new_leg(Some(&leg_to(19.0)), DIR, STOP_AT));
    }

    #[test]
    fn a_leg_that_would_overshoot_the_stop_point_is_replaced() {
        assert!(needs_new_leg(Some(&leg_to(28.0)), DIR, STOP_AT));
    }

    #[test]
    fn a_veered_heading_forces_a_new_leg() {
        assert!(needs_new_leg(Some(&leg_to(19.0)), (0.0, 1.0), STOP_AT));
    }
}
