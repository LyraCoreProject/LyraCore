use spacetimedb::{ReducerContext, ScheduleAt, TimeDuration, Timestamp};

pub(crate) fn now(ctx: &ReducerContext) -> Timestamp {
    #[cfg(feature = "conformance_test_fixture")]
    {
        if let Some(clock) = ctx.db.game_conformance_clock().id().find(0) {
            return Timestamp::from_micros_since_unix_epoch(clock.now_micros);
        }
    }
    ctx.timestamp
}

pub(crate) fn schedule_interval(ctx: &ReducerContext, interval: TimeDuration) -> ScheduleAt {
    #[cfg(feature = "conformance_test_fixture")]
    if is_virtual(ctx) {
        let future = now(ctx)
            .to_micros_since_unix_epoch()
            .saturating_add(86_400_000_000);
        return ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(future));
    }
    let _ = ctx;
    ScheduleAt::Interval(interval)
}

#[cfg(feature = "conformance_test_fixture")]
use crate::combat::game_melee_schedule;
#[cfg(feature = "conformance_test_fixture")]
use crate::creatures::tick::game_creature_move_schedule;
#[cfg(feature = "conformance_test_fixture")]
use crate::motion::game_motion_publish_schedule;
#[cfg(feature = "conformance_test_fixture")]
use crate::taxi::game_taxi_flight_schedule;
#[cfg(feature = "conformance_test_fixture")]
use spacetimedb::{reducer, table, Table};

#[cfg(feature = "conformance_test_fixture")]
#[table(accessor = game_conformance_clock)]
pub struct ConformanceClock {
    #[primary_key]
    pub id: u8,
    pub now_micros: i64,
    pub elapsed_micros: i64,
}

#[cfg(feature = "conformance_test_fixture")]
pub(crate) fn is_virtual(ctx: &ReducerContext) -> bool {
    ctx.db.game_conformance_clock().id().find(0).is_some()
}

#[cfg(feature = "conformance_test_fixture")]
fn park_schedules(ctx: &ReducerContext, future: Timestamp) {
    let motions = ctx.db.game_motion_publish_schedule();
    for mut row in motions.iter().collect::<Vec<_>>() {
        row.scheduled_at = ScheduleAt::Time(future);
        motions.scheduled_id().update(row);
    }
    let creatures = ctx.db.game_creature_move_schedule();
    for mut row in creatures.iter().collect::<Vec<_>>() {
        row.scheduled_at = ScheduleAt::Time(future);
        creatures.scheduled_id().update(row);
    }
    let melee = ctx.db.game_melee_schedule();
    for mut row in melee.iter().collect::<Vec<_>>() {
        row.scheduled_at = ScheduleAt::Time(future);
        melee.scheduled_id().update(row);
    }
}

#[cfg(feature = "conformance_test_fixture")]
#[reducer]
pub fn debug_start_virtual_clock(ctx: &ReducerContext, now_micros: i64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if is_virtual(ctx) {
        return Err("virtual clock is already running".into());
    }
    let future = now_micros
        .checked_add(86_400_000_000)
        .ok_or("virtual clock start overflows")?;
    park_schedules(ctx, Timestamp::from_micros_since_unix_epoch(future));
    ctx.db.game_conformance_clock().insert(ConformanceClock {
        id: 0,
        now_micros,
        elapsed_micros: 0,
    });
    Ok(())
}

#[cfg(feature = "conformance_test_fixture")]
#[reducer]
pub fn debug_advance_virtual_clock(ctx: &ReducerContext, delta_micros: i64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let clocks = ctx.db.game_conformance_clock();
    let mut clock = clocks.id().find(0).ok_or("virtual clock is not running")?;
    if !(1..=50_000).contains(&delta_micros)
        || clock.elapsed_micros % 50_000 + delta_micros > 50_000
    {
        return Err("virtual clock must advance to the next 50 ms boundary".into());
    }
    clock.now_micros = clock
        .now_micros
        .checked_add(delta_micros)
        .ok_or("virtual clock overflows")?;
    clock.elapsed_micros = clock
        .elapsed_micros
        .checked_add(delta_micros)
        .ok_or("virtual clock overflows")?;
    let elapsed_micros = clock.elapsed_micros;
    clocks.id().update(clock);

    if elapsed_micros % 50_000 == 0 {
        crate::motion::publish_staged(ctx);
    }
    if elapsed_micros % 100_000 == 0 {
        for schedule in ctx.db.game_melee_schedule().iter().collect::<Vec<_>>() {
            crate::combat::virtual_melee_tick(ctx, schedule);
        }
    }
    if elapsed_micros % crate::taxi::TAXI_TICK_MICROS == 0 {
        for schedule in ctx
            .db
            .game_taxi_flight_schedule()
            .iter()
            .collect::<Vec<_>>()
        {
            crate::taxi::virtual_taxi_tick(ctx, schedule);
        }
    }
    if elapsed_micros % 500_000 == 0 {
        for mut schedule in ctx
            .db
            .game_creature_move_schedule()
            .iter()
            .collect::<Vec<_>>()
        {
            schedule.scheduled_at = ScheduleAt::Interval(TimeDuration::from_micros(500_000));
            crate::creatures::tick::virtual_creature_tick(ctx, schedule);
        }
    }
    Ok(())
}
