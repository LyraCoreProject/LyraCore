mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use std::collections::BTreeMap;

use support::Standalone;

fn chances(shard: &Standalone, attacker: &str, target: &str) -> [u32; 5] {
    shard.assert_call("debug_compute_swing", &[attacker, target]);
    let row = &shard.query_rows("SELECT * FROM game_debug_readout WHERE key = 'swing'")[0];
    ["hit_miss_bp", "dodge_bp", "parry_bp", "block_bp", "crit_bp"]
        .map(|field| row[field].parse().unwrap())
}

fn sheet_crit(shard: &Standalone) -> String {
    shard.query_rows("SELECT sheet_crit_bp FROM game_world_entity WHERE guid = 1")[0]
        ["sheet_crit_bp"]
        .clone()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn white_melee_chances_follow_catalogue_skills_and_defender_eligibility() {
    let mut shard = Standalone::start("white-melee-inputs");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    shard.assert_sql("UPDATE game_character SET class = 1, level = 60 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("UPDATE game_world_entity SET simulation_indexed = false, level = 60, agility = 100, x = 0, y = 0, orientation = 0 WHERE guid = 1");
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    shard.assert_call("debug_set_skill", &["1", "95", "300"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    let target = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, agility = 0, x = 2, y = 0, orientation = 3.1415927 WHERE guid = {target}"
    ));
    assert_eq!(chances(&shard, "1", &target), [500, 500, 500, 500, 500]);
    assert_eq!(sheet_crit(&shard), "500");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 63 WHERE guid = {target}"
    ));
    assert_eq!(chances(&shard, "1", &target), [800, 650, 1400, 500, 200]);
    assert_eq!(sheet_crit(&shard), "500");
    shard.assert_call("debug_set_skill", &["1", "162", "295"]);
    assert_eq!(sheet_crit(&shard), "480");
    assert_eq!(chances(&shard, "1", &target), [900, 700, 1700, 500, 80]);
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    assert_eq!(sheet_crit(&shard), "500");
    shard.insert_row(
        "game_creature_defense",
        &[
            ("entry", &TEST_WOLF_ENTRY),
            ("no_parry", &true),
            ("no_block", &true),
        ],
    );
    assert_eq!(chances(&shard, "1", &target), [800, 650, 0, 0, 200]);
    shard.assert_sql(
        &format!("UPDATE game_creature_defense SET no_parry = false, no_block = false WHERE entry = {TEST_WOLF_ENTRY}"),
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0 WHERE guid = {target}"
    ));
    assert_eq!(chances(&shard, "1", &target), [800, 650, 0, 0, 200]);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 3.1415927 WHERE guid = {target}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET creature_type = 11 WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    assert_eq!(chances(&shard, "1", &target)[3], 0);
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET creature_type = 1 WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_creature_defense SET no_defense = true WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    assert_eq!(chances(&shard, "1", &target)[0], 0);
    shard.assert_sql(&format!(
        "UPDATE game_creature_defense SET no_defense = false WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    shard.insert_row(
        "game_crit_per_agility",
        &[
            ("class_level", &2108),
            ("class", &8),
            ("level", &60),
            ("rate", &19.4553),
        ],
    );
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_0 = 2049 WHERE guid = 1");
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    assert_eq!(sheet_crit(&shard), "833");
    assert_eq!(chances(&shard, "1", &target)[4], 533);
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_0 = 16777473 WHERE guid = 1");
    shard.assert_call("debug_set_skill", &["1", "162", "300"]);
    let incoming = chances(&shard, &target, "1");
    assert_eq!([incoming[0], incoming[4]], [440, 560]);
    let pvp = chances(&shard, "1", "1");
    assert_eq!([pvp[0], pvp[4]], [500, 500]);
}

const DISARM: u32 = 5_090_411;
const MAINHAND: &str = "15";

/// A Character beside a wolf with the virtual clock stopped; returns the wolf's guid.
fn beside_a_wolf(shard: &mut Standalone, character_sql: &str) -> String {
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    if !character_sql.is_empty() {
        shard.assert_sql(character_sql);
    }
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ))[0]["guid"]
        .clone()
}

fn stage_disarm(shard: &Standalone) {
    use lyracore_package_delta::packed_spell_effect_id;
    use lyracore_shared::spell::taxonomy::{A_DISARM, T_SELF};

    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &DISARM),
            ("name", &"Disarm fixture"),
            ("school_mask", &1),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(DISARM, 0)),
            ("spell_id", &DISARM),
            ("kind", &A_DISARM),
            ("target", &T_SELF),
        ],
    );
}

fn swing_readout(shard: &Standalone, target: &str) -> BTreeMap<String, String> {
    shard.assert_call("debug_compute_swing", &["1", target]);
    shard
        .query_rows("SELECT * FROM game_debug_readout WHERE key = 'swing'")
        .remove(0)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn main_hand_attack_time_follows_the_usable_weapon_and_feral_form() {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;
    use lyracore_shared::spell::taxonomy::{STANCE_BEAR, STANCE_CAT, STANCE_DIRE_BEAR};

    let mut shard = Standalone::start("white-melee-weapon-delay");
    let target = beside_a_wolf(&mut shard, "");
    stage_disarm(&shard);
    let attack_time = |shard: &Standalone| swing_readout(shard, &target)["attack_time_ms"].clone();

    // vmangos Player::_ApplyItemBonuses: the main hand takes the weapon's delay, else 2000 ms.
    assert_eq!(attack_time(&shard), "2000", "unarmed");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = 1900 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    assert_eq!(attack_time(&shard), "1900", "a 1.9 second blade");

    // vmangos Player::InitDataForForm swings Cat Form at 1.0 s and Bear Form at 2.5 s.
    for (stance, expected) in [
        (STANCE_CAT, "1000"),
        (STANCE_BEAR, "2500"),
        (STANCE_DIRE_BEAR, "2500"),
    ] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET stance = {stance} WHERE guid = 1"
        ));
        assert_eq!(attack_time(&shard), expected, "feral form {stance}");
    }
    shard.assert_sql("UPDATE game_world_entity SET stance = 0 WHERE guid = 1");

    // vmangos Aura::HandleAuraModDisarm sets the main hand back to BASE_ATTACK_TIME.
    shard.assert_call("debug_force_cast", &["1", &DISARM.to_string()]);
    assert_eq!(attack_time(&shard), "2000", "disarmed");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_feral_druid_swings_level_damage_instead_of_its_weapon() {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;
    use lyracore_shared::spell::taxonomy::{STANCE_BEAR, STANCE_CAT};

    let mut shard = Standalone::start("white-melee-feral-damage");
    let target = beside_a_wolf(
        &mut shard,
        "UPDATE game_character SET class = 11, level = 60 WHERE guid = 1",
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = 3800, damage_min = 100.0, damage_max = 150.0, \
         stat_strength = 0, stat_agility = 0 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    shard.assert_sql("UPDATE game_world_entity SET strength = 50, agility = 40 WHERE guid = 1");

    // vmangos at level 60: Cat 51-75 + 120 AP / 14; Bear 127.5-187.5 + 80 AP / 14 * 2.5.
    for (stance, expected) in [(STANCE_CAT, ("59", "83")), (STANCE_BEAR, ("141", "201"))] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET stance = {stance} WHERE guid = 1"
        ));
        let readout = swing_readout(&shard, &target);
        assert_eq!(
            (readout["base_min"].as_str(), readout["base_max"].as_str()),
            expected,
            "feral form {stance}"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_usable_weapon_change_in_combat_restarts_the_main_hand_swing() {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;
    use lyracore_shared::constants::unit_flags;
    use lyracore_shared::spell::taxonomy::STANCE_CAT;

    let mut shard = Standalone::start("white-melee-weapon-swap");
    let target = beside_a_wolf(&mut shard, "");
    stage_disarm(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_durability = 50 WHERE entry = {FIXTURE_BLADE}"
    ));
    let equip = |shard: &Standalone| {
        shard.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
        shard.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    };
    let unequip_restarts = |shard: &Standalone| {
        shard.assert_call("debug_unequip_item", &["1", MAINHAND]);
        shard.query_rows("SELECT last_swing_ms FROM game_melee_attack WHERE attacker_guid = 1")[0]
            ["last_swing_ms"]
            != "1"
    };
    shard.assert_call("debug_engage", &["1", &target]);
    let flags: u32 = shard.query_rows("SELECT unit_flags FROM game_world_entity WHERE guid = 1")[0]
        ["unit_flags"]
        .parse()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {} WHERE guid = 1",
        flags | unit_flags::IN_COMBAT
    ));

    // vmangos Player::_ApplyItemBonuses returns before the reset unless the hand can use it.
    equip(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET stance = {STANCE_CAT} WHERE guid = 1"
    ));
    assert!(!unequip_restarts(&shard), "a feral form holds no weapon");
    shard.assert_sql("UPDATE game_world_entity SET stance = 0 WHERE guid = 1");

    equip(&shard);
    shard.assert_call("debug_force_cast", &["1", &DISARM.to_string()]);
    assert!(
        !unequip_restarts(&shard),
        "a disarmed main hand holds no weapon"
    );
    shard.assert_sql(&format!(
        "DELETE FROM game_aura WHERE target_guid = 1 AND spell_id = {DISARM}"
    ));

    equip(&shard);
    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 15",
    );
    assert!(
        !unequip_restarts(&shard),
        "vmangos Player::_ApplyItemMods skips a broken weapon"
    );

    equip(&shard);
    assert!(
        unequip_restarts(&shard),
        "the swing timer restarts at the change"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_shapeshift_refreshes_the_sheet_attack_power_and_damage() {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;
    use lyracore_package_delta::packed_spell_effect_id;
    use lyracore_shared::spell::taxonomy::{A_FLAG, E_SET_STANCE, STANCE_CAT, T_SELF};
    const CAT_FORM: u32 = 5_090_413;
    const PREDATORY_STRIKES: u32 = 16_975;

    let mut shard = Standalone::start("white-melee-shapeshift-sheet");
    beside_a_wolf(
        &mut shard,
        "UPDATE game_character SET class = 11, level = 60 WHERE guid = 1",
    );
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &CAT_FORM),
            ("name", &"Cat Form fixture"),
            ("school_mask", &1),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(CAT_FORM, 0)),
            ("spell_id", &CAT_FORM),
            ("kind", &E_SET_STANCE),
            ("target", &T_SELF),
            ("p_0", &STANCE_CAT),
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = 2000, damage_min = 10.0, damage_max = 15.0, \
         stat_strength = 0, stat_agility = 0 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    shard.assert_sql("UPDATE game_world_entity SET strength = 80, agility = 40 WHERE guid = 1");
    let sheet = |shard: &Standalone| {
        let row = shard
            .query_rows(
                "SELECT sheet_ap_base, sheet_dmg_min, sheet_dmg_max FROM game_world_entity \
                 WHERE guid = 1",
            )
            .remove(0);
        [
            row["sheet_ap_base"].clone(),
            row["sheet_dmg_min"].clone(),
            row["sheet_dmg_max"].clone(),
        ]
    };

    // vmangos Player::InitDataForForm: Cat AP 2 * 80 + 40 - 20 swings 51-75 + 180 / 14.
    shard.assert_call("debug_force_cast", &["1", &CAT_FORM.to_string()]);
    assert_eq!(sheet(&shard), ["180", "63", "87"], "Cat Form");
    // Caster AP 2 * 80 - 20 swings the 10-15 weapon + 140 / 14 * 2.0.
    shard.assert_call("debug_force_cast", &["1", &CAT_FORM.to_string()]);
    assert_eq!(sheet(&shard), ["140", "30", "35"], "back in caster form");

    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &PREDATORY_STRIKES),
            ("name", &"Predatory Strikes fixture"),
            ("school_mask", &1),
            ("duration_ms", &600_000),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PREDATORY_STRIKES, 0)),
            ("spell_id", &PREDATORY_STRIKES),
            ("kind", &A_FLAG),
            ("base_points", &150),
            ("target", &T_SELF),
        ],
    );
    shard.assert_call("debug_force_cast", &["1", &PREDATORY_STRIKES.to_string()]);
    assert_eq!(
        sheet(&shard),
        ["140", "30", "35"],
        "the talent adds nothing in caster form"
    );
    // vmangos: Predatory Strikes 150 adds level 60 * 150 / 100 = 90 to Cat Form AP, 270 in all.
    shard.assert_call("debug_force_cast", &["1", &CAT_FORM.to_string()]);
    assert_eq!(
        sheet(&shard),
        ["270", "70", "94"],
        "Cat Form with the talent"
    );
}
