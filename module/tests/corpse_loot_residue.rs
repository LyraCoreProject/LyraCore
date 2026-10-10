//! A kill starts the corpse's loot from nothing: rows a live creature's guid already carries, such
//! as pickpocket loot, never collide with the kill roll.

mod support;

use support::{wolves_beside, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_kill_purges_loot_and_eligibility_left_on_the_living_creature() {
    let mut shard = Standalone::start("corpse-loot-residue");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let wolf = wolves_beside(&shard, "1", &[5.0]).remove(0);
    shard.assert_sql(&format!(
        "INSERT INTO game_corpse_loot (id, corpse_guid, slot, item_entry, count, quest_only, \
         reserved_for, designated_looter_guid, master_only, withheld, random_property_id) \
         VALUES (900001, {wolf}, 0, 2589, 1, false, 0, 0, false, false, 0)"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_corpse_loot_eligible (id, corpse_guid, eligible_guid) \
         VALUES (900002, {wolf}, 99)"
    ));

    shard.assert_call("debug_kill_creature", &["1", &wolf]);

    assert!(shard
        .query_rows("SELECT id FROM game_corpse_loot WHERE id = 900001")
        .is_empty());
    assert!(shard
        .query_rows("SELECT id FROM game_corpse_loot_eligible WHERE id = 900002")
        .is_empty());
}
