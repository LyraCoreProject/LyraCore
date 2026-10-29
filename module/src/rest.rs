use spacetimedb::{table, ReducerContext, Table, Timestamp};

use crate::game_character;
use crate::world::WorldEntity;
use lyracore_shared::packing;

struct RestTrigger {
    map_id: u32,
    x: f32,
    y: f32,
    radius: f32,
}

const REST_TRIGGERS: &[RestTrigger] = &[
    // The Goldshire inn threshold is a fixture estimate.
    RestTrigger {
        map_id: 0,
        x: -9464.0,
        y: 42.0,
        radius: 20.0,
    },
];

fn in_rest_area(map_id: u32, x: f32, y: f32) -> bool {
    REST_TRIGGERS.iter().any(|t| {
        t.map_id == map_id && {
            let (dx, dy) = (x - t.x, y - t.y);
            dx * dx + dy * dy <= t.radius * t.radius
        }
    })
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

pub(crate) fn check_rest_state(ctx: &ReducerContext, mover: &mut WorldEntity) {
    let now_resting = in_rest_area(mover.map_id, mover.x, mover.y);
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
        chars.guid().update(c);
    }

    let rest_byte = if now_resting {
        packing::REST_STATE_RESTED
    } else {
        packing::REST_STATE_NORMAL
    } as u32;
    mover.player_bytes_2 = (mover.player_bytes_2 & 0x00FF_FFFF) | (rest_byte << 24);

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
    check_rest_state(ctx, &mut e);
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
            chars.guid().update(c);
        }
    }
});
