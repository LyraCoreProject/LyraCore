//! In-memory creature world for the behavior cycle: no database, no `ReducerContext`. A test
//! describes creatures and their legs, runs one cycle, and reads back the authoritative state plus
//! the ordered movement effects a client would have received.

use super::*;
use crate::creatures::chase_step;
use lyracore_shared::spatial;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

/// A creature's authoritative state, as the cycle writes it.
#[derive(Clone, Copy, PartialEq, Debug)]
struct XCreature {
    at: Point,
    grid: (i32, i32),
    cell: i64,
    last_move_ms: u32,
    /// Where it looks. Only the facing turn writes it.
    orientation: f32,
    /// The ETA gate an idle leg arms, and the route cursor patrol walks by.
    leg_ends_ms: u32,
    wp_target: u64,
    // --- what the engagement phases read; every one has a quiet default a builder overrides.
    level: u32,
    faction_template: u32,
    map_id: u32,
    instance_id: u64,
    aggro_range: Option<u32>,
    detect_range_mod: f32,
    would_rout: bool,
    cannot_act: bool,
}

/// One movement leg the cycle emitted — everything the relay carries to a client. A zero `dur_ms`
/// with `dest == start` is a stop.
#[derive(Clone, Copy, PartialEq, Debug)]
struct MoveEffect {
    guid: u64,
    start: Point,
    dest: Point,
    dur_ms: u32,
    spline_id: u32,
    run: bool,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
    cell: i64,
    facing: bool,
    facing_angle: f32,
}

#[derive(Default)]
struct Scenario {
    creatures: RefCell<HashMap<u64, XCreature>>,
    legs: RefCell<Vec<LegInFlight>>,
    suppressed: RefCell<HashSet<u64>>,
    engaged: RefCell<HashSet<u64>>,
    routes: RefCell<HashMap<u64, Vec<Waypoint>>>,
    homes: RefCell<HashMap<u64, Home>>,
    /// Determinism input: the firing clock every cycle reads through `TickContext`.
    now_micros: Cell<u64>,
    /// Determinism input: the world's random stream, oldest roll first.
    rolls: RefCell<VecDeque<u32>>,
    /// Determinism input: the imported ground under every landing point. `None` is an unimported
    /// slice, where a leg keeps the height its decider fell back to.
    ground: Cell<Option<f32>>,
    /// Determinism input: navigation aims at this corner instead of the goal, i.e. the goal is
    /// blocked and the walk has to go around.
    detours: RefCell<HashMap<u64, (f32, f32)>>,
    /// Ordered movement effects, oldest first.
    effects: RefCell<Vec<MoveEffect>>,
    /// The players in the world, in the order the world reads them.
    players: RefCell<Vec<AggroTarget>>,
    /// Live melee engagements — what aggro arms, and what combat entry and chase read.
    fights: RefCell<Vec<Engagement>>,
    /// Creatures inside an open rout window: the rout leg is their sole mover.
    routing: RefCell<HashSet<u64>>,
    /// The longest offensive spell range a creature fights from; absent means melee-only.
    hold_ranges: RefCell<HashMap<u64, f32>>,
    /// Determinism input: when each PLAYER last moved. A creature's own move clock is on its row.
    player_moved_ms: RefCell<HashMap<u64, u32>>,
    /// Faction pairs at war, directed. Anything absent is not hostile, as missing data is.
    at_war: RefCell<HashSet<(u32, u32)>>,
    /// `(looker, unseen)` pairs with something solid in between.
    blind: RefCell<HashSet<(u64, u64)>>,
    /// Ordered engagements the cycle armed, oldest first.
    pulls: RefCell<Vec<(u64, u64, Pull)>>,
    /// Ordered units the cycle flagged in combat, oldest first.
    flagged: RefCell<Vec<u64>>,
    /// Scenario input for the active-cell sweep.
    awake: RefCell<TickSweep>,
    /// Positions read BEFORE any pass ran, i.e. by the sweep — still the leg starts, because the
    /// sweep runs ahead of spline advance.
    seen_by_sweep: RefCell<Vec<(u64, Point)>>,
    maintenance_runs: Cell<u32>,
    package_runs: Cell<u32>,
}

impl Scenario {
    fn new(now_micros: u64) -> Self {
        let s = Self::default();
        s.now_micros.set(now_micros);
        s
    }

    /// Place a creature at `at`, with its grid address derived the way the world derives it. It is
    /// level 10, of the creature faction, on the level-scaled aggro radius, and fit to fight.
    fn creature(self, guid: u64, at: Point) -> Self {
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        self.creatures.borrow_mut().insert(
            guid,
            XCreature {
                at,
                grid: (gx, gy),
                cell: spatial::grid_cell_id(gx, gy),
                last_move_ms: 0,
                orientation: 0.0,
                leg_ends_ms: 0,
                wp_target: 0,
                level: 10,
                faction_template: BEASTS,
                map_id: MAP,
                instance_id: INSTANCE,
                aggro_range: Some(0),
                detect_range_mod: 0.0,
                would_rout: false,
                cannot_act: false,
            },
        );
        self
    }

    /// Edit one creature's sensing state.
    fn tweak(self, guid: u64, edit: impl FnOnce(&mut XCreature)) -> Self {
        edit(self.creatures.borrow_mut().get_mut(&guid).unwrap());
        self
    }

    fn level(self, guid: u64, level: u32) -> Self {
        self.tweak(guid, |c| c.level = level)
    }

    /// The creature's template carries a hand-tuned aggro range instead of the level-scaled one.
    fn tuned_aggro_range(self, guid: u64, yards: u32) -> Self {
        self.tweak(guid, |c| c.aggro_range = Some(yards))
    }

    /// Mind Soothe: a signed yard change to this creature's own detection radius.
    fn soothed(self, guid: u64, yards: f32) -> Self {
        self.tweak(guid, |c| c.detect_range_mod = yards)
    }

    /// Near death and of a kind that runs — it must not be pulled into a fight it would flee.
    fn near_death(self, guid: u64) -> Self {
        self.tweak(guid, |c| c.would_rout = true)
    }

    /// Stunned, polymorphed or feared: it cannot act.
    fn crowd_controlled(self, guid: u64) -> Self {
        self.tweak(guid, |c| c.cannot_act = true)
    }

    fn faction(self, guid: u64, faction_template: u32) -> Self {
        self.tweak(guid, |c| c.faction_template = faction_template)
    }

    fn in_instance(self, guid: u64, instance_id: u64) -> Self {
        self.tweak(guid, |c| c.instance_id = instance_id)
    }

    /// Put a living, visible, level-10 ALLIANCE player at `at`.
    fn player(self, guid: u64, at: Point) -> Self {
        self.players.borrow_mut().push(AggroTarget {
            guid,
            at,
            level: 10,
            faction_template: ALLIANCE,
            map_id: MAP,
            instance_id: INSTANCE,
            dead: false,
            godmode: false,
            stealthed: false,
        });
        self
    }

    /// Edit one player.
    fn tweak_player(self, guid: u64, edit: impl FnOnce(&mut AggroTarget)) -> Self {
        edit(
            self.players
                .borrow_mut()
                .iter_mut()
                .find(|p| p.guid == guid)
                .unwrap(),
        );
        self
    }

    fn player_level(self, guid: u64, level: u32) -> Self {
        self.tweak_player(guid, |p| p.level = level)
    }

    /// A GM who cannot be killed.
    fn godmoded(self, guid: u64) -> Self {
        self.tweak_player(guid, |p| p.godmode = true)
    }

    fn corpse(self, guid: u64) -> Self {
        self.tweak_player(guid, |p| p.dead = true)
    }

    fn stealthed(self, guid: u64) -> Self {
        self.tweak_player(guid, |p| p.stealthed = true)
    }

    /// These two factions are enemies, both ways round.
    fn at_war(self, a: u32, b: u32) -> Self {
        self.at_war.borrow_mut().extend([(a, b), (b, a)]);
        self
    }

    /// Something solid stands between `looker` and `unseen`.
    fn wall_between(self, looker: u64, unseen: u64) -> Self {
        self.blind.borrow_mut().insert((looker, unseen));
        self
    }

    /// `attacker` is already swinging at `victim`.
    fn attacking(self, attacker: u64, victim: u64) -> Self {
        self.fights.borrow_mut().push(Engagement {
            attacker,
            victim,
            instance_id: INSTANCE,
            player_never_swung: false,
        });
        self
    }

    /// The creature is inside an open rout window, so the rout leg owns its movement.
    fn routing(self, guid: u64) -> Self {
        self.routing.borrow_mut().insert(guid);
        self
    }

    /// The creature is an offensive caster: it fights from `yards` away instead of closing.
    fn caster(self, guid: u64, yards: f32) -> Self {
        self.hold_ranges.borrow_mut().insert(guid, yards);
        self
    }

    /// The player RUNS to `at`: it stands there and its move clock reads now, so a chaser treats it
    /// as a kiter rather than a planted target.
    fn kiting(self, guid: u64, at: Point) -> Self {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        self.player_moved_ms.borrow_mut().insert(guid, now_ms);
        self.tweak_player(guid, |p| p.at = at)
    }

    /// The creature looks this way — what the facing turn measures its correction against.
    fn facing(self, guid: u64, orientation: f32) -> Self {
        self.tweak(guid, |c| c.orientation = orientation)
    }

    /// A player's auto-attack toggle armed at something it has never reached.
    fn aiming_at(self, player: u64, victim: u64) -> Self {
        self.fights.borrow_mut().push(Engagement {
            attacker: player,
            victim,
            instance_id: INSTANCE,
            player_never_swung: true,
        });
        self
    }

    fn pulls(&self) -> Vec<(u64, u64, Pull)> {
        self.pulls.borrow().clone()
    }

    fn flagged(&self) -> Vec<u64> {
        self.flagged.borrow().clone()
    }

    /// Wake these creatures: the active-cell sweep found them near a covered player this firing.
    fn awake(self, guids: impl IntoIterator<Item = u64>) -> Self {
        self.awake.borrow_mut().active = guids.into_iter().collect();
        self
    }

    /// Give the creature a patrol route, `(waypoint id, point)` in route order.
    fn route(self, guid: u64, waypoints: &[(u64, Point)]) -> Self {
        self.routes.borrow_mut().insert(
            guid,
            waypoints
                .iter()
                .map(|(id, at)| Waypoint { id: *id, at: *at })
                .collect(),
        );
        self
    }

    /// Point the route cursor at the waypoint the creature is walking TO.
    fn walking_to(self, guid: u64, waypoint_id: u64) -> Self {
        self.creatures
            .borrow_mut()
            .get_mut(&guid)
            .unwrap()
            .wp_target = waypoint_id;
        self
    }

    /// Give the creature a spawn post; `wanders` is cmangos RANDOM movement (an IDLE creature holds
    /// its post instead).
    fn home(self, guid: u64, at: Point, wanders: bool) -> Self {
        self.homes.borrow_mut().insert(guid, Home { at, wanders });
        self
    }

    fn fighting(self, guid: u64) -> Self {
        self.engaged.borrow_mut().insert(guid);
        self
    }

    /// The creature's current idle leg animates until `leg_ends_ms`.
    fn mid_leg(self, guid: u64, leg_ends_ms: u32) -> Self {
        self.creatures
            .borrow_mut()
            .get_mut(&guid)
            .unwrap()
            .leg_ends_ms = leg_ends_ms;
        self
    }

    fn rolls(self, rolls: impl IntoIterator<Item = u32>) -> Self {
        *self.rolls.borrow_mut() = rolls.into_iter().collect();
        self
    }

    /// Imported terrain: every landing point sits on this ground height.
    fn ground(self, z: f32) -> Self {
        self.ground.set(Some(z));
        self
    }

    /// The straight line to this creature's goal is blocked; navigation heads for `corner` instead.
    fn detour(self, guid: u64, corner: (f32, f32)) -> Self {
        self.detours.borrow_mut().insert(guid, corner);
        self
    }

    fn advance_clock(&self, micros: u64) {
        self.now_micros.set(self.now_micros.get() + micros);
    }

    /// Put `guid` mid-flight from `start` to `dest`, launched at `started_micros`.
    fn flying(
        self,
        guid: u64,
        start: Point,
        dest: Point,
        started_micros: u64,
        dur_ms: u32,
    ) -> Self {
        self.legs.borrow_mut().push(LegInFlight {
            guid,
            start,
            dest,
            started_micros,
            dur_ms,
            map_id: MAP,
            instance_id: INSTANCE,
            mover_gone: !self.creatures.borrow().contains_key(&guid),
        });
        self
    }

    fn rooted(self, guid: u64) -> Self {
        self.suppressed.borrow_mut().insert(guid);
        self
    }

    fn at(&self, guid: u64) -> XCreature {
        self.creatures.borrow()[&guid]
    }

    fn has_leg(&self, guid: u64) -> bool {
        self.legs.borrow().iter().any(|l| l.guid == guid)
    }

    fn effects(&self) -> Vec<MoveEffect> {
        self.effects.borrow().clone()
    }

    fn tick(&self, sense: bool, scope: TickScope) -> TickContext {
        TickContext {
            now_micros: self.now_micros.get(),
            now_ms: (self.now_micros.get() / 1000) as u32,
            tick_secs: crate::creatures::MOVE_TICK_SECS,
            sense,
            scope,
        }
    }
}

impl MotionSink for Scenario {
    fn legs_in_flight(&self) -> Vec<LegInFlight> {
        self.legs.borrow().clone()
    }
    fn movement_suppressed(&self, guid: u64) -> bool {
        self.suppressed.borrow().contains(&guid)
    }
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) {
        self.place(guid, at, Some(moved_ms));
    }
    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32) {
        let Some(grid) = self.place(leg.guid, at, None) else {
            return;
        };
        self.effects.borrow_mut().push(MoveEffect {
            guid: leg.guid,
            start: at,
            dest: at,
            dur_ms: 0,
            spline_id,
            run: false,
            map_id: leg.map_id,
            instance_id: leg.instance_id,
            grid,
            cell: spatial::grid_cell_id(grid.0, grid.1),
            facing: false,
            facing_angle: 0.0,
        });
        // The stop REPLACES the leg it interrupts (the writer upserts one row per mover), so the
        // next cycle reads it as landed and reaps it instead of resuming the old destination.
        let mut legs = self.legs.borrow_mut();
        legs.retain(|l| l.guid != leg.guid);
        legs.push(LegInFlight {
            guid: leg.guid,
            start: at,
            dest: at,
            started_micros: self.now_micros.get(),
            dur_ms: 0,
            map_id: leg.map_id,
            instance_id: leg.instance_id,
            mover_gone: false,
        });
    }
    fn drop_leg(&mut self, guid: u64) {
        self.legs.borrow_mut().retain(|l| l.guid != guid);
    }
}

impl Scenario {
    /// The state mirror behind both position writes, matching `CtxWorld::place`.
    fn place(&self, guid: u64, at: Point, moved_ms: Option<u32>) -> Option<(i32, i32)> {
        let mut creatures = self.creatures.borrow_mut();
        let c = creatures.get_mut(&guid)?;
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        c.at = at;
        c.grid = (gx, gy);
        c.cell = spatial::grid_cell_id(gx, gy);
        if let Some(ms) = moved_ms {
            c.last_move_ms = ms;
        }
        Some((gx, gy))
    }

    /// A fight's victim wherever it stands — a creature row or a player — as its position and move
    /// clock. Chase reads both: a kiter is run down, a planted one is stood next to and faced.
    fn unit(&self, guid: u64) -> Option<(Point, u32)> {
        if let Some(c) = self.creatures.borrow().get(&guid) {
            return Some((c.at, c.last_move_ms));
        }
        let moved = self.player_moved_ms.borrow().get(&guid).copied();
        self.players
            .borrow()
            .iter()
            .find(|p| p.guid == guid)
            .map(|p| (p.at, moved.unwrap_or(0)))
    }

    fn snapshot(&self) -> Vec<(u64, Point)> {
        let mut seen: Vec<(u64, Point)> = self
            .creatures
            .borrow()
            .iter()
            .map(|(guid, c)| (*guid, c.at))
            .collect();
        seen.sort_by_key(|(guid, _)| *guid);
        seen
    }
}

impl CreatureWorld for Scenario {
    fn awake_creatures(&self, _scope: &TickScope) -> TickSweep {
        *self.seen_by_sweep.borrow_mut() = self.snapshot();
        let awake = self.awake.borrow();
        TickSweep {
            active: awake.active.clone(),
            pets: awake.pets.clone(),
            in_combat: awake.in_combat.clone(),
        }
    }
    fn run_due_world_maintenance(&mut self) -> Vec<(&'static str, u64)> {
        self.maintenance_runs.set(self.maintenance_runs.get() + 1);
        Vec::new()
    }
    fn run_package_passes(&mut self) {
        self.package_runs.set(self.package_runs.get() + 1);
    }
}

// The in-memory idle world. A scenario holds only LIVE creature rows, so the "no players, no
// corpses" half of `CtxWorld::idle_creatures` has nothing to reject here.
impl IdleSink for Scenario {
    fn idle_creatures(&self, active: &HashSet<u64>) -> Vec<IdleCreature> {
        let creatures = self.creatures.borrow();
        let routes = self.routes.borrow();
        active
            .iter()
            .filter_map(|guid| creatures.get(guid).map(|c| (guid, c)))
            .map(|(guid, c)| IdleCreature {
                guid: *guid,
                at: c.at,
                leg_ends_ms: c.leg_ends_ms,
                wp_target: c.wp_target,
                patrols: routes.contains_key(guid),
            })
            .collect()
    }
    fn route_of(&self, guid: u64) -> Vec<Waypoint> {
        self.routes.borrow().get(&guid).cloned().unwrap_or_default()
    }
    fn home_of(&self, guid: u64) -> Option<Home> {
        self.homes.borrow().get(&guid).copied()
    }
    /// Either side of a live fight is engaged, the way a melee row makes it so in the world — plus
    /// whatever a scenario marked engaged without arming a fight.
    fn engaged(&self, guid: u64) -> bool {
        self.engaged.borrow().contains(&guid)
            || self
                .fights
                .borrow()
                .iter()
                .any(|f| f.attacker == guid || f.victim == guid)
    }
    fn speed_of(&self, _guid: u64, gait: Gait) -> f32 {
        match gait {
            Gait::Walk => lyracore_shared::constants::speeds::WALK,
            Gait::Run => lyracore_shared::constants::speeds::RUN,
        }
    }
    fn navigate(&self, guid: u64, to: (f32, f32), max_step: f32) -> (f32, f32) {
        let from = self.creatures.borrow()[&guid].at;
        let aim = self.detours.borrow().get(&guid).copied().unwrap_or(to);
        chase_step(from.x, from.y, aim.0, aim.1, max_step, 0.0)
    }
    fn roll(&self) -> u32 {
        self.rolls
            .borrow_mut()
            .pop_front()
            .expect("the scenario ran out of random rolls")
    }
    fn aim_at_waypoint(&mut self, guid: u64, waypoint_id: u64) {
        if let Some(c) = self.creatures.borrow_mut().get_mut(&guid) {
            c.wp_target = waypoint_id;
        }
    }
    /// The one leg writer: ground-snap, relay, and start the spline the NEXT cycle advances along —
    /// the production writer's three jobs in one place, exactly as `emit_creature_leg` does them.
    fn commit_leg(&mut self, guid: u64, leg: Leg, now_ms: u32) {
        let Some(from) = self.creatures.borrow().get(&guid).copied() else {
            return;
        };
        let dest = Point {
            x: leg.to.0,
            y: leg.to.1,
            z: self.ground.get().unwrap_or(leg.z_fallback),
        };
        if !finite_point(dest.x, dest.y, dest.z) {
            return; // the writer refuses a corrupt leg rather than writing it onto the creature
        }
        let dur_ms = leg.dur_ms.max(1); // a zero-duration lerp would divide by zero
        self.effects.borrow_mut().push(MoveEffect {
            guid,
            start: from.at,
            dest,
            dur_ms,
            spline_id: now_ms,
            run: leg.gait == Gait::Run,
            map_id: MAP,
            instance_id: INSTANCE,
            grid: from.grid,
            cell: from.cell,
            facing: false,
            facing_angle: 0.0,
        });
        let mut legs = self.legs.borrow_mut();
        legs.retain(|l| l.guid != guid);
        legs.push(LegInFlight {
            guid,
            start: from.at,
            dest,
            started_micros: self.now_micros.get(),
            dur_ms,
            map_id: MAP,
            instance_id: INSTANCE,
            mover_gone: false,
        });
        let mut creatures = self.creatures.borrow_mut();
        let c = creatures.get_mut(&guid).unwrap();
        c.last_move_ms = now_ms;
        if leg.hold_until_landed {
            c.leg_ends_ms = now_ms + leg.dur_ms;
        }
    }
}

// The in-memory engagement world. A scenario holds only live, non-pet creature rows, so the
// "no players, no corpses, no pets" half of `CtxWorld::sensing_creatures` has nothing to reject.
impl EngageSink for Scenario {
    fn players(&self) -> Vec<AggroTarget> {
        self.players.borrow().clone()
    }
    fn sensing_creatures(&self, active: &HashSet<u64>) -> Vec<Sensor> {
        let creatures = self.creatures.borrow();
        let fights = self.fights.borrow();
        active
            .iter()
            .filter_map(|guid| creatures.get(guid).map(|c| (*guid, c)))
            .filter(|(guid, _)| !fights.iter().any(|f| f.attacker == *guid))
            .map(|(guid, c)| Sensor {
                guid,
                at: c.at,
                level: c.level,
                faction_template: c.faction_template,
                map_id: c.map_id,
                instance_id: c.instance_id,
                aggro_range: c.aggro_range,
                detect_range_mod: c.detect_range_mod,
                would_rout: c.would_rout,
                cannot_act: c.cannot_act,
            })
            .collect()
    }
    fn hostile(&self, faction_template: u32, other: u32) -> bool {
        self.at_war.borrow().contains(&(faction_template, other))
    }
    /// The scenario blocks sight between two UNITS, so the sighted point is resolved back to the
    /// unit standing on it — unambiguous in a scenario, and it keeps the assist scan's habit of
    /// sighting a caller at the neighbor's own height out of the test's way.
    fn line_of_sight(&self, looker: u64, at: Point) -> bool {
        let seen = self
            .creatures
            .borrow()
            .iter()
            .find(|(_, c)| (c.at.x, c.at.y) == (at.x, at.y))
            .map(|(guid, _)| *guid)
            .or_else(|| {
                self.players
                    .borrow()
                    .iter()
                    .find(|p| (p.at.x, p.at.y) == (at.x, at.y))
                    .map(|p| p.guid)
            });
        seen.is_none_or(|seen| !self.blind.borrow().contains(&(looker, seen)))
    }
    fn engage(&mut self, creature: u64, victim: u64, pull: Pull) {
        self.pulls.borrow_mut().push((creature, victim, pull));
        let instance_id = self.creatures.borrow()[&creature].instance_id;
        self.fights.borrow_mut().push(Engagement {
            attacker: creature,
            victim,
            instance_id,
            player_never_swung: false,
        });
    }
    fn engagements(&self) -> Vec<Engagement> {
        self.fights.borrow().clone()
    }
    fn enter_combat(&mut self, guid: u64) {
        self.flagged.borrow_mut().push(guid);
    }
}

// The in-memory chase world. A fight whose attacker is not a creature row is a PLAYER's own attack
// row, which the production adapter drops for the same reason: chase moves creatures.
impl PursuitSink for Scenario {
    fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit> {
        let creatures = self.creatures.borrow();
        let legs = self.legs.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                let (victim_at, victim_last_move_ms) = self.unit(f.victim)?;
                scope.covers(c.instance_id).then_some(Pursuit {
                    guid: f.attacker,
                    at: c.at,
                    orientation: c.orientation,
                    victim_at,
                    victim_last_move_ms,
                    routing: self.routing.borrow().contains(&f.attacker),
                    leg: legs.iter().find(|l| l.guid == f.attacker).cloned(),
                })
            })
            .collect()
    }
    fn caster_hold_range(&self, guid: u64) -> f32 {
        self.hold_ranges.borrow().get(&guid).copied().unwrap_or(0.0)
    }
    fn face(&mut self, guid: u64, orientation: f32, spline_id: u32) {
        let Some(c) = self.creatures.borrow_mut().get_mut(&guid).map(|c| {
            c.orientation = orientation;
            *c
        }) else {
            return;
        };
        self.effects.borrow_mut().push(MoveEffect {
            guid,
            start: c.at,
            dest: c.at,
            dur_ms: 0,
            spline_id,
            run: false,
            map_id: c.map_id,
            instance_id: c.instance_id,
            grid: c.grid,
            cell: c.cell,
            facing: true,
            facing_angle: orientation,
        });
    }
}

// Not migrated yet: the cycle SEQUENCES these, the harness cannot run them.
impl LegacyPasses for Scenario {
    fn legacy_pet(&mut self, _scope: &TickScope, _now_ms: u32, _pets: &[u64]) -> usize {
        0
    }
    fn legacy_cast(&mut self, _scope: &TickScope) -> usize {
        0
    }
    fn legacy_threat_retarget(&mut self, _scope: &TickScope) -> usize {
        0
    }
    fn legacy_regen(&mut self) -> usize {
        0
    }
    fn legacy_combat_drop(&mut self, _in_combat: &[u64]) -> usize {
        0
    }
    fn legacy_flee(&mut self, _scope: &TickScope) -> usize {
        0
    }
    fn legacy_fear_flee(&mut self, _scope: &TickScope, _tick_secs: f32) -> usize {
        0
    }
}

const WOLF: u64 = 0x0000_0000_0000_0BEE;
/// A second and third wolf of the same pack, guid-ordered after `WOLF`.
const PACK_MATE: u64 = WOLF + 1;
const FAR_MATE: u64 = WOLF + 2;
const HUNTER: u64 = 0x0000_0000_0000_0A11;
const RANGER: u64 = HUNTER + 1;
const BEASTS: u32 = 14;
const ALLIANCE: u32 = 1;
const MAP: u32 = 0;
const INSTANCE: u64 = 0;
/// A one-second leg launched at t=0, sampled half way through.
const LEG_MS: u32 = 1000;
const HALF_WAY: u64 = 500_000;

fn p(x: f32, y: f32, z: f32) -> Point {
    Point { x, y, z }
}

fn catch_all() -> TickScope {
    TickScope::from_rows(crate::creatures::GLOBAL_TICK_INSTANCE, [])
}

/// A wolf half way through a 10-yard leg.
fn wolf_mid_flight(now_micros: u64) -> Scenario {
    Scenario::new(now_micros)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(10.0, 0.0, 10.0), 0, LEG_MS)
}

/// A route-less wolf awake near a player, with its spawn post at the origin: the shape both
/// walking home and loitering decide on.
fn idle_wolf(at: Point, wanders: bool) -> Scenario {
    Scenario::new(HALF_WAY)
        .creature(WOLF, at)
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), wanders)
}

#[test]
fn a_creature_mid_flight_moves_to_the_point_its_client_renders() {
    let mut w = wolf_mid_flight(HALF_WAY);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let wolf = w.at(WOLF);
    assert_eq!(
        wolf.at,
        p(5.0, 0.0, 10.0),
        "the authoritative position must track where the leg renders; leading it is what makes \
         range, melee and aggro checks fire early"
    );
    assert_eq!(
        (wolf.grid, wolf.cell),
        (
            spatial::grid_cell(5.0, 0.0),
            spatial::grid_cell_id(spatial::grid_cell(5.0, 0.0).0, spatial::grid_cell(5.0, 0.0).1)
        ),
        "grid address and packed cell must move with the position, or the creature is delivered to \
         the wrong players"
    );
    assert_eq!(
        wolf.last_move_ms,
        (HALF_WAY / 1000) as u32,
        "a creature that travelled must stamp its move clock, or the idle passes treat it as parked"
    );
    assert!(
        w.has_leg(WOLF),
        "a leg still in flight must keep playing; forgetting it strands the creature mid-route"
    );
}

#[test]
fn a_later_pass_reads_the_advanced_position_not_the_leg_start() {
    // The wolf renders at (5, 0) half way through its leg. The waypoint BEHIND it is the nearer one
    // from the leg start, the waypoint AHEAD is the nearer one from where it actually is.
    let mut w = wolf_mid_flight(HALF_WAY)
        .awake([WOLF])
        .route(WOLF, &[(1, p(-4.0, 0.0, 10.0)), (2, p(11.0, 0.0, 10.0))]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.seen_by_sweep.borrow().as_slice(),
        [(WOLF, p(0.0, 0.0, 10.0))],
        "the sweep runs before advance, so it still reads the leg start"
    );
    assert_eq!(
        w.effects().first().map(|e| e.dest),
        Some(p(11.0, 0.0, 10.0)),
        "every pass after advance must decide from the rendered position, or the whole cycle acts \
         on a place the creature is not — here the wolf would turn round and walk backwards"
    );
}

#[test]
fn a_patrolling_creature_walks_the_next_segment_of_its_route() {
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 1);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let now_ms = (HALF_WAY / 1000) as u32;
    let grid = spatial::grid_cell(0.0, 0.0);
    assert_eq!(
        w.effects(),
        [MoveEffect {
            guid: WOLF,
            start: p(0.0, 0.0, 10.0),
            dest: p(10.0, 0.0, 10.0),
            dur_ms: 4000, // 10 yd at WALK
            spline_id: now_ms,
            run: false,
            map_id: MAP,
            instance_id: INSTANCE,
            grid,
            cell: spatial::grid_cell_id(grid.0, grid.1),
            facing: false,
            facing_angle: 0.0,
        }],
        "a patroller must walk ONE segment of its route, at walk pace — a run leg or a leg to the \
         wrong waypoint is a creature that visibly leaves its route"
    );
    assert_eq!(
        (w.at(WOLF).wp_target, w.at(WOLF).leg_ends_ms),
        (2, now_ms + 4000),
        "the cursor must advance to the waypoint being walked to and the leg must be held to \
         completion, or the route re-decides itself every firing and the creature dithers"
    );
}

#[test]
fn a_patroller_outside_every_active_cell_stays_frozen_on_its_route() {
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .route(WOLF, &route)
        .walking_to(WOLF, 1);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty() && w.at(WOLF).wp_target == 1,
        "a creature no player can see must cost the tick nothing and keep its route state, so it \
         resumes exactly where it paused when a player walks back into range"
    );
}

#[test]
fn a_displaced_creature_runs_home_one_firing_at_a_time() {
    let mut w = idle_wolf(p(0.0, 20.0, 10.0), false);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let effect = w.effects()[0];
    assert_eq!(
        (effect.dest, effect.run, effect.dur_ms),
        (p(0.0, 16.5, 10.0), true, 500),
        "the walk home must be ONE firing's worth of run toward the post; a whole-distance leg \
         teleports the creature home the moment a player displaces it"
    );
}

#[test]
fn a_creature_that_runs_home_does_not_also_wander_in_the_same_firing() {
    let mut w = idle_wolf(p(0.0, 20.0, 10.0), true).rolls([0, 0, 0]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().len(),
        1,
        "two legs in one firing share a spline id and the client plays only the first, so the \
         creature would visibly stutter between home and its loiter point"
    );
    assert!(w.effects()[0].run, "walking home wins over loitering");
}

#[test]
fn a_creature_that_reaches_home_loiters_again() {
    // Just outside the leash: one run leg lands it back inside, so the NEXT cycle loiters instead.
    let mut w = idle_wolf(p(0.0, 16.0, 10.0), true).rolls([0, 0, 0]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    w.advance_clock(500_000); // the run leg's own duration — it has landed

    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.at(WOLF).at,
        p(0.0, 12.5, 10.0),
        "the second cycle must advance the creature onto the leg it was given, not re-issue it"
    );
    assert!(
        !w.effects()[1].run,
        "a creature that got home again is idle, so its next leg must be a walk-paced loiter hop, \
         not another run home"
    );
}

#[test]
fn an_idle_creature_loiters_near_its_post_on_about_a_third_of_firings() {
    // roll 0: below WANDER_CHANCE_PCT, so it hops. Angle 0 and a full radius roll aim due east.
    for (chance_roll, hops) in [(0u32, true), (99, false)] {
        let mut w = idle_wolf(p(0.0, 0.0, 10.0), true).rolls([chance_roll, 0, u32::MAX]);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            usize::from(hops),
            "the pause between hops is what makes a loiterer read as idle instead of jogging on \
             the spot (roll {chance_roll})"
        );
    }

    let mut w = idle_wolf(p(0.0, 0.0, 10.0), true).rolls([0, 0, u32::MAX]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    let effect = w.effects()[0];
    assert_eq!(
        (effect.dest, effect.run, effect.dur_ms),
        (p(WANDER_RADIUS, 0.0, 10.0), false, 2400),
        "the hop must stay inside the wander radius of the POST and stroll there, or a loiterer \
         drifts off its post hop by hop and trips its own leash"
    );
    assert_eq!(
        w.at(WOLF).leg_ends_ms,
        (HALF_WAY / 1000) as u32 + 2400,
        "the hop is held to completion, so the creature pauses on arrival instead of re-rolling \
         a new point mid-stroll"
    );
}

#[test]
fn loitering_waits_for_a_sense_firing_but_walking_home_does_not() {
    let mut loiterer = idle_wolf(p(0.0, 0.0, 10.0), true).rolls([0, 0, u32::MAX]);
    let tick = loiterer.tick(false, catch_all());
    run_cycle(&mut loiterer, tick);

    let mut displaced = idle_wolf(p(0.0, 20.0, 10.0), false);
    let tick = displaced.tick(false, catch_all());
    run_cycle(&mut displaced, tick);

    assert_eq!(
        (loiterer.effects().len(), displaced.effects().len()),
        (0, 1),
        "the hop chance is authored per SENSE firing, so rolling it every movement firing makes a \
         loiterer hop eight times as often; the walk home is the opposite — it must step on every \
         firing or a displaced creature crawls back at an eighth of its speed"
    );
}

#[test]
fn an_idle_creature_that_holds_its_post_never_loiters() {
    let mut w = idle_wolf(p(0.0, 0.0, 10.0), false).rolls([0, 0, 0]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty(),
        "quest givers, vendors and guards are IDLE-movement creatures: one that strolls off its \
         post is unreachable where the player was sent to find it"
    );
}

#[test]
fn a_creature_with_a_leg_still_in_flight_starts_no_new_leg() {
    let now_ms = (HALF_WAY / 1000) as u32;
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let mut patroller = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 1)
        .mid_leg(WOLF, now_ms + 1);
    let tick = patroller.tick(true, catch_all());
    run_cycle(&mut patroller, tick);

    let mut loiterer = idle_wolf(p(0.0, 0.0, 10.0), true)
        .mid_leg(WOLF, now_ms + 1)
        .rolls([0, 0, 0]);
    let tick = loiterer.tick(true, catch_all());
    run_cycle(&mut loiterer, tick);

    assert!(
        patroller.effects().is_empty() && loiterer.effects().is_empty(),
        "re-throwing a leg that is still animating is the dither the ETA gate exists to stop: the \
         client restarts the same move every firing and the creature never arrives"
    );
}

#[test]
fn an_engaged_or_suppressed_creature_is_moved_by_no_idle_behavior() {
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let holds: [fn(Scenario, u64) -> Scenario; 2] = [Scenario::fighting, Scenario::rooted];
    for held in holds {
        for patrols in [true, false] {
            // Displaced past its leash and RANDOM-movement, so all three idle movers would want it.
            let mut w = Scenario::new(HALF_WAY)
                .creature(WOLF, p(0.0, 20.0, 10.0))
                .awake([WOLF])
                .home(WOLF, p(0.0, 0.0, 10.0), true);
            if patrols {
                w = w.route(WOLF, &route).walking_to(WOLF, 1);
            }
            let mut w = held(w, WOLF);
            let tick = w.tick(true, catch_all());
            run_cycle(&mut w, tick);

            assert!(
                w.effects().is_empty(),
                "a fighting creature belongs to chase and a crowd-controlled one to fear; a second \
                 leg from an idle mover shares their spline id and the client throws it away \
                 (patrols={patrols})"
            );
            if patrols {
                assert_eq!(
                    w.at(WOLF).wp_target,
                    1,
                    "the held creature must keep its route state, or it resumes from the wrong \
                     waypoint once the hold ends"
                );
            }
        }
    }
}

#[test]
fn a_leg_lands_on_the_ground_under_its_destination() {
    let mut snapped = idle_wolf(p(0.0, 20.0, 10.0), false).ground(42.0);
    let tick = snapped.tick(true, catch_all());
    run_cycle(&mut snapped, tick);

    assert_eq!(
        (snapped.effects()[0].dest.z, {
            let mut off_slice = idle_wolf(p(0.0, 20.0, 10.0), false);
            let tick = off_slice.tick(true, catch_all());
            run_cycle(&mut off_slice, tick);
            off_slice.effects()[0].dest.z
        }),
        (42.0, 10.0),
        "a leg must land on imported ground and fall back to the post's own height where terrain \
         is missing, or the creature floats above the slope or sinks into it"
    );
}

#[test]
fn a_blocked_walk_home_goes_around_instead_of_through() {
    let mut w = idle_wolf(p(0.0, 20.0, 10.0), false).detour(WOLF, (20.0, 20.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()[0].dest,
        p(3.5, 20.0, 10.0),
        "the walk home must head for the detour corner navigation returns; walking the straight \
         line instead puts the creature inside the geometry between it and its post"
    );
}

#[test]
fn an_arrived_creature_stops_on_its_destination_and_the_leg_is_forgotten() {
    let mut w = wolf_mid_flight(LEG_MS as u64 * 1000);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.at(WOLF).at,
        p(10.0, 0.0, 10.0),
        "an arrived creature must land exactly on its destination"
    );
    assert!(
        !w.has_leg(WOLF),
        "a landed leg that is not forgotten replays forever and the creature never goes idle"
    );
}

#[test]
fn a_movement_suppressed_creature_freezes_where_it_renders() {
    let mut w = wolf_mid_flight(HALF_WAY).rooted(WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let wolf = w.at(WOLF);
    assert_eq!(
        wolf.at,
        p(5.0, 0.0, 10.0),
        "a rooted creature must stop where it renders, not slide on to the leg destination"
    );
    assert_eq!(
        wolf.last_move_ms, 0,
        "a frozen creature did not travel, so its move clock must not advance"
    );

    let grid = spatial::grid_cell(5.0, 0.0);
    assert_eq!(
        w.effects(),
        [MoveEffect {
            guid: WOLF,
            start: p(5.0, 0.0, 10.0),
            dest: p(5.0, 0.0, 10.0),
            dur_ms: 0,
            spline_id: (HALF_WAY / 1000) as u32,
            run: false,
            map_id: MAP,
            instance_id: INSTANCE,
            grid,
            cell: spatial::grid_cell_id(grid.0, grid.1),
            facing: false,
            facing_angle: 0.0,
        }],
        "the client must be told to stop where the server froze the creature, or it keeps sliding \
         into melee while rooted"
    );
}

#[test]
fn a_non_finite_leg_is_refused_and_the_creature_stays_put() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(f32::NAN, 0.0, 10.0), 0, LEG_MS);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.at(WOLF).at,
        p(0.0, 0.0, 10.0),
        "writing a non-finite position puts the creature in no grid cell at all — it becomes an \
         unreachable attacker, so the leg must be refused instead"
    );
    assert!(
        !w.has_leg(WOLF),
        "the corrupt leg must not survive the cycle"
    );
    assert!(
        w.effects().is_empty(),
        "a refused leg must relay nothing to the client"
    );
}

#[test]
fn a_leg_whose_mover_despawned_is_reaped() {
    let mut w =
        Scenario::new(HALF_WAY).flying(WOLF, p(0.0, 0.0, 10.0), p(10.0, 0.0, 10.0), 0, LEG_MS);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        !w.has_leg(WOLF),
        "a leg belonging to a despawned creature must be reaped, or it accumulates forever"
    );
    assert!(
        w.effects().is_empty(),
        "a despawned creature must not be relayed as moving"
    );
}

#[test]
fn world_maintenance_and_package_passes_run_only_on_the_catch_all_sense_firing() {
    for (sense, scope, maintenance, packages) in [
        (true, catch_all(), 1, 1),
        (false, catch_all(), 0, 1),
        (true, TickScope::Only(7), 0, 0),
    ] {
        let mut w = wolf_mid_flight(HALF_WAY);
        let tick = w.tick(sense, scope);
        run_cycle(&mut w, tick);
        assert_eq!(
            (w.maintenance_runs.get(), w.package_runs.get()),
            (maintenance, packages),
            "running decay, respawn or the package passes from a dedicated instance row multiplies \
             their effects — double regen, double decay — across the whole world (sense={sense})"
        );
    }
}

#[test]
fn the_cycle_reports_the_legs_it_advanced() {
    let mut w = wolf_mid_flight(HALF_WAY);
    let tick = w.tick(true, catch_all());
    let outcome = run_cycle(&mut w, tick);

    assert_eq!(
        outcome.rows_visited.first(),
        Some(&("advance", 1)),
        "operators spot a candidate-set regression from these counts; advance must report the legs \
         it actually moved"
    );
}

/// An awake wolf at the origin and a hostile player it may notice. Both are level 10, so the
/// level-scaled aggro radius is the flat 20 yards.
fn wolf_and_player(player_at: Point) -> Scenario {
    Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, player_at)
}

/// A scenario twist a table-driven test applies before it runs the cycle.
type Twist = fn(Scenario) -> Scenario;

/// A passive pack mate: it answers a call but never notices a player by itself.
fn pack_mate(w: Scenario, guid: u64, at: Point) -> Scenario {
    w.creature(guid, at).tuned_aggro_range(guid, 1)
}

#[test]
fn a_creature_engages_the_nearest_hostile_player_it_can_see() {
    let mut w = wolf_and_player(p(15.0, 0.0, 10.0)).player(RANGER, p(3.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pulls(),
        [(WOLF, RANGER, Pull::Noticed)],
        "a creature must pull the CLOSEST player it notices, and only one of them — picking the \
         farther player leaves someone walking away with a mob that skipped the man next to it"
    );
}

#[test]
fn a_godmoded_or_dead_player_draws_no_aggro() {
    let untouchable: [fn(Scenario, u64) -> Scenario; 2] = [Scenario::godmoded, Scenario::corpse];
    for hold in untouchable {
        let mut w = hold(wolf_and_player(p(5.0, 0.0, 10.0)), HUNTER);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.pulls().is_empty(),
            "a player who cannot die never resolves a pull the normal way — nothing kills them, so \
             nothing ever disengages, and creatures only ACCUMULATE: a GM standing in Northshire \
             collected 103 simultaneous attackers"
        );
    }
}

#[test]
fn a_creature_outside_every_active_cell_notices_nobody() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(2.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.pulls().is_empty(),
        "aggro is what the active-cell sweep exists to bound; scanning a creature no player is \
         near puts the whole world's population back on every sense firing"
    );
}

#[test]
fn a_stealthed_player_is_noticed_only_from_inside_the_detection_range() {
    // Level 10 against level 10: a 20-yard aggro radius, but only 5 yards of seeing through stealth.
    for (dist, noticed) in [(4.0f32, true), (9.0, false)] {
        let mut w = wolf_and_player(p(dist, 0.0, 10.0)).stealthed(HUNTER);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            !w.pulls().is_empty(),
            noticed,
            "stealth must GRADE the radius down, not switch it off: a rogue who steps on a mob is \
             seen, one crossing the camp at {dist} yards is not"
        );
    }
}

#[test]
fn the_aggro_radius_scales_with_level_honors_the_template_override_and_greys_out() {
    let cases = [
        (
            "equal levels, inside the base radius",
            10,
            10,
            None,
            19.0,
            true,
        ),
        ("equal levels, outside it", 10, 10, None, 21.0, false),
        (
            "the creature out-levels the player",
            15,
            10,
            None,
            24.0,
            true,
        ),
        (
            "a hand-tuned template range wins",
            10,
            10,
            Some(8),
            12.0,
            false,
        ),
        (
            "grey: the player out-levels it too far",
            5,
            20,
            None,
            3.0,
            false,
        ),
    ];
    for (case, creature_level, player_level, tuned, dist, noticed) in cases {
        let mut w = wolf_and_player(p(dist, 0.0, 10.0))
            .level(WOLF, creature_level)
            .player_level(HUNTER, player_level);
        if let Some(yards) = tuned {
            w = w.tuned_aggro_range(WOLF, yards);
        }
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            !w.pulls().is_empty(),
            noticed,
            "the reach a creature notices you from is the whole feel of walking through a zone: a \
             high-level player must cross a low one unmolested, and a tuned creature must keep the \
             range its author gave it ({case})"
        );
    }
}

#[test]
fn a_wall_between_them_stops_the_pull() {
    let mut w = wolf_and_player(p(5.0, 0.0, 10.0)).wall_between(WOLF, HUNTER);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.pulls().is_empty(),
        "a hostile on the other side of the abbey wall has not been seen; pulling it drags a mob \
         through geometry at a player who never had line of sight on it"
    );
}

#[test]
fn a_soothed_crowd_controlled_or_near_death_creature_starts_no_fight() {
    let quiet: [(&str, Twist); 3] = [
        ("soothed", |w| w.soothed(WOLF, -20.0)),
        ("crowd controlled", |w| w.crowd_controlled(WOLF)),
        ("near death", |w| w.near_death(WOLF)),
    ];
    for (case, hold) in quiet {
        let mut w = hold(wolf_and_player(p(5.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.pulls().is_empty(),
            "Mind Soothe and a stun both have to actually stop the pull to be worth casting, and a \
             creature about to rout must not be armed into the fight it is leaving ({case})"
        );
    }
}

#[test]
fn a_pack_mate_answers_a_call_it_can_hear() {
    let mut w = pack_mate(
        wolf_and_player(p(5.0, 0.0, 10.0)),
        PACK_MATE,
        p(0.0, 8.0, 10.0),
    );
    w = pack_mate(w, FAR_MATE, p(0.0, 40.0, 10.0)).awake([WOLF, PACK_MATE, FAR_MATE]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pulls(),
        [
            (WOLF, HUNTER, Pull::Noticed),
            (PACK_MATE, HUNTER, Pull::Assisted)
        ],
        "a pack answers its own: the neighbor joins the fight it never would have started, and a \
         mate out of earshot stays out of it — range is measured from the CALLER, so pulling one \
         wolf must not wake the whole valley"
    );
}

#[test]
fn a_neighbor_of_the_wrong_kind_place_or_sight_ignores_the_call() {
    let cases: [(&str, Twist); 3] = [
        ("another faction", |w| w.faction(PACK_MATE, 99)),
        ("another instance", |w| w.in_instance(PACK_MATE, 7)),
        ("a wall between them", |w| w.wall_between(PACK_MATE, WOLF)),
    ];
    for (case, apart) in cases {
        let mut w = apart(
            pack_mate(
                wolf_and_player(p(5.0, 0.0, 10.0)),
                PACK_MATE,
                p(0.0, 8.0, 10.0),
            )
            .awake([WOLF, PACK_MATE]),
        );
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pulls(),
            [(WOLF, HUNTER, Pull::Noticed)],
            "assist is same-kind, same-place and in plain sight; without those gates a copy of the \
             world in another instance, or a guard of another faction, joins a fight it can \
             neither see nor reach ({case})"
        );
    }
}

#[test]
fn nobody_pulling_means_no_assist_scan_at_all() {
    let mut w = pack_mate(
        Scenario::new(HALF_WAY)
            .creature(WOLF, p(0.0, 0.0, 10.0))
            .at_war(BEASTS, ALLIANCE)
            .player(HUNTER, p(100.0, 0.0, 10.0)),
        PACK_MATE,
        p(0.0, 5.0, 10.0),
    )
    .awake([WOLF, PACK_MATE]);
    let tick = w.tick(true, catch_all());
    let outcome = run_cycle(&mut w, tick);

    assert_eq!(
        outcome
            .rows_visited
            .iter()
            .find(|(key, _)| *key == "assist"),
        Some(&("assist", 0)),
        "no pull this firing is the overwhelmingly common case, so the assist scan must cost the \
         tick nothing at all rather than walking the active set for an empty call list"
    );
}

#[test]
fn a_neighbor_between_two_callers_answers_the_lower_guid() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(-3.0, 0.0, 10.0))
        .creature(PACK_MATE, p(3.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(-3.0, 2.0, 10.0))
        .player(RANGER, p(3.0, 2.0, 10.0));
    w = pack_mate(w, FAR_MATE, p(0.0, 0.0, 10.0)).awake([WOLF, PACK_MATE, FAR_MATE]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pulls(),
        [
            (WOLF, HUNTER, Pull::Noticed),
            (PACK_MATE, RANGER, Pull::Noticed),
            (FAR_MATE, HUNTER, Pull::Assisted)
        ],
        "a neighbor equally close to two calls must always join the same one — a hash-order \
         tie-break makes the same pull send the pack at a different player from run to run"
    );
}

#[test]
fn every_unit_in_a_covered_fight_is_flagged_in_combat_and_an_aiming_player_is_not() {
    for (scope, flagged) in [
        (catch_all(), vec![HUNTER, WOLF]),
        (TickScope::Only(7), Vec::new()),
    ] {
        let mut w = Scenario::new(HALF_WAY)
            .creature(WOLF, p(0.0, 0.0, 10.0))
            .player(HUNTER, p(2.0, 0.0, 10.0))
            .player(RANGER, p(30.0, 0.0, 10.0))
            .attacking(WOLF, HUNTER)
            .aiming_at(RANGER, WOLF);
        let tick = w.tick(false, scope);
        run_cycle(&mut w, tick);

        assert_eq!(
            w.flagged(),
            flagged,
            "both sides of a live fight carry the combat flag and a refreshed drop deadline, on \
             every firing rather than only the sensing ones; an auto-attack toggle aimed at \
             something 30 yards off is not combat and must leave the player free to walk away"
        );
    }
}

#[test]
fn a_creature_that_pulls_is_in_combat_before_the_same_cycle_ends() {
    let mut w = wolf_and_player(p(5.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.flagged(),
        [HUNTER, WOLF],
        "aggro IS combat: a creature that pulled must not wait a whole firing for its flag, or the \
         player sees a mob running at them while still reading as out of combat"
    );
}

/// A clock late enough that a unit which has never moved reads as STANDING STILL: chase plants a
/// creature only next to a victim whose move clock has gone quiet for `CHASE_TARGET_MOVING_MS`.
const SETTLED: u64 = 4_000_000;

/// A wolf at the origin and a hostile player, on the settled clock — the shape every fight scenario
/// starts from. The wolf is NOT awake: chase is driven by the engagements, not by the sweep.
fn wolf_fighting(player_at: Point) -> Scenario {
    Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, player_at)
}

#[test]
fn a_creature_that_notices_a_player_closes_on_it_in_the_same_cycle() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0)).awake([WOLF]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(w.pulls(), [(WOLF, HUNTER, Pull::Noticed)]);
    let effects = w.effects();
    assert_eq!(
        effects.iter().map(|e| (e.dest, e.run)).collect::<Vec<_>>(),
        [(p(11.0, 0.0, 10.0), true)],
        "a creature that pulls must start closing in the SAME firing, at a run and to just inside \
         melee reach; waiting a firing to move is a mob that stares at the player it just aggroed"
    );
}

#[test]
fn an_engaged_creature_outside_every_active_cell_still_chases() {
    for (scope, legs) in [(catch_all(), 1), (TickScope::Only(7), 0)] {
        let mut w = wolf_fighting(p(15.0, 0.0, 10.0)).attacking(WOLF, HUNTER);
        let tick = w.tick(false, scope);
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "a fight is driven by its engagement, not by the sweep, or a player freezes the mob on \
             them by walking out of every active cell; a firing that does not cover the fight's \
             instance must still leave it alone, or two rows step the same chaser at double speed"
        );
    }
}

#[test]
fn a_victim_past_the_pursuit_cutoff_is_left_alone() {
    let cutoff = CHASE_LEASH_SQ.sqrt();
    for (dist, legs) in [(cutoff - 5.0, 1), (cutoff + 5.0, 0)] {
        let mut w = wolf_fighting(p(dist, 0.0, 10.0)).attacking(WOLF, HUNTER);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "the pursuit timer ends a fight, distance does not — but a chaser that keeps closing \
             past the cutoff walks out of its own active cell and drags the fight across the zone \
             ({dist} yards)"
        );
    }
}

#[test]
fn a_chaser_rides_one_committed_leg_until_its_victim_veers() {
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .kiting(HUNTER, p(20.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    // The kiter keeps running straight: the chaser is already pointed at it.
    w.advance_clock(500_000);
    w = w.kiting(HUNTER, p(23.0, 0.0, 10.0));
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        1,
        "a chaser must RIDE its committed leg while the kiter holds its heading; re-throwing a leg \
         every firing makes the client re-compute the path each time, which is the visible jitter \
         the committed leg exists to remove"
    );

    // The kiter cuts away: the held heading no longer points at it.
    w.advance_clock(500_000);
    w = w.kiting(HUNTER, p(23.0, 20.0, 10.0));
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        2,
        "a chaser that never re-aims loses a kiter the moment they turn — the leg must be re-thrown \
         once the victim veers off it"
    );
}

#[test]
fn a_creature_that_reaches_a_standing_victim_stops_and_faces_it() {
    // Half way through a leg thrown at the player, who has since stopped two yards ahead of it.
    let mut w = wolf_fighting(p(7.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER)
        .flying(
            WOLF,
            p(0.0, 0.0, 10.0),
            p(10.0, 0.0, 10.0),
            SETTLED - 500_000,
            LEG_MS,
        );
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let stop_and_turn: Vec<(Point, u32, bool, f32)> = w
        .effects()
        .iter()
        .map(|e| (e.dest, e.dur_ms, e.facing, e.facing_angle))
        .collect();
    assert_eq!(
        stop_and_turn,
        [
            (p(5.0, 0.0, 10.0), 0, false, 0.0),
            (p(5.0, 0.0, 10.0), 0, true, 0.0),
        ],
        "a lead-chase aimed PAST the victim would ride on through them, so the client has to be \
         told to stop where the server planted the creature; and a spline is the only thing a \
         client derives creature facing from, so without the zero-duration turn the mob fights the \
         whole fight looking the way it came"
    );
    assert_eq!(
        w.at(WOLF).orientation,
        0.0,
        "the authoritative heading must move with the turn, or the next firing turns again"
    );

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        2,
        "a settled fight must go SILENT: a stop or a turn re-sent every firing is a packet per \
         creature per tick for every mob standing in melee"
    );
}

#[test]
fn a_creature_whose_victim_kites_out_of_reach_keeps_chasing() {
    // Inside melee reach, but running: vanilla mobs run a kiter down and swing on the move.
    let mut w = wolf_fighting(p(4.0, 0.0, 10.0))
        .kiting(HUNTER, p(4.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.dest, e.dur_ms > 0, e.run))
            .collect::<Vec<_>>(),
        [(p(12.0, 0.0, 10.0), true, true)],
        "planting into attack stance the moment a moving victim touches melee reach is the \
         run/stand/run flicker of chasing someone who never stops; the mob must keep running and \
         aim PAST them"
    );
}

#[test]
fn an_offensive_caster_holds_at_its_spell_range() {
    let cases: [(&str, Twist, usize); 3] = [
        ("inside its range", |w| w, 0),
        (
            "its victim stepped out of range",
            |w| w.tweak_player(HUNTER, |hunter| hunter.at = p(30.0, 0.0, 10.0)),
            1,
        ),
        ("a wall between them", |w| w.wall_between(WOLF, HUNTER), 1),
    ];
    for (case, twist, legs) in cases {
        let mut w = twist(
            wolf_fighting(p(20.0, 0.0, 10.0))
                .caster(WOLF, 30.0)
                .attacking(WOLF, HUNTER),
        );
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "a caster that face-tanks instead of standing at its range never casts the spell it \
             was authored around; one that holds through a wall it cannot cast through stands \
             there doing nothing at all ({case})"
        );
    }
}

#[test]
fn a_routing_or_suppressed_creature_is_not_moved_by_chase() {
    let held: [(&str, Twist); 2] = [
        ("routing", |w| w.routing(WOLF)),
        ("rooted", |w| w.rooted(WOLF)),
    ];
    for (case, hold) in held {
        let mut w = hold(wolf_fighting(p(15.0, 0.0, 10.0)).attacking(WOLF, HUNTER));
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.effects().is_empty(),
            "the rout leg and fear are the SOLE movers of the creatures they own; a chase leg in \
             the same firing shares their spline id and the client throws one of the two away \
             ({case})"
        );
    }
}

#[test]
fn a_blocked_chase_goes_around_instead_of_through() {
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .detour(WOLF, (0.0, 20.0));
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().first().map(|e| e.dest),
        Some(p(0.0, 16.0, 10.0)),
        "a chase leg must head for the detour corner navigation returns, or the mob walks into the \
         geometry between it and its victim and stands there swinging at nothing"
    );
}
