//! `teardown_package` against a real Shard, with the reference Package as the Package torn down.

mod support;

use support::Standalone;

const PACKAGE: &str = "example";

fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard
}

fn count(shard: &Standalone, query: &str) -> usize {
    shard.query_rows(query).len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn teardown_leaves_a_package_character_dormant_and_forgets_the_package_config() {
    let shard = stage("package-teardown-dormant");
    shard.assert_call(
        "debug_create_package_character",
        &[&arg(PACKAGE), &arg("Sleeper"), "1", "1"],
    );
    let character =
        shard.query_rows("SELECT guid, account_id FROM game_character WHERE name = 'Sleeper'");
    let guid = character[0]["guid"].clone();
    let account_id = character[0]["account_id"].clone();
    shard.assert_call("debug_spawn_player_entity", &[&guid]);
    shard.assert_call("debug_set_sessionless_action_consent", &[&guid, "true"]);
    shard.assert_call(
        "debug_emit_sessionless_group_intent",
        &[&guid, "1", "false"],
    );
    shard.assert_call(
        "set_package_config",
        &[&arg(PACKAGE), &arg("greeting"), &arg("hello"), "true"],
    );

    shard.assert_call("teardown_package", &[&arg(PACKAGE)]);

    for (what, query) in [
        ("live entity", format!("SELECT guid FROM game_world_entity WHERE guid = {guid}")),
        (
            "Sessionless Action Consent",
            format!("SELECT character_guid FROM game_sessionless_action_consent WHERE character_guid = {guid}"),
        ),
        (
            "Group Intent",
            format!("SELECT id FROM game_bot_invite_intent WHERE inviter_guid = {guid}"),
        ),
        (
            "Package Config",
            format!("SELECT id FROM game_package_config WHERE package_name = '{PACKAGE}'"),
        ),
    ] {
        assert_eq!(count(&shard, &query), 0, "teardown left the {what}");
    }
    let dormant = shard.query_rows(&format!(
        "SELECT online FROM game_character WHERE guid = {guid}"
    ));
    assert_eq!(dormant.len(), 1, "teardown deleted the Character");
    assert_eq!(dormant[0]["online"], "false");
    assert_eq!(
        count(
            &shard,
            &format!("SELECT id FROM game_account WHERE id = {account_id}")
        ),
        1,
        "teardown deleted the Package-owned Account"
    );
    assert_eq!(
        count(
            &shard,
            &format!(
                "SELECT package_name FROM game_package_teardown WHERE package_name = '{PACKAGE}'"
            )
        ),
        1,
        "teardown did not record the Package"
    );

    // Idempotent: a second pass over the same Shard finds nothing left to do.
    shard.assert_call("teardown_package", &[&arg(PACKAGE)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn teardown_refuses_a_package_this_build_does_not_compile() {
    let shard = stage("package-teardown-unknown");
    let output = shard.call("teardown_package", &[&arg("not-installed")]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "an unknown Package was torn down: {text}"
    );
    assert!(text.contains("not a Package this build compiles"), "{text}");
    assert_eq!(
        count(&shard, "SELECT package_name FROM game_package_teardown"),
        0
    );
}
