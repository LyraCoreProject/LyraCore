//! The ONE production adapter: a real `ReducerContext` wearing the whole cycle world.
//!
//! **This layer is the seam's own blind spot.** The harness substitutes an in-memory `Scenario` for
//! every line of it, so a no-op'd method here is invisible to every test in the crate. Keep every
//! method a pass-through: `harness::the_production_adapter_is_the_pass_through_the_harness_assumes`
//! pins this file's exact shape, and re-blessing an edit there is the only thing that covers it.

use std::collections::HashSet;

use lyracore_shared::{constants, spatial};
use spacetimedb::{ReducerContext, Table};

use super::{
    run_cycle, AggroTarget, CastSink, CastWhen, Caster, Combatant, CreatureWorld, CycleOutcome,
    EngageSink, Engagement, FearSink, Fighter, Gait, Home, IdleCreature, IdleSink, Leg,
    LegInFlight, MotionSink, Panicked, Pet, PetCommand, PetOwner, PetReact, PetSink, Point, Pull,
    Pursuit, PursuitSink, Recovering, RegenSink, RoutSink, Router, Sensor, SpellOption, ThreatSink,
    TickContext, Waypoint,
};
use crate::combat::MOVE_FLAG_FORWARD;
use crate::creatures::ai::TickScope;
use crate::creatures::cast_condition;
use crate::creatures::pet;
use crate::creatures::tick::{self, CreatureSpline, TickSweep};
use crate::spell::game_pending_cast;
use crate::{
    game_aura, game_creature_cast, game_creature_spawn, game_creature_spell, game_creature_spline,
    game_creature_template, game_creature_waypoint, game_melee_attack, game_spell,
    game_world_entity, MeleeAttack, WorldEntity,
};

/// `tick_creatures`' one call into the cycle. The adapter never leaves this module.
pub(crate) fn run(ctx: &ReducerContext, tick: TickContext) -> CycleOutcome {
    run_cycle(&mut CtxWorld { ctx }, tick)
}

struct CtxWorld<'a> {
    ctx: &'a ReducerContext,
}

impl CtxWorld<'_> {
    /// Move the creature's authoritative row to `at`, writing grid address and packed cell in the
    /// SAME statement (a stale `cell` puts the row in the wrong AOI cell), and hand back the row as
    /// written. `moved_ms` stamps the move clock; a halted creature passes `None` because it did not
    /// travel. `orientation` turns it in that same statement, so a stop-and-turn costs one write.
    fn place(
        &self,
        guid: u64,
        at: Point,
        moved_ms: Option<u32>,
        orientation: Option<f32>,
    ) -> Option<WorldEntity> {
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
        if let Some(rad) = orientation {
            e.orientation = rad;
        }
        Some(entities.guid().update(e))
    }
}

/// The spline row as the cycle reads a leg. `mover_gone` is the caller's to answer: the advance
/// phase has to reap orphans, while a chaser's leg belongs to a creature it just resolved.
fn as_leg(s: CreatureSpline, mover_gone: bool) -> LegInFlight {
    LegInFlight {
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
        mover_gone,
    }
}

/// A unit's live translation flags as the chase decision reads them. A client-driven unit stamps its
/// own on every heartbeat; a creature stamps none, so the leg the MODULE is moving it along is what
/// says it travels — the same thing vanilla's spline launch records on the unit itself. Spline
/// advance runs first in the cycle and reaps a landed leg, so a surviving leg with a real duration is
/// still in flight, while a halt leaves a zero-duration row, which is a stop.
fn translation_flags(unit: &WorldEntity, leg: Option<CreatureSpline>) -> u32 {
    let carried = leg.is_some_and(|l| l.dur_ms > 0);
    unit.movement_flags | if carried { MOVE_FLAG_FORWARD } else { 0 }
}

/// The longest ENEMY-cast range (yd) creature `entry` can bring to bear — the rotation nukes and
/// debuffs that target the melee victim, plus the legacy single-spell cast row, mapped through
/// `game_spell.range_yd`. 0 means it is no offensive caster (melee-only mobs, pure healers and
/// buff-bots), and chase then closes to melee.
fn caster_hold_range_yd(ctx: &ReducerContext, entry: u32) -> f32 {
    let spells = ctx.db.game_spell();
    let mut max_r = 0u32;
    for r in ctx.db.game_creature_spell().by_entry().filter(&entry) {
        if matches!(
            r.condition,
            cast_condition::ALWAYS | cast_condition::TARGET_MISSING_AURA
        ) {
            if let Some(h) = spells.spell_id().find(r.spell_id) {
                max_r = max_r.max(h.range_yd);
            }
        }
    }
    if let Some(c) = ctx.db.game_creature_cast().creature_entry().find(entry) {
        if let Some(h) = spells.spell_id().find(c.spell_id) {
            max_r = max_r.max(h.range_yd);
        }
    }
    max_r as f32
}

impl MotionSink for CtxWorld<'_> {
    fn legs_in_flight(&self) -> Vec<LegInFlight> {
        let entities = self.ctx.db.game_world_entity();
        self.ctx
            .db
            .game_creature_spline()
            .iter()
            .map(|s| {
                let gone = entities.guid().find(s.guid).is_none();
                as_leg(s, gone)
            })
            .collect()
    }
    fn movement_suppressed(&self, guid: u64) -> bool {
        crate::spell::is_self_movement_suppressed(self.ctx, guid)
    }
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) {
        self.place(guid, at, Some(moved_ms), None);
    }
    fn halt(&mut self, leg: &LegInFlight, at: Point, spline_id: u32) {
        if let Some(e) = self.place(leg.guid, at, None, None) {
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
                (e.grid_x, e.grid_y),
            );
        }
    }
    fn drop_leg(&mut self, guid: u64) {
        self.ctx.db.game_creature_spline().guid().delete(guid);
    }
}

impl IdleSink for CtxWorld<'_> {
    fn idle_creatures(&self, active: &HashSet<u64>) -> Vec<IdleCreature> {
        let entities = self.ctx.db.game_world_entity();
        let waypoints = self.ctx.db.game_creature_waypoint();
        active
            .iter()
            .filter_map(|guid| entities.guid().find(guid))
            .filter(|c| !c.is_player() && !c.dead)
            .map(|c| IdleCreature {
                guid: c.guid,
                at: Point {
                    x: c.x,
                    y: c.y,
                    z: c.z,
                },
                leg_ends_ms: c.leg_ends_ms,
                wp_target: c.wp_target,
                patrols: waypoints.by_creature().filter(&c.guid).next().is_some(),
            })
            .collect()
    }
    fn route_of(&self, guid: u64) -> Vec<Waypoint> {
        self.ctx
            .db
            .game_creature_waypoint()
            .by_creature()
            .filter(&guid)
            .map(|w| Waypoint {
                id: w.id,
                at: Point {
                    x: w.x,
                    y: w.y,
                    z: w.z,
                },
            })
            .collect()
    }
    fn home_of(&self, guid: u64) -> Option<Home> {
        self.ctx
            .db
            .game_creature_spawn()
            .guid()
            .find(guid)
            .map(|s| Home {
                at: Point {
                    x: s.x,
                    y: s.y,
                    z: s.z,
                },
                wanders: s.movement_type == crate::creatures::MOVEMENT_RANDOM,
            })
    }
    fn engaged(&self, guid: u64) -> bool {
        crate::combat::is_engaged(self.ctx, guid)
    }
    fn speed_of(&self, guid: u64, gait: Gait) -> f32 {
        crate::combat::effective_move_speed(
            self.ctx,
            guid,
            match gait {
                Gait::Walk => constants::speeds::WALK,
                Gait::Run => constants::speeds::RUN,
            },
        )
    }
    fn navigate(&self, guid: u64, to: (f32, f32), max_step: f32) -> (f32, f32) {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .map_or(to, |c| {
                crate::nav::nav_step(self.ctx, c.map_id, (c.x, c.y), to, max_step, 0.0, c.z)
            })
    }
    fn roll(&self) -> u32 {
        self.ctx.random()
    }
    fn aim_at_waypoint(&mut self, guid: u64, waypoint_id: u64) {
        let entities = self.ctx.db.game_world_entity();
        if let Some(mut e) = entities.guid().find(guid) {
            e.wp_target = waypoint_id;
            entities.guid().update(e);
        }
    }
    fn commit_leg(&mut self, guid: u64, leg: Leg, now_ms: u32) {
        if let Some(e) = self.ctx.db.game_world_entity().guid().find(guid) {
            tick::emit_creature_leg(
                self.ctx,
                e,
                leg.to,
                leg.z_fallback,
                leg.dur_ms,
                leg.gait == Gait::Run,
                now_ms,
                leg.hold_until_landed,
            );
        }
    }
}

impl EngageSink for CtxWorld<'_> {
    // KNOWN DEBT, inherited verbatim from the aggro pass: there is no players-only index, so the
    // snapshot reads every entity row once per sense firing. Budgeted in `tripwires.rs`.
    fn players(&self) -> Vec<AggroTarget> {
        self.ctx
            .db
            .game_world_entity()
            .iter()
            .filter(|e| e.is_player())
            .map(|e| AggroTarget {
                guid: e.guid,
                at: Point {
                    x: e.x,
                    y: e.y,
                    z: e.z,
                },
                level: e.level,
                faction_template: e.faction_template,
                map_id: e.map_id,
                instance_id: e.instance_id,
                dead: e.dead,
                godmode: e.godmode,
                stealthed: crate::spell::is_stealthed(self.ctx, e.guid),
            })
            .collect()
    }
    fn sensing_creatures(&self, active: &HashSet<u64>) -> Vec<Sensor> {
        let entities = self.ctx.db.game_world_entity();
        let melee = self.ctx.db.game_melee_attack();
        let templates = self.ctx.db.game_creature_template();
        active
            .iter()
            .filter_map(|guid| entities.guid().find(guid))
            .filter(|c| {
                !c.is_player()
                    && !c.dead
                    // A PET never pulls or assists on its owner's behalf — the pet phase arms it off
                    // the owner's combat instead.
                    && c.owner_guid == 0
                    && melee.attacker_guid().find(c.guid).is_none()
            })
            .map(|c| Sensor {
                guid: c.guid,
                at: Point {
                    x: c.x,
                    y: c.y,
                    z: c.z,
                },
                level: c.level,
                faction_template: c.faction_template,
                map_id: c.map_id,
                instance_id: c.instance_id,
                aggro_range: templates.entry().find(c.entry).map(|t| t.aggro_range),
                detect_range_mod: crate::spell::detect_range_mod(self.ctx, c.guid),
                would_rout: tick::rout_eligible(self.ctx, &c),
                cannot_act: crate::spell::is_action_blocked(self.ctx, c.guid),
            })
            .collect()
    }
    fn hostile(&self, faction_template: u32, other: u32) -> bool {
        crate::faction::is_hostile(self.ctx, faction_template, other)
    }
    fn line_of_sight(&self, looker: u64, at: Point) -> bool {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(looker)
            .is_none_or(|c| {
                crate::nav::has_los(self.ctx, c.map_id, (c.x, c.y, c.z), (at.x, at.y, at.z))
            })
    }
    fn engage(&mut self, creature: u64, victim: u64, pull: Pull) {
        let melee = self.ctx.db.game_melee_attack();
        if melee.attacker_guid().find(creature).is_none() {
            melee.insert(MeleeAttack {
                attacker_guid: creature,
                target_guid: victim,
                last_swing_ms: 0,   // swing on the next melee tick
                ranged_spell_id: 0, // proximity and pack aggro are melee
                last_offhand_swing_ms: 0,
                rout_ends_ms: 0,
                pursuit_ends_ms: 0,
                leash_x: 0.0,
                leash_y: 0.0,
            });
        }
        let entities = self.ctx.db.game_world_entity();
        if let Some(mut c) = entities.guid().find(creature) {
            if c.target_guid != victim {
                c.target_guid = victim;
                entities.guid().update(c);
            }
        }
        crate::hooks::fire_on_aggro(
            self.ctx,
            &crate::hooks::AggroPayload {
                creature_guid: creature,
                target_guid: victim,
                assist: pull == Pull::Assisted,
            },
        );
    }
    fn engagements(&self) -> Vec<Engagement> {
        let entities = self.ctx.db.game_world_entity();
        self.ctx
            .db
            .game_melee_attack()
            .iter()
            .filter_map(|a| {
                let attacker = entities.guid().find(a.attacker_guid);
                // Both sides gone: nothing to flag, and `enter_combat` would have no-opped anyway.
                let instance_id = attacker
                    .as_ref()
                    .map(|e| e.instance_id)
                    .or_else(|| entities.guid().find(a.target_guid).map(|e| e.instance_id))?;
                Some(Engagement {
                    attacker: a.attacker_guid,
                    victim: a.target_guid,
                    instance_id,
                    player_never_swung: a.last_swing_ms == 0
                        && a.last_offhand_swing_ms == 0
                        && attacker.is_some_and(|e| e.is_player()),
                })
            })
            .collect()
    }
    fn enter_combat(&mut self, guid: u64) {
        crate::combat::enter_combat(self.ctx, guid);
    }
    fn flagged_in_combat(&self, candidates: &[u64]) -> Vec<Combatant> {
        let entities = self.ctx.db.game_world_entity();
        candidates
            .iter()
            .filter_map(|guid| entities.guid().find(guid))
            .filter(|e| e.unit_flags & constants::unit_flags::IN_COMBAT != 0)
            .map(|e| Combatant {
                guid: e.guid,
                combat_until_ms: e.combat_until_ms,
            })
            .collect()
    }
    fn leave_combat(&mut self, guid: u64) {
        let entities = self.ctx.db.game_world_entity();
        if let Some(mut e) = entities.guid().find(guid) {
            e.unit_flags &= !constants::unit_flags::IN_COMBAT;
            entities.guid().update(e);
        }
    }
}

impl RegenSink for CtxWorld<'_> {
    // KNOWN DEBT, inherited verbatim with the pass: no index answers "hurt, or carrying a power
    // bar", so the candidates cost one entity read per sense firing. Budgeted in `tripwires.rs`.
    fn recovering(&self, flagged: &[u64]) -> Vec<Recovering> {
        // Both halves of "in combat": every side of a live engagement, plus the flag the sweep
        // harvested at the top of this cycle (a pure caster or a Bloodrage warrior carries the flag
        // with no melee row at all).
        let mut in_combat = crate::combat::melee_combatant_guids(self.ctx);
        in_combat.extend_from_slice(flagged);
        self.ctx
            .db
            .game_world_entity()
            .iter()
            .filter(|e| !e.dead && (e.health < e.max_health || e.max_power > 0))
            .map(|e| Recovering {
                guid: e.guid,
                health: e.health,
                max_health: e.max_health,
                power: e.power,
                max_power: e.max_power,
                in_combat: in_combat.contains(&e.guid),
            })
            .collect()
    }
    // ponytail: each rate re-finds the unit's row, where the old pass computed from the one snapshot
    // it had scanned. Ceiling: up to three PK point reads per recovering unit per sense firing. The
    // alternative is carrying spirit, level, power type and the five-second-rule clock in the
    // cycle's own vocabulary, which is the schema wearing a behavior hat.
    fn healed_to(&self, u: &Recovering) -> u32 {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(u.guid)
            .map_or(u.health, |e| crate::combat::regen_entity_health(&e))
    }
    fn combat_healed_to(&self, u: &Recovering) -> Option<u32> {
        let pct = u32::try_from(crate::spell::combat_health_regen_pct(self.ctx, u.guid))
            .ok()
            .filter(|pct| *pct > 0)?;
        let e = self.ctx.db.game_world_entity().guid().find(u.guid)?;
        Some(crate::combat::regen_health_in_combat(
            e.health,
            e.max_health,
            e.spirit,
            e.level,
            pct,
        ))
    }
    fn powered_to(&self, u: &Recovering) -> u32 {
        let now_ms = (self.ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(u.guid)
            .map_or(u.power, |e| {
                crate::combat::regen_entity_power(&e, u.in_combat, now_ms)
            })
    }
    // ponytail: one row write per unit, where the old pass wrote health and power in two separate
    // loops. Ceiling: a unit recovering both bars relays one update instead of two.
    fn restore(&mut self, guid: u64, health: Option<u32>, power: Option<u32>) {
        let entities = self.ctx.db.game_world_entity();
        // The LIVE row, never the snapshot the decision was made from: a movement write earlier in
        // this cycle must not be reverted by writing a stale position back.
        let Some(mut live) = entities.guid().find(guid) else {
            return;
        };
        let (next_health, next_power) =
            (health.unwrap_or(live.health), power.unwrap_or(live.power));
        if (next_health, next_power) == (live.health, live.power) {
            return;
        }
        live.health = next_health;
        live.power = next_power;
        entities.guid().update(live);
    }
}

impl CtxWorld<'_> {
    /// The template this creature was spawned from — the key both spell tables are authored against.
    fn entry_of(&self, guid: u64) -> Option<u32> {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .map(|c| c.entry)
    }
}

impl CastSink for CtxWorld<'_> {
    fn casters(&self, scope: &TickScope) -> Vec<Caster> {
        let entities = self.ctx.db.game_world_entity();
        let pending = self.ctx.db.game_pending_cast();
        // Same candidate discovery as chase: the engaged rows ARE the set, one per attacker.
        self.ctx
            .db
            .game_melee_attack()
            .iter()
            .filter_map(|row| {
                let c = tick::movable_creature(self.ctx, row.attacker_guid, scope)?;
                let guid = c.guid;
                let casting = pending.by_caster().filter(&guid).next().is_some();
                Some(Caster {
                    guid,
                    victim: row.target_guid,
                    victim_at: entities.guid().find(row.target_guid).map(|t| Point {
                        x: t.x,
                        y: t.y,
                        z: t.z,
                    }),
                    level: c.level as u8,
                    health: c.health,
                    max_health: c.max_health,
                    cannot_act: crate::spell::is_action_blocked(self.ctx, guid),
                    casting,
                })
            })
            .collect()
    }
    fn rotation_of(&self, guid: u64) -> Vec<SpellOption> {
        self.entry_of(guid).map_or(Vec::new(), |entry| {
            self.ctx
                .db
                .game_creature_spell()
                .by_entry()
                .filter(&entry)
                .map(|r| SpellOption {
                    spell_id: r.spell_id,
                    when: CastWhen::of(r.condition, r.condition_value),
                    priority: r.priority,
                    authored: r.id,
                })
                .collect()
        })
    }
    fn lone_spell(&self, guid: u64) -> Option<u32> {
        self.ctx
            .db
            .game_creature_cast()
            .creature_entry()
            .find(self.entry_of(guid)?)
            .map(|c| c.spell_id)
    }
    fn carries(&self, guid: u64, spell_id: u32) -> bool {
        crate::spell::has_aura(self.ctx, guid, spell_id)
    }
    fn begin_cast(&mut self, caster: &Caster, spell_id: u32, target: u64) -> bool {
        crate::spell::begin_cast(
            self.ctx,
            caster.guid,
            spell_id,
            caster.level,
            target,
            false,
            None,
        )
        .is_ok()
    }
}

impl ThreatSink for CtxWorld<'_> {
    fn fighters(&self, scope: &TickScope) -> Vec<Fighter> {
        self.ctx
            .db
            .game_melee_attack()
            .iter()
            .filter_map(|a| {
                tick::movable_creature(self.ctx, a.attacker_guid, scope).map(|c| Fighter {
                    guid: c.guid,
                    victim: a.target_guid,
                })
            })
            .collect()
    }
    fn taunted_onto(&self, guid: u64) -> Option<u64> {
        crate::threat::forced_target(
            self.ctx,
            guid,
            (self.ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64,
        )
    }
    fn top_threat(&self, guid: u64) -> Option<u64> {
        crate::threat::top_threat_target(self.ctx, guid)
    }
    fn threat_on(&self, creature: u64, source: u64) -> i64 {
        crate::threat::threat_of(self.ctx, creature, source)
    }
    fn retarget(&mut self, creature: u64, victim: u64) {
        // The engagement keeps its PK and its swing clock — only the victim moves, so the creature
        // swings at the new foe on its own cadence instead of restarting the swing timer.
        let melee = self.ctx.db.game_melee_attack();
        if let Some(mut row) = melee.attacker_guid().find(creature) {
            row.target_guid = victim;
            melee.attacker_guid().update(row);
        }
        let entities = self.ctx.db.game_world_entity();
        if let Some(mut c) = entities.guid().find(creature) {
            if c.target_guid != victim {
                c.target_guid = victim;
                entities.guid().update(c);
            }
        }
    }
}

impl PursuitSink for CtxWorld<'_> {
    fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit> {
        let entities = self.ctx.db.game_world_entity();
        let splines = self.ctx.db.game_creature_spline();
        // The engaged rows ARE the candidate set (one per attacker, few), so this stays O(fights)
        // rather than scanning the world. A player's own attack row is dropped by `movable_creature`.
        self.ctx
            .db
            .game_melee_attack()
            .iter()
            .filter_map(|row| {
                let c = tick::movable_creature(self.ctx, row.attacker_guid, scope)?;
                let victim = entities.guid().find(row.target_guid)?;
                Some(Pursuit {
                    guid: c.guid,
                    at: Point {
                        x: c.x,
                        y: c.y,
                        z: c.z,
                    },
                    orientation: c.orientation,
                    victim_at: Point {
                        x: victim.x,
                        y: victim.y,
                        z: victim.z,
                    },
                    victim_movement_flags: translation_flags(
                        &victim,
                        splines.guid().find(row.target_guid),
                    ),
                    routing: tick::creature_is_routing(self.ctx, &c),
                    leg: splines.guid().find(c.guid).map(|s| as_leg(s, false)),
                })
            })
            .collect()
    }
    fn caster_hold_range(&self, guid: u64) -> f32 {
        self.ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .map_or(0.0, |c| caster_hold_range_yd(self.ctx, c.entry))
    }
    fn face(&mut self, guid: u64, at: Point, orientation: f32, spline_id: u32) {
        // Position and facing land in ONE entity write, beside the ONE spline row the relay carries:
        // `place` re-finds the live row rather than carrying the snapshot the decision was made
        // from, so another write to it inside the same tick is not clobbered by a stale copy.
        let Some(e) = self.place(guid, at, None, Some(orientation)) else {
            return;
        };
        tick::emit_facing_spline(
            self.ctx,
            guid,
            (at.x, at.y, at.z),
            orientation,
            spline_id,
            e.map_id,
            e.instance_id,
            (e.grid_x, e.grid_y),
        );
    }
}

impl RoutSink for CtxWorld<'_> {
    // ponytail: `creature_is_routing` re-derives eligibility that `eligible` already carries, because
    // it is the ONE predicate chase, this phase and `combat::swing` share and forking it would let a
    // creature be diverted out of chasing with nothing left to move it. Ceiling: one extra template
    // read plus one aura scan per engaged creature per firing.
    fn routers(&self, scope: &TickScope) -> Vec<Router> {
        let entities = self.ctx.db.game_world_entity();
        let splines = self.ctx.db.game_creature_spline();
        // The engaged rows ARE the candidate set — the rout clock lives on them, so the state is in
        // hand with no extra lookup, and a creature whose row is gone can never have been routing.
        self.ctx
            .db
            .game_melee_attack()
            .iter()
            .filter_map(|row| {
                let c = tick::movable_creature(self.ctx, row.attacker_guid, scope)?;
                Some(Router {
                    guid: c.guid,
                    at: Point {
                        x: c.x,
                        y: c.y,
                        z: c.z,
                    },
                    victim: row.target_guid,
                    victim_at: entities.guid().find(row.target_guid).map(|t| Point {
                        x: t.x,
                        y: t.y,
                        z: t.z,
                    }),
                    health: c.health,
                    max_health: c.max_health,
                    eligible: tick::rout_eligible(self.ctx, &c),
                    rout_ends_ms: row.rout_ends_ms,
                    routing: tick::creature_is_routing(self.ctx, &c),
                    committed: splines.guid().find(c.guid).is_some(),
                })
            })
            .collect()
    }
    fn start_rout(&mut self, guid: u64, ends_ms: u32) {
        let melee = self.ctx.db.game_melee_attack();
        if let Some(mut row) = melee.attacker_guid().find(guid) {
            row.rout_ends_ms = ends_ms;
            melee.attacker_guid().update(row);
        }
    }
}

impl FearSink for CtxWorld<'_> {
    // KNOWN DEBT, inherited verbatim with the pass: `game_aura` carries no by-mechanic index, so the
    // fear rows are found by scanning the aura table, which is itself small — fear uptime is rare.
    // ponytail: one row per AURA, not per feared creature, so a doubly-feared creature is resolved
    // twice and the phase collapses it. Ceiling: a second `fear_source` scan for a rare case.
    fn panicked(&self, scope: &TickScope) -> Vec<Panicked> {
        let entities = self.ctx.db.game_world_entity();
        let melee = self.ctx.db.game_melee_attack();
        self.ctx
            .db
            .game_aura()
            .iter()
            .filter(|a| a.eff_kind == crate::spell::A_CONTROL && a.eff_p0 == crate::spell::M_FEAR)
            .filter_map(|a| {
                let c = tick::movable_creature(self.ctx, a.target_guid, scope)?;
                // Re-resolved rather than read off the row above, so which caster wins when a
                // creature carries two fears stays `fear_source`'s own find order.
                let source = crate::spell::fear_source(self.ctx, c.guid)?;
                let at = entities.guid().find(source).or_else(|| {
                    melee
                        .attacker_guid()
                        .find(c.guid)
                        .and_then(|r| entities.guid().find(r.target_guid))
                });
                Some(Panicked {
                    guid: c.guid,
                    at: Point {
                        x: c.x,
                        y: c.y,
                        z: c.z,
                    },
                    source_at: at.map(|s| Point {
                        x: s.x,
                        y: s.y,
                        z: s.z,
                    }),
                    frozen: crate::spell::is_movement_blocked(self.ctx, c.guid),
                })
            })
            .collect()
    }
}

impl PetSink for CtxWorld<'_> {
    fn pets(&self, scope: &TickScope, candidates: &[u64]) -> Vec<Pet> {
        let entities = self.ctx.db.game_world_entity();
        let melee = self.ctx.db.game_melee_attack();
        candidates
            .iter()
            .filter_map(|guid| entities.guid().find(guid))
            .filter(|p| scope.covers(p.instance_id))
            .map(|p| {
                let (command, react, command_target) =
                    pet::pet_command_state(self.ctx, p.owner_guid);
                Pet {
                    guid: p.guid,
                    at: Point {
                        x: p.x,
                        y: p.y,
                        z: p.z,
                    },
                    map_id: p.map_id,
                    instance_id: p.instance_id,
                    // A dead owner reads the same as a missing one: the pet despawns either way.
                    owner: entities
                        .guid()
                        .find(p.owner_guid)
                        .filter(|o| !o.dead)
                        .map(|o| PetOwner {
                            guid: o.guid,
                            at: Point {
                                x: o.x,
                                y: o.y,
                                z: o.z,
                            },
                            map_id: o.map_id,
                            instance_id: o.instance_id,
                            combat_target: pet::owner_combat_target(self.ctx, &o),
                        }),
                    dead: p.dead,
                    suppressed: crate::spell::is_self_movement_suppressed(self.ctx, p.guid),
                    command: PetCommand::of(command, command_target),
                    react: PetReact::of(react),
                    fighting: melee.attacker_guid().find(p.guid).is_some(),
                }
            })
            .collect()
    }
    fn pet_may_attack(&self, pet: &Pet, target: u64) -> bool {
        pet.owner
            .is_some_and(|o| crate::creatures::pet::may_attack(self.ctx, o.guid, pet.guid, target))
    }
    fn nearest_hostile_to(&self, pet: &Pet) -> Option<u64> {
        pet.owner
            .and_then(|o| crate::creatures::pet::nearest_hostile_for(self.ctx, o.guid, pet.guid))
    }
    fn cancel_attack_order(&mut self, owner: u64) {
        pet::cancel_attack_order(self.ctx, owner);
    }
    fn take_victim(&mut self, guid: u64, victim: u64) {
        let melee = self.ctx.db.game_melee_attack();
        match melee.attacker_guid().find(guid) {
            // Re-point at the owner's current foe WITHOUT resetting the swing cadence.
            Some(mut row) => {
                if row.target_guid != victim {
                    row.target_guid = victim;
                    melee.attacker_guid().update(row);
                }
            }
            None => {
                melee.insert(MeleeAttack {
                    attacker_guid: guid,
                    target_guid: victim,
                    last_swing_ms: 0,   // swing on the next melee tick
                    ranged_spell_id: 0, // an Imp's Firebolt is cast by the cast phase, not this field
                    last_offhand_swing_ms: 0,
                    rout_ends_ms: 0,
                    pursuit_ends_ms: 0,
                    leash_x: 0.0,
                    leash_y: 0.0,
                });
            }
        }
        let entities = self.ctx.db.game_world_entity();
        if let Some(mut p) = entities.guid().find(guid) {
            if p.target_guid != victim {
                p.target_guid = victim;
                entities.guid().update(p);
            }
        }
    }
    fn stand_down(&mut self, guid: u64) {
        crate::combat::break_own_attacks(self.ctx, guid);
    }
    fn dismiss(&mut self, guid: u64) {
        pet::despawn_pet(self.ctx, guid);
    }
    fn restage(&mut self, guid: u64, at: Point, map_id: u32, instance_id: u64, now_ms: u32) {
        let entities = self.ctx.db.game_world_entity();
        let Some(mut p) = entities.guid().find(guid) else {
            return;
        };
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        p.map_id = map_id;
        p.instance_id = instance_id;
        p.x = at.x;
        p.y = at.y;
        p.z = at.z;
        p.grid_x = gx;
        p.grid_y = gy;
        p.cell = spatial::grid_cell_id(gx, gy);
        p.last_move_ms = now_ms;
        entities.guid().update(p);
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
