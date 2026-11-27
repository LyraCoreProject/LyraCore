mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, E_DAMAGE, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Row, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const WOLF_LEVEL: u32 = 63;
const MELEE_SPELL: u32 = 5_131_711;
const RANGED_SPELL: u32 = 5_131_712;
const BASE_POINTS: i32 = 59;
const PHYSICAL: u32 = 0x01;
// vmangos SpellDamageClass.
const DAMAGE_CLASS_MELEE: u8 = 2;
const DAMAGE_CLASS_RANGED: u8 = 3;
// Spell.dbc Attributes of an instant ability: ability, not while shapeshifted, tracks its target.
const ABILITY_ATTRIBUTES: u32 = 0x0041_0010;
const SPELL_ATTR_NO_ACTIVE_DEFENSE: u32 = 0x0020_0000;
// No range check; a melee ability such as Bloodthirst has a 5 yd range and no speed.
const MELEE_RANGE_YD: u32 = 0;
const MELEE_ABILITY_RANGE_YD: u32 = 5;
const RANGED_RANGE_YD: u32 = 30;
const NO_SPEED: f32 = 0.0;
// Arcane Shot's Spell.dbc Speed.
const RANGED_SPEED_YPS: f32 = 40.0;
const APART_YD: f32 = 15.0;
const MELEE_APART_YD: f32 = 3.0;
// 20 ticks of 50 ms pass the 375 ms flight over 15 yards at 40 yd/s.
const FLIGHT_TICKS: &str = "20";
const GO_EVENTS: &str =
    "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND kind = 2";
const IMPACT_EVENTS: &str = "SELECT * FROM game_spell_impact_event_live WHERE caster_guid = 1";
// The weapon hit roll (which also decides the crit), the effect points and the block draw the one
// seeded state in that order. Splitmix state 12934727 draws 8467, 126 and 3 basis points, so its
// block draw of 3 falls under the wolf's 500 basis point block chance; state 4 draws 8682, 7776 and
// 9487.
const BLOCKING_CASTER_STATE: u64 = 12_934_727;
const CLEAR_CASTER_STATE: u64 = 4;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_melee_damage_spell_is_partly_blocked_by_a_facing_creature_and_reports_it() {
    let mut standalone = Standalone::start("spell-damage-melee-block");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        MELEE_SPELL,
        DAMAGE_CLASS_MELEE,
        (MELEE_RANGE_YD, NO_SPEED),
        ABILITY_ATTRIBUTES,
    );
    face_each_other(&standalone, wolf, APART_YD);
    seed_caster(&standalone, wolf, BLOCKING_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    assert_eq!(revenge_until_ms(&standalone, wolf), 0);
    cast(&standalone, MELEE_SPELL, wolf);
    assert!(
        revenge_until_ms(&standalone, wolf) > 0,
        "vmangos arms Revenge on the target of a blocked melee ability"
    );
    reseed_caster(&standalone, CLEAR_CASTER_STATE);
    cast(&standalone, MELEE_SPELL, wolf);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 2, "each landed spell logs its damage: {logs:?}");
    assert_blocked_once(&standalone, wolf, &logs[0], &logs[1], "blocked_amount");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_travelling_ranged_damage_spell_reports_its_block_when_it_lands() {
    let mut standalone = Standalone::start("spell-damage-ranged-block");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        RANGED_SPELL,
        DAMAGE_CLASS_RANGED,
        (RANGED_RANGE_YD, RANGED_SPEED_YPS),
        ABILITY_ATTRIBUTES,
    );
    face_each_other(&standalone, wolf, APART_YD);
    seed_caster(&standalone, wolf, BLOCKING_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[IMPACT_EVENTS]);

    cast(&standalone, RANGED_SPELL, wolf);
    assert_eq!(
        revenge_until_ms(&standalone, wolf),
        0,
        "the block arms Revenge when the spell lands, not at launch"
    );
    standalone.assert_call("debug_advance_roll_batch", &[FLIGHT_TICKS]);
    assert!(revenge_until_ms(&standalone, wolf) > 0);
    reseed_caster(&standalone, CLEAR_CASTER_STATE);
    cast(&standalone, RANGED_SPELL, wolf);
    standalone.assert_call("debug_advance_roll_batch", &[FLIGHT_TICKS]);
    fence_subscription(&standalone, &events);

    let impacts = event_rows(&events, "game_spell_impact_event_live");
    assert_eq!(impacts.len(), 2, "each spell lands once: {impacts:?}");
    assert_blocked_once(&standalone, wolf, &impacts[0], &impacts[1], "blocked");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_melee_damage_spell_with_no_active_defense_is_never_blocked() {
    let mut standalone = Standalone::start("spell-damage-no-active-defense");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        MELEE_SPELL,
        DAMAGE_CLASS_MELEE,
        (MELEE_RANGE_YD, NO_SPEED),
        ABILITY_ATTRIBUTES | SPELL_ATTR_NO_ACTIVE_DEFENSE,
    );
    face_each_other(&standalone, wolf, APART_YD);
    seed_caster(&standalone, wolf, BLOCKING_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    cast(&standalone, MELEE_SPELL, wolf);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(logs[0]["blocked_amount"], "0");
    assert!(number(&logs[0], "damage") > 0, "{logs:?}");
    assert_eq!(revenge_until_ms(&standalone, wolf), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_melee_damage_spell_with_a_range_and_no_speed_lands_in_its_cast() {
    let mut standalone = Standalone::start("spell-damage-melee-no-speed");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        MELEE_SPELL,
        DAMAGE_CLASS_MELEE,
        (MELEE_ABILITY_RANGE_YD, NO_SPEED),
        ABILITY_ATTRIBUTES,
    );
    face_each_other(&standalone, wolf, MELEE_APART_YD);
    seed_caster(&standalone, wolf, CLEAR_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    cast(&standalone, MELEE_SPELL, wolf);
    let landed = WOLF_HEALTH - health(&standalone, wolf);
    fence_subscription(&standalone, &events);

    assert!(
        landed > 0,
        "vmangos lands a spell without a speed in its cast"
    );
    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(
        number(&logs[0], "damage"),
        landed,
        "the GO carries the damage"
    );
}

fn assert_blocked_once(
    standalone: &Standalone,
    wolf: u64,
    blocked: &Row,
    unblocked: &Row,
    blocked_column: &str,
) {
    assert_eq!(unblocked[blocked_column], "0", "{unblocked:?}");
    assert!(number(blocked, blocked_column) > 0, "{blocked:?}");
    assert!(number(blocked, "damage") > 0, "{blocked:?}");
    assert_eq!(
        number(blocked, "damage") + number(blocked, blocked_column),
        number(unblocked, "damage"),
        "the same spell loses exactly the blocked amount"
    );
    assert_eq!(
        health(standalone, wolf),
        WOLF_HEALTH - number(blocked, "damage") - number(unblocked, "damage")
    );
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
        "game_creature_defense",
        &[
            ("entry", &TEST_WOLF_ENTRY),
            ("no_parry", &true),
            ("no_block", &false),
        ],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

// The caster stands `distance_yd` east of the wolf, and both face each other.
fn face_each_other(standalone: &Standalone, wolf: u64, distance_yd: f32) {
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
        wolf_x + distance_yd,
        std::f32::consts::PI
    ));
}

fn stage_spell(
    standalone: &Standalone,
    spell: u32,
    damage_class: u8,
    (range_yd, speed_yps): (u32, f32),
    attributes: u32,
) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell),
            ("name", &"Damage spell block fixture"),
            ("range_yd", &range_yd),
            ("speed_yps", &speed_yps),
            ("school_mask", &PHYSICAL),
            ("attributes", &attributes),
            ("damage_class", &damage_class),
            (
                "cast_flags",
                &(CAST_FLAG_ALWAYS_HIT | CAST_FLAG_CANNOT_CRIT),
            ),
            ("is_negative", &true),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell, 0)),
            ("spell_id", &spell),
            ("kind", &E_DAMAGE),
            ("base_points", &BASE_POINTS),
            ("target", &T_TARGET_ENEMY),
        ],
    );
}

fn seed_caster(standalone: &Standalone, wolf: u64, state: u64) {
    standalone.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &PLAYER),
            ("target_guid", &wolf),
            ("state", &state),
        ],
    );
}

fn reseed_caster(standalone: &Standalone, state: u64) {
    standalone.assert_sql(&format!(
        "UPDATE game_conformance_spell_random SET state = {state} WHERE character_guid = {PLAYER}"
    ));
}

fn cast(standalone: &Standalone, spell: u32, wolf: u64) {
    standalone.assert_call(
        "debug_cast_at",
        &[&PLAYER.to_string(), &spell.to_string(), &wolf.to_string()],
    );
}

fn revenge_until_ms(standalone: &Standalone, guid: u64) -> u64 {
    standalone.query_rows(&format!(
        "SELECT revenge_until_ms FROM game_world_entity WHERE guid = {guid}"
    ))[0]["revenge_until_ms"]
        .parse()
        .unwrap()
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
