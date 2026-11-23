//! Read docs/schema.md "Drunk value" before changing how a Character gets drunk or sobers.

use spacetimedb::{table, ReducerContext, Table, TimeDuration, Timestamp};

use crate::{game_world_entity, world::WorldEntity};

// PLAYER_BYTES_3 bits 1 to 15; bit 0 is gender.
const DRUNK_VALUE_MASK: u32 = 0xFFFE;
const INEBRIATION_PER_POINT: u32 = 256;
const SOBERING_STEP: u32 = 256;
const SOBERING_INTERVAL_MICROS: i64 = 10_000_000;
const OFFLINE_SOBERING_MICROS: u64 = 15 * 60 * 1_000_000;

#[table(accessor = game_drunk_sobering)]
pub struct DrunkSobering {
    #[primary_key]
    pub character_guid: u64,
    pub next_step_at: Timestamp,
}

crate::character_owned!(delete, fn sweep_delete_game_drunk_sobering(ctx, character_guid) {
    ctx.db.game_drunk_sobering().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_drunk_sobering());

pub(crate) fn player_bytes_3(gender: u8, drunk_value: u16) -> u32 {
    u32::from(gender) | (u32::from(drunk_value) & DRUNK_VALUE_MASK)
}

pub(crate) fn drunk_value(entity: &WorldEntity) -> u16 {
    (entity.player_bytes_3 & DRUNK_VALUE_MASK) as u16
}

fn set_drunk_value(entity: &mut WorldEntity, value: u32) {
    entity.player_bytes_3 = (entity.player_bytes_3 & !DRUNK_VALUE_MASK)
        | (value.min(u32::from(u16::MAX)) & DRUNK_VALUE_MASK);
}

pub(crate) fn sober_on_death(entity: &mut WorldEntity) {
    set_drunk_value(entity, 0);
}

/// vmangos `Player::LoadFromDB`: linear to zero over 15 offline minutes; no logout time keeps it.
pub(crate) fn sobered_offline(drunk_value: u16, last_logout_micros: u64, now_micros: i64) -> u16 {
    if last_logout_micros == 0 {
        return drunk_value;
    }
    let offline = (now_micros.max(0) as u64).saturating_sub(last_logout_micros);
    let remaining = OFFLINE_SOBERING_MICROS.saturating_sub(offline);
    (u64::from(drunk_value) * remaining / OFFLINE_SOBERING_MICROS) as u16
}

/// vmangos `Spell::EffectInebriate`: each effect point adds 256, up to 65534.
pub(crate) fn inebriate(ctx: &ReducerContext, target_guid: u64, points: i32) {
    let Some(mut entity) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .filter(WorldEntity::is_player)
    else {
        return;
    };
    let added = points.clamp(0, 255) as u32 * INEBRIATION_PER_POINT;
    let raised = u32::from(drunk_value(&entity)) + added;
    set_drunk_value(&mut entity, raised);
    let drunk = drunk_value(&entity) != 0;
    entity.update(ctx);
    if drunk {
        start_sobering(ctx, target_guid);
    }
}

/// Starts the 10-second sobering timer unless one already runs, as vmangos `m_drunkTimer` does.
pub(crate) fn start_sobering(ctx: &ReducerContext, character_guid: u64) {
    let timers = ctx.db.game_drunk_sobering();
    if timers.character_guid().find(character_guid).is_none() {
        timers.insert(DrunkSobering {
            character_guid,
            next_step_at: crate::time::now(ctx)
                + TimeDuration::from_micros(SOBERING_INTERVAL_MICROS),
        });
    }
}

/// vmangos `Player::HandleSobering`: every 10 seconds a drunk Character loses 256.
pub(crate) fn tick_sobering(ctx: &ReducerContext) {
    let now = crate::time::now(ctx);
    let timers = ctx.db.game_drunk_sobering();
    for mut timer in timers.iter().collect::<Vec<_>>() {
        let guid = timer.character_guid;
        let Some(mut entity) = ctx.db.game_world_entity().guid().find(guid) else {
            // A cross-map WorldPort removes the entity until the client acks; keep the timer.
            if !crate::helpers::character_by_guid(ctx, guid).is_some_and(|c| c.online) {
                timers.character_guid().delete(guid);
            }
            continue;
        };
        let value = u32::from(drunk_value(&entity));
        if value == 0 {
            timers.character_guid().delete(guid);
            continue;
        }
        let overdue =
            now.to_micros_since_unix_epoch() - timer.next_step_at.to_micros_since_unix_epoch();
        if overdue < 0 {
            continue;
        }
        let steps = 1 + overdue / SOBERING_INTERVAL_MICROS;
        let sobered = value.saturating_sub(SOBERING_STEP.saturating_mul(steps as u32));
        set_drunk_value(&mut entity, sobered);
        entity.update(ctx);
        if sobered == 0 {
            timers.character_guid().delete(guid);
        } else {
            timer.next_step_at += TimeDuration::from_micros(steps * SOBERING_INTERVAL_MICROS);
            timers.character_guid().update(timer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE_MICROS: u64 = 60 * 1_000_000;

    #[test]
    fn offline_sobering_is_linear_over_fifteen_minutes() {
        let logout = 1_000 * MINUTE_MICROS;
        let at = |minutes: u64| (logout + minutes * MINUTE_MICROS) as i64;
        assert_eq!(sobered_offline(12_800, logout, at(0)), 12_800);
        assert_eq!(sobered_offline(12_000, logout, at(5)), 8_000);
        assert_eq!(sobered_offline(12_800, logout, at(15)), 0);
        assert_eq!(sobered_offline(12_800, logout, at(60)), 0);
    }

    #[test]
    fn an_unknown_logout_time_keeps_the_drunk_value() {
        assert_eq!(sobered_offline(23_040, 0, 1_000_000), 23_040);
    }

    #[test]
    fn player_bytes_3_keeps_gender_in_bit_zero() {
        assert_eq!(player_bytes_3(1, 12_800), 0x3201);
        assert_eq!(player_bytes_3(0, 0xFFFF), 0xFFFE);
    }
}
