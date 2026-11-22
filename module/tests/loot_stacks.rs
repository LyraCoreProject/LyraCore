mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, TEST_WOLF_ENTRY};
use serde_json::json;
use support::{creatures_beside, wolves_beside, Field, Standalone};

const SEED_FOR_FIRST_STACK: u64 = 1;
const SEED_FOR_SECOND_STACK: u64 = 0;
const SEED_FOR_THIRD_STACK: u64 = 2;
const SEEDS_FOR_ONE_TWO_THREE: [u64; 3] = [
    SEED_FOR_FIRST_STACK,
    SEED_FOR_SECOND_STACK,
    SEED_FOR_THIRD_STACK,
];
const OPERATOR_BEAST_ENTRY: u32 = 52_000;
const CHEST_LOOT_ID: u32 = 52_100;
const CHEST_ENTRIES: [u32; 3] = [52_101, 52_102, 52_103];
const PACKAGE_LOOT_ROW: u64 = 9_000_001;
const IMPORTED_CHEST_ROW: u64 = 52_200;
const SOURCE_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard
}

fn stage(name: &str, rows: &[&[Field]]) -> Standalone {
    let shard = realm(name);
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_loot WHERE creature_entry = {TEST_WOLF_ENTRY}"
    ));
    shard.insert_rows("game_creature_loot", rows);
    shard
}

// The source's stream draws the drop chance first and the stack size second.
fn seed_source_stream(shard: &Standalone, source_guid: u64, seed: u64) {
    shard.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &(900_000 + seed)),
            ("target_guid", &source_guid),
            ("state", &seed),
        ],
    );
}

fn kill_with_seed(shard: &Standalone, wolf: &str, seed: u64) -> Vec<(u32, u32)> {
    shard.assert_call("debug_apply_damage", &[wolf, "1", "1"]);
    seed_source_stream(shard, wolf.parse().unwrap(), seed);
    shard.assert_call("debug_kill_creature", &["1", wolf]);
    drops_of(shard, wolf.parse().unwrap())
}

fn drops_of(shard: &Standalone, source_guid: u64) -> Vec<(u32, u32)> {
    let mut drops: Vec<(u32, u32)> = shard
        .query_rows(&format!(
            "SELECT item_entry, count FROM game_corpse_loot WHERE corpse_guid = {source_guid}"
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
        .zip(SEEDS_FOR_ONE_TWO_THREE)
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_operator_edited_one_to_three_row_drops_stacks_of_one_two_and_three() {
    let shard = realm("loot-stack-operator");
    shard.insert_row(
        "game_creature_template",
        &[
            ("entry", &OPERATOR_BEAST_ENTRY),
            ("name", &"Stack Beast"),
            ("display_id", &720),
            ("level", &1),
            ("health", &60),
            ("faction_template", &14),
            ("creature_type", &1),
            ("scale", &1.0),
            ("base_attack_time_ms", &2000),
        ],
    );
    let edits = json!([{"setCreatureLoot": {"creature_entry": OPERATOR_BEAST_ENTRY, "rows": [
        {"item_entry": FIXTURE_JERKY, "chance_bp": 10_000, "min_count": 1, "count": 3,
         "group_id": 0, "quest_only": false}
    ]}}]);
    shard.assert_call("operator_apply_creature_edits", &[&edits.to_string()]);
    let beasts = creatures_beside(&shard, "1", OPERATOR_BEAST_ENTRY, &[3.0, 4.0, 5.0]);

    let stacks: Vec<_> = beasts
        .iter()
        .zip(SEEDS_FOR_ONE_TWO_THREE)
        .map(|(beast, seed)| kill_with_seed(&shard, beast, seed))
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

fn chests_at_the_character(shard: &Standalone) -> Vec<u64> {
    shard.assert_sql("UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = 1");
    CHEST_ENTRIES
        .iter()
        .map(|entry| {
            shard.assert_call(
                "debug_spawn_gameobject",
                &[
                    &entry.to_string(),
                    "3",
                    "0",
                    "0",
                    "0",
                    "100",
                    "100",
                    "100",
                    &CHEST_LOOT_ID.to_string(),
                    "0",
                    "0",
                    "0",
                ],
            );
            shard.query_rows(&format!(
                "SELECT guid FROM game_gameobject WHERE template_entry = {entry}"
            ))[0]["guid"]
                .parse()
                .unwrap()
        })
        .collect()
}

fn chest_loot_delta(id: u64, operation: &str, fields: &str) -> String {
    format!(
        r#"{{"version":1,"package":"example.chest-stacks","source_hash":"{SOURCE_HASH}","claims":[{{"table":"game_gameobject_loot","key":{{"id":{id}}},"operation":"{operation}","fields":{fields}}}]}}"#
    )
}

fn chest_loot_insert(stack_fields: &str) -> String {
    let fields = format!(
        r#"{{"loot_id":{{"type":"u32","value":{CHEST_LOOT_ID}}},"item_entry":{{"type":"u32","value":{FIXTURE_JERKY}}},"chance_bp":{{"type":"u32","value":10000}},"group_id":{{"type":"u32","value":0}},"quest_only":{{"type":"bool","value":false}},{stack_fields}}}"#
    );
    chest_loot_delta(PACKAGE_LOOT_ROW, "insert", &fields)
}

fn apply_loot_delta(shard: &Standalone, delta: &str) -> std::process::Output {
    let arg = |value: &str| serde_json::to_string(value).unwrap();
    shard.call("apply_package_deltas", &[&arg("loot"), &arg(delta)])
}

fn apply_chest_loot_delta(shard: &Standalone, stack_fields: &str) {
    let output = apply_loot_delta(shard, &chest_loot_insert(stack_fields));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn open_with_seed(shard: &Standalone, entry: u32, chest_guid: u64, seed: u64) -> Vec<(u32, u32)> {
    seed_source_stream(shard, chest_guid, seed);
    shard.assert_call("debug_use_gameobject_entry", &["1", &entry.to_string()]);
    drops_of(shard, chest_guid)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_one_to_three_chest_row_drops_stacks_of_one_two_and_three() {
    let shard = realm("loot-stack-package");
    let chests = chests_at_the_character(&shard);
    apply_chest_loot_delta(
        &shard,
        r#""min_count":{"type":"u32","value":1},"count":{"type":"u32","value":3}"#,
    );

    let stacks: Vec<_> = CHEST_ENTRIES
        .iter()
        .zip(chests)
        .zip(SEEDS_FOR_ONE_TWO_THREE)
        .map(|((entry, chest), seed)| open_with_seed(&shard, *entry, chest, seed))
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
fn a_package_chest_row_without_min_count_applies_and_keeps_its_stack() {
    let shard = realm("loot-stack-package-fixed");
    let chests = chests_at_the_character(&shard);
    apply_chest_loot_delta(&shard, r#""count":{"type":"u32","value":3}"#);

    assert_eq!(
        open_with_seed(&shard, CHEST_ENTRIES[0], chests[0], SEED_FOR_FIRST_STACK),
        [(FIXTURE_JERKY, 3)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_loot_row_whose_final_minimum_exceeds_its_count_is_refused() {
    let shard = realm("loot-stack-package-refused");
    chests_at_the_character(&shard);
    shard.insert_row(
        "game_gameobject_loot",
        &[
            ("id", &IMPORTED_CHEST_ROW),
            ("loot_id", &CHEST_LOOT_ID),
            ("item_entry", &FIXTURE_JERKY),
            ("chance_bp", &10_000),
            ("count", &2),
        ],
    );
    let inverted_insert = chest_loot_insert(
        r#""min_count":{"type":"u32","value":4},"count":{"type":"u32","value":3}"#,
    );
    let minimum_above_stored_count = chest_loot_delta(
        IMPORTED_CHEST_ROW,
        "update",
        r#"{"min_count":{"type":"u32","value":3}}"#,
    );

    for (delta, expected) in [
        (inverted_insert, "needs min_count 4 <= count 3"),
        (minimum_above_stored_count, "needs min_count 3 <= count 2"),
    ] {
        let output = apply_loot_delta(&shard, &delta);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.status.success(), "{text}");
        assert!(text.contains(expected), "{text}");
    }
    assert!(shard
        .query_rows(&format!(
            "SELECT id FROM game_gameobject_loot WHERE id = {PACKAGE_LOOT_ROW}"
        ))
        .is_empty());
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT min_count FROM game_gameobject_loot WHERE id = {IMPORTED_CHEST_ROW}"
        ))[0]["min_count"],
        "0"
    );
}
