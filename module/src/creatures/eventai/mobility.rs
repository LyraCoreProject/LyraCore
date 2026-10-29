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
    // Summon guids repeat when the sequence wraps; this id distinguishes their lives.
    #[default(0u64)]
    pub life_seq: u64,
    #[default(None::<u32>)]
    pub guardian_faction: Option<u32>,
}

#[table(accessor = game_creature_ai_summon_origin)]
pub struct CreatureAiSummonOrigin {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
    pub summoner_guid: u64,
}

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

fn default_state(ctx: &ReducerContext, creature_guid: u64) -> CreatureAiState {
    CreatureAiState {
        creature_guid,
        phase: 0,
        lifecycle_id: crate::creatures::current_life_seq(ctx, creature_guid),
        engagement_id: 1,
        ranged_distance: 0.0,
        ranged_angle: 0.0,
        ranged_posture_active: false,
        definition_revision: 0,
        active_object: false,
        react_state: 2,
        melee_enabled: true,
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
        .unwrap_or_else(|| default_state(ctx, creature_guid));
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
        .unwrap_or_else(|| default_state(ctx, creature_guid));
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
    if let Some(pet) = crate::creatures::pet_of(ctx, summoner_guid) {
        if creature_entry == 0 || pet.entry == creature_entry {
            crate::creatures::despawn_pets(ctx, summoner_guid);
        }
    }
    let guardians: Vec<_> = ctx
        .db
        .game_world_entity()
        .by_owner_guid()
        .filter(summoner_guid)
        .filter(|entity| creature_entry == 0 || entity.entry == creature_entry)
        .filter(|entity| crate::creatures::guardian::is_guardian(ctx, entity.guid))
        .map(|entity| entity.guid)
        .collect();
    for guid in guardians {
        crate::creatures::despawn_creature_entity(ctx, guid);
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn mark_summon_origin_for_debug(
    ctx: &ReducerContext,
    creature_guid: u64,
    summoner_guid: u64,
) {
    ctx.db
        .game_creature_ai_summon_origin()
        .insert(CreatureAiSummonOrigin {
            creature_guid,
            summoner_guid,
        });
}

fn applied(applied: bool) -> ActionResult {
    if applied {
        ActionResult::Applied
    } else {
        ActionResult::Refused
    }
}

pub(crate) fn ranged_posture(ctx: &ReducerContext, creature_guid: u64) -> Option<(f32, f32)> {
    let creature = ctx.db.game_world_entity().guid().find(creature_guid)?;
    if !super::runs_eventai(ctx, &creature) {
        return None;
    }
    ctx.db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .filter(|state| state.ranged_posture_active)
        .map(|state| (state.ranged_distance, state.ranged_angle))
}

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

pub(crate) fn summon_life_seq(ctx: &ReducerContext, creature_guid: u64) -> u64 {
    ctx.db
        .game_creature_ai_summon_expiry()
        .creature_guid()
        .find(creature_guid)
        .map_or(0, |expiry| expiry.life_seq)
}

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
            life_seq: 0,
            guardian_faction: None,
        })
        .scheduled_id
}

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
    place_summon_entity(ctx, sequence, guid, entry, location, summoner, None);
}

fn place_summon_entity(
    ctx: &ReducerContext,
    sequence: u64,
    guid: u64,
    entry: u32,
    location: &SummonLocation,
    summoner: &EventAiUnit,
    guardian_seed: Option<u32>,
) {
    let guardian = guardian_seed.is_some();
    let expiry_table = ctx.db.game_creature_ai_summon_expiry();
    if let Some(mut expiry) = expiry_table.scheduled_id().find(sequence) {
        expiry.creature_guid = guid;
        expiry.guardian_faction = guardian.then_some(summoner.faction_template);
        // Capture the claim id before re-insertion can assign a different lifetime-check id.
        expiry.life_seq = sequence;
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
        life_seq: 0,
        source_guid: 0,
        instance_id: summoner.instance_id,
    };
    let mut entity = crate::creatures::build_creature_entity(
        ctx,
        &spawn,
        &template,
        guardian_seed.unwrap_or_else(|| ctx.random()),
        summoner.instance_id,
    );
    if guardian {
        entity.owner_guid = summoner.guid;
        entity.faction_template = summoner.faction_template;
    }
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
        .filter(|unit| !unit.is_player() && !unit.dead)
        .ok_or_else(|| format!("relay summoner {summoner_guid} is unavailable"))?;
    let guid = place_temporary_summon(ctx, &summoner, entry, location)?;
    if active {
        set_active_object(ctx, guid, true)?;
    }
    if run_by_default {
        super::movement::apply_relay_walking(ctx, guid, super::RelayForcedMovement::Run)?;
    }
    Ok(guid)
}

pub(crate) fn place_temporary_summon(
    ctx: &ReducerContext,
    caster: &crate::WorldEntity,
    entry: u32,
    location: SummonLocation,
) -> Result<u64, String> {
    place_owned_summon(ctx, caster, entry, location, None)
}

pub(crate) fn place_guardian(
    ctx: &ReducerContext,
    caster: &crate::WorldEntity,
    entry: u32,
    location: SummonLocation,
    level_roll: u32,
) -> Result<u64, String> {
    place_owned_summon(ctx, caster, entry, location, Some(level_roll))
}

fn place_owned_summon(
    ctx: &ReducerContext,
    caster: &crate::WorldEntity,
    entry: u32,
    location: SummonLocation,
    guardian_seed: Option<u32>,
) -> Result<u64, String> {
    let summoner_guid = caster.guid;
    let summoner = super::engine::unit_of(caster);
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_none()
    {
        return Err(format!("summon template {entry} is missing"));
    }
    if ![location.x, location.y, location.z, location.orientation]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err("summon location must be finite".to_string());
    }
    let sequence = claim_summon_sequence(ctx, location.lifetime_ms);
    let Some(guid) = summon_guid(entry, sequence) else {
        release_summon_sequence(ctx, sequence);
        return Err("summon sequence is unavailable".to_string());
    };
    if ctx.db.game_world_entity().guid().find(guid).is_some() {
        release_summon_sequence(ctx, sequence);
        return Err(format!("summon guid {guid} is already live"));
    }
    place_summon_entity(
        ctx,
        sequence,
        guid,
        entry,
        &location,
        &summoner,
        guardian_seed,
    );
    super::edges::eventai_on_summoned(ctx, summoner_guid, guid, entry);
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
    let fire_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros(i64::from(delay_ms) * 1_000))
        .unwrap_or(crate::time::now(ctx));
    ScheduleAt::Time(fire_at)
}

fn timestamp_ms(ctx: &ReducerContext) -> u64 {
    (crate::time::now(ctx).to_micros_since_unix_epoch() / 1_000) as u64
}

#[reducer]
pub fn expire_eventai_summon(ctx: &ReducerContext, expiry: CreatureAiSummonExpiry) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    apply_summon_expiry(ctx, expiry);
}

fn apply_summon_expiry(ctx: &ReducerContext, expiry: CreatureAiSummonExpiry) {
    let expiries = ctx.db.game_creature_ai_summon_expiry();
    let Some(current) = expiries.scheduled_id().find(expiry.scheduled_id) else {
        return;
    };
    if current.creature_guid != expiry.creature_guid || current.life_seq != expiry.life_seq {
        return;
    }
    // Remove only this check before rearming; an obsolete callback must not remove its successor.
    expiries.scheduled_id().delete(expiry.scheduled_id);
    let Some(creature) = ctx.db.game_world_entity().guid().find(expiry.creature_guid) else {
        crate::creatures::reset_creature_lifecycle(ctx, expiry.creature_guid);
        return;
    };
    if expiry.guardian_faction.is_some()
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(creature.owner_guid)
            .is_none_or(|owner| {
                owner.map_id != creature.map_id
                    || owner.instance_id != creature.instance_id
                    || (owner.x - creature.x).powi(2)
                        + (owner.y - creature.y).powi(2)
                        + (owner.z - creature.z).powi(2)
                        > 120.0 * 120.0
            })
    {
        despawn_temporary_summon(ctx, expiry.creature_guid);
        return;
    }
    if creature.dead && expiry.guardian_faction.is_none() {
        despawn_temporary_summon(ctx, expiry.creature_guid);
        return;
    }

    let now_ms = timestamp_ms(ctx);
    let elapsed_ms = now_ms
        .saturating_sub(expiry.last_checked_ms)
        .min(u64::from(u32::MAX)) as u32;
    let engaged =
        expiry.guardian_faction.is_none() && crate::combat::is_engaged(ctx, expiry.creature_guid);
    let Some(remaining_ms) =
        (if expiry.guardian_faction.is_some() && !creature.dead && expiry.lifetime_ms == 0 {
            Some(0)
        } else {
            summon_lifetime_after(engaged, expiry.lifetime_ms, expiry.remaining_ms, elapsed_ms)
        })
    else {
        despawn_temporary_summon(ctx, expiry.creature_guid);
        return;
    };
    let delay_ms = if engaged || remaining_ms == 0 {
        SUMMON_CHECK_MS
    } else {
        next_check_ms(remaining_ms)
    };
    expiries.insert(CreatureAiSummonExpiry {
        scheduled_id: 0,
        scheduled_at: schedule_after(ctx, delay_ms),
        creature_guid: expiry.creature_guid,
        lifetime_ms: expiry.lifetime_ms,
        remaining_ms,
        last_checked_ms: now_ms,
        life_seq: expiry.life_seq,
        guardian_faction: expiry.guardian_faction,
    });
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn verify_summon_expiry_boundaries_for_debug(
    ctx: &ReducerContext,
    caster: &crate::WorldEntity,
    entry: u32,
) -> Result<(), String> {
    let location = |lifetime_ms| SummonLocation {
        x: caster.x + 2.0,
        y: caster.y,
        z: caster.z,
        orientation: caster.orientation,
        lifetime_ms,
    };

    let replaced_guid = place_temporary_summon(ctx, caster, entry, location(60_000))?;
    let stale = ctx
        .db
        .game_creature_ai_summon_expiry()
        .creature_guid()
        .find(replaced_guid)
        .ok_or_else(|| "fixture summon has no expiry callback".to_string())?;
    despawn_temporary_summon(ctx, replaced_guid);

    let replacement_sequence = claim_summon_sequence(ctx, 60_000);
    let replacement_location = location(60_000);
    let summoner = super::engine::unit_of(caster);
    place_summon(
        ctx,
        replacement_sequence,
        replaced_guid,
        entry,
        &replacement_location,
        &summoner,
    );
    super::edges::eventai_on_summoned(ctx, caster.guid, replaced_guid, entry);
    let replacement = ctx
        .db
        .game_creature_ai_summon_expiry()
        .creature_guid()
        .find(replaced_guid)
        .ok_or_else(|| "replacement summon has no expiry callback".to_string())?;
    if replacement.life_seq == stale.life_seq {
        return Err("replacement summon reused the obsolete life number".to_string());
    }

    apply_summon_expiry(ctx, stale);
    let retained = ctx
        .db
        .game_creature_ai_summon_expiry()
        .creature_guid()
        .find(replaced_guid)
        .ok_or_else(|| "obsolete callback removed the replacement expiry".to_string())?;
    if retained.scheduled_id != replacement.scheduled_id
        || retained.life_seq != replacement.life_seq
        || ctx
            .db
            .game_world_entity()
            .guid()
            .find(replaced_guid)
            .is_none()
    {
        return Err("obsolete callback changed the replacement summon".to_string());
    }

    place_temporary_summon(ctx, caster, entry, location(5_000))?;
    Ok(())
}

fn despawn_temporary_summon(ctx: &ReducerContext, creature_guid: u64) {
    crate::creatures::despawn_creature_entity(ctx, creature_guid);
}

pub(crate) fn melee_enabled(ctx: &ReducerContext, creature_guid: u64) -> bool {
    ctx.db
        .game_creature_ai_state()
        .creature_guid()
        .find(creature_guid)
        .is_none_or(|state| state.melee_enabled)
}

pub(super) fn set_auto_attack(ctx: &ReducerContext, creature_guid: u64, enabled: bool) -> bool {
    if !ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .is_some_and(|entity| !entity.is_player() && !entity.dead)
    {
        return false;
    }
    let table = ctx.db.game_creature_ai_state();
    let existing = table.creature_guid().find(creature_guid);
    let mut state = existing.unwrap_or_else(|| default_state(ctx, creature_guid));
    state.melee_enabled = enabled;
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(state);
    } else {
        table.insert(state);
    }
    true
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
