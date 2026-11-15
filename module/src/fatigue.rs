use spacetimedb::{table, ReducerContext, Table, Timestamp};

use crate::breath::{advance_mirror_timer, MirrorTimerAdvance};
use crate::{game_world_entity, world::WorldEntity};
use lyracore_shared::constants::player_flags;

const FULL_FATIGUE_MICROS: i64 = 60_000_000;
const FATIGUE_RECOVERY_MICROS: i64 = FULL_FATIGUE_MICROS / 10;

fn advance_fatigue(
    remaining_micros: i64,
    in_deep_water: bool,
    elapsed_micros: i64,
) -> MirrorTimerAdvance {
    advance_mirror_timer(
        remaining_micros,
        FULL_FATIGUE_MICROS,
        FATIGUE_RECOVERY_MICROS,
        in_deep_water,
        elapsed_micros,
    )
}

fn exhaustion_damage(max_health: u32, level: u32, roll: u32) -> u32 {
    (max_health / 5).saturating_add(roll % level.max(1))
}

#[table(accessor = game_fatigue_state)]
pub struct FatigueState {
    #[primary_key]
    pub entity_guid: u64,
    pub remaining_micros: i64,
    pub last_evaluated: Timestamp,
    pub in_deep_water: bool,
}

crate::outside_table_catalog!(game_fatigue_state);

crate::character_owned!(delete, fn sweep_delete_game_fatigue_state(ctx, character_guid) {
    ctx.db.game_fatigue_state().entity_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_fatigue_state());

const FULL_FATIGUE_MS: u32 = (FULL_FATIGUE_MICROS / 1000) as u32;

pub(crate) fn feels_fatigue(entity: &WorldEntity) -> bool {
    entity.is_player()
        && !entity.godmode
        && (!entity.dead || entity.player_flags & player_flags::GHOST != 0)
}

// Exhaustion is left to the tick: the caller still holds the mover and writes it back.
pub(crate) fn update_fatigue_edge(ctx: &ReducerContext, mover: &WorldEntity, in_deep_water: bool) {
    let states = ctx.db.game_fatigue_state();
    let now = crate::time::now(ctx);
    let Some(mut state) = states.entity_guid().find(mover.guid) else {
        if in_deep_water {
            states.insert(FatigueState {
                entity_guid: mover.guid,
                remaining_micros: FULL_FATIGUE_MICROS,
                last_evaluated: now,
                in_deep_water: true,
            });
            crate::breath_relay::fatigue(ctx, mover.guid, true, FULL_FATIGUE_MS, FULL_FATIGUE_MS);
        }
        return;
    };
    if state.in_deep_water == in_deep_water {
        return;
    }
    let elapsed =
        now.to_micros_since_unix_epoch() - state.last_evaluated.to_micros_since_unix_epoch();
    state.remaining_micros =
        advance_fatigue(state.remaining_micros, state.in_deep_water, elapsed).remaining_micros;
    state.last_evaluated = now;
    state.in_deep_water = in_deep_water;
    let remaining_ms = (state.remaining_micros / 1000) as u32;
    states.entity_guid().update(state);
    crate::breath_relay::fatigue(
        ctx,
        mover.guid,
        in_deep_water,
        remaining_ms,
        FULL_FATIGUE_MS,
    );
}

pub(crate) fn tick_fatigue(ctx: &ReducerContext) {
    let now = crate::time::now(ctx);
    let states = ctx.db.game_fatigue_state();
    for mut state in states.iter().collect::<Vec<_>>() {
        let guid = state.entity_guid;
        let Some(entity) = ctx.db.game_world_entity().guid().find(guid) else {
            states.entity_guid().delete(guid);
            continue;
        };
        if !feels_fatigue(&entity) {
            states.entity_guid().delete(guid);
            crate::breath_relay::fatigue_stop(ctx, guid);
            continue;
        }
        let draining = state.in_deep_water && !crate::taxi::is_in_flight(ctx, guid);
        let elapsed =
            now.to_micros_since_unix_epoch() - state.last_evaluated.to_micros_since_unix_epoch();
        let advanced = advance_fatigue(state.remaining_micros, draining, elapsed);
        state.remaining_micros = advanced.remaining_micros;
        state.last_evaluated = now;
        let ended = advanced.expired_ticks > 0 && exhaust(ctx, entity, advanced.expired_ticks);
        if ended || (!state.in_deep_water && state.remaining_micros == FULL_FATIGUE_MICROS) {
            states.entity_guid().delete(guid);
            crate::breath_relay::fatigue_stop(ctx, guid);
            continue;
        }
        states.entity_guid().update(state);
    }
}

// Returns true when exhaustion ends the timer: a death, or a ghost returned to a graveyard.
fn exhaust(ctx: &ReducerContext, mut entity: WorldEntity, ticks: u32) -> bool {
    let guid = entity.guid;
    if entity.dead {
        let graveyard = crate::graveyard::resolve_graveyard(
            ctx,
            entity.map_id,
            entity.x,
            entity.y,
            entity.race(),
        );
        let pose = [graveyard.x, graveyard.y, graveyard.z, graveyard.o];
        if let Err(refusal) = crate::world::teleport(ctx, guid, graveyard.map, 0, pose) {
            spacetimedb::log::warn!("fatigue: ghost {guid} graveyard return refused: {refusal}");
        }
        return true;
    }
    let start_health = entity.health;
    for _ in 0..ticks {
        let nominal = exhaustion_damage(entity.max_health, entity.level, ctx.random::<u32>());
        let (health, lethal) = crate::world::resolve_environmental_damage(nominal, entity.health);
        let dealt = entity.health - health;
        if lethal {
            let remaining_health = entity.health;
            if remaining_health != start_health {
                entity.update(ctx);
            }
            crate::combat::kill_player(ctx, guid, guid);
            crate::breath_relay::exhaustion_damage(ctx, guid, remaining_health);
            return true;
        }
        entity.health = health;
        if dealt > 0 {
            crate::breath_relay::exhaustion_damage(ctx, guid, dealt);
        }
    }
    if entity.health != start_health {
        entity.update(ctx);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatigue_drains_for_a_minute_then_exhausts_each_second() {
        assert_eq!(
            advance_fatigue(FULL_FATIGUE_MICROS, true, 59_000_000),
            MirrorTimerAdvance {
                remaining_micros: 1_000_000,
                expired_ticks: 0
            }
        );
        assert_eq!(
            advance_fatigue(1_000_000, true, 3_000_000),
            MirrorTimerAdvance {
                remaining_micros: 0,
                expired_ticks: 3
            }
        );
    }

    #[test]
    fn fatigue_recovers_in_six_seconds_outside_deep_water() {
        assert_eq!(
            advance_fatigue(0, false, 3_000_000).remaining_micros,
            30_000_000
        );
        assert_eq!(
            advance_fatigue(0, false, 6_000_000).remaining_micros,
            FULL_FATIGUE_MICROS
        );
    }

    #[test]
    fn exhaustion_damage_is_a_fifth_of_health_plus_a_level_roll() {
        assert_eq!(exhaustion_damage(1000, 60, 0), 200);
        assert_eq!(exhaustion_damage(1000, 60, 59), 259);
        assert_eq!(exhaustion_damage(1000, 60, 60), 200);
        assert_eq!(exhaustion_damage(1000, 1, 7), 200);
    }
}
