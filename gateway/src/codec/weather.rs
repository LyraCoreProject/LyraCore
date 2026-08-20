//! Weather wire mapping: one zone's stored sky to `SMSG_WEATHER`.
//!
//! The Module owns the durable state and never mentions sounds; the sound band is a client-facing
//! protocol detail, so it lives here with the rest of the packet mapping.

use wow_world_messages::vanilla::{WeatherChangeType, WeatherType, SMSG_WEATHER};

/// A minimal mirror of the `game_zone_weather` row the encoder needs — the same shape as
/// [`super::EntityView`] and the other row views, decoupled from the module's table type.
///
/// `weather_type` is the vanilla wire discriminant (0 fine, 1 rain, 2 snow, 3 storm) and
/// `intensity` is the packet's grade. [`Default`] is fine weather, which is also what a zone with
/// no row means.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ZoneWeatherView {
    pub weather_type: u8,
    pub intensity: f32,
}

// CMaNGOS 1.12 weather sound ids (`WeatherSounds` in cmangos-classic `Weather.cpp`). The client
// resolves these against its own sound catalogue; sending 0 means "no weather sound".
const WEATHER_NOSOUND: u32 = 0;
const RAIN_SOUNDS: [u32; 3] = [8533, 8534, 8535];
const SNOW_SOUNDS: [u32; 3] = [8536, 8537, 8538];
const STORM_SOUNDS: [u32; 3] = [8556, 8557, 8558];

/// Grade thresholds separating the light / medium / heavy sound bands, from cmangos-classic
/// `Weather::GetSound()`. Both comparisons are strict `<`, so a grade sitting exactly on a
/// boundary belongs to the HEAVIER band.
const MEDIUM_BAND: f32 = 0.40;
const HEAVY_BAND: f32 = 0.70;

/// Rain alone has a fourth, silent band. CMaNGOS's own comment: rain below this grade is not drawn
/// on every client weather-detail setting, and rain without sound beats sound without rain. Snow
/// and storm have no such cutoff — their lightest grade still carries its light sound.
const RAIN_AUDIBLE: f32 = 0.27;

/// The band index of `grade` within a three-sound family: 0 light, 1 medium, 2 heavy.
fn band(grade: f32) -> usize {
    if grade < MEDIUM_BAND {
        0
    } else if grade < HEAVY_BAND {
        1
    } else {
        2
    }
}

/// The 1.12 sound id for a weather type at a grade. An unknown type is fine weather, which is
/// silent.
fn weather_sound(weather_type: WeatherType, grade: f32) -> u32 {
    match weather_type {
        WeatherType::Rain if grade < RAIN_AUDIBLE => WEATHER_NOSOUND,
        WeatherType::Rain => RAIN_SOUNDS[band(grade)],
        WeatherType::Snow => SNOW_SOUNDS[band(grade)],
        WeatherType::Storm => STORM_SOUNDS[band(grade)],
        WeatherType::Fine => WEATHER_NOSOUND,
    }
}

/// Build `SMSG_WEATHER` for a zone's current sky.
///
/// Degrades rather than refuses, because the alternative is a broken world entry: an unknown
/// weather discriminant becomes fine weather, and a grade the Module's Gates should already have
/// kept finite and inside `0.0..=1.0` is clamped again here — remote state is untrusted input, and
/// a NaN grade reaches the client as a packet no 1.12 client can render safely. Fine weather is
/// always sent at grade 0 whatever the row claims, matching the client's own expectation that a
/// clear sky has no intensity.
///
/// `change` is [`WeatherChangeType::Instant`] for a synchronizing send (world entry, zone entry,
/// forced weather) so stale client weather is replaced at once, and
/// [`WeatherChangeType::Smooth`] for a scheduled change that should fade in.
pub fn build_weather(view: ZoneWeatherView, change: WeatherChangeType) -> SMSG_WEATHER {
    let weather_type =
        WeatherType::try_from(u32::from(view.weather_type)).unwrap_or(WeatherType::Fine);
    let grade = if weather_type == WeatherType::Fine || !view.intensity.is_finite() {
        0.0
    } else {
        view.intensity.clamp(0.0, 1.0)
    };
    SMSG_WEATHER {
        weather_type,
        grade,
        sound_id: weather_sound(weather_type, grade),
        change,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sky(weather_type: u8, intensity: f32) -> ZoneWeatherView {
        ZoneWeatherView {
            weather_type,
            intensity,
        }
    }

    /// The band table, pinned against cmangos-classic `Weather::GetSound()` rather than recomputed
    /// from the implementation: rain is silent below 0.27, and every boundary value belongs to the
    /// heavier band because CMaNGOS compares with a strict `<`.
    #[test]
    fn rain_sound_bands_match_cmangos() {
        let sound = |grade| build_weather(sky(1, grade), WeatherChangeType::Smooth).sound_id;
        assert_eq!(sound(0.0001), 0);
        assert_eq!(sound(0.269), 0);
        assert_eq!(sound(0.27), 8533);
        assert_eq!(sound(0.399), 8533);
        assert_eq!(sound(0.40), 8534);
        assert_eq!(sound(0.699), 8534);
        assert_eq!(sound(0.70), 8535);
        assert_eq!(sound(0.9999), 8535);
    }

    /// Snow and storm have no silent band — the lightest grade still carries its light sound.
    #[test]
    fn snow_and_storm_sound_bands_match_cmangos() {
        let sound =
            |kind, grade| build_weather(sky(kind, grade), WeatherChangeType::Smooth).sound_id;
        assert_eq!(sound(2, 0.0001), 8536);
        assert_eq!(sound(2, 0.40), 8537);
        assert_eq!(sound(2, 0.70), 8538);
        assert_eq!(sound(3, 0.0001), 8556);
        assert_eq!(sound(3, 0.40), 8557);
        assert_eq!(sound(3, 0.70), 8558);
    }

    #[test]
    fn fine_weather_is_silent_and_gradeless() {
        let msg = build_weather(sky(0, 0.8), WeatherChangeType::Instant);
        assert_eq!(msg.weather_type, WeatherType::Fine);
        assert_eq!(msg.grade, 0.0);
        assert_eq!(msg.sound_id, 0);
        assert_eq!(msg.change, WeatherChangeType::Instant);
    }

    #[test]
    fn an_unknown_weather_discriminant_becomes_fine_weather() {
        let msg = build_weather(sky(9, 0.5), WeatherChangeType::Smooth);
        assert_eq!(msg.weather_type, WeatherType::Fine);
        assert_eq!(msg.grade, 0.0);
        assert_eq!(msg.sound_id, 0);
    }

    /// Story 11: the grade an unmodified 1.12 client receives is always finite and inside the
    /// vanilla range, however damaged the row that produced it.
    #[test]
    fn the_grade_sent_is_always_finite_and_inside_the_vanilla_range() {
        for intensity in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -3.0, 42.0, 1.0] {
            let msg = build_weather(sky(2, intensity), WeatherChangeType::Smooth);
            assert!(
                msg.grade.is_finite() && (0.0..=1.0).contains(&msg.grade),
                "grade {} escaped the vanilla range for intensity {intensity}",
                msg.grade
            );
        }
        assert_eq!(
            build_weather(sky(2, 42.0), WeatherChangeType::Smooth).grade,
            1.0
        );
        assert_eq!(
            build_weather(sky(2, -3.0), WeatherChangeType::Smooth).grade,
            0.0
        );
        assert_eq!(
            build_weather(sky(2, f32::NAN), WeatherChangeType::Smooth).grade,
            0.0
        );
    }

    /// A zone with no row reads as fine weather, so the view's own default has to be fine.
    #[test]
    fn the_default_view_is_fine_weather() {
        let msg = build_weather(ZoneWeatherView::default(), WeatherChangeType::Instant);
        assert_eq!(msg.weather_type, WeatherType::Fine);
        assert_eq!(msg.grade, 0.0);
        assert_eq!(msg.sound_id, 0);
    }
}
