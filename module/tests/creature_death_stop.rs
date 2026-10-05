//! A creature killed on a leg stops where the client renders it at the kill, not at the position
//! the last advance firing stored. The kill lands between firings, which only a real shard shows.

mod support;

use std::time::Duration;

use spacetimedb::Timestamp;
use support::{leg, leg_destination, lone_wolf, number, Row, Standalone, LEG_YD};

/// `combat::death::CORPSE_DECAY_MICROS`: the kill arms the corpse decay this long after itself.
const CORPSE_DECAY_MICROS: i64 = 60_000_000;

fn timestamp_micros(value: &str) -> i64 {
    Timestamp::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("invalid durable timestamp {value:?}: {error}"))
        .to_micros_since_unix_epoch()
}

fn assert_stored_at(row: &Row, at: (f32, f32, f32), when: &str) {
    let stored: (f32, f32, f32) = (number(row, "x"), number(row, "y"), number(row, "z"));
    assert!(
        (stored.0 - at.0).abs() < 0.01
            && (stored.1 - at.1).abs() < 0.01
            && (stored.2 - at.2).abs() < 0.01,
        "{when}: the corpse must rest at {at:?}, where the client rendered the kill, but it is at \
         {stored:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_creature_killed_mid_leg_stops_where_the_client_renders_it() {
    let mut shard = Standalone::start("creature-death-stop");
    let wolf = lone_wolf(&mut shard);
    let guid = wolf["guid"].clone();
    let (dest_x, y, z) = leg_destination(&wolf);

    shard.assert_call(
        "debug_encounter_move",
        &[
            &guid,
            &dest_x.to_string(),
            &y.to_string(),
            &z.to_string(),
            "false",
        ],
    );
    let leg = leg(&shard, &guid);
    let dur_ms: u32 = number(&leg, "dur_ms");
    assert!(dur_ms >= 4_000, "{LEG_YD} yd walked in only {dur_ms} ms");

    // Kill part way along the leg, at no particular phase of the 0.5 s firing.
    std::thread::sleep(Duration::from_millis(1_730));
    shard.assert_call("debug_kill_creature", &["1", &guid]);

    let spawn = shard.query_rows(&format!(
        "SELECT despawn_at FROM game_creature_spawn WHERE guid = {guid}"
    ));
    assert_eq!(spawn.len(), 1, "{spawn:?}");
    let killed_micros = timestamp_micros(&spawn[0]["despawn_at"]) - CORPSE_DECAY_MICROS;
    let started_micros: i64 = number(&leg, "start_micros");
    let walked =
        ((killed_micros - started_micros) as f32 / (dur_ms as f32 * 1_000.0)).clamp(0.0, 1.0);
    assert!(walked < 1.0, "the kill must land before the leg ends");
    let lerp = |from: &str, to: &str| {
        let (from, to): (f32, f32) = (number(&leg, from), number(&leg, to));
        from + (to - from) * walked
    };
    let rendered = (lerp("sx", "dx"), lerp("sy", "dy"), lerp("sz", "dz"));

    let row_query = format!("SELECT * FROM game_world_entity WHERE guid = {guid}");
    let corpse = shard.query_rows(&row_query).remove(0);
    assert_eq!(corpse["dead"], "true", "{corpse:?}");
    assert_stored_at(&corpse, rendered, "at the kill");
    // The next firings play the stop leg. A stop packet anywhere else would move the corpse there.
    std::thread::sleep(Duration::from_millis(1_200));
    assert_stored_at(
        &shard.query_rows(&row_query).remove(0),
        rendered,
        "after the stop leg played",
    );
}
