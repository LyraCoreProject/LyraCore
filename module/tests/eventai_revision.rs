mod support;

use std::thread;
use std::time::{Duration, Instant};

use support::Standalone;

const PLAYER_GUID: u64 = 1;
const WOLF_ENTRY: u32 = 51_000;
const WOLF_GUID: u64 = (0xF130_u64 << 48) | ((WOLF_ENTRY as u64) << 24) | 1;
const SOURCE_RULE_ID: u64 = 900_001;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn scheduled_visits_preserve_state_then_clean_a_reloaded_definition() {
    let standalone = Standalone::start("eventai-revision");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER_GUID.to_string()]);

    let first_rules = format!(
        "{SOURCE_RULE_ID},health:0:100,100,4294967295,repeat:60000:60000,all,ordinary,\
         phase:1+phase:2+phase:3+posture:18:0"
    );
    stage_fixture(&standalone, &first_rules);

    wait_for_verifier(&standalone, true);

    thread::sleep(Duration::from_millis(1_200));
    assert_verifier(&standalone, true);

    let changed_rules = format!("{SOURCE_RULE_ID},aggro,100,4294967295,once,all,ordinary,phase:7");
    stage_fixture(&standalone, &changed_rules);

    wait_for_verifier(&standalone, false);
}

fn stage_fixture(standalone: &Standalone, rules: &str) {
    let args = [
        WOLF_GUID.to_string(),
        PLAYER_GUID.to_string(),
        packed_definition(rules),
    ];
    standalone.assert_call(
        "debug_stage_eventai_revision_fixture",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn wait_for_verifier(standalone: &Standalone, expect_rule_state: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = verifier(standalone, expect_rule_state);
        if output.status.success() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "EventAI fixture did not reach the expected state\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn assert_verifier(standalone: &Standalone, expect_rule_state: bool) {
    let output = verifier(standalone, expect_rule_state);
    assert!(
        output.status.success(),
        "EventAI fixture verification failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn verifier(standalone: &Standalone, expect_rule_state: bool) -> std::process::Output {
    let args = [
        WOLF_GUID.to_string(),
        SOURCE_RULE_ID.to_string(),
        expect_rule_state.to_string(),
    ];
    standalone.call(
        "debug_verify_eventai_revision_fixture",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

fn packed_definition(rules: &str) -> String {
    let subject = format!("entry:{WOLF_ENTRY}");
    let material = format!("{subject}@{rules}");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(material.as_bytes());
    let revision = u64::from_le_bytes(
        hasher.finalize().as_bytes()[..8]
            .try_into()
            .expect("a BLAKE3 digest has at least eight bytes"),
    );
    format!("{subject}@{revision}@{rules}")
}
