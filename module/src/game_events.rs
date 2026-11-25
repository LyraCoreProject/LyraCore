// vmangos `GameEventMgr` spawn rules; read docs/schema.md#game-events first.

use std::collections::BTreeMap;

use spacetimedb::{
    log, reducer, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration,
};

/// One vmangos `game_event` row: its calendar on the Realm Clock.
#[table(accessor = game_event)]
pub struct GameEvent {
    #[primary_key]
    pub event_entry: u32,
    pub start_secs: i64,
    pub end_secs: i64,
    pub occurrence_mins: u32,
    pub length_mins: u32,
    // False for a vmangos `hardcoded` or `disabled` event: only an Operator runs it.
    pub follows_calendar: bool,
    pub description: String,
}

/// A Game Event Link from vmangos `game_event_creature` or `game_event_gameobject`.
#[table(
    accessor = game_event_link,
    index(accessor = by_spawn, btree(columns = [spawn_guid])),
    index(accessor = by_event, btree(columns = [event_entry]))
)]
pub struct GameEventLink {
    #[primary_key]
    pub link_id: u64,
    // A creature spawn's source guid or a world GameObject's guid.
    pub spawn_guid: u64,
    pub event_entry: u32,
    // A negative vmangos event id: the spawn shows while the event does not run.
    pub while_inactive: bool,
}

/// A top-level GameObject pool that rolls only while its Game Event runs.
#[table(accessor = game_event_pool, index(accessor = by_event, btree(columns = [event_entry])))]
pub struct GameEventPool {
    #[primary_key]
    pub pool_id: u32,
    pub event_entry: u32,
}

/// The state the world was last brought to for an event; a missing row has not been applied.
#[table(accessor = game_event_status)]
pub struct GameEventStatus {
    #[primary_key]
    pub event_entry: u32,
    pub state: GameEventState,
}

/// An Operator's start or stop, which holds until the Operator clears it.
#[table(accessor = game_event_override)]
pub struct GameEventOverride {
    #[primary_key]
    pub event_entry: u32,
    pub running: bool,
}

#[table(accessor = game_event_schedule, scheduled(tick_game_events))]
pub struct GameEventSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameEventState {
    Active,
    Inactive,
    /// Neither the calendar nor an Operator runs or stops it, so no spawn tied to it shows.
    Unscheduled,
}

// One pass a minute keeps each calendar boundary within a minute and corrects a missed one.
pub(crate) const GAME_EVENT_TICK_MICROS: i64 = 60_000_000;

// One pass changes at most this many spawns; the next pass carries on where it stopped.
const SPAWN_CHANGES_PER_PASS: usize = 500;

const DAY_SECS: i64 = 86_400;
const YEAR_MINS: u32 = 525_600;

/// When an event runs, as vmangos `CheckOneGameEvent` reads `game_event`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Occurrence {
    pub start_secs: i64,
    pub end_secs: i64,
    pub occurrence_mins: u32,
    pub length_mins: u32,
}

impl GameEvent {
    fn occurrence(&self) -> Occurrence {
        Occurrence {
            start_secs: self.start_secs,
            end_secs: self.end_secs,
            occurrence_mins: self.occurrence_mins,
            length_mins: self.length_mins,
        }
    }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The 29 Februaries from `start_secs` up to `now_secs`, which a yearly event skips over to keep
/// its calendar date; the deliberate deviations in `conformance/features/world/game_events.toml`.
fn leap_days(start_secs: i64, now_secs: i64) -> i64 {
    let year_of =
        |secs: i64| lyracore_shared::calendar::civil_from_days(secs.div_euclid(DAY_SECS)).0;
    (year_of(start_secs)..=year_of(now_secs))
        .filter(|&year| is_leap_year(year))
        .map(|year| lyracore_shared::calendar::days_from_civil(year, 2, 29) * DAY_SECS)
        .filter(|&leap_day| start_secs <= leap_day && leap_day < now_secs)
        .count() as i64
}

/// Whether the calendar runs the event at `now_secs`: inside its start and end, within its length
/// of the latest occurrence.
pub(crate) fn in_window(occurrence: Occurrence, now_secs: i64) -> bool {
    let Occurrence {
        start_secs,
        end_secs,
        occurrence_mins,
        length_mins,
    } = occurrence;
    if occurrence_mins == 0 || length_mins == 0 || now_secs < start_secs || now_secs >= end_secs {
        return false;
    }
    let skipped = if occurrence_mins == YEAR_MINS && length_mins < YEAR_MINS {
        leap_days(start_secs, now_secs) * DAY_SECS
    } else {
        0
    };
    (now_secs - start_secs - skipped).rem_euclid(i64::from(occurrence_mins) * 60)
        < i64::from(length_mins) * 60
}

pub(crate) fn event_state(
    event: Option<&GameEvent>,
    running: Option<bool>,
    now_secs: i64,
) -> GameEventState {
    let Some(event) = event else {
        return GameEventState::Unscheduled;
    };
    match running {
        Some(true) => GameEventState::Active,
        Some(false) => GameEventState::Inactive,
        None if !event.follows_calendar => GameEventState::Unscheduled,
        None if in_window(event.occurrence(), now_secs) => GameEventState::Active,
        None => GameEventState::Inactive,
    }
}

/// Whether a spawn shows, from each of its links' `while_inactive` and event state. One running
/// event of its plain links is enough; every event it shows without must be stopped.
pub(crate) fn shows_for(links: impl IntoIterator<Item = (bool, GameEventState)>) -> bool {
    let mut needs_running = false;
    let mut running = false;
    for (while_inactive, state) in links {
        if while_inactive {
            if state != GameEventState::Inactive {
                return false;
            }
        } else {
            needs_running = true;
            running |= state == GameEventState::Active;
        }
    }
    !needs_running || running
}

fn now_secs(ctx: &ReducerContext) -> i64 {
    crate::time::now(ctx)
        .to_micros_since_unix_epoch()
        .div_euclid(1_000_000)
}

fn state_of(ctx: &ReducerContext, event_entry: u32, now_secs: i64) -> GameEventState {
    event_state(
        ctx.db.game_event().event_entry().find(event_entry).as_ref(),
        ctx.db
            .game_event_override()
            .event_entry()
            .find(event_entry)
            .map(|forced| forced.running),
        now_secs,
    )
}

/// Whether the creature spawn source or world GameObject `spawn_guid` shows now.
pub(crate) fn spawn_shows(ctx: &ReducerContext, spawn_guid: u64) -> bool {
    let now = now_secs(ctx);
    shows_for(
        ctx.db
            .game_event_link()
            .by_spawn()
            .filter(spawn_guid)
            .map(|link| (link.while_inactive, state_of(ctx, link.event_entry, now))),
    )
}

/// Whether the top-level GameObject pool `pool_id` may roll now.
pub(crate) fn pool_shows(ctx: &ReducerContext, pool_id: u32) -> bool {
    ctx.db
        .game_event_pool()
        .pool_id()
        .find(pool_id)
        .is_none_or(|pool| state_of(ctx, pool.event_entry, now_secs(ctx)) == GameEventState::Active)
}

/// Every event's state at one instant.
struct Calendar {
    states: BTreeMap<u32, GameEventState>,
}

impl Calendar {
    fn load(ctx: &ReducerContext) -> Self {
        let now = now_secs(ctx);
        let overrides = ctx.db.game_event_override();
        let states = ctx
            .db
            .game_event()
            .iter()
            .map(|event| {
                let running = overrides
                    .event_entry()
                    .find(event.event_entry)
                    .map(|forced| forced.running);
                (event.event_entry, event_state(Some(&event), running, now))
            })
            .collect();
        Self { states }
    }

    fn state(&self, event_entry: u32) -> GameEventState {
        self.states
            .get(&event_entry)
            .copied()
            .unwrap_or(GameEventState::Unscheduled)
    }

    fn spawn_shows(&self, ctx: &ReducerContext, spawn_guid: u64) -> bool {
        shows_for(
            ctx.db
                .game_event_link()
                .by_spawn()
                .filter(spawn_guid)
                .map(|link| (link.while_inactive, self.state(link.event_entry))),
        )
    }
}

/// Brings every spawn of an event whose state changed since it was last applied to that state,
/// through the spawn lifecycle; docs/schema.md#game-events.
pub(crate) fn reconcile(ctx: &ReducerContext) {
    let calendar = Calendar::load(ctx);
    let statuses = ctx.db.game_event_status();
    let mut budget = SPAWN_CHANGES_PER_PASS;
    for (&event_entry, &state) in &calendar.states {
        if statuses
            .event_entry()
            .find(event_entry)
            .is_some_and(|status| status.state == state)
        {
            continue;
        }
        if !apply(ctx, &calendar, event_entry, &mut budget) {
            log::info!(
                "game events: {SPAWN_CHANGES_PER_PASS} spawn changes reached; event \
                 {event_entry} continues on the next pass"
            );
            return;
        }
        let status = GameEventStatus { event_entry, state };
        if statuses.event_entry().find(event_entry).is_some() {
            statuses.event_entry().update(status);
        } else {
            statuses.insert(status);
        }
    }
}

/// Shows or hides each spawn and pool tied to the event; false when `budget` ran out first.
fn apply(ctx: &ReducerContext, calendar: &Calendar, event_entry: u32, budget: &mut usize) -> bool {
    let spawns: Vec<u64> = ctx
        .db
        .game_event_link()
        .by_event()
        .filter(event_entry)
        .map(|link| link.spawn_guid)
        .collect();
    for spawn_guid in spawns {
        if *budget == 0 {
            return false;
        }
        let shows = calendar.spawn_shows(ctx, spawn_guid);
        let changed = if crate::gameobject::is_gameobject_guid(spawn_guid) {
            usize::from(crate::gameobject::show_for_events(ctx, spawn_guid, shows))
        } else {
            crate::creatures::show_spawn_for_events(ctx, spawn_guid, shows)
        };
        *budget = budget.saturating_sub(changed);
    }
    let pools: Vec<u32> = ctx
        .db
        .game_event_pool()
        .by_event()
        .filter(event_entry)
        .map(|pool| pool.pool_id)
        .collect();
    pools.is_empty()
        || crate::gameobject::show_event_pools(
            ctx,
            &pools,
            calendar.state(event_entry) == GameEventState::Active,
            budget,
        )
}

#[reducer]
pub fn tick_game_events(ctx: &ReducerContext, _schedule: GameEventSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    reconcile(ctx);
}

pub(crate) fn rearm_schedule(ctx: &ReducerContext) {
    let schedule = ctx.db.game_event_schedule();
    let stale: Vec<u64> = schedule.iter().map(|row| row.scheduled_id).collect();
    for scheduled_id in stale {
        schedule.scheduled_id().delete(scheduled_id);
    }
    schedule.insert(GameEventSchedule {
        scheduled_id: 0,
        scheduled_at: crate::time::schedule_interval(
            ctx,
            TimeDuration::from_micros(GAME_EVENT_TICK_MICROS),
        ),
    });
}

fn hold(ctx: &ReducerContext, event_entry: u32, running: Option<bool>) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if ctx
        .db
        .game_event()
        .event_entry()
        .find(event_entry)
        .is_none()
    {
        return Err(format!("game event {event_entry} does not exist"));
    }
    let overrides = ctx.db.game_event_override();
    overrides.event_entry().delete(event_entry);
    if let Some(running) = running {
        overrides.insert(GameEventOverride {
            event_entry,
            running,
        });
    }
    reconcile(ctx);
    Ok(())
}

/// Runs the event until `debug_event_clear`, whatever its calendar says.
#[reducer]
pub fn debug_event_start(ctx: &ReducerContext, event_entry: u32) -> Result<(), String> {
    hold(ctx, event_entry, Some(true))
}

/// Stops the event until `debug_event_clear`, whatever its calendar says.
#[reducer]
pub fn debug_event_stop(ctx: &ReducerContext, event_entry: u32) -> Result<(), String> {
    hold(ctx, event_entry, Some(false))
}

/// Hands the event back to its calendar.
#[reducer]
pub fn debug_event_clear(ctx: &ReducerContext, event_entry: u32) -> Result<(), String> {
    hold(ctx, event_entry, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60;
    const HOUR: i64 = 3_600;

    fn yearly(start_secs: i64, length_mins: u32) -> Occurrence {
        Occurrence {
            start_secs,
            end_secs: 2_145_916_800,
            occurrence_mins: YEAR_MINS,
            length_mins,
        }
    }

    fn event(follows_calendar: bool, occurrence: Occurrence) -> GameEvent {
        GameEvent {
            event_entry: 7,
            start_secs: occurrence.start_secs,
            end_secs: occurrence.end_secs,
            occurrence_mins: occurrence.occurrence_mins,
            length_mins: occurrence.length_mins,
            follows_calendar,
            description: String::new(),
        }
    }

    // 2020-01-02 21:00 UTC, vmangos "Night": every 1440 minutes for 720.
    const NIGHT_START: i64 = 1_577_998_800;

    fn night() -> Occurrence {
        Occurrence {
            start_secs: NIGHT_START,
            end_secs: 2_145_916_800,
            occurrence_mins: 1_440,
            length_mins: 720,
        }
    }

    #[test]
    fn a_repeating_event_runs_for_its_length_from_each_occurrence() {
        // 2026-11-25 21:00 UTC opens a night and 2026-11-26 09:00 UTC closes it.
        let opens = 1_795_640_400;
        let closes = 1_795_683_600;
        assert!(!in_window(night(), opens - MINUTE));
        assert!(in_window(night(), opens));
        assert!(in_window(night(), closes - 1));
        assert!(!in_window(night(), closes));
    }

    #[test]
    fn an_event_runs_neither_before_its_start_nor_from_its_end() {
        let ended = Occurrence {
            end_secs: NIGHT_START + 2 * 24 * HOUR,
            ..night()
        };
        assert!(!in_window(ended, NIGHT_START - 1));
        assert!(in_window(ended, NIGHT_START + 24 * HOUR));
        assert!(!in_window(ended, NIGHT_START + 2 * 24 * HOUR));
    }

    #[test]
    fn an_event_with_no_length_or_no_occurrence_never_runs() {
        for occurrence in [
            Occurrence {
                length_mins: 0,
                ..night()
            },
            Occurrence {
                occurrence_mins: 0,
                ..night()
            },
        ] {
            assert!(!in_window(occurrence, NIGHT_START + MINUTE));
        }
    }

    #[test]
    fn a_yearly_event_comes_back_on_its_calendar_date_after_a_leap_day() {
        // 2021-03-01 00:00 UTC for one day; 2024 has a 29 February before the date comes back.
        let festival = yearly(1_614_556_800, 1_440);
        let leap_day_noon = 1_709_208_000;
        let first_of_march_2024 = 1_709_251_200;
        assert!(!in_window(festival, leap_day_noon));
        assert!(in_window(festival, first_of_march_2024));
        assert!(in_window(festival, first_of_march_2024 + 23 * HOUR));
        assert!(!in_window(festival, first_of_march_2024 + 24 * HOUR));
    }

    #[test]
    fn a_yearly_event_skips_no_leap_day_before_its_start() {
        // 2020-06-21 20:00 UTC, vmangos Midsummer, comes back 2026-06-21 20:00 UTC.
        let midsummer = yearly(1_592_769_600, 20_160);
        let back = 1_782_072_000;
        assert!(!in_window(midsummer, back - 1));
        assert!(in_window(midsummer, back));
    }

    #[test]
    fn the_calendar_runs_a_scheduled_event_and_never_a_scripted_one() {
        let inside = NIGHT_START + MINUTE;
        let outside = NIGHT_START + 13 * HOUR;
        let scheduled = event(true, night());
        assert_eq!(
            event_state(Some(&scheduled), None, inside),
            GameEventState::Active
        );
        assert_eq!(
            event_state(Some(&scheduled), None, outside),
            GameEventState::Inactive
        );
        let scripted = event(false, night());
        assert_eq!(
            event_state(Some(&scripted), None, inside),
            GameEventState::Unscheduled
        );
        assert_eq!(event_state(None, None, inside), GameEventState::Unscheduled);
    }

    #[test]
    fn an_operator_start_or_stop_holds_over_the_calendar() {
        let outside = NIGHT_START + 13 * HOUR;
        let inside = NIGHT_START + MINUTE;
        let scheduled = event(true, night());
        let scripted = event(false, night());
        assert_eq!(
            event_state(Some(&scheduled), Some(true), outside),
            GameEventState::Active
        );
        assert_eq!(
            event_state(Some(&scheduled), Some(false), inside),
            GameEventState::Inactive
        );
        assert_eq!(
            event_state(Some(&scripted), Some(true), outside),
            GameEventState::Active
        );
        assert_eq!(
            event_state(None, Some(true), inside),
            GameEventState::Unscheduled
        );
    }

    #[test]
    fn a_spawn_without_links_always_shows() {
        assert!(shows_for([]));
    }

    #[test]
    fn a_linked_spawn_shows_only_while_its_event_runs() {
        use GameEventState::*;
        assert!(shows_for([(false, Active)]));
        assert!(!shows_for([(false, Inactive)]));
        assert!(!shows_for([(false, Unscheduled)]));
    }

    #[test]
    fn a_negative_link_shows_only_while_its_event_is_stopped() {
        use GameEventState::*;
        assert!(shows_for([(true, Inactive)]));
        assert!(!shows_for([(true, Active)]));
        assert!(!shows_for([(true, Unscheduled)]));
    }

    #[test]
    fn a_spawn_of_several_events_needs_one_running_and_none_it_hides_for() {
        use GameEventState::*;
        assert!(shows_for([(false, Inactive), (false, Active)]));
        assert!(!shows_for([(true, Inactive), (true, Active)]));
        assert!(!shows_for([(false, Active), (true, Active)]));
        assert!(shows_for([(false, Active), (true, Inactive)]));
    }
}
