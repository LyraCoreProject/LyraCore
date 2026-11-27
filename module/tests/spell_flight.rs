mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_MOD_SPEED, CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, CAST_FLAG_IGNORE_RESISTANCES,
    E_DAMAGE, P_SPEED_KIND, SPEED_MOVE, T_TARGET_ENEMY,
};
use support::Standalone;

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const SLOW_BOLT: u32 = 5_136_301;
const FROST: u8 = 0x10;
const DAMAGE_CLASS_MAGIC: u8 = 1;
const BOLT_POINTS: i32 = 59;
const SLOW_POINTS: i32 = -50;
const DURATION_MS: u32 = 10_000;
const BOLT_SPEED_YPS: f32 = 28.0;
const APART_YD: f32 = 15.0;
// 20 ticks of 50 ms pass the 535 ms flight over 15 yards at 28 yd/s.
const FLIGHT_TICKS: &str = "20";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_travelling_spells_slow_lands_with_its_damage_not_at_launch() {
    let mut standalone = Standalone::start("spell-flight-slow-at-impact");
    let wolf = stage(&mut standalone);

    cast(&standalone, wolf);
    assert_eq!(
        slow_stacks(&standalone, wolf),
        None,
        "vmangos applies a travelling spell's Aura when it reaches its target"
    );
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
    assert_eq!(
        standalone
            .query_rows(&format!(
                "SELECT scheduled_id FROM game_pending_spell_impact WHERE target_guid = {wolf}"
            ))
            .len(),
        1,
        "one impact carries the damage and the slow"
    );

    standalone.assert_call("debug_advance_roll_batch", &[FLIGHT_TICKS]);
    assert!(slow_stacks(&standalone, wolf).is_some());
    assert!(health(&standalone, wolf) < WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_target_that_dies_while_the_spell_travels_gets_no_aura() {
    let mut standalone = Standalone::start("spell-flight-dead-target");
    let wolf = stage(&mut standalone);

    cast(&standalone, wolf);
    standalone.assert_call(
        "debug_apply_damage",
        &[
            &wolf.to_string(),
            &WOLF_HEALTH.to_string(),
            &PLAYER.to_string(),
        ],
    );
    assert_eq!(
        standalone.query_rows(&format!(
            "SELECT dead FROM game_world_entity WHERE guid = {wolf}"
        ))[0]["dead"],
        "true"
    );

    standalone.assert_call("debug_advance_roll_batch", &[FLIGHT_TICKS]);
    assert_eq!(
        slow_stacks(&standalone, wolf),
        None,
        "vmangos Unit::AddSpellAuraHolder refuses an Aura on a dead unit"
    );
    assert!(standalone
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_spell_impact WHERE target_guid = {wolf}"
        ))
        .is_empty());
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
    let wolf_row = standalone.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {wolf}"
    ));
    let wolf_x: f32 = wolf_row[0]["x"].parse().unwrap();
    let wolf_y: f32 = wolf_row[0]["y"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {wolf_y}, orientation = {} WHERE guid = {PLAYER}",
        wolf_x + APART_YD,
        std::f32::consts::PI
    ));
    stage_slow_bolt(standalone);
    wolf
}

fn stage_slow_bolt(standalone: &Standalone) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &SLOW_BOLT),
            ("name", &"Slow bolt fixture"),
            ("school_mask", &FROST),
            ("damage_class", &DAMAGE_CLASS_MAGIC),
            ("range_yd", &30),
            ("speed_yps", &BOLT_SPEED_YPS),
            ("duration_ms", &DURATION_MS),
            ("is_negative", &true),
            (
                "cast_flags",
                &(CAST_FLAG_ALWAYS_HIT | CAST_FLAG_CANNOT_CRIT | CAST_FLAG_IGNORE_RESISTANCES),
            ),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(SLOW_BOLT, 0)),
            ("spell_id", &SLOW_BOLT),
            ("effect_index", &0u8),
            ("kind", &E_DAMAGE),
            ("base_points", &BOLT_POINTS),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(SLOW_BOLT, 1)),
            ("spell_id", &SLOW_BOLT),
            ("effect_index", &1u8),
            ("kind", &A_MOD_SPEED),
            ("base_points", &SLOW_POINTS),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &i32::from(SPEED_MOVE)),
            ("p_0_kind", &P_SPEED_KIND),
        ],
    );
}

fn cast(standalone: &Standalone, wolf: u64) {
    standalone.assert_call(
        "debug_cast_at",
        &[
            &PLAYER.to_string(),
            &SLOW_BOLT.to_string(),
            &wolf.to_string(),
        ],
    );
}

fn slow_stacks(standalone: &Standalone, wolf: u64) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT stacks FROM game_aura WHERE target_guid = {wolf} AND spell_id = {SLOW_BOLT}"
        ))
        .first()
        .map(|row| row["stacks"].parse().unwrap())
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
