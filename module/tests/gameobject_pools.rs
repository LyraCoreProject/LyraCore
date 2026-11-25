mod support;

use lyracore_package_delta::fixture_ids::FIXTURE_JERKY;
use support::{number, refused, Row, Standalone};

const CHARACTER: u64 = 1;
const VIRTUAL_START_MICROS: u64 = 1_800_000_000_000_000;
const MINING: u32 = 186;

const CHEST_A: u32 = 5_288_001;
const CHEST_B: u32 = 5_288_002;
const NODE: u32 = 5_288_003;
const SPREAD_NODE: u32 = 5_288_004;

const MOTHER_POOL: u32 = 5_288_100;
const LEFT_POOL: u32 = 5_288_101;
const RIGHT_POOL: u32 = 5_288_102;
const NODE_POOL: u32 = 5_288_103;
const SPREAD_POOL: u32 = 5_288_110;
const MIXED_POOL: u32 = 5_288_120;
const MIXED_CHILD_POOL: u32 = 5_288_121;

const LEFT_POINT: u64 = 5_288_201;
const RIGHT_POINT: u64 = 5_288_202;
const NODE_POINT: u64 = 5_288_203;
const SPREAD_POINTS: u64 = 5_288_300;
const MIXED_POINTS: u64 = 5_288_400;

struct Place {
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
}

fn realm(name: &str) -> (Standalone, Place) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call(
        "debug_start_virtual_clock",
        &[&VIRTUAL_START_MICROS.to_string()],
    );
    let at = shard.query_rows(&format!(
        "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
    ))[0]
        .clone();
    let place = Place {
        map_id: number(&at, "map_id"),
        x: number(&at, "x"),
        y: number(&at, "y"),
        z: number(&at, "z"),
    };
    (shard, place)
}

fn chest_template(shard: &Standalone, entry: u32) {
    shard.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &entry),
            ("type_id", &3),
            ("name", &"Pooled chest"),
            ("respawn_secs", &600),
        ],
    );
}

fn node_template(shard: &Standalone, entry: u32) {
    shard.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &entry),
            ("type_id", &25),
            ("name", &"Pooled vein"),
            ("data0", &FIXTURE_JERKY),
            ("data1", &1),
            ("gather_skill_line", &MINING),
            ("respawn_secs", &300),
        ],
    );
}

fn pool(shard: &Standalone, pool_id: u32, max_active: u32) {
    shard.insert_row(
        "game_gameobject_pool",
        &[("pool_id", &pool_id), ("max_active", &max_active)],
    );
}

fn link(shard: &Standalone, pool_id: u32, parent_pool_id: u32, map_id: u32) {
    shard.insert_row(
        "game_gameobject_pool_link",
        &[
            ("pool_id", &pool_id),
            ("parent_pool_id", &parent_pool_id),
            ("weight", &1),
            ("map_id", &map_id),
        ],
    );
}

fn member(
    shard: &Standalone,
    point_id: u64,
    pool_id: u32,
    entry: u32,
    at: &Place,
    respawn_secs: u32,
) {
    shard.insert_row(
        "game_gameobject_pool_member",
        &[
            ("point_id", &point_id),
            ("pool_id", &pool_id),
            ("template_entry", &entry),
            ("map_id", &at.map_id),
            ("x", &at.x),
            ("y", &at.y),
            ("z", &at.z),
            ("weight", &1),
            ("respawn_min_secs", &respawn_secs),
            ("respawn_max_secs", &respawn_secs),
        ],
    );
}

fn objects(shard: &Standalone, entry: u32) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT guid, x, instance_id, despawned, respawn_at_micros FROM game_gameobject \
         WHERE template_entry = {entry}"
    ))
}

fn shown(object: &Row) -> bool {
    object["despawned"] == "false"
}

fn shows_at(object: &Row) -> u64 {
    number(object, "respawn_at_micros")
}

fn use_entry(shard: &Standalone, entry: u32) {
    shard.assert_call(
        "debug_use_gameobject_entry",
        &[&CHARACTER.to_string(), &entry.to_string()],
    );
}

/// Lets the respawn pass reach every waiting object of `entry`.
fn make_due(shard: &Standalone, entry: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_gameobject SET respawn_at_micros = 1 WHERE template_entry = {entry}"
    ));
    shard.assert_call("debug_advance_roll_batch", &["1000"]);
}

/// A mother pool showing one of two child pools, each holding one chest at the Character.
fn nested_chests(shard: &Standalone, at: &Place) {
    chest_template(shard, CHEST_A);
    chest_template(shard, CHEST_B);
    pool(shard, MOTHER_POOL, 1);
    pool(shard, LEFT_POOL, 1);
    pool(shard, RIGHT_POOL, 1);
    link(shard, MOTHER_POOL, 0, at.map_id);
    link(shard, LEFT_POOL, MOTHER_POOL, at.map_id);
    link(shard, RIGHT_POOL, MOTHER_POOL, at.map_id);
    member(shard, LEFT_POINT, LEFT_POOL, CHEST_A, at, 120);
    member(shard, RIGHT_POINT, RIGHT_POOL, CHEST_B, at, 240);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_nested_pool_keeps_only_its_limit_of_child_pools_up() {
    let (shard, at) = realm("gameobject-pool-limit");
    node_template(&shard, SPREAD_NODE);
    pool(&shard, SPREAD_POOL, 2);
    link(&shard, SPREAD_POOL, 0, at.map_id);
    for child in 1..=4u32 {
        let child_pool = SPREAD_POOL + child;
        pool(&shard, child_pool, 1);
        link(&shard, child_pool, SPREAD_POOL, at.map_id);
        let place = Place {
            x: at.x + child as f32,
            ..at
        };
        for slot in 0..2u64 {
            member(
                &shard,
                SPREAD_POINTS + u64::from(child) * 10 + slot,
                child_pool,
                SPREAD_NODE,
                &place,
                0,
            );
        }
    }

    for _ in 0..2 {
        shard.assert_call("arm_all_pools", &[]);

        let up = objects(&shard, SPREAD_NODE);
        assert_eq!(up.len(), 2, "two of the four child pools: {up:?}");
        assert!(up.iter().all(shown));
        assert_ne!(up[0]["x"], up[1]["x"], "each from another child pool");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_looted_pooled_chest_moves_its_pool_to_another_place_shown_after_its_delay() {
    let (shard, at) = realm("gameobject-pool-nested-loot");
    nested_chests(&shard, &at);
    shard.assert_call("arm_all_pools", &[]);
    let (used, other, other_delay_secs) = if objects(&shard, CHEST_A).is_empty() {
        (CHEST_B, CHEST_A, 120)
    } else {
        (CHEST_A, CHEST_B, 240)
    };
    assert_eq!(objects(&shard, used).len(), 1);
    assert!(objects(&shard, other).is_empty(), "one child pool is up");

    use_entry(&shard, used);

    assert!(objects(&shard, used).is_empty(), "the looted chest leaves");
    let replacement = objects(&shard, other);
    assert_eq!(replacement.len(), 1, "the other child pool comes up");
    assert!(!shown(&replacement[0]), "hidden until its delay passes");
    assert_eq!(
        shows_at(&replacement[0]),
        VIRTUAL_START_MICROS + other_delay_secs * 1_000_000
    );

    make_due(&shard, other);

    assert!(shown(&objects(&shard, other)[0]));
    assert!(objects(&shard, used).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_gathered_pooled_node_comes_back_only_after_its_respawn_delay() {
    let (shard, at) = realm("gameobject-pool-gather");
    node_template(&shard, NODE);
    pool(&shard, NODE_POOL, 1);
    member(&shard, NODE_POINT, NODE_POOL, NODE, &at, 90);
    shard.assert_call("arm_all_pools", &[]);
    assert!(shown(&objects(&shard, NODE)[0]));
    shard.assert_call(
        "debug_set_skill",
        &[&CHARACTER.to_string(), &MINING.to_string(), "50"],
    );

    use_entry(&shard, NODE);

    let waiting = objects(&shard, NODE);
    assert_eq!(waiting.len(), 1);
    assert!(!shown(&waiting[0]), "no node shows at once");
    assert_eq!(shows_at(&waiting[0]), VIRTUAL_START_MICROS + 90_000_000);

    make_due(&shard, NODE);

    let back = objects(&shard, NODE);
    assert!(shown(&back[0]));
    assert_eq!(shows_at(&back[0]), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_new_instance_rolls_its_own_pool_objects() {
    let (shard, at) = realm("gameobject-pool-instance");
    nested_chests(&shard, &at);

    shard.assert_call("debug_create_fixture_instance", &[&CHARACTER.to_string()]);

    let run: u64 = number(
        &shard.query_rows(&format!(
            "SELECT instance_id FROM game_world_entity WHERE guid = {CHARACTER}"
        ))[0],
        "instance_id",
    );
    assert_ne!(run, 0);
    let mut up = objects(&shard, CHEST_A);
    up.extend(objects(&shard, CHEST_B));
    assert_eq!(up.len(), 1, "one child pool is up in the Instance: {up:?}");
    assert_eq!(number::<u64>(&up[0], "instance_id"), run);
    assert!(shown(&up[0]));
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT copy_id FROM game_gameobject_pool_copy WHERE instance_id = {run}"
            ))
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_operator_update_moves_a_pooled_member_and_a_delete_is_refused() {
    let (shard, at) = realm("gameobject-pool-operator");
    node_template(&shard, NODE);
    pool(&shard, NODE_POOL, 1);
    member(&shard, NODE_POINT, NODE_POOL, NODE, &at, 90);
    shard.assert_call("arm_all_pools", &[]);
    let guid = objects(&shard, NODE)[0]["guid"].clone();
    let moved_x = at.x + 3.0;
    let fields = serde_json::json!({
        "template_entry": NODE,
        "map_id": at.map_id,
        "x": moved_x,
        "y": at.y,
        "z": at.z,
        "orientation": 0.0,
        "state": 0,
        "rotation_0": 0.0,
        "rotation_1": 0.0,
        "rotation_2": 0.0,
        "rotation_3": 1.0,
    });

    shard.assert_call("operator_update_gameobject", &[&guid, &fields.to_string()]);

    let member_x: f32 = number(
        &shard.query_rows(&format!(
            "SELECT x FROM game_gameobject_pool_member WHERE point_id = {NODE_POINT}"
        ))[0],
        "x",
    );
    assert_eq!(member_x, moved_x, "the next roll stands at the new place");
    let object = &objects(&shard, NODE)[0];
    assert_eq!(object["guid"], guid);
    assert_eq!(number::<f32>(object, "x"), moved_x);
    assert!(shown(object));

    let mut other_map = fields.clone();
    other_map["map_id"] = serde_json::json!(at.map_id + 1);
    refused(
        &shard,
        "operator_update_gameobject",
        &[&guid, &other_map.to_string()],
        "the import owns the pool's map",
    );
    refused(
        &shard,
        "operator_delete_gameobject",
        &[&guid],
        "the import owns the pool",
    );
    assert_eq!(objects(&shard, NODE).len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pools_child_pools_and_own_members_share_its_limit() {
    let (shard, at) = realm("gameobject-pool-mixed");
    node_template(&shard, SPREAD_NODE);
    pool(&shard, MIXED_POOL, 1);
    link(&shard, MIXED_POOL, 0, at.map_id);
    pool(&shard, MIXED_CHILD_POOL, 1);
    link(&shard, MIXED_CHILD_POOL, MIXED_POOL, at.map_id);
    let child_place = Place {
        x: at.x + 1.0,
        ..at
    };
    member(
        &shard,
        MIXED_POINTS,
        MIXED_CHILD_POOL,
        SPREAD_NODE,
        &child_place,
        0,
    );
    let own_place = Place {
        x: at.x + 2.0,
        ..at
    };
    member(
        &shard,
        MIXED_POINTS + 1,
        MIXED_POOL,
        SPREAD_NODE,
        &own_place,
        0,
    );

    shard.assert_call("arm_all_pools", &[]);

    assert_eq!(
        objects(&shard, SPREAD_NODE).len(),
        1,
        "one object for a limit of 1"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_import_rolls_listed_top_level_pools_again_without_doubling_them() {
    let (shard, at) = realm("gameobject-pool-arm-batch");
    nested_chests(&shard, &at);
    let up = || objects(&shard, CHEST_A).len() + objects(&shard, CHEST_B).len();

    for _ in 0..2 {
        shard.assert_call("arm_pools", &[&format!("\"{MOTHER_POOL}\"")]);
        assert_eq!(up(), 1);
    }
    refused(
        &shard,
        "arm_pools",
        &[&format!("\"{LEFT_POOL}\"")],
        "is not a top-level pool",
    );
    assert_eq!(up(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_realm_without_a_pool_reimport_keeps_its_seeded_pool_and_static_chests_working() {
    let (shard, at) = realm("gameobject-pool-legacy");
    let tier: Vec<Row> = [1731u32, 1732]
        .into_iter()
        .flat_map(|entry| objects(&shard, entry))
        .collect();
    assert_eq!(tier.len(), 1, "the seeded tier pool shows one vein");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = -9620.11, y = -46.3336, z = 47.3641, level = 60 \
         WHERE guid = {CHARACTER}"
    ));
    shard.assert_call(
        "debug_set_skill",
        &[&CHARACTER.to_string(), &MINING.to_string(), "100"],
    );
    for (ore, name) in [(2770u32, "Copper Ore"), (2771, "Tin Ore")] {
        shard.insert_row(
            "game_item_template",
            &[("entry", &ore), ("name", &name), ("max_stack", &20)],
        );
    }
    let vein = shard.query_rows(&format!(
        "SELECT template_entry FROM game_gameobject WHERE guid = {}",
        tier[0]["guid"]
    ))[0]["template_entry"]
        .clone();

    shard.assert_call(
        "debug_use_gameobject_entry",
        &[&CHARACTER.to_string(), &vein],
    );

    let tier: Vec<Row> = [1731u32, 1732]
        .into_iter()
        .flat_map(|entry| objects(&shard, entry))
        .collect();
    assert_eq!(tier.len(), 1);
    assert!(!shown(&tier[0]));
    assert_eq!(shows_at(&tier[0]), VIRTUAL_START_MICROS + 300_000_000);

    chest_template(&shard, CHEST_A);
    shard.assert_call(
        "debug_spawn_gameobject",
        &[
            &CHEST_A.to_string(),
            "3",
            "0",
            "0",
            &at.map_id.to_string(),
            "-9620.11",
            "-46.3336",
            "47.3641",
            "0",
            "0",
            "600",
            "0",
        ],
    );
    let chest = objects(&shard, CHEST_A)[0]["guid"].clone();
    use_entry(&shard, CHEST_A);
    let looted = &objects(&shard, CHEST_A)[0];
    assert_eq!(looted["guid"], chest, "a static chest despawns in place");
    assert!(!shown(looted));
    make_due(&shard, CHEST_A);
    assert!(shown(&objects(&shard, CHEST_A)[0]));
}
