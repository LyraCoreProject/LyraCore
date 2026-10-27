mod support;

use support::{poll_until, Standalone, POLL_TIMEOUT};

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn empty_relay_catalogue_and_spell_guardian_cleanup_apply_through_real_reducers() {
    let mut standalone = Standalone::start("eventai-runtime-boundaries");
    standalone.publish_module_anonymous();
    standalone.assert_call_anonymous("debug_verify_eventai_spell_guardian_cleanup", &[]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn obsolete_callbacks_leave_replacement_summons_and_living_summons_expire_naturally() {
    let mut standalone = Standalone::start("eventai-summon-expiry-boundaries");
    standalone.publish_module();
    standalone.assert_call("debug_verify_eventai_summon_expiry", &[]);

    let natural = standalone
        .query_rows(
            "SELECT creature_guid FROM game_creature_ai_summon_expiry WHERE lifetime_ms = 5000",
        )
        .remove(0)["creature_guid"]
        .clone();
    assert_eq!(
        standalone
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE guid = {natural}"
            ))
            .len(),
        1,
        "the living summon must be observable before its allowed lifetime ends"
    );
    assert!(
        poll_until(POLL_TIMEOUT, || standalone
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE guid = {natural}"
            ))
            .is_empty()),
        "the disengaged summon did not expire after its allowed lifetime"
    );
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_creature_ai_summon_expiry WHERE creature_guid = {natural}"
        ))
        .is_empty());
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_creature_ai_summon_origin WHERE creature_guid = {natural}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn delayed_scripts_continue_after_failure_and_abort_without_canceling_child_scripts() {
    let mut standalone = Standalone::start("eventai-script-failure");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_stage_script_failure_policies", &[]);
    let first = lyracore_package_delta::packed_creature_spawn_guid(51_000, 1);
    let second = lyracore_package_delta::packed_creature_spawn_guid(51_000, 2);
    let stand_state = |guid| {
        standalone
            .query_rows(&format!(
                "SELECT unit_bytes_1 FROM game_world_entity WHERE guid = {guid}"
            ))
            .first()
            .and_then(|row| row["unit_bytes_1"].parse::<u64>().ok())
            .map(|value| value & 0xff)
    };
    assert!(
        poll_until(POLL_TIMEOUT, || stand_state(first) == Some(3)),
        "a continued failure lost the delayed command"
    );
    assert!(poll_until(POLL_TIMEOUT, || standalone
        .query_rows("SELECT id FROM game_creature_ai_relay_run WHERE relay_id = 90102")
        .is_empty()));
    assert_ne!(
        stand_state(second),
        Some(4),
        "an aborted script ran its later command"
    );
    assert!(
        poll_until(POLL_TIMEOUT, || stand_state(second) == Some(8)),
        "aborting the parent canceled its delayed child"
    );
    assert!(standalone
        .query_rows("SELECT id FROM game_creature_ai_relay_run WHERE relay_id = 90102")
        .is_empty());
    assert!(standalone
        .query_rows("SELECT id FROM game_creature_ai_relay_run WHERE relay_id = 90103")
        .is_empty());
}
