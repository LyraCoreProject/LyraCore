// vmangos `GameEventMgr` spawn, creature data, quest, vendor and mail rules; read
// docs/schema.md#game-events first.

use std::collections::{BTreeMap, BTreeSet};

use spacetimedb::{
    log, reducer, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration,
};

mod darkmoon_faire;
mod mail;

/// One vmangos `game_event` row: its calendar on the Realm Clock.
#[table(accessor = game_event)]
pub struct GameEvent {
    #[primary_key]
    pub event_entry: u32,
    pub start_secs: i64,
    pub end_secs: i64,
    pub occurrence_mins: u32,
    pub length_mins: u32,
    // vmangos `hardcoded`: its script runs it; only the Darkmoon Faire has one here.
    pub scripted: bool,
    // vmangos `disabled`, or left out of patch 1.12: nothing runs or stops it.
    pub disabled: bool,
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

/// A quest from vmangos `game_event_quest`: its givers offer it only while its event runs.
#[table(accessor = game_event_quest)]
pub struct GameEventQuest {
    #[primary_key]
    pub quest_entry: u32,
    pub event_entry: u32,
}

/// A vendor item whose vmangos `npc_vendor` condition is `CONDITION_ACTIVE_GAME_EVENT`.
#[table(
    accessor = game_event_vendor_item,
    index(accessor = by_vendor, btree(columns = [creature_entry]))
)]
pub struct GameEventVendorItem {
    #[primary_key]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub event_entry: u32,
}

/// A vmangos `game_event_creature_data` row: what a creature spawn becomes while its event runs.
#[table(
    accessor = game_event_creature_data,
    index(accessor = by_spawn, btree(columns = [spawn_guid])),
    index(accessor = by_event, btree(columns = [event_entry]))
)]
#[derive(Clone, Debug, PartialEq)]
pub struct GameEventCreatureData {
    #[primary_key]
    pub data_id: u64,
    // A creature spawn's source guid, as in `game_event_link`.
    pub spawn_guid: u64,
    pub event_entry: u32,
    // The creature template it takes; 0 keeps its own.
    pub creature_entry: u32,
    // The item entries its virtual equipment shows; all three 0 keep its own.
    pub main_hand: u32,
    pub off_hand: u32,
    pub ranged: u32,
    // Cast on itself as the event starts and taken off as it stops; 0 for none.
    pub start_spell: u32,
    // Cast on itself as the event stops; 0 for none.
    pub end_spell: u32,
}

impl GameEventCreatureData {
    pub(crate) fn equipment(&self) -> Option<[u32; 3]> {
        let items = [self.main_hand, self.off_hand, self.ranged];
        (items != [0; 3]).then_some(items)
    }
}

/// The state an event's spawns are being brought to, and how far that has come.
#[table(accessor = game_event_status)]
#[derive(Clone, Debug, PartialEq)]
pub struct GameEventStatus {
    #[primary_key]
    pub event_entry: u32,
    pub state: GameEventState,
    // The lowest link, pool and creature data row not yet brought to `state`; the type's maximum
    // once all are.
    pub next_link_id: u64,
    pub next_pool_id: u32,
    #[default(u64::MAX)]
    pub next_creature_data_id: u64,
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

// No Package claims or reads Game Event rows.
crate::outside_table_catalog!(
    game_event,
    game_event_link,
    game_event_pool,
    game_event_quest,
    game_event_vendor_item,
    game_event_creature_data,
    game_event_status,
    game_event_override,
    game_event_schedule
);

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameEventState {
    Active,
    Inactive,
    /// Neither the calendar nor an Operator runs or stops it, so no spawn tied to it shows.
    Unscheduled,
}

// One pass a minute keeps each calendar boundary within a minute and corrects a missed one.
pub(crate) const GAME_EVENT_TICK_MICROS: i64 = 60_000_000;

// One pass changes at most this many spawns, pools and creatures; the next pass carries on there.
const CHANGES_PER_PASS: usize = 500;

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

impl GameEventStatus {
    fn settled(&self) -> bool {
        self.next_link_id == u64::MAX
            && self.next_pool_id == u32::MAX
            && self.next_creature_data_id == u64::MAX
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

/// vmangos `GameEventMgr::Update`: a disabled or zero-length event is not checked, a scripted
/// one follows its script or stays stopped without one, and an Operator's start or stop wins.
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
        None if event.disabled || event.length_mins == 0 => GameEventState::Unscheduled,
        None if event.scripted => {
            darkmoon_faire::state(event.event_entry, now_secs).unwrap_or(GameEventState::Inactive)
        }
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

/// The work toward `state`: a status already heading there keeps its progress, any other starts
/// again from the first link, pool and creature data row.
fn retarget(
    status: Option<GameEventStatus>,
    event_entry: u32,
    state: GameEventState,
) -> GameEventStatus {
    match status {
        Some(status) if status.state == state => status,
        _ => GameEventStatus {
            event_entry,
            state,
            next_link_id: 0,
            next_pool_id: 0,
            next_creature_data_id: 0,
        },
    }
}

/// The part of a pass's budget each of `pending` events gets first, so one large event cannot
/// hold back the others; what is left goes to them in order.
fn fair_share(budget: usize, pending: usize) -> usize {
    (budget / pending.max(1)).max(1)
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

/// vmangos `GameEventMgr::IsActiveEvent`, which `CONDITION_ACTIVE_GAME_EVENT` reads.
pub(crate) fn running(ctx: &ReducerContext, event_entry: u32) -> bool {
    state_of(ctx, event_entry, crate::time::now_secs(ctx) as i64) == GameEventState::Active
}

/// A loot row's vmangos `CONDITION_ACTIVE_GAME_EVENT` on `event_entry`, or its reversed form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EventCondition {
    pub event_entry: u32,
    pub reversed: bool,
}

impl EventCondition {
    /// The condition a row's `event_entry` and `event_reversed` columns store; 0 is none.
    pub(crate) fn of_row(event_entry: u32, reversed: bool) -> Option<Self> {
        (event_entry != 0).then_some(Self {
            event_entry,
            reversed,
        })
    }

    pub(crate) fn holds(self, ctx: &ReducerContext) -> bool {
        running(ctx, self.event_entry) != self.reversed
    }
}

/// vmangos `Quest::IsActive`: whether givers offer `quest_entry` now.
pub(crate) fn quest_offered(ctx: &ReducerContext, quest_entry: u32) -> bool {
    ctx.db
        .game_event_quest()
        .quest_entry()
        .find(quest_entry)
        .is_none_or(|quest| running(ctx, quest.event_entry))
}

/// The items vendor `creature_entry` neither lists nor sells now, because their event does not run.
pub(crate) fn withheld_vendor_items(ctx: &ReducerContext, creature_entry: u32) -> BTreeSet<u32> {
    ctx.db
        .game_event_vendor_item()
        .by_vendor()
        .filter(creature_entry)
        .filter(|item| !running(ctx, item.event_entry))
        .map(|item| item.item_entry)
        .collect()
}

/// Whether any Game Event Link exists; until the `events` family is imported, none does.
pub(crate) fn links_exist(ctx: &ReducerContext) -> bool {
    ctx.db.game_event_link().count() > 0
}

/// Whether the creature spawn source or world GameObject `spawn_guid` shows now.
pub(crate) fn spawn_shows(ctx: &ReducerContext, spawn_guid: u64) -> bool {
    let now = crate::time::now_secs(ctx) as i64;
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
    let now = crate::time::now_secs(ctx) as i64;
    ctx.db
        .game_event_pool()
        .pool_id()
        .find(pool_id)
        .is_none_or(|pool| state_of(ctx, pool.event_entry, now) == GameEventState::Active)
}

/// The data rows of the creature spawn's running events, in the order they apply.
fn worn_data(
    ctx: &ReducerContext,
    spawn_guid: u64,
    state: impl Fn(u32) -> GameEventState,
) -> Vec<GameEventCreatureData> {
    let mut worn: Vec<GameEventCreatureData> = ctx
        .db
        .game_event_creature_data()
        .by_spawn()
        .filter(spawn_guid)
        .filter(|data| state(data.event_entry) == GameEventState::Active)
        .collect();
    worn.sort_unstable_by_key(|data| data.data_id);
    worn
}

/// vmangos `GetCreatureUpdateDataForActiveEvent`: the data rows the creature spawn source
/// `spawn_guid` wears now; see the deliberate deviations in
/// `conformance/features/world/game_events.toml`.
pub(crate) fn creature_data_worn(
    ctx: &ReducerContext,
    spawn_guid: u64,
) -> Vec<GameEventCreatureData> {
    let now = crate::time::now_secs(ctx) as i64;
    worn_data(ctx, spawn_guid, |event_entry| {
        state_of(ctx, event_entry, now)
    })
}

/// Every event's state at one instant.
struct Calendar {
    states: BTreeMap<u32, GameEventState>,
}

impl Calendar {
    fn load(ctx: &ReducerContext) -> Self {
        let now = crate::time::now_secs(ctx) as i64;
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

/// Sends the mail of each event that started, then brings the spawns, pools and creature data of
/// every event whose state is not yet applied toward that state, through the spawn lifecycle;
/// docs/schema.md#game-events.
pub(crate) fn reconcile(ctx: &ReducerContext) {
    reconcile_within(ctx, CHANGES_PER_PASS);
}

fn reconcile_within(ctx: &ReducerContext, mut budget: usize) {
    let calendar = Calendar::load(ctx);
    mail::follow(ctx, &calendar.states);
    let statuses = ctx.db.game_event_status();
    let mut pending: Vec<GameEventStatus> = calendar
        .states
        .iter()
        .map(|(&event_entry, &state)| {
            retarget(statuses.event_entry().find(event_entry), event_entry, state)
        })
        .filter(|status| !status.settled())
        .collect();
    let share = fair_share(budget, pending.len());
    for round_share in [Some(share), None] {
        for status in &mut pending {
            if status.settled() || budget == 0 {
                continue;
            }
            let mut allowance = round_share.map_or(budget, |share| share.min(budget));
            let granted = allowance;
            advance(ctx, &calendar, status, &mut allowance);
            budget -= granted - allowance;
        }
    }
    for status in pending {
        if !status.settled() {
            log::info!(
                "game events: event {} continues toward {:?} on the next pass",
                status.event_entry,
                status.state
            );
        }
        if statuses.event_entry().find(status.event_entry).is_some() {
            statuses.event_entry().update(status);
        } else {
            statuses.insert(status);
        }
    }
}

/// Brings the event's links, then its pools, then its creature data rows, from the status cursors
/// toward its state, until `budget` changes are spent.
fn advance(
    ctx: &ReducerContext,
    calendar: &Calendar,
    status: &mut GameEventStatus,
    budget: &mut usize,
) {
    if status.next_link_id != u64::MAX {
        let from = status.next_link_id;
        let mut links: Vec<(u64, u64)> = ctx
            .db
            .game_event_link()
            .by_event()
            .filter(status.event_entry)
            .filter(|link| link.link_id >= from)
            .map(|link| (link.link_id, link.spawn_guid))
            .collect();
        links.sort_unstable();
        for (link_id, spawn_guid) in links {
            if *budget == 0 {
                status.next_link_id = link_id;
                return;
            }
            let shows = calendar.spawn_shows(ctx, spawn_guid);
            let changed = if crate::gameobject::is_gameobject_guid(spawn_guid) {
                usize::from(crate::gameobject::show_for_events(ctx, spawn_guid, shows))
            } else {
                crate::creatures::show_spawn_for_events(ctx, spawn_guid, shows)
            };
            *budget = budget.saturating_sub(changed);
        }
        status.next_link_id = u64::MAX;
    }
    if status.next_pool_id != u32::MAX {
        let from = status.next_pool_id;
        let mut pools: Vec<u32> = ctx
            .db
            .game_event_pool()
            .by_event()
            .filter(status.event_entry)
            .map(|pool| pool.pool_id)
            .filter(|&pool_id| pool_id >= from)
            .collect();
        pools.sort_unstable();
        let shows = calendar.state(status.event_entry) == GameEventState::Active;
        for pool_id in pools {
            if *budget == 0 {
                status.next_pool_id = pool_id;
                return;
            }
            *budget -= usize::from(crate::gameobject::show_event_pool(ctx, pool_id, shows));
        }
        status.next_pool_id = u32::MAX;
    }
    if status.next_creature_data_id != u64::MAX {
        let from = status.next_creature_data_id;
        let mut rows: Vec<GameEventCreatureData> = ctx
            .db
            .game_event_creature_data()
            .by_event()
            .filter(status.event_entry)
            .filter(|data| data.data_id >= from)
            .collect();
        rows.sort_unstable_by_key(|data| data.data_id);
        let running = calendar.state(status.event_entry) == GameEventState::Active;
        for data in rows {
            if *budget == 0 {
                status.next_creature_data_id = data.data_id;
                return;
            }
            let worn = worn_data(ctx, data.spawn_guid, |event_entry| {
                calendar.state(event_entry)
            });
            let changed = crate::creatures::event_data::follow_event(ctx, &data, running, &worn);
            *budget = budget.saturating_sub(changed);
        }
        status.next_creature_data_id = u64::MAX;
    }
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

fn hold(ctx: &ReducerContext, event_entry: u32, running: bool) -> Result<(), String> {
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
    overrides.insert(GameEventOverride {
        event_entry,
        running,
    });
    reconcile(ctx);
    Ok(())
}

/// Runs the event until `debug_event_clear`, whatever its calendar says.
#[reducer]
pub fn debug_event_start(ctx: &ReducerContext, event_entry: u32) -> Result<(), String> {
    hold(ctx, event_entry, true)
}

/// Stops the event until `debug_event_clear`, whatever its calendar says.
#[reducer]
pub fn debug_event_stop(ctx: &ReducerContext, event_entry: u32) -> Result<(), String> {
    hold(ctx, event_entry, false)
}

/// Hands the event back to its calendar; it also clears a hold on an event an import removed.
#[reducer]
pub fn debug_event_clear(ctx: &ReducerContext, event_entry: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !ctx
        .db
        .game_event_override()
        .event_entry()
        .delete(event_entry)
    {
        return Err(format!("game event {event_entry} has no Operator hold"));
    }
    reconcile(ctx);
    Ok(())
}

/// Runs one pass that changes at most `budget` spawns, pools and creatures.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_run_game_event_pass(ctx: &ReducerContext, budget: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    reconcile_within(ctx, budget as usize);
    Ok(())
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

    fn event(scripted: bool, disabled: bool, occurrence: Occurrence) -> GameEvent {
        GameEvent {
            event_entry: 7,
            start_secs: occurrence.start_secs,
            end_secs: occurrence.end_secs,
            occurrence_mins: occurrence.occurrence_mins,
            length_mins: occurrence.length_mins,
            scripted,
            disabled,
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
    fn the_calendar_runs_a_scheduled_event_and_keeps_a_scripted_one_stopped() {
        use GameEventState::*;
        let inside = NIGHT_START + MINUTE;
        let outside = NIGHT_START + 13 * HOUR;
        let scheduled = event(false, false, night());
        assert_eq!(event_state(Some(&scheduled), None, inside), Active);
        assert_eq!(event_state(Some(&scheduled), None, outside), Inactive);
        let scripted = event(true, false, night());
        assert_eq!(event_state(Some(&scripted), None, inside), Inactive);
    }

    #[test]
    fn the_darkmoon_faire_runs_on_its_schedule_without_an_operator() {
        use lyracore_shared::game_event::{DARKMOON_FAIRE_ELWYNN, DARKMOON_FAIRE_MULGORE};
        // 2026-12-07 12:00 UTC, the Faire's first day in Elwynn.
        let faire_day = 1_796_644_800;
        let faire = |event_entry| GameEvent {
            event_entry,
            ..event(true, false, night())
        };
        let elwynn = faire(DARKMOON_FAIRE_ELWYNN);
        let mulgore = faire(DARKMOON_FAIRE_MULGORE);
        assert_eq!(
            event_state(Some(&elwynn), None, faire_day),
            GameEventState::Active
        );
        assert_eq!(
            event_state(Some(&mulgore), None, faire_day),
            GameEventState::Inactive
        );
        assert_eq!(
            event_state(Some(&elwynn), Some(false), faire_day),
            GameEventState::Inactive
        );
    }

    #[test]
    fn a_disabled_missing_or_zero_length_event_is_unscheduled() {
        let inside = NIGHT_START + MINUTE;
        let disabled = event(true, true, night());
        let empty = event(
            false,
            false,
            Occurrence {
                length_mins: 0,
                ..night()
            },
        );
        for definition in [Some(&disabled), Some(&empty), None] {
            assert_eq!(
                event_state(definition, None, inside),
                GameEventState::Unscheduled
            );
        }
    }

    #[test]
    fn an_operator_start_or_stop_holds_over_the_calendar() {
        use GameEventState::*;
        let outside = NIGHT_START + 13 * HOUR;
        let inside = NIGHT_START + MINUTE;
        let scheduled = event(false, false, night());
        let disabled = event(true, true, night());
        assert_eq!(event_state(Some(&scheduled), Some(true), outside), Active);
        assert_eq!(event_state(Some(&scheduled), Some(false), inside), Inactive);
        assert_eq!(event_state(Some(&disabled), Some(true), outside), Active);
        assert_eq!(event_state(None, Some(true), inside), Unscheduled);
    }

    fn status(state: GameEventState, next_link_id: u64) -> GameEventStatus {
        GameEventStatus {
            event_entry: 7,
            state,
            next_link_id,
            next_pool_id: 0,
            next_creature_data_id: 0,
        }
    }

    #[test]
    fn a_state_change_part_way_through_starts_again_from_the_first_link() {
        let halfway = status(GameEventState::Active, 501);
        assert_eq!(
            retarget(Some(halfway), 7, GameEventState::Inactive),
            status(GameEventState::Inactive, 0)
        );
    }

    #[test]
    fn work_toward_an_unchanged_state_keeps_its_progress() {
        let halfway = status(GameEventState::Active, 501);
        assert_eq!(
            retarget(Some(halfway.clone()), 7, GameEventState::Active),
            halfway
        );
        assert_eq!(
            retarget(None, 7, GameEventState::Active),
            status(GameEventState::Active, 0)
        );
    }

    #[test]
    fn every_pending_event_gets_a_share_of_the_pass() {
        assert_eq!(fair_share(500, 4), 125);
        assert_eq!(fair_share(500, 0), 500);
        assert_eq!(fair_share(3, 10), 1);
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
