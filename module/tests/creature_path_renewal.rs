//! A Route Path renewed mid-leg starts where the client renders the mover at the renewal, not at
//! the position the last advance firing stored. The renewal lands between firings, which only a
//! real shard shows.

mod support;

use std::time::Duration;

use support::{assert_near, leg, leg_destination, lone_wolf, number, position, Standalone, LEG_YD};

/// The stored row of an out-of-combat walker stays at the leg start until it drifts 4 yd, which a
/// walk of 2.5 yd/s reaches at about 1.6 s. A renewal this long after the first path lands well
/// before that.
const RENEW_AFTER: Duration = Duration::from_millis(900);
/// The least the stored row must lag the drawn point for the renewal to prove anything.
const MIN_LAG_YD: f32 = 1.0;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_path_renewed_mid_leg_starts_where_the_client_renders_the_mover() {
    let mut shard = Standalone::start("creature-path-renewal");
    let wolf = lone_wolf(&mut shard);
    let guid = wolf["guid"].clone();
    let destination = leg_destination(&wolf);
    let destination = [
        destination.0.to_string(),
        destination.1.to_string(),
        destination.2.to_string(),
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

    std::thread::sleep(RENEW_AFTER);
    let row_query = format!("SELECT * FROM game_world_entity WHERE guid = {guid}");
    let stored_before = position(&shard.query_rows(&row_query)[0]);
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
    let lag = ((rendered.0 - stored_before.0).powi(2)
        + (rendered.1 - stored_before.1).powi(2)
        + (rendered.2 - stored_before.2).powi(2))
    .sqrt();
    assert!(
        lag >= MIN_LAG_YD,
        "the stored row must lag the drawn point {rendered:?} before the renewal, or the test \
         cannot show the bug, but it is at {stored_before:?}"
    );

    let start: (f32, f32, f32) = (
        number(&renewed, "sx"),
        number(&renewed, "sy"),
        number(&renewed, "sz"),
    );
    assert_near(
        start,
        rendered,
        "the renewed path must start where the client rendered the wolf",
    );
    assert_near(
        position(&shard.query_rows(&row_query)[0]),
        start,
        "the stored row must hold the renewed path's start",
    );
}
