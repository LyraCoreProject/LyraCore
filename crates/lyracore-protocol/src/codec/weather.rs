use wow_world_messages::vanilla::{WeatherChangeType, WeatherType, SMSG_WEATHER};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ZoneWeatherView {
    pub weather_type: u8,
    pub intensity: f32,
}

const WEATHER_NOSOUND: u32 = 0;
const RAIN_SOUNDS: [u32; 3] = [8533, 8534, 8535];
const SNOW_SOUNDS: [u32; 3] = [8536, 8537, 8538];
const STORM_SOUNDS: [u32; 3] = [8556, 8557, 8558];

const MEDIUM_BAND: f32 = 0.40;
const HEAVY_BAND: f32 = 0.70;

const RAIN_AUDIBLE: f32 = 0.27;

fn band(grade: f32) -> usize {
    if grade < MEDIUM_BAND {
        0
    } else if grade < HEAVY_BAND {
        1
    } else {
        2
    }
}

fn weather_sound(weather_type: WeatherType, grade: f32) -> u32 {
    match weather_type {
        WeatherType::Rain if grade < RAIN_AUDIBLE => WEATHER_NOSOUND,
        WeatherType::Rain => RAIN_SOUNDS[band(grade)],
        WeatherType::Snow => SNOW_SOUNDS[band(grade)],
        WeatherType::Storm => STORM_SOUNDS[band(grade)],
        WeatherType::Fine => WEATHER_NOSOUND,
    }
}

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

    #[test]
    fn the_default_view_is_fine_weather() {
        let msg = build_weather(ZoneWeatherView::default(), WeatherChangeType::Instant);
        assert_eq!(msg.weather_type, WeatherType::Fine);
        assert_eq!(msg.grade, 0.0);
        assert_eq!(msg.sound_id, 0);
    }
}
