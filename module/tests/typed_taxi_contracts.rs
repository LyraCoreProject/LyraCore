mod support;

use std::path::PathBuf;
use std::process::{Command, Output};

use support::Standalone;

const ACTOR: &str = "1";
const MISSING_ACTOR: &str = "999999";

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn gateway_taxi_gates_keep_refusals_typed_and_invariants_fatal() {
    let wasm = build_module_bytes();
    let standalone = Standalone::start("typed-taxi-contracts");
    standalone.publish_module_bytes(&wasm);
    assert_loot_boundary_failure(&standalone, ACTOR, "loot:boundary_operator_rejected");
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[ACTOR]);
    standalone.assert_sql(
        "INSERT INTO game_active_taxi_flight \
         (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,\
          current_node_index,started_micros) \
         VALUES (1,5090102,5090100,5090101,1147,25,0,1)",
    );

    for (reducer, args, tag) in taxi_cases() {
        assert_refusal(&standalone, reducer, args, tag);
    }
    assert_loot_boundary_failure(&standalone, MISSING_ACTOR, "loot:boundary_missing_actor");

    for (reducer, args) in [
        ("gw_trainer_buy", &[MISSING_ACTOR, "0", "0"][..]),
        ("gw_use_item", &[MISSING_ACTOR, "0"][..]),
        ("gw_group_leave", &[MISSING_ACTOR][..]),
        ("gw_add_friend", &[MISSING_ACTOR, "2"][..]),
    ] {
        let output = standalone.call(reducer, args);
        let text = failed_text(reducer, output);
        assert!(text.contains("mover not in world"), "{reducer}: {text}");
        for prefix in ["trainer:", "item:", "loot:", "group:", "social:"] {
            assert!(!text.contains(prefix), "{reducer} leaked {prefix}: {text}");
        }
    }

    standalone.assert_sql("DELETE FROM game_active_taxi_flight WHERE character_guid = 1");
    standalone.assert_call("realm_group_op", &["0", "1", "2", "0", "0"]);
    standalone.assert_call("realm_group_op", &["1", "2", "0", "0", "0"]);
    standalone.assert_call("realm_group_op", &["0", "3", "4", "0", "0"]);
    standalone.assert_call("realm_group_op", &["1", "4", "0", "0", "0"]);
    standalone.assert_call("realm_group_op", &["0", "3", "5", "0", "0"]);
    standalone.assert_sql("DELETE FROM game_group WHERE leader_guid = 3");

    let valid_group_before = standalone.query_rows("SELECT * FROM game_group WHERE leader_guid = 1");
    for args in [
        &["0", "1", "3", "0", "0"][..],
        &["1", "5", "0", "0", "0"][..],
        &["3", "3", "0", "0", "0"][..],
        &["4", "1", "3", "0", "0"][..],
        &["5", "1", "3", "2", "2"][..],
        &["0", "3", "6", "0", "0"][..],
    ] {
        assert_group_invariant(&standalone, args);
    }
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_group WHERE leader_guid = 1"),
        valid_group_before,
        "failed membership checks must not change the valid Group"
    );
    assert!(
        standalone
            .query_rows("SELECT * FROM game_group_invite WHERE target_guid = 3")
            .is_empty(),
        "a dangling target must not receive an invite"
    );
    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_group_invite WHERE target_guid = 5")
            .len(),
        1,
        "a failed accept must roll its invite deletion back"
    );
    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_group_member WHERE character_guid = 3")
            .len(),
        1,
        "a failed leave or kick must preserve the dangling membership for repair"
    );
}

fn assert_group_invariant(standalone: &Standalone, args: &[&str]) {
    let text = failed_text("realm_group_op", standalone.call("realm_group_op", args));
    assert!(text.contains("group invariant failed"), "{args:?}: {text}");
    assert!(!text.contains("group:"), "{args:?}: {text}");
}

fn taxi_cases() -> Vec<(&'static str, &'static [&'static str], &'static str)> {
    vec![
        ("gw_trainer_buy", &[ACTOR, "0", "0"], "trainer:unavailable"),
        ("gw_use_item", &[ACTOR, "0"], "item:not_right_now"),
        ("gw_equip_item", &[ACTOR, "0"], "item:not_right_now"),
        ("gw_move_item", &[ACTOR, "0", "1"], "item:not_right_now"),
        ("gw_unequip_item", &[ACTOR, "0"], "item:not_right_now"),
        (
            "gw_take_loot",
            &[ACTOR, "0", "0"],
            "loot:looter_unavailable",
        ),
        (
            "gw_open_creature_loot",
            &[ACTOR, "0"],
            "loot:looter_unavailable",
        ),
        ("gw_loot_money", &[ACTOR, "0"], "loot:looter_unavailable"),
        (
            "gw_use_gameobject",
            &[ACTOR, "0"],
            "loot:looter_unavailable",
        ),
        ("gw_skin", &[ACTOR, "0"], "loot:looter_unavailable"),
        (
            "gw_loot_roll",
            &[ACTOR, "0", "0", "0"],
            "loot:looter_unavailable",
        ),
        (
            "gw_loot_master_give",
            &[ACTOR, "0", "0", "0"],
            "loot:looter_unavailable",
        ),
        (
            "gw_accept_group_invite",
            &[ACTOR],
            "group:actor_unavailable",
        ),
        (
            "gw_party_chat",
            &[ACTOR, "\"taxi\""],
            "group:actor_unavailable",
        ),
        ("gw_group_invite", &[ACTOR, "2"], "group:actor_unavailable"),
        ("gw_group_decline", &[ACTOR], "group:actor_unavailable"),
        ("gw_group_leave", &[ACTOR], "group:actor_unavailable"),
        (
            "gw_group_uninvite",
            &[ACTOR, "2"],
            "group:actor_unavailable",
        ),
        (
            "gw_group_loot_method",
            &[ACTOR, "0", "0", "0"],
            "group:actor_unavailable",
        ),
        ("gw_add_friend", &[ACTOR, "2"], "social:actor_unavailable"),
        ("gw_del_friend", &[ACTOR, "2"], "social:actor_unavailable"),
        ("gw_add_ignore", &[ACTOR, "2"], "social:actor_unavailable"),
        ("gw_del_ignore", &[ACTOR, "2"], "social:actor_unavailable"),
    ]
}

fn assert_loot_boundary_failure(standalone: &Standalone, actor: &str, tag: &str) {
    for (reducer, args, refusal) in taxi_cases() {
        if !refusal.starts_with("loot:") {
            continue;
        }
        let mut args = args.to_vec();
        args[0] = actor;
        let text = failed_text(reducer, standalone.call(reducer, &args));
        assert!(text.contains(tag), "{reducer} did not return {tag}: {text}");
    }
}

fn assert_refusal(standalone: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let text = failed_text(reducer, standalone.call(reducer, args));
    assert!(text.contains(tag), "{reducer} did not return {tag}: {text}");
}

fn failed_text(reducer: &str, output: Output) -> String {
    let text = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "{reducer} unexpectedly succeeded: {text}"
    );
    text
}

fn build_module_bytes() -> Vec<u8> {
    let module_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = module_dir.parent().unwrap();
    let output = Command::new("cargo")
        .current_dir(workspace)
        .args([
            "build",
            "--locked",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "lyracore-module",
            "--features=debug_reducers",
            "--message-format=json-render-diagnostics",
        ])
        .output()
        .expect("failed to run the Wasm preflight build");
    assert!(
        output.status.success(),
        "the Wasm preflight build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let wasm = String::from_utf8(output.stdout)
        .expect("Cargo artifact output was not UTF-8")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == "lyracore_module")
        .filter_map(|message| message["filenames"].as_array().cloned())
        .flatten()
        .filter_map(|filename| filename.as_str().map(PathBuf::from))
        .find(|path| path.extension().is_some_and(|extension| extension == "wasm"))
        .expect("Cargo did not report the built Module Wasm artifact");
    let bytes = std::fs::read(&wasm).expect("the Wasm preflight output is missing");
    assert!(bytes.starts_with(b"\0asm"), "the built module is not Wasm");
    bytes
}
