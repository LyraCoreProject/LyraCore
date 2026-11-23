mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const PHYSICAL: u32 = 1;
// AttackOutcome::wire codes in game_combat_event_live.hit_info.
const HIT_INFO_IMMUNE: &str = "8";
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_whose_template_is_immune_to_physical_takes_no_white_swing_damage() {
    let mut standalone = Standalone::start("school-immunity-white-swing");
    let wolf = stage(&mut standalone);
    set_template_school_immunity(&standalone, PHYSICAL);
    let events = subscribe_with_fence(&standalone, &[SWING_EVENTS]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    swing(&standalone);
    fence_subscription(&standalone, &events);
    let swings = player_swings(&events);
    assert_eq!(swings.len(), 1);
    assert_eq!(swings[0]["hit_info"], HIT_INFO_IMMUNE);
    assert_eq!(swings[0]["damage"], "0");
    assert_eq!(
        health(&standalone, wolf),
        WOLF_HEALTH,
        "an immune creature loses no health"
    );

    set_template_school_immunity(&standalone, 0);
    swing(&standalone);
    fence_subscription(&standalone, &events);
    let swings = player_swings(&events);
    assert_eq!(swings.len(), 2);
    assert!(swings[1]["damage"].parse::<u32>().unwrap() > 0);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "without the immunity the same swing lands"
    );
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = 1, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.assert_sql(
        "UPDATE game_world_entity SET health = 10000, max_health = 10000 WHERE guid = 1",
    );
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn set_template_school_immunity(standalone: &Standalone, mask: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_creature_template SET school_immune_mask = {mask} \
         WHERE entry = {TEST_WOLF_ENTRY}"
    ));
}

// A fixed swing roll stream connects against a level 1 wolf, as in the damage shield test.
fn swing(standalone: &Standalone) {
    standalone
        .assert_sql("UPDATE game_conformance_swing_random SET state = 1 WHERE character_guid = 1");
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &["4"]);
}

fn player_swings(
    events: &support::TableSubscription,
) -> Vec<std::collections::BTreeMap<String, String>> {
    event_rows(events, "game_combat_event_live")
        .into_iter()
        .filter(|row| row["attacker_guid"] == PLAYER.to_string())
        .collect()
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
