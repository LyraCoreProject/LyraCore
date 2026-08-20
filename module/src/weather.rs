//! Per-zone weather: the seasonal weights a zone's climate comes from, the current-weather row the
//! gateway relays, the slow roll that advances it, and the operator's forced-weather Verb.
//!
//! The Module owns every durable weather fact and every Gate on it. The Gateway owns the wire
//! encoding (`SMSG_WEATHER`, its sound bands, and smooth-versus-instant transitions) and the routing
//! of a row change to the World Sessions currently in that zone — none of which appears here.
//!
//! Three tables:
//!
//!   - `game_weather` — per-zone seasonal rain/snow/storm chances, in CMaNGOS's `game_weather`
//!     column shape so the world-data import that eventually fills it is mechanical. Private: only
//!     the roll reads it.
//!   - `game_zone_weather` — one row per zone that has weather, holding what a client currently
//!     sees. Public, because this row IS the relay source: the gateway subscribes to it and turns a
//!     row change into a packet. Upserted in place, never appended to.
//!   - `game_weather_schedule` — the ten-minute firing that rolls every zone.
//!
//! The decisions are pure and live above the tables: [`season_of`], [`weather_for_roll`] and
//! [`regenerate`] take a timestamp and already-drawn random values, so weather is testable without
//! a clock or a chance.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::helpers::require_operator;

// =================================================================================================
//  The vanilla packet contract
// =================================================================================================

/// `SMSG_WEATHER`'s `WeatherType` discriminants. Vanilla has exactly these four; the 1.12.1 client
/// renders nothing else, so they are also the full set of values a durable row may hold.
pub(crate) const WEATHER_FINE: u8 = 0;
pub(crate) const WEATHER_RAIN: u8 = 1;
pub(crate) const WEATHER_SNOW: u8 = 2;
pub(crate) const WEATHER_STORM: u8 = 3;

/// The lowest intensity a non-fine zone may store, and the highest any zone may. CMaNGOS's
/// `Weather::NormalizeGrade` pins the same two values: a grade at or above 1 renders wrong, and a
/// grade of 0 under a weather type renders nothing at all, which reads to a player as a stuck sky.
const MIN_INTENSITY: f32 = 0.0001;
const MAX_INTENSITY: f32 = 0.9999;

/// One third of the intensity range — the step CMaNGOS's improve and worsen branches take.
const INTENSITY_STEP: f32 = 0.333_333_34;

/// Two thirds of the intensity range — the radical branch's drop from heavy back towards light.
const RADICAL_STEP: f32 = 0.666_666_7;

/// The spread of one intensity band (light, medium, heavy). CMaNGOS spells this `0.3333` rather than
/// deriving it from [`INTENSITY_STEP`], and the two are deliberately not the same number: a band's
/// spread has to leave room under the `0.3334` and `0.6667` offsets stacked on it.
const BAND_WIDTH: f32 = 0.3333;

/// A weather type the 1.12.1 client renders.
fn is_wire_weather_type(weather_type: u8) -> bool {
    (WEATHER_FINE..=WEATHER_STORM).contains(&weather_type)
}

// =================================================================================================
//  Tables
// =================================================================================================

/// One zone's seasonal climate: the percent chance of rain, snow and storm in each of the four
/// seasons. Fine weather is whatever the three leave over and is never a stored column.
///
/// The column shape is CMaNGOS's `game_weather` table verbatim (a zone key plus twelve chance
/// columns), because the world-data import will eventually load this table from that dump and a
/// shape that already matches makes that change mechanical. Until then [`seed_weather_weights`]
/// hand-authors a couple of zones.
///
/// Private: nothing outside the Module reads climate data. The Gateway reads only the CURRENT
/// weather, from [`ZoneWeather`].
#[table(accessor = game_weather)]
pub struct ZoneWeatherChance {
    #[primary_key]
    pub zone_id: u32,
    pub spring_rain_chance: u8,
    pub spring_snow_chance: u8,
    pub spring_storm_chance: u8,
    pub summer_rain_chance: u8,
    pub summer_snow_chance: u8,
    pub summer_storm_chance: u8,
    pub fall_rain_chance: u8,
    pub fall_snow_chance: u8,
    pub fall_storm_chance: u8,
    pub winter_rain_chance: u8,
    pub winter_snow_chance: u8,
    pub winter_storm_chance: u8,
}

impl ZoneWeatherChance {
    /// This zone's chances for `season` (0 spring, 1 summer, 2 fall, 3 winter). An out-of-range
    /// index reads as spring rather than panicking — [`season_of`] cannot produce one, but the
    /// caller is a scheduled reducer and a panic there would stop every zone, not just this one.
    fn season(&self, season: usize) -> SeasonChance {
        let (rain, snow, storm) = match season {
            1 => (
                self.summer_rain_chance,
                self.summer_snow_chance,
                self.summer_storm_chance,
            ),
            2 => (
                self.fall_rain_chance,
                self.fall_snow_chance,
                self.fall_storm_chance,
            ),
            3 => (
                self.winter_rain_chance,
                self.winter_snow_chance,
                self.winter_storm_chance,
            ),
            _ => (
                self.spring_rain_chance,
                self.spring_snow_chance,
                self.spring_storm_chance,
            ),
        };
        SeasonChance::from_percent(rain, snow, storm)
    }
}

/// What one zone's sky currently shows. **The relay source**: the gateway subscribes to this table
/// and turns an inserted or updated row into `SMSG_WEATHER` for the World Sessions in that zone, so
/// a row is written only when the state a client would render actually changed.
///
/// One row per zone, upserted in place — durable state and relay source in the same row, rather than
/// a second one-shot event table. That keeps one authority: world entry and zone entry read the same
/// row a live change relays, so a reconnect replays exactly what everyone else already has and no
/// TTL can race a client's arrival. A zone with no row has fine weather.
///
/// `weather_type` holds the wire discriminant (see [`WEATHER_FINE`] and its siblings) and
/// `intensity` is the packet's grade, always finite and inside `0.0..=1.0`. The Gates that keep both
/// true are [`gate_forced_weather`] and [`normalize_intensity`]; nothing else writes this table.
#[table(accessor = game_zone_weather, public)]
pub struct ZoneWeather {
    #[primary_key]
    pub zone_id: u32,
    pub weather_type: u8,
    pub intensity: f32,
    /// When the visible state last changed, in `ctx.timestamp` micros. An unchanged roll does not
    /// write the row, so this is the age of what a client sees rather than the time of the last
    /// firing.
    pub changed_at_micros: i64,
}

/// Drives [`tick_weather`]. One row; its `scheduled_at` **is** the cadence (the
/// `CreatureMoveSchedule` precedent — a separate `tick_ms` column would only duplicate it and then
/// drift from it).
///
/// Weather rides its own schedule rather than the creature tick: ten minutes is three orders of
/// magnitude slower than movement, and a pass that fires 1200 times for every one time it does
/// anything is a pass that eventually gets quantized wrong.
#[table(accessor = game_weather_schedule, scheduled(tick_weather))]
pub struct WeatherSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// Ten minutes between rolls, matching CMaNGOS's default weather-change interval. Slow on purpose:
/// every firing that changes something is a packet to every client in the zone, and vanilla weather
/// is meant to drift rather than flicker.
pub(crate) const WEATHER_TICK_MICROS: i64 = 600_000_000;

// =================================================================================================
//  Pure decisions
// =================================================================================================

/// The rain, snow and storm chances of ONE season, as percentages. Fine weather is the remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeasonChance {
    pub rain: u32,
    pub snow: u32,
    pub storm: u32,
}

impl SeasonChance {
    /// Read three stored chances as percentages. A value above 100 is clamped where it is read
    /// rather than refused: this table is filled by the world-data import and editable by an
    /// operator's SQL, and a chance of 200 would silently swallow the band below it instead of
    /// failing loudly. The three are deliberately NOT normalized against each other — rescaling
    /// them into a distribution would describe a different climate than the data does.
    pub(crate) fn from_percent(rain: u8, snow: u8, storm: u8) -> Self {
        Self {
            rain: rain.min(100) as u32,
            snow: snow.min(100) as u32,
            storm: storm.min(100) as u32,
        }
    }
}

/// What one zone's sky shows, independent of the row it is stored in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeatherState {
    pub weather_type: u8,
    pub intensity: f32,
}

impl WeatherState {
    /// Clear sky. What a zone with no climate data, no row, or no weather shows.
    pub(crate) const FINE: Self = Self {
        weather_type: WEATHER_FINE,
        intensity: 0.0,
    };
}

/// The random values one roll consumes, drawn at the reducer boundary so [`regenerate`] stays a pure
/// decision. The names and ranges follow CMaNGOS `Weather::ReGenerate`'s own draws, in order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WeatherDraw {
    /// `urand(0, 99)`: picks no change, improve, worsen or radical change.
    pub branch: u32,
    /// `urand(0, 99)`: the radical branch's coin for how severe the change is.
    pub severity: u32,
    /// `urand(1, 100)`: the weighted weather-type selection.
    pub weather: u32,
    /// `rand_norm_f()` as basis points, `0..=9999`: where inside its band a new intensity falls.
    pub intensity_bp: u32,
    /// `urand(0, 99)`: the medium-versus-heavy coin for a radically new intensity.
    pub band: u32,
}

/// The season a timestamp falls in: 0 spring, 1 summer, 2 fall, 3 winter.
///
/// CMaNGOS's derivation, `((day_of_year - 78 + 365) / 91) % 4`: 78 days separate 1 January from the
/// 20 March equinox and 91 days is a quarter of 365, so each season runs 91 days and winter wraps
/// both ends of the year.
///
/// The truncating division turns spring one day EARLIER than CMaNGOS's own comment claims — 364 is a
/// whole multiple of 91, so day 77 (19 March outside a leap year) already reads as spring. That
/// off-by-one is preserved rather than corrected: CMaNGOS's arithmetic is the behaviour this
/// reproduces, and a day of drift in a season boundary is invisible next to the drift a corrected
/// version would put between this realm and every other server built on the same numbers.
///
/// The realm clock is UTC. Weather must not change because the host it runs on moved timezone.
pub(crate) fn season_of(now_micros: i64) -> usize {
    let day_of_year = day_of_year(now_micros) as i64;
    (((day_of_year - 78 + 365) / 91) % 4) as usize
}

/// Day of the year, 0-based (1 January is day 0), in UTC.
pub(crate) fn day_of_year(now_micros: i64) -> u32 {
    /// Days before the first of each month in a year that is not a leap year.
    const BEFORE_MONTH: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

    let days = now_micros.div_euclid(86_400_000_000);
    let (year, month, day) = civil_from_days(days);
    let leap_day = (month > 2 && is_leap_year(year)) as u32;
    BEFORE_MONTH[(month - 1) as usize] + day - 1 + leap_day
}

/// `(year, month 1..=12, day 1..=31)` for a count of days since 1970-01-01, proleptic Gregorian.
///
/// The era arithmetic shifts the year to start in March so the leap day lands last and needs no
/// special case. Exact for any timestamp a realm can hold, including dates before 1970.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468; // days since 0000-03-01
    let era = shifted.div_euclid(146_097); // one 400-year cycle
    let day_of_era = shifted - era * 146_097; // 0..=146096
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // 0..=399
    let march_year = year_of_era + era * 400;
    let day_of_march_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_march_year + 2) / 153; // 0 is March
    let day = (day_of_march_year - (153 * march_month + 2) / 5 + 1) as u32;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    } as u32;
    let year = if month <= 2 {
        march_year + 1
    } else {
        march_year
    };
    (year, month, day)
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The weather type a `1..=100` roll selects from one season's chances.
///
/// CMaNGOS's cumulative sampling: the roll walks rain, then rain plus snow, then rain plus snow plus
/// storm, and anything past the last band is fine weather. Fine therefore has no chance column of
/// its own — it is exactly the probability the three leave unclaimed.
pub(crate) fn weather_for_roll(chance: SeasonChance, roll_1_to_100: u32) -> u8 {
    let through_rain = chance.rain;
    let through_snow = through_rain + chance.snow;
    let through_storm = through_snow + chance.storm;
    if roll_1_to_100 <= through_rain {
        WEATHER_RAIN
    } else if roll_1_to_100 <= through_snow {
        WEATHER_SNOW
    } else if roll_1_to_100 <= through_storm {
        WEATHER_STORM
    } else {
        WEATHER_FINE
    }
}

/// Hold an intensity inside the vanilla packet contract: finite, and inside `0.0..=1.0`. Fine
/// weather is exactly zero; anything else is kept off both ends of the range.
///
/// CMaNGOS normalizes when it SENDS. This normalizes when it STORES, because here the durable row is
/// both what the gateway encodes and what "did the visible state change?" is answered against — a
/// row holding a grade no client can be shown would make both of those lie.
pub(crate) fn normalize_intensity(weather_type: u8, intensity: f32) -> f32 {
    if weather_type == WEATHER_FINE || !intensity.is_finite() {
        return 0.0;
    }
    intensity.clamp(MIN_INTENSITY, MAX_INTENSITY)
}

/// Roll one zone's next weather. `None` means the sky does not change, which is what stops the roll
/// from writing a row and relaying a packet that says nothing new.
///
/// CMaNGOS `Weather::ReGenerate`, decision for decision: 30 percent of firings leave the zone alone,
/// 30 percent improve it (or clear it, when it was barely doing anything), 30 percent worsen it, and
/// 10 percent change it radically — light goes straight to heavy, heavy has an even chance of
/// dropping two thirds, and anything else clears up and re-rolls a fresh type from the season's
/// weights. A zone that is already fine goes straight to that re-roll.
pub(crate) fn regenerate(
    current: WeatherState,
    chance: SeasonChance,
    draw: WeatherDraw,
) -> Option<WeatherState> {
    if draw.branch < 30 {
        return None;
    }

    let mut next = current;

    // Below one third there is barely anything to improve, so improving it is clearing it.
    if draw.branch < 60 && next.intensity < INTENSITY_STEP {
        next = WeatherState::FINE;
    }
    if draw.branch < 60 && next.weather_type != WEATHER_FINE {
        next.intensity -= INTENSITY_STEP;
        return changed(current, next);
    }
    if draw.branch < 90 && next.weather_type != WEATHER_FINE {
        next.intensity += INTENSITY_STEP;
        return changed(current, next);
    }
    if next.weather_type != WEATHER_FINE {
        if next.intensity < INTENSITY_STEP {
            next.intensity = MAX_INTENSITY;
            return changed(current, next);
        }
        if next.intensity > RADICAL_STEP && draw.severity < 50 {
            next.intensity -= RADICAL_STEP;
            return changed(current, next);
        }
        next = WeatherState::FINE;
    }

    // Only a zone that is now fine reaches here: pick this season's weather afresh.
    next.weather_type = weather_for_roll(chance, draw.weather);
    let within_band = draw.intensity_bp.min(9_999) as f32 / 10_000.0;
    next.intensity = if next.weather_type == WEATHER_FINE {
        0.0
    } else if draw.branch < 90 {
        within_band * BAND_WIDTH // light, the common case
    } else if draw.band < 50 {
        within_band * BAND_WIDTH + 0.3334 // medium
    } else {
        within_band * BAND_WIDTH + 0.6667 // heavy
    };
    changed(current, next)
}

/// Normalize a rolled state and report it only when the sky it describes differs from the current
/// one. Both halves matter: normalizing first is what makes two states that render identically
/// compare equal, so a storm already at the top of the range stops relaying when it worsens again.
fn changed(current: WeatherState, next: WeatherState) -> Option<WeatherState> {
    let next = WeatherState {
        weather_type: next.weather_type,
        intensity: normalize_intensity(next.weather_type, next.intensity),
    };
    (next != current).then_some(next)
}

/// The Gates on operator-forced weather, pure so their Refusals are unit-testable. A forced state
/// reaches a client verbatim, so it must satisfy the packet contract before it is stored.
pub(crate) fn gate_forced_weather(
    weather_type: u8,
    intensity: f32,
) -> Result<WeatherState, String> {
    if !is_wire_weather_type(weather_type) {
        return Err(format!(
            "weather type {weather_type} is not one of fine (0), rain (1), snow (2), storm (3)"
        ));
    }
    if !intensity.is_finite() || !(0.0..=1.0).contains(&intensity) {
        return Err(format!(
            "weather intensity {intensity} is outside the vanilla 0.0..=1.0 grade range"
        ));
    }
    Ok(WeatherState {
        weather_type,
        intensity: normalize_intensity(weather_type, intensity),
    })
}

// =================================================================================================
//  The durable write path
// =================================================================================================

/// The one durable write. Both the scheduled roll and the forced Verb come through here, so there is
/// a single answer to what a zone's row looks like after a change. Upsert in place: the row is keyed
/// by zone and its update IS the relay.
fn apply_zone_weather(ctx: &ReducerContext, zone_id: u32, state: WeatherState, now_micros: i64) {
    let rows = ctx.db.game_zone_weather();
    let row = ZoneWeather {
        zone_id,
        weather_type: state.weather_type,
        intensity: state.intensity,
        changed_at_micros: now_micros,
    };
    if rows.zone_id().find(zone_id).is_some() {
        rows.zone_id().update(row);
    } else {
        rows.insert(row);
    }
}

/// What `zone_id`'s sky currently shows. A zone that has never rolled reads as fine, the same answer
/// a zone with no climate data gets — missing weather data is fine weather, silently.
fn current_state(ctx: &ReducerContext, zone_id: u32) -> WeatherState {
    ctx.db
        .game_zone_weather()
        .zone_id()
        .find(zone_id)
        .map(|row| WeatherState {
            weather_type: row.weather_type,
            intensity: row.intensity,
        })
        .unwrap_or(WeatherState::FINE)
}

/// Draw one roll's random values at the reducer boundary. `ctx.random` is the module RNG, so the
/// draw is deterministic per transaction and the decision that reads it stays pure.
fn draw_weather(ctx: &ReducerContext) -> WeatherDraw {
    WeatherDraw {
        branch: ctx.random::<u32>() % 100,
        severity: ctx.random::<u32>() % 100,
        weather: ctx.random::<u32>() % 100 + 1,
        intensity_bp: ctx.random::<u32>() % 10_000,
        band: ctx.random::<u32>() % 100,
    }
}

// =================================================================================================
//  The roll
// =================================================================================================

/// Roll every zone that has climate data (scheduled, scheduler-only).
///
/// Zones are independent: each draws its own random values and reads its own current row, so one
/// zone's missing data or unchanged roll neither changes nor delays another's. A zone whose rolled
/// sky matches the one it already shows writes nothing at all.
///
/// The weights table is read whole. It holds one row per zone with climate data, and the pass exists
/// to visit every one of them, so there is no narrower read to be had. Each World Shard rolls the
/// zones ITS weights table holds; the import is not map-filtered, so a shard also rolls zones it does
/// not own, and those rows are inert because a zone is only ever observed on the shard that owns its
/// map.
#[reducer]
pub fn tick_weather(ctx: &ReducerContext, _schedule: WeatherSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let season = season_of(now_micros);
    // Collect first: the loop writes one table while walking another.
    let zones: Vec<ZoneWeatherChance> = ctx.db.game_weather().iter().collect();
    for weights in zones {
        let draw = draw_weather(ctx);
        let current = current_state(ctx, weights.zone_id);
        if let Some(next) = regenerate(current, weights.season(season), draw) {
            apply_zone_weather(ctx, weights.zone_id, next, now_micros);
        }
    }
}

// =================================================================================================
//  The operator's lever
// =================================================================================================

/// Force one zone's weather, so live-client and wire Verification does not have to wait on chance.
///
/// Operator-gated rather than `debug_reducers`-gated, so the lever exists in a production build —
/// the same reasoning as `set_motion_tick_ms`. It is not client-callable: no client identity passes
/// [`require_operator`].
///
/// The forced state goes through [`apply_zone_weather`], the same durable write the scheduled roll
/// uses, so a forced change and a rolled one are indistinguishable downstream. A Refusal returns
/// before any write and leaves every row exactly as it was.
///
/// A zone with no weights row is Refused rather than forced. Climate data is what decides that a
/// zone has weather at all, and a row forced onto a zone the roll never visits is a sky that can
/// never change back.
#[reducer]
pub fn gw_force_zone_weather(
    ctx: &ReducerContext,
    zone_id: u32,
    weather_type: u8,
    intensity: f32,
) -> Result<(), String> {
    require_operator(ctx)?;
    let state = gate_forced_weather(weather_type, intensity)?;
    if ctx.db.game_weather().zone_id().find(zone_id).is_none() {
        return Err(format!("zone {zone_id} has no weather weights"));
    }
    apply_zone_weather(
        ctx,
        zone_id,
        state,
        ctx.timestamp.to_micros_since_unix_epoch(),
    );
    Ok(())
}

// =================================================================================================
//  Seeds
// =================================================================================================

/// The provenance family [`seed_weather_weights`] stamps. Deliberately distinct from the family the
/// world-data import will use, so the two never claim the same rows: when the import lands it fills
/// `game_weather` under its own family and this seed goes away in the same change.
const WEATHER_SEED_FAMILY: &str = "weather_seed";

/// One zone's four seasons of `(rain, snow, storm)` percentages, spring first.
type SeasonalPercents = [(u8, u8, u8); 4];

/// Hand-authored climate for the two zones the feature is verified in, TEMPORARY until the
/// world-data import includes CMaNGOS's `game_weather` — that change fills this table from the dump
/// and deletes this seed, so a zone's climate only ever has one authority.
///
/// The numbers are plausible rather than sourced: Elwynn Forest is a mild temperate forest that rains
/// often and only snows in winter; Westfall is a dry coastal plain that mostly stays clear.
/// Percentages per season in `(rain, snow, storm)` order, spring first.
const TEMPORARY_ZONE_CLIMATE: &[(u32, SeasonalPercents)] = &[
    // Elwynn Forest.
    (12, [(40, 0, 10), (25, 0, 10), (40, 0, 15), (30, 10, 5)]),
    // Westfall.
    (40, [(15, 0, 5), (10, 0, 5), (15, 0, 5), (20, 0, 5)]),
];

/// Seed the temporary climate rows. Only-if-empty, like `seed_createinfo_spells`: an operator edit, a
/// delete, and a real import all stick across restarts, and calling this from both `init` and
/// `debug_repair_after_publish` is safe because `init` does not re-run on an auto-migrate republish.
///
/// Returns how many climate rows the table holds afterwards, for the repair pass's provenance stamp.
pub(crate) fn seed_weather_weights(ctx: &ReducerContext) -> u64 {
    let table = ctx.db.game_weather();
    if table.count() > 0 {
        return table.count();
    }
    for &(zone_id, seasons) in TEMPORARY_ZONE_CLIMATE {
        let [spring, summer, fall, winter] = seasons;
        table.insert(ZoneWeatherChance {
            zone_id,
            spring_rain_chance: spring.0,
            spring_snow_chance: spring.1,
            spring_storm_chance: spring.2,
            summer_rain_chance: summer.0,
            summer_snow_chance: summer.1,
            summer_storm_chance: summer.2,
            fall_rain_chance: fall.0,
            fall_snow_chance: fall.1,
            fall_storm_chance: fall.2,
            winter_rain_chance: winter.0,
            winter_snow_chance: winter.1,
            winter_storm_chance: winter.2,
        });
    }
    let seeded = TEMPORARY_ZONE_CLIMATE.len() as u64;
    // Empty source sha and file hash: this data is hand-authored, not loaded from a dump.
    crate::import_meta::stamp(ctx, WEATHER_SEED_FAMILY, "", "", seeded);
    seeded
}

/// Re-arm the weather roll at its canonical interval: delete whatever schedule rows exist, insert
/// one. The single definition of the armed row — `seed::init` calls it on a fresh database and
/// `debug_repair_after_publish` calls it on an already-migrated one, where `init` does not re-run
/// and an unarmed schedule would mean weather silently stops.
///
/// Re-arm rather than ensure — unlike the movement republish there is no operator cadence knob to
/// preserve here, so the canonical interval is the only interval, and re-arming also collapses a
/// table that somehow holds two rows back to one.
///
/// No third self-healing net (the one `motion::ensure_schedule_armed` carries): weather has no hot
/// path to hang one off. Nothing but this schedule and the operator's lever ever writes a weather
/// row, so there is no ordinary call that could notice the schedule missing and fix it.
pub(crate) fn rearm_weather_schedule(ctx: &ReducerContext) {
    let schedule = ctx.db.game_weather_schedule();
    let stale: Vec<u64> = schedule.iter().map(|row| row.scheduled_id).collect();
    for scheduled_id in stale {
        schedule.scheduled_id().delete(scheduled_id);
    }
    schedule.insert(WeatherSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(WEATHER_TICK_MICROS)),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A draw that takes the branch it is given; the rest are values the branch under test either
    /// does not read or reads on a path each test names.
    fn draw(branch: u32) -> WeatherDraw {
        WeatherDraw {
            branch,
            severity: 99,
            weather: 100,
            intensity_bp: 0,
            band: 99,
        }
    }

    /// Micros at midnight UTC on a date, counted forward from the epoch a year at a time so the
    /// expectation does not come from [`civil_from_days`] run backwards.
    fn micros_at(year: i64, month: u32, day: u32) -> i64 {
        const BEFORE_MONTH: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let mut days = 0i64;
        for earlier in 1970..year {
            days += if is_leap_year(earlier) { 366 } else { 365 };
        }
        days += BEFORE_MONTH[(month - 1) as usize];
        if month > 2 && is_leap_year(year) {
            days += 1;
        }
        days += day as i64 - 1;
        days * 86_400_000_000
    }

    #[test]
    fn day_of_year_matches_worked_calendar_dates() {
        assert_eq!(day_of_year(0), 0, "the epoch is 1 January 1970");
        assert_eq!(day_of_year(micros_at(2026, 3, 20)), 78, "31 + 28 + 19");
        assert_eq!(
            day_of_year(micros_at(2024, 3, 1)),
            60,
            "31 + 29 in a leap year"
        );
        assert_eq!(day_of_year(micros_at(2026, 12, 31)), 364);
        assert_eq!(day_of_year(micros_at(2024, 12, 31)), 365, "a leap year");
    }

    /// Worked from the arithmetic, not from CMaNGOS's prose: `(day_of_year - 78 + 365) / 91` first
    /// reaches 4 at 364, which is day 77 — 19 March in 2026 (31 + 28 + 18). Each season then runs
    /// 91 days and winter takes what is left at both ends of the year.
    #[test]
    fn seasons_turn_a_day_before_the_equinox_and_every_ninety_one_days_after_that() {
        let day = 86_400_000_000i64;
        let turn = micros_at(2026, 3, 19);
        assert_eq!(day_of_year(turn), 77, "the day the season index steps");
        assert_eq!(season_of(turn - day), 3, "18 March is still winter");
        assert_eq!(season_of(turn), 0, "then spring");
        assert_eq!(season_of(turn + 90 * day), 0, "spring runs 91 days");
        assert_eq!(season_of(turn + 91 * day), 1, "then summer");
        assert_eq!(season_of(turn + 182 * day), 2, "then fall");
        assert_eq!(season_of(turn + 273 * day), 3, "then winter");
        assert_eq!(
            season_of(micros_at(2027, 1, 1)),
            3,
            "winter wraps the new year"
        );
    }

    #[test]
    fn a_weighted_roll_walks_rain_then_snow_then_storm_and_leaves_fine_as_the_remainder() {
        let chance = SeasonChance {
            rain: 30,
            snow: 10,
            storm: 5,
        };
        assert_eq!(weather_for_roll(chance, 1), WEATHER_RAIN);
        assert_eq!(weather_for_roll(chance, 30), WEATHER_RAIN);
        assert_eq!(weather_for_roll(chance, 31), WEATHER_SNOW);
        assert_eq!(weather_for_roll(chance, 40), WEATHER_SNOW);
        assert_eq!(weather_for_roll(chance, 41), WEATHER_STORM);
        assert_eq!(weather_for_roll(chance, 45), WEATHER_STORM);
        assert_eq!(weather_for_roll(chance, 46), WEATHER_FINE);
        assert_eq!(weather_for_roll(chance, 100), WEATHER_FINE);
    }

    #[test]
    fn a_band_with_no_chance_is_never_selected() {
        let chance = SeasonChance {
            rain: 0,
            snow: 0,
            storm: 100,
        };
        for roll in 1..=100 {
            assert_eq!(
                weather_for_roll(chance, roll),
                WEATHER_STORM,
                "roll {roll} must step over the two empty bands"
            );
        }
    }

    #[test]
    fn a_chance_above_a_hundred_percent_reads_as_a_hundred() {
        assert_eq!(
            SeasonChance::from_percent(200, 0, 0),
            SeasonChance {
                rain: 100,
                snow: 0,
                storm: 0
            }
        );
    }

    #[test]
    fn the_first_thirty_percent_of_rolls_leave_the_zone_alone() {
        let storm = WeatherState {
            weather_type: WEATHER_STORM,
            intensity: 0.5,
        };
        let chance = SeasonChance {
            rain: 100,
            snow: 0,
            storm: 0,
        };
        for branch in 0..30 {
            assert!(
                regenerate(storm, chance, draw(branch)).is_none(),
                "branch {branch} must not change the sky"
            );
        }
        assert!(
            regenerate(storm, chance, draw(30)).is_some(),
            "branch 30 is the first that acts"
        );
    }

    #[test]
    fn improving_weather_steps_down_one_third() {
        let rain = WeatherState {
            weather_type: WEATHER_RAIN,
            intensity: 0.9,
        };
        let next = regenerate(rain, SeasonChance::from_percent(0, 0, 0), draw(30))
            .expect("the improve branch changes the sky");
        assert_eq!(next.weather_type, WEATHER_RAIN);
        assert!(
            (next.intensity - 0.566_666_7).abs() < 1e-5,
            "0.9 less one third, got {}",
            next.intensity
        );
    }

    #[test]
    fn improving_weather_that_is_barely_there_clears_the_sky() {
        let drizzle = WeatherState {
            weather_type: WEATHER_RAIN,
            intensity: 0.2,
        };
        let next = regenerate(drizzle, SeasonChance::from_percent(0, 0, 0), draw(30))
            .expect("clearing up changes the sky");
        assert_eq!(next, WeatherState::FINE);
    }

    #[test]
    fn worsening_weather_steps_up_one_third() {
        let rain = WeatherState {
            weather_type: WEATHER_RAIN,
            intensity: 0.4,
        };
        let next = regenerate(rain, SeasonChance::from_percent(0, 0, 0), draw(60))
            .expect("the worsen branch changes the sky");
        assert_eq!(next.weather_type, WEATHER_RAIN);
        assert!(
            (next.intensity - 0.733_333_4).abs() < 1e-5,
            "0.4 plus one third, got {}",
            next.intensity
        );
    }

    #[test]
    fn a_radical_change_takes_light_weather_straight_to_heavy() {
        let drizzle = WeatherState {
            weather_type: WEATHER_RAIN,
            intensity: 0.1,
        };
        let next = regenerate(drizzle, SeasonChance::from_percent(0, 0, 0), draw(90))
            .expect("going to the top of the range changes the sky");
        assert_eq!(next.weather_type, WEATHER_RAIN);
        assert_eq!(next.intensity, MAX_INTENSITY);
    }

    #[test]
    fn a_radical_change_can_drop_heavy_weather_two_thirds() {
        let downpour = WeatherState {
            weather_type: WEATHER_RAIN,
            intensity: 0.9,
        };
        let severe = WeatherDraw {
            severity: 49,
            ..draw(90)
        };
        let next = regenerate(downpour, SeasonChance::from_percent(0, 0, 0), severe)
            .expect("the severity coin changes the sky");
        assert_eq!(next.weather_type, WEATHER_RAIN);
        assert!(
            (next.intensity - 0.233_333_3).abs() < 1e-5,
            "0.9 less two thirds, got {}",
            next.intensity
        );
    }

    #[test]
    fn a_radical_change_that_loses_its_coin_clears_up_and_rolls_a_fresh_type() {
        let downpour = WeatherState {
            weather_type: WEATHER_RAIN,
            intensity: 0.9,
        };
        let clear_then_snow = WeatherDraw {
            severity: 50,
            weather: 1,
            ..draw(90)
        };
        let next = regenerate(
            downpour,
            SeasonChance::from_percent(0, 100, 0),
            clear_then_snow,
        )
        .expect("a fresh type changes the sky");
        assert_eq!(next.weather_type, WEATHER_SNOW);
    }

    #[test]
    fn a_fine_zone_that_rolls_fine_again_does_not_change() {
        let chance = SeasonChance::from_percent(0, 0, 0);
        for branch in 30..100 {
            assert!(
                regenerate(WeatherState::FINE, chance, draw(branch)).is_none(),
                "branch {branch} rolled fine over fine and must write nothing"
            );
        }
    }

    #[test]
    fn a_storm_already_at_the_top_of_the_range_does_not_worsen_further() {
        let capped = WeatherState {
            weather_type: WEATHER_STORM,
            intensity: MAX_INTENSITY,
        };
        assert!(
            regenerate(capped, SeasonChance::from_percent(0, 0, 0), draw(60)).is_none(),
            "worsening a capped storm renders the same sky, so it relays nothing"
        );
    }

    #[test]
    fn a_rolled_intensity_always_lands_inside_the_vanilla_grade_range() {
        let chance = SeasonChance {
            rain: 25,
            snow: 25,
            storm: 25,
        };
        for branch in 30..100 {
            for intensity_bp in [0u32, 1, 5_000, 9_998, 9_999, 12_345] {
                for band in [0u32, 49, 50, 99] {
                    for severity in [0u32, 49, 50, 99] {
                        for start in [0.0f32, 0.1, 0.5, 0.9, MAX_INTENSITY] {
                            for weather_type in
                                [WEATHER_FINE, WEATHER_RAIN, WEATHER_SNOW, WEATHER_STORM]
                            {
                                let current = WeatherState {
                                    weather_type,
                                    intensity: start,
                                };
                                let rolled = WeatherDraw {
                                    branch,
                                    severity,
                                    weather: 1,
                                    intensity_bp,
                                    band,
                                };
                                let Some(next) = regenerate(current, chance, rolled) else {
                                    continue;
                                };
                                assert!(
                                    next.intensity.is_finite()
                                        && (0.0..=1.0).contains(&next.intensity),
                                    "intensity {} escaped the range",
                                    next.intensity
                                );
                                assert!(is_wire_weather_type(next.weather_type));
                                if next.weather_type == WEATHER_FINE {
                                    assert_eq!(next.intensity, 0.0, "fine weather has no grade");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn forced_weather_accepts_every_type_the_client_renders() {
        for weather_type in [WEATHER_FINE, WEATHER_RAIN, WEATHER_SNOW, WEATHER_STORM] {
            let state = gate_forced_weather(weather_type, 0.5).expect("a rendered type");
            assert_eq!(state.weather_type, weather_type);
        }
        assert_eq!(
            gate_forced_weather(WEATHER_RAIN, 1.0)
                .expect("the top of the range")
                .intensity,
            MAX_INTENSITY,
            "a grade of exactly 1 renders wrong and is held just below it"
        );
        assert_eq!(
            gate_forced_weather(WEATHER_FINE, 0.9)
                .expect("fine weather")
                .intensity,
            0.0,
            "fine weather has no grade, whatever intensity was asked for"
        );
    }

    #[test]
    fn forced_weather_refuses_a_type_the_client_cannot_render() {
        for weather_type in [4u8, 5, 22, 255] {
            assert!(
                gate_forced_weather(weather_type, 0.5).is_err(),
                "type {weather_type} is not in the vanilla packet"
            );
        }
    }

    #[test]
    fn forced_weather_refuses_an_intensity_outside_the_packet_contract() {
        for intensity in [-0.1f32, 1.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                gate_forced_weather(WEATHER_RAIN, intensity).is_err(),
                "intensity {intensity} is outside the vanilla grade range"
            );
        }
    }

    #[test]
    fn the_roll_fires_every_ten_minutes() {
        assert_eq!(
            WEATHER_TICK_MICROS,
            10 * 60 * 1_000_000,
            "the weather cadence is CMaNGOS's default; every change is a packet to a whole zone"
        );
    }

    /// Every reducer in this file must open with its gate — `require_operator` for the operator's
    /// lever, the scheduler-only sender fence for the roll — before it reads or writes anything.
    /// The `gw.rs` precedent, applied to the file that holds weather's own two reducers.
    #[test]
    fn every_reducer_here_opens_with_its_gate() {
        let src = include_str!("weather.rs");
        // Built at run time so this test's own source can never match the needle.
        let needle = format!("#[{}]", "reducer");
        let mut chunks = src.split(needle.as_str());
        chunks.next(); // preamble
        let mut seen = 0;
        for chunk in chunks {
            let body = chunk
                .split_once('{')
                .map(|(_, body)| body)
                .unwrap_or("")
                .trim_start();
            assert!(
                body.starts_with("require_operator(ctx)?;")
                    || body.starts_with("if ctx.sender() != ctx.database_identity()"),
                "a reducer here must open with require_operator(ctx)?; or the scheduler-only \
                 sender fence — got:\n{}",
                &body[..body.len().min(120)]
            );
            seen += 1;
        }
        assert_eq!(seen, 2, "the scan found {seen} reducers, expected 2");
    }

    #[test]
    fn the_seeded_climate_covers_the_two_verification_zones_with_valid_percentages() {
        let zones: Vec<u32> = TEMPORARY_ZONE_CLIMATE
            .iter()
            .map(|(zone_id, _)| *zone_id)
            .collect();
        assert_eq!(zones, vec![12, 40], "Elwynn Forest and Westfall");
        for (zone_id, seasons) in TEMPORARY_ZONE_CLIMATE {
            for (rain, snow, storm) in seasons {
                assert!(
                    *rain as u32 + *snow as u32 + *storm as u32 <= 100,
                    "zone {zone_id} claims more than all of a season"
                );
            }
        }
    }
}
