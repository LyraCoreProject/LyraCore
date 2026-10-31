mod support;

use support::Standalone;

fn assert_call_fails(standalone: &Standalone, reducer: &str, args: &[&str]) {
    let output = standalone.call(reducer, args);
    assert!(
        !output.status.success(),
        "{reducer} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn durable_choice_turn_in_grants_the_selected_item_with_all_other_rewards() {
    let mut standalone = Standalone::start("quest-choice-reward");
    standalone.publish_module();
    for (reducer, args) in [
        ("claim_operator", &[][..]),
        ("install_guid_range", &["0"][..]),
        ("debug_spawn_player_entity", &["1"][..]),
        ("debug_stage_choice_reward_fixture", &["false"][..]),
        ("debug_turn_in_choice_reward_fixture", &["0"][..]),
        ("debug_verify_choice_reward_fixture", &["0"][..]),
    ] {
        standalone.assert_call(reducer, args);
    }

    assert_call_fails(&standalone, "debug_turn_in_choice_reward_fixture", &["0"]);
    standalone.assert_call("debug_verify_choice_reward_fixture", &["0"]);

    for (reducer, args) in [
        ("debug_stage_choice_reward_fixture", &["false"][..]),
        ("debug_turn_in_choice_reward_fixture", &["1"][..]),
        ("debug_verify_choice_reward_fixture", &["1"][..]),
    ] {
        standalone.assert_call(reducer, args);
    }

    standalone.assert_call("debug_stage_choice_reward_fixture", &["false"]);
    assert_call_fails(&standalone, "debug_turn_in_choice_reward_fixture", &["2"]);
    standalone.assert_call("debug_verify_choice_reward_refusal_fixture", &["0"]);

    standalone.assert_call("debug_stage_choice_reward_fixture", &["true"]);
    assert_call_fails(&standalone, "debug_turn_in_choice_reward_fixture", &["0"]);
    standalone.assert_call("debug_verify_choice_reward_refusal_fixture", &["15"]);
}
