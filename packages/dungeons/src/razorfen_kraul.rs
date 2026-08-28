use crate::encounter::{self, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE};
use crate::{game_gameobject, game_world_entity};

const MAP_ID: u32 = 47;
const ENCOUNTER_ID: u32 = 1;
const WARD_KEEPER: u32 = 4625;
const AGATHELOS_WARD: u32 = 21099;

crate::encounter_package!(RazorfenKraulWardKeepers, fn ward_keepers(ctx, instance_id, signal) {
    if signal != EncounterSignal::Complete {
        return Err(format!("Ward Keepers do not accept encounter signal {signal:?}"));
    }
    let another_keeper_lives = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .any(|entity| {
            entity.instance_id == instance_id && entity.entry == WARD_KEEPER && !entity.dead
        });
    if another_keeper_lives {
        return Ok(());
    }

    let gameobjects = ctx.db.game_gameobject();
    let guids: Vec<u64> = gameobjects
        .by_map()
        .filter(&MAP_ID)
        .filter(|go| go.instance_id == instance_id && go.template_entry == AGATHELOS_WARD)
        .map(|go| go.guid)
        .collect();
    for guid in guids {
        if let Some(mut ward) = gameobjects.guid().find(guid) {
            ward.state = DOOR_OPEN_STATE;
            gameobjects.guid().update(ward);
        }
    }
    encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_DONE)
});
