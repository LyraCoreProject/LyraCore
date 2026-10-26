use spacetimedb::{table, ReducerContext, Table};

use super::tables::{energy_ticks_between, regen_energy};
use crate::game_world_entity;
use lyracore_shared::packing::power_type::ENERGY;

const CLOCK_ID: u8 = 0;

#[table(accessor = game_energy_clock)]
pub struct EnergyClock {
    #[primary_key]
    pub id: u8,
    pub credited_through_ms: u64,
}

// Runs on every catch-all firing; returns the energy users it credited.
pub(crate) fn regenerate_energy(ctx: &ReducerContext) -> usize {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch().max(0) / 1_000) as u64;
    let clocks = ctx.db.game_energy_clock();
    let Some(clock) = clocks.id().find(CLOCK_ID) else {
        clocks.insert(EnergyClock {
            id: CLOCK_ID,
            credited_through_ms: now_ms,
        });
        return 0;
    };
    let ticks = energy_ticks_between(clock.credited_through_ms, now_ms);
    if ticks == 0 {
        return 0;
    }
    clocks.id().update(EnergyClock {
        credited_through_ms: now_ms,
        ..clock
    });
    let entities = ctx.db.game_world_entity();
    let regenerating: Vec<_> = entities
        .by_regeneration_pending()
        .filter(true)
        .filter(|e| e.power_type() == ENERGY && e.power < e.max_power)
        .collect();
    let credited = regenerating.len();
    for mut e in regenerating {
        let regen_pct = crate::spell::power_regen_pct(ctx, e.guid, ENERGY);
        e.power = regen_energy(e.power, e.max_power, ticks, regen_pct);
        e.update(ctx);
    }
    credited
}
