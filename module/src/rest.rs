use spacetimedb::{table, ReducerContext, Table, Timestamp};

use crate::world::WorldEntity;
use crate::{
    game_area, game_area_trigger, game_character, game_realm, game_tavern_area_trigger,
    game_world_entity, GameArea,
};
use lyracore_shared::{constants::player_flags, faction, packing};

const AREA_FLAG_CAPITAL: u32 = 0x0000_0100;
const AREA_TEAM_ALLIANCE: u32 = 2;
const AREA_TEAM_HORDE: u32 = 4;
const AREA_TEAM_NONE: u32 = 0;

pub(crate) fn rest_state_after_bonus(previous: u8, bonus: u32, level: u8) -> u8 {
    if level >= 60 || bonus <= 1 {
        packing::REST_STATE_NORMAL
    } else if bonus > 10 {
        packing::REST_STATE_RESTED
    } else if previous == packing::REST_STATE_RESTED {
        packing::REST_STATE_RESTED
    } else {
        packing::REST_STATE_NORMAL
    }
}

pub(crate) fn set_rest_state_byte(mover: &mut WorldEntity, rest_state: u8) {
    mover.player_bytes_2 = (mover.player_bytes_2 & 0x00FF_FFFF) | ((rest_state as u32) << 24);
}

fn in_rest_area(ctx: &ReducerContext, mover: &WorldEntity, area: Option<&GameArea>) -> bool {
    if ctx.db.game_tavern_area_trigger().iter().any(|tavern| {
        ctx.db
            .game_area_trigger()
            .id()
            .find(tavern.trigger_id)
            .is_some_and(|trigger| {
                trigger.map_id == mover.map_id
                    && crate::quest::AreaTriggerVolume::of(&trigger).contains(
                        [trigger.x, trigger.y, trigger.z],
                        [mover.x, mover.y, mover.z],
                    )
            })
    }) {
        return true;
    }
    let Some(area) = area else {
        return false;
    };
    let Some(zone) = ctx.db.game_area().id().find(crate::terrain::zone_of(area)) else {
        return false;
    };
    if zone.flags & AREA_FLAG_CAPITAL == 0 {
        return false;
    }
    match zone.faction_group {
        AREA_TEAM_ALLIANCE => faction::team_for_race(mover.race()) == faction::TEAM_ALLIANCE,
        AREA_TEAM_HORDE => faction::team_for_race(mover.race()) == faction::TEAM_HORDE,
        AREA_TEAM_NONE => !ctx
            .db
            .game_realm()
            .iter()
            .any(|realm| matches!(realm.realm_type, 1 | 8 | 16)),
        _ => true,
    }
}

#[table(accessor = game_rest_state_event, public)]
pub struct RestStateEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub player_bytes_2: u32,
    pub created_at: Timestamp,
}

crate::character_owned!(delete, fn sweep_delete_game_rest_state_event(ctx, character_guid) {
    let ev = ctx.db.game_rest_state_event();
    for r in ev.iter().filter(|r| r.character_guid == character_guid).collect::<Vec<_>>() {
        ev.id().delete(r.id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_rest_state_event());

pub(crate) fn check_rest_state(
    ctx: &ReducerContext,
    mover: &mut WorldEntity,
    area: Option<&GameArea>,
) {
    let now_resting = in_rest_area(ctx, mover, area);
    if now_resting {
        mover.player_flags |= player_flags::RESTING;
    } else {
        mover.player_flags &= !player_flags::RESTING;
    }
    if now_resting == mover.resting {
        return;
    }
    mover.resting = now_resting;

    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(mover.guid) {
        c.resting = now_resting;
        if now_resting {
            c.rested_since_micros = now as u64;
        } else {
            let elapsed = (now as u64).saturating_sub(c.rested_since_micros);
            c.rested_xp = crate::xp::rest_accrue_live(c.rested_xp, elapsed, c.level as u32);
            c.rested_since_micros = 0;
        }
        c.rest_state = rest_state_after_bonus(c.rest_state, c.rested_xp, c.level);
        set_rest_state_byte(mover, c.rest_state);
        chars.guid().update(c);
    }

    ctx.db.game_rest_state_event().insert(RestStateEvent {
        id: 0,
        character_guid: mover.guid,
        player_bytes_2: mover.player_bytes_2,
        created_at: crate::time::now(ctx),
    });
}

pub(crate) fn materialize_on_logout(c: &mut crate::character::Character, now_micros: i64) {
    if c.rested_since_micros == 0 {
        return;
    }
    let elapsed = (now_micros as u64).saturating_sub(c.rested_since_micros);
    c.rested_xp = crate::xp::rest_accrue_live(c.rested_xp, elapsed, c.level as u32);
    c.rest_state = rest_state_after_bonus(c.rest_state, c.rested_xp, c.level);
    c.rested_since_micros = 0;
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_check_rest_at(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let mut e = crate::helpers::live_entity(ctx, guid)?;
    e.map_id = map_id;
    e.x = x;
    e.y = y;
    let area = crate::terrain::area_at(ctx, e.map_id, e.x, e.y);
    check_rest_state(ctx, &mut e, area.as_ref());
    e.update(ctx);
    Ok(())
}

crate::game_tick_pass!(fn rested_accrue_pass(ctx) {
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
    const QUANTUM_MICROS: i64 = 30_000_000;
    const TICK_MICROS: i64 = 500_000;
    if now.rem_euclid(QUANTUM_MICROS) >= TICK_MICROS {
        return;
    }
    let chars = ctx.db.game_character();
    let resting: Vec<crate::character::Character> = chars
        .iter()
        .filter(|c| c.resting && c.rested_since_micros > 0)
        .filter(|c| !crate::transfer::is_in_transit(ctx, c.guid))
        .collect();
    for mut c in resting {
        let elapsed = (now as u64).saturating_sub(c.rested_since_micros);
        let grown = crate::xp::rest_accrue_live(c.rested_xp, elapsed, c.level as u32);
        if grown > c.rested_xp {
            c.rested_xp = grown;
            c.rested_since_micros = now as u64;
            let previous = c.rest_state;
            c.rest_state = rest_state_after_bonus(previous, grown, c.level);
            if c.rest_state != previous {
                if let Some(mut entity) = ctx.db.game_world_entity().guid().find(c.guid) {
                    set_rest_state_byte(&mut entity, c.rest_state);
                    entity.update(ctx);
                }
            }
            chars.guid().update(c);
        }
    }
});

#[cfg(test)]
mod tests {
    use super::rest_state_after_bonus;
    use lyracore_shared::packing::{REST_STATE_NORMAL, REST_STATE_RESTED};

    #[test]
    fn rest_state_follows_bonus_thresholds_and_holds_between_them() {
        assert_eq!(
            rest_state_after_bonus(REST_STATE_NORMAL, 11, 1),
            REST_STATE_RESTED
        );
        assert_eq!(
            rest_state_after_bonus(REST_STATE_RESTED, 10, 1),
            REST_STATE_RESTED
        );
        assert_eq!(
            rest_state_after_bonus(REST_STATE_RESTED, 2, 1),
            REST_STATE_RESTED
        );
        assert_eq!(
            rest_state_after_bonus(REST_STATE_RESTED, 1, 1),
            REST_STATE_NORMAL
        );
        assert_eq!(
            rest_state_after_bonus(REST_STATE_NORMAL, 10, 1),
            REST_STATE_NORMAL
        );
        assert_eq!(rest_state_after_bonus(0, 5, 1), REST_STATE_NORMAL);
        assert_eq!(
            rest_state_after_bonus(REST_STATE_RESTED, 100, 60),
            REST_STATE_NORMAL
        );
    }
}
