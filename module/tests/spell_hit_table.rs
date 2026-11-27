mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, E_DAMAGE, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Row, Standalone};

const PLAYER: u64 = 1;
const PLAYER_LEVEL: u32 = 10;
const WOLF_LEVEL: u32 = 60;
const WOLF_HEALTH: u32 = 100_000;
const DAMAGE_SPELL: u32 = 5_134_801;
const PHYSICAL: u32 = 0x01;
const FIRE: u32 = 0x04;
// vmangos SpellDamageClass.
const DAMAGE_CLASS_MAGIC: u8 = 1;
const DAMAGE_CLASS_MELEE: u8 = 2;
const DAMAGE_CLASS_RANGED: u8 = 3;
const CAST_MISS_NONE: &str = "0";
const CAST_MISS_DODGE: &str = "3";
const CAST_MISS_PARRY: &str = "4";
const GO_EVENTS: &str =
    "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND kind = 2";
// The hit roll is the cast's first draw. Splitmix state 29 draws 16 basis points and state 44
// draws 9987. A level 60 wolf gives a level 10 caster with any weapon skill up to 300 a dodge band
// of 500 to 3500 basis points and, capped at 50 skill, a parry band of over 15000 after it.
const DODGED_STATE: u64 = 29;
const PARRIED_STATE: u64 = 44;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_melee_damage_spell_is_dodged_or_parried_and_arms_the_reacts() {
    let mut standalone = Standalone::start("spell-hit-table-melee");
    let wolf = stage(&mut standalone, DAMAGE_CLASS_MELEE, PHYSICAL);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    cast_with_state(&standalone, wolf, DODGED_STATE);
    assert!(
        react_until_ms(&standalone, PLAYER, "overpower_until_ms") > 0,
        "vmangos arms the caster's Overpower when its melee ability is dodged"
    );
    assert!(react_until_ms(&standalone, wolf, "revenge_until_ms") > 0);
    cast_with_state(&standalone, wolf, PARRIED_STATE);
    fence_subscription(&standalone, &events);

    let gos = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(
        miss_codes(&gos),
        [CAST_MISS_DODGE, CAST_MISS_PARRY],
        "Bloodthirst-like spells roll the weapon table: {gos:?}"
    );
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ranged_damage_spell_is_neither_dodged_nor_parried() {
    let mut standalone = Standalone::start("spell-hit-table-ranged");
    let wolf = stage(&mut standalone, DAMAGE_CLASS_RANGED, PHYSICAL);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    cast_with_state(&standalone, wolf, DODGED_STATE);
    cast_with_state(&standalone, wolf, PARRIED_STATE);
    fence_subscription(&standalone, &events);

    let gos = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(
        miss_codes(&gos),
        [CAST_MISS_NONE, CAST_MISS_NONE],
        "vmangos RollMeleeOutcomeAgainst rolls only miss and crit for a ranged attack: {gos:?}"
    );
    assert!(health(&standalone, wolf) < WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_magic_damage_spell_is_neither_dodged_nor_parried() {
    let mut standalone = Standalone::start("spell-hit-table-magic");
    let wolf = stage(&mut standalone, DAMAGE_CLASS_MAGIC, FIRE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    cast_with_state(&standalone, wolf, DODGED_STATE);
    cast_with_state(&standalone, wolf, PARRIED_STATE);
    fence_subscription(&standalone, &events);

    let gos = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(
        miss_codes(&gos),
        [CAST_MISS_NONE, CAST_MISS_NONE],
        "a magic spell rolls only the spell hit table: {gos:?}"
    );
    assert_eq!(react_until_ms(&standalone, wolf, "revenge_until_ms"), 0);
}

fn miss_codes(gos: &[Row]) -> Vec<&str> {
    gos.iter().map(|go| go["miss_info"].as_str()).collect()
}

// The caster stands 15 yards east of the wolf and both face each other; the spell has no range
// check and no travel.
fn stage(standalone: &mut Standalone, damage_class: u8, school_mask: u32) -> u64 {
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
    let wolf_row = standalone.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {wolf}"
    ));
    let wolf_x: f32 = wolf_row[0]["x"].parse().unwrap();
    let wolf_y: f32 = wolf_row[0]["y"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = {WOLF_LEVEL}, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH}, orientation = 0 WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = {PLAYER_LEVEL}, x = {}, y = {wolf_y}, \
         orientation = {} WHERE guid = {PLAYER}",
        wolf_x + 15.0,
        std::f32::consts::PI
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &DAMAGE_SPELL),
            ("name", &"Hit table fixture"),
            ("school_mask", &school_mask),
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
            ("id", &packed_spell_effect_id(DAMAGE_SPELL, 0)),
            ("spell_id", &DAMAGE_SPELL),
            ("kind", &E_DAMAGE),
            ("base_points", &59),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    standalone.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &PLAYER),
            ("target_guid", &wolf),
            ("state", &0u64),
        ],
    );
    wolf
}

fn cast_with_state(standalone: &Standalone, wolf: u64, state: u64) {
    standalone.assert_sql(&format!(
        "UPDATE game_conformance_spell_random SET state = {state} WHERE character_guid = {PLAYER}"
    ));
    standalone.assert_call(
        "debug_cast_at",
        &[
            &PLAYER.to_string(),
            &DAMAGE_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );
}

fn react_until_ms(standalone: &Standalone, guid: u64, column: &str) -> u64 {
    standalone.query_rows(&format!(
        "SELECT {column} FROM game_world_entity WHERE guid = {guid}"
    ))[0][column]
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
