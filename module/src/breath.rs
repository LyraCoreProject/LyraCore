//! Player breath: a tiny non-spatial timer for underwater players. The movement edge only arms or
//! flips rows; this scheduled reducer advances them so a motionless player still drowns.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_world_entity, world::WorldEntity};
use lyracore_shared::env::{advance_breath, drowning_damage, FULL_AIR_MICROS};

// Vanilla's `Player::HandleDrowning` gives a player about one minute of air. Surfacing recovers
// the same bar over ten seconds; the slower recovery prevents brief surface bobs from resetting it.

/// One active breath timer. This is deliberately non-spatial: the scheduled tick must iterate all
/// rows, and spatial iteration would silently omit players outside the caller's subscribed slice.
/// A row exists only while the player is draining or recovering a partial bar.
#[table(accessor = game_breath_state)]
pub struct BreathState {
    #[primary_key]
    pub entity_guid: u64,
    pub remaining_air_micros: i64,
    pub last_evaluated: Timestamp,
    pub draining: bool,
}

// Breath is transient live-world state, not character progression. Delete it with the character;
// never transport it so an arrival starts with a fresh bar instead of replaying stale water state.
crate::character_owned!(delete, fn sweep_delete_game_breath_state(ctx, character_guid) {
    ctx.db.game_breath_state().entity_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_breath_state());

/// Drives breath independently of movement packets. [server]
#[table(accessor = game_breath_schedule, scheduled(tick_breath))]
pub struct BreathSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// Fold one movement edge into the state table. Callers have already passed the godmode/dead/ghost
/// exemption and supplied the authoritative underwater predicate.
pub(crate) fn update_breath_edge(ctx: &ReducerContext, mover: &WorldEntity, submerged: bool) {
    let states = ctx.db.game_breath_state();
    let now = ctx.timestamp;
    let now_micros = now.to_micros_since_unix_epoch();
    match states.entity_guid().find(mover.guid) {
        None if submerged => {
            states.insert(BreathState {
                entity_guid: mover.guid,
                remaining_air_micros: FULL_AIR_MICROS,
                last_evaluated: now,
                draining: true,
            });
            crate::breath_relay::start(
                ctx,
                mover.guid,
                (FULL_AIR_MICROS / 1000) as u32,
                (FULL_AIR_MICROS / 1000) as u32,
            );
        }
        None => {}
        Some(mut state) => {
            let elapsed = now_micros - state.last_evaluated.to_micros_since_unix_epoch();
            let advanced = advance_breath(state.remaining_air_micros, state.draining, elapsed);
            state.remaining_air_micros = advanced.remaining_air_micros;
            state.last_evaluated = now;
            if state.draining != submerged {
                if submerged {
                    crate::breath_relay::start(
                        ctx,
                        mover.guid,
                        (state.remaining_air_micros / 1000) as u32,
                        (FULL_AIR_MICROS / 1000) as u32,
                    );
                } else {
                    crate::breath_relay::stop(ctx, mover.guid);
                }
            }
            state.draining = submerged;
            if !submerged && state.remaining_air_micros == FULL_AIR_MICROS {
                states.entity_guid().delete(mover.guid);
            } else {
                states.entity_guid().update(state);
            }
        }
    }
}

#[reducer]
pub fn tick_breath(ctx: &ReducerContext, _schedule: BreathSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let now = ctx.timestamp;
    let now_micros = now.to_micros_since_unix_epoch();
    let states = ctx.db.game_breath_state();
    let active: Vec<BreathState> = states.iter().collect();
    for mut state in active {
        let entity_guid = state.entity_guid;
        let Some(entity) = ctx.db.game_world_entity().guid().find(entity_guid) else {
            states.entity_guid().delete(entity_guid);
            continue;
        };
        // A dead player, ghost, or godmode mover must never retain a draining/refilling timer.
        if entity.dead
            || entity.godmode
            || !entity.is_player()
            || entity.player_flags & lyracore_shared::constants::player_flags::GHOST != 0
        {
            states.entity_guid().delete(entity_guid);
            continue;
        }
        let elapsed = now_micros - state.last_evaluated.to_micros_since_unix_epoch();
        let advanced = advance_breath(state.remaining_air_micros, state.draining, elapsed);
        state.remaining_air_micros = advanced.remaining_air_micros;
        state.last_evaluated = now;

        if !state.draining && state.remaining_air_micros == FULL_AIR_MICROS {
            states.entity_guid().delete(entity_guid);
            continue;
        }
        states.entity_guid().update(state);

        if advanced.drowning_ticks == 0 {
            continue;
        }
        // Preserve one one-second hit (and one relay line) per catch-up tick. `kill_player` owns
        // the lethal write, so the last line reports the health actually removed rather than an
        // overkill nominal value.
        let mut health = entity.health;
        let mut died = false;
        for _ in 0..advanced.drowning_ticks {
            let nominal = drowning_damage(entity.max_health);
            let actual = nominal.min(health);
            let (_, lethal) = crate::world::resolve_environmental_damage(nominal, health);
            if lethal {
                // Self/environmental killer: no player receives kill credit or loot.
                if health != entity.health {
                    if let Some(mut pre_death) = ctx.db.game_world_entity().guid().find(entity_guid)
                    {
                        pre_death.health = health;
                        ctx.db.game_world_entity().guid().update(pre_death);
                    }
                }
                crate::combat::kill_player(ctx, entity_guid, entity_guid);
                crate::breath_relay::drowning_damage(ctx, entity_guid, actual);
                states.entity_guid().delete(entity_guid);
                died = true;
                break;
            }
            if actual > 0 {
                health -= actual;
                crate::breath_relay::drowning_damage(ctx, entity_guid, actual);
            }
        }
        if !died && health != entity.health {
            let mut hurt = entity;
            hurt.health = health;
            ctx.db.game_world_entity().guid().update(hurt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::env::{BreathAdvance, MAX_DROWNING_CATCHUP_TICKS};

    #[test]
    fn breath_drains_then_crosses_zero_once() {
        assert_eq!(
            advance_breath(FULL_AIR_MICROS, true, 12_000_000),
            BreathAdvance {
                remaining_air_micros: 48_000_000,
                drowning_ticks: 0
            }
        );
        assert_eq!(
            advance_breath(200_000, true, 300_000),
            BreathAdvance {
                remaining_air_micros: 0,
                drowning_ticks: 1
            }
        );
    }

    #[test]
    fn long_stall_has_bounded_one_second_damage_ticks() {
        assert_eq!(
            advance_breath(0, true, 20_000_000),
            BreathAdvance {
                remaining_air_micros: 0,
                drowning_ticks: MAX_DROWNING_CATCHUP_TICKS
            }
        );
    }

    #[test]
    fn empty_bar_counts_one_tick_per_elapsed_second() {
        assert_eq!(
            advance_breath(0, true, 1_000_000),
            BreathAdvance {
                remaining_air_micros: 0,
                drowning_ticks: 1
            }
        );
    }

    #[test]
    fn refill_clamps_to_full() {
        assert_eq!(
            advance_breath(54_000_000, false, 2_000_000),
            BreathAdvance {
                remaining_air_micros: FULL_AIR_MICROS,
                drowning_ticks: 0
            }
        );
    }

    #[test]
    fn drowning_damage_is_fixed_fraction_and_capped_by_caller() {
        assert_eq!(drowning_damage(100), 20);
        assert_eq!(drowning_damage(101), 21);
    }

    #[test]
    fn scheduled_tick_never_scans_the_spatial_entity_table() {
        let tick = crate::test_scan::code_of(include_str!("breath.rs"), "pub fn tick_breath(");
        assert!(tick.contains("let states = ctx.db.game_breath_state();"));
        assert!(tick.contains("states.iter()"));
        assert!(
            !tick.contains("game_world_entity().iter()"),
            "breath must iterate only its dedicated non-spatial state table"
        );
    }

    #[test]
    fn lethal_drowning_routes_through_the_shared_player_death_path() {
        let tick = crate::test_scan::code_of(include_str!("breath.rs"), "pub fn tick_breath(");
        assert!(tick.contains("crate::combat::kill_player(ctx, entity_guid, entity_guid)"));
        assert!(
            !tick.contains("hurt.health = 0"),
            "drowning must never write a lethal health value inline"
        );
    }
}
