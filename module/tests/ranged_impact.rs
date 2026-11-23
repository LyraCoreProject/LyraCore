mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_shared::item::weapon_subclass::{THROWN, WAND};
use lyracore_shared::spell::taxonomy::CAST_FLAG_RANGED_AUTO_REPEAT;
use support::{event_rows, fence_subscription, subscribe_with_fence, Row, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const WOLF_LEVEL: u32 = 63;
const AUTO_SHOT: u32 = 75;
const SHOOT: u32 = 5019;
const RANGED_SLOT: u8 = 17;
const ARCANE: u32 = 0x40;
const ARCANE_SCHOOL: u8 = 6;
const PHYSICAL: u32 = 0x01;
// vmangos SpellDamageClass: Shoot is magic, Auto Shot is ranged.
const DAMAGE_CLASS_MAGIC: u8 = 1;
const DAMAGE_CLASS_RANGED: u8 = 3;
const DAMAGE_LOG_EVENTS: &str =
    "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND is_proc_log = true";
// The attack table's Normal outcome: past every band of a level 1 shooter against a level 63 wolf.
const NORMAL_TABLE_ROLL: u32 = 9_999;
// Below the wolf's partial block chance, and above it.
const BLOCKING_ROLL: u32 = 0;
const NOT_BLOCKING_ROLL: u32 = 9_999;
// First Target stream draw of state 6: an integer resist roll of 0, which resists three quarters.
const RESIST_SEED_STATE: u32 = 6;
const SHOT_TICKS: &str = "4";
// 20 ticks of 50 ms pass the longest flight in these tests.
const FLIGHT_TICKS: &str = "20";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_landed_auto_shot_is_partly_blocked_by_a_facing_creature_and_reports_it() {
    let mut standalone = Standalone::start("ranged-impact-auto-shot-block");
    let wolf = stage(&mut standalone);
    insert_shot_spell(&standalone, AUTO_SHOT, PHYSICAL, DAMAGE_CLASS_RANGED);
    equip_ranged(&standalone, THROWN, 0);
    aim(&standalone, wolf, AUTO_SHOT);
    let events = subscribe_with_fence(&standalone, &[DAMAGE_LOG_EVENTS]);

    shoot(&standalone, &[NORMAL_TABLE_ROLL, 0, BLOCKING_ROLL]);
    shoot(&standalone, &[NORMAL_TABLE_ROLL, 0, NOT_BLOCKING_ROLL]);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 2, "each landed shot logs its damage: {logs:?}");
    let (blocked, unblocked) = (&logs[0], &logs[1]);
    assert_eq!(unblocked["blocked_amount"], "0");
    assert!(number(blocked, "blocked_amount") > 0, "{blocked:?}");
    assert_eq!(
        number(blocked, "damage") + number(blocked, "blocked_amount"),
        number(unblocked, "damage"),
        "the same shot loses exactly the blocked amount"
    );
    assert_eq!(
        health(&standalone, wolf),
        WOLF_HEALTH - number(blocked, "damage") - number(unblocked, "damage")
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_wand_shot_is_never_partly_blocked() {
    let mut standalone = Standalone::start("ranged-impact-wand-no-block");
    let wolf = stage(&mut standalone);
    insert_shot_spell(&standalone, SHOOT, ARCANE, DAMAGE_CLASS_MAGIC);
    equip_ranged(&standalone, WAND, 0);
    aim(&standalone, wolf, SHOOT);
    let events = subscribe_with_fence(&standalone, &[DAMAGE_LOG_EVENTS]);

    shoot(&standalone, &[NORMAL_TABLE_ROLL, 0, BLOCKING_ROLL]);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert!(number(&logs[0], "damage") > 0);
    assert_eq!(logs[0]["blocked_amount"], "0");
    assert_eq!(
        standalone
            .query_rows(
                "SELECT * FROM game_conformance_scripted_swing_draw WHERE character_guid = 1"
            )
            .len(),
        1,
        "the wand draws its table and weapon rolls and leaves the block draw alone"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_partly_resisted_wand_shot_reports_the_resisted_amount() {
    let mut standalone = Standalone::start("ranged-impact-wand-resist");
    let wolf = stage(&mut standalone);
    insert_shot_spell(&standalone, SHOOT, ARCANE, DAMAGE_CLASS_MAGIC);
    equip_ranged(&standalone, WAND, ARCANE_SCHOOL);
    aim(&standalone, wolf, SHOOT);
    standalone.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &PLAYER),
            ("target_guid", &wolf),
            ("state", &RESIST_SEED_STATE),
        ],
    );
    let events = subscribe_with_fence(&standalone, &[DAMAGE_LOG_EVENTS]);

    shoot(&standalone, &[NORMAL_TABLE_ROLL, 0]);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 1, "{logs:?}");
    let (damage, resisted) = (number(&logs[0], "damage"), number(&logs[0], "resisted"));
    assert!(
        resisted > damage,
        "a roll of 0 resists three quarters of the shot: {logs:?}"
    );
    assert_eq!(logs[0]["blocked_amount"], "0");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH - damage);
}

fn number(row: &Row, column: &str) -> u32 {
    row[column].parse().unwrap()
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
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = {WOLF_LEVEL}, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
    ));
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &PLAYER), ("state", &1)],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn insert_shot_spell(standalone: &Standalone, spell_id: u32, school_mask: u32, damage_class: u8) {
    standalone.assert_sql(&format!(
        "DELETE FROM game_spell WHERE spell_id = {spell_id}"
    ));
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Ranged impact fixture {spell_id}")),
            ("school_mask", &school_mask),
            ("damage_class", &damage_class),
            ("cast_flags", &CAST_FLAG_RANGED_AUTO_REPEAT),
        ],
    );
}

fn equip_ranged(standalone: &Standalone, subclass: u8, damage_school: u8) {
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET subclass = {subclass}, damage_school = {damage_school}, \
         damage_min = 40.0, damage_max = 40.0, delay_ms = 100000 WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {RANGED_SLOT} \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_BLADE}"
    ));
}

// The shooter stands 15 yards east of the wolf, and both face each other.
fn aim(standalone: &Standalone, wolf: u64, spell_id: u32) {
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET ranged_spell_id = {spell_id} WHERE attacker_guid = {PLAYER}"
    ));
    let wolf_row = standalone.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {wolf}"
    ));
    let wolf_x: f32 = wolf_row[0]["x"].parse().unwrap();
    let wolf_y: f32 = wolf_row[0]["y"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0 WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {wolf_y}, orientation = {} WHERE guid = {PLAYER}",
        wolf_x + 15.0,
        std::f32::consts::PI
    ));
}

// Fires one shot from scripted swing draws (table, weapon, partial block) and lets it land.
fn shoot(standalone: &Standalone, draws: &[u32]) {
    let draws = format!(
        "[{}]",
        draws
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    standalone.assert_call("debug_script_swing_draws", &[&PLAYER.to_string(), &draws]);
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &[SHOT_TICKS]);
    standalone.assert_call("debug_advance_roll_batch", &[FLIGHT_TICKS]);
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
