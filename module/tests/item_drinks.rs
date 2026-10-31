mod support;

use support::{actor, Standalone};

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn drink_spells_restore_mana_and_refuse_rage_even_without_the_item_flag() {
    let mut shard = Standalone::start("drink-spell-effects");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("UPDATE game_item_template SET restores_power = false WHERE entry = 5090052");
    shard.assert_sql("UPDATE game_spell_effect SET kind = 3, base_points = 10, die_sides = 0, p0 = 0, p0_kind = 4 WHERE spell_id = 50115");
    shard.assert_call("debug_grant_item", &["1", "5090052", "2"]);
    let item = &shard
        .query_rows("SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = 5090052")
        [0];
    let result = shard.call("gw_use_item", &[&actor("1"), &item["slot"]]);
    assert!(!result.status.success());
    assert_eq!(
        shard.query_rows(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = 1 AND entry = 5090052"
        )[0]["stack_count"],
        "2"
    );
    shard.assert_sql("UPDATE game_character SET class = 5 WHERE guid = 1");
    shard.assert_sql(
        "UPDATE game_world_entity SET unit_bytes_0 = 1281, power = 0, max_power = 100 WHERE guid = 1",
    );
    shard.assert_call("gw_use_item", &[&actor("1"), &item["slot"]]);
    assert_eq!(
        shard.query_rows(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = 1 AND entry = 5090052"
        )[0]["stack_count"],
        "1"
    );
    let power = shard.query_rows("SELECT power FROM game_world_entity WHERE guid = 1")[0]["power"]
        .parse::<u32>()
        .unwrap();
    assert!(power >= 10, "drink restored only {power} mana");
}
