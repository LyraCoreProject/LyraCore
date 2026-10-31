use spacetimedb::{ReducerContext, ScheduleAt, TimeDuration, Timestamp};

pub(crate) fn now(ctx: &ReducerContext) -> Timestamp {
    #[cfg(feature = "conformance_test_fixture")]
    if let Some(clock) = ctx.db.game_conformance_clock().id().find(0) {
        return Timestamp::from_micros_since_unix_epoch(clock.now_micros);
    }
    ctx.timestamp
}

pub(crate) fn schedule_interval(ctx: &ReducerContext, interval: TimeDuration) -> ScheduleAt {
    #[cfg(feature = "conformance_test_fixture")]
    if is_virtual(ctx) {
        let due = now(ctx)
            .to_micros_since_unix_epoch()
            .checked_add(interval.to_micros())
            .expect("virtual schedule interval overflows");
        return park_at(ctx, due, interval.to_micros());
    }
    let _ = ctx;
    ScheduleAt::Interval(interval)
}

pub(crate) fn schedule_time(ctx: &ReducerContext, due: Timestamp) -> ScheduleAt {
    #[cfg(feature = "conformance_test_fixture")]
    if is_virtual(ctx) {
        return park_at(ctx, due.to_micros_since_unix_epoch(), 0);
    }
    let _ = ctx;
    ScheduleAt::Time(due)
}

pub(crate) fn scheduled_time(ctx: &ReducerContext, at: Timestamp) -> Timestamp {
    #[cfg(feature = "conformance_test_fixture")]
    if is_virtual(ctx) {
        let due = ctx
            .db
            .game_conformance_arming()
            .parked_micros()
            .find(at.to_micros_since_unix_epoch())
            .expect("virtual schedule has no due time")
            .due_micros;
        return Timestamp::from_micros_since_unix_epoch(due);
    }
    let _ = ctx;
    at
}

pub(crate) fn scheduler_authorized(ctx: &ReducerContext) -> bool {
    #[cfg(feature = "conformance_test_fixture")]
    if is_virtual(ctx) {
        return ctx.sender() != ctx.database_identity()
            && crate::helpers::require_operator(ctx).is_ok();
    }
    ctx.sender() == ctx.database_identity()
}

#[cfg(feature = "conformance_test_fixture")]
use crate::items::game_temporary_enchantment;
#[cfg(feature = "conformance_test_fixture")]
use crate::native::{
    game_native_admission_tick, game_native_logon_reaper, game_native_member_stats,
};
#[cfg(feature = "conformance_test_fixture")]
use crate::tables::*;
#[cfg(feature = "conformance_test_fixture")]
use crate::transport::{game_transport_carry_schedule, game_transport_crossing};
#[cfg(feature = "conformance_test_fixture")]
use spacetimedb::{reducer, table, Table};

#[cfg(feature = "conformance_test_fixture")]
const PARK_DELAY_MICROS: i64 = 43_200_000_000;

#[cfg(feature = "conformance_test_fixture")]
#[derive(Clone)]
#[table(accessor = game_conformance_clock)]
pub struct ConformanceClock {
    #[primary_key]
    pub id: u8,
    pub now_micros: i64,
    pub elapsed_micros: i64,
}

#[cfg(feature = "conformance_test_fixture")]
#[derive(Clone)]
#[table(accessor = game_conformance_schedule)]
pub struct ConformanceSchedule {
    #[primary_key]
    pub key: u128,
    pub due_micros: i64,
    pub interval_micros: i64,
    pub parked_micros: i64,
}

#[cfg(feature = "conformance_test_fixture")]
#[table(accessor = game_conformance_arming)]
pub struct ConformanceArming {
    #[primary_key]
    pub parked_micros: i64,
    pub due_micros: i64,
    pub interval_micros: i64,
}

#[cfg(feature = "conformance_test_fixture")]
pub(crate) fn is_virtual(ctx: &ReducerContext) -> bool {
    ctx.db.game_conformance_clock().id().find(0).is_some()
}

#[cfg(feature = "conformance_test_fixture")]
fn park_at(ctx: &ReducerContext, due_micros: i64, interval_micros: i64) -> ScheduleAt {
    let arming = ctx.db.game_conformance_arming();
    let mut parked_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(PARK_DELAY_MICROS)
        .and_then(|value| value.checked_add(arming.count() as i64))
        .expect("virtual schedule parking overflows");
    while arming.parked_micros().find(parked_micros).is_some() {
        parked_micros = parked_micros
            .checked_add(1)
            .expect("virtual schedule parking overflows");
    }
    arming.insert(ConformanceArming {
        parked_micros,
        due_micros,
        interval_micros,
    });
    ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(parked_micros))
}

#[cfg(feature = "conformance_test_fixture")]
macro_rules! schedule_tables {
    ($macro:ident, $ctx:ident $(, $extra:ident)*) => {
        $macro!($ctx, 0, game_motion_publish_schedule, scheduled_id, crate::motion::publish_motion $(, $extra)*);
        $macro!($ctx, 1, game_melee_schedule, scheduled_id, crate::combat::tick_melee $(, $extra)*);
        $macro!($ctx, 2, game_taxi_flight_schedule, scheduled_id, crate::taxi::advance_taxi_flight $(, $extra)*);
        $macro!($ctx, 3, game_creature_move_schedule, scheduled_id, crate::creatures::tick::tick_creatures $(, $extra)*);
        $macro!($ctx, 4, game_weather_schedule, scheduled_id, crate::weather::tick_weather $(, $extra)*);
        $macro!($ctx, 5, game_aura_schedule, scheduled_id, crate::spell::tick_auras $(, $extra)*);
        $macro!($ctx, 6, game_ground_area_schedule, scheduled_id, crate::spell::tick_ground_areas $(, $extra)*);
        $macro!($ctx, 7, game_pending_cast, scheduled_id, crate::spell::fire_pending_cast $(, $extra)*);
        $macro!($ctx, 8, game_pending_spell_impact, scheduled_id, crate::spell::fire_spell_impact $(, $extra)*);
        $macro!($ctx, 9, game_breath_schedule, scheduled_id, crate::breath::tick_breath $(, $extra)*);
        $macro!($ctx, 10, game_transfer_reaper_schedule, scheduled_id, crate::transfer::reap_transfers $(, $extra)*);
        $macro!($ctx, 11, game_instance_reaper_schedule, scheduled_id, crate::instance::reap_instances $(, $extra)*);
        $macro!($ctx, 12, game_instance_removal, scheduled_id, crate::instance::expire_instance_removal $(, $extra)*);
        $macro!($ctx, 13, game_gateway_lease_reaper_schedule, scheduled_id, crate::gw::reap_gateway_leases $(, $extra)*);
        $macro!($ctx, 14, game_transport_carry_schedule, scheduled_id, crate::transport::carry_transport_passengers $(, $extra)*);
        $macro!($ctx, 15, game_transport_crossing, scheduled_id, crate::transport::cross_transport_map $(, $extra)*);
        $macro!($ctx, 16, game_event_reaper_schedule, scheduled_id, crate::gc::reap_movement_events $(, $extra)*);
        $macro!($ctx, 17, game_duel_schedule, scheduled_id, crate::duel::tick_duels $(, $extra)*);
        $macro!($ctx, 18, game_mail_escrow_reaper_schedule, scheduled_id, crate::mail_escrow::reap_mail_escrows $(, $extra)*);
        $macro!($ctx, 19, game_mail_timer, scheduled_id, crate::mail_timer::fire_mail_timer $(, $extra)*);
        $macro!($ctx, 20, game_auction_expiry, scheduled_id, crate::auction::expire_auction $(, $extra)*);
        $macro!($ctx, 21, game_session_reaper_schedule, scheduled_id, crate::auth::reap_sessions $(, $extra)*);
        $macro!($ctx, 22, game_meeting_stone_reminder_schedule, scheduled_id, crate::meeting_stone::remind_queued_parties $(, $extra)*);
        $macro!($ctx, 23, game_native_admission_tick, id, crate::native::native_admission_tick $(, $extra)*);
        $macro!($ctx, 24, game_native_logon_reaper, id, crate::native::native_reap_logon_addresses $(, $extra)*);
        $macro!($ctx, 25, game_native_member_stats, id, crate::native::tick_native_member_stats $(, $extra)*);
        $macro!($ctx, 26, game_pet_care_schedule, scheduled_id, crate::creatures::tick_pet_care $(, $extra)*);
        $macro!($ctx, 27, game_ranged_impact_schedule, scheduled_id, crate::combat::ranged_impact $(, $extra)*);
        $macro!($ctx, 28, game_creature_ai_relay_continuation, scheduled_id, crate::creatures::resume_relay_run $(, $extra)*);
        $macro!($ctx, 29, game_creature_ai_relay_arrival, scheduled_id, crate::creatures::resume_relay_arrival $(, $extra)*);
        $macro!($ctx, 30, game_creature_ai_summon_expiry, scheduled_id, crate::creatures::expire_eventai_summon $(, $extra)*);
        $macro!($ctx, 31, game_creature_ai_forced_despawn, scheduled_id, crate::creatures::fire_eventai_forced_despawn $(, $extra)*);
        $macro!($ctx, 32, game_temporary_enchantment, scheduled_id, crate::items::expire_temporary_enchantment $(, $extra)*);
    };
}

#[cfg(feature = "conformance_test_fixture")]
macro_rules! capture_schedule {
    ($ctx:ident, $kind:expr, $accessor:ident, $id:ident, $callback:path, $now:ident, $initial:ident) => {{
        let table = $ctx.db.$accessor();
        for mut row in table.iter().collect::<Vec<_>>() {
            let key = (($kind as u128) << 64) | u128::from(row.$id);
            let previous = $ctx.db.game_conformance_schedule().key().find(key);
            if previous.as_ref().is_some_and(|record| {
                matches!(&row.scheduled_at, ScheduleAt::Time(at) if at.to_micros_since_unix_epoch() == record.parked_micros)
            }) {
                continue;
            }
            let (due_micros, interval_micros) = match &row.scheduled_at {
                ScheduleAt::Interval(interval) => {
                    let micros = interval.to_micros();
                    if micros <= 0 {
                        return Err(format!("schedule {}:{} has a nonpositive interval", $kind, row.$id));
                    }
                    ($now.checked_add(micros).ok_or("virtual schedule overflows")?, micros)
                }
                ScheduleAt::Time(at) if $initial => (at.to_micros_since_unix_epoch(), 0),
                ScheduleAt::Time(at) => {
                    let parked = at.to_micros_since_unix_epoch();
                    let arming_table = $ctx.db.game_conformance_arming();
                    let arming = arming_table.parked_micros().find(parked)
                        .ok_or_else(|| format!("schedule {}:{} has no virtual due time", $kind, row.$id))?;
                    arming_table.parked_micros().delete(parked);
                    (arming.due_micros, arming.interval_micros)
                }
            };
            row.scheduled_at = park_at($ctx, due_micros, interval_micros);
            let ScheduleAt::Time(at) = row.scheduled_at else { unreachable!() };
            let parked_micros = at.to_micros_since_unix_epoch();
            table.$id().update(row);
            let record = ConformanceSchedule { key, due_micros, interval_micros, parked_micros };
            let schedules = $ctx.db.game_conformance_schedule();
            if let Some(previous) = previous {
                $ctx.db.game_conformance_arming().parked_micros().delete(previous.parked_micros);
                schedules.key().update(record);
            } else {
                schedules.insert(record);
            }
        }
    }};
}

#[cfg(feature = "conformance_test_fixture")]
fn capture_schedules(ctx: &ReducerContext, now_micros: i64, initial: bool) -> Result<(), String> {
    schedule_tables!(capture_schedule, ctx, now_micros, initial);
    Ok(())
}

#[cfg(feature = "conformance_test_fixture")]
trait ScheduledResult {
    fn finish(self) -> Result<(), String>;
}

#[cfg(feature = "conformance_test_fixture")]
impl ScheduledResult for () {
    fn finish(self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(feature = "conformance_test_fixture")]
impl ScheduledResult for Result<(), String> {
    fn finish(self) -> Result<(), String> {
        self
    }
}

#[cfg(feature = "conformance_test_fixture")]
macro_rules! fire_schedule {
    ($ctx:ident, $kind:expr, $accessor:ident, $id:ident, $callback:path, $which:ident, $row_id:ident, $interval:ident, $parked:ident) => {{
        if $which == $kind {
            let table = $ctx.db.$accessor();
            if let Some(mut row) = table.$id().find($row_id) {
                if $interval > 0 {
                    row.scheduled_at = ScheduleAt::Interval(TimeDuration::from_micros($interval));
                }
                $callback($ctx, row).finish()?;
                if $interval == 0 && table.$id().find($row_id).is_some_and(|current| {
                    matches!(current.scheduled_at, ScheduleAt::Time(at) if at.to_micros_since_unix_epoch() == $parked)
                }) {
                    table.$id().delete($row_id);
                }
                if $interval > 0 && table.$id().find($row_id).is_none() {
                    $ctx.db.game_conformance_schedule().key().delete((($kind as u128) << 64) | u128::from($row_id));
                }
                if $interval == 0 || table.$id().find($row_id).is_none() {
                    $ctx.db.game_conformance_arming().parked_micros().delete($parked);
                }
            } else {
                $ctx.db.game_conformance_schedule().key().delete((($kind as u128) << 64) | u128::from($row_id));
                $ctx.db.game_conformance_arming().parked_micros().delete($parked);
            }
            return Ok(());
        }
    }};
}

#[cfg(feature = "conformance_test_fixture")]
fn fire_schedule(
    ctx: &ReducerContext,
    key: u128,
    interval_micros: i64,
    parked_micros: i64,
) -> Result<(), String> {
    let kind = (key >> 64) as u8;
    let id = key as u64;
    schedule_tables!(fire_schedule, ctx, kind, id, interval_micros, parked_micros);
    Err(format!("unknown virtual schedule kind {kind}"))
}

#[cfg(feature = "conformance_test_fixture")]
#[reducer]
pub fn debug_start_virtual_clock(ctx: &ReducerContext, now_micros: i64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if is_virtual(ctx) {
        return Err("virtual clock is already running".into());
    }
    ctx.db.game_conformance_clock().insert(ConformanceClock {
        id: 0,
        now_micros,
        elapsed_micros: 0,
    });
    capture_schedules(ctx, now_micros, true)
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
    let target = clock
        .now_micros
        .checked_add(delta_micros)
        .ok_or("virtual clock overflows")?;
    let elapsed = clock
        .elapsed_micros
        .checked_add(delta_micros)
        .ok_or("virtual clock overflows")?;
    capture_schedules(ctx, clock.now_micros, false)?;
    let mut fired = 0;
    while let Some(record) = ctx
        .db
        .game_conformance_schedule()
        .iter()
        .filter(|record| record.due_micros <= target)
        // Equal deadlines use table kind then row id; live scheduling does not promise this order.
        .min_by_key(|record| (record.due_micros, record.key))
    {
        fired += 1;
        if fired > 4096 {
            return Err("virtual schedule fired more than 4096 times in one advance".into());
        }
        let fired_at = record.due_micros.max(clock.now_micros);
        clock.now_micros = fired_at;
        clock.elapsed_micros = elapsed - (target - fired_at);
        clocks.id().update(clock.clone());
        let schedules = ctx.db.game_conformance_schedule();
        if record.interval_micros > 0 {
            schedules.key().update(ConformanceSchedule {
                due_micros: record
                    .due_micros
                    .checked_add(record.interval_micros)
                    .ok_or("virtual schedule overflows")?,
                ..record.clone()
            });
        } else {
            schedules.key().delete(record.key);
        }
        fire_schedule(
            ctx,
            record.key,
            record.interval_micros,
            record.parked_micros,
        )?;
        capture_schedules(ctx, clock.now_micros, false)?;
    }
    clock.now_micros = target;
    clock.elapsed_micros = elapsed;
    clocks.id().update(clock);
    Ok(())
}

#[cfg(feature = "conformance_test_fixture")]
#[reducer]
pub fn debug_stage_virtual_spell_impact(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    damage: i32,
    delay_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !is_virtual(ctx) || delay_micros <= 0 {
        return Err("spell impact requires a running virtual clock and positive delay".into());
    }
    let due = now(ctx)
        .checked_add(TimeDuration::from_micros(delay_micros))
        .ok_or("spell impact due time overflows")?;
    ctx.db
        .game_pending_spell_impact()
        .insert(crate::PendingSpellImpact {
            scheduled_id: 0,
            scheduled_at: schedule_time(ctx, due),
            caster_guid,
            target_guid,
            spell_id: 6136,
            school_index: 0,
            is_crit: false,
            resisted: 0,
            after_resist: damage,
            triggered: false,
        });
    Ok(())
}
