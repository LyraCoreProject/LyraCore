use super::aura_test_support::Character;
use serde_json::json;

impl Character {
    fn equip_staff(&mut self) {
        self.shard.assert_sql("UPDATE game_item_template SET class=2,subclass=10,inventory_type=17,damage_min=10,damage_max=10,delay_ms=2000,spellid_1=0,spellid_2=0,spellid_3=0,spellid_4=0,spellid_5=0 WHERE entry=5090050");
        self.shard
            .assert_call("debug_set_skill", &[&self.guid.to_string(), "136", "300"]);
        self.shard.assert_call(
            "debug_grant_item",
            &[&self.guid.to_string(), "5090050", "1"],
        );
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot=23 WHERE owner_guid={} AND entry=5090050",
            self.guid
        ));
        self.request(0x10a, vec![255, 23]);
        assert_eq!(
            self.shard.query_rows(&format!(
                "SELECT slot FROM game_item_instance WHERE owner_guid={} AND entry=5090050",
                self.guid
            ))[0]["slot"],
            "15"
        );
        self.shard.assert_sql(&format!("UPDATE game_world_entity SET health=1000000000,max_health=1000000000,power=10000,max_power=10000 WHERE guid={}",self.guid));
        self.shard
            .assert_call("debug_set_health", &[&self.guid.to_string(), "1000000000"]);
    }

    fn cast_at(&mut self, spell: u32, target: u64) {
        self.shard.assert_call(
            "debug_learn_spell",
            &[&self.guid.to_string(), &spell.to_string()],
        );
        let mut body = spell.to_le_bytes().to_vec();
        body.extend(2u16.to_le_bytes());
        let bytes = target.to_le_bytes();
        let mask = bytes.iter().enumerate().fold(0u8, |mask, (index, byte)| {
            mask | (u8::from(*byte != 0) << index)
        });
        body.push(mask);
        body.extend(bytes.into_iter().filter(|byte| *byte != 0));
        self.request(0x12e, body);
    }

    fn packets(&mut self, opcode: u16) -> Vec<Vec<u8>> {
        self.connection
            .call("native_world_receive", json!([0x1dc, vec![0u8; 8]]))
            .unwrap();
        self.connection
            .drain_packets()
            .into_iter()
            .filter_map(|row| {
                let row: serde_json::Value = serde_json::from_str(row.get()).unwrap();
                (row[2][0] == 0 && row[2][1] == opcode).then(|| {
                    let body = row[3].as_str().unwrap();
                    (0..body.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&body[i..i + 2], 16).unwrap())
                        .collect()
                })
            })
            .collect()
    }
}

#[test]
#[ignore = "requires local World Source, stock client MPQs, SpacetimeDB 2.14.0 and Wasm"]
fn imported_thorns_reflects_melee_with_its_school_packet_and_bearer_credit() {
    let mut character =
        Character::imported("imported-thorns-consumer", &[467, 15438, 16560, 11390, 17]);
    character.cast(467);
    character.advance(1);
    character.cast(11390);
    character.advance(1);
    character.cast(16560);
    character.equip(15438);
    character.set_pools();
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET max_health=1000000,health=1000000 WHERE guid={}",
        character.guid
    ));
    character.shard.assert_call(
        "debug_set_health",
        &[&character.guid.to_string(), "1000000"],
    );
    character
        .shard
        .assert_call("debug_seed_scenario_fixtures", &[]);
    character.shard.assert_call(
        "debug_spawn_at_feet",
        &[&character.guid.to_string(), "51000", "2"],
    );
    let wolf: u64 = character
        .shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry=51000")
        .iter()
        .map(|row| row["guid"].parse().unwrap())
        .max()
        .unwrap();
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level=60,health=10000,max_health=10000,faction_template=1 WHERE guid={wolf}"
    ));
    character.advance(1);
    character.cast_at(17, wolf);
    let capacity = character.shard.query_rows(&format!(
        "SELECT amount_remaining FROM game_aura WHERE target_guid={wolf} AND spell_id=17"
    ))[0]["amount_remaining"]
        .clone();
    assert!(capacity.parse::<u32>().unwrap() > 0);
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template=14 WHERE guid={wolf}"
    ));
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET base_attack_time_ms=600000 WHERE guid={}",
        character.guid
    ));
    character.connection.drain_packets();
    character.request(0x141, wolf.to_le_bytes().to_vec());
    let now_micros: u64 = character
        .shard
        .query_rows("SELECT now_micros FROM game_conformance_clock")[0]["now_micros"]
        .parse()
        .unwrap();
    character.shard.assert_sql(&format!(
        "UPDATE game_melee_attack SET last_swing_ms={} WHERE attacker_guid={}",
        (now_micros / 1000) as u32,
        character.guid
    ));
    character.advance(10);
    let packets = character.packets(0x24f);
    assert!(packets.len() >= 2, "landed wolf swings must reflect damage: packets={packets:?}; melee={:?}; units={:?}; aura={:?}",character.shard.query_rows("SELECT * FROM game_melee_attack"),character.shard.query_rows(&format!("SELECT guid,health,dead,x,y FROM game_world_entity WHERE guid={wolf} OR guid={}",character.guid)), character.shard.query_rows(&format!("SELECT spell_id,eff_kind,amount FROM game_aura WHERE target_guid={}",character.guid)));
    for body in &packets {
        assert_eq!(body.len(), 24);
        assert_eq!(
            u64::from_le_bytes(body[0..8].try_into().unwrap()),
            character.guid
        );
        assert_eq!(u64::from_le_bytes(body[8..16].try_into().unwrap()), wolf);
        let school = u32::from_le_bytes(body[20..24].try_into().unwrap());
        assert!(matches!(school, 2 | 3 | 6));
        assert_eq!(
            u32::from_le_bytes(body[16..20].try_into().unwrap()),
            if school == 2 { 30 } else { 3 }
        );
    }
    assert!(
        packets
            .iter()
            .any(|body| body[20..24] == 6u32.to_le_bytes()),
        "the equipped shield reflects Arcane damage"
    );
    assert!(
        packets
            .iter()
            .any(|body| body[20..24] == 3u32.to_le_bytes()),
        "the cast shield reflects Nature damage"
    );
    assert!(
        packets
            .iter()
            .any(|body| body[20..24] == 2u32.to_le_bytes()),
        "Flame Wrath adds the bearer's 20 spell power to its 10 Fire damage"
    );
    assert_eq!(
        character.shard.query_rows(&format!(
            "SELECT amount_remaining FROM game_aura WHERE target_guid={wolf} AND spell_id=17"
        ))[0]["amount_remaining"],
        capacity,
        "reflection bypasses absorb capacity"
    );
    let aura = character.shard.query_rows(&format!(
        "SELECT amount_remaining FROM game_aura WHERE target_guid={} AND spell_id=467",
        character.guid
    ));
    assert_eq!(aura.len(), 1);
    assert_eq!(aura[0]["amount_remaining"], "0");
    let before: u32 = character.shard.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid={wolf}"
    ))[0]["health"]
        .parse()
        .unwrap();
    character.advance(5);
    let packets = character.packets(0x24f);
    assert!(!packets.is_empty());
    let after: u32 = character.shard.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid={wolf}"
    ))[0]["health"]
        .parse()
        .unwrap();
    assert_eq!(before - after, packets.len() as u32 * 3);
    assert!(!character
        .shard
        .query_rows(&format!(
            "SELECT * FROM game_threat WHERE creature_guid={wolf} AND source_guid={}",
            character.guid
        ))
        .is_empty());
}

#[test]
#[ignore = "requires local World Source, stock client MPQs, SpacetimeDB 2.14.0 and Wasm"]
fn imported_shields_do_not_recurse_and_can_kill_the_attacker() {
    let mut character = Character::imported(
        "imported-shield-death",
        &[467, 15279, 11976, 22572, 642, 1044, 77, 133],
    );
    character
        .shard
        .assert_call("debug_seed_scenario_fixtures", &[]);
    character.shard.assert_call(
        "debug_spawn_at_feet",
        &[&character.guid.to_string(), "51000", "2"],
    );
    let target: u64 = character
        .shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry=51000")
        .iter()
        .map(|row| row["guid"].parse().unwrap())
        .max()
        .unwrap();
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET entry=38,faction_template=1 WHERE guid={target}"
    ));
    character.shard.assert_call(
        "debug_prepare_roll_character",
        &[
            &character.guid.to_string(),
            &target.to_string(),
            "60",
            "1",
            "1",
        ],
    );
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET base_attack_time_ms=2000 WHERE guid={}",
        character.guid
    ));
    if character
        .shard
        .query_rows(&format!(
            "SELECT * FROM game_creature_ai_state WHERE creature_guid={target}"
        ))
        .is_empty()
    {
        character.shard.assert_sql(&format!("INSERT INTO game_creature_ai_state (creature_guid,phase,lifecycle_id,engagement_id,ranged_distance,ranged_angle,ranged_posture_active,definition_revision,active_object,react_state,melee_enabled) VALUES ({target},0,0,1,0,0,false,0,false,0,false)"));
    } else {
        character.shard.assert_sql(&format!(
            "UPDATE game_creature_ai_state SET react_state=0,melee_enabled=false WHERE creature_guid={target}"
        ));
    }
    character.equip_staff();
    character.cast(467);
    character.advance(1);
    character.cast_at(467, target);
    assert_eq!(
        character
            .shard
            .query_rows(&format!(
                "SELECT * FROM game_aura WHERE target_guid={target} AND spell_id=467"
            ))
            .len(),
        1
    );
    character.advance(1);
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template=14 WHERE guid={target}"
    ));
    character.connection.drain_packets();
    let before = character.pool("health");
    character.request(0x141, target.to_le_bytes().to_vec());
    character.advance(3);
    character.request(0x142, vec![]);
    let packets = character.packets(0x24f);
    assert!(!packets.is_empty(), "white hits on the shield must reflect: melee={:?}; units={:?}; auras={:?}; ai={:?}",character.shard.query_rows("SELECT * FROM game_melee_attack"),character.shard.query_rows(&format!("SELECT guid,health,dead,unit_flags,x,y,orientation FROM game_world_entity WHERE guid={target} OR guid={}",character.guid)),character.shard.query_rows(&format!("SELECT spell_id,eff_kind,amount FROM game_aura WHERE target_guid={target}")),character.shard.query_rows(&format!("SELECT * FROM game_creature_ai_state WHERE creature_guid={target}")));
    assert!(
        packets
            .iter()
            .all(|body| u64::from_le_bytes(body[..8].try_into().unwrap()) == target),
        "reflection must not trigger the attacker's shield"
    );
    assert_eq!(before - character.pool("health"), 3 * packets.len() as u32);
    character.advance(1);
    character.shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES ({},{target},1)",character.guid));
    let before: u32 = character.shard.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid={target}"
    ))[0]["health"]
        .parse()
        .unwrap();
    character.connection.drain_packets();
    character.cast_at(133, target);
    character.advance(2);
    let after: u32 = character.shard.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid={target}"
    ))[0]["health"]
        .parse()
        .unwrap();
    assert!(after < before, "the imported Fireball must land");
    assert!(
        character.packets(0x24f).is_empty(),
        "ordinary spell damage does not trigger a shield"
    );
    character.cast(1044);
    assert!(
        character
            .shard
            .query_rows(&format!(
                "SELECT * FROM game_aura WHERE target_guid={} AND spell_id=77",
                character.guid
            ))
            .is_empty(),
        "immunity metadata must not apply Turtle's Shell as a linked spell"
    );
    character.advance(1);
    character.connection.drain_packets();
    character.shard.assert_sql(&format!(
        "UPDATE game_conformance_swing_random SET state=1 WHERE character_guid={}",
        character.guid
    ));
    character.cast_at(22572, target);
    let strike = character.packets(0x24f);
    assert_eq!(
        strike.len(),
        1,
        "a landed weapon-proc melee spell triggers the shield: spell={:?}; target={:?}; casts={:?}", character.shard.query_rows("SELECT cast_flags,cost,power_type,cast_time_ms,range_yd FROM game_spell WHERE spell_id=22572"),character.shard.query_rows(&format!("SELECT health,dead,x,y FROM game_world_entity WHERE guid={target}")),character.shard.query_rows("SELECT * FROM game_pending_cast")
    );
    character.advance(1);
    character.connection.drain_packets();
    character.shard.assert_sql(&format!(
        "UPDATE game_conformance_swing_random SET state=1 WHERE character_guid={}",
        character.guid
    ));
    character.request(0x141, target.to_le_bytes().to_vec());
    character.cast_at(11976, target);
    character
        .shard
        .assert_call("debug_advance_roll_batch", &["2"]);
    character.request(0x142, vec![]);
    assert_eq!(
        character.packets(0x24f).len(),
        1,
        "a landed queued Strike triggers the shield once"
    );
    character.advance(1);
    character.cast(642);
    character.advance(1);
    let before = character.pool("health");
    character.connection.drain_packets();
    character.shard.assert_sql(&format!(
        "UPDATE game_conformance_swing_random SET state=1 WHERE character_guid={}",
        character.guid
    ));
    character.cast_at(22572, target);
    assert_eq!(
        character.pool("health"),
        before,
        "Divine Shield prevents reflected Nature damage"
    );
    let packets = character.packets(0x263);
    assert_eq!(
        packets.len(),
        1,
        "positive shield against school immunity sends the source immunity packet"
    );
    character.advance(6);
    character.advance(1);
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template=1 WHERE guid={target}"
    ));
    character.cast_at(15279, target);
    character.advance(1);
    assert_eq!(
        character
            .shard
            .query_rows(&format!(
                "SELECT * FROM game_aura WHERE target_guid={target} AND spell_id=15279"
            ))
            .len(),
        1
    );
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template=14 WHERE guid={target}"
    ));
    character
        .shard
        .assert_call("debug_set_health", &[&character.guid.to_string(), "3"]);
    character.connection.drain_packets();
    character.shard.assert_sql(&format!(
        "UPDATE game_conformance_swing_random SET state=1 WHERE character_guid={}",
        character.guid
    ));
    character.request(0x141, target.to_le_bytes().to_vec());
    character.advance(3);
    let packets = character.packets(0x24f);
    assert_eq!(
        character.pool("health"),
        0,
        "reflection may kill a Character attacker"
    );
    assert_eq!(
        packets.len(),
        2,
        "both shields report once, even when the first kills"
    );
    assert_eq!(
        u32::from_le_bytes(packets[0][16..20].try_into().unwrap()),
        3
    );
    assert_eq!(
        u32::from_le_bytes(packets[1][16..20].try_into().unwrap()),
        0,
        "a dead attacker takes zero from the remaining shield"
    );
}
