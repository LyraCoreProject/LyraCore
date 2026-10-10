use super::super::*;

#[derive(Default)]
pub(crate) struct WeatherState {
    /// Canned `game_zone_weather` rows, keyed by zone. A zone absent here has no row, which the
    /// Module defines as fine weather — the default, so a store that says nothing about weather
    /// behaves exactly like today's weatherless world.
    pub(crate) zone_weather: Vec<(u32, codec::ZoneWeatherView)>,
    /// When set, every weather read fails with this message — the "the Store could not answer"
    /// case, which must still leave the player with a sky rather than a failed login.
    pub(crate) weather_error: Option<String>,
}

impl WeatherStore for WorldFake {
    fn zone_weather(&self, zone_id: u32) -> Result<Option<codec::ZoneWeatherView>> {
        if let Some(e) = &self.weather.weather_error {
            return Err(anyhow!("{e}"));
        }
        Ok(self
            .weather
            .zone_weather
            .iter()
            .find(|(zone, _)| *zone == zone_id)
            .map(|(_, view)| *view))
    }
}
