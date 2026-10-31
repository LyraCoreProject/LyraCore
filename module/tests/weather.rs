mod support;

use std::collections::BTreeMap;

use support::Standalone;

const ELWYNN: &str = "12";
const WESTFALL: &str = "40";

const FINE: &str = "0";
const RAIN: &str = "1";
const SNOW: &str = "2";

fn zone_weather(standalone: &Standalone, zone_id: &str) -> Vec<BTreeMap<String, String>> {
    standalone.query_rows(&format!(
        "SELECT * FROM game_zone_weather WHERE zone_id = {zone_id}"
    ))
}

fn one_row(standalone: &Standalone, zone_id: &str) -> BTreeMap<String, String> {
    let rows = zone_weather(standalone, zone_id);
    assert_eq!(rows.len(), 1, "zone {zone_id} should have exactly one row");
    rows.into_iter().next().unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn forced_weather_is_per_zone_and_a_refusal_leaves_every_row_unchanged() {
    let mut standalone = Standalone::start("weather-forced");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    assert_eq!(
        standalone.query_rows("SELECT * FROM game_weather").len(),
        2,
        "init seeds the two temporary climate rows"
    );
    assert!(
        standalone
            .query_rows("SELECT * FROM game_zone_weather")
            .is_empty(),
        "a zone that has never rolled has no row, and reads as fine weather"
    );

    standalone.assert_call("gw_force_zone_weather", &[ELWYNN, RAIN, "0.5"]);
    let elwynn = one_row(&standalone, ELWYNN);
    assert_eq!(elwynn["weather_type"], RAIN);
    assert!(
        zone_weather(&standalone, WESTFALL).is_empty(),
        "forcing Elwynn must not give Westfall weather"
    );

    for (zone_id, weather_type, intensity, why) in [
        (
            ELWYNN,
            "9",
            "0.5",
            "a weather type outside fine/rain/snow/storm",
        ),
        (ELWYNN, RAIN, "2.0", "an intensity above the vanilla range"),
        (
            "3456",
            RAIN,
            "0.5",
            "a zone with no climate data has no weather to force",
        ),
    ] {
        let refused = standalone.call("gw_force_zone_weather", &[zone_id, weather_type, intensity]);
        assert!(
            !refused.status.success(),
            "{why} must be Refused\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&refused.stdout),
            String::from_utf8_lossy(&refused.stderr),
        );
        assert_eq!(
            one_row(&standalone, ELWYNN),
            elwynn,
            "the Refusal of {why} changed Elwynn's durable row"
        );
        assert_eq!(
            standalone
                .query_rows("SELECT * FROM game_zone_weather")
                .len(),
            1,
            "the Refusal of {why} created a row"
        );
    }

    standalone.assert_call("gw_force_zone_weather", &[WESTFALL, SNOW, "0.25"]);
    assert_eq!(one_row(&standalone, WESTFALL)["weather_type"], SNOW);
    assert_eq!(
        one_row(&standalone, ELWYNN),
        elwynn,
        "Westfall's weather must not reach Elwynn's row"
    );

    standalone.assert_call("gw_force_zone_weather", &[ELWYNN, FINE, "0.0"]);
    let cleared = one_row(&standalone, ELWYNN);
    assert_eq!(cleared["weather_type"], FINE);
    assert_eq!(
        cleared["intensity"], "0",
        "fine weather stores no grade whatever intensity was asked for"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_weather_roll_and_its_climate_survive_a_republish() {
    let mut standalone = Standalone::start("weather-republish");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("gw_force_zone_weather", &[ELWYNN, RAIN, "0.75"]);

    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_weather_schedule")
            .len(),
        1,
        "init arms exactly one weather roll"
    );

    standalone.publish_module();
    standalone.assert_call("debug_repair_after_publish", &[]);

    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_weather_schedule")
            .len(),
        1,
        "the repair pass re-arms the roll without leaving a second schedule row behind"
    );
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_weather").len(),
        2,
        "the climate seed is only-if-empty, so the repair pass does not duplicate it"
    );
    assert_eq!(
        one_row(&standalone, ELWYNN)["weather_type"],
        RAIN,
        "a zone keeps the weather it had across a republish"
    );

    standalone.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_weather_schedule")
            .len(),
        1
    );
    assert_eq!(standalone.query_rows("SELECT * FROM game_weather").len(), 2);
}
