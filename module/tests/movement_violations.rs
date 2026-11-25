mod support;

use lyracore_package_delta::packed_spell_effect_id;
use support::{actor, number, Standalone};

const CHARACTER: &str = "1";
const START_FORWARD: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD;
const HEARTBEAT: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;
const FALL_LAND: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND;
const FORWARD: &str = "[1,0,0,0]";
// FORWARD and SAFE_FALL (0x2000_0000) in the little-endian flag word.
const FORWARD_SLOW_FALLING: &str = "[1,0,0,32]";

// `game_movement_violation.kind` values.
const MOVE_VIOLATION_TELEPORT: u8 = 2;
const MOVE_VIOLATION_FALL: u8 = 3;
const MOVE_VIOLATION_GRAVITY_FLAG: u8 = 4;

const FEATHER_FALL: u32 = 5_091_001;
// vmangos AuraType SPELL_AURA_FEATHER_FALL, kept in an A_FLAG effect's p1.
const AURA_FEATHER_FALL: i32 = 105;

fn fixture(name: &str) -> (Standalone, [f32; 3]) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    // The reaper would delete the violations under test.
    shard.clear_schedule("game_event_reaper_schedule");
    // A GM's Characters log no movement violations, and the seeded Tester's Account is a GM.
    shard.assert_sql("UPDATE game_account SET gm_level = 0 WHERE username = 'TEST'");
    let rows = shard.query_rows("SELECT x, y, z FROM game_world_entity WHERE guid = 1");
    let [x, y, z] = ["x", "y", "z"].map(|axis| number(&rows[0], axis));
    (shard, [x, y, z])
}

fn send_move(shard: &Standalone, opcode: u32, flags: &str, at: [f32; 3], time_ms: u32) {
    let [x, y, z] = at;
    let args = [
        actor(CHARACTER),
        opcode.to_string(),
        flags.to_string(),
        x.to_string(),
        y.to_string(),
        z.to_string(),
        "1".to_string(),
        time_ms.to_string(),
    ];
    shard.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

// A movement body with no optional tail: flags, timestamp, pose and fall time.
fn landing_body(fall_time_ms: u32) -> String {
    let mut body = vec![0u8; 24];
    body.extend_from_slice(&fall_time_ms.to_le_bytes());
    format!(
        "[{}]",
        body.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
    )
}

fn violations(shard: &Standalone) -> Vec<u8> {
    shard
        .query_rows("SELECT kind FROM game_movement_violation WHERE guid = 1")
        .iter()
        .map(|row| number(row, "kind"))
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_zero_client_time_does_not_turn_the_teleport_check_off() {
    let (shard, [x, y, z]) = fixture("movement-violation-zero-time");
    send_move(&shard, START_FORWARD, FORWARD, [x, y, z], 1_000);
    send_move(&shard, START_FORWARD, FORWARD, [x + 1.0, y, z], 0);
    send_move(&shard, HEARTBEAT, FORWARD, [x + 100.0, y, z], 1_500);
    assert_eq!(violations(&shard), vec![MOVE_VIOLATION_TELEPORT]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_landing_further_down_than_its_fall_time_allows_is_a_fall_violation() {
    let (shard, [x, y, z]) = fixture("movement-violation-fall");
    send_move(&shard, START_FORWARD, FORWARD, [x, y, z], 1_000);

    // Two seconds of falling cover about 38.6 yd.
    let landed = [x, y, z - 30.0];
    send_move(&shard, FALL_LAND, &landing_body(2_000), landed, 3_000);
    assert_eq!(violations(&shard), Vec::<u8>::new(), "a covered fall");

    send_move(&shard, START_FORWARD, FORWARD, landed, 3_500);
    send_move(&shard, FALL_LAND, &landing_body(0), [x, y, z - 70.0], 4_000);
    assert_eq!(violations(&shard), vec![MOVE_VIOLATION_FALL]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_slow_fall_flag_is_a_gravity_violation_only_without_a_feather_fall_aura() {
    let (shard, [x, y, z]) = fixture("movement-violation-gravity-flag");
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &FEATHER_FALL),
            ("name", &"Fixture Feather Fall"),
            ("duration_ms", &600000),
            ("school_mask", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(FEATHER_FALL, 0)),
            ("spell_id", &FEATHER_FALL),
            ("kind", &lyracore_shared::spell::taxonomy::A_FLAG),
            ("p_1", &AURA_FEATHER_FALL),
        ],
    );
    shard.assert_call("debug_force_cast", &[CHARACTER, &FEATHER_FALL.to_string()]);
    send_move(&shard, START_FORWARD, FORWARD, [x, y, z], 1_000);

    send_move(
        &shard,
        HEARTBEAT,
        FORWARD_SLOW_FALLING,
        [x + 3.0, y, z],
        1_500,
    );
    assert_eq!(violations(&shard), Vec::<u8>::new(), "the Aura grants it");

    shard.assert_sql(&format!(
        "DELETE FROM game_aura WHERE spell_id = {FEATHER_FALL}"
    ));
    send_move(&shard, HEARTBEAT, FORWARD, [x + 6.0, y, z], 2_000);
    send_move(
        &shard,
        HEARTBEAT,
        FORWARD_SLOW_FALLING,
        [x + 9.0, y, z],
        2_500,
    );
    assert_eq!(violations(&shard), vec![MOVE_VIOLATION_GRAVITY_FLAG]);
}
