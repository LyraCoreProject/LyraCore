//! The ONE production adapter: a real `ReducerContext` wearing the whole cycle world.
//!
//! **This layer is the seam's own blind spot.** The harness substitutes an in-memory `Scenario` for
//! every line of it, so a no-op'd method here is invisible to every test in the crate. Keep every
//! method a pass-through; ticket 09 pins its exact shape the way `transfer::tests::
//! the_production_adapter_is_the_pass_through_the_harness_assumes` pins `CtxShard`.

use std::collections::HashSet;

use lyracore_shared::{constants, spatial};
use spacetimedb::{ReducerContext, Table};

use super::{
    run_cycle, AggroTarget, CreatureWorld, CycleOutcome, EngageSink, Engagement, Gait, Home,
    IdleCreature, IdleSink, Leg, LegInFlight, LegacyPasses, MotionSink, Point, Pull, Sensor,
    TickContext, Waypoint,
};
use crate::creatures::ai::TickScope;
use crate::creatures::tick::{self, TickSweep};
use crate::{
    game_creature_spawn, game_creature_spline, game_creature_template, game_creature_waypoint,
    game_melee_attack, game_world_entity, MeleeAttack,
};

/// `tick_creatures`' one call into the cycle. The adapter never leaves this module.
pub(crate) fn run(ctx: &ReducerContext, tick: TickContext, interval_micros: i64) -> CycleOutcome {
    run_cycle(
        &mut CtxWorld {
            ctx,
            interval_micros,
        },
        tick,
    )
}

struct CtxWorld<'a> {
    ctx: &'a ReducerContext,
    /// The firing row's own cadence — only `pass_pet` still reads it.
    // ponytail: migration scaffolding, deleted with `legacy_pet` in ticket 07.
    interval_micros: i64,
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
                    // A PET never pulls or assists on its owner's behalf — `pass_pet` arms it off
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
