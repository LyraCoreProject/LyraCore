use super::*;
use std::path::Path;

fn imported_spells(ids: &[u32]) -> crate::standalone_support::Standalone {
    imported_spell_fixture(ids, false)
}

fn imported_spell_fixture(ids: &[u32], melee: bool) -> crate::standalone_support::Standalone {
    let dbc = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
    let path = std::env::var("LYRACORE_VMANGOS_SQLITE").expect("set LYRACORE_VMANGOS_SQLITE");
    let source = crate::world_source::WorldSource::open(Path::new(&path)).unwrap();
    let plan = crate::spell::selected_world_plan(&source, &dbc, ids, &[]).unwrap();
    let mut shard = crate::standalone_support::Standalone::start("spell-power-world-plan");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_start_virtual_clock", &["10000000"]);
    for sql in plan.stmts {
        shard.assert_sql(&sql);
    }
    for sql in crate::dbc::replacement_plan(&source, &dbc).unwrap().stmts {
        if sql.starts_with("DELETE FROM game_faction_template ")
            || sql.starts_with("INSERT INTO game_faction_template ")
        {
            shard.assert_sql(&sql);
        }
    }
    shard.clear_schedule("game_creature_move_schedule");
    if !melee {
        shard.clear_schedule("game_melee_schedule");
    }
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_level", &["1", "60"]);
    let wolf = ((0xf130u64 << 48) | (51000u64 << 24) | 1).to_string();
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(-8949.95, -132.493);
    for guid in ["1", &wolf] {
        shard.assert_sql(&format!("UPDATE game_world_entity SET x = -8949.95, y = -132.493, z = 83.5312, grid_x = {grid_x}, grid_y = {grid_y}, level = 60, health = 100000, max_health = 100000, power = 100000, max_power = 100000, armor = 0 WHERE guid = {guid}"));
    }
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{wolf},0)"));
    shard
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_casts_keep_school_sign_and_repair() {
    let shard = imported_spells(&[
        11390, 17539, 23271, 702, 604, 1008, 8599, 7685, 18382, 5102, 20473, 12559, 12871, 133,
        172, 1449, 28143,
    ]);
    let rows = shard.query_rows(
        "SELECT kind,p0,base_points,equip_supported FROM game_spell_effect WHERE spell_id = 11390",
    );
    assert_eq!(rows[0]["kind"], "208");
    assert_eq!(rows[0]["p0"], "126");
    assert_eq!(rows[0]["base_points"], "20");
    assert_eq!(rows[0]["equip_supported"], "true");
    let area = shard.query_rows("SELECT kind,equip_supported FROM game_spell_effect WHERE spell_id = 28143 AND effect_index = 0");
    assert_eq!(area[0]["kind"], "185");
    assert_eq!(area[0]["equip_supported"], "false");
    shard.assert_call("debug_begin_cast", &["1", "11390", "1"]);
    let auras = shard.query_rows(
        "SELECT eff_kind,amount,eff_p0 FROM game_aura WHERE target_guid = 1 AND spell_id = 11390",
    );
    assert_eq!(auras.len(), 1);
    assert_eq!(auras[0]["amount"], "20");
    assert_eq!(auras[0]["eff_kind"], "208");
    shard.assert_sql("UPDATE game_aura SET eff_kind = 185, eff_p0 = 0 WHERE spell_id = 11390");
    shard.assert_call("debug_repair_after_publish", &[]);
    let repaired = shard.query_rows(
        "SELECT eff_kind,amount,eff_p0 FROM game_aura WHERE target_guid = 1 AND spell_id = 11390",
    );
    assert_eq!(repaired, auras);
    for (spell, effect, expected) in [
        (20473, 0, 1.0f32),
        (12559, 0, 0.12321428),
        (12871, 0, 0.038333334),
        (133, 0, 0.123),
        (172, 0, 0.08),
        (1449, 0, 0.111),
    ] {
        let coefficient: f32 = shard.query_rows(&format!("SELECT bonus_coefficient FROM game_spell_effect WHERE spell_id = {spell} AND effect_index = {effect}"))[0]["bonus_coefficient"].parse().unwrap();
        assert!(
            (coefficient - expected).abs() < 0.000001,
            "{spell}: {coefficient}"
        );
    }
}

fn cast(shard: &crate::standalone_support::Standalone, caster: u64, spell: u32, target: u64) {
    shard.assert_call(
        "debug_begin_cast",
        &[&caster.to_string(), &spell.to_string(), &target.to_string()],
    );
    shard.assert_call("debug_advance_roll_batch", &["61"]);
}

fn fireball(shard: &crate::standalone_support::Standalone, seed: u64) -> u32 {
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    shard.assert_sql("DELETE FROM game_aura WHERE spell_id = 133");
    shard.assert_sql(&format!(
        "UPDATE game_conformance_spell_random SET state = {seed} WHERE character_guid = 1"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 100000 WHERE guid = {wolf}"
    ));
    shard.assert_sql(
        "UPDATE game_world_entity SET power = 100000, max_power = 100000 WHERE guid = 1",
    );
    cast(shard, 1, 133, wolf);
    let health: u32 = shard.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {wolf}"
    ))[0]["health"]
        .parse()
        .unwrap();
    100000 - health
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_scales_cast_and_equipped_damage_once() {
    let shard = imported_spells(&[133, 11390, 23271, 7685]);
    let ordinary = fireball(&shard, 0);
    assert!(ordinary > 0);
    cast(&shard, 1, 11390, 1);
    assert_eq!(fireball(&shard, 0), ordinary + 3);
    cast(&shard, 1, 23271, 1);
    assert_eq!(fireball(&shard, 0), ordinary + 24);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(fireball(&shard, 0), ordinary + 24);

    shard.assert_sql("UPDATE game_character SET class = 9 WHERE guid = 1");
    let packed: u32 = shard.query_rows("SELECT unit_bytes_0 FROM game_world_entity WHERE guid = 1")
        [0]["unit_bytes_0"]
        .parse()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {} WHERE guid = 1",
        (packed & !0xff00_ff00) | (9 << 8)
    ));
    import_items(&shard, &[25, 50, 51, 52, 6898]);
    shard.assert_call("debug_grant_item", &["1", "6898", "1"]);
    let item = shard
        .query_rows("SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = 6898");
    shard.assert_call("debug_equip_item", &["1", &item[0]["slot"]]);
    let equipped = shard.query_rows(
        "SELECT amount,eff_p0 FROM game_aura WHERE target_guid = 1 AND spell_id = 7685",
    );
    assert_eq!(equipped.len(), 1);
    assert_eq!(equipped[0]["amount"], "4");
    assert_eq!(fireball(&shard, 0), ordinary + 25);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(fireball(&shard, 0), ordinary + 25);
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_dampen_and_amplify_scale_incoming_damage() {
    let shard = imported_spells(&[133, 604, 1008]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    let ordinary = fireball(&shard, 0);
    cast(&shard, wolf, 604, wolf);
    assert_eq!(fireball(&shard, 0), ordinary - 1);
    shard.assert_sql("DELETE FROM game_aura WHERE spell_id = 604");
    cast(&shard, wolf, 1008, wolf);
    assert_eq!(fireball(&shard, 0), ordinary + 2);
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_school_crit_changes_a_cast_without_spell_power() {
    let shard = imported_spells(&[133, 5812, 5835, 18382]);
    let ordinary = fireball(&shard, 88);
    assert!(ordinary > 0);
    cast(&shard, 1, 5812, 1);
    assert_eq!(fireball(&shard, 88), ordinary);
    cast(&shard, 1, 5835, 1);
    let crit = fireball(&shard, 88);
    assert_eq!(crit, ordinary * 3 / 2);
    shard.assert_sql("DELETE FROM game_aura WHERE spell_id = 5835");
    import_items(&shard, &[25, 50, 51, 52, 19145]);
    shard.assert_call("debug_grant_item", &["1", "19145", "1"]);
    let item = shard
        .query_rows("SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = 19145");
    shard.assert_call("debug_equip_item", &["1", &item[0]["slot"]]);
    let aura = shard.query_rows(
        "SELECT amount,eff_p0,eff_kind FROM game_aura WHERE target_guid = 1 AND spell_id = 18382",
    );
    assert_eq!(aura[0]["amount"], "2");
    assert_eq!(aura[0]["eff_p0"], "126");
    assert_eq!(aura[0]["eff_kind"], "210");
    assert_eq!(fireball(&shard, 88), crit);
}

fn import_items(shard: &crate::standalone_support::Standalone, entries: &[u32]) {
    let path = std::env::var("LYRACORE_VMANGOS_SQLITE").unwrap();
    let source = WorldSource::open(Path::new(&path)).unwrap();
    let items = build_items_and_loot(
        &source,
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    let templates = items
        .templates
        .into_iter()
        .filter(|row| {
            entries
                .iter()
                .any(|entry| row.starts_with(&format!("({entry},")))
        })
        .collect::<Vec<_>>();
    let mut sql = Vec::new();
    push_item_statements(&mut sql, &templates, &[], &[]);
    for statement in sql {
        shard.assert_sql(&statement);
    }
}

fn equip_weapon(shard: &crate::standalone_support::Standalone, entry: u32) {
    shard.assert_call(
        "debug_learn_spell",
        &["1", if entry == 37 { "196" } else { "201" }],
    );
    let mut rows = shard.query_rows(&format!("SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = {entry} AND slot >= 23"));
    if rows.is_empty() {
        shard.assert_call("debug_grant_item", &["1", &entry.to_string(), "1"]);
        rows = shard.query_rows(&format!("SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = {entry} AND slot >= 23"));
    }
    shard.assert_call("debug_equip_item", &["1", &rows[0]["slot"]]);
}

fn sheet_min(shard: &crate::standalone_support::Standalone) -> u32 {
    shard.query_rows("SELECT sheet_dmg_min FROM game_world_entity WHERE guid = 1")[0]
        ["sheet_dmg_min"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_curse_enrage_and_weapon_requirements_change_weapon_damage() {
    let shard = imported_spells(&[702, 8599, 4334, 196, 201]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    let ordinary = sheet_min(&shard);
    assert!(ordinary > 3);
    cast(&shard, wolf, 702, 1);
    assert_eq!(sheet_min(&shard), ordinary - 3);
    cast(&shard, 1, 8599, 1);
    assert_eq!(sheet_min(&shard), ordinary + 57);
    import_items(&shard, &[25, 37, 50, 51, 52]);
    equip_weapon(&shard, 37);
    let axe = sheet_min(&shard);
    equip_weapon(&shard, 25);
    let sword = sheet_min(&shard);
    cast(&shard, 1, 4334, 1);
    assert_eq!(sheet_min(&shard), sword);
    equip_weapon(&shard, 37);
    assert_eq!(sheet_min(&shard), axe + 1);
    equip_weapon(&shard, 25);
    assert_eq!(sheet_min(&shard), sword);
    shard.assert_sql("DELETE FROM game_aura WHERE target_guid = 1");
    shard.assert_sql(
        "UPDATE game_world_entity SET level = 1, strength = 0, agility = 0 WHERE guid = 1",
    );
    cast(&shard, wolf, 702, 1);
    assert_eq!(sheet_min(&shard), 0);
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_damage_power_does_not_double_healing() {
    let shard = imported_spells(&[2050, 23271, 11390, 604, 1008]);
    let healing = || {
        shard.assert_sql(
            "UPDATE game_conformance_spell_random SET state = 0 WHERE character_guid = 1",
        );
        shard.assert_sql("UPDATE game_world_entity SET health = 100 WHERE guid = 1");
        cast(&shard, 1, 2050, 1);
        let health: u32 = shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1")
            [0]["health"]
            .parse()
            .unwrap();
        health - 100
    };
    let ordinary = healing();
    cast(&shard, 1, 11390, 1);
    assert_eq!(healing(), ordinary);
    cast(&shard, 1, 23271, 1);
    let ephemeral = healing();
    assert!([ordinary + 21, ordinary + 22].contains(&ephemeral));
    cast(&shard, 1, 604, 1);
    let dampened = healing();
    assert!([ephemeral - 2, ephemeral - 3].contains(&dampened));
}

fn damage_values(
    connection: &mut lyracore_test_support::ReducerConnection,
) -> std::collections::BTreeMap<u16, u32> {
    let mut fields = std::collections::BTreeMap::new();
    for packet in connection.drain_packets() {
        let packet: serde_json::Value = serde_json::from_str(packet.get()).unwrap();
        if packet[2][1].as_u64() != Some(0xa9) {
            continue;
        }
        let encoded = packet[3].as_str().unwrap();
        let body: Vec<_> = (0..encoded.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&encoded[i..i + 2], 16).unwrap())
            .collect();
        for update in lyracore_shared::values_mask::parse_values_updates(&body) {
            if update.guid == 1 && update.fields.iter().any(|(index, _)| *index == 1201) {
                assert!(!update.fields.iter().any(|(index, _)| *index == 2));
                fields.extend(update.fields);
            }
        }
    }
    fields
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_sends_signed_school_damage_fields_on_cast_and_removal() {
    use serde_json::json;
    let shard = imported_spells(&[11390, 702, 12880]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([1])).unwrap();
    wire.drain_packets();
    wire.call("debug_begin_cast", json!([1, 11390, 1])).unwrap();
    let fields = damage_values(&mut wire);
    assert_eq!(fields[&1201], 0);
    for school in 1..7 {
        assert_eq!(fields[&(1201 + school)], 20);
    }
    for school in 0..7 {
        assert_eq!(fields[&(1208 + school)], 0);
        assert_eq!(fields[&(1215 + school)], 1.0f32.to_bits());
    }
    wire.call("debug_begin_cast", json!([wolf, 702, 1]))
        .unwrap();
    assert_eq!(damage_values(&mut wire)[&1208], (-3i32) as u32);
    wire.call("debug_advance_roll_batch", json!([31])).unwrap();
    wire.call("debug_begin_cast", json!([1, 12880, 1])).unwrap();
    assert_eq!(damage_values(&mut wire)[&1215], 1.05f32.to_bits());
    wire.call(
        "native_world_receive",
        json!([0x136, 11390u32.to_le_bytes().to_vec()]),
    )
    .unwrap();
    let fields = damage_values(&mut wire);
    assert_eq!(fields[&1203], 0);
    assert_eq!(fields[&1208], (-3i32) as u32);
    assert_eq!(fields[&1215], 1.05f32.to_bits());
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_snapshots_corruption_with_its_dot_coefficient() {
    let shard = imported_spells(&[172, 11390, 17539]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    let corruption = || {
        shard.assert_sql("DELETE FROM game_aura WHERE spell_id = 172");
        shard.assert_sql(
            "UPDATE game_conformance_spell_random SET state = 0 WHERE character_guid = 1",
        );
        shard.assert_call("debug_begin_cast", &["1", "172", &wolf.to_string()]);
        shard.assert_call("debug_advance_roll_batch", &["45"]);
        let auras = shard.query_rows("SELECT amount FROM game_aura WHERE spell_id = 172");
        assert_eq!(auras.len(), 1);
        auras[0]["amount"].parse::<i32>().unwrap()
    };
    let ordinary = corruption();
    cast(&shard, 1, 11390, 1);
    assert_eq!(corruption(), ordinary + 1);
    shard.assert_sql("DELETE FROM game_aura WHERE spell_id = 11390");
    cast(&shard, 1, 17539, 1);
    assert_eq!(corruption(), ordinary + 2);
}

fn seal_damage(connection: &mut lyracore_test_support::ReducerConnection) -> Vec<u32> {
    let mut hits = Vec::new();
    for packet in connection.drain_packets() {
        let packet: serde_json::Value = serde_json::from_str(packet.get()).unwrap();
        if packet[2][1].as_u64() != Some(0x250) {
            continue;
        }
        let encoded = packet[3].as_str().unwrap();
        let body: Vec<_> = (0..encoded.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&encoded[i..i + 2], 16).unwrap())
            .collect();
        let mut offset = 0;
        for _ in 0..2 {
            offset += 1 + body[offset].count_ones() as usize;
        }
        let spell = u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
        if spell == 25742 {
            hits.push(u32::from_le_bytes(
                body[offset + 4..offset + 8].try_into().unwrap(),
            ));
        }
    }
    hits
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_scales_righteousness_with_its_weapon_coefficient() {
    use serde_json::json;
    let shard = imported_spell_fixture(&[21084, 25742, 11390], true);
    import_items(&shard, &[25, 50, 51, 52]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    shard.assert_sql(
        "UPDATE game_player_skill SET current = 300, max_rank = 300 WHERE character_guid = 1",
    );
    shard.assert_sql(
        "INSERT INTO game_conformance_swing_random (character_guid,state) VALUES (1,1)",
    );
    shard.assert_call("debug_repair_after_publish", &[]);
    shard.clear_schedule("game_creature_move_schedule");
    cast(&shard, 1, 21084, 1);
    let auras = shard.query_rows("SELECT eff_kind,amount FROM game_aura WHERE spell_id = 21084");
    assert_eq!(auras.iter().filter(|a| a["eff_kind"] == "167").count(), 1);
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([1])).unwrap();
    wire.drain_packets();
    for bonus in [false, true] {
        if bonus {
            shard.assert_sql("DELETE FROM game_aura WHERE spell_id = 21084 AND effect_id = 84336");
            shard.assert_sql("UPDATE game_aura SET eff_kind = 167 WHERE spell_id = 21084");
            wire.call("debug_repair_after_publish", json!([])).unwrap();
            shard.clear_schedule("game_creature_move_schedule");
            let restored = shard.query_rows(
                "SELECT amount FROM game_aura WHERE spell_id = 21084 AND effect_id = 84336",
            );
            assert_eq!(restored.len(), 1);
            assert_eq!(restored[0]["amount"], "216");
            wire.call("debug_begin_cast", json!([1, 11390, 1])).unwrap();
        }
        shard.assert_sql(
            "UPDATE game_conformance_swing_random SET state = 1 WHERE character_guid = 1",
        );
        shard.assert_sql(
            "UPDATE game_conformance_spell_random SET state = 0 WHERE character_guid = 1",
        );
        wire.call(
            "native_world_receive",
            json!([0x141, wolf.to_le_bytes().to_vec()]),
        )
        .unwrap();
        wire.call("debug_advance_roll_batch", json!([45])).unwrap();
        wire.call("native_world_receive", json!([0x142, Vec::<u8>::new()]))
            .unwrap();
        let hits = seal_damage(&mut wire);
        assert!(!hits.is_empty());
        let expected = if bonus { [5, 6] } else { [3, 4] };
        assert!(hits.iter().all(|hit| expected.contains(hit)), "{hits:?}");
    }
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_scales_flamestrike_ground_ticks() {
    let shard = imported_spells(&[2120, 23271]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = 14 WHERE guid = {wolf}"
    ));
    for bonus in [false, true] {
        shard.assert_sql("DELETE FROM game_ground_area");
        if bonus {
            cast(&shard, 1, 23271, 1);
        }
        shard.assert_sql(
            "UPDATE game_conformance_spell_random SET state = 0 WHERE character_guid = 1",
        );
        cast(&shard, 1, 2120, wolf);
        let areas = shard.query_rows("SELECT amount FROM game_ground_area WHERE spell_id = 2120");
        assert_eq!(areas.len(), 1);
        let expected = if bonus { 14 } else { 12 };
        assert_eq!(areas[0]["amount"].parse::<u32>().unwrap(), expected);
        let health = || {
            shard.query_rows(&format!(
                "SELECT health FROM game_world_entity WHERE guid = {wolf}"
            ))[0]["health"]
                .parse::<u32>()
                .unwrap()
        };
        let before = health();
        shard.assert_call("debug_advance_roll_batch", &["45"]);
        assert_eq!(before - health(), expected);
    }
}

#[test]
#[ignore = "requires World Source, client DBCs, SpacetimeDB 2.14.0 and Wasm"]
fn actual_spell_power_world_plan_scales_eviscerate_as_nonweapon_damage() {
    let shard = imported_spells(&[2098, 702, 8599]);
    let wolf = (0xf130u64 << 48) | (51000u64 << 24) | 1;
    shard.assert_sql(
        "INSERT INTO game_conformance_swing_random (character_guid,state) VALUES (1,1)",
    );
    let eviscerate = || {
        shard.assert_sql("DELETE FROM game_combo_point WHERE owner_guid = 1");
        shard.assert_sql(&format!(
            "INSERT INTO game_combo_point (id,owner_guid,target_guid,points) VALUES (0,1,{wolf},1)"
        ));
        shard.assert_sql(
            "UPDATE game_conformance_swing_random SET state = 1 WHERE character_guid = 1",
        );
        shard.assert_sql(
            "UPDATE game_conformance_spell_random SET state = 0 WHERE character_guid = 1",
        );
        shard.assert_sql("UPDATE game_world_entity SET power = 100000 WHERE guid = 1");
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET health = 100000 WHERE guid = {wolf}"
        ));
        cast(&shard, 1, 2098, wolf);
        let health: u32 = shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {wolf}"
        ))[0]["health"]
            .parse()
            .unwrap();
        100000 - health
    };
    let ordinary = eviscerate();
    assert!(ordinary > 3);
    cast(&shard, wolf, 702, 1);
    assert_eq!(eviscerate(), ordinary - 3);
    cast(&shard, 1, 8599, 1);
    assert_eq!(eviscerate(), ordinary + 57);
}
