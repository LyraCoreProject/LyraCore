//! EventAI summon actions and ranged posture.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use super::{ActionKind, ActionResult, CreatureAiState, EventContext, RuleAction, TargetPolicy};
use crate::{
    game_creature_ai_state, game_creature_ai_summon, game_creature_template, game_world_entity,
};

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

pub(super) fn execute(
    ctx: &ReducerContext,
    context: &EventContext,
    action: &RuleAction,
) -> ActionResult {
    match action.kind {
        ActionKind::Summon => summon(ctx, context, action),
        ActionKind::SetRangedPosture => set_ranged_posture(ctx, context, action),
        _ => ActionResult::Unsupported,
    }
}

pub(crate) fn ranged_posture(ctx: &ReducerContext, creature_guid: u64) -> Option<(f32, f32)> {
    ctx.db.game_world_entity().guid().find(creature_guid)?;
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
}

fn set_ranged_posture(
    ctx: &ReducerContext,
    context: &EventContext,
    action: &RuleAction,
) -> ActionResult {
    let distance = action.params[0] as f32;
    let angle = if action.params[0] == 0 {
        0.0
    } else {
        (action.params[1] as i32 as f32).to_radians()
    };
    let states = ctx.db.game_creature_ai_state();
    match states.creature_guid().find(context.creature_guid) {
        Some(mut state) => {
            state.ranged_distance = distance;
            state.ranged_angle = angle;
            state.ranged_posture_active = true;
            states.creature_guid().update(state);
        }
        None => {
            states.insert(CreatureAiState {
                creature_guid: context.creature_guid,
                phase: 0,
                lifecycle_id: 1,
                engagement_id: 1,
                ranged_distance: distance,
                ranged_angle: angle,
                ranged_posture_active: true,
            });
        }
    }
    ActionResult::Applied
}

fn summon(ctx: &ReducerContext, context: &EventContext, action: &RuleAction) -> ActionResult {
    let entities = ctx.db.game_world_entity();
    let Some(summoner) = entities.guid().find(context.creature_guid) else {
        return ActionResult::Refused;
    };
    let Some(location) = ctx.db.game_creature_ai_summon().id().find(action.params[2]) else {
        return ActionResult::Refused;
    };
    let Some(template) = ctx
        .db
        .game_creature_template()
        .entry()
        .find(action.params[0])
    else {
        return ActionResult::Refused;
    };
    if ![location.x, location.y, location.z, location.orientation]
        .into_iter()
        .all(f32::is_finite)
    {
        return ActionResult::Refused;
    }

    let selected_target = super::combat::target(ctx, context, action);
    let now_ms = timestamp_ms(ctx);
    let expiry = ctx
        .db
        .game_creature_ai_summon_expiry()
        .insert(CreatureAiSummonExpiry {
            scheduled_id: 0,
            scheduled_at: schedule_after(ctx, next_check_ms(location.lifetime_ms)),
            creature_guid: 0,
            lifetime_ms: location.lifetime_ms,
            remaining_ms: location.lifetime_ms,
            last_checked_ms: now_ms,
        });
    let Some(guid) = summon_guid(action.params[0], expiry.scheduled_id) else {
        ctx.db
            .game_creature_ai_summon_expiry()
            .scheduled_id()
            .delete(expiry.scheduled_id);
        return ActionResult::Refused;
    };
    if entities.guid().find(guid).is_some() {
        ctx.db
            .game_creature_ai_summon_expiry()
            .scheduled_id()
            .delete(expiry.scheduled_id);
        return ActionResult::Refused;
    }
    let expiry_table = ctx.db.game_creature_ai_summon_expiry();
    let mut expiry = expiry;
    expiry.creature_guid = guid;
    expiry_table.scheduled_id().update(expiry);

    let spawn = crate::creatures::CreatureSpawn {
        guid,
        entry: action.params[0],
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

    if action.target != TargetPolicy::SelfActor {
        if let Some(target_guid) = selected_target {
            engage_summon(ctx, guid, target_guid);
        }
    }
    ActionResult::Applied
}

fn engage_summon(ctx: &ReducerContext, creature_guid: u64, target_guid: u64) {
    if crate::combat::apply_start_attack(ctx, creature_guid, target_guid).is_err() {
        return;
    }
    let entities = ctx.db.game_world_entity();
    if let Some(mut creature) = entities.guid().find(creature_guid) {
        creature.target_guid = target_guid;
        entities.guid().update(creature);
    }
    crate::hooks::fire_on_aggro(
        ctx,
        &crate::hooks::AggroPayload {
            creature_guid,
            target_guid,
            assist: true,
        },
    );
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
    let remaining_ms = if engaged {
        expiry.lifetime_ms
    } else {
        expiry.remaining_ms.saturating_sub(elapsed_ms)
    };
    if !engaged && remaining_ms == 0 {
        despawn_temporary_summon(ctx, expiry.creature_guid);
        return;
    }
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
