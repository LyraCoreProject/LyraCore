mod support;

use support::{poll_until, Standalone, POLL_TIMEOUT};

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
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
