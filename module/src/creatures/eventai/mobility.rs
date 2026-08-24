//! EventAI summon actions and ranged posture.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use super::engine::EventAiWorld;
use super::{
    ActionResult, CreatureAiState, CreatureInstruction, CreatureReactState, EventAiUnit,
    EventContext, InstructionTarget, SummonLocation,
};
use crate::{game_creature_ai_state, game_creature_template, game_world_entity};

const SUMMON_CHECK_MS: u32 = 500;
const SUMMON_LOW_BAND: u64 = 0x40_0000;
const SUMMON_SEQUENCE_MASK: u64 = SUMMON_LOW_BAND - 1;

/// One temporary EventAI summon waiting for its out-of-combat lifetime to finish. Module only.
#[table(
    accessor = game_creature_ai_summon_expiry,
    scheduled(expire_eventai_summon)
)]
pub struct CreatureAiSummonExpiry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub creature_guid: u64,
    pub lifetime_ms: u32,
    pub remaining_ms: u32,
    pub last_checked_ms: u64,
}

/// The EventAI creature that created one temporary summon. Module only.
#[table(accessor = game_creature_ai_summon_origin)]
pub struct CreatureAiSummonOrigin {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
    pub summoner_guid: u64,
}

/// One authored delayed source despawn. Module only.
#[table(
    accessor = game_creature_ai_forced_despawn,
    scheduled(fire_eventai_forced_despawn)
)]
pub struct CreatureAiForcedDespawn {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub creature_guid: u64,
}

pub(super) fn execute<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    instruction: &CreatureInstruction,
    choice: u64,
) -> ActionResult {
    match instruction {
        CreatureInstruction::Summon(summon_instruction) => summon(
            world,
            context,
            summon_instruction.creature_entry,
            summon_instruction.summon_location_id,
            summon_instruction.target,
            choice,
        ),
        CreatureInstruction::SpawnAtActor(spawn) => spawn_at_actor(
            world,
            context,
            spawn.creature_entry,
            spawn.lifetime_ms,
            spawn.target,
            choice,
        ),
        CreatureInstruction::SetRangedPosture(posture) => {
            let distance = posture.distance_yd as f32;
            let angle_rad = if posture.distance_yd == 0 {
                0.0
            } else {
                (posture.angle_degrees as f32).to_radians()
            };
            world.set_eventai_ranged_posture(context.creature_guid, distance, angle_rad);
            ActionResult::Applied
        }
        CreatureInstruction::Movement(operation) => {
            applied(world.apply_eventai_movement(context.creature_guid, *operation))
        }
        CreatureInstruction::SetFacing(facing) => {
            let operation = if facing.reset {
                super::MovementOperation::ResetFacing
            } else {
                let Some(target_guid) =
                    super::combat::unit_target(world, context, facing.target, None, choice)
                else {
                    return ActionResult::Refused;
                };
                super::MovementOperation::Face(target_guid)
            };
            applied(world.apply_eventai_movement(context.creature_guid, operation))
        }
        _ => ActionResult::Unsupported,
    }
}

fn spawn_at_actor<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    creature_entry: u32,
    lifetime_ms: u32,
    target: InstructionTarget,
    choice: u64,
) -> ActionResult {
    let Some(summoner) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    if !world.eventai_summon_template_exists(creature_entry) {
        return ActionResult::Refused;
    }
    let selected_target = super::combat::unit_target(world, context, target, None, choice);
    if target != InstructionTarget::SelfActor && selected_target.is_none() {
        return ActionResult::Refused;
    }
    let sequence = world.eventai_claim_summon_sequence(lifetime_ms);
    let Some(guid) = summon_guid(creature_entry, sequence) else {
        world.eventai_release_summon_sequence(sequence);
        return ActionResult::Refused;
    };
    let location = SummonLocation {
        x: summoner.x,
        y: summoner.y,
        z: summoner.z,
        orientation: summoner.orientation,
        lifetime_ms,
    };
    world.eventai_place_summon(sequence, guid, creature_entry, &location, &summoner);
    if target != InstructionTarget::SelfActor {
        if let Some(target_guid) = selected_target {
            world.eventai_engage_summon(guid, target_guid);
        }
    }
    ActionResult::Applied
}

fn default_state(creature_guid: u64) -> CreatureAiState {
    CreatureAiState {
        creature_guid,
        phase: 0,
        lifecycle_id: 1,
        engagement_id: 1,
        ranged_distance: 0.0,
        ranged_angle: 0.0,
        ranged_posture_active: false,
        definition_revision: 0,
        active_object: false,
        react_state: 2,
    }
}

pub(crate) fn set_active_object(
    ctx: &ReducerContext,
    creature_guid: u64,
    active: bool,
) -> Result<(), String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player() && !entity.dead)
        .ok_or_else(|| format!("active-object creature {creature_guid} is unavailable"))?;
    let table = ctx.db.game_creature_ai_state();
    let mut state = table
        .creature_guid()
        .find(creature_guid)
        .unwrap_or_else(|| default_state(creature_guid));
    state.active_object = active;
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(state);
    } else {
        table.insert(state);
    }
    Ok(())
}

pub(crate) fn set_react_state(
    ctx: &ReducerContext,
    creature_guid: u64,
    react: CreatureReactState,
) -> Result<(), String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player() && !entity.dead)
        .ok_or_else(|| format!("react-state creature {creature_guid} is unavailable"))?;
    let table = ctx.db.game_creature_ai_state();
    let mut state = table
        .creature_guid()
        .find(creature_guid)
        .unwrap_or_else(|| default_state(creature_guid));
    state.react_state = match react {
        CreatureReactState::Passive => 0,
        CreatureReactState::Defensive => 1,
        CreatureReactState::Aggressive => 2,
    };
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(state);
    } else {
        table.insert(state);
    }
    Ok(())
}

pub(crate) fn react_state(ctx: &ReducerContext, creature_guid: u64) -> CreatureReactState {
    match ctx
        .db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .map(|state| state.react_state)
        .unwrap_or(2)
    {
        0 => CreatureReactState::Passive,
        1 => CreatureReactState::Defensive,
        _ => CreatureReactState::Aggressive,
    }
}

pub(crate) fn active_object(ctx: &ReducerContext, creature_guid: u64) -> bool {
    ctx.db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .is_some_and(|state| state.active_object)
}

pub(crate) fn force_despawn(
    ctx: &ReducerContext,
    creature_guid: u64,
    delay_ms: u32,
) -> Result<(), String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .filter(|entity| !entity.is_player())
        .ok_or_else(|| format!("forced-despawn creature {creature_guid} is unavailable"))?;
    if delay_ms == 0 {
        crate::creatures::despawn_creature_entity(ctx, creature_guid);
        return Ok(());
    }
    ctx.db
        .game_creature_ai_forced_despawn()
        .creature_guid()
        .delete(creature_guid);
    ctx.db
        .game_creature_ai_forced_despawn()
        .insert(CreatureAiForcedDespawn {
            scheduled_id: 0,
            scheduled_at: schedule_after(ctx, delay_ms),
            creature_guid,
        });
    Ok(())
}

pub(crate) fn drop_forced_despawn(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_ai_forced_despawn()
        .creature_guid()
        .delete(creature_guid);
}

#[reducer]
pub fn fire_eventai_forced_despawn(ctx: &ReducerContext, row: CreatureAiForcedDespawn) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(row.creature_guid)
        .is_some()
    {
        crate::creatures::despawn_creature_entity(ctx, row.creature_guid);
    }
}

pub(crate) fn remove_guardians(
    ctx: &ReducerContext,
    summoner_guid: u64,
    creature_entry: u32,
) -> Result<(), String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(summoner_guid)
        .filter(|entity| !entity.is_player())
        .ok_or_else(|| format!("guardian owner {summoner_guid} is unavailable"))?;
    let entities = ctx.db.game_world_entity();
    let mut guardians = ctx
        .db
        .game_creature_ai_summon_origin()
        .iter()
        .filter(|origin| origin.summoner_guid == summoner_guid)
        .filter_map(|origin| {
            let entity = entities.guid().find(origin.creature_guid)?;
            (creature_entry == 0 || entity.entry == creature_entry).then_some(entity.guid)
        })
        .collect::<Vec<_>>();
    guardians.sort_unstable();
    if creature_entry != 0 {
        guardians.truncate(1);
    }
    for guardian in guardians {
        crate::creatures::despawn_creature_entity(ctx, guardian);
    }
    Ok(())
}

fn applied(applied: bool) -> ActionResult {
    if applied {
        ActionResult::Applied
    } else {
        ActionResult::Refused
    }
}

/// The authored ranged posture holding this creature at `(distance, angle)` from its victim, if
/// one is active. Behind `runs_eventai` like every other EventAI read: a tamed creature answers
/// its owner, never a posture its wild entry's state row may still carry.
pub(crate) fn ranged_posture(ctx: &ReducerContext, creature_guid: u64) -> Option<(f32, f32)> {
    let creature = ctx.db.game_world_entity().guid().find(creature_guid)?;
    if !super::runs_eventai(&creature) {
        return None;
    }
    ctx.db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .filter(|state| state.ranged_posture_active)
        .map(|state| (state.ranged_distance, state.ranged_angle))
}

/// Forget a despawned creature's pending summon-lifetime check.
pub(crate) fn drop_summon_expiry(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_ai_summon_expiry()
        .creature_guid()
        .delete(creature_guid);
    ctx.db
        .game_creature_ai_summon_origin()
        .creature_guid()
        .delete(creature_guid);
}

fn summon<W: EventAiWorld>(
    world: &mut W,
    context: &EventContext,
    creature_entry: u32,
    summon_location_id: u32,
    target: InstructionTarget,
    choice: u64,
) -> ActionResult {
    let Some(summoner) = world.eventai_unit(context.creature_guid) else {
        return ActionResult::Refused;
    };
    let Some(location) = world.eventai_summon_location(summon_location_id) else {
        return ActionResult::Refused;
    };
    if !world.eventai_summon_template_exists(creature_entry) {
        return ActionResult::Refused;
    }
    if ![location.x, location.y, location.z, location.orientation]
        .into_iter()
        .all(f32::is_finite)
    {
        return ActionResult::Refused;
    }

    let selected_target = super::combat::unit_target(world, context, target, None, choice);
    if target != InstructionTarget::SelfActor && selected_target.is_none() {
        return ActionResult::Refused;
    }
    let sequence = world.eventai_claim_summon_sequence(location.lifetime_ms);
    let Some(guid) = summon_guid(creature_entry, sequence) else {
        world.eventai_release_summon_sequence(sequence);
        return ActionResult::Refused;
    };
    if world.eventai_unit(guid).is_some() {
        world.eventai_release_summon_sequence(sequence);
        return ActionResult::Refused;
    }
    world.eventai_place_summon(sequence, guid, creature_entry, &location, &summoner);

    if target != InstructionTarget::SelfActor {
        if let Some(target_guid) = selected_target {
            world.eventai_engage_summon(guid, target_guid);
        }
    }
    ActionResult::Applied
}

/// Reserve the durable expiry slot whose id is the summon's sequence number.
pub(super) fn claim_summon_sequence(ctx: &ReducerContext, lifetime_ms: u32) -> u64 {
    ctx.db
        .game_creature_ai_summon_expiry()
        .insert(CreatureAiSummonExpiry {
            scheduled_id: 0,
            scheduled_at: schedule_after(ctx, next_check_ms(lifetime_ms)),
            creature_guid: 0,
            lifetime_ms,
            remaining_ms: lifetime_ms,
            last_checked_ms: timestamp_ms(ctx),
        })
        .scheduled_id
}

/// Give back a claimed slot whose summon was refused.
pub(super) fn release_summon_sequence(ctx: &ReducerContext, sequence: u64) {
    ctx.db
        .game_creature_ai_summon_expiry()
        .scheduled_id()
        .delete(sequence);
}

pub(super) fn place_summon(
    ctx: &ReducerContext,
    sequence: u64,
    guid: u64,
    entry: u32,
    location: &SummonLocation,
    summoner: &EventAiUnit,
) {
    let expiry_table = ctx.db.game_creature_ai_summon_expiry();
    if let Some(mut expiry) = expiry_table.scheduled_id().find(sequence) {
        expiry.creature_guid = guid;
        expiry_table.scheduled_id().update(expiry);
    }
    ctx.db
        .game_creature_ai_summon_origin()
        .insert(CreatureAiSummonOrigin {
            creature_guid: guid,
            summoner_guid: summoner.guid,
        });
    let Some(template) = ctx.db.game_creature_template().entry().find(entry) else {
        return;
    };

    let spawn = crate::creatures::CreatureSpawn {
        guid,
        entry,
        map_id: summoner.map_id,
        x: location.x,
        y: location.y,
        z: location.z,
        orientation: location.orientation,
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: u32::MAX,
    };
    let entity = crate::creatures::build_creature_entity(
        &spawn,
        &template,
        ctx.random(),
        summoner.instance_id,
    );
    crate::creatures::insert_creature_entity(ctx, entity);
}

pub(super) fn engage_summon(ctx: &ReducerContext, creature_guid: u64, target_guid: u64) {
    crate::combat::arm_creature_engagement(ctx, creature_guid, target_guid, true);
}

pub(super) fn place_relay_summon(
    ctx: &ReducerContext,
    summoner_guid: u64,
    entry: u32,
    location: SummonLocation,
    active: bool,
    run_by_default: bool,
) -> Result<u64, String> {
    let summoner = ctx
        .db
        .game_world_entity()
        .guid()
        .find(summoner_guid)
        .map(|entity| super::EventAiUnit {
            guid: entity.guid,
            entry: entity.entry,
            x: entity.x,
            y: entity.y,
            z: entity.z,
            map_id: entity.map_id,
            instance_id: entity.instance_id,
            zone_id: entity.zone_id,
            health: entity.health,
            max_health: entity.max_health,
            power: entity.power,
            max_power: entity.max_power,
            power_type: (entity.unit_bytes_0 >> 24) as u8,
            level: entity.level,
            faction_template: entity.faction_template,
            dead: entity.dead,
            is_player: entity.is_player(),
            orientation: entity.orientation,
            owner_guid: entity.owner_guid,
        })
        .filter(|unit| !unit.is_player && !unit.dead)
        .ok_or_else(|| format!("relay summoner {summoner_guid} is unavailable"))?;
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_none()
    {
        return Err(format!("relay summon template {entry} is missing"));
    }
    if ![location.x, location.y, location.z, location.orientation]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err("relay summon location must be finite".to_string());
    }
    let sequence = claim_summon_sequence(ctx, location.lifetime_ms);
    let guid = summon_guid(entry, sequence)
        .ok_or_else(|| "relay summon sequence is unavailable".to_string())?;
    if ctx.db.game_world_entity().guid().find(guid).is_some() {
        release_summon_sequence(ctx, sequence);
        return Err(format!("relay summon guid {guid} is already live"));
    }
    place_summon(ctx, sequence, guid, entry, &location, &summoner);
    super::edges::eventai_on_summoned(ctx, summoner_guid, guid, entry);
    if active {
        set_active_object(ctx, guid, true)?;
    }
    if run_by_default {
        super::movement::apply_relay_walking(ctx, guid, super::RelayForcedMovement::Run)?;
    }
    Ok(guid)
}

fn summon_guid(entry: u32, scheduled_id: u64) -> Option<u64> {
    let sequence = scheduled_id.checked_sub(1)? % SUMMON_SEQUENCE_MASK + 1;
    Some(crate::encounter::wave_guid(
        entry,
        SUMMON_LOW_BAND | sequence,
    ))
}

fn next_check_ms(remaining_ms: u32) -> u32 {
    remaining_ms.clamp(1, SUMMON_CHECK_MS)
}

/// One lifetime check on a temporary summon: the out-of-combat ms it has left after `elapsed_ms`,
/// or `None` when its time is up and it despawns. A fight refills the whole lifetime, so an
/// engaged summon never runs out mid-swing.
pub(crate) fn summon_lifetime_after(
    engaged: bool,
    lifetime_ms: u32,
    remaining_ms: u32,
    elapsed_ms: u32,
) -> Option<u32> {
    if engaged {
        return Some(lifetime_ms);
    }
    let remaining = remaining_ms.saturating_sub(elapsed_ms);
    (remaining != 0).then_some(remaining)
}

fn schedule_after(ctx: &ReducerContext, delay_ms: u32) -> ScheduleAt {
    let fire_at = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros(i64::from(delay_ms) * 1_000))
        .unwrap_or(ctx.timestamp);
    ScheduleAt::Time(fire_at)
}

fn timestamp_ms(ctx: &ReducerContext) -> u64 {
    (ctx.timestamp.to_micros_since_unix_epoch() / 1_000) as u64
}

#[reducer]
pub fn expire_eventai_summon(ctx: &ReducerContext, expiry: CreatureAiSummonExpiry) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let Some(creature) = ctx.db.game_world_entity().guid().find(expiry.creature_guid) else {
        crate::creatures::reset_creature_lifecycle(ctx, expiry.creature_guid);
        return;
    };
    if creature.dead {
        despawn_temporary_summon(ctx, expiry.creature_guid);
        return;
    }

    let now_ms = timestamp_ms(ctx);
    let elapsed_ms = now_ms
        .saturating_sub(expiry.last_checked_ms)
        .min(u64::from(u32::MAX)) as u32;
    let engaged = crate::combat::is_engaged(ctx, expiry.creature_guid);
    let Some(remaining_ms) =
        summon_lifetime_after(engaged, expiry.lifetime_ms, expiry.remaining_ms, elapsed_ms)
    else {
        despawn_temporary_summon(ctx, expiry.creature_guid);
        return;
    };
    let delay_ms = if engaged {
        SUMMON_CHECK_MS
    } else {
        next_check_ms(remaining_ms)
    };
    ctx.db
        .game_creature_ai_summon_expiry()
        .insert(CreatureAiSummonExpiry {
            scheduled_id: 0,
            scheduled_at: schedule_after(ctx, delay_ms),
            creature_guid: expiry.creature_guid,
            lifetime_ms: expiry.lifetime_ms,
            remaining_ms,
            last_checked_ms: now_ms,
        });
}

fn despawn_temporary_summon(ctx: &ReducerContext, creature_guid: u64) {
    crate::creatures::despawn_creature_entity(ctx, creature_guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summon_guids_reuse_the_eventai_band_without_corrupting_entry_bits() {
        let guid = summon_guid(123, 7).unwrap();
        assert_eq!(guid & 0xFF_FFFF, SUMMON_LOW_BAND | 7);
        assert_eq!((guid >> 24) & 0xFF_FFFF, 123);
        assert_eq!(
            summon_guid(123, SUMMON_SEQUENCE_MASK + 1).unwrap() & 0xFF_FFFF,
            SUMMON_LOW_BAND | 1
        );
        assert!(summon_guid(123, 0).is_none());
    }
}
