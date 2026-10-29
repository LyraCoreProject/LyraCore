use lyracore_shared::constants::unit_flags;
use spacetimedb::{table, ReducerContext, Table};

use super::tick;
use crate::{game_creature_spawn, game_world_entity};

#[table(accessor = game_creature_distraction)]
pub struct CreatureDistraction {
    #[primary_key]
    pub creature_guid: u64,
    pub ends_ms: u64,
}

fn length_ms(seconds: i32) -> Option<u64> {
    u64::try_from(seconds)
        .ok()
        .filter(|&s| s > 0)
        .map(|s| s * 1_000)
}

fn now_ms(ctx: &ReducerContext) -> u64 {
    (crate::time::now(ctx).to_micros_since_unix_epoch() / 1_000) as u64
}

pub(crate) fn distract(
    ctx: &ReducerContext,
    creature_guid: u64,
    dest: (f32, f32, f32),
    seconds: i32,
) {
    let Some(length_ms) = length_ms(seconds) else {
        return;
    };
    let entities = ctx.db.game_world_entity();
    let Some(mut creature) = entities.guid().find(creature_guid) else {
        return;
    };
    if creature.is_player()
        || creature.dead
        || creature.unit_flags & unit_flags::IN_COMBAT != 0
        || crate::combat::is_engaged(ctx, creature_guid)
        || super::eventai::movement::returning_home(ctx, creature_guid)
        || crate::spell::is_action_blocked(ctx, creature_guid)
        || !crate::nav::has_los(
            ctx,
            creature.map_id,
            creature.instance_id,
            dest,
            (creature.x, creature.y, creature.z),
        )
    {
        return;
    }
    tick::stop_facing(ctx, &mut creature, (dest.0, dest.1));
    creature.update(ctx);
    let row = CreatureDistraction {
        creature_guid,
        ends_ms: now_ms(ctx) + length_ms,
    };
    let table = ctx.db.game_creature_distraction();
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(row);
    } else {
        table.insert(row);
    }
}

fn ends_ms(ctx: &ReducerContext, guid: u64) -> Option<u64> {
    ctx.db
        .game_creature_distraction()
        .creature_guid()
        .find(guid)
        .map(|row| row.ends_ms)
}

pub(crate) fn active(ctx: &ReducerContext, guid: u64) -> bool {
    ends_ms(ctx, guid).is_some_and(|ends| now_ms(ctx) < ends)
}

fn expired(ctx: &ReducerContext, guid: u64) -> bool {
    ends_ms(ctx, guid).is_some_and(|ends| now_ms(ctx) >= ends)
}

pub(crate) fn expired_facing(ctx: &ReducerContext, guid: u64) -> Option<f32> {
    if !expired(ctx, guid) {
        return None;
    }
    ctx.db
        .game_creature_spawn()
        .guid()
        .find(guid)
        .map(|spawn| spawn.orientation)
        .or_else(|| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(guid)
                .map(|creature| creature.orientation)
        })
}

pub(crate) fn end_expired(ctx: &ReducerContext, guid: u64) {
    if expired(ctx, guid) {
        clear(ctx, guid);
    }
}

pub(crate) fn clear(ctx: &ReducerContext, guid: u64) {
    ctx.db
        .game_creature_distraction()
        .creature_guid()
        .delete(guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_effect_amount_is_the_length_in_seconds() {
        assert_eq!(length_ms(10), Some(10_000));
        assert_eq!(length_ms(0), None);
        assert_eq!(length_ms(-5), None);
    }
}
