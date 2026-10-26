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

#[derive(Debug, PartialEq)]
enum ClockStep {
    Hold,
    // The clock moved backward, so the stored time restarts at now without credit.
    Rebase,
    Credit(u64),
}

fn clock_step(credited_through_ms: u64, now_ms: u64) -> ClockStep {
    if now_ms < credited_through_ms {
        return ClockStep::Rebase;
    }
    match energy_ticks_between(credited_through_ms, now_ms) {
        0 => ClockStep::Hold,
        ticks => ClockStep::Credit(ticks),
    }
}

// Call on every catch-all firing; returns the energy users it credited.
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
    let step = clock_step(clock.credited_through_ms, now_ms);
    if step != ClockStep::Hold {
        clocks.id().update(EnergyClock {
            credited_through_ms: now_ms,
            ..clock
        });
    }
    match step {
        ClockStep::Credit(ticks) => credit_energy(ctx, ticks),
        ClockStep::Hold | ClockStep::Rebase => 0,
    }
}

fn credit_energy(ctx: &ReducerContext, ticks: u64) -> usize {
    let regenerating: Vec<_> = ctx
        .db
        .game_world_entity()
        .by_regeneration_pending()
        .filter(true)
        .filter(|e| e.power_type() == ENERGY && e.power < e.max_power)
        .collect();
    let credited = regenerating.len();
    for mut e in regenerating {
        let regen_rate = crate::spell::power_regen_rate(ctx, e.guid, ENERGY);
        e.power = regen_energy(e.power, e.max_power, ticks, regen_rate);
        e.update(ctx);
    }
    credited
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backward_clock_rebases_and_the_next_boundary_credits_again() {
        assert_eq!(clock_step(300_000, 0), ClockStep::Rebase);
        assert_eq!(clock_step(0, 1_999), ClockStep::Hold);
        assert_eq!(clock_step(0, 2_000), ClockStep::Credit(1));
    }

    #[test]
    fn a_repeated_firing_in_the_same_tick_holds() {
        assert_eq!(clock_step(2_100, 2_100), ClockStep::Hold);
        assert_eq!(clock_step(2_100, 3_999), ClockStep::Hold);
        assert_eq!(clock_step(2_100, 4_000), ClockStep::Credit(1));
    }
}
