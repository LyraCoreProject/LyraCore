//! A Route Path renewed mid-leg starts where the client renders the mover at the renewal, not at
//! the position the last advance firing stored. The renewal lands between firings, which only a
//! real shard shows.

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use lyracore_shared::spatial::{grid_cell, GRID_CELL_SIZE, MAP_COORD_MAX};
use support::Standalone;

/// Long enough to span several firings, short enough to stay inside one 50 yd cell.
const LEG_YD: f32 = 20.0;

type Row = BTreeMap<String, String>;

fn number<T: std::str::FromStr>(row: &Row, column: &str) -> T {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is not a number in {row:?}"))
}

fn leg(shard: &Standalone, guid: &str) -> Row {
    let mut legs = shard.query_rows(&format!(
        "SELECT sx, sy, sz, dx, dy, dz, start_micros, dur_ms FROM game_creature_spline \
         WHERE guid = {guid}"
    ));
    assert_eq!(legs.len(), 1, "{legs:?}");
    legs.remove(0)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_path_renewed_mid_leg_starts_where_the_client_renders_the_mover() {
    let mut shard = Standalone::start("creature-path-renewal");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "5"]);
    // With no Character left, the wolf stays out of combat, so its stored row may lag its leg.
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");

    let wolf = shard.query_rows("SELECT * FROM game_world_entity WHERE entry = 51000");
    assert_eq!(wolf.len(), 1, "{wolf:?}");
    let guid = wolf[0]["guid"].clone();
    let (x, y, z): (f32, f32, f32) = (
        number(&wolf[0], "x"),
        number(&wolf[0], "y"),
        number(&wolf[0], "z"),
    );
    let (grid_x, _) = grid_cell(x, y);
    let centre_x = MAP_COORD_MAX - (grid_x as f32 + 0.5) * GRID_CELL_SIZE;
    let destination = [
        (x + LEG_YD.copysign(centre_x - x)).to_string(),
        y.to_string(),
        z.to_string(),
    ];
    let path_args = [
        guid.as_str(),
        &destination[0],
        &destination[1],
        &destination[2],
        "false",
    ];

    shard.assert_call("debug_creature_path", &path_args);
    let first = leg(&shard, &guid);
    let dur_ms: u32 = number(&first, "dur_ms");
    assert!(dur_ms >= 4_000, "{LEG_YD} yd walked in only {dur_ms} ms");

    // Renew part way along the leg, at no particular phase of the 0.5 s firing.
    std::thread::sleep(Duration::from_millis(1_730));
    shard.assert_call("debug_creature_path", &path_args);
    let renewed = leg(&shard, &guid);

    let walked = ((number::<i64>(&renewed, "start_micros") - number::<i64>(&first, "start_micros"))
        as f32
        / (dur_ms as f32 * 1_000.0))
        .clamp(0.0, 1.0);
    assert!(
        walked > 0.0 && walked < 1.0,
        "the renewal must land inside the first leg"
    );
    let lerp = |from: &str, to: &str| {
        let (from, to): (f32, f32) = (number(&first, from), number(&first, to));
        from + (to - from) * walked
    };
    let rendered = (lerp("sx", "dx"), lerp("sy", "dy"), lerp("sz", "dz"));
    let start: (f32, f32, f32) = (
        number(&renewed, "sx"),
        number(&renewed, "sy"),
        number(&renewed, "sz"),
    );
    assert!(
        (start.0 - rendered.0).abs() < 0.01
            && (start.1 - rendered.1).abs() < 0.01
            && (start.2 - rendered.2).abs() < 0.01,
        "the renewed path must start at {rendered:?}, where the client rendered the wolf, but it \
         starts at {start:?}"
    );
}
