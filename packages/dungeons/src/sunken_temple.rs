use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::encounter::{
    self, EncounterSignal, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
};
use crate::game_world_entity;

const MAP_ID: u32 = 109;
const ENCOUNTER_ID: u32 = 4;
const SHADE_OF_HAKKAR: u32 = 8440;
const SUPPRESSION: u32 = 12623;

#[table(
    accessor = sunken_temple_suppression,
    scheduled(expire_avatar_suppression),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct SunkenTempleSuppression {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
}

crate::encounter_package!(SunkenTempleAvatar, fn avatar(ctx, instance_id, signal) {
    match signal {
        EncounterSignal::Begin => {
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_IN_PROGRESS)
        }
        EncounterSignal::Fail => {
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_FAILED)
        }
        EncounterSignal::Complete => {
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_DONE)
        }
        EncounterSignal::InterruptAvatarSuppression => arm_suppression(ctx, instance_id),
        other => Err(format!("Avatar does not accept encounter signal {other:?}")),
    }
});

fn arm_suppression(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    if !ctx
        .db
        .sunken_temple_suppression()
        .by_instance()
        .filter(&instance_id)
        .any(|_| true)
    {
        let scheduled_at = ScheduleAt::Time(
            ctx.timestamp
                .checked_add(TimeDuration::from_micros(20_000_000))
                .unwrap_or(ctx.timestamp),
        );
        ctx.db
            .sunken_temple_suppression()
            .insert(SunkenTempleSuppression {
                scheduled_id: 0,
                scheduled_at,
                instance_id,
            });
    }
    Ok(())
}

#[reducer]
pub fn expire_avatar_suppression(ctx: &ReducerContext, timer: SunkenTempleSuppression) {
    if ctx.sender() != ctx.database_identity()
        || encounter::get_encounter_state(ctx, timer.instance_id, ENCOUNTER_ID)
            != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    let suppression_remains = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .find(|entity| {
            entity.instance_id == timer.instance_id
                && entity.entry == SHADE_OF_HAKKAR
                && !entity.dead
        })
        .is_some_and(|shade| crate::spell::has_aura(ctx, shade.guid, SUPPRESSION));
    if suppression_remains {
        let _ =
            encounter::set_encounter_state(ctx, timer.instance_id, ENCOUNTER_ID, ENCOUNTER_FAILED);
    }
}
