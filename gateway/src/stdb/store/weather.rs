//! `Coordinator`'s [`WeatherStore`] adapter.

use anyhow::Result;

use crate::codec;
use crate::codec::ZoneWeatherView;
use crate::stdb::bindings::*;
use crate::stdb::Coordinator;
use crate::world::WeatherStore;

impl WeatherStore for crate::stdb::Coordinator {
    fn zone_weather(&self, zone_id: u32) -> Result<Option<codec::ZoneWeatherView>> {
        crate::stdb::Coordinator::zone_weather(self, zone_id)
    }
}

impl Coordinator {
    /// One zone's current sky, read from the shard's `game_zone_weather` cache.
    ///
    /// `None` means the zone has no row, which the Module defines as fine weather; zone 0 is the
    /// unresolved zone (`WorldEntity.zone_id`'s "no terrain covers this position") and never has
    /// one. Callers that only need a packet go through
    /// [`crate::world::zone_weather_message`], which folds both cases into fine weather.
    pub fn zone_weather(&self, zone_id: u32) -> Result<Option<ZoneWeatherView>, anyhow::Error> {
        if zone_id == 0 {
            return Ok(None);
        }
        Ok(self
            .0
            .coord()
            .conn
            .db
            .game_zone_weather()
            .zone_id()
            .find(&zone_id)
            .map(|row| ZoneWeatherView {
                weather_type: row.weather_type,
                intensity: row.intensity,
            }))
    }
}
