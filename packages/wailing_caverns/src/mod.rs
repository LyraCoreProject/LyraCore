use crate::encounter::{
    self, EncounterSignal, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
    ENCOUNTER_NOT_STARTED,
};
use crate::game_world_entity;

const DISCIPLE_ENCOUNTER_ID: u32 = 4;
const DISCIPLE_ESCORT_READY: u32 = 1;

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
    set_boss_state(ctx, instance_id, 5, signal, "Mutanus")
});

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
    if let Some(disciple) = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&43u32)
        .find(|entity| entity.instance_id == instance_id && entity.entry == 3678 && !entity.dead)
    {
        let _ = crate::chat::apply_send_chat(
            ctx,
            disciple,
            crate::chat::CHAT_SAY,
            0,
            "At last! Naralex can be awakened! Come aid me, brave adventurers!".to_string(),
        );
    }
}
