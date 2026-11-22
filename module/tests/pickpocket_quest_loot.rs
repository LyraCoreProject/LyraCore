mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_JERKY, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{E_PICKPOCKET, T_TARGET_ANY};
use support::Standalone;

const PICKPOCKET: u32 = 9_164_100;

fn rogue_beside_wolf(name: &str) -> (Standalone, u64) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1"],
    );
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &PICKPOCKET),
            ("name", &"Pickpocket fixture"),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PICKPOCKET, 0)),
            ("spell_id", &PICKPOCKET),
            ("kind", &E_PICKPOCKET),
            ("target", &T_TARGET_ANY),
        ],
    );
    shard.insert_row(
        "game_pickpocket_loot",
        &[
            ("id", &9_164_101),
            ("creature_entry", &TEST_WOLF_ENTRY),
            ("item_entry", &FIXTURE_JERKY),
            ("chance_bp", &10_000),
            ("count", &1),
            ("group_id", &0),
            ("quest_only", &true),
        ],
    );
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} AND dead = false"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    (shard, wolf)
}

fn pickpocket(shard: &Standalone, wolf: u64) {
    shard.assert_call(
        "debug_force_cast_at",
        &["1", &PICKPOCKET.to_string(), &wolf.to_string()],
    );
}

fn lootable(shard: &Standalone, wolf: u64) -> bool {
    let flags: u32 = shard.query_rows(&format!(
        "SELECT dynamic_flags FROM game_world_entity WHERE guid = {wolf}"
    ))[0]["dynamic_flags"]
        .parse()
        .unwrap();
    flags & 1 != 0
}

fn loot_rows(shard: &Standalone, wolf: u64) -> usize {
    shard
        .query_rows(&format!(
            "SELECT id FROM game_corpse_loot WHERE corpse_guid = {wolf}"
        ))
        .len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pickpocket_drops_a_quest_item_its_rogue_does_not_need() {
    let (shard, wolf) = rogue_beside_wolf("pickpocket-quest-item-unneeded");

    pickpocket(&shard, wolf);

    assert_eq!(
        shard.query_rows(&format!(
            "SELECT pickpocketed FROM game_world_entity WHERE guid = {wolf}"
        ))[0]["pickpocketed"],
        "true"
    );
    assert_eq!(loot_rows(&shard, wolf), 0);
    assert!(!lootable(&shard, wolf));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pickpocket_keeps_a_quest_item_its_rogue_needs() {
    let (shard, wolf) = rogue_beside_wolf("pickpocket-quest-item-needed");
    shard.insert_row(
        "game_quest_template",
        &[
            ("entry", &9_164_102),
            ("quest_level", &1),
            ("title", &"Pocket jerky"),
        ],
    );
    shard.insert_row(
        "game_quest_objective",
        &[
            ("id", &9_164_103),
            ("quest_entry", &9_164_102),
            ("kind", &1),
            ("target_entry", &FIXTURE_JERKY),
            ("required_count", &1),
        ],
    );
    shard.assert_call("debug_grant_quest", &["1", "9164102"]);

    pickpocket(&shard, wolf);

    assert_eq!(loot_rows(&shard, wolf), 1);
    assert!(lootable(&shard, wolf));
}
