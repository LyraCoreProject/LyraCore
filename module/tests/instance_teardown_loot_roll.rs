mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_JERKY, TEST_WOLF_ENTRY};
use lyracore_shared::loot_roll::vote_kind;
use support::{number, Standalone};

const WINNER: u64 = 1;
const ABSENT_VOTER: u64 = 99;
const WON_ROLL: u64 = 900_201;
const PASSED_ROLL: u64 = 900_202;
const JERKY_PER_SLOT: u32 = 2;

fn jerky_held(shard: &Standalone) -> u32 {
    shard
        .query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = {WINNER} \
             AND entry = {FIXTURE_JERKY}"
        ))
        .iter()
        .map(|row| number::<u32>(row, "stack_count"))
        .sum()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn instance_teardown_settles_open_loot_rolls_before_it_removes_their_items() {
    let mut shard = Standalone::start("instance-teardown-loot-roll");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&WINNER.to_string()]);
    shard.assert_call("debug_create_fixture_instance", &[&WINNER.to_string()]);
    let run = shard.query_rows(&format!(
        "SELECT instance_id FROM game_world_entity WHERE guid = {WINNER}"
    ))[0]["instance_id"]
        .clone();
    assert_ne!(run, "0");
    let wolves_in_run = || {
        shard.query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} \
             AND instance_id = {run}"
        ))
    };
    let copies = wolves_in_run();
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&WINNER.to_string(), &TEST_WOLF_ENTRY.to_string(), "3"],
    );
    let boss = wolves_in_run()
        .into_iter()
        .map(|row| row["guid"].clone())
        .find(|guid| copies.iter().all(|copy| copy["guid"] != *guid))
        .expect("the spawned creature is in the run");
    shard.assert_call("debug_enter_instance", &[&WINNER.to_string(), "0"]);

    for (row, slot, roll) in [(900_101, 0, WON_ROLL), (900_102, 1, PASSED_ROLL)] {
        shard.insert_row(
            "game_corpse_loot",
            &[
                ("id", &row),
                ("corpse_guid", &boss),
                ("slot", &slot),
                ("item_entry", &FIXTURE_JERKY),
                ("count", &JERKY_PER_SLOT),
                ("withheld", &true),
            ],
        );
        shard.insert_row(
            "game_loot_roll",
            &[
                ("id", &roll),
                ("corpse_guid", &boss),
                ("slot", &slot),
                ("item_entry", &FIXTURE_JERKY),
                ("deadline_micros", &i64::MAX),
                ("promotion_source", &format!("0x{}", "00".repeat(32))),
            ],
        );
    }
    shard.insert_rows(
        "game_loot_roll_vote",
        &[
            &[
                ("id", &900_301),
                ("roll_id", &WON_ROLL),
                ("voter_guid", &WINNER),
                ("voted", &true),
                ("vote", &vote_kind::NEED),
                ("rolled", &40),
            ],
            &[
                ("id", &900_302),
                ("roll_id", &WON_ROLL),
                ("voter_guid", &ABSENT_VOTER),
            ],
            &[
                ("id", &900_303),
                ("roll_id", &PASSED_ROLL),
                ("voter_guid", &WINNER),
            ],
            &[
                ("id", &900_304),
                ("roll_id", &PASSED_ROLL),
                ("voter_guid", &ABSENT_VOTER),
            ],
        ],
    );
    let held_before = jerky_held(&shard);

    shard.assert_call("debug_reap_instance", &[&run]);

    assert_eq!(
        jerky_held(&shard),
        held_before + JERKY_PER_SLOT,
        "the only Need vote wins the first item"
    );
    for table in ["game_corpse_loot", "game_loot_roll"] {
        let left = shard.query_rows(&format!(
            "SELECT id FROM {table} WHERE corpse_guid = {boss}"
        ));
        assert!(left.is_empty(), "{table} kept {left:?}");
    }
    for roll in [WON_ROLL, PASSED_ROLL] {
        assert!(shard
            .query_rows(&format!(
                "SELECT id FROM game_loot_roll_vote WHERE roll_id = {roll}"
            ))
            .is_empty());
    }
}
