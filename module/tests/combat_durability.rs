mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use support::{
    event_rows, fence_subscription, subscribe_with_fence, Standalone, TableSubscription,
};

type SqlRow = std::collections::BTreeMap<String, String>;

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const BLADE_DURABILITY: u32 = 50;
const HEAD_DURABILITY: u32 = 20;
const HEAD_ITEM_GUID: u64 = 9_000_001;
const MAIN_HAND_SLOT: u8 = 15;
const HEAD_SLOT: u8 = 0;
const OUT_OF_REACH_LEVEL: u32 = 60;
const AVOIDED_SWING_HIT_INFO: &str = "2";
const HIT: u32 = 5;

// Wear streams for `Wear(1)`: the first draw is under 0.5% and the second picks the slot.
const SEED_PICKING_MAIN_HAND: u64 = 7128;
const SEED_PICKING_HEAD: u64 = 991;
const SEED_FAILING_THE_ROLL: u64 = 1;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_landed_hit_wears_the_equipment_slot_the_roll_picks_and_a_failed_roll_wears_nothing() {
    let mut standalone = Standalone::start("combat-durability-hit-done");
    let wolf = stage(&mut standalone);
    standalone.insert_row(
        "game_conformance_wear_random",
        &[
            ("character_guid", &PLAYER),
            ("state", &SEED_PICKING_MAIN_HAND),
        ],
    );

    hit(&standalone, wolf, PLAYER);
    assert_eq!(
        durability(&standalone, MAIN_HAND_SLOT),
        BLADE_DURABILITY - 1
    );
    assert_eq!(durability(&standalone, HEAD_SLOT), HEAD_DURABILITY);

    seed_wear(&standalone, SEED_PICKING_HEAD);
    hit(&standalone, wolf, PLAYER);
    assert_eq!(
        durability(&standalone, MAIN_HAND_SLOT),
        BLADE_DURABILITY - 1
    );
    assert_eq!(durability(&standalone, HEAD_SLOT), HEAD_DURABILITY - 1);

    seed_wear(&standalone, SEED_FAILING_THE_ROLL);
    hit(&standalone, wolf, PLAYER);
    assert_eq!(
        durability(&standalone, MAIN_HAND_SLOT),
        BLADE_DURABILITY - 1
    );
    assert_eq!(durability(&standalone, HEAD_SLOT), HEAD_DURABILITY - 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hit_taken_wears_the_victims_armour_slot_the_roll_picks() {
    let mut standalone = Standalone::start("combat-durability-hit-taken");
    let wolf = stage(&mut standalone);
    standalone.insert_row(
        "game_conformance_wear_random",
        &[("character_guid", &PLAYER), ("state", &SEED_PICKING_HEAD)],
    );

    hit(&standalone, PLAYER, wolf);

    assert_eq!(durability(&standalone, HEAD_SLOT), HEAD_DURABILITY - 1);
    assert_eq!(durability(&standalone, MAIN_HAND_SLOT), BLADE_DURABILITY);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missed_swing_never_wears_even_when_the_roll_would_pick_the_weapon() {
    let mut standalone = Standalone::start("combat-durability-missed-swing");
    let wolf = stage(&mut standalone);
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &PLAYER), ("state", &1)],
    );
    standalone.insert_row(
        "game_conformance_wear_random",
        &[
            ("character_guid", &PLAYER),
            ("state", &SEED_PICKING_MAIN_HAND),
        ],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let events = subscribe_with_fence(&standalone, &["SELECT * FROM game_combat_event_live"]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = {OUT_OF_REACH_LEVEL} WHERE guid = {wolf}"
    ));
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &["4"]);
    fence_subscription(&standalone, &events);

    let swings = player_swings(&events);
    assert_eq!(swings.len(), 1);
    assert_eq!(swings[0]["hit_info"], AVOIDED_SWING_HIT_INFO);
    assert_eq!(durability(&standalone, MAIN_HAND_SLOT), BLADE_DURABILITY);
    assert_eq!(
        wear_stream_state(&standalone),
        SEED_PICKING_MAIN_HAND,
        "an avoided swing draws no wear roll"
    );
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
    ));
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
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET max_durability = {BLADE_DURABILITY} \
         WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    standalone.insert_row(
        "game_item_instance",
        &[
            ("guid", &HEAD_ITEM_GUID),
            ("entry", &FIXTURE_BLADE),
            ("owner_guid", &PLAYER),
            ("slot", &HEAD_SLOT),
            ("stack_count", &1),
            ("durability", &HEAD_DURABILITY),
        ],
    );
    wolf
}

fn hit(standalone: &Standalone, target: u64, attacker: u64) {
    standalone.assert_call(
        "debug_apply_damage",
        &[&target.to_string(), &HIT.to_string(), &attacker.to_string()],
    );
}

fn seed_wear(standalone: &Standalone, state: u64) {
    standalone.assert_sql(&format!(
        "UPDATE game_conformance_wear_random SET state = {state} WHERE character_guid = {PLAYER}"
    ));
}

fn wear_stream_state(standalone: &Standalone) -> u64 {
    standalone.query_rows(&format!(
        "SELECT state FROM game_conformance_wear_random WHERE character_guid = {PLAYER}"
    ))[0]["state"]
        .parse()
        .unwrap()
}

fn durability(standalone: &Standalone, slot: u8) -> u32 {
    standalone.query_rows(&format!(
        "SELECT durability FROM game_item_instance WHERE owner_guid = {PLAYER} AND slot = {slot}"
    ))[0]["durability"]
        .parse()
        .unwrap()
}

fn player_swings(events: &TableSubscription) -> Vec<SqlRow> {
    event_rows(events, "game_combat_event_live")
        .into_iter()
        .filter(|row| row["attacker_guid"] == PLAYER.to_string())
        .collect()
}
