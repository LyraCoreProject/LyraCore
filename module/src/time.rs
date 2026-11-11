use spacetimedb::{ReducerContext, ScheduleAt, TimeDuration, Timestamp};

pub(crate) fn now(ctx: &ReducerContext) -> Timestamp {
    #[cfg(feature = "conformance_test_fixture")]
    if let Some(clock) = ctx.db.game_conformance_clock().id().find(0) {
        return Timestamp::from_micros_since_unix_epoch(clock.now_micros);
    }
    ctx.timestamp
}

/// Realm Clock milliseconds since the Unix epoch; a pre-epoch clock reads as 0.
pub(crate) fn now_ms(ctx: &ReducerContext) -> u64 {
    now(ctx).to_micros_since_unix_epoch().max(0) as u64 / 1_000
}

/// Realm Clock seconds since the Unix epoch; a pre-epoch clock reads as 0.
pub(crate) fn now_secs(ctx: &ReducerContext) -> u64 {
    now(ctx).to_micros_since_unix_epoch().max(0) as u64 / 1_000_000
}

/// A due time far enough ahead that a timer armed with it never fires.
pub(crate) fn timer_never(ctx: &ReducerContext) -> Timestamp {
    now(ctx)
        .checked_add(TimeDuration::from_micros(u32::MAX as i64 * 1_000_000))
        .unwrap_or(now(ctx))
}

/// Keeps a scheduled table declared in the same file off the conformance virtual clock.
/// `module/build.rs` registers every other scheduled table with it. An exempt table schedules
/// from the host clock (`ctx.timestamp`), not through `schedule_time` or `schedule_interval`.
#[macro_export]
macro_rules! virtual_clock_exempt {
    ($table:ident) => {};
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
#[table(accessor = game_conformance_schedule_revision)]
pub struct ConformanceScheduleRevision {
    #[primary_key]
    pub id: u8,
    pub revision: u64,
}

#[cfg(feature = "conformance_test_fixture")]
fn schedule_revision(ctx: &ReducerContext) -> u64 {
    ctx.db
        .game_conformance_schedule_revision()
        .id()
        .find(0)
        .map_or(0, |row| row.revision)
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
    let revisions = ctx.db.game_conformance_schedule_revision();
    let previous = revisions.id().find(0);
    let row = ConformanceScheduleRevision {
        id: 0,
        revision: previous.as_ref().map_or(1, |row| row.revision + 1),
    };
    if previous.is_some() {
        revisions.id().update(row);
    } else {
        revisions.insert(row);
    }
    arming.insert(ConformanceArming {
        parked_micros,
        due_micros,
        interval_micros,
    });
    ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(parked_micros))
}

#[cfg(feature = "conformance_test_fixture")]
include!(concat!(env!("OUT_DIR"), "/virtual_schedules.rs"));

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
    virtual_schedules!(capture_schedule, ctx, now_micros, initial);
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
    virtual_schedules!(fire_schedule, ctx, kind, id, interval_micros, parked_micros);
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
    let clock = clocks.id().find(0).ok_or("virtual clock is not running")?;
    if !(1..=50_000).contains(&delta_micros)
        || clock.elapsed_micros % 50_000 + delta_micros > 50_000
    {
        return Err("virtual clock must advance to the next 50 ms boundary".into());
    }
    advance_virtual_clock(ctx, delta_micros)
}

#[cfg(feature = "conformance_test_fixture")]
pub(crate) fn advance_virtual_clock(ctx: &ReducerContext, delta: i64) -> Result<(), String> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let clocks = ctx.db.game_conformance_clock();
    let mut clock = clocks.id().find(0).ok_or("virtual clock is not running")?;
    let target = clock
        .now_micros
        .checked_add(delta)
        .ok_or("virtual clock overflows")?;
    let elapsed = clock
        .elapsed_micros
        .checked_add(delta)
        .ok_or("virtual clock overflows")?;
    capture_schedules(ctx, clock.now_micros, false)?;
    let schedules = ctx.db.game_conformance_schedule();
    let calendar = || {
        schedules
            .iter()
            .map(|row| Reverse((row.due_micros, row.key)))
            .collect::<BinaryHeap<_>>()
    };
    let mut queue = calendar();
    let start = clock.now_micros;
    let mut boundary = 0;
    let mut fired = 0;
    while let Some(Reverse((due, key))) = queue.pop() {
        if due > target {
            break;
        }
        let Some(record) = schedules.key().find(key) else {
            continue;
        };
        if record.due_micros != due {
            continue;
        }
        let next_boundary = (due - start - 1).max(0) / 50_000;
        if next_boundary != boundary {
            boundary = next_boundary;
            fired = 0;
        }
        fired += 1;
        if fired > 4096 {
            return Err("roll batch schedule limit exceeded".into());
        }
        let fired_at = due.max(clock.now_micros);
        clock.now_micros = fired_at;
        clock.elapsed_micros = elapsed - (target - fired_at);
        clocks.id().update(clock.clone());
        if record.interval_micros > 0 {
            schedules.key().update(ConformanceSchedule {
                due_micros: due
                    .checked_add(record.interval_micros)
                    .ok_or("virtual schedule overflows")?,
                ..record.clone()
            });
        } else {
            schedules.key().delete(key);
        }
        let revision = schedule_revision(ctx);
        fire_schedule(ctx, key, record.interval_micros, record.parked_micros)?;
        if schedule_revision(ctx) != revision {
            capture_schedules(ctx, clock.now_micros, false)?;
            queue = calendar();
        } else if let Some(next) = schedules.key().find(key) {
            queue.push(Reverse((next.due_micros, key)));
        }
    }
    clock.now_micros = target;
    clock.elapsed_micros = elapsed;
    clocks.id().update(clock);
    Ok(())
}
