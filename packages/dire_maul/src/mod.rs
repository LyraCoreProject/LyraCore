use crate::encounter::{
    self, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
};
use crate::game_gameobject;

const MAP_ID: u32 = 429;
const ENCOUNTER_ID: u32 = 0;
const CRUMBLE_WALL: u32 = 177220;
const CORRUPT_VINE: u32 = 179502;
const WALL_BROKEN: u32 = 1;

crate::encounter_package!(DireMaulAlzzin, fn alzzin(ctx, instance_id, signal) {
    match signal {
        EncounterSignal::Begin => {
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_IN_PROGRESS)
        }
        EncounterSignal::Fail => {
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_FAILED)
        }
        EncounterSignal::Complete => {
            break_wall(ctx, instance_id)?;
            open_known_gameobject(ctx, instance_id, CORRUPT_VINE);
            respawn_felvine_shards(ctx, instance_id);
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_DONE)
        }
        EncounterSignal::BreakAlzzinCrumbleWall => break_wall(ctx, instance_id),
        other => Err(format!("Alzzin does not accept encounter signal {other:?}")),
    }
});

fn break_wall(ctx: &spacetimedb::ReducerContext, instance_id: u64) -> Result<(), String> {
    if encounter::get_encounter_data(ctx, instance_id, ENCOUNTER_ID) != WALL_BROKEN {
        open_known_gameobject(ctx, instance_id, CRUMBLE_WALL);
        encounter::set_encounter_data(ctx, instance_id, ENCOUNTER_ID, WALL_BROKEN)?;
    }
    Ok(())
}

fn respawn_felvine_shards(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    const FELVINE_SHARD: u32 = 179559;
    let gameobjects = ctx.db.game_gameobject();
    let guids: Vec<u64> = gameobjects
        .by_map()
        .filter(&MAP_ID)
        .filter(|go| go.instance_id == instance_id && go.template_entry == FELVINE_SHARD)
        .map(|go| go.guid)
        .collect();
    for guid in guids {
        if let Some(mut shard) = gameobjects.guid().find(guid) {
            shard.state = 0;
            shard.respawn_at_micros = 0;
            gameobjects.guid().update(shard);
        }
    }
}

fn open_known_gameobject(ctx: &spacetimedb::ReducerContext, instance_id: u64, entry: u32) {
    let gameobjects = ctx.db.game_gameobject();
    let guids: Vec<u64> = gameobjects
        .by_map()
        .filter(&MAP_ID)
        .filter(|go| go.instance_id == instance_id && go.template_entry == entry)
        .map(|go| go.guid)
        .collect();
    for guid in guids {
        if let Some(mut go) = gameobjects.guid().find(guid) {
            go.state = DOOR_OPEN_STATE;
            gameobjects.guid().update(go);
        }
    }
}
