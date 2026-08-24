//! Durable authored movement intent and its named operations.

use spacetimedb::{table, ReducerContext, Table};

use super::edges::game_creature_ai_returning_home;
use super::mobility::game_creature_ai_summon_origin;
use super::{DefinitionRevision, IdleMovementIntent, MovementOperation, WalkingMode};
use super::{RelayForcedMovement, RelayMovement};
use crate::{game_creature_spawn, game_creature_waypoint, game_world_entity};

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoredIdleMovement {
    InheritSpawn,
    Stationary,
    RandomAroundCurrentPosition,
    Patrol,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthoredWalkingMode {
    Inherit,
    RunByDefault,
    WalkByDefault,
    RunWhileChasing,
    WalkWhileChasing,
}

/// Runtime movement state belongs to one creature lifecycle and one definition revision. An
/// engagement reset clears only combat-scoped immobilization. Death, despawn, respawn, and a
/// definition revision drop the full row.
#[table(accessor = game_creature_ai_movement_intent)]
#[derive(Clone)]
pub struct CreatureAiMovementIntent {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
    pub definition_revision: u64,
    pub creature_entry: u32,
    pub map_id: u32,
    pub instance_id: u64,
    pub idle: AuthoredIdleMovement,
    pub idle_active: bool,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub anchor_z: f32,
    pub random_radius_yd: f32,
    pub path_id: u32,
    pub patrol_paused: bool,
    pub combat_movement_active: bool,
    pub combat_movement_enabled: bool,
    pub follow_movement_active: bool,
    pub follow_movement_enabled: bool,
    pub walking: AuthoredWalkingMode,
    pub immobilized: bool,
    pub immobilized_combat_only: bool,
    pub facing_pending: bool,
    pub facing_reset: bool,
    pub facing_target_guid: u64,
}

/// An explicit authored patrol path for one creature entry on one map. Path zero never reads this
/// table. It always means that spawn's existing `game_creature_waypoint` route.
#[table(
    accessor = game_creature_ai_movement_path_waypoint,
    index(accessor = by_subject, btree(columns = [creature_entry, map_id, path_id]))
)]
pub struct CreatureAiMovementPathWaypoint {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub map_id: u32,
    pub path_id: u32,
    pub point: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub(crate) fn apply(
    ctx: &ReducerContext,
    creature_guid: u64,
    revision: DefinitionRevision,
    operation: MovementOperation,
) -> bool {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return false;
    };
    let table = ctx.db.game_creature_ai_movement_intent();
    let mut intent = table
        .creature_guid()
        .find(creature_guid)
        .filter(|intent| {
            intent.definition_revision == revision.value && same_subject(intent, &creature)
        })
        .unwrap_or_else(|| empty_intent(creature_guid, revision, &creature));

    match operation {
        MovementOperation::ReplaceIdle(idle) => {
            let path_id = match idle {
                IdleMovementIntent::Patrol(patrol) => patrol.path_id,
                _ => 0,
            };
            if matches!(idle, IdleMovementIntent::Patrol(_))
                && !path_exists(ctx, creature_guid, path_id)
            {
                return false;
            }
            intent.idle_active = true;
            intent.patrol_paused = false;
            intent.path_id = path_id;
            intent.idle = match idle {
                IdleMovementIntent::Stationary => AuthoredIdleMovement::Stationary,
                IdleMovementIntent::RandomAroundCurrentPosition(random) => {
                    intent.anchor_x = creature.x;
                    intent.anchor_y = creature.y;
                    intent.anchor_z = creature.z;
                    intent.random_radius_yd = random.radius_yd as f32;
                    AuthoredIdleMovement::RandomAroundCurrentPosition
                }
                IdleMovementIntent::Patrol(_) => AuthoredIdleMovement::Patrol,
            };
        }
        MovementOperation::SetPatrolPaused(pause) => {
            let authored_patrol = intent.idle_active && intent.idle == AuthoredIdleMovement::Patrol;
            let inherited_spawn_patrol = !intent.idle_active && path_exists(ctx, creature_guid, 0);
            if !authored_patrol && !inherited_spawn_patrol {
                return false;
            }
            intent.patrol_paused = pause.paused;
        }
        MovementOperation::SetCombatMovement(switch) => {
            let enabled = if intent.combat_movement_active {
                intent.combat_movement_enabled
            } else {
                true
            };
            if enabled == switch.enabled {
                return false;
            }
            intent.combat_movement_active = true;
            intent.combat_movement_enabled = switch.enabled;
        }
        MovementOperation::SetWalking(mode) => {
            intent.walking = match mode {
                WalkingMode::RunByDefault => AuthoredWalkingMode::RunByDefault,
                WalkingMode::WalkByDefault => AuthoredWalkingMode::WalkByDefault,
                WalkingMode::RunWhileChasing => AuthoredWalkingMode::RunWhileChasing,
                WalkingMode::WalkWhileChasing => AuthoredWalkingMode::WalkWhileChasing,
            };
        }
        MovementOperation::SetImmobilized(immobilized) => {
            intent.immobilized = immobilized.enabled;
            intent.immobilized_combat_only = immobilized.enabled && immobilized.combat_only;
        }
        MovementOperation::SetFollowMovement(switch) => {
            intent.follow_movement_active = true;
            intent.follow_movement_enabled = switch.enabled;
        }
        MovementOperation::Face(target_guid) => {
            let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
                return false;
            };
            if target.map_id != creature.map_id || target.instance_id != creature.instance_id {
                return false;
            }
            intent.facing_pending = true;
            intent.facing_reset = false;
            intent.facing_target_guid = target_guid;
        }
        MovementOperation::ResetFacing => {
            intent.facing_pending = true;
            intent.facing_reset = true;
            intent.facing_target_guid = 0;
        }
        MovementOperation::SetRangedMode(_) | MovementOperation::Evade(_) => return false,
    }

    match table.creature_guid().find(creature_guid) {
        Some(_) => {
            table.creature_guid().update(intent);
        }
        None => {
            table.insert(intent);
        }
    }
    true
}

pub(crate) fn apply_relay_idle(
    ctx: &ReducerContext,
    creature_guid: u64,
    idle: RelayMovement,
    forced: RelayForcedMovement,
) -> Result<(), String> {
    let revision = super::current_definition_revision(ctx, creature_guid);
    let operation = match idle {
        RelayMovement::Stationary => MovementOperation::ReplaceIdle(IdleMovementIntent::Stationary),
        RelayMovement::RandomAroundCurrent(super::relay::RelayRandomMovement { radius_yd }) => {
            MovementOperation::ReplaceIdle(IdleMovementIntent::RandomAroundCurrentPosition(
                super::RandomMovementIntent { radius_yd },
            ))
        }
        RelayMovement::Patrol(super::relay::RelayPatrolMovement { path_id }) => {
            if !path_exists(ctx, creature_guid, path_id) {
                return Err(format!("relay patrol path {path_id} is missing"));
            }
            MovementOperation::ReplaceIdle(IdleMovementIntent::Patrol(super::PatrolIntent {
                path_id,
            }))
        }
    };
    if !apply(ctx, creature_guid, revision, operation) {
        return Err(format!(
            "relay movement subject {creature_guid} refused idle movement"
        ));
    }
    if forced != RelayForcedMovement::Inherit {
        apply_relay_walking(ctx, creature_guid, forced)?;
    }
    Ok(())
}

pub(crate) fn apply_relay_walking(
    ctx: &ReducerContext,
    creature_guid: u64,
    movement: RelayForcedMovement,
) -> Result<(), String> {
    let mode = match movement {
        RelayForcedMovement::Inherit => return Ok(()),
        RelayForcedMovement::Walk => WalkingMode::WalkByDefault,
        RelayForcedMovement::Run => WalkingMode::RunByDefault,
    };
    apply(
        ctx,
        creature_guid,
        super::current_definition_revision(ctx, creature_guid),
        MovementOperation::SetWalking(mode),
    )
    .then_some(())
    .ok_or_else(|| format!("relay movement subject {creature_guid} refused walking mode"))
}

pub(crate) fn apply_relay_patrol_pause(
    ctx: &ReducerContext,
    creature_guid: u64,
    paused: bool,
) -> Result<(), String> {
    apply(
        ctx,
        creature_guid,
        super::current_definition_revision(ctx, creature_guid),
        MovementOperation::SetPatrolPaused(super::PatrolPause { paused }),
    )
    .then_some(())
    .ok_or_else(|| format!("relay movement subject {creature_guid} has no patrol to pause"))
}

pub(crate) fn apply_relay_facing(
    ctx: &ReducerContext,
    creature_guid: u64,
    target_guid: Option<u64>,
) -> Result<(), String> {
    let operation = target_guid.map_or(MovementOperation::ResetFacing, MovementOperation::Face);
    apply(
        ctx,
        creature_guid,
        super::current_definition_revision(ctx, creature_guid),
        operation,
    )
    .then_some(())
    .ok_or_else(|| format!("relay movement subject {creature_guid} refused facing"))
}

pub(crate) fn apply_relay_orientation(
    ctx: &ReducerContext,
    creature_guid: u64,
    orientation: f32,
) -> Result<(), String> {
    if !orientation.is_finite() {
        return Err("relay orientation must be finite".to_string());
    }
    let entities = ctx.db.game_world_entity();
    let mut creature = entities
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player() && !entity.dead)
        .ok_or_else(|| format!("relay facing subject {creature_guid} is unavailable"))?;
    creature.orientation = orientation;
    entities.guid().update(creature);
    Ok(())
}

pub(crate) fn relay_runs_by_default(ctx: &ReducerContext, creature_guid: u64) -> bool {
    !matches!(
        intent(ctx, creature_guid).map(|intent| intent.walking),
        Some(AuthoredWalkingMode::WalkByDefault | AuthoredWalkingMode::WalkWhileChasing)
    )
}

fn empty_intent(
    creature_guid: u64,
    revision: DefinitionRevision,
    creature: &crate::WorldEntity,
) -> CreatureAiMovementIntent {
    CreatureAiMovementIntent {
        creature_guid,
        definition_revision: revision.value,
        creature_entry: creature.entry,
        map_id: creature.map_id,
        instance_id: creature.instance_id,
        idle: AuthoredIdleMovement::InheritSpawn,
        idle_active: false,
        anchor_x: creature.x,
        anchor_y: creature.y,
        anchor_z: creature.z,
        random_radius_yd: 0.0,
        path_id: 0,
        patrol_paused: false,
        combat_movement_active: false,
        combat_movement_enabled: true,
        follow_movement_active: false,
        follow_movement_enabled: true,
        walking: AuthoredWalkingMode::Inherit,
        immobilized: false,
        immobilized_combat_only: false,
        facing_pending: false,
        facing_reset: false,
        facing_target_guid: 0,
    }
}

fn path_exists(ctx: &ReducerContext, creature_guid: u64, path_id: u32) -> bool {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return false;
    };
    if path_id == 0 {
        return ctx
            .db
            .game_creature_waypoint()
            .by_creature()
            .filter(&creature_guid)
            .next()
            .is_some();
    }
    ctx.db
        .game_creature_ai_movement_path_waypoint()
        .by_subject()
        .filter((creature.entry, creature.map_id, path_id))
        .next()
        .is_some()
}

pub(crate) fn intent(ctx: &ReducerContext, creature_guid: u64) -> Option<CreatureAiMovementIntent> {
    let creature = ctx.db.game_world_entity().guid().find(creature_guid)?;
    if !super::runs_eventai(&creature) {
        return None;
    }
    ctx.db
        .game_creature_ai_movement_intent()
        .creature_guid()
        .find(creature_guid)
        .filter(|intent| same_subject(intent, &creature))
}

pub(crate) fn explicit_route(
    ctx: &ReducerContext,
    creature_guid: u64,
    path_id: u32,
) -> Vec<CreatureAiMovementPathWaypoint> {
    let Some(intent) = intent(ctx, creature_guid).filter(|intent| intent.path_id == path_id) else {
        return Vec::new();
    };
    ctx.db
        .game_creature_ai_movement_path_waypoint()
        .by_subject()
        .filter((intent.creature_entry, intent.map_id, path_id))
        .collect()
}

fn same_subject(intent: &CreatureAiMovementIntent, creature: &crate::WorldEntity) -> bool {
    intent.creature_entry == creature.entry
        && intent.map_id == creature.map_id
        && intent.instance_id == creature.instance_id
}

pub(crate) fn follow_target(ctx: &ReducerContext, creature_guid: u64) -> Option<u64> {
    let intent = intent(ctx, creature_guid)?;
    if !intent.follow_movement_active || !intent.follow_movement_enabled {
        return None;
    }
    ctx.db
        .game_creature_ai_summon_origin()
        .creature_guid()
        .find(creature_guid)
        .map(|origin| origin.summoner_guid)
}

pub(crate) fn facing(ctx: &ReducerContext, creature_guid: u64) -> Option<f32> {
    let intent = intent(ctx, creature_guid)?;
    if !intent.facing_pending {
        return None;
    }
    let creature = ctx.db.game_world_entity().guid().find(creature_guid)?;
    if intent.facing_reset {
        return ctx
            .db
            .game_creature_spawn()
            .guid()
            .find(creature_guid)
            .map(|spawn| spawn.orientation);
    }
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(intent.facing_target_guid)?;
    (target.map_id == creature.map_id && target.instance_id == creature.instance_id)
        .then(|| (target.y - creature.y).atan2(target.x - creature.x))
}

pub(crate) fn clear_facing(ctx: &ReducerContext, creature_guid: u64) {
    let table = ctx.db.game_creature_ai_movement_intent();
    if let Some(mut intent) = table.creature_guid().find(creature_guid) {
        intent.facing_pending = false;
        intent.facing_reset = false;
        intent.facing_target_guid = 0;
        table.creature_guid().update(intent);
    }
}

pub(crate) fn reset_engagement(ctx: &ReducerContext, creature_guid: u64) {
    let table = ctx.db.game_creature_ai_movement_intent();
    if let Some(mut intent) = table.creature_guid().find(creature_guid) {
        if intent.immobilized_combat_only {
            intent.immobilized = false;
            intent.immobilized_combat_only = false;
            table.creature_guid().update(intent);
        }
    }
}

pub(crate) fn reset_revision(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_ai_movement_intent()
        .creature_guid()
        .delete(creature_guid);
}

pub(crate) fn drop_lifecycle(ctx: &ReducerContext, creature_guid: u64) {
    reset_revision(ctx, creature_guid);
}

pub(crate) fn returning_home(ctx: &ReducerContext, creature_guid: u64) -> bool {
    ctx.db
        .game_creature_ai_returning_home()
        .creature_guid()
        .find(creature_guid)
        .is_some()
}
