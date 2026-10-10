//! Crowd-control diminishing returns advance only when the control aura lands on the target.

mod support;

use lyracore_shared::constants::tracer_spell::{AURA_SLOTS, BUFF_SLOT_COUNT};
use support::{wolves_beside, Standalone};

/// The `Test Poly` fixture: 10 s of `A_CONTROL` with the polymorph mechanic.
const TEST_POLY: &str = "50023";

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_control_aura_without_a_free_slot_leaves_diminishing_returns_unchanged() {
    let mut shard = Standalone::start("diminishing-returns-refusal");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    // Two casters, so the second cast does not wait out the first caster's global cooldown.
    let casters = wolves_beside(&shard, "1", &[3.0, 5.0]);
    let debuff_slots = (AURA_SLOTS - BUFF_SLOT_COUNT).to_string();
    shard.assert_call(
        "debug_fill_aura_slots",
        &["1", &casters[0], "true", &debuff_slots],
    );

    shard.assert_call("debug_force_cast_at", &[&casters[0], TEST_POLY, "1"]);
    assert!(polymorphs(&shard).is_empty());
    assert!(dr_levels(&shard).is_empty());

    shard.assert_sql("DELETE FROM game_aura WHERE target_guid = 1");
    shard.assert_call("debug_force_cast_at", &[&casters[1], TEST_POLY, "1"]);
    assert_eq!(polymorphs(&shard).len(), 1);
    assert_eq!(dr_levels(&shard), ["1"]);
}

fn polymorphs(shard: &Standalone) -> Vec<support::Row> {
    shard.query_rows(&format!(
        "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = {TEST_POLY}"
    ))
}

fn dr_levels(shard: &Standalone) -> Vec<String> {
    shard
        .query_rows("SELECT level FROM game_dr_state WHERE target_guid = 1")
        .into_iter()
        .map(|row| row["level"].clone())
        .collect()
}
