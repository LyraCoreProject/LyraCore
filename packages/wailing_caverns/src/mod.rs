use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::encounter::{
    self, EncounterSignal, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
    ENCOUNTER_NOT_STARTED,
};
use crate::{game_encounter_spawn, game_instance, game_world_entity};

const MAP_ID: u32 = 43;
const DISCIPLE_ENCOUNTER_ID: u32 = 4;
const DISCIPLE_ESCORT_READY: u32 = 1;
const DISCIPLE_OF_NARALEX: u32 = 3678;
const NARALEX: u32 = 3679;
const MUTANUS: u32 = 3654;
const DEVIATE_MOCCASIN: u32 = 5762;
const NIGHTMARE_ECTOPLASM: u32 = 5763;
const WAILING_START_OPTION_ROW: u32 = 50_296;
const ESCORT_FACTION: u32 = 250;
const AWAKENING: u32 = 6271;
const SHAPESHIFT: u32 = 8153;

const PHASE_MOVE_FIRST_CORNER: u8 = 0;
const PHASE_MOVE_CIRCLE: u8 = 1;
const PHASE_MOVE_CHAMBER: u8 = 2;
const PHASE_BEGIN_RITUAL: u8 = 3;
const PHASE_CAST_AWAKENING: u8 = 4;
const PHASE_SPAWN_MOCCASINS: u8 = 5;
const PHASE_WAIT_MOCCASINS: u8 = 6;
const PHASE_SPAWN_ECTOPLASMS: u8 = 7;
const PHASE_WAIT_ECTOPLASMS: u8 = 8;
const PHASE_SPAWN_MUTANUS: u8 = 9;
const PHASE_WAIT_MUTANUS: u8 = 10;
const PHASE_NARALEX_AWAKE: u8 = 11;
const PHASE_DISCIPLE_AWAKE: u8 = 12;
const PHASE_NARALEX_THANKS: u8 = 13;
const PHASE_FAREWELL: u8 = 14;
const PHASE_SHAPESHIFT: u8 = 15;
const PHASE_EXIT: u8 = 16;
const PHASE_DESPAWN: u8 = 17;

#[table(accessor = wailing_escort_progress)]
pub struct WailingEscortProgress {
    #[primary_key]
    pub instance_id: u64,
    pub disciple_guid: u64,
    pub naralex_guid: u64,
    pub phase: u8,
}

#[table(
    accessor = wailing_escort_schedule,
    scheduled(advance_wailing_escort),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct WailingEscortSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
    pub phase: u8,
}

crate::encounter_package!(WailingCavernsAnacondra, fn anacondra(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 0, signal, "Anacondra")
});

crate::encounter_package!(WailingCavernsCobrahn, fn cobrahn(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 1, signal, "Cobrahn")
});

crate::encounter_package!(WailingCavernsPythas, fn pythas(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 2, signal, "Pythas")
});

crate::encounter_package!(WailingCavernsSerpentis, fn serpentis(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 3, signal, "Serpentis")
});

crate::encounter_package!(WailingCavernsMutanus, fn mutanus(ctx, instance_id, signal) {
    set_boss_state(ctx, instance_id, 5, signal, "Mutanus")?;
    if signal == EncounterSignal::Complete {
        begin_awakening(ctx, instance_id);
    }
    Ok(())
});

crate::game_hook!(on_gossip_select, fn disciple_start_selected(ctx, payload) {
    if payload.option_row_id != WAILING_START_OPTION_ROW {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let (Some(player), Some(disciple)) = (
        entities.guid().find(payload.character_guid),
        entities.guid().find(payload.npc_guid),
    ) else {
        return;
    };
    if !player.is_player()
        || player.dead
        || disciple.dead
        || disciple.entry != DISCIPLE_OF_NARALEX
        || disciple.map_id != MAP_ID
        || player.map_id != MAP_ID
        || disciple.instance_id == 0
        || player.instance_id != disciple.instance_id
        || !instance_belongs_to_wailing(ctx, disciple.instance_id)
        || encounter::get_encounter_data(ctx, disciple.instance_id, DISCIPLE_ENCOUNTER_ID)
            != DISCIPLE_ESCORT_READY
        || !matches!(
            encounter::get_encounter_state(ctx, disciple.instance_id, DISCIPLE_ENCOUNTER_ID),
            ENCOUNTER_NOT_STARTED | ENCOUNTER_FAILED
        )
    {
        return;
    }
    start_escort(ctx, disciple.instance_id, disciple.guid);
});

crate::game_hook!(on_creature_death, fn wailing_ritual_add_died(ctx, payload) {
    if payload.instance_id == 0
        || !instance_belongs_to_wailing(ctx, payload.instance_id)
        || !matches!(payload.entry, DEVIATE_MOCCASIN | NIGHTMARE_ECTOPLASM)
        || encounter::get_encounter_state(ctx, payload.instance_id, DISCIPLE_ENCOUNTER_ID)
            != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(payload.instance_id)
    else {
        return;
    };
    let (waiting_phase, next_phase) = if payload.entry == DEVIATE_MOCCASIN {
        (PHASE_WAIT_MOCCASINS, PHASE_SPAWN_ECTOPLASMS)
    } else {
        (PHASE_WAIT_ECTOPLASMS, PHASE_SPAWN_MUTANUS)
    };
    if progress.phase != waiting_phase
        || ctx
            .db
            .game_encounter_spawn()
            .by_instance()
            .filter(&payload.instance_id)
            .filter(|spawn| encounter::entry_of_unit_guid(spawn.guid) == payload.entry)
            .any(|spawn| {
                ctx.db
                    .game_world_entity()
                    .guid()
                    .find(spawn.guid)
                    .is_some_and(|entity| !entity.dead)
            })
    {
        return;
    }
    set_progress_phase(ctx, progress, next_phase);
    schedule_escort_phase(ctx, payload.instance_id, next_phase, 1_000_000);
});

#[reducer]
pub fn advance_wailing_escort(ctx: &ReducerContext, scheduled: WailingEscortSchedule) {
    if ctx.sender() != ctx.database_identity()
        || !instance_belongs_to_wailing(ctx, scheduled.instance_id)
    {
        return;
    }
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(scheduled.instance_id)
    else {
        return;
    };
    if progress.phase != scheduled.phase {
        return;
    }
    if let Err(error) = perform_escort_phase(ctx, progress) {
        spacetimedb::log::warn!("Wailing Caverns escort stopped: {error}");
    }
}

fn set_boss_state(
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
    encounter::set_encounter_state(ctx, instance_id, encounter_id, state)?;
    refresh_disciple_gate(ctx, instance_id)
}

fn refresh_disciple_gate(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
) -> Result<(), String> {
    let all_leaders_done = (0..=3).all(|encounter_id| {
        encounter::get_encounter_state(ctx, instance_id, encounter_id) == ENCOUNTER_DONE
    });
    let disciple_state = encounter::get_encounter_state(ctx, instance_id, DISCIPLE_ENCOUNTER_ID);
    if all_leaders_done && matches!(disciple_state, ENCOUNTER_NOT_STARTED | ENCOUNTER_FAILED) {
        if disciple_state == ENCOUNTER_NOT_STARTED {
            speak_disciple_intro(ctx, instance_id);
        }
        encounter::set_encounter_data(
            ctx,
            instance_id,
            DISCIPLE_ENCOUNTER_ID,
            DISCIPLE_ESCORT_READY,
        )?;
    }
    Ok(())
}

fn speak_disciple_intro(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    speak_entry(
        ctx,
        instance_id,
        DISCIPLE_OF_NARALEX,
        "At last! Naralex can be awakened! Come aid me, brave adventurers!",
    );
}

fn start_escort(ctx: &ReducerContext, instance_id: u64, disciple_guid: u64) {
    let Some(naralex) = live_instance_creature(ctx, instance_id, NARALEX) else {
        return;
    };
    clear_escort_schedule(ctx, instance_id);
    if let Some(mut disciple) = ctx.db.game_world_entity().guid().find(disciple_guid) {
        disciple.faction_template = ESCORT_FACTION;
        ctx.db.game_world_entity().guid().update(disciple);
    }
    if encounter::set_encounter_state(
        ctx,
        instance_id,
        DISCIPLE_ENCOUNTER_ID,
        ENCOUNTER_IN_PROGRESS,
    )
    .is_err()
    {
        return;
    }
    let progress = WailingEscortProgress {
        instance_id,
        disciple_guid,
        naralex_guid: naralex.guid,
        phase: PHASE_MOVE_FIRST_CORNER,
    };
    let table = ctx.db.wailing_escort_progress();
    if table.instance_id().find(instance_id).is_some() {
        table.instance_id().update(progress);
    } else {
        table.insert(progress);
    }
    schedule_escort_phase(ctx, instance_id, PHASE_MOVE_FIRST_CORNER, 100_000);
}

fn begin_awakening(ctx: &ReducerContext, instance_id: u64) {
    let Some(progress) = ctx
        .db
        .wailing_escort_progress()
        .instance_id()
        .find(instance_id)
    else {
        return;
    };
    if encounter::get_encounter_state(ctx, instance_id, DISCIPLE_ENCOUNTER_ID)
        != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    clear_escort_schedule(ctx, instance_id);
    set_progress_phase(ctx, progress, PHASE_NARALEX_AWAKE);
    schedule_escort_phase(ctx, instance_id, PHASE_NARALEX_AWAKE, 100_000);
}

fn perform_escort_phase(
    ctx: &ReducerContext,
    progress: WailingEscortProgress,
) -> Result<(), String> {
    match progress.phase {
        PHASE_MOVE_FIRST_CORNER => move_and_continue(
            ctx,
            progress,
            (-104.28827, 234.40804, -91.64163),
            PHASE_MOVE_CIRCLE,
            2_000_000,
        ),
        PHASE_MOVE_CIRCLE => move_and_continue(
            ctx,
            progress,
            (-54.713943, 273.85025, -92.84426),
            PHASE_MOVE_CHAMBER,
            1_000_000,
        ),
        PHASE_MOVE_CHAMBER => move_and_continue(
            ctx,
            progress,
            (114.51453, 235.30222, -96.1607),
            PHASE_BEGIN_RITUAL,
            1_000_000,
        ),
        PHASE_BEGIN_RITUAL => {
            speak_guid(
                ctx,
                progress.disciple_guid,
                "Protect me, brave souls, as I delve into the Emerald Dream to rescue Naralex and put an end to this corruption!",
            );
            continue_after(ctx, progress, PHASE_CAST_AWAKENING, 5_000_000);
            Ok(())
        }
        PHASE_CAST_AWAKENING => {
            crate::actor::cast_at(
                ctx,
                progress.disciple_guid,
                AWAKENING,
                progress.disciple_guid,
            )?;
            continue_after(ctx, progress, PHASE_SPAWN_MOCCASINS, 3_000_000);
            Ok(())
        }
        PHASE_SPAWN_MOCCASINS => {
            spawn_at_source_positions(
                ctx,
                progress.instance_id,
                DEVIATE_MOCCASIN,
                &[
                    (171.39545, 213.76605, -105.50746),
                    (156.72229, 189.91829, -107.48995),
                    (121.39977, 166.31746, -105.54061),
                ],
            );
            set_progress_phase(ctx, progress, PHASE_WAIT_MOCCASINS);
            Ok(())
        }
        PHASE_SPAWN_ECTOPLASMS => {
            spawn_at_source_positions(
                ctx,
                progress.instance_id,
                NIGHTMARE_ECTOPLASM,
                &[
                    (162.06705, 218.71494, -105.36240),
                    (115.55489, 168.22847, -105.68655),
                    (82.065025, 280.37723, -103.29671),
                    (144.84305, 278.07928, -104.57445),
                    (155.84459, 186.68817, -107.08412),
                    (145.35356, 219.34600, -102.98572),
                    (164.62735, 274.12335, -107.29780),
                ],
            );
            set_progress_phase(ctx, progress, PHASE_WAIT_ECTOPLASMS);
            Ok(())
        }
        PHASE_SPAWN_MUTANUS => {
            spawn_at_source_positions(
                ctx,
                progress.instance_id,
                MUTANUS,
                &[(150.94276, 262.79715, -103.90348)],
            );
            encounter::set_encounter_state(ctx, progress.instance_id, 5, ENCOUNTER_IN_PROGRESS)?;
            set_progress_phase(ctx, progress, PHASE_WAIT_MUTANUS);
            Ok(())
        }
        PHASE_NARALEX_AWAKE => {
            if let Some(mut naralex) = ctx
                .db
                .game_world_entity()
                .guid()
                .find(progress.naralex_guid)
            {
                naralex.unit_bytes_1 &= !0xFF;
                ctx.db.game_world_entity().guid().update(naralex);
            }
            speak_guid(ctx, progress.naralex_guid, "I am awake, at last!");
            encounter::set_encounter_state(
                ctx,
                progress.instance_id,
                DISCIPLE_ENCOUNTER_ID,
                ENCOUNTER_DONE,
            )?;
            continue_after(ctx, progress, PHASE_DISCIPLE_AWAKE, 5_000_000);
            Ok(())
        }
        PHASE_DISCIPLE_AWAKE => {
            speak_guid(
                ctx,
                progress.disciple_guid,
                "At last! Naralex can be awakened! Come aid me, brave adventurers!",
            );
            continue_after(ctx, progress, PHASE_NARALEX_THANKS, 1_000_000);
            Ok(())
        }
        PHASE_NARALEX_THANKS => {
            speak_guid(
                ctx,
                progress.naralex_guid,
                "Ah, to be pulled from the dreaded nightmare! I thank you, my loyal Disciple, and your brave companions as well.",
            );
            continue_after(ctx, progress, PHASE_FAREWELL, 7_000_000);
            Ok(())
        }
        PHASE_FAREWELL => {
            speak_guid(
                ctx,
                progress.naralex_guid,
                "We must go and gather with the other Disciples. There is much work to be done before I can make another attempt to restore the Barrens. Farewell, brave souls!",
            );
            continue_after(ctx, progress, PHASE_SHAPESHIFT, 3_000_000);
            Ok(())
        }
        PHASE_SHAPESHIFT => {
            crate::actor::cast_at(
                ctx,
                progress.naralex_guid,
                SHAPESHIFT,
                progress.naralex_guid,
            )?;
            crate::actor::cast_at(
                ctx,
                progress.disciple_guid,
                SHAPESHIFT,
                progress.disciple_guid,
            )?;
            continue_after(ctx, progress, PHASE_EXIT, 8_000_000);
            Ok(())
        }
        PHASE_EXIT => {
            encounter::move_to_point(ctx, progress.disciple_guid, 134.0, 199.0, -103.0, true)?;
            encounter::move_to_point(ctx, progress.naralex_guid, 129.0, 199.0, -103.0, true)?;
            continue_after(ctx, progress, PHASE_DESPAWN, 30_000_000);
            Ok(())
        }
        PHASE_DESPAWN => {
            crate::creatures::despawn_creature_entity(ctx, progress.naralex_guid);
            crate::creatures::despawn_creature_entity(ctx, progress.disciple_guid);
            ctx.db
                .wailing_escort_progress()
                .instance_id()
                .delete(progress.instance_id);
            Ok(())
        }
        phase => Err(format!("unsupported Wailing Caverns escort phase {phase}")),
    }
}

fn move_and_continue(
    ctx: &ReducerContext,
    progress: WailingEscortProgress,
    destination: (f32, f32, f32),
    next_phase: u8,
    pause_micros: i64,
) -> Result<(), String> {
    let mover = ctx
        .db
        .game_world_entity()
        .guid()
        .find(progress.disciple_guid)
        .ok_or_else(|| format!("Disciple {} is missing", progress.disciple_guid))?;
    let dx = destination.0 - mover.x;
    let dy = destination.1 - mover.y;
    let speed = crate::combat::effective_move_speed(
        ctx,
        progress.disciple_guid,
        lyracore_shared::constants::speeds::WALK,
    );
    if speed <= 0.0 {
        return Err("Disciple cannot move while immobilized".to_string());
    }
    let movement_micros = (((dx * dx + dy * dy).sqrt() / speed) * 1_000_000.0) as i64;
    encounter::move_to_point(
        ctx,
        progress.disciple_guid,
        destination.0,
        destination.1,
        destination.2,
        false,
    )?;
    continue_after(
        ctx,
        progress,
        next_phase,
        movement_micros.saturating_add(pause_micros),
    );
    Ok(())
}

fn continue_after(
    ctx: &ReducerContext,
    progress: WailingEscortProgress,
    next_phase: u8,
    delay_micros: i64,
) {
    let instance_id = progress.instance_id;
    set_progress_phase(ctx, progress, next_phase);
    schedule_escort_phase(ctx, instance_id, next_phase, delay_micros);
}

fn set_progress_phase(ctx: &ReducerContext, mut progress: WailingEscortProgress, phase: u8) {
    progress.phase = phase;
    ctx.db
        .wailing_escort_progress()
        .instance_id()
        .update(progress);
}

fn schedule_escort_phase(ctx: &ReducerContext, instance_id: u64, phase: u8, delay_micros: i64) {
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(delay_micros))
            .unwrap_or(ctx.timestamp),
    );
    ctx.db
        .wailing_escort_schedule()
        .insert(WailingEscortSchedule {
            scheduled_id: 0,
            scheduled_at,
            instance_id,
            phase,
        });
}

fn clear_escort_schedule(ctx: &ReducerContext, instance_id: u64) {
    let schedules = ctx.db.wailing_escort_schedule();
    let ids: Vec<u64> = schedules
        .by_instance()
        .filter(&instance_id)
        .map(|scheduled| scheduled.scheduled_id)
        .collect();
    for id in ids {
        schedules.scheduled_id().delete(id);
    }
}

fn spawn_at_source_positions(
    ctx: &ReducerContext,
    instance_id: u64,
    entry: u32,
    positions: &[(f32, f32, f32)],
) {
    for &(x, y, z) in positions {
        encounter::spawn_wave(
            ctx,
            instance_id,
            DISCIPLE_ENCOUNTER_ID,
            MAP_ID,
            &[entry],
            x + 2.0,
            y,
            z,
            0.0,
        );
    }
}

fn live_instance_creature(
    ctx: &ReducerContext,
    instance_id: u64,
    entry: u32,
) -> Option<crate::WorldEntity> {
    ctx.db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .find(|entity| entity.instance_id == instance_id && entity.entry == entry && !entity.dead)
}

fn speak_entry(ctx: &ReducerContext, instance_id: u64, entry: u32, message: &str) {
    if let Some(speaker) = live_instance_creature(ctx, instance_id, entry) {
        let _ = crate::chat::apply_send_chat(
            ctx,
            speaker,
            crate::chat::CHAT_SAY,
            0,
            message.to_string(),
        );
    }
}

fn speak_guid(ctx: &ReducerContext, guid: u64, message: &str) {
    if let Some(speaker) = ctx.db.game_world_entity().guid().find(guid) {
        let _ = crate::chat::apply_send_chat(
            ctx,
            speaker,
            crate::chat::CHAT_SAY,
            0,
            message.to_string(),
        );
    }
}

fn instance_belongs_to_wailing(ctx: &ReducerContext, instance_id: u64) -> bool {
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_some_and(|instance| instance.map_id == MAP_ID)
}
