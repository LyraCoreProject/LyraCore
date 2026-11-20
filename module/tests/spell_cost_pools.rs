mod support;

use lyracore_package_delta::fixture_ids::{EATING_SPELL, FIXTURE_JERKY, TEST_WOLF_ENTRY};
use lyracore_shared::guid;
use lyracore_shared::packing::power_type::{HEALTH, MANA, RAGE};
use support::{actor, number, refused, Row, Standalone};

const WOLF: u64 = guid::creature(TEST_WOLF_ENTRY, 1);
const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const FIREBALL: &str = "133";
const COST: u32 = 10;
const MAGE_CLASS: u32 = 8;
const WARRIOR_CLASS: u32 = 1;

fn fixture(name: &str, spell_power_type: u8) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET class = {MAGE_CLASS} WHERE guid = 1"
    ));
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_learn_spell", &["1", FIREBALL]);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = 0, gcd_ms = 0, cooldown_ms = 0, \
         power_type = {spell_power_type}, cost = {COST} WHERE spell_id = {FIREBALL}"
    ));
    shard.assert_sql("UPDATE game_world_entity SET simulation_indexed = false WHERE guid = 1");
    shard
}

fn entity(shard: &Standalone) -> Row {
    shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .remove(0)
}

fn cast_fireball(shard: &Standalone) {
    shard.assert_call("gw_cast_spell", &[ACTOR, FIREBALL, &WOLF.to_string()]);
}

fn become_warrior(shard: &Standalone) {
    let bytes: u32 = number(&entity(shard), "unit_bytes_0");
    let bytes = (bytes & 0x00FF_00FF) | (WARRIOR_CLASS << 8) | (u32::from(RAGE) << 24);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {bytes}, power = 1000, max_power = 1000 \
         WHERE guid = 1"
    ));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mana_cost_is_refused_for_a_caster_without_a_mana_pool() {
    let shard = fixture("cost-pool-missing", MANA);
    become_warrior(&shard);
    refused(
        &shard,
        "gw_cast_spell",
        &[ACTOR, FIREBALL, &WOLF.to_string()],
        "not enough power: have 0, need 10",
    );
    assert_eq!(
        number::<u32>(&entity(&shard), "power"),
        1000,
        "rage is untouched"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_health_cost_is_paid_from_health() {
    let shard = fixture("cost-pool-health", HEALTH);
    shard.assert_call("debug_set_health", &["1", "50"]);
    shard.assert_call("debug_set_power", &["1", "50"]);
    let mana = number::<u32>(&entity(&shard), "power");
    assert_eq!(mana, 50, "the mage holds mana that could pay the cost");
    cast_fireball(&shard);
    let after = entity(&shard);
    assert_eq!(number::<u32>(&after, "health"), 50 - COST);
    assert_eq!(number::<u32>(&after, "power"), mana, "mana is untouched");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_health_cost_is_refused_at_or_below_the_health_it_takes() {
    let shard = fixture("cost-pool-health-short", HEALTH);
    shard.assert_call("debug_set_power", &["1", "50"]);
    assert!(
        number::<u32>(&entity(&shard), "power") >= COST,
        "mana could pay the cost"
    );
    shard.assert_call("debug_set_health", &["1", &COST.to_string()]);
    refused(
        &shard,
        "gw_cast_spell",
        &[ACTOR, FIREBALL, &WOLF.to_string()],
        "not enough power",
    );
    assert_eq!(number::<u32>(&entity(&shard), "health"), COST);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_spell_casts_without_its_cost_for_a_caster_without_that_pool() {
    let mut shard = Standalone::start("cost-pool-item");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("UPDATE game_world_entity SET simulation_indexed = false WHERE guid = 1");
    become_warrior(&shard);
    shard.assert_call("debug_set_power", &["1", "50"]);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET power_type = {MANA}, cost = {COST} WHERE spell_id = {EATING_SPELL}"
    ));
    shard.assert_call("debug_grant_item", &["1", &FIXTURE_JERKY.to_string(), "2"]);
    let slot = shard
        .query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = {FIXTURE_JERKY}"
        ))
        .remove(0)["slot"]
        .clone();
    shard.assert_call("gw_use_item", &[&actor("1"), &slot]);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = 1 AND entry = {FIXTURE_JERKY}"
        ))[0]["stack_count"],
        "1"
    );
    assert_eq!(
        number::<u32>(&entity(&shard), "power"),
        50,
        "rage is untouched"
    );
}
