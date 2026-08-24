use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::encounter::{
    self, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
};
use crate::{game_creature_template, game_gameobject, game_world_entity};

const MAP_ID: u32 = 33;
const FENRUS: u32 = 4274;
const ARCHMAGE_ARUGAL: u32 = 4275;
const ARUGAL_DOOR: u32 = 18971;
const SUMMON_LIFETIME_MS: u32 = 30_000;
const SUMMON_LOW_BAND: u64 = 0x20_0000;

#[table(accessor = shadowfang_summon_expiry, scheduled(expire_shadowfang_summon))]
pub struct ShadowfangSummonExpiry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub creature_guid: u64,
}

crate::encounter_package!(ShadowfangKeepRethilgore, fn rethilgore(ctx, instance_id, signal) {
    if signal == EncounterSignal::Complete {
        speak_rethilgore_outcome(ctx, instance_id);
    }
    set_standard_state(ctx, instance_id, 2, signal, "Rethilgore")
});

crate::encounter_package!(ShadowfangKeepFenrus, fn fenrus(ctx, instance_id, signal) {
    if signal == EncounterSignal::Complete {
        summon_archmage_arugal(ctx, instance_id);
    }
    set_standard_state(ctx, instance_id, 3, signal, "Fenrus")
});

crate::encounter_package!(ShadowfangKeepNandos, fn nandos(ctx, instance_id, signal) {
    if signal == EncounterSignal::Complete {
        open_arugal_door(ctx, instance_id);
    }
    set_standard_state(ctx, instance_id, 4, signal, "Nandos")
});

fn set_standard_state(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    signal: EncounterSignal,
    name: &str,
) -> Result<(), String> {
    let state = match signal {
        EncounterSignal::Begin => ENCOUNTER_IN_PROGRESS,
        EncounterSignal::Fail => ENCOUNTER_FAILED,
        EncounterSignal::Complete => ENCOUNTER_DONE,
        other => return Err(format!("{name} does not accept encounter signal {other:?}")),
    };
    encounter::set_encounter_state(ctx, instance_id, encounter_id, state)
}

fn open_arugal_door(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    let gameobjects = ctx.db.game_gameobject();
    let guids: Vec<u64> = gameobjects
        .by_map()
        .filter(&MAP_ID)
        .filter(|go| go.instance_id == instance_id && go.template_entry == ARUGAL_DOOR)
        .map(|go| go.guid)
        .collect();
    for guid in guids {
        if let Some(mut door) = gameobjects.guid().find(guid) {
            door.state = DOOR_OPEN_STATE;
            gameobjects.guid().update(door);
        }
    }
}

fn speak_rethilgore_outcome(ctx: &ReducerContext, instance_id: u64) {
    for (entry, text) in [
        (3849, "About time someone killed the wretch."),
        (3850, "For once I agree with you... scum."),
    ] {
        if let Some(speaker) = ctx
            .db
            .game_world_entity()
            .by_map()
            .filter(&MAP_ID)
            .find(|entity| {
                entity.instance_id == instance_id && entity.entry == entry && !entity.dead
            })
        {
            let _ = crate::chat::apply_send_chat(
                ctx,
                speaker,
                crate::chat::CHAT_SAY,
                0,
                text.to_string(),
            );
        }
    }
}

fn summon_archmage_arugal(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    let fenrus_exists = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .any(|entity| entity.instance_id == instance_id && entity.entry == FENRUS);
    if !fenrus_exists {
        return;
    }
    let Some(template) = ctx
        .db
        .game_creature_template()
        .entry()
        .find(ARCHMAGE_ARUGAL)
    else {
        return;
    };
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(
                i64::from(SUMMON_LIFETIME_MS) * 1_000,
            ))
            .unwrap_or(ctx.timestamp),
    );
    let expiry = ctx
        .db
        .shadowfang_summon_expiry()
        .insert(ShadowfangSummonExpiry {
            scheduled_id: 0,
            scheduled_at,
            creature_guid: 0,
        });
    let guid = encounter::wave_guid(
        ARCHMAGE_ARUGAL,
        SUMMON_LOW_BAND | (expiry.scheduled_id.saturating_sub(1) % (SUMMON_LOW_BAND - 1) + 1),
    );
    let mut expiry = expiry;
    expiry.creature_guid = guid;
    ctx.db
        .shadowfang_summon_expiry()
        .scheduled_id()
        .update(expiry);
    let spawn = crate::CreatureSpawn {
        guid,
        entry: ARCHMAGE_ARUGAL,
        map_id: MAP_ID,
        x: -136.89,
        y: 2169.17,
        z: 136.58,
        orientation: 2.794,
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: u32::MAX,
    };
    let entity =
        crate::creatures::build_creature_entity(&spawn, &template, ctx.random(), instance_id);
    crate::creatures::insert_creature_entity(ctx, entity);
}

#[reducer]
pub fn expire_shadowfang_summon(ctx: &ReducerContext, expiry: ShadowfangSummonExpiry) {
    if ctx.sender() == ctx.database_identity() {
        crate::creatures::despawn_creature_entity(ctx, expiry.creature_guid);
    }
}
