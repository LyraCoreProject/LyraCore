mod support;

use std::time::Duration;

use support::{poll_until, Standalone};

const CHARACTER_GUID: u64 = 1;
const START_FORWARD: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD;
const SET_PITCH: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_SET_PITCH;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pitch_change_coalesces_behind_a_staged_transition() {
    let mut standalone = Standalone::start("pitch-motion");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER_GUID.to_string()]);
    // The first movement arms the publisher; stop it only after it has published.
    move_with(&standalone, SET_PITCH, 1);
    assert!(
        poll_until(Duration::from_secs(5), || published_seq(&standalone)
            .is_some()),
        "scheduled publisher did not create the public motion row"
    );
    standalone.clear_schedule("game_motion_publish_schedule");
    let published = published_seq(&standalone);

    move_with(&standalone, START_FORWARD, 2);
    move_with(&standalone, SET_PITCH, 3);
    assert_eq!(
        staged_opcode(&standalone),
        Some(START_FORWARD.to_string()),
        "the staged transition stays queued"
    );
    assert_eq!(
        published_seq(&standalone),
        published,
        "a pitch change behind a transition must not publish in the movement transaction"
    );
}

fn published_seq(standalone: &Standalone) -> Option<String> {
    standalone
        .query_rows(&format!(
            "SELECT seq FROM game_entity_motion WHERE guid = {CHARACTER_GUID}"
        ))
        .first()
        .and_then(|row| row.get("seq").cloned())
}

fn move_with(standalone: &Standalone, opcode: u32, move_time_ms: u32) {
    let args = [
        support::actor(&CHARACTER_GUID.to_string()),
        opcode.to_string(),
        "[]".to_string(),
        "100.0".to_string(),
        "100.0".to_string(),
        "20.0".to_string(),
        "0.0".to_string(),
        move_time_ms.to_string(),
    ];
    standalone.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn staged_opcode(standalone: &Standalone) -> Option<String> {
    standalone
        .query_rows(&format!(
            "SELECT opcode FROM game_entity_motion_pending WHERE guid = {CHARACTER_GUID}"
        ))
        .first()
        .and_then(|row| row.get("opcode").cloned())
}
