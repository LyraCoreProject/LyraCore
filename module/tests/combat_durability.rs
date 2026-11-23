mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use support::{
    event_rows, fence_subscription, number, poll_until, subscribe_with_fence, Standalone,
    TableSubscription, POLL_TIMEOUT,
};

type SqlRow = std::collections::BTreeMap<String, String>;

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const BLADE_DURABILITY: u32 = 50;
const HEAD_DURABILITY: u32 = 20;
const HELM_ENTRY: u32 = 9_000_100;
const HELM_STAMINA: u32 = 20;
const HELM_GUID: u64 = 9_000_001;
const HEAD_INVENTORY_TYPE: u8 = 1;
const ARMOUR_CLASS: u8 = 4;
const BACKPACK_SLOT: u8 = 23;
const DUEL: u64 = 509_0001;
/// `state::ACTIVE` in `module/src/duel.rs`.
const DUEL_ACTIVE: u8 = 2;
const MAIN_HAND_SLOT: u8 = 15;
const HEAD_SLOT: u8 = 0;
const OUT_OF_REACH_LEVEL: u32 = 60;
const MISSED_SWING_HIT_INFO: &str = "2";
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
    give_helm(&standalone, HEAD_SLOT, HEAD_DURABILITY);
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
    give_helm(&standalone, HEAD_SLOT, HEAD_DURABILITY);
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
    assert_eq!(swings[0]["hit_info"], MISSED_SWING_HIT_INFO);
    assert_eq!(durability(&standalone, MAIN_HAND_SLOT), BLADE_DURABILITY);
    assert_eq!(
        wear_stream_state(&standalone),
        SEED_PICKING_MAIN_HAND,
        "an avoided swing draws no wear roll"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn wear_that_breaks_a_helm_removes_its_stamina() {
    let mut standalone = Standalone::start("combat-durability-broken-helm");
    let wolf = stage(&mut standalone);
    let unequipped_health = max_health(&standalone);
    give_helm(&standalone, BACKPACK_SLOT, 1);
    standalone.assert_call("debug_equip_item", &["1", &BACKPACK_SLOT.to_string()]);
    assert!(
        max_health(&standalone) > unequipped_health,
        "the equipped helm adds stamina"
    );
    standalone.insert_row(
        "game_conformance_wear_random",
        &[("character_guid", &PLAYER), ("state", &SEED_PICKING_HEAD)],
    );

    hit(&standalone, wolf, PLAYER);

    assert_eq!(durability(&standalone, HEAD_SLOT), 0);
    assert_eq!(max_health(&standalone), unequipped_health);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_killing_blow_wears_nothing() {
    let mut standalone = Standalone::start("combat-durability-killing-blow");
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
        "UPDATE game_world_entity SET level = 1, health = 1, max_health = 1 WHERE guid = {wolf}"
    ));
    let events = subscribe_with_fence(&standalone, &["SELECT * FROM game_combat_event_live"]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &["4"]);
    fence_subscription(&standalone, &events);

    let swings = player_swings(&events);
    assert_eq!(swings.len(), 1);
    assert_eq!(swings[0]["killing_blow"], "true");
    assert_eq!(durability(&standalone, MAIN_HAND_SLOT), BLADE_DURABILITY);
    assert_eq!(
        wear_stream_state(&standalone),
        SEED_PICKING_MAIN_HAND,
        "a killing blow draws no wear roll"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_ending_hit_wears_like_any_other_damaging_hit() {
    let mut standalone = Standalone::start("combat-durability-duel-ending-hit");
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_event_reaper_schedule");
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    standalone.assert_call(
        "debug_create_package_character",
        &[r#""duel-test""#, r#""Rival""#, "1", "1"],
    );
    let rival: u64 = number(
        &standalone
            .query_rows("SELECT guid FROM game_character WHERE name = 'Rival'")
            .remove(0),
        "guid",
    );
    standalone.assert_call("debug_spawn_player_entity", &[&rival.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET max_durability = {BLADE_DURABILITY} \
         WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    let player = standalone
        .query_rows(&format!(
            "SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    let (x, y, z): (f32, f32, f32) = (
        number(&player, "x"),
        number(&player, "y"),
        number(&player, "z"),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, x = {}, y = {y}, z = {z}, health = 1 WHERE guid = {rival}",
        x + 1.0
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0, godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.insert_row(
        "game_duel",
        &[
            ("id", &DUEL),
            ("flag_entry", &21680),
            ("initiator_guid", &PLAYER),
            ("challenged_guid", &rival),
            ("map_id", &player["map_id"]),
            ("instance_id", &player["instance_id"]),
            ("flag_x", &x),
            ("flag_y", &y),
            ("flag_z", &z),
            ("state", &DUEL_ACTIVE),
        ],
    );
    standalone.insert_row(
        "game_conformance_wear_random",
        &[
            ("character_guid", &PLAYER),
            ("state", &SEED_PICKING_MAIN_HAND),
        ],
    );
    let events = subscribe_with_fence(
        &standalone,
        &[&format!("SELECT * FROM game_duel WHERE id = {DUEL}")],
    );

    standalone.assert_call("debug_engage", &[&PLAYER.to_string(), &rival.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || {
            standalone.assert_sql(&format!(
                "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {PLAYER}"
            ));
            events
                .deletes("game_duel")
                .iter()
                .any(|row| row["id"] == DUEL)
        }),
        "no swing ended the Duel"
    );

    assert_eq!(
        durability(&standalone, MAIN_HAND_SLOT),
        BLADE_DURABILITY - 1
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
        "game_item_template",
        &[
            ("entry", &HELM_ENTRY),
            ("class", &ARMOUR_CLASS),
            ("name", &"Durability fixture helm"),
            ("quality", &1),
            ("inventory_type", &HEAD_INVENTORY_TYPE),
            ("max_durability", &HEAD_DURABILITY),
            ("stat_stamina", &HELM_STAMINA),
        ],
    );
    wolf
}

fn give_helm(standalone: &Standalone, slot: u8, durability: u32) {
    standalone.insert_row(
        "game_item_instance",
        &[
            ("guid", &HELM_GUID),
            ("entry", &HELM_ENTRY),
            ("owner_guid", &PLAYER),
            ("slot", &slot),
            ("stack_count", &1),
            ("durability", &durability),
        ],
    );
}

fn max_health(standalone: &Standalone) -> u32 {
    standalone.query_rows(&format!(
        "SELECT max_health FROM game_world_entity WHERE guid = {PLAYER}"
    ))[0]["max_health"]
        .parse()
        .unwrap()
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
