//! Weather delivery: the Store seam the world session reads a zone's sky through, and the one
//! place a zone id becomes an `SMSG_WEATHER` message.
//!
//! There is no client opcode here. The client never asks for weather and never reports its zone
//! (`CMSG_ZONEUPDATE` stays unhandled — the zone is the Module's answer, resolved from trusted
//! terrain). Weather reaches a client on exactly two paths: world entry, below, and the live
//! `game_zone_weather` relay in `stdb::world_view`.

use super::super::*;

pub(crate) trait WeatherStore: Send + Sync {
    /// The zone's current sky, or `None` when the zone has no weather at all. Missing data is not
    /// a failure: it means fine weather (story 10), and `Err` is reserved for a Store that could
    /// not answer.
    fn zone_weather(&self, zone_id: u32) -> Result<Option<codec::ZoneWeatherView>>;
}

impl WeatherStore for crate::stdb::Coordinator {
    fn zone_weather(&self, zone_id: u32) -> Result<Option<codec::ZoneWeatherView>> {
        crate::stdb::Coordinator::zone_weather(self, zone_id)
    }
}

/// The `SMSG_WEATHER` a client in `zone_id` should be holding right now.
///
/// Total by design: an absent row, an unresolved zone, and a Store that could not answer all mean
/// fine weather, so no weather problem can fail a world entry or a zone crossing. A read failure
/// is logged once at the point it happens rather than propagated, because the caller's only
/// alternative would be to drop the player's login over the sky.
pub(crate) fn zone_weather_message<St: WeatherStore + ?Sized>(
    store: &St,
    zone_id: u32,
    change: WeatherChangeType,
) -> ServerOpcodeMessage {
    let view = match store.zone_weather(zone_id) {
        Ok(view) => view.unwrap_or_default(),
        Err(e) => {
            log::warn!(
                "world: could not read weather for zone {zone_id} ({e:#}) — sending fine weather"
            );
            codec::ZoneWeatherView::default()
        }
    };
    ServerOpcodeMessage::SMSG_WEATHER(Box::new(codec::build_weather(view, change)))
}
