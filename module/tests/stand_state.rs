mod support;

use support::{actor, number, Standalone};

const CHARACTER: u64 = 1;

fn spawned(test_name: &str) -> Standalone {
    let mut standalone = Standalone::start(test_name);
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone
}

fn unit_bytes_1(standalone: &Standalone) -> u32 {
    let rows = standalone.query_rows(&format!(
        "SELECT unit_bytes_1 FROM game_world_entity WHERE guid = {CHARACTER}"
    ));
    number(&rows[0], "unit_bytes_1")
}

fn set_stand_state(standalone: &Standalone, state: &str) {
    standalone.assert_call(
        "gw_set_stand_state",
        &[&actor(&CHARACTER.to_string()), state],
    );
}

fn refused(standalone: &Standalone, state: &str, tag: &str) {
    let result = standalone.call(
        "gw_set_stand_state",
        &[&actor(&CHARACTER.to_string()), state],
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "gw_set_stand_state({state}) should refuse with {tag}: {output}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_player_sits_and_stands_but_cannot_pick_a_server_state() {
    let standalone = spawned("stand-state");
    let upper_bytes = unit_bytes_1(&standalone) & !0xFF;

    set_stand_state(&standalone, "1");
    assert_eq!(unit_bytes_1(&standalone), upper_bytes | 1, "sitting");
    set_stand_state(&standalone, "8");
    assert_eq!(unit_bytes_1(&standalone), upper_bytes | 8, "kneeling");

    refused(&standalone, "2", "invalid stand state");
    refused(&standalone, "7", "invalid stand state");
    assert_eq!(unit_bytes_1(&standalone), upper_bytes | 8);

    set_stand_state(&standalone, "0");
    assert_eq!(unit_bytes_1(&standalone), upper_bytes, "standing");

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, dead = true WHERE guid = {CHARACTER}"
    ));
    set_stand_state(&standalone, "1");
    assert_eq!(unit_bytes_1(&standalone), upper_bytes | 1, "a ghost sits");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_stands_a_seated_player() {
    let standalone = spawned("stand-on-damage");
    let upper_bytes = unit_bytes_1(&standalone) & !0xFF;
    set_stand_state(&standalone, "3");
    assert_eq!(unit_bytes_1(&standalone), upper_bytes | 3, "sleeping");

    standalone.assert_call("debug_apply_damage", &[&CHARACTER.to_string(), "10", "0"]);
    assert_eq!(
        unit_bytes_1(&standalone),
        upper_bytes,
        "the hit stands the sleeper"
    );
}
