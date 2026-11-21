mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, TEST_WOLF_ENTRY};
use support::{wolves_beside, Field, Standalone};

const SEED_FOR_FIRST_STACK: u64 = 1;
const SEED_FOR_SECOND_STACK: u64 = 0;
const SEED_FOR_THIRD_STACK: u64 = 2;

fn stage(name: &str, rows: &[&[Field]]) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_loot WHERE creature_entry = {TEST_WOLF_ENTRY}"
    ));
    shard.insert_rows("game_creature_loot", rows);
    shard
}

// The wolf's stream draws the drop chance first and the stack size second.
fn kill_with_seed(shard: &Standalone, wolf: &str, seed: u64) -> Vec<(u32, u32)> {
    shard.assert_call("debug_apply_damage", &[wolf, "1", "1"]);
    let wolf_guid: u64 = wolf.parse().unwrap();
    shard.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &(900_000 + seed)),
            ("target_guid", &wolf_guid),
            ("state", &seed),
        ],
    );
    shard.assert_call("debug_kill_creature", &["1", wolf]);
    let mut drops: Vec<(u32, u32)> = shard
        .query_rows(&format!(
            "SELECT item_entry, count FROM game_corpse_loot WHERE corpse_guid = {wolf}"
        ))
        .iter()
        .map(|row| {
            (
                row["item_entry"].parse().unwrap(),
                row["count"].parse().unwrap(),
            )
        })
        .collect();
    drops.sort_unstable();
    drops
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_one_to_three_row_drops_stacks_of_one_two_and_three() {
    let shard = stage(
        "loot-stack-range",
        &[&[
            ("id", &900_101),
            ("creature_entry", &TEST_WOLF_ENTRY),
            ("item_entry", &FIXTURE_JERKY),
            ("chance_bp", &10_000),
            ("count", &3),
            ("group_id", &0),
            ("min_count", &1),
        ]],
    );
    let wolves = wolves_beside(&shard, "1", &[3.0, 4.0, 5.0]);

    let stacks: Vec<_> = wolves
        .iter()
        .zip([
            SEED_FOR_FIRST_STACK,
            SEED_FOR_SECOND_STACK,
            SEED_FOR_THIRD_STACK,
        ])
        .map(|(wolf, seed)| kill_with_seed(&shard, wolf, seed))
        .collect();

    assert_eq!(
        stacks,
        [
            [(FIXTURE_JERKY, 1)],
            [(FIXTURE_JERKY, 2)],
            [(FIXTURE_JERKY, 3)]
        ]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fixed_row_and_a_row_without_a_minimum_keep_their_stack() {
    let shard = stage(
        "loot-stack-fixed",
        &[
            &[
                ("id", &900_101),
                ("creature_entry", &TEST_WOLF_ENTRY),
                ("item_entry", &FIXTURE_JERKY),
                ("chance_bp", &10_000),
                ("count", &2),
                ("group_id", &0),
                ("min_count", &2),
            ],
            &[
                ("id", &900_102),
                ("creature_entry", &TEST_WOLF_ENTRY),
                ("item_entry", &FIXTURE_BLADE),
                ("chance_bp", &10_000),
                ("count", &3),
                ("group_id", &0),
            ],
        ],
    );
    let wolf = wolves_beside(&shard, "1", &[3.0]).remove(0);

    let mut expected = [(FIXTURE_JERKY, 2), (FIXTURE_BLADE, 3)];
    expected.sort_unstable();
    assert_eq!(
        kill_with_seed(&shard, &wolf, SEED_FOR_FIRST_STACK),
        expected
    );
}
