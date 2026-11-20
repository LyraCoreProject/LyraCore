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

const CAT_FORM: u32 = 768;
const BEAR_FORM: u32 = 5487;
const DIRE_BEAR_FORM: u32 = 9634;

/// Writes the druid Forms without their passives, so only the Form's own rules move the numbers.
fn seed_druid_forms(shard: &Standalone) {
    shard.assert_call("debug_seed_druid_forms", &[]);
    shard.assert_sql("DELETE FROM game_form_spell");
}

fn shift(shard: &Standalone, form_spell: u32) {
    shard.assert_sql("DELETE FROM game_global_cooldown WHERE caster_guid = 1");
    shard.assert_call("debug_force_cast", &["1", &form_spell.to_string()]);
}

/// Leaves the Form as the client's CMSG_CANCEL_AURA does.
fn leave_form(shard: &Standalone, form_spell: u32) {
    shard.assert_call("debug_shed_spell_auras", &["1", &form_spell.to_string()]);
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

    let mut shard = Standalone::start("white-melee-weapon-delay");
    let target = beside_a_wolf(&mut shard, "");
    seed_druid_forms(&shard);
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
    for (form, expected) in [
        (CAT_FORM, "1000"),
        (BEAR_FORM, "2500"),
        (DIRE_BEAR_FORM, "2500"),
    ] {
        shift(&shard, form);
        assert_eq!(attack_time(&shard), expected, "Form spell {form}");
        leave_form(&shard, form);
    }
    assert_eq!(
        attack_time(&shard),
        "1900",
        "leaving the Form takes the blade up"
    );

    // vmangos Aura::HandleAuraModDisarm sets the main hand back to BASE_ATTACK_TIME.
    shard.assert_call("debug_force_cast", &["1", &DISARM.to_string()]);
    assert_eq!(attack_time(&shard), "2000", "disarmed");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_feral_druid_swings_level_damage_instead_of_its_weapon() {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;

    let mut shard = Standalone::start("white-melee-feral-damage");
    let target = beside_a_wolf(
        &mut shard,
        "UPDATE game_character SET class = 11, level = 60 WHERE guid = 1",
    );
    seed_druid_forms(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = 3800, damage_min = 100.0, damage_max = 150.0, \
         stat_strength = 0, stat_agility = 0 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    shard.assert_sql("UPDATE game_world_entity SET strength = 50, agility = 40 WHERE guid = 1");

    // vmangos at level 60: Cat 51-75 + 120 AP / 14; Bear 127.5-187.5 + 80 AP / 14 * 2.5.
    for (form, expected) in [(CAT_FORM, ("59", "83")), (BEAR_FORM, ("141", "201"))] {
        shift(&shard, form);
        let readout = swing_readout(&shard, &target);
        assert_eq!(
            (readout["base_min"].as_str(), readout["base_max"].as_str()),
            expected,
            "Form spell {form}"
        );
        leave_form(&shard, form);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_usable_weapon_change_in_combat_restarts_the_main_hand_swing() {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;
    use lyracore_shared::constants::unit_flags;

    let mut shard = Standalone::start("white-melee-weapon-swap");
    let target = beside_a_wolf(&mut shard, "");
    seed_druid_forms(&shard);
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
    shift(&shard, CAT_FORM);
    assert!(!unequip_restarts(&shard), "a feral form holds no weapon");
    leave_form(&shard, CAT_FORM);

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

fn feral_druid(name: &str) -> Standalone {
    use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;

    let mut shard = Standalone::start(name);
    beside_a_wolf(
        &mut shard,
        "UPDATE game_character SET class = 11, level = 60 WHERE guid = 1",
    );
    seed_druid_forms(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = 2000, damage_min = 10.0, damage_max = 15.0, \
         stat_strength = 0, stat_agility = 0 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    shard.assert_sql("UPDATE game_world_entity SET strength = 80, agility = 40 WHERE guid = 1");
    shard
}

fn predatory_strikes_fixture(shard: &Standalone, spell_id: u32, amount: i32, duration_ms: u32) {
    use lyracore_package_delta::packed_spell_effect_id;
    use lyracore_shared::spell::taxonomy::{A_FLAG, T_SELF};

    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &"Predatory Strikes fixture"),
            ("school_mask", &1),
            ("duration_ms", &duration_ms),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &A_FLAG),
            ("base_points", &amount),
            ("target", &T_SELF),
        ],
    );
}

fn druid_sheet(shard: &Standalone) -> [String; 3] {
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
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_shapeshift_refreshes_the_sheet_attack_power_and_damage() {
    let shard = feral_druid("white-melee-shapeshift-sheet");

    // vmangos Player::InitDataForForm: Cat AP 2 * 80 + 40 - 20 swings 51-75 + 180 / 14.
    shift(&shard, CAT_FORM);
    assert_eq!(druid_sheet(&shard), ["180", "63", "87"], "Cat Form");
    // Caster AP 2 * 80 - 20 swings the 10-15 weapon + 140 / 14 * 2.0.
    leave_form(&shard, CAT_FORM);
    assert_eq!(
        druid_sheet(&shard),
        ["140", "30", "35"],
        "back in caster form"
    );
}

fn advance_virtual_clock(shard: &Standalone, ticks: usize) {
    let mut connection = shard.connect_reducers(&[]);
    for _ in 0..ticks {
        connection
            .call("debug_advance_virtual_clock", serde_json::json!([50_000]))
            .unwrap();
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_sheet_follows_the_predatory_strikes_aura_as_it_comes_and_goes() {
    const PREDATORY_STRIKES: u32 = 16_975;
    const AURA_TICKS: usize = 20;

    let shard = feral_druid("white-melee-predatory-strikes-aura");
    predatory_strikes_fixture(&shard, PREDATORY_STRIKES, 150, 1_000);
    shift(&shard, CAT_FORM);
    assert_eq!(druid_sheet(&shard), ["180", "63", "87"], "Cat Form");

    // vmangos: Predatory Strikes 150 adds level 60 * 150 / 100 = 90 to Cat Form AP, 270 in all.
    shard.assert_call("debug_force_cast", &["1", &PREDATORY_STRIKES.to_string()]);
    assert_eq!(
        druid_sheet(&shard),
        ["270", "70", "94"],
        "the Aura arrives in Cat Form"
    );

    advance_virtual_clock(&shard, AURA_TICKS);
    assert_eq!(
        druid_sheet(&shard),
        ["180", "63", "87"],
        "the Aura expires in Cat Form"
    );

    shard.assert_call("debug_force_cast", &["1", &PREDATORY_STRIKES.to_string()]);
    assert_eq!(
        druid_sheet(&shard),
        ["270", "70", "94"],
        "the Aura arrives again"
    );
    leave_form(&shard, CAT_FORM);
    assert_eq!(
        druid_sheet(&shard),
        ["140", "30", "35"],
        "the talent adds nothing in caster form"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_casters_flat_attack_power_offsets_a_negative_base() {
    use lyracore_package_delta::packed_spell_effect_id;
    use lyracore_shared::spell::taxonomy::{A_MOD_COMBAT, COMBAT_ATTACK_POWER, T_SELF};
    const FLAT_ATTACK_POWER: u32 = 5_090_414;

    let mut shard = Standalone::start("white-melee-negative-base");
    let target = beside_a_wolf(
        &mut shard,
        "UPDATE game_character SET class = 8, level = 1 WHERE guid = 1",
    );
    shard.assert_sql("UPDATE game_world_entity SET strength = 5 WHERE guid = 1");
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &FLAT_ATTACK_POWER),
            ("name", &"Flat attack power fixture"),
            ("school_mask", &1),
            ("duration_ms", &600_000),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(FLAT_ATTACK_POWER, 0)),
            ("spell_id", &FLAT_ATTACK_POWER),
            ("kind", &A_MOD_COMBAT),
            ("base_points", &232),
            ("target", &T_SELF),
            ("p_0", &COMBAT_ATTACK_POWER),
        ],
    );
    let range = |shard: &Standalone| {
        let readout = swing_readout(shard, &target);
        (readout["base_min"].clone(), readout["base_max"].clone())
    };

    // vmangos: a Mage's base AP is Strength 5 - 10 = -5, so the total clamps at 0 and the
    // unarmed swing is 1-3.
    assert_eq!(range(&shard), ("1".into(), "3".into()), "before the bonus");
    // -5 + 232 = 227 AP adds 227 / 14 * 2.0 = 32; clamping the base first would add 33.
    shard.assert_call("debug_force_cast", &["1", &FLAT_ATTACK_POWER.to_string()]);
    assert_eq!(range(&shard), ("33".into(), "35".into()), "with +232 AP");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn learning_predatory_strikes_in_cat_form_raises_the_sheet_attack_power() {
    const TALENT: u32 = 9_001;
    const RANK_1: u32 = 16_972;

    let shard = feral_druid("white-melee-predatory-strikes-talent");
    predatory_strikes_fixture(&shard, RANK_1, 50, 600_000);
    shard.insert_row(
        "game_talent_tab",
        &[
            ("tab_id", &1),
            ("name", &"Feral Combat fixture"),
            ("class_mask", &(1u32 << 10)),
            ("race_mask", &0),
            ("order_index", &0),
        ],
    );
    shard.insert_row(
        "game_talent",
        &[
            ("talent_id", &TALENT),
            ("name", &"Predatory Strikes fixture"),
            ("tab_id", &1),
            ("max_rank", &3),
            ("spell_id", &RANK_1),
        ],
    );
    shift(&shard, CAT_FORM);
    assert_eq!(druid_sheet(&shard), ["180", "63", "87"], "Cat Form");

    shard.assert_call("debug_learn_talent", &["1", &TALENT.to_string()]);
    // Rank 1 adds level 60 * 50 / 100 = 30: 210 AP swings 51-75 + 210 / 14.
    assert_eq!(
        druid_sheet(&shard),
        ["210", "66", "90"],
        "Predatory Strikes 1"
    );
}
