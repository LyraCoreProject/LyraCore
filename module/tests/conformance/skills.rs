use std::collections::BTreeMap;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{A_MOD_SKILL, P_SKILL_LINE};
use lyracore_test_support::{ReducerConnection, Standalone};
use wire_client::decode::Field;

use super::{decoded_taxi_entry_packets, enter, fixture};

fn import_spell_plan(shard: &Standalone, mut connection: Option<&mut ReducerConnection>) -> bool {
    let Some(path) = std::env::var_os("LYRACORE_SKILL_SPELL_PLAN") else {
        return false;
    };
    let statements: Vec<String> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for statement in statements {
        shard.assert_sql(&statement);
        if let Some(connection) = connection.as_mut() {
            connection
                .call(
                    "native_world_receive",
                    serde_json::json!([0x1dcu32, vec![0u8; 8]]),
                )
                .unwrap();
        }
    }
    true
}

fn human_warrior_stats(
    shard: &Standalone,
    level: u32,
    [strength, agility, stamina, intellect, spirit]: [u32; 5],
) {
    let race_class_level = (1 << 16) | (1 << 8) | level;
    shard.assert_sql(&format!(
        "DELETE FROM game_level_stats WHERE race_class_level = {race_class_level}"
    ));
    shard.insert_row(
        "game_level_stats",
        &[
            ("race_class_level", &race_class_level),
            ("race", &1),
            ("class", &1),
            ("level", &level),
            ("strength", &strength),
            ("agility", &agility),
            ("stamina", &stamina),
            ("intellect", &intellect),
            ("spirit", &spirit),
        ],
    );
}

fn warrior_crit_per_agility(shard: &Standalone, level: u32, rate: f32) {
    let class_level = (1 << 8) | level;
    shard.assert_sql(&format!(
        "DELETE FROM game_crit_per_agility WHERE class_level = {class_level}"
    ));
    shard.insert_row(
        "game_crit_per_agility",
        &[
            ("class_level", &class_level),
            ("class", &1),
            ("level", &level),
            ("rate", &rate),
        ],
    );
}

fn starting_spell(shard: &Standalone, id: u32, spell: u32) {
    shard.insert_row(
        "game_createinfo_spell",
        &[
            ("id", &id),
            ("race", &1),
            ("class", &1),
            ("spell_id", &spell),
        ],
    );
}

fn skill_effect(
    shard: &Standalone,
    spell: u32,
    index: u8,
    line: u32,
    points: i32,
    permanent: bool,
) {
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell, index)),
            ("spell_id", &spell),
            ("effect_index", &index),
            ("kind", &A_MOD_SKILL),
            ("base_points", &points),
            ("die_sides", &1),
            ("p_0", &line),
            ("p_0_kind", &P_SKILL_LINE),
            ("p_1", &u8::from(permanent)),
        ],
    );
}

fn racial_spells(shard: &Standalone) {
    human_warrior_stats(shard, 1, [23, 20, 22, 20, 20]);
    warrior_crit_per_agility(shard, 1, 4.0);
    if import_spell_plan(shard, None) {
        return;
    }
    for (spell, name, lines) in [
        (20597, "Sword Specialization", [43, 55]),
        (20864, "Mace Specialization", [54, 160]),
    ] {
        shard.insert_row(
            "game_spell",
            &[
                ("spell_id", &spell),
                ("name", &name),
                ("school_mask", &1),
                ("attributes", &80),
                ("spell_level", &1),
            ],
        );
        for (index, line) in (0u8..).zip(lines) {
            skill_effect(shard, spell, index, line, 5, true);
        }
        starting_spell(shard, spell, spell);
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

#[track_caller]
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
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell),
            ("name", &name),
            ("duration_ms", &900_000),
            ("school_mask", &1),
            ("attributes", &attributes),
            ("spell_level", &1),
            ("max_duration_ms", &900_000),
        ],
    );
    skill_effect(shard, spell, 0, line, points, permanent);
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET equip_supported = true WHERE spell_id = {spell}"
    ));
}

fn cast(shard: &Standalone, connection: &mut ReducerConnection, spell: u32) {
    let mut body = spell.to_le_bytes().to_vec();
    body.extend(0u16.to_le_bytes());
    if let Err(reason) =
        connection.call("native_world_receive", serde_json::json!([0x12eu32, body]))
    {
        let replies: Vec<(u16, Vec<u8>)> =
            serde_json::from_str(reason.strip_prefix("native_reply:").expect(&reason)).unwrap();
        assert!(
            !replies
                .iter()
                .any(|(opcode, body)| *opcode == 0x130 && body.get(4) == Some(&2)),
            "cast {spell} failed: {reason}"
        );
        connection
            .call("native_world_reply", serde_json::json!([replies]))
            .unwrap();
    }
    let header = shard
        .query_rows(&format!(
            "SELECT cast_time_ms,gcd_ms FROM game_spell WHERE spell_id = {spell}"
        ))
        .pop()
        .unwrap();
    let cast_ms = header["cast_time_ms"].parse::<u32>().unwrap();
    let gcd_ms = header["gcd_ms"].parse::<u32>().unwrap();
    shard.assert_call(
        "debug_advance_roll_batch",
        &[&cast_ms.max(gcd_ms).div_ceil(50).max(1).to_string()],
    );
}

fn swing_hit_info(
    shard: &Standalone,
    connection: &mut ReducerConnection,
    guid: u64,
    target: u64,
) -> u64 {
    use super::buy_in_slot::request;
    shard.assert_sql(&format!(
        "DELETE FROM game_conformance_swing_random WHERE character_guid = {guid}"
    ));
    shard.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &guid), ("state", &7875)],
    );
    request(connection, 0x141, target.to_le_bytes().to_vec());
    connection
        .call("debug_advance_roll_batch", serde_json::json!([4]))
        .unwrap();
    connection
        .call(
            "native_world_receive",
            serde_json::json!([0x1dcu32, vec![0u8; 8]]),
        )
        .unwrap();
    let packets = connection.drain_packets();
    let hits: Vec<_> = packets
        .into_iter()
        .filter_map(|row| {
            let super::Packet(_, _, (tag, opcode), body, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            if tag != 0 || serde_json::from_str::<u16>(opcode.get()).unwrap() != 0x14a {
                return None;
            }
            let body: Vec<_> = (0..body.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&body[i..i + 2], 16).unwrap())
                .collect();
            let packet = wire_client::decode::decode(0x14a, &body).unwrap();
            if packet.fields.get("attacker") != Some(&Field::Guid(guid)) {
                return None;
            }
            match packet.fields.get("hit_info") {
                Some(Field::Unsigned(info)) => Some(*info),
                other => panic!("swing flags missing: {other:?}"),
            }
        })
        .collect();
    request(connection, 0x142, vec![]);
    assert_eq!(hits.len(), 1, "expected one native white swing: {hits:?}");
    hits[0]
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equipped_and_cast_skill_bonuses_stack_expire_and_repair_without_changing_progress() {
    use super::buy_in_slot::{grant, request, stage};
    let (shard, mut connection, guid, _) = stage("skill-equipment-and-casts");
    human_warrior_stats(&shard, 20, [47, 35, 43, 22, 27]);
    warrior_crit_per_agility(&shard, 20, 7.80031);
    shard.assert_sql(&format!(
        "UPDATE game_character SET level = 20 WHERE guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 20, agility = 35 WHERE guid = {guid}"
    ));
    shard.assert_call("debug_repair_after_publish", &[]);
    let imported = import_spell_plan(&shard, Some(&mut connection));
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
    warrior_crit_per_agility(&shard, 1, 4.0);
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
    assert_eq!(skill(&values, 95), (1, 100, 19, 0));
    let mut reconnect = enter(&shard, guid);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 100, 19, 0));

    shard.assert_sql(&format!("UPDATE game_aura SET eff_kind = 185, eff_p0 = 0, eff_p0_kind = 0 WHERE target_guid = {guid} AND spell_id = 27720"));
    shard.assert_call("debug_repair_after_publish", &[]);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 100, 19, 0));
    shard.assert_sql(
        "UPDATE game_spell_effect SET kind = 185, equip_supported = false WHERE spell_id = 7515",
    );
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 100, 13, 0));
    if imported {
        import_spell_plan(&shard, Some(&mut reconnect));
    } else {
        shard.assert_sql(
            "UPDATE game_spell_effect SET kind = 189, equip_supported = true WHERE spell_id = 7515",
        );
    }
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(skill(&fields(&mut reconnect, guid), 95), (1, 100, 19, 0));
    grant(&shard, guid, 25, 1, 25);
    request(&mut reconnect, 0x10a, vec![255, 25]);
    let before_crit = shard.query_rows(&format!(
        "SELECT sheet_crit_bp FROM game_world_entity WHERE guid = {guid}"
    ))[0]["sheet_crit_bp"]
        .parse::<u32>()
        .unwrap();
    shard.assert_call("debug_spawn_at_feet", &[&guid.to_string(), "51000", "2"]);
    let target = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .into_iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!("UPDATE game_world_entity SET level = 20,health = 100000,max_health = 100000 WHERE guid = {target}"));
    shard.assert_sql(&format!(
        "UPDATE game_creature_ai_state SET melee_enabled = false WHERE creature_guid = {target}"
    ));
    // Seed 7875 draws 2490: above the 2380/2480 miss bands, below the reduced-skill band 2500.
    assert_eq!(
        swing_hit_info(&shard, &mut reconnect, guid, target) & 0x10,
        0
    );
    cast(&shard, &mut reconnect, 7038);
    let reduction = shard.query_rows(&format!("SELECT eff_kind,eff_p0,amount FROM game_aura WHERE target_guid = {guid} AND spell_id = 7038"));
    assert_eq!(
        reduction.len(),
        1,
        "skill reduction did not apply: {reduction:?}"
    );
    assert_eq!(
        (
            reduction[0]["eff_kind"].as_str(),
            reduction[0]["eff_p0"].as_str(),
            reduction[0]["amount"].as_str()
        ),
        ("189", "43", "-100")
    );
    assert_eq!(
        skill(&fields(&mut reconnect, guid), 43),
        (1, 100, -100, if imported { 5 } else { 0 })
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT sheet_crit_bp FROM game_world_entity WHERE guid = {guid}"
        ))[0]["sheet_crit_bp"],
        (before_crit - if imported { 24 } else { 4 }).to_string()
    );
    assert_eq!(
        swing_hit_info(&shard, &mut reconnect, guid, target) & 0x10,
        0x10
    );
    shard.assert_call("debug_clear_creatures", &["0"]);
    let defense_current = shard.query_rows(&format!(
        "SELECT current FROM game_player_skill WHERE character_guid = {guid} AND skill_line = 95"
    ))[0]["current"]
        .parse::<u16>()
        .unwrap();
    request(&mut reconnect, 0x10c, vec![255, 23, 255, 10]);
    request(&mut reconnect, 0x10c, vec![255, 24, 255, 11]);
    let mut reconnect = enter(&shard, guid);
    assert_eq!(
        skill(&fields(&mut reconnect, guid), 95),
        (defense_current, 100, 13, 0)
    );
    let duration_ms = [27720, 7038]
        .into_iter()
        .map(|spell| {
            shard.query_rows(&format!(
                "SELECT duration_ms FROM game_spell WHERE spell_id = {spell}"
            ))[0]["duration_ms"]
                .parse::<u32>()
                .unwrap()
        })
        .max()
        .unwrap();
    let mut ticks = duration_ms.div_ceil(50) + 20;
    while ticks > 0 {
        let batch = ticks.min(4000);
        shard.assert_call("debug_advance_roll_batch", &[&batch.to_string()]);
        ticks -= batch;
    }
    let mut reconnect = enter(&shard, guid);
    let values = fields(&mut reconnect, guid);
    assert_eq!(skill(&values, 95), (defense_current, 100, 0, 0));
    assert_eq!(
        skill(&values, 43),
        (1, 100, 0, if imported { 5 } else { 0 })
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn ranged_skill_caps_and_empty_slots_survive_repair_and_reconnect() {
    use super::buy_in_slot::{grant, request};
    let (shard, guid, _) = fixture("ranged-skill-caps");
    racial_spells(&shard);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.insert_row(
        "game_skill_ability",
        &[
            ("id", &999_990),
            ("spell_id", &264),
            ("skill_line", &45),
            ("class_mask", &13),
            ("min_skill", &1),
            ("acquire_method", &2),
        ],
    );
    starting_spell(&shard, 999_990, 264);
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
    human_warrior_stats(&shard, 2, [24, 21, 23, 20, 20]);
    warrior_crit_per_agility(&shard, 2, 4.0);
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
    if !import_spell_plan(&shard, Some(&mut connection)) {
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
