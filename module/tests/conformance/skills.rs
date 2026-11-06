use std::collections::BTreeMap;

use lyracore_test_support::{ReducerConnection, Standalone};
use wire_client::decode::Field;

use super::{decoded_taxi_entry_packets, enter, fixture};

fn import_spell_plan(shard: &Standalone) -> bool {
    let Some(path) = std::env::var_os("LYRACORE_SKILL_SPELL_PLAN") else {
        return false;
    };
    let statements: Vec<String> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for statement in statements {
        shard.assert_sql(&statement);
    }
    true
}

fn racial_spells(shard: &Standalone) {
    shard.assert_sql("DELETE FROM game_level_stats WHERE race_class_level = 65793");
    shard.assert_sql("INSERT INTO game_level_stats (race_class_level,race,class,level,strength,agility,stamina,intellect,spirit) VALUES (65793,1,1,1,23,20,22,20,20)");
    shard.assert_sql("DELETE FROM game_crit_per_agility WHERE class_level = 257");
    shard.assert_sql(
        "INSERT INTO game_crit_per_agility (class_level,class,level,rate) VALUES (257,1,1,4.0)",
    );
    if import_spell_plan(shard) {
        return;
    }
    for (spell, name, lines) in [
        (20597, "Sword Specialization", [43, 55]),
        (20864, "Mace Specialization", [54, 160]),
    ] {
        shard.assert_sql(&format!("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES ({spell},'{name}',0,0,0,0,0,0,0,1,0,0,0,0,80,1,0,false,0,0,0,0,0,0,0,0)"));
        for (index, line) in lines.into_iter().enumerate() {
            shard.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({},{spell},{index},189,5,1,0.0,0,0,0.0,0,0,0,{line},18,1,0,false,0.0)", spell * 4 + index));
        }
        shard.assert_sql(&format!("INSERT INTO game_createinfo_spell (id,race,class,spell_id) VALUES ({spell},1,1,{spell})"));
    }
}

fn fields(connection: &mut ReducerConnection, guid: u64) -> BTreeMap<String, Field> {
    connection
        .call(
            "native_world_receive",
            serde_json::json!([0x1dcu32, vec![0u8; 8]]),
        )
        .unwrap();
    let mut result = BTreeMap::new();
    for packet in decoded_taxi_entry_packets(&connection.drain_packets()) {
        let Some(Field::List(objects)) = packet.fields.get("objects") else {
            continue;
        };
        for object in objects {
            let Field::Object(object) = object else {
                continue;
            };
            if object.get("guid") != Some(&Field::Guid(guid)) {
                continue;
            }
            if let Some(Field::Object(values)) = object.get("values") {
                result.extend(values.clone());
            }
        }
    }
    result
}

fn skill(values: &BTreeMap<String, Field>, line: u32) -> (u16, u16, i16, i16) {
    let word = |field| match values.get(&format!("field_{field}")) {
        Some(Field::Unsigned(value)) => *value as u32,
        None => 0,
        other => panic!("unexpected skill field: {other:?}"),
    };
    let index = (718..1102)
        .step_by(3)
        .find(|&index| word(index) & 0xffff == line)
        .unwrap_or_else(|| panic!("skill {line} is absent"));
    let ranks = word(index + 1);
    let bonuses = word(index + 2);
    (
        ranks as u16,
        (ranks >> 16) as u16,
        bonuses as i16,
        (bonuses >> 16) as i16,
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn starting_skills_and_crit_survive_login_reconnect_and_publish_repair() {
    let (shard, guid, _) = fixture("starting-skills");
    racial_spells(&shard);
    let mut connection = enter(&shard, guid);
    let values = fields(&mut connection, guid);
    assert_eq!(skill(&values, 43), (1, 5, 0, 5));
    assert_eq!(skill(&values, 54), (1, 5, 0, 5));
    assert_eq!(skill(&values, 162), (1, 5, 0, 0));
    assert_eq!(skill(&values, 95), (1, 5, 0, 0));
    assert_eq!(values.get("field_1109"), Some(&Field::Unsigned(1084311470)));
    assert_eq!(values.get("field_1110"), Some(&Field::Unsigned(1083808154)));

    shard.assert_sql(&format!("UPDATE game_player_skill SET current = 4 WHERE character_guid = {guid} AND skill_line = 43"));
    shard.assert_call("debug_repair_after_publish", &[]);
    shard.assert_call("debug_repair_after_publish", &[]);
    let mut reconnect = enter(&shard, guid);
    let values = fields(&mut reconnect, guid);
    assert_eq!(skill(&values, 43), (4, 5, 0, 5));
    assert_eq!(
        values.get("field_1109"),
        Some(&Field::Unsigned(5.16f32.to_bits().into()))
    );
    assert_eq!(values.get("field_1110"), Some(&Field::Unsigned(1083808154)));
}

fn skill_spell(
    shard: &Standalone,
    spell: u32,
    name: &str,
    line: u32,
    points: i32,
    attributes: u32,
    permanent: bool,
) {
    shard.assert_sql(&format!("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES ({spell},'{name}',0,0,0,0,0,0,900000,1,0,0,0,0,{attributes},1,0,false,0,0,0,0,0,0,0,900000)"));
    shard.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({},{spell},0,189,{points},1,0.0,0,0,0.0,0,0,0,{line},18,{},0,false,0.0)",spell*4,u8::from(permanent)));
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET equip_supported = true WHERE spell_id = {spell}"
    ));
}

fn cast(shard: &Standalone, connection: &mut ReducerConnection, spell: u32) {
    let mut body = spell.to_le_bytes().to_vec();
    body.extend(0u16.to_le_bytes());
    connection
        .call("native_world_receive", serde_json::json!([0x12eu32, body]))
        .unwrap();
    let cast_ms: u32 = shard.query_rows(&format!(
        "SELECT cast_time_ms FROM game_spell WHERE spell_id = {spell}"
    ))[0]["cast_time_ms"]
        .parse()
        .unwrap();
    shard.assert_call(
        "debug_advance_roll_batch",
        &[&cast_ms.div_ceil(50).max(1).to_string()],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equipped_and_cast_skill_bonuses_stack_expire_and_repair_without_changing_progress() {
    use super::buy_in_slot::{grant, request, stage};
    let (shard, mut connection, guid, _) = stage("skill-equipment-and-casts");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET agility = 20 WHERE guid = {guid}"
    ));
    let imported = import_spell_plan(&shard);
    if !imported {
        skill_spell(&shard, 7515, "Increased Defense", 95, 3, 192, false);
        skill_spell(&shard, 27720, "Buttermilk Delight", 95, 13, 0, false);
        skill_spell(
            &shard,
            7038,
            "Forsaken Skill: Swords",
            43,
            -100,
            201326608,
            false,
        );
    }
    shard.assert_sql("DELETE FROM game_crit_per_agility WHERE class_level = 257");
    shard.assert_sql(
        "INSERT INTO game_crit_per_agility (class_level,class,level,rate) VALUES (257,1,1,4.0)",
    );
    for spell in [27720, 7038] {
        shard.assert_call(
            "debug_learn_spell",
            &[&guid.to_string(), &spell.to_string()],
        );
    }
    shard.assert_sql("UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 11, required_level = 1, max_stack = 1, bonding = 0, stat_stamina = 0, spellid_1 = 7515, spelltrigger_1 = 1, spellid_2 = 0, spellid_3 = 0, spellid_4 = 0, spellid_5 = 0 WHERE entry = 5090050");
    grant(&shard, guid, 5090050, 1, 23);
    grant(&shard, guid, 5090050, 1, 24);
    for slot in [23, 24] {
        request(&mut connection, 0x10a, vec![255, slot]);
    }
    cast(&shard, &mut connection, 27720);
    let values = fields(&mut connection, guid);
    assert_eq!(skill(&values, 95), (1, 5, 19, 0));
    let mut reconnect = enter(&shard, guid);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 5, 19, 0));

    shard.assert_sql(&format!("UPDATE game_aura SET eff_kind = 185, eff_p0 = 0, eff_p0_kind = 0 WHERE target_guid = {guid} AND spell_id = 27720"));
    shard.assert_call("debug_repair_after_publish", &[]);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 5, 19, 0));
    shard.assert_sql(
        "UPDATE game_spell_effect SET kind = 185, equip_supported = false WHERE spell_id = 7515",
    );
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 5, 13, 0));
    if imported {
        import_spell_plan(&shard);
    } else {
        shard.assert_sql(
            "UPDATE game_spell_effect SET kind = 189, equip_supported = true WHERE spell_id = 7515",
        );
    }
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 5, 19, 0));
    grant(&shard, guid, 25, 1, 25);
    request(&mut reconnect, 0x10a, vec![255, 25]);
    cast(&shard, &mut reconnect, 7038);
    assert_eq!(
        skill(&fields(&mut reconnect, guid), 43),
        (1, 5, -100, if imported { 5 } else { 0 })
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT sheet_crit_bp FROM game_world_entity WHERE guid = {guid}"
        ))[0]["sheet_crit_bp"],
        "480"
    );
    request(&mut reconnect, 0x10c, vec![255, 23, 255, 10]);
    request(&mut reconnect, 0x10c, vec![255, 24, 255, 11]);
    let mut reconnect = enter(&shard, guid);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 5, 13, 0));
    shard.assert_sql(&format!("UPDATE game_aura SET expires_at = 1800000000000000 WHERE target_guid = {guid} AND spell_id IN (27720, 7038)"));
    shard.assert_call("debug_advance_roll_batch", &["20"]);
    let mut reconnect = enter(&shard, guid);
    let values = fields(&mut reconnect, guid);
    assert_eq!(skill(&values, 95), (1, 5, 0, 0));
    assert_eq!(skill(&values, 43), (1, 5, 0, if imported { 5 } else { 0 }));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn ranged_skill_caps_and_empty_slots_survive_repair_and_reconnect() {
    use super::buy_in_slot::{grant, request};
    let (shard, guid, _) = fixture("ranged-skill-caps");
    racial_spells(&shard);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("INSERT INTO game_skill_ability (id,spell_id,skill_line,race_mask,class_mask,min_skill,acquire_method,gray,green) VALUES (999990,264,45,0,13,1,2,0,0)");
    shard.assert_sql(
        "INSERT INTO game_createinfo_spell (id,race,class,spell_id) VALUES (999990,1,1,264)",
    );
    let mut connection = enter(&shard, guid);
    let values = fields(&mut connection, guid);
    assert_eq!(skill(&values, 45), (1, 5, 0, 0));
    assert_eq!(values.get("field_1110"), Some(&Field::Unsigned(1083808154)));
    shard.assert_sql(&format!("UPDATE game_player_skill SET current = 4 WHERE character_guid = {guid} AND skill_line = 45"));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 2 WHERE guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_character SET level = 2 WHERE guid = {guid}"
    ));
    shard.assert_sql("DELETE FROM game_level_stats WHERE race_class_level = 65794");
    shard.assert_sql("INSERT INTO game_level_stats (race_class_level,race,class,level,strength,agility,stamina,intellect,spirit) VALUES (65794,1,1,2,24,21,23,20,20)");
    shard.assert_sql("DELETE FROM game_crit_per_agility WHERE class_level = 258");
    shard.assert_sql(
        "INSERT INTO game_crit_per_agility (class_level,class,level,rate) VALUES (258,1,2,4.0)",
    );
    shard.assert_call("debug_repair_after_publish", &[]);
    let mut connection = enter(&shard, guid);
    assert_eq!(skill(&fields(&mut connection, guid), 45), (4, 10, 0, 0));
    shard.assert_sql("UPDATE game_item_template SET class = 2, subclass = 2, inventory_type = 15, required_skill = 0, required_level = 1, spellid_1 = 0, spellid_2 = 0, spellid_3 = 0, spellid_4 = 0, spellid_5 = 0 WHERE entry = 5090050");
    grant(&shard, guid, 5090050, 1, 30);
    request(&mut connection, 0x10a, vec![255, 30]);
    let mut connection = enter(&shard, guid);
    let with_bow = fields(&mut connection, guid);
    request(&mut connection, 0x10c, vec![255, 30, 255, 17]);
    let mut connection = enter(&shard, guid);
    let without_bow = fields(&mut connection, guid);
    let crit = |values: &BTreeMap<String, Field>| match values.get("field_1110") {
        Some(Field::Unsigned(value)) => f32::from_bits(*value as u32),
        other => panic!("ranged crit missing: {other:?}"),
    };
    assert!((crit(&with_bow) - crit(&without_bow) - 0.16).abs() < 0.00001);
    assert_eq!(skill(&without_bow, 45), (4, 10, 0, 0));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_skill_aura_does_not_grant_a_profession_but_applies_when_the_skill_is_learned() {
    use super::buy_in_slot::{grant, request, stage};
    let (shard, mut connection, guid, _) = stage("skill-profession-requirement");
    if !import_spell_plan(&shard) {
        skill_spell(&shard, 7824, "Blacksmithing Skill +10", 164, 10, 128, false);
    }
    shard.assert_call("debug_learn_spell", &[&guid.to_string(), "7824"]);
    shard.assert_sql("UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 11, required_level = 1, required_skill = 164, required_skill_rank = 10, max_stack = 1, bonding = 0, spellid_1 = 0, spellid_2 = 0, spellid_3 = 0, spellid_4 = 0, spellid_5 = 0 WHERE entry = 5090050");
    let item = grant(&shard, guid, 5090050, 1, 23);
    cast(&shard, &mut connection, 7824);
    assert!(shard.query_rows(&format!("SELECT current FROM game_player_skill WHERE character_guid = {guid} AND skill_line = 164")).is_empty());
    let packets = request(&mut connection, 0x10a, vec![255, 23]);
    assert!(packets.iter().any(|(opcode, _)| *opcode == 0x112));
    shard.assert_call("debug_learn_profession", &[&guid.to_string(), "164", "75"]);
    assert_eq!(skill(&fields(&mut connection, guid), 164), (1, 75, 10, 0));
    request(&mut connection, 0x10a, vec![255, 23]);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE guid = {item}"
        ))[0]["slot"],
        "10"
    );
    assert_eq!(shard.query_rows(&format!("SELECT current FROM game_player_skill WHERE character_guid = {guid} AND skill_line = 164"))[0]["current"], "1");
}
