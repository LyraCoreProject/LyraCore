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
const DARK_OFFERING: u32 = 7154;
const SUMMON_LOW_BAND: u64 = 0x20_0000;
const IMMUNE_TO_PLAYERS: u32 = 0x0000_0100;
const IMMUNE_TO_CREATURES: u32 = 0x0000_0200;

const STEP_SHOW_AND_YELL: u8 = 0;
const STEP_FIRE: u8 = 1;
const STEP_LIGHTNING: u8 = 2;
const STEP_INVISIBILITY: u8 = 3;
const STEP_VOIDWALKERS: u8 = 4;

pub(crate) const VOIDWALKER_ROUTE: [(f32, f32, f32); 23] = [
    (-159.547, 2178.11, 128.944),
    (-171.113, 2182.69, 129.255),
    (-177.613, 2175.59, 128.161),
    (-185.396, 2178.35, 126.413),
    (-184.004, 2188.31, 124.122),
    (-172.781, 2188.71, 121.611),
    (-173.245, 2176.93, 119.085),
    (-183.145, 2176.04, 116.995),
    (-185.551, 2185.77, 114.784),
    (-177.502, 2190.75, 112.681),
    (-171.218, 2182.61, 110.314),
    (-173.857, 2175.1, 109.255),
    (-171.218, 2182.61, 110.314),
    (-177.502, 2190.75, 112.681),
    (-185.551, 2185.77, 114.784),
    (-183.145, 2176.04, 116.995),
    (-173.245, 2176.93, 119.085),
    (-172.781, 2188.71, 121.611),
    (-184.004, 2188.31, 124.122),
    (-185.396, 2178.35, 126.413),
    (-177.613, 2175.59, 128.161),
    (-171.113, 2182.69, 129.255),
    (-159.547, 2178.11, 128.944),
];

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

#[table(accessor = shadowfang_voidwalker_group)]
pub struct ShadowfangVoidwalkerGroup {
    #[primary_key]
    pub instance_id: u64,
    pub walker_guids: Vec<u64>,
    pub leader_guid: u64,
    pub route_point: u8,
}

#[table(
    accessor = shadowfang_voidwalker_group_schedule,
    scheduled(advance_voidwalker_group),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct ShadowfangVoidwalkerGroupSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
    pub route_point: u8,
}

#[table(
    accessor = shadowfang_dark_offering_schedule,
    scheduled(advance_dark_offering),
    index(accessor = by_instance, btree(columns = [instance_id])),
    index(accessor = by_caster, btree(columns = [caster_guid]))
)]
pub struct ShadowfangDarkOfferingSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
    pub caster_guid: u64,
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
        life_seq: 0,
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
        STEP_VOIDWALKERS => {
            let walkers = spawn_voidwalkers(ctx, choreography.instance_id);
            if !walkers.is_empty() {
                begin_voidwalker_group(ctx, choreography.instance_id, walkers);
            }
        }
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
    clear_dark_offering(ctx, payload.creature_guid);
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
    clear_voidwalker_group_schedule(ctx, payload.instance_id);
    if another_lives {
        if let Some(group) = ctx
            .db
            .shadowfang_voidwalker_group()
            .instance_id()
            .find(payload.instance_id)
        {
            schedule_voidwalker_group(ctx, payload.instance_id, group.route_point, 1_000_000);
        }
    } else {
        set_gameobject_state(ctx, payload.instance_id, SORCERER_DOOR, DOOR_OPEN_STATE);
    }
});

crate::game_hook!(on_aggro, fn arugal_voidwalker_entered_combat(ctx, payload) {
    let Some(walker) = ctx.db.game_world_entity().guid().find(payload.creature_guid) else {
        return;
    };
    if walker.entry != ARUGAL_VOIDWALKER
        || walker.map_id != MAP_ID
        || walker.instance_id == 0
        || !instance_belongs_to_shadowfang(ctx, walker.instance_id)
    {
        return;
    }
    if let Some(group) = ctx
        .db
        .shadowfang_voidwalker_group()
        .instance_id()
        .find(walker.instance_id)
    {
        clear_voidwalker_group_schedule(ctx, walker.instance_id);
        schedule_voidwalker_group(ctx, walker.instance_id, group.route_point, 1_000_000);
    }
    clear_dark_offering(ctx, walker.guid);
    schedule_dark_offering(
        ctx,
        walker.instance_id,
        walker.guid,
        dark_offering_initial_delay(ctx.random::<u32>()),
    );
});

crate::game_tick_pass!(fn shadowfang_restart_recovery_pass(ctx) {
    restore_sorcerer_doors(ctx);
    sweep_shadowfang_package_state(ctx);
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

fn spawn_voidwalkers(ctx: &ReducerContext, instance_id: u64) -> Vec<u64> {
    let mut walker_guids = Vec::with_capacity(4);
    for &(x, y, z, orientation) in &[
        (-155.352, 2172.780, 128.448, 4.679),
        (-147.059, 2163.193, 128.696, 0.128),
        (-148.869, 2180.859, 128.448, 1.814),
        (-140.203, 2175.263, 128.448, 0.373),
    ] {
        walker_guids.extend(encounter::spawn_wave(
            ctx,
            instance_id,
            3,
            MAP_ID,
            &[ARUGAL_VOIDWALKER],
            x + 2.0,
            y,
            z,
            orientation,
        ));
    }
    walker_guids
}

fn begin_voidwalker_group(ctx: &ReducerContext, instance_id: u64, mut walker_guids: Vec<u64>) {
    walker_guids.sort_unstable();
    walker_guids.dedup();
    let Some(&leader_guid) = walker_guids.first() else {
        return;
    };
    let table = ctx.db.shadowfang_voidwalker_group();
    if table.instance_id().find(instance_id).is_some() {
        table.instance_id().delete(instance_id);
    }
    table.insert(ShadowfangVoidwalkerGroup {
        instance_id,
        walker_guids,
        leader_guid,
        route_point: 0,
    });
    clear_voidwalker_group_schedule(ctx, instance_id);
    schedule_voidwalker_group(ctx, instance_id, 0, 100_000);
}

#[reducer]
pub fn advance_voidwalker_group(
    ctx: &ReducerContext,
    scheduled: ShadowfangVoidwalkerGroupSchedule,
) {
    if ctx.sender() != ctx.database_identity()
        || !instance_belongs_to_shadowfang(ctx, scheduled.instance_id)
    {
        return;
    }
    let table = ctx.db.shadowfang_voidwalker_group();
    let Some(mut group) = table.instance_id().find(scheduled.instance_id) else {
        return;
    };
    let entities = ctx.db.game_world_entity();
    group.walker_guids.retain(|guid| {
        entities.guid().find(*guid).is_some_and(|entity| {
            entity.map_id == MAP_ID
                && entity.instance_id == scheduled.instance_id
                && entity.entry == ARUGAL_VOIDWALKER
                && !entity.dead
        })
    });
    if group.walker_guids.is_empty() {
        table.instance_id().update(group);
        return;
    }
    if !group.walker_guids.contains(&group.leader_guid) {
        group.walker_guids.sort_unstable();
        group.leader_guid = group.walker_guids[0];
    }
    if group
        .walker_guids
        .iter()
        .any(|guid| crate::combat::is_engaged(ctx, *guid))
    {
        let route_point = group.route_point;
        table.instance_id().update(group);
        schedule_voidwalker_group(ctx, scheduled.instance_id, route_point, 1_000_000);
        return;
    }

    let route_point = usize::from(scheduled.route_point) % VOIDWALKER_ROUTE.len();
    let destination = VOIDWALKER_ROUTE[route_point];
    let previous = if route_point == 0 {
        VOIDWALKER_ROUTE[VOIDWALKER_ROUTE.len() - 2]
    } else {
        VOIDWALKER_ROUTE[route_point - 1]
    };
    let heading = (destination.1 - previous.1).atan2(destination.0 - previous.0);
    let mut movement_micros = 1_i64;
    let walker_guids = group.walker_guids.clone();
    for (position, guid) in walker_guids.into_iter().enumerate() {
        let Some(walker) = entities.guid().find(guid) else {
            continue;
        };
        let target = if position == 0 {
            destination
        } else {
            let angle = heading + std::f32::consts::FRAC_PI_2 * position as f32;
            (
                destination.0 + angle.cos(),
                destination.1 + angle.sin(),
                destination.2,
            )
        };
        let dx = target.0 - walker.x;
        let dy = target.1 - walker.y;
        let speed = crate::combat::effective_move_speed(
            ctx,
            guid,
            lyracore_shared::constants::speeds::WALK,
        );
        if speed > 0.0 {
            movement_micros = movement_micros
                .max(((((dx * dx + dy * dy).sqrt() / speed) * 1_000_000.0) as i64).max(1));
            if let Err(error) =
                encounter::move_to_point(ctx, guid, target.0, target.1, target.2, false)
            {
                spacetimedb::log::warn!("Arugal voidwalker {guid} could not patrol: {error}");
            }
        }
    }
    let next_route_point = ((route_point + 1) % VOIDWALKER_ROUTE.len()) as u8;
    group.route_point = next_route_point;
    table.instance_id().update(group);
    schedule_voidwalker_group(
        ctx,
        scheduled.instance_id,
        next_route_point,
        movement_micros,
    );
}

fn schedule_voidwalker_group(
    ctx: &ReducerContext,
    instance_id: u64,
    route_point: u8,
    delay_micros: i64,
) {
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(delay_micros))
            .unwrap_or(ctx.timestamp),
    );
    ctx.db
        .shadowfang_voidwalker_group_schedule()
        .insert(ShadowfangVoidwalkerGroupSchedule {
            scheduled_id: 0,
            scheduled_at,
            instance_id,
            route_point,
        });
}

fn clear_voidwalker_group_schedule(ctx: &ReducerContext, instance_id: u64) {
    let table = ctx.db.shadowfang_voidwalker_group_schedule();
    let ids: Vec<u64> = table
        .by_instance()
        .filter(&instance_id)
        .map(|row| row.scheduled_id)
        .collect();
    for id in ids {
        table.scheduled_id().delete(id);
    }
}

fn dark_offering_initial_delay(roll: u32) -> i64 {
    delay_from_roll(roll, 4_400_000, 12_500_000)
}

fn dark_offering_repeat_delay(roll: u32) -> i64 {
    delay_from_roll(roll, 4_000_000, 12_000_000)
}

fn delay_from_roll(roll: u32, minimum: i64, maximum: i64) -> i64 {
    minimum + ((u64::from(roll) * (maximum - minimum) as u64) / u64::from(u32::MAX)) as i64
}

fn schedule_dark_offering(
    ctx: &ReducerContext,
    instance_id: u64,
    caster_guid: u64,
    delay_micros: i64,
) {
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(delay_micros))
            .unwrap_or(ctx.timestamp),
    );
    ctx.db
        .shadowfang_dark_offering_schedule()
        .insert(ShadowfangDarkOfferingSchedule {
            scheduled_id: 0,
            scheduled_at,
            instance_id,
            caster_guid,
        });
}

fn clear_dark_offering(ctx: &ReducerContext, caster_guid: u64) {
    let table = ctx.db.shadowfang_dark_offering_schedule();
    let ids: Vec<u64> = table
        .by_caster()
        .filter(&caster_guid)
        .map(|row| row.scheduled_id)
        .collect();
    for id in ids {
        table.scheduled_id().delete(id);
    }
}

#[reducer]
pub fn advance_dark_offering(ctx: &ReducerContext, scheduled: ShadowfangDarkOfferingSchedule) {
    if ctx.sender() != ctx.database_identity()
        || !instance_belongs_to_shadowfang(ctx, scheduled.instance_id)
    {
        return;
    }
    let Some(caster) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(scheduled.caster_guid)
        .filter(|entity| {
            entity.map_id == MAP_ID
                && entity.instance_id == scheduled.instance_id
                && entity.entry == ARUGAL_VOIDWALKER
                && !entity.dead
        })
    else {
        return;
    };
    if !crate::combat::is_engaged(ctx, caster.guid) {
        return;
    }
    let target = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .filter(|candidate| {
            candidate.instance_id == scheduled.instance_id
                && !candidate.dead
                && candidate.max_health.saturating_sub(candidate.health) >= 290
                && crate::combat::may_help(ctx, &caster, candidate)
                && {
                    let dx = candidate.x - caster.x;
                    let dy = candidate.y - caster.y;
                    dx * dx + dy * dy <= 100.0
                }
        })
        .min_by_key(|candidate| candidate.health);
    let delay = if let Some(target) = target {
        if crate::actor::cast_at(ctx, caster.guid, DARK_OFFERING, target.guid).is_ok() {
            dark_offering_repeat_delay(ctx.random::<u32>())
        } else {
            500_000
        }
    } else {
        500_000
    };
    schedule_dark_offering(ctx, scheduled.instance_id, caster.guid, delay);
}

fn restore_sorcerer_doors(ctx: &ReducerContext) {
    let instances: Vec<u64> = ctx
        .db
        .game_gameobject()
        .by_map()
        .filter(&MAP_ID)
        .filter(|gameobject| gameobject.template_entry == SORCERER_DOOR)
        .map(|gameobject| gameobject.instance_id)
        .collect();
    for instance_id in instances {
        if !instance_belongs_to_shadowfang(ctx, instance_id)
            || encounter::get_encounter_state(ctx, instance_id, 3) != ENCOUNTER_DONE
            || ctx
                .db
                .shadowfang_fenrus_choreography()
                .by_instance()
                .filter(&instance_id)
                .next()
                .is_some()
        {
            continue;
        }
        let another_lives = ctx
            .db
            .shadowfang_voidwalker_group()
            .instance_id()
            .find(instance_id)
            .is_some_and(|group| {
                group.walker_guids.iter().any(|guid| {
                    ctx.db
                        .game_world_entity()
                        .guid()
                        .find(*guid)
                        .is_some_and(|entity| !entity.dead)
                })
            });
        if !another_lives {
            set_gameobject_state(ctx, instance_id, SORCERER_DOOR, DOOR_OPEN_STATE);
        }
    }
}

fn sweep_shadowfang_package_state(ctx: &ReducerContext) {
    let stale_instances: Vec<u64> = ctx
        .db
        .shadowfang_voidwalker_group()
        .iter()
        .filter(|group| !instance_belongs_to_shadowfang(ctx, group.instance_id))
        .map(|group| group.instance_id)
        .collect();
    for instance_id in stale_instances {
        clear_voidwalker_group_schedule(ctx, instance_id);
        ctx.db
            .shadowfang_voidwalker_group()
            .instance_id()
            .delete(instance_id);
    }

    let stale_offerings: Vec<(u64, u64)> = ctx
        .db
        .shadowfang_dark_offering_schedule()
        .iter()
        .filter(|scheduled| {
            !instance_belongs_to_shadowfang(ctx, scheduled.instance_id)
                || ctx
                    .db
                    .game_world_entity()
                    .guid()
                    .find(scheduled.caster_guid)
                    .is_none_or(|caster| caster.dead || caster.instance_id != scheduled.instance_id)
        })
        .map(|scheduled| (scheduled.scheduled_id, scheduled.caster_guid))
        .collect();
    for (scheduled_id, caster_guid) in stale_offerings {
        ctx.db
            .shadowfang_dark_offering_schedule()
            .scheduled_id()
            .delete(scheduled_id);
        clear_dark_offering(ctx, caster_guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arugal_voidwalker_route_matches_the_pinned_source_account() {
        assert_eq!(VOIDWALKER_ROUTE.len(), 23);
        assert_eq!(VOIDWALKER_ROUTE[0], (-159.547, 2178.11, 128.944));
        assert_eq!(VOIDWALKER_ROUTE[11], (-173.857, 2175.1, 109.255));
        assert_eq!(VOIDWALKER_ROUTE[22], (-159.547, 2178.11, 128.944));
    }

    #[test]
    fn dark_offering_delays_keep_the_source_bounds() {
        assert_eq!(dark_offering_initial_delay(0), 4_400_000);
        assert_eq!(dark_offering_initial_delay(u32::MAX), 12_500_000);
        assert_eq!(dark_offering_repeat_delay(0), 4_000_000);
        assert_eq!(dark_offering_repeat_delay(u32::MAX), 12_000_000);
    }
}
