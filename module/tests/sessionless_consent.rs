//! Consent admission and row lifetime through real Module operations on a private Standalone.

mod support;

use support::Standalone;

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard
}

fn assert_refusal(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let output = shard.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "{reducer} unexpectedly committed: {text}"
    );
    assert!(text.contains(tag), "{reducer}: expected {tag}, got {text}");
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn admission_reads_current_consent_and_session_ownership() {
    let shard = stage("sessionless-admission");
    assert_refusal(
        &shard,
        "debug_set_sessionless_action_consent",
        &["999", "false"],
        "no character 999",
    );
    assert!(shard
        .query_rows("SELECT * FROM game_sessionless_action_consent WHERE character_guid = 999")
        .is_empty());
    shard.assert_call("admit_sessionless_group_action", &["1"]);
    shard.assert_call("debug_set_sessionless_action_consent", &["1", "false"]);
    assert_refusal(
        &shard,
        "admit_sessionless_group_action",
        &["1"],
        "group:action_suppressed",
    );
    shard.assert_call("debug_set_sessionless_action_consent", &["1", "true"]);
    shard.assert_call("admit_sessionless_group_action", &["1"]);
    shard.assert_sql("UPDATE game_character SET online = true WHERE guid = 1");
    assert_refusal(
        &shard,
        "admit_sessionless_group_action",
        &["1"],
        "group:actor_unavailable",
    );
    shard.assert_sql("UPDATE game_character SET online = false WHERE guid = 1");
    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    assert_refusal(
        &shard,
        "admit_sessionless_group_action",
        &["1"],
        "group:actor_unavailable",
    );
    assert_refusal(
        &shard,
        "admit_sessionless_group_action",
        &["999"],
        "group:actor_unavailable",
    );
    shard.assert_call("debug_delete_character", &["1"]);
    assert!(shard
        .query_rows("SELECT * FROM game_sessionless_action_consent WHERE character_guid = 1")
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn each_selection_clears_only_its_characters_unclaimed_group_intents() {
    let shard = stage("sessionless-intents");
    // Disable scheduled event GC so row lifetime cannot hide a failed cleanup or claim.
    shard.assert_sql("DELETE FROM game_event_reaper_schedule");
    shard.assert_call("debug_emit_sessionless_group_intent", &["1", "2", "false"]);
    shard.assert_call("debug_emit_sessionless_group_intent", &["1", "0", "true"]);
    shard.assert_call("debug_emit_sessionless_group_intent", &["2", "1", "false"]);
    let original = shard.query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 1");
    assert_eq!(original.len(), 2);
    shard.assert_call("debug_set_sessionless_action_consent", &["1", "false"]);
    assert!(shard
        .query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 1")
        .is_empty());
    assert_eq!(
        shard
            .query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 2")
            .len(),
        1
    );
    assert_refusal(
        &shard,
        "claim_bot_invite_intent",
        &[&original[0]["id"]],
        "group:intent_already_claimed",
    );

    shard.assert_call("debug_emit_sessionless_group_intent", &["1", "2", "false"]);
    let pending = shard.query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 1");
    let id = &pending[0]["id"];
    assert_refusal(
        &shard,
        "claim_bot_invite_intent",
        &[id],
        "group:action_suppressed",
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 1"),
        pending
    );
    shard.assert_call("debug_set_sessionless_action_consent", &["1", "false"]);
    assert!(shard
        .query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 1")
        .is_empty());

    shard.assert_call("debug_set_sessionless_action_consent", &["1", "true"]);
    shard.assert_call("debug_emit_sessionless_group_intent", &["1", "2", "false"]);
    let admitted = shard.query_rows("SELECT * FROM game_bot_invite_intent WHERE inviter_guid = 1");
    shard.assert_call("claim_bot_invite_intent", &[&admitted[0]["id"]]);
    assert_refusal(
        &shard,
        "claim_bot_invite_intent",
        &[&admitted[0]["id"]],
        "group:intent_already_claimed",
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn unsharded_actor_acceptance_obeys_consent_without_restricting_sessionful_characters() {
    let shard = stage("sessionless-local-answer");
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call(
        "create_character",
        &["1", "\"Inviter\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let inviter = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Inviter'")[0]
        ["guid"]
        .clone();
    let actor = format!(r#"{{"guid":{inviter},"ownership":null}}"#);
    shard.assert_call("debug_set_sessionless_action_consent", &["1", "false"]);
    shard.assert_call("realm_group_op", &["0", &actor, "1", "0", "0"]);
    let invitation = shard.query_rows("SELECT * FROM game_group_invite WHERE target_guid = 1");
    assert_eq!(invitation.len(), 1);
    assert_refusal(
        &shard,
        "gw_accept_group_invite",
        &[r#"{"guid":1,"ownership":null}"#],
        "group:action_suppressed",
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_group_invite WHERE target_guid = 1"),
        invitation
    );
    assert!(shard
        .query_rows("SELECT * FROM game_group_member WHERE character_guid = 1")
        .is_empty());

    shard.assert_sql("UPDATE game_character SET online = true WHERE guid = 1");
    shard.assert_call(
        "gw_accept_group_invite",
        &[r#"{"guid":1,"ownership":null}"#],
    );
    assert!(shard
        .query_rows("SELECT * FROM game_group_invite WHERE target_guid = 1")
        .is_empty());
    assert_eq!(
        shard
            .query_rows("SELECT * FROM game_group_member WHERE character_guid = 1")
            .len(),
        1
    );
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn suppressed_consent_travels_with_the_character_through_export_and_import() {
    let source = stage("sessionless-transfer-source");
    source.assert_call("install_guid_range", &["0"]);
    let mut destination = Standalone::start("sessionless-transfer-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    source.assert_call("debug_set_sessionless_action_consent", &["1", "false"]);
    let actor = r#"{"guid":1,"ownership":null}"#;
    source.assert_call(
        "begin_transfer",
        &["1", actor, "0", "0", "1200", "1200", "50", "0", "true"],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 1");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    destination.assert_call("import_character_blob", &["1", &blob, actor]);
    destination.assert_call("release_transfer", &["1", actor]);
    destination.assert_call("debug_spawn_player_entity", &["1"]);
    let rows = destination
        .query_rows("SELECT * FROM game_sessionless_action_consent WHERE character_guid = 1");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["allowed"], "false");
    assert_refusal(
        &destination,
        "admit_sessionless_group_action",
        &["1"],
        "group:action_suppressed",
    );
    source.assert_call("confirm_import", &["1", actor]);
    source.assert_call("finish_transfer", &["1", actor]);
    assert!(source
        .query_rows("SELECT * FROM game_sessionless_action_consent WHERE character_guid = 1")
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn active_account_ownership_refuses_admission_before_character_online_changes() {
    let shard = stage("sessionless-account-ownership");
    shard.assert_call("claim_account", &["1", "1", "501"]);
    let character = shard.query_rows("SELECT online FROM game_character WHERE guid = 1");
    assert_eq!(character[0]["online"], "false");

    assert_refusal(
        &shard,
        "admit_sessionless_group_action",
        &["1"],
        "group:actor_unavailable",
    );
}
