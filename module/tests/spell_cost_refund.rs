mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::packing::power_type::{ENERGY, RAGE};
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, CAST_FLAG_DISCOUNT_POWER_ON_MISS, E_WEAPON_STRIKE,
    T_TARGET_ENEMY,
};
use support::{number, Standalone};

const PLAYER: u64 = 1;
const WOLF_LEVEL: u32 = 63;
const WOLF_HEALTH: u32 = 100_000;
const STRIKE: u32 = 5_136_401;
const PHYSICAL: u32 = 0x01;
const DAMAGE_CLASS_MELEE: u8 = 2;
const ROGUE_CLASS: u32 = 4;
const WARRIOR_CLASS: u32 = 1;
const SINISTER_STRIKE_COST: u32 = 45;
const HAMSTRING_COST: u32 = 100;
// The hit roll is the cast's first draw; splitmix state 29 draws 16 basis points. Against the
// level 63 wolf that falls in the miss band, or for an always-hit strike in the dodge band that
// then opens the table.
const AVOIDED_STATE: u64 = 29;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missed_energy_strike_gives_back_82_percent_of_its_cost() {
    let mut standalone = Standalone::start("spell-cost-refund-energy");
    let wolf = stage(&mut standalone, ROGUE_CLASS, ENERGY, 100);
    stage_strike(&standalone, ENERGY, SINISTER_STRIKE_COST, 0);
    seed_caster(&standalone, wolf);

    strike(&standalone, wolf);

    assert_eq!(
        power(&standalone),
        100 - SINISTER_STRIKE_COST + 37,
        "vmangos gives back lroundf(45 * 0.82) energy on a miss"
    );
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rage_strike_gives_back_82_percent_of_its_cost_when_dodged_and_none_when_missed() {
    let mut standalone = Standalone::start("spell-cost-refund-rage");
    let wolf = stage(&mut standalone, WARRIOR_CLASS, RAGE, 1000);
    stage_strike(&standalone, RAGE, HAMSTRING_COST, CAST_FLAG_ALWAYS_HIT);
    seed_caster(&standalone, wolf);

    strike(&standalone, wolf);
    assert_eq!(
        power(&standalone),
        1000 - HAMSTRING_COST + 82,
        "vmangos gives back lroundf(100 * 0.82) rage on a dodge"
    );

    standalone.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = {} WHERE spell_id = {STRIKE}",
        CAST_FLAG_DISCOUNT_POWER_ON_MISS | CAST_FLAG_CANNOT_CRIT
    ));
    reseed_caster(&standalone);
    strike(&standalone, wolf);
    assert_eq!(
        power(&standalone),
        1000 - 2 * HAMSTRING_COST + 82,
        "vmangos keeps the whole rage cost of a miss"
    );
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

// The Character stands 15 yards east of the wolf, both face each other, and it holds a full
// `power_type` pool of `power`. The wolf can neither parry nor block.
fn stage(standalone: &mut Standalone, class: u32, power_type: u8, power: u32) -> u64 {
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
    let bytes: u32 = number(
        &standalone.query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0],
        "unit_bytes_0",
    );
    let bytes = (bytes & 0x00FF_00FF) | (class << 8) | (u32::from(power_type) << 24);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {bytes}, power = {power}, \
         max_power = {power}, x = {}, y = {wolf_y}, orientation = {} WHERE guid = {PLAYER}",
        wolf_x + 15.0,
        std::f32::consts::PI
    ));
    standalone.insert_row(
        "game_creature_defense",
        &[
            ("entry", &TEST_WOLF_ENTRY),
            ("no_parry", &true),
            ("no_block", &true),
        ],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn stage_strike(standalone: &Standalone, power_type: u8, cost: u32, extra_cast_flags: u32) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &STRIKE),
            ("name", &"Refunding strike fixture"),
            ("power_type", &power_type),
            ("cost", &cost),
            ("range_yd", &30),
            ("school_mask", &PHYSICAL),
            ("damage_class", &DAMAGE_CLASS_MELEE),
            (
                "cast_flags",
                &(CAST_FLAG_DISCOUNT_POWER_ON_MISS | CAST_FLAG_CANNOT_CRIT | extra_cast_flags),
            ),
            ("is_negative", &true),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(STRIKE, 0)),
            ("spell_id", &STRIKE),
            ("kind", &E_WEAPON_STRIKE),
            ("base_points", &0),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &100),
            ("p_0_kind", &255),
        ],
    );
}

fn seed_caster(standalone: &Standalone, wolf: u64) {
    standalone.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &PLAYER),
            ("target_guid", &wolf),
            ("state", &AVOIDED_STATE),
        ],
    );
}

fn reseed_caster(standalone: &Standalone) {
    standalone.assert_sql(&format!(
        "UPDATE game_conformance_spell_random SET state = {AVOIDED_STATE} \
         WHERE character_guid = {PLAYER}"
    ));
}

fn strike(standalone: &Standalone, wolf: u64) {
    standalone.assert_call(
        "debug_cast_at",
        &[&PLAYER.to_string(), &STRIKE.to_string(), &wolf.to_string()],
    );
}

fn power(standalone: &Standalone) -> u32 {
    number(
        &standalone.query_rows(&format!(
            "SELECT power FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0],
        "power",
    )
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    number(
        &standalone.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {guid}"
        ))[0],
        "health",
    )
}
