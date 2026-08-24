use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::encounter::{
    self, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
};
use crate::{
    game_creature_template, game_encounter_spawn, game_gameobject, game_instance, game_world_entity,
};

const MAP_ID: u32 = 33;
const FENRUS: u32 = 4274;
const ARCHMAGE_ARUGAL: u32 = 4275;
const ARUGAL_FIRE: u32 = 6422;
const ARUGAL_DOOR: u32 = 18971;
const SORCERER_DOOR: u32 = 18972;
const ARUGAL_FOCUS: u32 = 18973;
const ARUGAL_VOIDWALKER: u32 = 4627;
const SUMMON_LOW_BAND: u64 = 0x20_0000;
const IMMUNE_TO_PLAYERS: u32 = 0x0000_0100;
const IMMUNE_TO_CREATURES: u32 = 0x0000_0200;

const STEP_SHOW_AND_YELL: u8 = 0;
const STEP_FIRE: u8 = 1;
const STEP_LIGHTNING: u8 = 2;
const STEP_INVISIBILITY: u8 = 3;
const STEP_VOIDWALKERS: u8 = 4;

#[table(
    accessor = shadowfang_fenrus_choreography,
    scheduled(advance_fenrus_choreography),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct ShadowfangFenrusChoreography {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
    pub arugal_guid: u64,
    pub step: u8,
}

crate::encounter_package!(ShadowfangKeepRethilgore, fn rethilgore(ctx, instance_id, signal) {
    if signal == EncounterSignal::Complete {
        speak_rethilgore_outcome(ctx, instance_id);
    }
    set_standard_state(ctx, instance_id, 2, signal, "Rethilgore")
});

crate::encounter_package!(ShadowfangKeepFenrus, fn fenrus(ctx, instance_id, signal) {
    set_standard_state(ctx, instance_id, 3, signal, "Fenrus")?;
    if signal == EncounterSignal::Complete {
        begin_fenrus_choreography(ctx, instance_id);
    }
    Ok(())
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
    set_gameobject_state(ctx, instance_id, ARUGAL_DOOR, DOOR_OPEN_STATE);
}

fn set_gameobject_state(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
    entry: u32,
    state: u8,
) {
    let gameobjects = ctx.db.game_gameobject();
    let guids: Vec<u64> = gameobjects
        .by_map()
        .filter(&MAP_ID)
        .filter(|go| go.instance_id == instance_id && go.template_entry == entry)
        .map(|go| go.guid)
        .collect();
    for guid in guids {
        if let Some(mut gameobject) = gameobjects.guid().find(guid) {
            gameobject.state = state;
            gameobject.respawn_at_micros = 0;
            gameobjects.guid().update(gameobject);
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

fn begin_fenrus_choreography(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    let fenrus_exists = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .any(|entity| entity.instance_id == instance_id && entity.entry == FENRUS);
    if !fenrus_exists {
        return;
    }
    clear_fenrus_choreography(ctx, instance_id);
    schedule_fenrus_step(ctx, instance_id, 0, STEP_SHOW_AND_YELL, 100_000);
}

fn spawn_archmage_arugal(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
    sequence: u64,
) -> Option<u64> {
    let Some(template) = ctx
        .db
        .game_creature_template()
        .entry()
        .find(ARCHMAGE_ARUGAL)
    else {
        spacetimedb::log::warn!("Fenrus choreography has no Archmage Arugal template");
        return None;
    };
    let guid = encounter::wave_guid(
        ARCHMAGE_ARUGAL,
        SUMMON_LOW_BAND | (sequence % (SUMMON_LOW_BAND - 1) + 1),
    );
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
    let mut entity =
        crate::creatures::build_creature_entity(&spawn, &template, ctx.random(), instance_id);
    entity.unit_flags |= IMMUNE_TO_PLAYERS | IMMUNE_TO_CREATURES;
    crate::creatures::insert_creature_entity(ctx, entity);
    Some(guid)
}

#[reducer]
pub fn advance_fenrus_choreography(
    ctx: &ReducerContext,
    choreography: ShadowfangFenrusChoreography,
) {
    if ctx.sender() != ctx.database_identity()
        || !instance_belongs_to_shadowfang(ctx, choreography.instance_id)
    {
        return;
    }
    match choreography.step {
        STEP_SHOW_AND_YELL => {
            let Some(arugal_guid) =
                spawn_archmage_arugal(ctx, choreography.instance_id, choreography.scheduled_id)
            else {
                return;
            };
            if let Some(arugal) = ctx.db.game_world_entity().guid().find(arugal_guid) {
                let _ = crate::chat::apply_send_chat(
                    ctx,
                    arugal,
                    crate::chat::CHAT_YELL,
                    0,
                    "Who dares interfere with the Sons of Arugal?".to_string(),
                );
            }
            schedule_fenrus_step(
                ctx,
                choreography.instance_id,
                arugal_guid,
                STEP_FIRE,
                2_000_000,
            );
        }
        STEP_FIRE => {
            if let Err(error) = crate::actor::cast_at(
                ctx,
                choreography.arugal_guid,
                ARUGAL_FIRE,
                choreography.arugal_guid,
            ) {
                spacetimedb::log::warn!("Archmage Arugal fire cast refused: {error}");
            }
            schedule_fenrus_step(
                ctx,
                choreography.instance_id,
                choreography.arugal_guid,
                STEP_LIGHTNING,
                5_000_000,
            );
        }
        STEP_LIGHTNING => {
            set_gameobject_state(ctx, choreography.instance_id, ARUGAL_FOCUS, DOOR_OPEN_STATE);
            schedule_fenrus_step(
                ctx,
                choreography.instance_id,
                choreography.arugal_guid,
                STEP_INVISIBILITY,
                5_000_000,
            );
        }
        STEP_INVISIBILITY => {
            crate::creatures::despawn_creature_entity(ctx, choreography.arugal_guid);
            schedule_fenrus_step(ctx, choreography.instance_id, 0, STEP_VOIDWALKERS, 500_000);
        }
        STEP_VOIDWALKERS => spawn_voidwalkers(ctx, choreography.instance_id),
        step => spacetimedb::log::warn!("unknown Fenrus choreography step {step}"),
    }
}

crate::game_hook!(on_creature_death, fn arugal_voidwalker_died(ctx, payload) {
    if payload.entry != ARUGAL_VOIDWALKER
        || payload.instance_id == 0
        || !instance_belongs_to_shadowfang(ctx, payload.instance_id)
        || encounter::get_encounter_state(ctx, payload.instance_id, 3) != ENCOUNTER_DONE
    {
        return;
    }
    let another_lives = ctx
        .db
        .game_encounter_spawn()
        .by_instance()
        .filter(&payload.instance_id)
        .filter(|spawn| encounter::entry_of_unit_guid(spawn.guid) == ARUGAL_VOIDWALKER)
        .any(|spawn| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(spawn.guid)
                .is_some_and(|entity| !entity.dead)
        });
    if !another_lives {
        set_gameobject_state(ctx, payload.instance_id, SORCERER_DOOR, DOOR_OPEN_STATE);
    }
});

fn schedule_fenrus_step(
    ctx: &ReducerContext,
    instance_id: u64,
    arugal_guid: u64,
    step: u8,
    delay_micros: i64,
) {
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(delay_micros))
            .unwrap_or(ctx.timestamp),
    );
    ctx.db
        .shadowfang_fenrus_choreography()
        .insert(ShadowfangFenrusChoreography {
            scheduled_id: 0,
            scheduled_at,
            instance_id,
            arugal_guid,
            step,
        });
}

fn clear_fenrus_choreography(ctx: &ReducerContext, instance_id: u64) {
    let table = ctx.db.shadowfang_fenrus_choreography();
    let rows: Vec<(u64, u64)> = table
        .by_instance()
        .filter(&instance_id)
        .map(|row| (row.scheduled_id, row.arugal_guid))
        .collect();
    for (scheduled_id, arugal_guid) in rows {
        table.scheduled_id().delete(scheduled_id);
        if arugal_guid != 0 {
            crate::creatures::despawn_creature_entity(ctx, arugal_guid);
        }
    }
}

fn instance_belongs_to_shadowfang(ctx: &ReducerContext, instance_id: u64) -> bool {
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_some_and(|instance| instance.map_id == MAP_ID)
}

fn spawn_voidwalkers(ctx: &ReducerContext, instance_id: u64) {
    for &(x, y, z, orientation) in &[
        (-155.352, 2172.780, 128.448, 4.679),
        (-147.059, 2163.193, 128.696, 0.128),
        (-148.869, 2180.859, 128.448, 1.814),
        (-140.203, 2175.263, 128.448, 0.373),
    ] {
        encounter::spawn_wave(
            ctx,
            instance_id,
            3,
            MAP_ID,
            &[ARUGAL_VOIDWALKER],
            x + 2.0,
            y,
            z,
            orientation,
        );
    }
}
