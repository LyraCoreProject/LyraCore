use crate::encounter::{self, EncounterSignal};
use crate::game_world_entity;

const MAP_ID: u32 = 309;
const BLOODLORD_MANDOKIR: u32 = 11382;

crate::encounter_package!(ZulGurubOhgan, fn ohgan(ctx, instance_id, signal) {
    if signal != EncounterSignal::SendMandokirDownstairs {
        return Err(format!("Ohgan does not accept encounter signal {signal:?}"));
    }
    if let Some(mandokir) = ctx
        .db
        .game_world_entity()
        .by_map()
        .filter(&MAP_ID)
        .find(|entity| {
            entity.instance_id == instance_id
                && entity.entry == BLOODLORD_MANDOKIR
                && !entity.dead
        })
    {
        encounter::move_to_point(
            ctx,
            mandokir.guid,
            -12196.30,
            -1948.37,
            130.31,
            true,
        )?;
    }
    Ok(())
});
