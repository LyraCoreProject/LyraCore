//! In-memory creature world for the behavior cycle: no database, no `ReducerContext`. A test
//! describes creatures and their legs, runs one cycle, and reads back the authoritative state plus
//! the ordered movement effects a client would have received.

use super::*;
use crate::combat::MOVE_FLAG_FORWARD;
use crate::creatures::ai::ROUT_DURATION_MS;
use crate::creatures::{chase_step, rout_window_open};
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
    /// What a heal-when-low rotation line reads. Full health unless a scenario hurts it.
    health: u32,
    max_health: u32,
    /// The power bar regeneration ticks. `max_power` 0 — every creature — has no bar at all.
    power: u32,
    max_power: u32,
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

/// What the pet phase did to one pet, in the order it did it. The follow leg is NOT here — it goes
/// through the shared leg writer and shows up in `effects()` like every other movement.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PetEffect {
    Took(u64, u64),
    StoodDown(u64),
    Dismissed(u64),
    Restaged(u64, Point, u32, u64),
    /// The owner's stale ATTACK order was cleared.
    OrderCleared(u64),
}

#[derive(Default)]
struct Scenario {
    creatures: RefCell<HashMap<u64, XCreature>>,
    legs: RefCell<Vec<LegInFlight>>,
    /// FROZEN by crowd control — a stun, root or polymorph. Fear is the other axis, below.
    frozen: RefCell<HashSet<u64>>,
    /// Live fear auras, `(feared unit, caster)`, in the order the world reads them: one row each, so
    /// a creature feared twice really is listed twice.
    fears: RefCell<Vec<(u64, u64)>>,
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
    /// The last carrier row each creature wrote, and the firing clock it was written at. A creature
    /// has ONE spline row and a subscriber sees only a transaction's net change, so a second write
    /// in one firing would reach no client at all — [`Scenario::record`] refuses it.
    last_carrier: RefCell<HashMap<u64, (u64, MoveEffect)>>,
    /// The players in the world, in the order the world reads them.
    players: RefCell<Vec<AggroTarget>>,
    /// Live melee engagements — what aggro arms, and what combat entry and chase read.
    fights: RefCell<Vec<Engagement>>,
    /// Each engagement's rout clock, `creature -> the ms its window closes at`. Absent is the
    /// never-routed sentinel; the rout phase is the only thing that writes it.
    rout_clock: RefCell<HashMap<u64, u32>>,
    /// The longest offensive spell range a creature fights from; absent means melee-only.
    hold_ranges: RefCell<HashMap<u64, f32>>,
    /// Determinism input: when each PLAYER last moved. A creature's own move clock is on its row.
    player_moved_ms: RefCell<HashMap<u64, u32>>,
    /// Determinism input: each PLAYER's live movement flags, as its heartbeats stamp them. A creature
    /// stamps none — the leg the module moves it along is what says it travels.
    player_move_flags: RefCell<HashMap<u64, u32>>,
    /// Faction pairs at war, directed. Anything absent is not hostile, as missing data is.
    at_war: RefCell<HashSet<(u32, u32)>>,
    /// `(looker, unseen)` pairs with something solid in between.
    blind: RefCell<HashSet<(u64, u64)>>,
    /// Ordered engagements the cycle armed, oldest first.
    pulls: RefCell<Vec<(u64, u64, Pull)>>,
    /// Ordered units the cycle flagged in combat, oldest first.
    flagged: RefCell<Vec<u64>>,
    /// The in-combat flag as state: `unit -> the ms its flag stops being earned`. Combat entry
    /// stamps it, combat exit clears it, the way the world's own flag works.
    combat_flags: RefCell<HashMap<u64, u64>>,
    /// Ordered units the cycle dropped out of combat, oldest first.
    unflagged: RefCell<Vec<u64>>,
    /// Units carrying a combat-regen aura — the only ones that heal while they fight.
    combat_regen: RefCell<HashSet<u64>>,
    /// Authored spell rotations, per creature (the world keys them by template).
    rotations: RefCell<HashMap<u64, Vec<SpellOption>>>,
    /// The single spell a creature with no rotation falls back to.
    lone_spells: RefCell<HashMap<u64, u32>>,
    /// `(unit, spell)` auras already on a unit — what a missing-aura condition reads.
    auras: RefCell<HashSet<(u64, u32)>>,
    /// Creatures with a cast bar already running.
    casting: RefCell<HashSet<u64>>,
    /// Fault injection: spells the spell module refuses — on cooldown, unaffordable or out of range.
    not_ready: RefCell<HashSet<u32>>,
    /// Ordered casts the cycle began, oldest first.
    casts: RefCell<Vec<CastEffect>>,
    /// The threat table, `(creature, source) -> threat`.
    threat: RefCell<HashMap<(u64, u64), i64>>,
    /// Live taunt locks: the creature is pinned on this unit whatever the threat table says.
    taunts: RefCell<HashMap<u64, u64>>,
    /// Live pets, `pet -> owner`. A pet is an ordinary creature row plus this link.
    pet_owners: RefCell<HashMap<u64, u64>>,
    /// The pet bar per OWNER; an absent entry is the vanilla default (Follow + Defensive).
    pet_bars: RefCell<HashMap<u64, (PetCommand, PetReact)>>,
    /// Creatures killed but not yet removed. Only the pet phase reads this: a pet has no spawn row,
    /// so corpse decay never reaps it.
    corpses: RefCell<HashSet<u64>>,
    /// Ordered pet effects, oldest first.
    pet_effects: RefCell<Vec<PetEffect>>,
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
                health: 100,
                max_health: 100,
                power: 0,
                max_power: 0,
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

    /// The creature is already routing: wounded, of a kind that runs, and inside an open window.
    fn routing(self, guid: u64) -> Self {
        let ends_ms = (self.now_micros.get() / 1000) as u32 + ROUT_DURATION_MS;
        self.rout_clock.borrow_mut().insert(guid, ends_ms);
        self.wounded_runner(guid)
    }

    /// Hurt past the rout threshold and of a kind that runs — `rout_eligible` in the world.
    fn wounded_runner(self, guid: u64) -> Self {
        self.hurt(guid, 10).near_death(guid)
    }

    /// `caster` has fear up on `guid`. Called twice, the creature carries two live fear auras.
    fn feared_by(self, guid: u64, caster: u64) -> Self {
        self.fears.borrow_mut().push((guid, caster));
        self
    }

    /// The creature is an offensive caster: it fights from `yards` away instead of closing.
    fn caster(self, guid: u64, yards: f32) -> Self {
        self.hold_ranges.borrow_mut().insert(guid, yards);
        self
    }

    /// Author one line of the creature's spell rotation.
    fn rotation_line(self, guid: u64, spell_id: u32, when: CastWhen, priority: u8) -> Self {
        let mut rotations = self.rotations.borrow_mut();
        let lines = rotations.entry(guid).or_default();
        lines.push(SpellOption {
            spell_id,
            when,
            priority,
            authored: lines.len() as u64,
        });
        drop(rotations);
        self
    }

    /// The creature has no rotation, only the one authored spell.
    fn lone_spell(self, guid: u64, spell_id: u32) -> Self {
        self.lone_spells.borrow_mut().insert(guid, spell_id);
        self
    }

    /// The unit already carries this spell's aura.
    fn carrying(self, guid: u64, spell_id: u32) -> Self {
        self.auras.borrow_mut().insert((guid, spell_id));
        self
    }

    /// The creature's cast bar is already running.
    fn mid_cast(self, guid: u64) -> Self {
        self.casting.borrow_mut().insert(guid);
        self
    }

    /// Fault injection: the spell module refuses this spell — on cooldown, unaffordable, out of range.
    fn not_ready(self, spell_id: u32) -> Self {
        self.not_ready.borrow_mut().insert(spell_id);
        self
    }

    fn hurt(self, guid: u64, health: u32) -> Self {
        self.tweak(guid, |c| c.health = health)
    }

    /// `source` has done enough to `creature` to hold this much of its threat table.
    fn threat(self, creature: u64, source: u64, threat: i64) -> Self {
        self.threat.borrow_mut().insert((creature, source), threat);
        self
    }

    /// A live taunt: the creature is pinned on `taunter` for the window's duration.
    fn taunted(self, creature: u64, taunter: u64) -> Self {
        self.taunts.borrow_mut().insert(creature, taunter);
        self
    }

    fn casts(&self) -> Vec<CastEffect> {
        self.casts.borrow().clone()
    }

    /// Who each creature is fighting now, guid-ordered — what retargeting rewrites.
    fn victims(&self) -> Vec<(u64, u64)> {
        let mut pairs: Vec<(u64, u64)> = self
            .fights
            .borrow()
            .iter()
            .map(|f| (f.attacker, f.victim))
            .collect();
        pairs.sort_unstable();
        pairs
    }

    /// The player RUNS to `at`: it stands there, its move clock reads now and it is holding the
    /// forward key, so a chaser treats it as a kiter rather than a planted target.
    fn kiting(self, guid: u64, at: Point) -> Self {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        self.player_moved_ms.borrow_mut().insert(guid, now_ms);
        self.player_move_flags
            .borrow_mut()
            .insert(guid, MOVE_FLAG_FORWARD);
        self.tweak_player(guid, |p| p.at = at)
    }

    /// The player RELEASED the movement key where it stands: its translation flags go to zero while
    /// its move clock keeps reading whenever it last ran.
    fn stopped(self, guid: u64) -> Self {
        self.player_move_flags.borrow_mut().insert(guid, 0);
        self
    }

    /// The player is TURNING IN PLACE — spinning the camera, going nowhere. Vanilla does not count
    /// this as movement, so neither may a chaser.
    fn turning(self, guid: u64) -> Self {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        self.player_moved_ms.borrow_mut().insert(guid, now_ms);
        self.player_move_flags
            .borrow_mut()
            .insert(guid, MOVE_FLAG_TURN_LEFT);
        self
    }

    /// How long ago this player's last movement heartbeat landed.
    fn since_last_move_ms(&self, guid: u64) -> u32 {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        now_ms.wrapping_sub(
            self.player_moved_ms
                .borrow()
                .get(&guid)
                .copied()
                .unwrap_or(0),
        )
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

    /// The unit carries the in-combat flag until `until_ms`, and the sweep harvested it into this
    /// firing's candidates — the two always travel together, because the sweep IS the flag scan.
    fn in_combat_until(self, guid: u64, until_ms: u64) -> Self {
        self.combat_flags.borrow_mut().insert(guid, until_ms);
        self.awake.borrow_mut().in_combat.push(guid);
        self
    }

    /// Is this unit still flagged in combat?
    fn in_combat(&self, guid: u64) -> bool {
        self.combat_flags.borrow().contains_key(&guid)
    }

    fn unflagged(&self) -> Vec<u64> {
        self.unflagged.borrow().clone()
    }

    /// Give the unit a power bar, the way only a player has one.
    fn power(self, guid: u64, power: u32, max_power: u32) -> Self {
        self.tweak(guid, |c| {
            c.power = power;
            c.max_power = max_power;
        })
    }

    /// A combat-regen aura (the Troll Regeneration racial): this unit heals even while it fights.
    fn combat_regen(self, guid: u64) -> Self {
        self.combat_regen.borrow_mut().insert(guid);
        self
    }

    /// Summon `guid` as `owner`'s pet at `at`: a creature row plus the owner link, harvested into
    /// this firing's pet candidates by the sweep.
    fn pet(self, guid: u64, owner: u64, at: Point) -> Self {
        let s = self.creature(guid, at);
        s.awake.borrow_mut().pets.push(guid);
        s.pet_owners.borrow_mut().insert(guid, owner);
        s
    }

    /// The owner's pet bar, as the player left it. An absent entry is the vanilla default
    /// (Follow + Defensive), which is exactly what a pet did before the bar existed.
    fn pet_bar(self, owner: u64, command: PetCommand, react: PetReact) -> Self {
        self.pet_bars.borrow_mut().insert(owner, (command, react));
        self
    }

    /// The creature was killed and its row is still there.
    fn slain(self, guid: u64) -> Self {
        self.corpses.borrow_mut().insert(guid);
        self
    }

    fn pet_effects(&self) -> Vec<PetEffect> {
        self.pet_effects.borrow().clone()
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
        self.frozen.borrow_mut().insert(guid);
        self
    }

    /// This engagement's rout clock, as the phase left it.
    fn rout_ends_ms(&self, guid: u64) -> u32 {
        self.rout_clock.borrow().get(&guid).copied().unwrap_or(0)
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
            sense_secs: crate::creatures::MOVE_TICK_SECS
                * crate::creatures::SENSE_EVERY_N_TICKS as f32,
            scope,
        }
    }

    /// One firing of a schedule row ticking every `interval_micros`, with the cadence resolved from
    /// that interval exactly as `tick_creatures` resolves it, and the scenario clock moved to it.
    fn firing(&self, now_micros: u64, interval_micros: i64, scope: TickScope) -> TickContext {
        self.now_micros.set(now_micros);
        TickContext {
            now_micros,
            now_ms: (now_micros / 1000) as u32,
            tick_secs: crate::creatures::tick_secs_for_interval(interval_micros),
            sense: crate::creatures::is_sense_tick_for_interval(now_micros as i64, interval_micros),
            sense_secs: crate::creatures::sense_period_secs_for_interval(interval_micros),
            scope,
        }
    }
}

impl MotionSink for Scenario {
    fn legs_in_flight(&self) -> Vec<LegInFlight> {
        self.legs.borrow().clone()
    }
    fn movement_suppressed(&self, guid: u64) -> bool {
        self.cc(guid).2
    }
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) {
        self.place(guid, at, Some(moved_ms), None);
    }
    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32) {
        let Some(c) = self.place(leg.guid, at, None, None) else {
            return;
        };
        self.record(MoveEffect {
            guid: leg.guid,
            start: at,
            dest: at,
            dur_ms: 0,
            spline_id,
            run: false,
            map_id: leg.map_id,
            instance_id: leg.instance_id,
            grid: c.grid,
            cell: c.cell,
            facing: false,
            facing_angle: 0.0,
        });
        self.park_leg(leg.guid, at, leg.map_id, leg.instance_id);
    }
    fn drop_leg(&mut self, guid: u64) {
        self.legs.borrow_mut().retain(|l| l.guid != guid);
    }
}

impl Scenario {
    /// The crowd-control lattice, composed the way the world composes it — `spell::cc_blocks` over
    /// this scenario's frozen set and its fear rows: `(action blocked, movement blocked, self
    /// movement suppressed)`.
    fn cc(&self, guid: u64) -> (bool, bool, bool) {
        let frozen = self.frozen.borrow().contains(&guid);
        let feared = self.fears.borrow().iter().any(|(unit, _)| *unit == guid);
        crate::spell::cc_blocks(frozen, false, feared, false)
    }

    /// Is this creature ACTIVELY routing — the world's `creature_is_routing`: wounded and of a kind
    /// that runs, inside an open window, and free to move. Chase, the rout leg and the swing pass all
    /// read this one verdict, so the harness answers it in one place too.
    fn is_routing(&self, guid: u64) -> bool {
        let eligible = self
            .creatures
            .borrow()
            .get(&guid)
            .is_some_and(|c| c.would_rout);
        let now_ms = (self.now_micros.get() / 1000) as u32;
        eligible && rout_window_open(now_ms, self.rout_ends_ms(guid)) && !self.cc(guid).2
    }

    /// The state mirror behind every position write, matching `CtxWorld::place`: the row moves, and
    /// the move clock and the facing move with it only when the caller says so.
    fn place(
        &self,
        guid: u64,
        at: Point,
        moved_ms: Option<u32>,
        orientation: Option<f32>,
    ) -> Option<XCreature> {
        let mut creatures = self.creatures.borrow_mut();
        let c = creatures.get_mut(&guid)?;
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        c.at = at;
        c.grid = (gx, gy);
        c.cell = spatial::grid_cell_id(gx, gy);
        if let Some(ms) = moved_ms {
            c.last_move_ms = ms;
        }
        if let Some(rad) = orientation {
            c.orientation = rad;
        }
        Some(*c)
    }

    /// The one place a `MoveEffect` is recorded, and the rule production gets from the schema: a
    /// creature has ONE `game_creature_spline` row, so two writes for it in one firing reach a
    /// client as the second alone. The Fake refuses rather than recording a relay that cannot happen.
    fn record(&self, effect: MoveEffect) {
        let now = self.now_micros.get();
        if let Some((wrote_at, first)) = self.last_carrier.borrow().get(&effect.guid) {
            assert!(
                *wrote_at != now,
                "guid {} wrote two carrier rows in one firing: {first:?} then {effect:?} — only \
                 the second would ever reach a client, so the first is a decision the player \
                 never sees",
                effect.guid
            );
        }
        self.last_carrier
            .borrow_mut()
            .insert(effect.guid, (now, effect));
        self.effects.borrow_mut().push(effect);
    }

    /// The zero-duration row a stop leaves behind. It REPLACES the leg it interrupts (the writer
    /// upserts one row per mover), so the next cycle reads it as landed and reaps it instead of
    /// resuming the old destination.
    fn park_leg(&self, guid: u64, at: Point, map_id: u32, instance_id: u64) {
        let mut legs = self.legs.borrow_mut();
        legs.retain(|l| l.guid != guid);
        legs.push(LegInFlight {
            guid,
            start: at,
            dest: at,
            started_micros: self.now_micros.get(),
            dur_ms: 0,
            map_id,
            instance_id,
            mover_gone: false,
        });
    }

    /// A fight's victim wherever it stands — a creature row or a player — as its position and live
    /// translation flags. Chase reads both: a kiter is run down, a planted one is stood next to and
    /// faced. A creature carries no flags of its own, so the leg the cycle is moving it along is what
    /// says it travels, exactly as the production adapter derives it.
    fn unit(&self, guid: u64) -> Option<(Point, u32)> {
        if let Some(c) = self.creatures.borrow().get(&guid) {
            let carried = self
                .legs
                .borrow()
                .iter()
                .any(|l| l.guid == guid && l.dur_ms > 0);
            return Some((c.at, if carried { MOVE_FLAG_FORWARD } else { 0 }));
        }
        let flags = self.player_move_flags.borrow().get(&guid).copied();
        self.players
            .borrow()
            .iter()
            .find(|p| p.guid == guid)
            .map(|p| (p.at, flags.unwrap_or(0)))
    }

    /// A pet's owner as the pet reads it. `None` for an owner that logged out or died — the two are
    /// the same despawn to a pet.
    fn pet_owner(&self, guid: u64) -> Option<PetOwner> {
        let (at, map_id, instance_id) = self
            .players
            .borrow()
            .iter()
            .find(|p| p.guid == guid && !p.dead)
            .map(|p| (p.at, p.map_id, p.instance_id))
            .or_else(|| {
                self.creatures
                    .borrow()
                    .get(&guid)
                    .filter(|_| !self.corpses.borrow().contains(&guid))
                    .map(|c| (c.at, c.map_id, c.instance_id))
            })?;
        // A live melee row IS the owner's fight. The in-combat-selection fallback the world also
        // reads (`pet::owner_combat_target`) needs no scenario shape of its own — either way this
        // is "the enemy the owner is fighting".
        let combat_target = self
            .fights
            .borrow()
            .iter()
            .find(|f| f.attacker == guid)
            .map(|f| f.victim)
            .unwrap_or(0);
        Some(PetOwner {
            guid,
            at,
            map_id,
            instance_id,
            combat_target,
        })
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
        self.record(MoveEffect {
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
                cannot_act: c.cannot_act || self.cc(guid).0,
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
    /// The instance is the ATTACKER's, read live — the world reads it off the attacker's own entity
    /// row, so a scenario that puts a creature in an instance puts its fight there too.
    fn engagements(&self) -> Vec<Engagement> {
        let creatures = self.creatures.borrow();
        self.fights
            .borrow()
            .iter()
            .map(|f| Engagement {
                instance_id: creatures
                    .get(&f.attacker)
                    .map_or(f.instance_id, |c| c.instance_id),
                ..*f
            })
            .collect()
    }
    fn enter_combat(&mut self, guid: u64) {
        self.flagged.borrow_mut().push(guid);
        let now_ms = self.now_micros.get() / 1000;
        self.combat_flags
            .borrow_mut()
            .insert(guid, now_ms + crate::combat::COMBAT_DROP_MS);
    }
    fn flagged_in_combat(&self, candidates: &[u64]) -> Vec<Combatant> {
        let flags = self.combat_flags.borrow();
        candidates
            .iter()
            .filter_map(|guid| {
                flags.get(guid).map(|until_ms| Combatant {
                    guid: *guid,
                    combat_until_ms: *until_ms,
                })
            })
            .collect()
    }
    fn leave_combat(&mut self, guid: u64) {
        self.unflagged.borrow_mut().push(guid);
        self.combat_flags.borrow_mut().remove(&guid);
    }
}

/// What a full out-of-combat tick recovers here. The RATES are `combat::tables`' in production;
/// this stands in for them, because what the cycle decides is WHO recovers and WHEN, never how much.
const REGEN_TICK: u32 = 5;
/// What a combat-regen aura leaves of that tick — a fraction, as the in-combat rate is.
const COMBAT_REGEN_TICK: u32 = 1;

// The in-memory regeneration world. A scenario holds only creature rows, so a unit with a power bar
// stands in for the player half the production pass also covers.
impl RegenSink for Scenario {
    fn recovering(&self, flagged: &[u64]) -> Vec<Recovering> {
        let fights = self.fights.borrow();
        let corpses = self.corpses.borrow();
        let mut units: Vec<Recovering> = self
            .creatures
            .borrow()
            .iter()
            .filter(|(guid, c)| {
                !corpses.contains(*guid) && (c.health < c.max_health || c.max_power > 0)
            })
            .map(|(guid, c)| Recovering {
                guid: *guid,
                health: c.health,
                max_health: c.max_health,
                power: c.power,
                max_power: c.max_power,
                // Both halves the world reads: either side of a live fight, plus the flag the sweep
                // harvested for a unit with no melee row of its own.
                in_combat: flagged.contains(guid)
                    || fights
                        .iter()
                        .any(|f| f.attacker == *guid || f.victim == *guid),
            })
            .collect();
        units.sort_unstable_by_key(|u| u.guid);
        units
    }
    fn healed_to(&self, u: &Recovering) -> u32 {
        (u.health + REGEN_TICK).min(u.max_health)
    }
    fn combat_healed_to(&self, u: &Recovering) -> Option<u32> {
        self.combat_regen
            .borrow()
            .contains(&u.guid)
            .then(|| (u.health + COMBAT_REGEN_TICK).min(u.max_health))
    }
    fn powered_to(&self, u: &Recovering) -> u32 {
        (u.power + REGEN_TICK).min(u.max_power)
    }
    fn restore(&mut self, guid: u64, health: Option<u32>, power: Option<u32>) {
        let mut creatures = self.creatures.borrow_mut();
        let Some(c) = creatures.get_mut(&guid) else {
            return;
        };
        c.health = health.unwrap_or(c.health);
        c.power = power.unwrap_or(c.power);
    }
}

// The in-memory casting world. The spell module is a fake: `begin_cast` records the action and
// reports "not ready" for the spells a scenario put on cooldown, which is all the cycle reads back.
impl CastSink for Scenario {
    fn casters(&self, scope: &TickScope) -> Vec<Caster> {
        let creatures = self.creatures.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                scope.covers(c.instance_id).then_some(Caster {
                    guid: f.attacker,
                    victim: f.victim,
                    victim_at: self.unit(f.victim).map(|(at, _)| at),
                    level: c.level as u8,
                    health: c.health,
                    max_health: c.max_health,
                    cannot_act: c.cannot_act || self.cc(f.attacker).0,
                    casting: self.casting.borrow().contains(&f.attacker),
                })
            })
            .collect()
    }
    fn rotation_of(&self, guid: u64) -> Vec<SpellOption> {
        self.rotations
            .borrow()
            .get(&guid)
            .cloned()
            .unwrap_or_default()
    }
    fn lone_spell(&self, guid: u64) -> Option<u32> {
        self.lone_spells.borrow().get(&guid).copied()
    }
    fn carries(&self, guid: u64, spell_id: u32) -> bool {
        self.auras.borrow().contains(&(guid, spell_id))
    }
    fn begin_cast(&mut self, caster: &Caster, spell_id: u32, target: u64) -> bool {
        if self.not_ready.borrow().contains(&spell_id) {
            return false;
        }
        self.casts
            .borrow_mut()
            .push((caster.guid, spell_id, target));
        self.auras.borrow_mut().insert((target, spell_id));
        true
    }
}

// The in-memory threat world. The compare and the taunt window are the real `crate::threat`'s in
// production; here a scenario states the table and the lock outright.
impl ThreatSink for Scenario {
    fn fighters(&self, scope: &TickScope) -> Vec<Fighter> {
        let creatures = self.creatures.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                scope.covers(c.instance_id).then_some(Fighter {
                    guid: f.attacker,
                    victim: f.victim,
                })
            })
            .collect()
    }
    fn taunted_onto(&self, guid: u64) -> Option<u64> {
        self.taunts.borrow().get(&guid).copied()
    }
    fn top_threat(&self, guid: u64) -> Option<u64> {
        self.threat
            .borrow()
            .iter()
            .filter(|((creature, _), _)| *creature == guid)
            // Highest threat wins; the lowest guid breaks a tie, as the threat table does.
            .max_by(|a, b| a.1.cmp(b.1).then(b.0 .1.cmp(&a.0 .1)))
            .map(|((_, source), _)| *source)
    }
    fn threat_on(&self, creature: u64, source: u64) -> i64 {
        self.threat
            .borrow()
            .get(&(creature, source))
            .copied()
            .unwrap_or(0)
    }
    fn retarget(&mut self, creature: u64, victim: u64) {
        for f in self.fights.borrow_mut().iter_mut() {
            if f.attacker == creature {
                f.victim = victim;
            }
        }
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
                let (victim_at, victim_movement_flags) = self.unit(f.victim)?;
                scope.covers(c.instance_id).then_some(Pursuit {
                    guid: f.attacker,
                    at: c.at,
                    orientation: c.orientation,
                    victim_at,
                    victim_movement_flags,
                    routing: self.is_routing(f.attacker),
                    leg: legs.iter().find(|l| l.guid == f.attacker).cloned(),
                })
            })
            .collect()
    }
    fn caster_hold_range(&self, guid: u64) -> f32 {
        self.hold_ranges.borrow().get(&guid).copied().unwrap_or(0.0)
    }
    /// The stop AND the turn, as one carrier row: the creature is planted at `at` looking at
    /// `orientation`, and the zero-duration facing row replaces the leg it was riding, exactly as
    /// the production writer's upsert does.
    fn face(&mut self, guid: u64, at: Point, orientation: f32, spline_id: u32) {
        let Some(c) = self.place(guid, at, None, Some(orientation)) else {
            return;
        };
        self.record(MoveEffect {
            guid,
            start: at,
            dest: at,
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
        self.park_leg(guid, at, c.map_id, c.instance_id);
    }
}

// The in-memory rout world. A fight whose attacker is not a creature row is a PLAYER's own attack
// row, which the production adapter drops for the same reason: only creatures rout.
impl RoutSink for Scenario {
    fn routers(&self, scope: &TickScope) -> Vec<Router> {
        let creatures = self.creatures.borrow();
        let legs = self.legs.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                scope.covers(c.instance_id).then_some(Router {
                    guid: f.attacker,
                    at: c.at,
                    victim: f.victim,
                    victim_at: self.unit(f.victim).map(|(at, _)| at),
                    health: c.health,
                    max_health: c.max_health,
                    eligible: c.would_rout,
                    rout_ends_ms: self.rout_ends_ms(f.attacker),
                    routing: self.is_routing(f.attacker),
                    committed: legs.iter().any(|l| l.guid == f.attacker),
                })
            })
            .collect()
    }
    fn start_rout(&mut self, guid: u64, ends_ms: u32) {
        self.rout_clock.borrow_mut().insert(guid, ends_ms);
    }
}

// The in-memory fear world: one candidate per live fear aura, as the world's aura scan yields them.
impl FearSink for Scenario {
    fn panicked(&self, scope: &TickScope) -> Vec<Panicked> {
        let creatures = self.creatures.borrow();
        let fears = self.fears.borrow();
        fears
            .iter()
            .filter_map(|(guid, _)| {
                let c = creatures.get(guid)?;
                // Which caster wins is the FIRST fear row on the unit, not this row's — the same
                // tie-break `spell::fear_source` makes.
                let (_, caster) = fears.iter().find(|(unit, _)| unit == guid)?;
                // The caster if it is still in the world; failing that, whatever the creature was
                // fighting — it still flees SOMETHING.
                let source = self.unit(*caster).map(|(at, _)| at).or_else(|| {
                    let victim = self
                        .fights
                        .borrow()
                        .iter()
                        .find(|f| f.attacker == *guid)
                        .map(|f| f.victim)?;
                    self.unit(victim).map(|(at, _)| at)
                });
                scope.covers(c.instance_id).then_some(Panicked {
                    guid: *guid,
                    at: c.at,
                    source_at: source,
                    frozen: self.cc(*guid).1,
                })
            })
            .collect()
    }
}

// The in-memory pet world. An owner is a player in production; the harness resolves a creature row
// too, and a gone-or-dead owner reads as `None` either way — which is what despawns the pet.
impl PetSink for Scenario {
    fn pets(&self, scope: &TickScope, candidates: &[u64]) -> Vec<Pet> {
        let creatures = self.creatures.borrow();
        let owners = self.pet_owners.borrow();
        let bars = self.pet_bars.borrow();
        candidates
            .iter()
            .filter_map(|guid| {
                let c = creatures.get(guid)?;
                let owner_guid = *owners.get(guid)?;
                let (command, react) = bars
                    .get(&owner_guid)
                    .copied()
                    .unwrap_or((PetCommand::Follow, PetReact::Defensive));
                scope.covers(c.instance_id).then_some(Pet {
                    guid: *guid,
                    at: c.at,
                    map_id: c.map_id,
                    instance_id: c.instance_id,
                    owner: self.pet_owner(owner_guid),
                    dead: self.corpses.borrow().contains(guid),
                    suppressed: self.cc(*guid).2,
                    command,
                    react,
                    fighting: self.fights.borrow().iter().any(|f| f.attacker == *guid),
                })
            })
            .collect()
    }
    fn pet_may_attack(&self, pet: &Pet, target: u64) -> bool {
        let Some(owner) = pet.owner else {
            return false;
        };
        if target == 0 || target == pet.guid || target == owner.guid {
            return false;
        }
        // Only a live CREATURE row is a valid foe: a player, a corpse and another pet are all out.
        // The FACTION half of the guard is the world's (`pet::may_attack`) — a scenario states who
        // exists, not who is friendly to whom.
        self.creatures.borrow().contains_key(&target)
            && !self.corpses.borrow().contains(&target)
            && !self.pet_owners.borrow().contains_key(&target)
    }
    /// The seek RADIUS is the world's (`pet::nearest_hostile_near`); what the cycle decides is WHEN
    /// to ask, so the scenario answers with the nearest foe it holds at all.
    fn nearest_hostile_to(&self, pet: &Pet) -> Option<u64> {
        let creatures = self.creatures.borrow();
        creatures
            .iter()
            .filter(|(guid, c)| {
                self.pet_may_attack(pet, **guid)
                    && (c.map_id, c.instance_id) == (pet.map_id, pet.instance_id)
            })
            .min_by(|a, b| {
                dist_sq(a.1.at, pet.at)
                    .partial_cmp(&dist_sq(b.1.at, pet.at))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(b.0))
            })
            .map(|(guid, _)| *guid)
    }
    fn cancel_attack_order(&mut self, owner: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::OrderCleared(owner));
        let mut bars = self.pet_bars.borrow_mut();
        if let Some((command, _)) = bars.get_mut(&owner) {
            *command = PetCommand::Follow;
        }
    }
    fn take_victim(&mut self, pet: u64, victim: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::Took(pet, victim));
        let instance_id = self.creatures.borrow()[&pet].instance_id;
        let mut fights = self.fights.borrow_mut();
        match fights.iter_mut().find(|f| f.attacker == pet) {
            Some(f) => f.victim = victim,
            None => fights.push(Engagement {
                attacker: pet,
                victim,
                instance_id,
                player_never_swung: false,
            }),
        }
    }
    fn stand_down(&mut self, pet: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::StoodDown(pet));
        self.fights.borrow_mut().retain(|f| f.attacker != pet);
    }
    fn dismiss(&mut self, pet: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::Dismissed(pet));
        self.creatures.borrow_mut().remove(&pet);
        self.pet_owners.borrow_mut().remove(&pet);
        self.legs.borrow_mut().retain(|l| l.guid != pet);
        self.fights
            .borrow_mut()
            .retain(|f| f.attacker != pet && f.victim != pet);
    }
    fn restage(&mut self, pet: u64, at: Point, map_id: u32, instance_id: u64, now_ms: u32) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::Restaged(pet, at, map_id, instance_id));
        self.place(pet, at, Some(now_ms), None);
        if let Some(c) = self.creatures.borrow_mut().get_mut(&pet) {
            c.map_id = map_id;
            c.instance_id = instance_id;
        }
    }
}

const WOLF: u64 = 0x0000_0000_0000_0BEE;
/// A second and third wolf of the same pack, guid-ordered after `WOLF`.
const PACK_MATE: u64 = WOLF + 1;
const FAR_MATE: u64 = WOLF + 2;
const HUNTER: u64 = 0x0000_0000_0000_0A11;
const RANGER: u64 = HUNTER + 1;
/// A warlock's summoned Imp, and the guid its owner carries.
const IMP: u64 = 0x0000_0000_0000_1117;
/// An authored offensive spell, a heal, a debuff and a self-buff.
const NUKE: u32 = 100;
const HEAL: u32 = 101;
const DEBUFF: u32 = 102;
const BUFF: u32 = 103;
const BEASTS: u32 = 14;
const ALLIANCE: u32 = 1;
const MAP: u32 = 0;
const INSTANCE: u64 = 0;
/// A one-second leg launched at t=0, sampled half way through.
const LEG_MS: u32 = 1000;
const HALF_WAY: u64 = 500_000;
/// The 1.12 turn-left bit — a movement flag that is deliberately NOT translation.
const MOVE_FLAG_TURN_LEFT: u32 = 0x10;

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
fn a_badly_displaced_patroller_steps_toward_its_waypoint_one_firing_at_a_time() {
    // Combat dragged the wolf 300 yd off its route while it was already walking to waypoint 2.
    let route = [(1, p(5.0, 5.0, 10.0)), (2, p(0.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 300.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 2);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let effect = w.effects()[0];
    assert_eq!(
        (effect.dest, effect.run, effect.dur_ms),
        (p(0.0, 298.75, 10.0), false, 500),
        "a badly displaced patroller must walk ONE firing's worth toward its waypoint; a \
         whole-distance leg slides it home through terrain over minutes"
    );
    assert_eq!(
        (w.at(WOLF).wp_target, w.at(WOLF).leg_ends_ms),
        (2, 0),
        "the cursor must stay on the same waypoint and the leg must NOT be held to completion, or \
         the next firing cannot re-derive the step from the creature's new position"
    );
}

#[test]
fn a_displaced_patroller_converges_then_resumes_ordinary_patrolling() {
    // Just past the displaced threshold: one stepped firing closes enough of the gap that the next
    // one falls back to an ordinary, held-to-completion leg straight to the waypoint.
    let route = [(1, p(5.0, 5.0, 10.0)), (2, p(0.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 41.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 2);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    w.advance_clock(500_000); // the stepped leg's own duration — it has landed

    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()[0].dest,
        p(0.0, 39.75, 10.0),
        "the first firing must still be a bounded step, not a leap to the waypoint"
    );
    let now_ms = (HALF_WAY / 1000) as u32 + 500;
    assert_eq!(
        (w.effects()[1].dest, w.effects()[1].dur_ms, w.at(WOLF).leg_ends_ms),
        (p(0.0, 0.0, 10.0), 15900, now_ms + 15900),
        "once close enough, patrolling must resume the ordinary single-leg-held-to-completion shape \
         and carry the creature the rest of the way to its waypoint"
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

/// One cast the cycle began: who cast, which spell, and at whom.
type CastEffect = (u64, u32, u64);

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
    let legs = w.effects();
    assert_eq!(
        legs.len(),
        2,
        "a chaser that never re-aims loses a kiter the moment they turn — the leg must be re-thrown \
         once the victim veers off it"
    );
    assert!(
        legs[1].spline_id > legs[0].spline_id,
        "the replacement must carry a NEWER spline id, or the client keeps the obsolete leg and \
         rides it past the victim's new position"
    );
}

#[test]
fn a_creature_that_reaches_a_standing_victim_stops_and_faces_it() {
    // Half way through a leg thrown at the player, who has since stopped two yards ahead of it.
    let launched = SETTLED - 500_000;
    let mut w = wolf_fighting(p(7.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER)
        .flying(
            WOLF,
            p(0.0, 0.0, 10.0),
            p(10.0, 0.0, 10.0),
            launched,
            LEG_MS,
        );
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let rendered = p(5.0, 0.0, 10.0);
    let stop = w.effects();
    assert_eq!(
        stop.iter()
            .map(|e| (e.start, e.dest, e.dur_ms, e.facing, e.facing_angle))
            .collect::<Vec<_>>(),
        [(rendered, rendered, 0, true, 0.0)],
        "the stop and the turn are ONE facing spline at the point the client renders the creature: \
         two rows land on its single spline row inside one transaction, so the client would receive \
         the turn alone and ride the lead leg on through the player it is supposed to be swinging at"
    );
    assert!(
        stop[0].spline_id > (launched / 1000) as u32,
        "the stop must carry a NEWER spline id than the leg it interrupts, or the client keeps the \
         obsolete leg and runs it to its end"
    );
    assert_eq!(
        (w.at(WOLF).at, w.at(WOLF).orientation),
        (rendered, 0.0),
        "server and client must agree on where the creature stopped and which way it now looks"
    );

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        1,
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

/// The window the DISPLACED rule read the victim's move clock over. A stop packet lands well inside
/// it, which is why a clock cannot answer "is this player still running".
const STALE_MOVE_WINDOW_MS: u32 = 700;

/// The bearings a player walks up to a creature from. Nothing about the answer may depend on which
/// side they stand on.
const APPROACHES: [f32; 8] = [
    0.0,
    std::f32::consts::FRAC_PI_4,
    std::f32::consts::FRAC_PI_2,
    2.356_194_5,
    std::f32::consts::PI,
    -2.356_194_5,
    -std::f32::consts::FRAC_PI_2,
    -std::f32::consts::FRAC_PI_4,
];

#[test]
fn a_standing_victim_inside_reach_is_turned_to_from_every_approach() {
    for bearing in APPROACHES {
        let at = p(3.0 * bearing.cos(), 3.0 * bearing.sin(), 10.0);
        let mut w = wolf_fighting(at).facing(WOLF, 0.0).attacking(WOLF, HUNTER);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        let effects = w.effects();
        assert!(
            effects.iter().all(|e| e.facing),
            "a creature already in reach of a standing player must not travel at all: a positional \
             leg here is the mob running to their flank instead of squaring up ({bearing} rad)"
        );
        assert!(
            effects.len() <= 1 && w.at(WOLF).at == p(0.0, 0.0, 10.0),
            "the answer must come from distance and movement, not from the side the player walked \
             up on — one turn at most, and the creature stays where it stands ({bearing} rad)"
        );
    }
}

#[test]
fn a_player_who_just_released_the_movement_key_is_standing_at_once() {
    let mut w = wolf_fighting(p(3.0, 0.0, 10.0))
        .kiting(HUNTER, p(3.0, 0.0, 10.0))
        .stopped(HUNTER)
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    assert!(
        w.since_last_move_ms(HUNTER) < STALE_MOVE_WINDOW_MS,
        "the point of this scenario is a move clock that still reads FRESH"
    );
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.facing, e.dur_ms))
            .collect::<Vec<_>>(),
        [(true, 0)],
        "a stop takes effect on the next covered firing, not once a move clock goes quiet: the \
         creature must turn to the player, and a lead leg thrown at a player who has stopped runs \
         eight yards through them"
    );
}

#[test]
fn a_player_turning_in_place_is_not_translating() {
    let mut w = wolf_fighting(p(3.0, 0.0, 10.0))
        .turning(HUNTER)
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.facing, e.dur_ms))
            .collect::<Vec<_>>(),
        [(true, 0)],
        "spinning the camera goes nowhere, so the creature it is standing next to must square up \
         rather than chase a player who never left the spot"
    );
}

#[test]
fn a_kiter_inside_reach_is_chased_without_a_stop_between_firings() {
    let mut w = wolf_fighting(p(4.0, 0.0, 10.0))
        .kiting(HUNTER, p(4.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    // Three firings of a straight-line kite: the victim keeps its heading, so the committed leg
    // keeps its own.
    for step in 0..3 {
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);
        w.advance_clock(500_000);
        w = w.kiting(HUNTER, p(7.0 + 3.0 * step as f32, 0.0, 10.0));
    }

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.dest, e.run))
            .collect::<Vec<_>>(),
        [(p(12.0, 0.0, 10.0), true)],
        "a genuinely translating victim must be run down on ONE committed leg: a stop between \
         firings is the run/stand/run flicker, and re-throwing the leg is the client re-computing \
         its path every tick"
    );
}

#[test]
fn a_lead_leg_is_stopped_where_it_renders_once_the_victim_stops() {
    let mut w = wolf_fighting(p(6.0, 0.0, 10.0))
        .kiting(HUNTER, p(6.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    // The player releases the key half a firing into the lead leg, inside melee reach of where the
    // creature now renders.
    w.advance_clock(500_000);
    w = w.stopped(HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let rendered = p(3.5, 0.0, 10.0);
    let effects = w.effects();
    assert_eq!(
        effects
            .iter()
            .map(|e| (e.start, e.dest, e.dur_ms, e.facing))
            .collect::<Vec<_>>(),
        [
            (p(0.0, 0.0, 10.0), p(14.0, 0.0, 10.0), 2000, false),
            (rendered, rendered, 0, true),
        ],
        "the leg aimed past a running player has to be ended where the client RENDERS the creature, \
         and the one row that ends it is the facing row; stopping anywhere else puts the server's \
         melee reach somewhere the player cannot see the mob"
    );
    assert!(
        effects[1].spline_id > effects[0].spline_id,
        "the stop must carry a NEWER spline id than the lead leg, or the client keeps the obsolete \
         leg and runs the eight yards past the player anyway"
    );
    assert_eq!(
        (w.at(WOLF).at, w.at(WOLF).orientation),
        (rendered, 0.0),
        "server and client must agree on where the creature stopped and which way it now looks"
    );

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        2,
        "the firing after the stop must be silent, or every mob standing in melee costs a packet \
         per tick"
    );
}

/// The rule the production carrier has by construction, made visible: one `game_creature_spline`
/// row per creature, and a subscriber sees only a transaction's net change. Written as a pair of
/// direct sink calls — the very pair `stand_and_face` used to make — because no scenario can reach
/// the forbidden write any more, and a rule nothing proves detects anything is not a rule.
#[test]
#[should_panic(expected = "wrote two carrier rows in one firing")]
fn the_fake_refuses_a_second_carrier_row_for_one_creature_in_one_firing() {
    let mut w = Scenario::new(SETTLED).creature(WOLF, p(0.0, 0.0, 10.0));
    let leg = LegInFlight {
        guid: WOLF,
        start: p(0.0, 0.0, 10.0),
        dest: p(10.0, 0.0, 10.0),
        started_micros: SETTLED - 500_000,
        dur_ms: LEG_MS,
        map_id: MAP,
        instance_id: INSTANCE,
        mover_gone: false,
    };
    let at = p(5.0, 0.0, 10.0);
    let spline_id = (SETTLED / 1000) as u32;

    w.halt(&leg, at, spline_id);
    w.face(WOLF, at, 0.0, spline_id);
}

#[test]
fn every_chase_leg_travels_at_the_configured_run_speed() {
    let run = lyracore_shared::constants::speeds::RUN;
    for (case, twist) in [
        ("a standing victim", (|w| w) as Twist),
        ("a kiting victim", |w: Scenario| {
            w.kiting(HUNTER, p(15.0, 0.0, 10.0))
        }),
    ] {
        let mut w = twist(wolf_fighting(p(15.0, 0.0, 10.0)).attacking(WOLF, HUNTER));
        let tick = w.firing(SETTLED, 500_000, catch_all());
        run_cycle(&mut w, tick);

        let leg = w.effects();
        assert_eq!(leg.len(), 1, "one run leg on the covered firing ({case})");
        let travelled = ((leg[0].dest.x - leg[0].start.x).powi(2)
            + (leg[0].dest.y - leg[0].start.y).powi(2))
        .sqrt();
        let implied = travelled / (leg[0].dur_ms as f32 / 1000.0);
        assert!(
            leg[0].run && (implied - run).abs() < 0.05,
            "a leg whose distance and duration disagree with the configured run speed is a \
             creature that teleports or crawls: {travelled} yd in {} ms implies {implied} yd/s, \
             not {run} ({case})",
            leg[0].dur_ms
        );
    }
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
fn a_crowd_controlled_creature_is_not_moved_by_chase() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .rooted(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty(),
        "a rooted creature keeps swinging but must not slide toward its victim; the client would \
         show it walking while the server says it is pinned"
    );
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

/// A wolf fighting the hunter with ONE authored spell and 30 yards of spell range — the caster shape
/// the cast phase decides on. The cast phase is a sensing phase, so its scenarios fire `sense`.
fn wolf_casting(player_at: Point) -> Scenario {
    wolf_fighting(player_at)
        .attacking(WOLF, HUNTER)
        .caster(WOLF, 30.0)
        .lone_spell(WOLF, NUKE)
}

#[test]
fn a_caster_at_spell_range_casts_and_stays_where_it_is() {
    let mut w = wolf_casting(p(20.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.casts(),
        [(WOLF, NUKE, HUNTER)],
        "a caster must begin its cast in the SAME firing it is engaged, or the spell it was \
         authored around never goes off"
    );
    assert!(
        w.effects().is_empty() && w.victims() == [(WOLF, HUNTER)],
        "casting decides a spell, not a move and not a fight: a closing leg in the firing it casts \
         drags the caster into melee, and rewriting the engagement would restart the swing"
    );
}

#[test]
fn a_wall_blocked_caster_drops_the_spell_and_closes_instead() {
    let mut w = wolf_casting(p(20.0, 0.0, 10.0)).wall_between(WOLF, HUNTER);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.casts().is_empty() && w.effects().len() == 1,
        "cast and chase must reach ONE verdict about the same line: a caster that holds at range \
         through a wall it cannot cast through stands there doing nothing for the whole fight"
    );
}

#[test]
fn a_caster_that_cannot_act_or_is_already_casting_begins_nothing() {
    let held: [(&str, Twist); 2] = [
        ("stunned", |w| w.crowd_controlled(WOLF)),
        ("already casting", |w| w.mid_cast(WOLF)),
    ];
    for (case, hold) in held {
        let mut w = hold(wolf_casting(p(20.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.casts().is_empty(),
            "a stun has to stop the cast to be worth casting, and re-entering a running cast bar \
             deletes and restarts it every firing, so the spell would never land ({case})"
        );
    }
}

#[test]
fn the_rotation_fires_its_highest_priority_ready_action() {
    // A heal it only wants below half health, over a nuke it can always throw.
    let rotation = |w: Scenario| {
        w.rotation_line(WOLF, HEAL, CastWhen::Hurt(50), 10)
            .rotation_line(WOLF, NUKE, CastWhen::Always, 1)
    };
    let cases: [(&str, Twist, u32, u64); 3] = [
        ("unhurt, so the heal is not eligible", |w| w, NUKE, HUNTER),
        (
            "hurt, so the heal outranks the nuke",
            |w| w.hurt(WOLF, 10),
            HEAL,
            WOLF,
        ),
        (
            "hurt but the heal is on cooldown",
            |w| w.hurt(WOLF, 10).not_ready(HEAL),
            NUKE,
            HUNTER,
        ),
    ];
    for (case, twist, spell_id, target) in cases {
        let mut w = twist(rotation(wolf_casting(p(20.0, 0.0, 10.0))));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.casts(),
            [(WOLF, spell_id, target)],
            "the rotation is an authored priority list: firing the wrong line, or giving up when \
             the top one is on cooldown, is a caster that never uses the spell it was given ({case})"
        );
    }
}

#[test]
fn a_missing_aura_line_waits_until_the_aura_is_actually_missing() {
    let cases: [(&str, Twist, Vec<CastEffect>); 4] = [
        (
            "the victim lacks the debuff",
            |w| w,
            vec![(WOLF, DEBUFF, HUNTER)],
        ),
        (
            "the victim already has it",
            |w| w.carrying(HUNTER, DEBUFF),
            vec![],
        ),
        (
            "the buff has lapsed",
            |w| {
                w.carrying(HUNTER, DEBUFF)
                    .rotation_line(WOLF, BUFF, CastWhen::SelfLacksIt, 1)
            },
            vec![(WOLF, BUFF, WOLF)],
        ),
        (
            "it is already buffed",
            |w| {
                w.carrying(HUNTER, DEBUFF)
                    .rotation_line(WOLF, BUFF, CastWhen::SelfLacksIt, 1)
                    .carrying(WOLF, BUFF)
            },
            vec![],
        ),
    ];
    for (case, twist, casts) in cases {
        let mut w = twist(wolf_casting(p(20.0, 0.0, 10.0)).rotation_line(
            WOLF,
            DEBUFF,
            CastWhen::VictimLacksIt,
            5,
        ));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.casts(),
            casts.as_slice(),
            "a debuff re-applied every firing wastes the whole rotation on a spell that is already \
             running, and a buff never re-applied leaves the creature fighting without it ({case})"
        );
    }
}

#[test]
fn only_a_creature_with_no_rotation_at_all_falls_back_to_its_one_spell() {
    let cases: [(&str, Twist, Vec<CastEffect>); 3] = [
        ("no rotation", |w| w, vec![(WOLF, NUKE, HUNTER)]),
        (
            "a rotation wins over the lone spell",
            |w| w.rotation_line(WOLF, DEBUFF, CastWhen::Always, 1),
            vec![(WOLF, DEBUFF, HUNTER)],
        ),
        (
            "a rotation this server cannot read is still a rotation",
            |w| w.rotation_line(WOLF, DEBUFF, CastWhen::Never, 1),
            vec![],
        ),
    ];
    for (case, twist, casts) in cases {
        let mut w = twist(wolf_casting(p(20.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.casts(),
            casts.as_slice(),
            "the single-spell casters authored before rotations existed must keep casting, and a \
             creature whose rotation this build does not understand must not silently fall back to \
             a spell its author replaced ({case})"
        );
    }
}

#[test]
fn a_creature_re_points_at_a_stronger_threat_and_chases_it_in_the_same_cycle() {
    let mut w = wolf_fighting(p(5.0, 0.0, 10.0))
        .player(RANGER, p(20.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .threat(WOLF, HUNTER, 50)
        .threat(WOLF, RANGER, 100);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.victims(),
        [(WOLF, RANGER)],
        "the creature must fight whoever out-threats its current target, or a healer can never \
         pull a mob off the player it first aggroed"
    );
    assert_eq!(
        w.effects().first().map(|e| e.dest),
        Some(p(16.0, 0.0, 10.0)),
        "the retarget must land BEFORE the movement decision, or the creature spends the firing \
         running at the player it no longer fights and only turns round on the next one"
    );
}

#[test]
fn equal_threat_leaves_the_target_where_it_is() {
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .player(RANGER, p(5.0, 0.0, 10.0))
        .attacking(WOLF, RANGER)
        .threat(WOLF, HUNTER, 50)
        .threat(WOLF, RANGER, 50);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.victims(),
        [(WOLF, RANGER)],
        "the switch needs STRICTLY more threat: on a tie the creature would flip target every \
         firing and swing at nobody"
    );
}

#[test]
fn a_taunt_pins_the_target_whatever_the_threat_table_says() {
    let cases: [(&str, Twist, u64); 2] = [
        (
            "pinned on the weaker source",
            |w| w.taunted(WOLF, HUNTER),
            HUNTER,
        ),
        (
            "yanked onto the taunter",
            |w| w.taunted(WOLF, RANGER),
            RANGER,
        ),
    ];
    for (case, taunt, victim) in cases {
        let mut w = taunt(
            wolf_fighting(p(5.0, 0.0, 10.0))
                .player(RANGER, p(20.0, 0.0, 10.0))
                .attacking(WOLF, HUNTER)
                .threat(WOLF, HUNTER, 50)
                .threat(WOLF, RANGER, 100),
        );
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.victims(),
            [(WOLF, victim)],
            "taunt is a forced target, not a threat bump: a tank whose taunt is overtaken by the \
             next nuke has no way to hold a mob off the group ({case})"
        );
    }
}

/// A wolf swinging at a hunter standing INSIDE melee reach and already faced: chase decides to stand
/// still, so every movement effect in these scenarios is the rout's or fear's own.
fn wolf_cornered() -> Scenario {
    wolf_fighting(p(3.0, 0.0, 10.0)).attacking(WOLF, HUNTER)
}

/// What a healthy creature's run leg over the committed rout distance would take.
fn healthy_rout_ms() -> u32 {
    (FLEE_LEG_YD / lyracore_shared::constants::speeds::RUN * 1000.0) as u32
}

#[test]
fn a_wounded_runner_breaks_off_and_sprints_away_from_its_victim() {
    let mut w = wolf_cornered().wounded_runner(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let now_ms = (SETTLED / 1000) as u32;
    let leg = w.effects();
    assert_eq!(leg.len(), 1, "the rout is the router's SOLE mover");
    assert_eq!(
        (leg[0].dest, leg[0].run),
        (p(-FLEE_LEG_YD, 0.0, 10.0), true),
        "a routing creature must run the full committed dash directly AWAY from what it fights; a \
         short leg re-decided every firing is the flee spin the commit exists to remove"
    );
    assert!(
        leg[0].dur_ms > healthy_rout_ms(),
        "a wounded runner must travel SLOWER than a healthy one, or the player can never close the \
         gap and the mob is uncatchable"
    );
    assert_eq!(
        w.rout_ends_ms(WOLF),
        now_ms + ROUT_DURATION_MS,
        "the rout must be BOUNDED — an unstamped window is a creature that runs until it dies"
    );
    assert!(
        w.flagged().contains(&WOLF) && w.flagged().contains(&HUNTER),
        "routing is a SHARED combat state: without the re-stamp on both sides the combat drop \
         fires mid-run and the pair simply untargets each other"
    );
}

#[test]
fn a_creature_that_fights_to_the_death_never_enters_the_rout_path() {
    // Identically wounded, but a beast/undead/elemental or a pet: it stands and swings.
    let mut w = wolf_cornered().hurt(WOLF, 10);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty() && w.rout_ends_ms(WOLF) == 0,
        "fleeing is SELECTIVE: a Northshire wolf that runs away at 10% health drops the fight the \
         player was winning"
    );
}

#[test]
fn a_router_rides_its_committed_leg_and_re_rolls_only_once_it_ends() {
    let mut w = wolf_cornered().wounded_runner(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let dur_ms = w.effects()[0].dur_ms;

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        1,
        "re-picking an away-bearing every firing snap-rotates the client's facing — the flee spin; \
         the committed leg must play out"
    );
    assert_eq!(
        w.victims(),
        [(WOLF, HUNTER)],
        "a router must stay ENGAGED while it runs, or it can be neither chased down nor killed"
    );

    // The dash lands, and the window is still open: the next one is rolled from where it stopped.
    w.advance_clock(dur_ms as u64 * 1000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        2,
        "a rout whose leg ended mid-window must keep running, not stand still until the window \
         closes"
    );
}

#[test]
fn a_spent_rout_returns_the_creature_to_chasing_and_never_opens_a_second() {
    let mut w = wolf_cornered().wounded_runner(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let opened_at = w.rout_ends_ms(WOLF);

    // Past the window: the dash has landed and the creature is an ordinary attacker again.
    w.advance_clock(ROUT_DURATION_MS as u64 * 1000 + 1_000_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let chase = w.effects()[1];
    assert!(
        chase.dest.x > chase.start.x,
        "once the window closes the creature must turn round and CLOSE on its victim; standing \
         where the rout left it is a mob that never fights again"
    );
    assert_eq!(
        w.rout_ends_ms(WOLF),
        opened_at,
        "the spent clock is what forbids a second rout — re-stamping it would let a creature run \
         for the rest of the fight"
    );
}

#[test]
fn a_feared_creature_is_moved_by_fear_and_by_nothing_else() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .feared_by(WOLF, HUNTER)
        .rolls([u32::MAX / 2]);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let leg = w.effects();
    assert_eq!(
        leg.len(),
        1,
        "every other mover must skip a feared creature: two legs in one firing share a spline id \
         and the client throws one of them away"
    );
    assert!(
        leg[0].dest.x < -14.0 && leg[0].run,
        "a feared creature is force-walked AWAY from whoever feared it, at a run; got {:?}",
        leg[0].dest
    );
    assert_eq!(
        w.victims(),
        [(WOLF, HUNTER)],
        "fear does NOT disengage: when the aura lapses the creature must turn and fight the unit \
         it was already fighting"
    );
}

#[test]
fn two_fear_sources_panic_the_creature_once() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .feared_by(WOLF, HUNTER)
        .feared_by(WOLF, RANGER)
        .rolls([u32::MAX / 2]);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().len(),
        1,
        "a creature two casters feared is still ONE panicking creature; a leg per aura row would \
         double its terror speed and burn a second spline id"
    );
}

#[test]
fn fear_owns_a_wounded_runner_and_leaves_its_rout_window_intact() {
    let cases: [(&str, Twist, bool); 2] = [
        ("about to rout", |w| w.wounded_runner(WOLF), false),
        ("already routing", |w| w.routing(WOLF), true),
    ];
    for (case, twist, was_open) in cases {
        let mut w = twist(
            wolf_fighting(p(15.0, 0.0, 10.0))
                .attacking(WOLF, HUNTER)
                .rolls([u32::MAX / 2]),
        )
        .feared_by(WOLF, HUNTER);
        let before = w.rout_ends_ms(WOLF);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            1,
            "fear owns forced movement outright: a rout leg in the same firing would fight the \
             fear dash for the one spline id ({case})"
        );
        assert_eq!(
            (w.rout_ends_ms(WOLF), before != 0),
            (before, was_open),
            "fear must leave the rout window exactly as it found it, or a creature loses (or \
             silently spends) its one rout to a crowd control that already ended ({case})"
        );
    }
}

#[test]
fn a_crowd_controlled_router_keeps_its_state_and_resumes_when_the_control_ends() {
    let mut w = wolf_cornered().routing(WOLF).rooted(WOLF);
    let open_until = w.rout_ends_ms(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty(),
        "a frozen creature must not move at all — not by chase, not by its own rout"
    );
    assert_eq!(
        (w.victims(), w.rout_ends_ms(WOLF)),
        (vec![(WOLF, HUNTER)], open_until),
        "crowd control suspends the behavior, it does not destroy it: losing the engagement or the \
         window here is a creature that stands still for good once the control lifts"
    );

    w.frozen.borrow_mut().remove(&WOLF); // the root expires
    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().len(),
        1,
        "the creature must resume its rout on the very next firing, inside the window it kept"
    );
}

/// A warlock at the origin with its Imp beside it, on the settled clock. The pet phase is a SENSING
/// phase, so every scenario below fires `sense`.
fn warlock_and_imp(imp_at: Point) -> Scenario {
    Scenario::new(SETTLED)
        .player(HUNTER, p(0.0, 0.0, 10.0))
        .pet(IMP, HUNTER, imp_at)
}

#[test]
fn a_pet_takes_its_owners_foe_and_closes_on_it_in_the_same_cycle() {
    let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .attacking(HUNTER, WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pet_effects(),
        [PetEffect::Took(IMP, WOLF)],
        "a pet exists to fight what its owner fights; one that never arms an engagement is an Imp \
         that follows the warlock around watching them die"
    );
    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.guid, e.dest, e.run))
            .collect::<Vec<_>>(),
        [(IMP, p(16.0, 0.0, 10.0), true)],
        "the pet phase runs before chase, so the pet it just armed must close in the SAME firing \
         through the chase every other creature uses; waiting a firing is a pet that stares at the \
         foe its owner is already fighting"
    );
}

#[test]
fn only_a_pet_left_off_passive_assists_its_owner() {
    for (react, assists) in [
        (PetReact::Passive, false),
        (PetReact::Defensive, true),
        (PetReact::Aggressive, true),
    ] {
        let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
            .creature(WOLF, p(20.0, 0.0, 10.0))
            .attacking(HUNTER, WOLF)
            .pet_bar(HUNTER, PetCommand::Follow, react);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pet_effects() == [PetEffect::Took(IMP, WOLF)],
            assists,
            "PASSIVE is the stance a player picks to keep the pet out of a fight — a pet that \
             piles in anyway pulls the pack the player was sneaking past ({react:?})"
        );
    }
}

#[test]
fn only_an_aggressive_pet_seeks_a_foe_its_owner_is_not_fighting() {
    for (react, seeks) in [(PetReact::Defensive, false), (PetReact::Aggressive, true)] {
        let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
            .creature(WOLF, p(6.0, 0.0, 10.0))
            .pet_bar(HUNTER, PetCommand::Follow, react);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pet_effects() == [PetEffect::Took(IMP, WOLF)],
            seeks,
            "AGGRESSIVE is the only stance that starts a fight nobody asked for; a DEFENSIVE pet \
             that does it too makes the safe stance unusable ({react:?})"
        );
    }
}

#[test]
fn an_attack_order_wins_over_the_owners_fight_and_a_stale_one_is_dropped() {
    let commanded = || {
        warlock_and_imp(p(0.0, 0.0, 10.0))
            .creature(WOLF, p(20.0, 0.0, 10.0))
            .creature(PACK_MATE, p(-20.0, 0.0, 10.0))
            .attacking(HUNTER, WOLF)
            .pet_bar(HUNTER, PetCommand::Attack(PACK_MATE), PetReact::Defensive)
    };

    let mut w = commanded();
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.pet_effects(),
        [PetEffect::Took(IMP, PACK_MATE)],
        "the attack button is an explicit order: a pet that assists its owner's fight instead \
         cannot be sent at anything"
    );

    let mut w = commanded().slain(PACK_MATE);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.pet_effects(),
        [PetEffect::OrderCleared(HUNTER), PetEffect::Took(IMP, WOLF)],
        "an order whose foe died must be dropped, or the pet stands over the corpse for the rest \
         of the fight and never assists again"
    );
}

#[test]
fn a_pet_that_drifted_out_of_the_follow_band_runs_back_and_one_inside_it_holds() {
    for (imp_at, legs) in [(p(-20.0, 0.0, 10.0), 1), (p(-7.0, 0.0, 10.0), 0)] {
        let mut w = warlock_and_imp(imp_at);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "the follow band is what keeps a pet from jogging on the spot beside its owner; \
             following from inside it re-throws a leg every sense firing ({imp_at:?})"
        );
    }

    let mut w = warlock_and_imp(p(-20.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    let leg = w.effects()[0];
    assert_eq!(
        (leg.dest, leg.run),
        (p(-4.0, 0.0, 10.0), true),
        "the pet must RUN back and stop a few yards short of its owner; landing on the owner puts \
         it inside the player's own model, and walking means it never catches up"
    );
    assert_eq!(
        leg.dur_ms,
        (16.0 / lyracore_shared::constants::speeds::RUN * 1000.0) as u32,
        "the leg has to span the whole SENSE period, because that is when the pet next decides — a \
         one-firing step lets a moving owner outrun the pet"
    );
}

#[test]
fn a_pet_whose_owner_is_gone_or_dead_despawns_and_so_does_its_own_corpse() {
    let cases: [(&str, Twist); 2] = [
        ("the owner died", |w| w.corpse(HUNTER)),
        ("the pet was killed", |w| w.slain(IMP)),
    ];
    for (case, twist) in cases {
        let mut w = twist(warlock_and_imp(p(0.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pet_effects(),
            [PetEffect::Dismissed(IMP)],
            "a pet has no spawn row, so nothing else on the tick reaps it: left alone it outlives \
             its owner as an unownable mob, or lingers as a corpse forever ({case})"
        );
    }

    let mut orphan = Scenario::new(SETTLED).pet(IMP, HUNTER, p(0.0, 0.0, 10.0));
    let tick = orphan.tick(true, catch_all());
    run_cycle(&mut orphan, tick);
    assert_eq!(
        orphan.pet_effects(),
        [PetEffect::Dismissed(IMP)],
        "an owner who logged out leaves no row at all, and their pet must go with them"
    );
}

#[test]
fn a_frozen_pet_holds_still_and_a_feared_one_is_moved_by_fear_alone() {
    let mut rooted = warlock_and_imp(p(-20.0, 0.0, 10.0)).rooted(IMP);
    let tick = rooted.tick(true, catch_all());
    run_cycle(&mut rooted, tick);
    assert!(
        rooted.effects().is_empty() && rooted.pet_effects().is_empty(),
        "a rooted pet that still runs home to its owner is a client walking a unit the server says \
         is pinned"
    );

    let mut feared = warlock_and_imp(p(-20.0, 0.0, 10.0))
        .feared_by(IMP, HUNTER)
        .rolls([u32::MAX / 2]);
    let tick = feared.tick(true, catch_all());
    run_cycle(&mut feared, tick);
    assert!(
        feared.pet_effects().is_empty() && feared.effects().len() == 1,
        "fear is the feared pet's SOLE mover; a follow leg in the same firing shares fear's spline \
         id and the client throws one of them away"
    );
    assert!(
        feared.effects()[0].dest.x < -20.0,
        "the surviving leg must be the fear dash AWAY from the warlock, not the follow leg back \
         toward them; got {:?}",
        feared.effects()[0].dest
    );
}

#[test]
fn a_pet_told_to_stay_stops_trailing_its_owner_but_still_fights() {
    let stay = |w: Scenario| w.pet_bar(HUNTER, PetCommand::Stay, PetReact::Defensive);

    let mut w = stay(warlock_and_imp(p(-20.0, 0.0, 10.0)));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert!(
        w.effects().is_empty(),
        "STAY is how a player parks a pet on a spot; one that trails them anyway cannot be left \
         behind to hold a pull"
    );

    let mut w = stay(
        warlock_and_imp(p(-20.0, 0.0, 10.0))
            .creature(WOLF, p(20.0, 0.0, 10.0))
            .attacking(HUNTER, WOLF),
    );
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.pet_effects(),
        [PetEffect::Took(IMP, WOLF)],
        "STAY governs movement, not fighting: a parked pet that refuses to engage is a pet the \
         player has to un-park to use"
    );
}

#[test]
fn a_pet_whose_owner_stopped_fighting_drops_its_own_attack() {
    let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .attacking(IMP, WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pet_effects(),
        [PetEffect::StoodDown(IMP)],
        "the owner walked away from the fight, so the pet must too — one that keeps its melee row \
         holds the whole camp in combat and drags it after the player"
    );
    assert!(
        w.effects().is_empty() && w.victims().is_empty(),
        "a pet that stood down this firing must not also be chased into the fight it just left"
    );
}

#[test]
fn a_wounded_pet_armed_this_firing_closes_instead_of_routing() {
    let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
        .hurt(IMP, 10)
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .attacking(HUNTER, WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let leg = w.effects();
    assert_eq!(leg.len(), 1, "one leg per firing, and it is the chase's");
    assert!(
        leg[0].dest.x > 0.0 && w.rout_ends_ms(IMP) == 0,
        "an engagement armed by the pet phase is visible to the rout in the same cycle, and a pet \
         is not of a kind that runs: one that flees at low health abandons the fight its owner is \
         in the middle of"
    );
}

#[test]
fn a_pet_whose_owner_changed_instance_is_snapped_across_rather_than_walked() {
    let mut w = warlock_and_imp(p(-20.0, 0.0, 10.0)).in_instance(IMP, 7);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pet_effects(),
        [PetEffect::Restaged(IMP, p(0.0, 0.0, 10.0), MAP, INSTANCE)],
        "no movement leg crosses a map or an instance, so a pet left behind by its owner's \
         teleport has to be re-placed instead — walking it strands it in the copy of the world its \
         owner left"
    );
    assert!(
        w.effects().is_empty(),
        "the snap must relay no leg; the AOI create/destroy carries the pet across"
    );
    let imp = w.at(IMP);
    let grid = spatial::grid_cell(0.0, 0.0);
    assert_eq!(
        (imp.at, imp.grid, imp.cell),
        (p(0.0, 0.0, 10.0), grid, spatial::grid_cell_id(grid.0, grid.1)),
        "position, grid address and packed cell must move together, or the pet is delivered to the \
         wrong players from its new instance"
    );
}

/// The `SETTLED` clock in ms — what a combat-drop deadline is measured against.
const SETTLED_MS: u64 = SETTLED / 1000;

#[test]
fn a_creature_that_engaged_and_chased_this_cycle_is_not_healed_by_the_same_cycle() {
    // The fight is armed by the aggro phase of THIS cycle, so regeneration's gate can only see it
    // by running after that phase.
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .awake([WOLF])
        .hurt(WOLF, 10);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        !w.pulls().is_empty() && !w.effects().is_empty(),
        "the wolf must have pulled and closed on the player this firing, or the ordering under \
         test never happens"
    );
    assert_eq!(
        w.at(WOLF).health,
        10,
        "a creature healing while it fights cannot be killed at the rate the fight damages it; \
         regeneration has to run after chase so its gate sees the still-engaged chaser"
    );
}

#[test]
fn an_out_of_combat_unit_recovers_both_bars_and_is_not_moved_by_it() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .hurt(WOLF, 10)
        .power(WOLF, 20, 100);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let wolf = w.at(WOLF);
    assert_eq!(
        (wolf.health, wolf.power),
        (15, 25),
        "a unit out of combat recovers both bars every sense firing; one that recovers neither \
         never returns to full and has to be killed a second time at the health the last fight \
         left it"
    );
    assert!(
        w.effects().is_empty() && wolf.at == p(0.0, 0.0, 10.0),
        "regeneration writes health and power only — writing a whole row back would revert a \
         position another phase decided this firing"
    );
}

#[test]
fn a_unit_in_combat_heals_only_through_a_combat_regen_aura() {
    let cases: [(&str, Twist, u32); 2] = [
        ("no aura, the vanilla default", |w| w, 10),
        ("carrying Troll Regeneration", |w| w.combat_regen(WOLF), 11),
    ];
    for (case, twist, health) in cases {
        // Flagged in combat with no melee row of its own: a pure caster, which is the half of the
        // in-combat verdict the engagement rows do not answer.
        let mut w = twist(
            Scenario::new(SETTLED)
                .creature(WOLF, p(0.0, 0.0, 10.0))
                .hurt(WOLF, 10)
                .in_combat_until(WOLF, SETTLED_MS + 1000),
        );
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.at(WOLF).health,
            health,
            "fighting is what stops regeneration, and an aura is the only thing that gives any of \
             it back — healing everyone in combat at the free rate makes every fight unwinnable \
             for the side that deals less damage ({case})"
        );
    }
}

#[test]
fn a_unit_past_its_combat_deadline_loses_the_flag_and_one_inside_it_keeps_it() {
    let mut w = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .in_combat_until(WOLF, SETTLED_MS - 1000)
        .creature(PACK_MATE, p(5.0, 0.0, 10.0))
        .in_combat_until(PACK_MATE, SETTLED_MS + 1000);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.unflagged(),
        [WOLF],
        "the flag lifts about six seconds after the last hostile action, not the instant the \
         swinging stops; lifting it early lets a player eat and drink mid-fight, and never lifting \
         it leaves them stuck in combat"
    );
    assert!(
        !w.in_combat(WOLF) && w.in_combat(PACK_MATE),
        "only the expired deadline may clear"
    );
}

#[test]
fn combat_entry_outlasts_combat_exit_in_the_same_cycle() {
    let mut w = wolf_fighting(p(5.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .in_combat_until(WOLF, SETTLED_MS - 1000);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.unflagged().is_empty() && w.in_combat(WOLF),
        "entry stamps a deadline in the future and exit only clears one already behind us, so a \
         creature in a live fight can never be dropped out of combat by the firing that just \
         re-armed it"
    );
}

#[test]
fn a_creature_the_leash_dropped_walks_home_and_leaves_combat_in_one_cycle() {
    // The leash pass in `tick_melee` already evaded this creature: its engagement is gone and its
    // combat deadline has run out. What is left is what the cycle owns.
    let mut evaded = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 20.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .hurt(WOLF, 10)
        .in_combat_until(WOLF, SETTLED_MS - 1000);
    let tick = evaded.tick(true, catch_all());
    run_cycle(&mut evaded, tick);

    let legs = evaded.effects();
    assert_eq!(legs.len(), 1, "one leg per firing, and it is the walk home");
    assert!(
        legs[0].dest.y < 20.0 && legs[0].run,
        "a creature the leash let go has to run back to its post, or it stands where the player \
         abandoned it; got {:?}",
        legs[0].dest
    );
    assert_eq!(
        (evaded.unflagged(), evaded.at(WOLF).health),
        (vec![WOLF], 10),
        "the flag lifts this firing, and the recovery it gates starts on the NEXT one — a unit \
         healed by the same firing that released it recovers a tick early for free"
    );

    // The same creature with the pursuit still live: the fight owns it, so nothing walks it home.
    let mut pursuing = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 20.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(0.0, 40.0, 10.0))
        .attacking(WOLF, HUNTER)
        .in_combat_until(WOLF, SETTLED_MS - 1000);
    let tick = pursuing.tick(true, catch_all());
    run_cycle(&mut pursuing, tick);

    let legs = pursuing.effects();
    assert_eq!(legs.len(), 1, "one leg per firing, and it is the chase");
    assert!(
        legs[0].dest.y > 20.0 && pursuing.in_combat(WOLF),
        "a live pursuit keeps the creature engaged and closing; walking it home mid-fight is how a \
         player loses a mob that is still swinging at them; got {:?}",
        legs[0].dest
    );
}

#[test]
fn regeneration_and_combat_exit_run_only_on_the_catch_all_sense_firing() {
    let cases: [(&str, bool, TickScope, u32, Vec<u64>); 3] = [
        (
            "the catch-all sense firing",
            true,
            catch_all(),
            15,
            vec![WOLF],
        ),
        ("a movement-only firing", false, catch_all(), 10, vec![]),
        (
            "a dedicated instance firing",
            true,
            TickScope::Only(7),
            10,
            vec![],
        ),
    ];
    for (case, sense, scope, health, unflagged) in cases {
        let mut w = Scenario::new(SETTLED)
            .creature(WOLF, p(5.0, 0.0, 10.0))
            .in_combat_until(WOLF, SETTLED_MS - 1000)
            .creature(PACK_MATE, p(0.0, 0.0, 10.0))
            .hurt(PACK_MATE, 10);
        let tick = w.tick(sense, scope);
        run_cycle(&mut w, tick);

        assert_eq!(
            (w.at(PACK_MATE).health, w.unflagged()),
            (health, unflagged),
            "the recovery amount is quantized to the sense cadence and every instance is covered \
             from the catch-all firing, so a second schedule row running either pass multiplies \
             the whole world's regen rate and sweeps the deadlines twice ({case})"
        );
    }
}

// ================================================================================================
//  COVERAGE, CADENCE AND COST — what one firing is allowed to touch, and how often
// ================================================================================================

/// The instance that has a schedule row of its own in the partition scenarios.
const DEDICATED: u64 = 7;
/// A third player, so each instance's fight has its own victim.
const WARDEN: u64 = RANGER + 1;

/// One fight per instance — the open world, the dedicated-row instance, and a third instance with
/// no row of its own. Every creature stands 15 yards from its victim, so chase moves all three.
fn one_fight_per_instance() -> Scenario {
    Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(PACK_MATE, p(0.0, 0.0, 10.0))
        .in_instance(PACK_MATE, DEDICATED)
        .creature(FAR_MATE, p(0.0, 0.0, 10.0))
        .in_instance(FAR_MATE, 9)
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .player(RANGER, p(15.0, 0.0, 10.0))
        .player(WARDEN, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .attacking(PACK_MATE, RANGER)
        .attacking(FAR_MATE, WARDEN)
}

#[test]
fn the_catch_all_firing_covers_every_instance_exactly_once() {
    let mut w = one_fight_per_instance();
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().iter().map(|e| e.guid).collect::<Vec<_>>(),
        [WOLF, PACK_MATE, FAR_MATE],
        "with no dedicated row armed, the seeded row is every creature's only ticker: one it \
         misses stands frozen mid-fight, and one it steps twice closes at double speed"
    );
}

#[test]
fn a_dedicated_row_and_the_catch_all_stay_a_strict_partition() {
    let mut w = one_fight_per_instance();
    // The catch-all skips the instance holding a row of its own; that row then fires for it.
    let tick = w.tick(
        false,
        TickScope::from_rows(
            crate::creatures::GLOBAL_TICK_INSTANCE,
            [crate::creatures::GLOBAL_TICK_INSTANCE, DEDICATED],
        ),
    );
    run_cycle(&mut w, tick);
    let after_catch_all = w.effects().iter().map(|e| e.guid).collect::<Vec<_>>();

    let tick = w.tick(false, TickScope::Only(DEDICATED));
    run_cycle(&mut w, tick);

    assert_eq!(
        after_catch_all,
        [WOLF, FAR_MATE],
        "the catch-all must hand the dedicated instance over whole; ticking it here is the work \
         MULTIPLICATION a second schedule row exists to avoid"
    );
    assert_eq!(
        w.effects().iter().map(|e| e.guid).collect::<Vec<_>>(),
        [WOLF, FAR_MATE, PACK_MATE],
        "coverage is a partition, so the two firings together move every creature exactly once — \
         no creature chases, casts or routs twice in the same world second"
    );
    let mut flagged = w.flagged();
    flagged.sort_unstable();
    let mut once = flagged.clone();
    once.dedup();
    assert_eq!(
        flagged, once,
        "combat entry follows the same partition: a unit flagged by both rows has its combat \
         deadline re-stamped twice a firing and never drops out of combat"
    );
}

#[test]
fn movement_runs_every_firing_while_sensing_holds_the_four_second_cadence() {
    // A 100ms dedicated row and the seeded 500ms world row, over the SAME closed four seconds
    // (t = 0 through 4s), so both are compared at the same instant.
    let mut settled_at = Vec::new();
    for (interval_micros, firings) in [(100_000i64, 41u64), (crate::creatures::MOVE_TICK_MICROS, 9)]
    {
        // A minute-long leg, so there is movement work left on every firing of the window.
        let mut w = Scenario::new(0).creature(WOLF, p(0.0, 0.0, 10.0)).flying(
            WOLF,
            p(0.0, 0.0, 10.0),
            p(60.0, 0.0, 10.0),
            0,
            60_000,
        );
        let mut moved = 0u64;
        for k in 0..firings {
            let tick = w.firing(k * interval_micros as u64, interval_micros, catch_all());
            let outcome = run_cycle(&mut w, tick);
            moved += outcome.rows_visited[0].1;
        }
        assert_eq!(
            (moved, w.maintenance_runs.get()),
            (firings, 2),
            "a tighter row must smooth movement latency WITHOUT multiplying the sensing scans \
             or the effects quantized to them — regen amount, wander chance and respawn timers \
             all assume the ~4s sensing cadence, whatever the row's own interval \
             ({interval_micros}us row)"
        );
        settled_at.push(w.at(WOLF).at);
    }
    assert_eq!(
        settled_at[0], settled_at[1],
        "four seconds of world time must move a creature the same distance whatever its row's \
         cadence, or arming a dedicated row visibly speeds every creature in that instance up"
    );
}

#[test]
fn discovery_stays_on_the_narrow_candidate_universes() {
    // A crowded world: twelve creatures, of which two are awake near the player and exactly one is
    // fighting. The counts below are what an operator greps for; a phase that regressed to a
    // full-world scan reports twelve here and keeps every behavior test green.
    let mut w = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(PACK_MATE, p(60.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .awake([WOLF, PACK_MATE]);
    for i in 0..10 {
        w = w.creature(FAR_MATE + i, p(500.0 + i as f32, 0.0, 10.0));
    }
    w = w.hurt(FAR_MATE, 10);
    let population = w.creatures.borrow().len();

    let tick = w.tick(true, catch_all());
    let outcome = run_cycle(&mut w, tick);

    assert_eq!(
        population, 12,
        "this scenario only means anything while the world is far bigger than any phase's \
         candidate set"
    );
    assert_eq!(
        (outcome.awake, outcome.rows_visited),
        (
            2,
            vec![
                ("advance", 0),
                ("patrol", 0),
                ("aggro", 1),
                ("assist", 0),
                ("pet", 0),
                ("cast", 1),
                ("threat_retarget", 1),
                ("chase", 1),
                ("combat_enter", 1),
                ("idle", 2),
                ("regen*", 1),
                ("combat_drop*", 0),
                ("rout", 1),
                ("fear", 0),
            ]
        ),
        "the engaged phases must cost one row per FIGHT and the idle phases one per AWAKE \
         creature, never one per creature in the world: this world holds {population}, and a \
         count that reaches it is a candidate set that went back to scanning everything"
    );
}

// ================================================================================================
//  THE PRODUCTION ADAPTER'S SHAPE — the seam's own blind spot
// ================================================================================================

/// `Scenario` above stands in for EVERY line of `ctx.rs`, so a method there that quietly stops
/// doing what it says leaves this whole file green while a live shard's creatures freeze. Nothing
/// else can reach that layer: no headless test in this crate can execute a `ReducerContext`, and
/// cargo-mutants cannot either. So the instrument is exact text — equality, never `contains`, which
/// a leftover copy in a dead branch defeats.
///
/// What a silent edit here costs, method by method: `commit_position` no-op'd and every creature
/// stands still while the client animates on; `drop_leg` no-op'd and one leg replays forever;
/// `engage` no-op'd and nothing ever aggroes; `restore` no-op'd and health never comes back;
/// `awake_creatures` returning an empty sweep and the world goes dormant with every test passing.
///
/// Several methods are deliberately more than one expression — `place`, `engage`, `retarget`,
/// `combat_healed_to`, `restore`, `face` and `take_victim` — so the pin is the exact current body
/// rather than a "one expression" rule. `retarget` most of all: it updates the engagement row IN
/// PLACE, which is what carries the swing clock across a threat switch, and only this assertion
/// says so. Re-bless a deliberate change here with the same care.
#[test]
fn the_production_adapter_is_the_pass_through_the_harness_assumes() {
    let src = include_str!("ctx.rs");
    for (signature, want) in [
        (
            "pub(crate) fn run(ctx: &ReducerContext, tick: TickContext) -> CycleOutcome {",
            "{ run_cycle(&mut CtxWorld { ctx }, tick) }",
        ),
        ("struct CtxWorld<'a> {", "{ ctx: &'a ReducerContext, }"),
        (
            "impl CtxWorld<'_> {",
            concat!(
                "{ fn place( &self, guid: u64, at: Point, moved_ms: Option<u32>, orientation: ",
                "Option<f32>, ) -> Option<WorldEntity> { let entities = ",
                "self.ctx.db.game_world_entity(); let mut e = entities.guid().find(guid)?; let ",
                "(gx, gy) = spatial::grid_cell(at.x, at.y); e.x = at.x; e.y = at.y; e.z = at.z; ",
                "e.grid_x = gx; e.grid_y = gy; e.cell = spatial::grid_cell_id(gx, gy); if let ",
                "Some(ms) = moved_ms { e.last_move_ms = ms; } if let Some(rad) = orientation { ",
                "e.orientation = rad; } Some(entities.guid().update(e)) } }",
            ),
        ),
        (
            "fn as_leg(s: CreatureSpline, mover_gone: bool) -> LegInFlight {",
            concat!(
                "{ LegInFlight { guid: s.guid, start: Point { x: s.sx, y: s.sy, z: s.sz, }, ",
                "dest: Point { x: s.dx, y: s.dy, z: s.dz, }, started_micros: s.start_micros, ",
                "dur_ms: s.dur_ms, map_id: s.map_id, instance_id: s.instance_id, mover_gone, } ",
                "}",
            ),
        ),
        (
            "fn translation_flags(unit: &WorldEntity, leg: Option<CreatureSpline>) -> u32 {",
            concat!(
                "{ let carried = leg.is_some_and(|l| l.dur_ms > 0); unit.movement_flags | if ",
                "carried { MOVE_FLAG_FORWARD } else { 0 } }",
            ),
        ),
        (
            "fn caster_hold_range_yd(ctx: &ReducerContext, entry: u32) -> f32 {",
            concat!(
                "{ let spells = ctx.db.game_spell(); let mut max_r = 0u32; for r in ",
                "ctx.db.game_creature_spell().by_entry().filter(&entry) { if matches!( ",
                "r.condition, cast_condition::ALWAYS | cast_condition::TARGET_MISSING_AURA ) { ",
                "if let Some(h) = spells.spell_id().find(r.spell_id) { max_r = ",
                "max_r.max(h.range_yd); } } } if let Some(c) = ",
                "ctx.db.game_creature_cast().creature_entry().find(entry) { if let Some(h) = ",
                "spells.spell_id().find(c.spell_id) { max_r = max_r.max(h.range_yd); } } max_r ",
                "as f32 }",
            ),
        ),
        (
            "impl MotionSink for CtxWorld<'_> {",
            concat!(
                "{ fn legs_in_flight(&self) -> Vec<LegInFlight> { let entities = ",
                "self.ctx.db.game_world_entity(); self.ctx .db .game_creature_spline() .iter() ",
                ".map(|s| { let gone = entities.guid().find(s.guid).is_none(); as_leg(s, gone) ",
                "}) .collect() } fn movement_suppressed(&self, guid: u64) -> bool { ",
                "crate::spell::is_self_movement_suppressed(self.ctx, guid) } fn ",
                "commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) { ",
                "self.place(guid, at, Some(moved_ms), None); } fn halt(&mut self, leg: ",
                "&LegInFlight, at: Point, spline_id: u32) { if let Some(e) = ",
                "self.place(leg.guid, at, None, None) { tick::emit_move_spline( self.ctx, ",
                "leg.guid, (at.x, at.y, at.z), (at.x, at.y, at.z), 0, false, spline_id, ",
                "leg.map_id, leg.instance_id, (e.grid_x, e.grid_y), ); } } fn ",
                "drop_leg(&mut self, guid: u64) { ",
                "self.ctx.db.game_creature_spline().guid().delete(guid); } }",
            ),
        ),
        (
            "impl IdleSink for CtxWorld<'_> {",
            concat!(
                "{ fn idle_creatures(&self, active: &HashSet<u64>) -> Vec<IdleCreature> { let ",
                "entities = self.ctx.db.game_world_entity(); let waypoints = ",
                "self.ctx.db.game_creature_waypoint(); active .iter() .filter_map(|guid| ",
                "entities.guid().find(guid)) .filter(|c| !c.is_player() && !c.dead) .map(|c| ",
                "IdleCreature { guid: c.guid, at: Point { x: c.x, y: c.y, z: c.z, }, ",
                "leg_ends_ms: c.leg_ends_ms, wp_target: c.wp_target, patrols: ",
                "waypoints.by_creature().filter(&c.guid).next().is_some(), }) .collect() } fn ",
                "route_of(&self, guid: u64) -> Vec<Waypoint> { self.ctx .db ",
                ".game_creature_waypoint() .by_creature() .filter(&guid) .map(|w| Waypoint { ",
                "id: w.id, at: Point { x: w.x, y: w.y, z: w.z, }, }) .collect() } fn ",
                "home_of(&self, guid: u64) -> Option<Home> { self.ctx .db ",
                ".game_creature_spawn() .guid() .find(guid) .map(|s| Home { at: Point { x: s.x, ",
                "y: s.y, z: s.z, }, wanders: s.movement_type == ",
                "crate::creatures::MOVEMENT_RANDOM, }) } fn engaged(&self, guid: u64) -> bool { ",
                "crate::combat::is_engaged(self.ctx, guid) } fn speed_of(&self, guid: u64, ",
                "gait: Gait) -> f32 { crate::combat::effective_move_speed( self.ctx, guid, ",
                "match gait { Gait::Walk => constants::speeds::WALK, Gait::Run => ",
                "constants::speeds::RUN, }, ) } fn navigate(&self, guid: u64, to: (f32, f32), ",
                "max_step: f32) -> (f32, f32) { self.ctx .db .game_world_entity() .guid() ",
                ".find(guid) .map_or(to, |c| { crate::nav::nav_step(self.ctx, c.map_id, (c.x, ",
                "c.y), to, max_step, 0.0, c.z) }) } fn roll(&self) -> u32 { self.ctx.random() } ",
                "fn aim_at_waypoint(&mut self, guid: u64, waypoint_id: u64) { let entities = ",
                "self.ctx.db.game_world_entity(); if let Some(mut e) = ",
                "entities.guid().find(guid) { e.wp_target = waypoint_id; ",
                "entities.guid().update(e); } } fn commit_leg(&mut self, guid: u64, leg: Leg, ",
                "now_ms: u32) { if let Some(e) = ",
                "self.ctx.db.game_world_entity().guid().find(guid) { tick::emit_creature_leg( ",
                "self.ctx, e, leg.to, leg.z_fallback, leg.dur_ms, leg.gait == Gait::Run, ",
                "now_ms, leg.hold_until_landed, ); } } }",
            ),
        ),
        (
            "impl EngageSink for CtxWorld<'_> {",
            concat!(
                "{ fn players(&self) -> Vec<AggroTarget> { self.ctx .db .game_world_entity() ",
                ".iter() .filter(|e| e.is_player()) .map(|e| AggroTarget { guid: e.guid, at: ",
                "Point { x: e.x, y: e.y, z: e.z, }, level: e.level, faction_template: ",
                "e.faction_template, map_id: e.map_id, instance_id: e.instance_id, dead: ",
                "e.dead, godmode: e.godmode, stealthed: crate::spell::is_stealthed(self.ctx, ",
                "e.guid), }) .collect() } fn sensing_creatures(&self, active: &HashSet<u64>) -> ",
                "Vec<Sensor> { let entities = self.ctx.db.game_world_entity(); let melee = ",
                "self.ctx.db.game_melee_attack(); let templates = ",
                "self.ctx.db.game_creature_template(); active .iter() .filter_map(|guid| ",
                "entities.guid().find(guid)) .filter(|c| { !c.is_player() && !c.dead && ",
                "c.owner_guid == 0 && melee.attacker_guid().find(c.guid).is_none() }) .map(|c| ",
                "Sensor { guid: c.guid, at: Point { x: c.x, y: c.y, z: c.z, }, level: c.level, ",
                "faction_template: c.faction_template, map_id: c.map_id, instance_id: ",
                "c.instance_id, aggro_range: templates.entry().find(c.entry).map(|t| ",
                "t.aggro_range), detect_range_mod: crate::spell::detect_range_mod(self.ctx, ",
                "c.guid), would_rout: tick::rout_eligible(self.ctx, &c), cannot_act: ",
                "crate::spell::is_action_blocked(self.ctx, c.guid), }) .collect() } fn ",
                "hostile(&self, faction_template: u32, other: u32) -> bool { ",
                "crate::faction::is_hostile(self.ctx, faction_template, other) } fn ",
                "line_of_sight(&self, looker: u64, at: Point) -> bool { self.ctx .db ",
                ".game_world_entity() .guid() .find(looker) .is_none_or(|c| { ",
                "crate::nav::has_los(self.ctx, c.map_id, (c.x, c.y, c.z), (at.x, at.y, at.z)) ",
                "}) } fn engage(&mut self, creature: u64, victim: u64, pull: Pull) { let melee ",
                "= self.ctx.db.game_melee_attack(); if ",
                "melee.attacker_guid().find(creature).is_none() { melee.insert(MeleeAttack { ",
                "attacker_guid: creature, target_guid: victim, last_swing_ms: 0, ",
                "ranged_spell_id: 0, last_offhand_swing_ms: 0, rout_ends_ms: 0, ",
                "pursuit_ends_ms: 0, leash_x: 0.0, leash_y: 0.0, }); } let entities = ",
                "self.ctx.db.game_world_entity(); if let Some(mut c) = ",
                "entities.guid().find(creature) { if c.target_guid != victim { c.target_guid = ",
                "victim; entities.guid().update(c); } } crate::hooks::fire_on_aggro( self.ctx, ",
                "&crate::hooks::AggroPayload { creature_guid: creature, target_guid: victim, ",
                "assist: pull == Pull::Assisted, }, ); } fn engagements(&self) -> ",
                "Vec<Engagement> { let entities = self.ctx.db.game_world_entity(); self.ctx .db ",
                ".game_melee_attack() .iter() .filter_map(|a| { let attacker = ",
                "entities.guid().find(a.attacker_guid); let instance_id = attacker .as_ref() ",
                ".map(|e| e.instance_id) .or_else(|| ",
                "entities.guid().find(a.target_guid).map(|e| e.instance_id))?; Some(Engagement ",
                "{ attacker: a.attacker_guid, victim: a.target_guid, instance_id, ",
                "player_never_swung: a.last_swing_ms == 0 && a.last_offhand_swing_ms == 0 && ",
                "attacker.is_some_and(|e| e.is_player()), }) }) .collect() } fn ",
                "enter_combat(&mut self, guid: u64) { crate::combat::enter_combat(self.ctx, ",
                "guid); } fn flagged_in_combat(&self, candidates: &[u64]) -> Vec<Combatant> { ",
                "let entities = self.ctx.db.game_world_entity(); candidates .iter() ",
                ".filter_map(|guid| entities.guid().find(guid)) .filter(|e| e.unit_flags & ",
                "constants::unit_flags::IN_COMBAT != 0) .map(|e| Combatant { guid: e.guid, ",
                "combat_until_ms: e.combat_until_ms, }) .collect() } fn leave_combat(&mut self, ",
                "guid: u64) { let entities = self.ctx.db.game_world_entity(); if let Some(mut ",
                "e) = entities.guid().find(guid) { e.unit_flags &= ",
                "!constants::unit_flags::IN_COMBAT; entities.guid().update(e); } } }",
            ),
        ),
        (
            "impl RegenSink for CtxWorld<'_> {",
            concat!(
                "{ fn recovering(&self, flagged: &[u64]) -> Vec<Recovering> { let mut in_combat ",
                "= crate::combat::melee_combatant_guids(self.ctx); ",
                "in_combat.extend_from_slice(flagged); self.ctx .db .game_world_entity() ",
                ".iter() .filter(|e| !e.dead && (e.health < e.max_health || e.max_power > 0)) ",
                ".map(|e| Recovering { guid: e.guid, health: e.health, max_health: ",
                "e.max_health, power: e.power, max_power: e.max_power, in_combat: ",
                "in_combat.contains(&e.guid), }) .collect() } fn healed_to(&self, u: ",
                "&Recovering) -> u32 { self.ctx .db .game_world_entity() .guid() .find(u.guid) ",
                ".map_or(u.health, |e| crate::combat::regen_entity_health(&e)) } fn ",
                "combat_healed_to(&self, u: &Recovering) -> Option<u32> { let pct = ",
                "u32::try_from(crate::spell::combat_health_regen_pct(self.ctx, u.guid)) .ok() ",
                ".filter(|pct| *pct > 0)?; let e = ",
                "self.ctx.db.game_world_entity().guid().find(u.guid)?; ",
                "Some(crate::combat::regen_health_in_combat( e.health, e.max_health, e.spirit, ",
                "e.level, pct, )) } fn powered_to(&self, u: &Recovering) -> u32 { let now_ms = ",
                "(self.ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64; self.ctx .db ",
                ".game_world_entity() .guid() .find(u.guid) .map_or(u.power, |e| { ",
                "crate::combat::regen_entity_power(&e, u.in_combat, now_ms) }) } fn ",
                "restore(&mut self, guid: u64, health: Option<u32>, power: Option<u32>) { let ",
                "entities = self.ctx.db.game_world_entity(); let Some(mut live) = ",
                "entities.guid().find(guid) else { return; }; let (next_health, next_power) = ",
                "(health.unwrap_or(live.health), power.unwrap_or(live.power)); if (next_health, ",
                "next_power) == (live.health, live.power) { return; } live.health = ",
                "next_health; live.power = next_power; entities.guid().update(live); } }",
            ),
        ),
        (
            "fn entry_of(&self, guid: u64) -> Option<u32> {",
            "{ self.ctx .db .game_world_entity() .guid() .find(guid) .map(|c| c.entry) }",
        ),
        (
            "impl CastSink for CtxWorld<'_> {",
            concat!(
                "{ fn casters(&self, scope: &TickScope) -> Vec<Caster> { let entities = ",
                "self.ctx.db.game_world_entity(); let pending = ",
                "self.ctx.db.game_pending_cast(); self.ctx .db .game_melee_attack() .iter() ",
                ".filter_map(|row| { let c = tick::movable_creature(self.ctx, ",
                "row.attacker_guid, scope)?; let guid = c.guid; let casting = ",
                "pending.by_caster().filter(&guid).next().is_some(); Some(Caster { guid, ",
                "victim: row.target_guid, victim_at: ",
                "entities.guid().find(row.target_guid).map(|t| Point { x: t.x, y: t.y, z: t.z, ",
                "}), level: c.level as u8, health: c.health, max_health: c.max_health, ",
                "cannot_act: crate::spell::is_action_blocked(self.ctx, guid), casting, }) }) ",
                ".collect() } fn rotation_of(&self, guid: u64) -> Vec<SpellOption> { ",
                "self.entry_of(guid).map_or(Vec::new(), |entry| { self.ctx .db ",
                ".game_creature_spell() .by_entry() .filter(&entry) .map(|r| SpellOption { ",
                "spell_id: r.spell_id, when: CastWhen::of(r.condition, r.condition_value), ",
                "priority: r.priority, authored: r.id, }) .collect() }) } fn lone_spell(&self, ",
                "guid: u64) -> Option<u32> { self.ctx .db .game_creature_cast() ",
                ".creature_entry() .find(self.entry_of(guid)?) .map(|c| c.spell_id) } fn ",
                "carries(&self, guid: u64, spell_id: u32) -> bool { ",
                "crate::spell::has_aura(self.ctx, guid, spell_id) } fn begin_cast(&mut self, ",
                "caster: &Caster, spell_id: u32, target: u64) -> bool { ",
                "crate::spell::begin_cast( self.ctx, caster.guid, spell_id, caster.level, ",
                "target, false, None, ) .is_ok() } }",
            ),
        ),
        (
            "impl ThreatSink for CtxWorld<'_> {",
            concat!(
                "{ fn fighters(&self, scope: &TickScope) -> Vec<Fighter> { self.ctx .db ",
                ".game_melee_attack() .iter() .filter_map(|a| { ",
                "tick::movable_creature(self.ctx, a.attacker_guid, scope).map(|c| Fighter { ",
                "guid: c.guid, victim: a.target_guid, }) }) .collect() } fn taunted_onto(&self, ",
                "guid: u64) -> Option<u64> { crate::threat::forced_target( self.ctx, guid, ",
                "(self.ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64, ) } fn ",
                "top_threat(&self, guid: u64) -> Option<u64> { ",
                "crate::threat::top_threat_target(self.ctx, guid) } fn threat_on(&self, ",
                "creature: u64, source: u64) -> i64 { crate::threat::threat_of(self.ctx, ",
                "creature, source) } fn retarget(&mut self, creature: u64, victim: u64) { let ",
                "melee = self.ctx.db.game_melee_attack(); if let Some(mut row) = ",
                "melee.attacker_guid().find(creature) { row.target_guid = victim; ",
                "melee.attacker_guid().update(row); } let entities = ",
                "self.ctx.db.game_world_entity(); if let Some(mut c) = ",
                "entities.guid().find(creature) { if c.target_guid != victim { c.target_guid = ",
                "victim; entities.guid().update(c); } } } }",
            ),
        ),
        (
            "impl PursuitSink for CtxWorld<'_> {",
            concat!(
                "{ fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit> { let entities = ",
                "self.ctx.db.game_world_entity(); let splines = ",
                "self.ctx.db.game_creature_spline(); self.ctx .db .game_melee_attack() .iter() ",
                ".filter_map(|row| { let c = tick::movable_creature(self.ctx, ",
                "row.attacker_guid, scope)?; let victim = ",
                "entities.guid().find(row.target_guid)?; Some(Pursuit { guid: c.guid, at: Point ",
                "{ x: c.x, y: c.y, z: c.z, }, orientation: c.orientation, victim_at: Point { x: ",
                "victim.x, y: victim.y, z: victim.z, }, victim_movement_flags: ",
                "translation_flags( &victim, splines.guid().find(row.target_guid), ), routing: ",
                "tick::creature_is_routing(self.ctx, &c), leg: ",
                "splines.guid().find(c.guid).map(|s| as_leg(s, false)), }) }) .collect() } fn ",
                "caster_hold_range(&self, guid: u64) -> f32 { self.ctx .db .game_world_entity() ",
                ".guid() .find(guid) .map_or(0.0, |c| caster_hold_range_yd(self.ctx, c.entry)) ",
                "} fn face(&mut self, guid: u64, at: Point, orientation: f32, spline_id: u32) { ",
                "let Some(e) = self.place(guid, at, None, Some(orientation)) else { return; }; ",
                "tick::emit_facing_spline( self.ctx, guid, (at.x, at.y, at.z), orientation, ",
                "spline_id, e.map_id, e.instance_id, (e.grid_x, e.grid_y), ); } }",
            ),
        ),
        (
            "impl RoutSink for CtxWorld<'_> {",
            concat!(
                "{ fn routers(&self, scope: &TickScope) -> Vec<Router> { let entities = ",
                "self.ctx.db.game_world_entity(); let splines = ",
                "self.ctx.db.game_creature_spline(); self.ctx .db .game_melee_attack() .iter() ",
                ".filter_map(|row| { let c = tick::movable_creature(self.ctx, ",
                "row.attacker_guid, scope)?; Some(Router { guid: c.guid, at: Point { x: c.x, y: ",
                "c.y, z: c.z, }, victim: row.target_guid, victim_at: ",
                "entities.guid().find(row.target_guid).map(|t| Point { x: t.x, y: t.y, z: t.z, ",
                "}), health: c.health, max_health: c.max_health, eligible: ",
                "tick::rout_eligible(self.ctx, &c), rout_ends_ms: row.rout_ends_ms, routing: ",
                "tick::creature_is_routing(self.ctx, &c), committed: ",
                "splines.guid().find(c.guid).is_some(), }) }) .collect() } fn start_rout(&mut ",
                "self, guid: u64, ends_ms: u32) { let melee = self.ctx.db.game_melee_attack(); ",
                "if let Some(mut row) = melee.attacker_guid().find(guid) { row.rout_ends_ms = ",
                "ends_ms; melee.attacker_guid().update(row); } } }",
            ),
        ),
        (
            "impl FearSink for CtxWorld<'_> {",
            concat!(
                "{ fn panicked(&self, scope: &TickScope) -> Vec<Panicked> { let entities = ",
                "self.ctx.db.game_world_entity(); let melee = self.ctx.db.game_melee_attack(); ",
                "self.ctx .db .game_aura() .iter() .filter(|a| a.eff_kind == ",
                "crate::spell::A_CONTROL && a.eff_p0 == crate::spell::M_FEAR) .filter_map(|a| { ",
                "let c = tick::movable_creature(self.ctx, a.target_guid, scope)?; let source = ",
                "crate::spell::fear_source(self.ctx, c.guid)?; let at = ",
                "entities.guid().find(source).or_else(|| { melee .attacker_guid() .find(c.guid) ",
                ".and_then(|r| entities.guid().find(r.target_guid)) }); Some(Panicked { guid: ",
                "c.guid, at: Point { x: c.x, y: c.y, z: c.z, }, source_at: at.map(|s| Point { ",
                "x: s.x, y: s.y, z: s.z, }), frozen: ",
                "crate::spell::is_movement_blocked(self.ctx, c.guid), }) }) .collect() } }",
            ),
        ),
        (
            "impl PetSink for CtxWorld<'_> {",
            concat!(
                "{ fn pets(&self, scope: &TickScope, candidates: &[u64]) -> Vec<Pet> { let ",
                "entities = self.ctx.db.game_world_entity(); let melee = ",
                "self.ctx.db.game_melee_attack(); candidates .iter() .filter_map(|guid| ",
                "entities.guid().find(guid)) .filter(|p| scope.covers(p.instance_id)) .map(|p| ",
                "{ let (command, react, command_target) = pet::pet_command_state(self.ctx, ",
                "p.owner_guid); Pet { guid: p.guid, at: Point { x: p.x, y: p.y, z: p.z, }, ",
                "map_id: p.map_id, instance_id: p.instance_id, owner: entities .guid() ",
                ".find(p.owner_guid) .filter(|o| !o.dead) .map(|o| PetOwner { guid: o.guid, at: ",
                "Point { x: o.x, y: o.y, z: o.z, }, map_id: o.map_id, instance_id: ",
                "o.instance_id, combat_target: pet::owner_combat_target(self.ctx, &o), }), ",
                "dead: p.dead, suppressed: crate::spell::is_self_movement_suppressed(self.ctx, ",
                "p.guid), command: PetCommand::of(command, command_target), react: ",
                "PetReact::of(react), fighting: melee.attacker_guid().find(p.guid).is_some(), } ",
                "}) .collect() } fn pet_may_attack(&self, pet: &Pet, target: u64) -> bool { ",
                "pet.owner .is_some_and(|o| crate::creatures::pet::may_attack(self.ctx, o.guid, ",
                "pet.guid, target)) } fn nearest_hostile_to(&self, pet: &Pet) -> Option<u64> { ",
                "pet.owner .and_then(|o| crate::creatures::pet::nearest_hostile_for(self.ctx, ",
                "o.guid, pet.guid)) } fn cancel_attack_order(&mut self, owner: u64) { ",
                "pet::cancel_attack_order(self.ctx, owner); } fn take_victim(&mut self, guid: ",
                "u64, victim: u64) { let melee = self.ctx.db.game_melee_attack(); match ",
                "melee.attacker_guid().find(guid) { Some(mut row) => { if row.target_guid != ",
                "victim { row.target_guid = victim; melee.attacker_guid().update(row); } } None ",
                "=> { melee.insert(MeleeAttack { attacker_guid: guid, target_guid: victim, ",
                "last_swing_ms: 0, ranged_spell_id: 0, last_offhand_swing_ms: 0, rout_ends_ms: ",
                "0, pursuit_ends_ms: 0, leash_x: 0.0, leash_y: 0.0, }); } } let entities = ",
                "self.ctx.db.game_world_entity(); if let Some(mut p) = ",
                "entities.guid().find(guid) { if p.target_guid != victim { p.target_guid = ",
                "victim; entities.guid().update(p); } } } fn stand_down(&mut self, guid: u64) { ",
                "crate::combat::break_own_attacks(self.ctx, guid); } fn dismiss(&mut self, ",
                "guid: u64) { pet::despawn_pet(self.ctx, guid); } fn restage(&mut self, guid: ",
                "u64, at: Point, map_id: u32, instance_id: u64, now_ms: u32) { let entities = ",
                "self.ctx.db.game_world_entity(); let Some(mut p) = entities.guid().find(guid) ",
                "else { return; }; let (gx, gy) = spatial::grid_cell(at.x, at.y); p.map_id = ",
                "map_id; p.instance_id = instance_id; p.x = at.x; p.y = at.y; p.z = at.z; ",
                "p.grid_x = gx; p.grid_y = gy; p.cell = spatial::grid_cell_id(gx, gy); ",
                "p.last_move_ms = now_ms; entities.guid().update(p); } }",
            ),
        ),
        (
            "impl CreatureWorld for CtxWorld<'_> {",
            concat!(
                "{ fn awake_creatures(&self, scope: &TickScope) -> TickSweep { ",
                "tick::active_cell_creatures(self.ctx, scope) } fn ",
                "run_due_world_maintenance(&mut self) -> Vec<(&'static str, u64)> { vec![ ",
                "(\"decay*\", tick::pass_decay(self.ctx) as u64), (\"respawn*\", ",
                "tick::pass_respawn(self.ctx) as u64), ( \"go_respawn*\", ",
                "tick::pass_gameobject_respawn(self.ctx) as u64, ), ] } fn ",
                "run_package_passes(&mut self) { ",
                "crate::hooks::run_package_tick_passes(self.ctx); } }",
            ),
        ),
    ] {
        let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            crate::test_scan::shape_of(src, signature),
            want,
            "`{signature}` is no longer the exact pass-through this harness assumes it is. Every \
             scenario above runs the shared phase bodies with `Scenario` substituted for all of \
             this, so a no-op'd or short-circuited line here leaves the suite green while the \
             world stops moving."
        );
    }
}
