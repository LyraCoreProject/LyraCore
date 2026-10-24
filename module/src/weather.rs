use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::helpers::require_operator;

pub(crate) const WEATHER_FINE: u8 = 0;
pub(crate) const WEATHER_RAIN: u8 = 1;
pub(crate) const WEATHER_SNOW: u8 = 2;
pub(crate) const WEATHER_STORM: u8 = 3;

// The client needs non-fine intensity strictly between zero and one to render correctly.
const MIN_INTENSITY: f32 = 0.0001;
const MAX_INTENSITY: f32 = 0.9999;

const INTENSITY_STEP: f32 = 0.333_333_34;

const RADICAL_STEP: f32 = 0.666_666_7;

const BAND_WIDTH: f32 = 0.3333;

fn is_wire_weather_type(weather_type: u8) -> bool {
    (WEATHER_FINE..=WEATHER_STORM).contains(&weather_type)
}

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

#[table(accessor = game_zone_weather, public)]
pub struct ZoneWeather {
    #[primary_key]
    pub zone_id: u32,
    pub weather_type: u8,
    pub intensity: f32,
    pub changed_at_micros: i64,
}

#[table(accessor = game_weather_schedule, scheduled(tick_weather))]
pub struct WeatherSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) const WEATHER_TICK_MICROS: i64 = 600_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeasonChance {
    pub rain: u32,
    pub snow: u32,
    pub storm: u32,
}

impl SeasonChance {
    pub(crate) fn from_percent(rain: u8, snow: u8, storm: u8) -> Self {
        Self {
            rain: rain.min(100) as u32,
            snow: snow.min(100) as u32,
            storm: storm.min(100) as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeatherState {
    pub weather_type: u8,
    pub intensity: f32,
}

impl WeatherState {
    pub(crate) const FINE: Self = Self {
        weather_type: WEATHER_FINE,
        intensity: 0.0,
    };
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WeatherDraw {
    pub branch: u32,
    pub severity: u32,
    pub weather: u32,
    pub intensity_bp: u32,
    pub band: u32,
}

pub(crate) fn season_of(now_micros: i64) -> usize {
    let day_of_year = day_of_year(now_micros) as i64;
    (((day_of_year - 78 + 365) / 91) % 4) as usize
}

pub(crate) fn day_of_year(now_micros: i64) -> u32 {
    const BEFORE_MONTH: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

    let days = now_micros.div_euclid(86_400_000_000);
    let (year, month, day) = lyracore_shared::calendar::civil_from_days(days);
    let leap_day = (month > 2 && is_leap_year(year)) as u32;
    BEFORE_MONTH[(month - 1) as usize] + day - 1 + leap_day
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

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

pub(crate) fn normalize_intensity(weather_type: u8, intensity: f32) -> f32 {
    if weather_type == WEATHER_FINE || !intensity.is_finite() {
        return 0.0;
    }
    intensity.clamp(MIN_INTENSITY, MAX_INTENSITY)
}

pub(crate) fn regenerate(
    current: WeatherState,
    chance: SeasonChance,
    draw: WeatherDraw,
) -> Option<WeatherState> {
    if draw.branch < 30 {
        return None;
    }

    let mut next = current;

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

    next.weather_type = weather_for_roll(chance, draw.weather);
    let within_band = draw.intensity_bp.min(9_999) as f32 / 10_000.0;
    next.intensity = if next.weather_type == WEATHER_FINE {
        0.0
    } else if draw.branch < 90 {
        within_band * BAND_WIDTH
    } else if draw.band < 50 {
        within_band * BAND_WIDTH + 0.3334
    } else {
        within_band * BAND_WIDTH + 0.6667
    };
    changed(current, next)
}

fn changed(current: WeatherState, next: WeatherState) -> Option<WeatherState> {
    let next = WeatherState {
        weather_type: next.weather_type,
        intensity: normalize_intensity(next.weather_type, next.intensity),
    };
    (next != current).then_some(next)
}

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

fn apply_zone_weather(ctx: &ReducerContext, zone_id: u32, state: WeatherState, now_micros: i64) {
    let rows = ctx.db.game_zone_weather();
    let row = ZoneWeather {
        zone_id,
        weather_type: state.weather_type,
        intensity: state.intensity,
        changed_at_micros: now_micros,
    };
    let row = if rows.zone_id().find(zone_id).is_some() {
        rows.zone_id().update(row)
    } else {
        rows.insert(row)
    };
    crate::native::weather_changed(ctx, &row);
}

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

fn draw_weather(ctx: &ReducerContext) -> WeatherDraw {
    WeatherDraw {
        branch: ctx.random::<u32>() % 100,
        severity: ctx.random::<u32>() % 100,
        weather: ctx.random::<u32>() % 100 + 1,
        intensity_bp: ctx.random::<u32>() % 10_000,
        band: ctx.random::<u32>() % 100,
    }
}

#[reducer]
pub fn tick_weather(ctx: &ReducerContext, _schedule: WeatherSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let season = season_of(now_micros);
    let zones: Vec<ZoneWeatherChance> = ctx.db.game_weather().iter().collect();
    for weights in zones {
        let draw = draw_weather(ctx);
        let current = current_state(ctx, weights.zone_id);
        if let Some(next) = regenerate(current, weights.season(season), draw) {
            apply_zone_weather(ctx, weights.zone_id, next, now_micros);
        }
    }
}

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

pub(crate) const WEATHER_SEED_FAMILY: &str = "weather_seed";

type SeasonalPercents = [(u8, u8, u8); 4];

const TEMPORARY_ZONE_CLIMATE: &[(u32, SeasonalPercents)] = &[
    (12, [(40, 0, 10), (25, 0, 10), (40, 0, 15), (30, 10, 5)]),
    (40, [(15, 0, 5), (10, 0, 5), (15, 0, 5), (20, 0, 5)]),
];

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
    crate::import_meta::stamp(ctx, WEATHER_SEED_FAMILY, "", "", seeded);
    seeded
}

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

    fn draw(branch: u32) -> WeatherDraw {
        WeatherDraw {
            branch,
            severity: 99,
            weather: 100,
            intensity_bp: 0,
            band: 99,
        }
    }

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
