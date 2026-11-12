mod support;

use support::{wolves_beside, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_kill_purges_loot_and_eligibility_left_on_the_living_creature() {
    let mut shard = Standalone::start("corpse-loot-residue");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let wolf = wolves_beside(&shard, "1", &[5.0]).remove(0);
    shard.insert_row(
        "game_corpse_loot",
        &[
            ("id", &900001),
            ("corpse_guid", &wolf),
            ("item_entry", &2589),
            ("count", &1),
        ],
    );
    shard.insert_row(
        "game_corpse_loot_eligible",
        &[
            ("id", &900002),
            ("corpse_guid", &wolf),
            ("eligible_guid", &99),
        ],
    );

    shard.assert_call("debug_kill_creature", &["1", &wolf]);

    assert!(shard
        .query_rows("SELECT id FROM game_corpse_loot WHERE id = 900001")
        .is_empty());
    assert!(shard
        .query_rows("SELECT id FROM game_corpse_loot_eligible WHERE id = 900002")
        .is_empty());
}
