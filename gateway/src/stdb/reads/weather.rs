use super::*;
use crate::codec::ZoneWeatherView;

impl super::super::connection::Coordinator {
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
