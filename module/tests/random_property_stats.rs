mod support;

use std::collections::BTreeMap;

use support::{poll_until, Standalone, POLL_TIMEOUT};

const ITEM: &str = "5090050";
const POOL: u32 = 5090200;
const PROPERTY: u32 = 5090201;
const ENCHANT: u32 = 5090202;

fn fixture(name: &str, class: u8) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_character SET class = {class} WHERE guid = 1"
    ));
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql(&format!(
        "INSERT INTO game_item_random_property (property_id,enchant_id_1,enchant_id_2,enchant_id_3,suffix) VALUES ({PROPERTY},{ENCHANT},0,0,'of the Stat Fixture')"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_item_property_weight (id,pool_id,property_id,weight) VALUES ({POOL},{POOL},{PROPERTY},10000)"
    ));
    shard
}

fn enchantments(shard: &Standalone, effects: &[(u8, i32, u32)]) {
    let values = effects
        .iter()
        .enumerate()
        .map(|(index, (kind, amount, school_mask))| {
            format!(
                "({},{ENCHANT},{index},{kind},{amount},0,{school_mask})",
                u64::from(ENCHANT) * 256 + index as u64
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    shard.assert_sql(&format!(
        "INSERT INTO game_item_enchantment (id,enchant_id,effect_index,kind,amount,spell_id,school_mask) VALUES {values}"
    ));
}

fn grant_and_equip(shard: &Standalone) {
    shard.assert_call("debug_grant_item", &["1", ITEM, "1"]);
    shard.assert_call("debug_equip_item", &["1", "23"]);
}

fn world_row(shard: &Standalone) -> BTreeMap<String, String> {
    shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .pop()
        .expect("the fixture player must be live")
}

fn readout(shard: &Standalone, key: &str) -> BTreeMap<String, String> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_debug_readout WHERE key = '{key}'"
        ))
        .pop()
        .expect("the debug readout must exist")
}

fn create_damage_spell(shard: &Standalone, spell_id: u32, school_mask: u8) {
    shard.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES ({spell_id},'Stat fixture damage',0,0,0,0,0,40,0,{school_mask},0,0,0,0,0,1,0,true,0,0,0,0,0,0,0)"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES ({},{spell_id},0,1,100,0,0.0,0,1,0.0,0,0,0,0,0,0,0,false)",
        u64::from(spell_id) * 4
    ));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn direct_pools_and_regeneration_follow_working_equipment() {
    let shard = fixture("property-stat-vitals", 8);
    enchantments(&shard, &[(6, 25, 0), (7, 40, 0), (18, 10, 0), (19, 15, 0)]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET random_property = {POOL}, stat_stamina = 0, stat_intellect = 0 WHERE entry = {ITEM}"
    ));
    let bare = world_row(&shard);
    let bare_health: u32 = bare["max_health"].parse().unwrap();
    let bare_mana: u32 = bare["max_power"].parse().unwrap();

    grant_and_equip(&shard);
    let equipped = world_row(&shard);
    assert_eq!(
        equipped["max_health"].parse::<u32>().unwrap(),
        bare_health + 25
    );
    assert_eq!(
        equipped["max_power"].parse::<u32>().unwrap(),
        bare_mana + 40
    );

    shard.assert_call("debug_unequip_item", &["1", "15"]);
    let unequipped = world_row(&shard);
    assert_eq!(
        unequipped["max_health"].parse::<u32>().unwrap(),
        bare_health
    );
    assert_eq!(unequipped["max_power"].parse::<u32>().unwrap(), bare_mana);

    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 23",
    );
    shard.assert_call("debug_equip_item", &["1", "23"]);
    let broken = world_row(&shard);
    assert_eq!(broken["max_health"].parse::<u32>().unwrap(), bare_health);
    assert_eq!(broken["max_power"].parse::<u32>().unwrap(), bare_mana);

    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 70 WHERE owner_guid = 1 AND slot = 15",
    );
    shard.assert_sql("UPDATE game_world_entity SET health = 100, max_health = 100000, power = 100, max_power = 100000, spirit = 30, mana_regen_paused_until_ms = 0 WHERE guid = 1");
    assert!(
        poll_until(POLL_TIMEOUT, || {
            let row = world_row(&shard);
            row["health"] != "100" || row["power"] != "100"
        }),
        "the scheduled regeneration pass did not run"
    );
    let regenerated = world_row(&shard);
    assert_eq!(regenerated["health"], "144");
    assert_eq!(regenerated["power"], "124");

    shard.assert_sql("UPDATE game_world_entity SET power = 100, mana_regen_paused_until_ms = 9999999999999 WHERE guid = 1");
    assert!(
        poll_until(POLL_TIMEOUT, || world_row(&shard)["power"] != "100"),
        "flat mana regeneration stopped during the five-second rule"
    );
    assert_eq!(world_row(&shard)["power"], "108");

    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND dead = false")
        .pop()
        .unwrap()["guid"]
        .clone();
    shard.assert_sql(
        "UPDATE game_world_entity SET health = 100, power = 100000, godmode = true WHERE guid = 1",
    );
    shard.assert_call("debug_engage", &[&wolf, "1"]);
    assert!(
        poll_until(POLL_TIMEOUT, || world_row(&shard)["health"] != "100"),
        "flat health regeneration stopped in combat"
    );
    assert_eq!(world_row(&shard)["health"], "112");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn defenses_and_each_magic_resistance_reach_combat_callers() {
    let shard = fixture("property-stat-defense", 1);
    enchantments(
        &shard,
        &[
            (8, 6, 2),
            (9, 8, 4),
            (10, 10, 8),
            (11, 12, 16),
            (12, 14, 32),
            (13, 16, 64),
            (14, 80, 0),
            (22, 4, 0),
            (23, 200, 0),
            (24, 300, 0),
            (25, 400, 0),
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET random_property = {POOL}, stat_armor = 920, holy_res = 1, fire_res = 2, nature_res = 3, frost_res = 4, shadow_res = 5, arcane_res = 6 WHERE entry = 50053"
    ));
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND dead = false")
        .pop()
        .unwrap()["guid"]
        .clone();

    shard.assert_sql("UPDATE game_world_entity SET armor = 40 WHERE guid = 1");
    shard.assert_call("debug_compute_swing", &[&wolf, "1"]);
    let bare = readout(&shard, "swing");
    shard.assert_call("debug_equip_offhand", &["1", "50053"]);
    shard.assert_call("debug_compute_swing", &[&wolf, "1"]);
    let equipped = readout(&shard, "swing");
    assert_eq!(
        equipped["defender_defense_skill"].parse::<u32>().unwrap(),
        bare["defender_defense_skill"].parse::<u32>().unwrap() + 4
    );
    assert_eq!(
        equipped["dodge_bp"].parse::<u32>().unwrap(),
        bare["dodge_bp"].parse::<u32>().unwrap() + 240
    );
    assert_eq!(
        equipped["parry_bp"].parse::<u32>().unwrap(),
        bare["parry_bp"].parse::<u32>().unwrap() + 340
    );
    assert_eq!(equipped["block_bp"], "940");
    assert_eq!(bare["mitigation_pct"], "7");
    assert_eq!(
        equipped["mitigation_pct"], "68",
        "40 base + 920 template + 80 property armor must mitigate once"
    );

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 10 WHERE guid = {wolf}"
    ));
    for (index, (school, expected)) in
        [(2u8, 90u32), (4, 85), (8, 81), (16, 76), (32, 72), (64, 67)]
            .into_iter()
            .enumerate()
    {
        let spell = 5090210 + index as u32;
        create_damage_spell(&shard, spell, school);
        shard.assert_call("debug_compute_spell", &[&wolf, "1", &spell.to_string()]);
        assert_eq!(
            readout(&shard, "spell")["spell_hit_normal"]
                .parse::<u32>()
                .unwrap(),
            expected,
            "school mask {school} used the wrong resistance"
        );
    }

    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 16",
    );
    shard.assert_call("debug_compute_swing", &[&wolf, "1"]);
    let broken = readout(&shard, "swing");
    assert_eq!(
        broken["defender_defense_skill"],
        bare["defender_defense_skill"]
    );
    assert_eq!(broken["dodge_bp"], bare["dodge_bp"]);
    assert_eq!(broken["parry_bp"], bare["parry_bp"]);
    assert_eq!(broken["block_bp"], "0");
    assert_eq!(broken["mitigation_pct"], "3");
    shard.assert_call("debug_compute_spell", &[&wolf, "1", "5090211"]);
    assert_eq!(readout(&shard, "spell")["spell_hit_normal"], "100");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn spell_power_selects_school_and_healing_power_only_affects_heals() {
    let shard = fixture("property-stat-spells", 8);
    enchantments(
        &shard,
        &[(16, 20, 4), (16, 30, 16), (17, 40, 2), (17, 60, 4)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET random_property = {POOL} WHERE entry = {ITEM}"
    ));
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND dead = false")
        .pop()
        .unwrap()["guid"]
        .clone();
    for (spell, school) in [(5090230, 4u8), (5090231, 16), (5090232, 64)] {
        create_damage_spell(&shard, spell, school);
    }
    let baseline = [5090230u32, 5090231, 5090232].map(|spell| {
        shard.assert_call("debug_compute_spell", &["1", &wolf, &spell.to_string()]);
        readout(&shard, "spell")["spell_hit_normal"]
            .parse::<u32>()
            .unwrap()
    });

    grant_and_equip(&shard);
    for (index, expected_bonus) in [20u32, 30, 0].into_iter().enumerate() {
        let spell = 5090230 + index as u32;
        shard.assert_call("debug_compute_spell", &["1", &wolf, &spell.to_string()]);
        assert_eq!(
            readout(&shard, "spell")["spell_hit_normal"]
                .parse::<u32>()
                .unwrap(),
            baseline[index] + expected_bonus
        );
    }

    shard.assert_sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090233,'Stat fixture heal',0,0,0,0,0,40,0,2,0,0,0,0,0,1,0,false,0,0,0,0,0,0,0)");
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360932,5090233,0,2,100,0,0.0,0,0,0.0,0,0,0,0,0,0,0,false)");
    shard
        .assert_sql("UPDATE game_world_entity SET health = 100, max_health = 10000 WHERE guid = 1");
    shard.assert_call("debug_cast_at", &["1", "5090233", "1"]);
    let healed: u32 = world_row(&shard)["health"].parse().unwrap();
    assert_eq!(
        healed, 240,
        "healing power adds forty to the 100-point heal"
    );

    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 15",
    );
    shard.assert_call("debug_compute_spell", &["1", &wolf, "5090230"]);
    assert_eq!(
        readout(&shard, "spell")["spell_hit_normal"]
            .parse::<u32>()
            .unwrap(),
        baseline[0]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn weapon_damage_uses_the_selected_main_off_and_ranged_item() {
    let shard = fixture("property-stat-weapons", 8);
    enchantments(&shard, &[(15, 20, 0)]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET random_property = {POOL}, damage_min = 8.0, damage_max = 12.0, delay_ms = 2600 WHERE entry = {ITEM}"
    ));
    shard.assert_sql("UPDATE game_world_entity SET strength = 0, agility = 0 WHERE guid = 1");
    shard.assert_call("debug_equip_weapon", &["1", ITEM]);
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND dead = false")
        .pop()
        .unwrap()["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET armor = 0, health = 100000, max_health = 100000 WHERE guid = {wolf}"
    ));
    shard.assert_call("debug_compute_swing", &["1", &wolf]);
    let main = readout(&shard, "swing");
    assert_eq!(main["base_min"], "28");
    assert_eq!(main["base_max"], "32");

    shard.assert_sql(&format!(
        "UPDATE game_item_template SET inventory_type = 26, subclass = 19 WHERE entry = {ITEM}"
    ));
    shard.assert_sql("UPDATE game_item_instance SET slot = 17 WHERE owner_guid = 1 AND slot = 15");
    shard.assert_call("debug_set_level", &["1", "1"]);
    let ranged = world_row(&shard);
    assert_eq!(ranged["sheet_ranged_dmg_min"], "28");
    assert_eq!(ranged["sheet_ranged_dmg_max"], "32");
    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 17",
    );
    shard.assert_call("debug_set_level", &["1", "1"]);
    let broken_ranged = world_row(&shard);
    assert_eq!(broken_ranged["sheet_ranged_dmg_min"], "0");
    assert_eq!(broken_ranged["sheet_ranged_dmg_max"], "0");

    shard.assert_sql(&format!(
        "UPDATE game_item_template SET inventory_type = 22, subclass = 7 WHERE entry = {ITEM}"
    ));
    shard.assert_sql("UPDATE game_item_instance SET slot = 16, durability = 70 WHERE owner_guid = 1 AND slot = 17");
    shard.assert_sql("UPDATE game_world_entity SET strength = 0, agility = 0, health = 100000, max_health = 100000 WHERE guid = 1");
    shard.assert_sql("DELETE FROM game_combat_event");
    shard.assert_call("debug_engage", &["1", &wolf]);
    assert!(
        poll_until(POLL_TIMEOUT, || {
            shard
                .query_rows("SELECT damage FROM game_combat_event WHERE attacker_guid = 1")
                .iter()
                .any(|row| row["damage"].parse::<u32>().unwrap() >= 14)
        }),
        "the off-hand stream never used its 14..16 property-adjusted range"
    );

    let before = shard
        .query_rows("SELECT last_offhand_swing_ms FROM game_melee_attack WHERE attacker_guid = 1")
        [0]["last_offhand_swing_ms"]
        .clone();
    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 16",
    );
    shard.assert_sql("DELETE FROM game_combat_event");
    shard.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    assert!(poll_until(POLL_TIMEOUT, || {
        !shard
            .query_rows("SELECT id FROM game_combat_event WHERE attacker_guid = 1")
            .is_empty()
    }));
    let after = shard
        .query_rows("SELECT last_offhand_swing_ms FROM game_melee_attack WHERE attacker_guid = 1")
        [0]["last_offhand_swing_ms"]
        .clone();
    assert_eq!(
        after, before,
        "a broken off-hand must not advance its swing clock"
    );
}
