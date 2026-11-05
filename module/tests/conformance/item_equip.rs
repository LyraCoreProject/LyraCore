use super::buy_in_slot::{buy, grant, inventory, request, stage};
use super::enter;
use lyracore_test_support::{ReducerConnection, Standalone};
use wow_world_messages::vanilla::{
    ClientMessage, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags, CMSG_USE_ITEM,
};

const ITEM: u32 = 5090050;
const EQUIP: u32 = 5090900;
const USE: u32 = 5090901;

fn fixture(name: &str) -> (Standalone, ReducerConnection, u64, u64) {
    let (shard, connection, guid, vendor) = stage(name);
    configure(&shard);
    (shard, connection, guid, vendor)
}

pub(super) fn configure(shard: &Standalone) {
    for (spell, kind, points, attributes) in [(EQUIP, 0xa0, 7, 64), (USE, 0xa2, 50, 0)] {
        shard.assert_sql(&format!(
            "INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES ({spell},'Equip fixture {spell}',0,0,0,0,0,0,1000,1,1,0,5,0,{attributes},1,0,false,0,0,0,0,0,0,0,0)"
        ));
        shard.assert_sql(&format!(
            "INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({},{spell},0,{kind},{points},0,0.0,0,0,0.0,0,0,0,2,1,0,0,false,0.0)",
            u64::from(spell) * 4
        ));
    }
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 11, required_level = 1, max_stack = 1, bonding = 0, stat_stamina = 0, spellid_1 = {EQUIP}, spelltrigger_1 = 1, spellid_2 = 0, spellid_3 = 0, spellid_4 = 0, spellid_5 = {USE}, spelltrigger_5 = 0 WHERE entry = {ITEM}"
    ));
}

fn auras(shard: &Standalone, guid: u64) -> Vec<std::collections::BTreeMap<String, String>> {
    shard.query_rows(&format!(
        "SELECT * FROM game_aura WHERE target_guid = {guid} AND spell_id = {EQUIP}"
    ))
}

pub(super) fn assert_effect(shard: &Standalone, guid: u64, count: usize) {
    let rows = auras(shard, guid);
    assert_eq!(rows.len(), count, "{rows:?}");
    for row in rows {
        assert_eq!(row["amount"], "7");
        assert_eq!(row["stacks"], "1");
        assert_eq!(row["slot"], "255");
    }
    let sheet = shard.query_rows(&format!(
        "SELECT sheet_sta_bonus FROM game_world_entity WHERE guid = {guid}"
    ));
    assert_eq!(sheet[0]["sheet_sta_bonus"], (count * 7).to_string());
}

fn move_item(connection: &mut ReducerConnection, from: u8, to: u8) -> Vec<(u16, Vec<u8>)> {
    request(connection, 0x10c, vec![255, to, 255, from])
}

fn equip(connection: &mut ReducerConnection, from: u8) -> Vec<(u16, Vec<u8>)> {
    request(connection, 0x10a, vec![255, from])
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equip_moves_swaps_and_shared_spells_keep_each_items_effect_once() {
    let (shard, mut connection, guid, _) = fixture("equip-shared-spell");
    let first = grant(&shard, guid, ITEM, 1, 23);
    let second = grant(&shard, guid, ITEM, 1, 24);
    let third = grant(&shard, guid, ITEM, 1, 25);
    equip(&mut connection, 23);
    assert_effect(&shard, guid, 1);
    equip(&mut connection, 24);
    assert_effect(&shard, guid, 2);
    move_item(&mut connection, 25, 10);
    assert_effect(&shard, guid, 2);
    let sources = shard.query_rows("SELECT item_guid FROM game_item_aura");
    assert!(sources
        .iter()
        .any(|row| row["item_guid"] == third.to_string()));
    assert!(sources
        .iter()
        .any(|row| row["item_guid"] == second.to_string()));
    assert!(!sources
        .iter()
        .any(|row| row["item_guid"] == first.to_string()));
    move_item(&mut connection, 10, 11);
    assert_effect(&shard, guid, 2);
    move_item(&mut connection, 11, 26);
    assert_effect(&shard, guid, 1);
    move_item(&mut connection, 10, 27);
    assert_effect(&shard, guid, 0);
    assert!(shard.query_rows("SELECT * FROM game_item_aura").is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equip_cooldown_packets_gate_zero_cooldown_use_and_preserve_longer_delays() {
    let (shard, mut connection, guid, vendor) = fixture("equip-cooldown");
    let item = grant(&shard, guid, ITEM, 1, 23);
    let packets = equip(&mut connection, 23);
    let mut expected = item.to_le_bytes().to_vec();
    expected.extend(USE.to_le_bytes());
    assert_eq!(
        packets
            .iter()
            .filter(|(opcode, _)| *opcode == 0xb0)
            .collect::<Vec<_>>(),
        vec![&(0xb0, expected)]
    );
    let before = inventory(&shard, guid);
    let mut frame = Vec::new();
    CMSG_USE_ITEM {
        bag_index: 255,
        bag_slot: 10,
        spell_index: 4,
        targets: SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::default(),
        },
    }
    .write_unencrypted_client(&mut std::io::Cursor::new(&mut frame))
    .unwrap();
    let packets = request(&mut connection, 0xab, frame[6..].to_vec());
    assert_eq!(
        packets,
        vec![(
            0x130,
            lyracore_protocol::codec::build_cast_result_failed(USE, 0x3c)
        )]
    );
    assert_eq!(inventory(&shard, guid), before);
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    let refused = shard.call("gw_use_item", &[&actor, "10"]);
    assert!(
        !refused.status.success(),
        "use during equip cooldown succeeded"
    );
    assert_eq!(inventory(&shard, guid), before);
    shard.assert_call("debug_advance_time", &["29999"]);
    assert!(!shard.call("gw_use_item", &[&actor, "10"]).status.success());
    shard.assert_call("debug_advance_time", &["1"]);
    shard.assert_call("gw_use_item", &[&actor, "10"]);

    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cd WHERE caster_guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET item_flags = 128 WHERE entry = {ITEM}"
    ));
    let packets = buy(&mut connection, vendor, ITEM, guid, 10, 1);
    assert!(!packets.iter().any(|(opcode, _)| *opcode == 0xb0));
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_spell_cd WHERE caster_guid = {guid}"
        ))
        .is_empty());
    move_item(&mut connection, 10, 23);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET item_flags = 0 WHERE entry = {ITEM}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cooldown_ms = 120000 WHERE spell_id = {USE}"
    ));
    shard.assert_call(
        "debug_begin_cast",
        &[&guid.to_string(), &USE.to_string(), &guid.to_string()],
    );
    let before = shard.query_rows(&format!(
        "SELECT * FROM game_spell_cd WHERE caster_guid = {guid}"
    ));
    equip(&mut connection, 23);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_spell_cd WHERE caster_guid = {guid}"
        )),
        before
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn passive_equip_auras_are_hidden_permanent_and_not_cancellable_or_dispellable() {
    let (shard, mut connection, guid, _) = fixture("equip-passive");
    shard.assert_call(
        "debug_fill_aura_slots",
        &[&guid.to_string(), &guid.to_string(), "false", "32"],
    );
    grant(&shard, guid, ITEM, 1, 23);
    let packets = equip(&mut connection, 23);
    assert_effect(&shard, guid, 1);
    assert!(packets
        .iter()
        .flat_map(|(opcode, body)| super::properties::updates(*opcode, body).unwrap())
        .filter(|update| update.guid == guid)
        .flat_map(|update| update.fields)
        .all(|(_, value)| value != EQUIP));
    assert!(!packets.iter().any(|(opcode, _)| *opcode == 0x137));
    let original = auras(&shard, guid);
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    let packets = request(&mut connection, 0x136, EQUIP.to_le_bytes().to_vec());
    assert!(!packets.iter().any(|(opcode, _)| *opcode == 0x137));
    assert_eq!(auras(&shard, guid), original);
    shard.assert_call("debug_advance_time", &["120000"]);
    assert_eq!(auras(&shard, guid), original);
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET kind = 4, p0 = 1 WHERE spell_id = {USE}"
    ));
    shard.assert_call("debug_learn_spell", &[&guid.to_string(), &USE.to_string()]);
    shard.assert_call(
        "debug_cast_spell",
        &[&guid.to_string(), &USE.to_string(), &guid.to_string()],
    );
    assert_effect(&shard, guid, 1);
    assert!(!shard
        .call("gw_cancel_aura", &[&actor, &EQUIP.to_string()])
        .status
        .success());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn sale_buyback_destroy_and_equipment_purchase_share_equip_effects() {
    let (shard, mut connection, guid, vendor) = fixture("equip-economy");
    let packets = buy(&mut connection, vendor, ITEM, guid, 10, 1);
    assert!(packets.iter().any(|(opcode, _)| *opcode == 0xb0));
    assert_effect(&shard, guid, 1);
    let item: u64 = inventory(&shard, guid)[0]["guid"].parse().unwrap();
    let mut sale = vendor.to_le_bytes().to_vec();
    sale.extend(item.to_le_bytes());
    sale.push(1);
    request(&mut connection, 0x1a0, sale);
    assert_effect(&shard, guid, 0);
    let mut restore = vendor.to_le_bytes().to_vec();
    restore.extend(69u32.to_le_bytes());
    request(&mut connection, 0x290, restore);
    assert_effect(&shard, guid, 0);
    let slot = inventory(&shard, guid)
        .iter()
        .find(|row| row["guid"] == item.to_string())
        .unwrap()["slot"]
        .parse()
        .unwrap();
    equip(&mut connection, slot);
    assert_effect(&shard, guid, 1);
    request(&mut connection, 0x111, vec![255, 10, 1, 0, 0, 0]);
    assert_effect(&shard, guid, 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn broken_equipment_loses_auras_and_repair_restores_them_without_a_cooldown() {
    let (shard, mut connection, guid, _) = fixture("equip-durability");
    let item = grant(&shard, guid, ITEM, 1, 23);
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET durability = 1 WHERE guid = {item}"
    ));
    equip(&mut connection, 23);
    assert_effect(&shard, guid, 1);
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cd WHERE caster_guid = {guid}"
    ));
    shard.assert_call("debug_set_health", &[&guid.to_string(), "0"]);
    assert_effect(&shard, guid, 0);
    shard.assert_call("debug_set_health", &[&guid.to_string(), "1"]);
    shard.assert_call("debug_repair_item", &[&guid.to_string(), "10"]);
    assert_effect(&shard, guid, 1);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_spell_cd WHERE caster_guid = {guid}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn login_reconnect_and_resurrection_rebuild_equip_auras_once_without_a_new_cooldown() {
    let (shard, mut connection, guid, _) = fixture("equip-lifecycle");
    grant(&shard, guid, ITEM, 1, 23);
    equip(&mut connection, 23);
    assert_effect(&shard, guid, 1);
    drop(connection);
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    assert_effect(&shard, guid, 1);
    let mut connection = enter(&shard, guid);
    assert_effect(&shard, guid, 1);
    connection.drain_packets();
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    assert_effect(&shard, guid, 1);
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cd WHERE caster_guid = {guid}"
    ));
    shard.assert_call("debug_set_health", &[&guid.to_string(), "0"]);
    shard.assert_call("debug_set_health", &[&guid.to_string(), "1"]);
    assert_effect(&shard, guid, 1);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_spell_cd WHERE caster_guid = {guid}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equip_spells_follow_form_exclusions_requirements_and_unshifted_exemption() {
    let (shard, mut connection, guid, _) = fixture("equip-forms");
    shard.assert_call("debug_learn_spell", &[&guid.to_string(), "768"]);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET form_mask = 1 WHERE spell_id = {EQUIP}"
    ));
    grant(&shard, guid, ITEM, 1, 23);
    equip(&mut connection, 23);
    assert_effect(&shard, guid, 0);
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cooldown WHERE caster_guid = {guid}"
    ));
    shard.assert_call(
        "debug_cast_spell",
        &[&guid.to_string(), "768", &guid.to_string()],
    );
    assert_effect(&shard, guid, 1);
    let original = auras(&shard, guid);
    move_item(&mut connection, 10, 11);
    assert_eq!(auras(&shard, guid), original);
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cooldown WHERE caster_guid = {guid}"
    ));
    shard.assert_call(
        "debug_cast_spell",
        &[&guid.to_string(), "768", &guid.to_string()],
    );
    assert_effect(&shard, guid, 0);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET attributes_ex2 = 524288 WHERE spell_id = {EQUIP}"
    ));
    move_item(&mut connection, 11, 10);
    assert_effect(&shard, guid, 1);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET form_exclude = 1 WHERE spell_id = {EQUIP}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cooldown WHERE caster_guid = {guid}"
    ));
    shard.assert_call(
        "debug_cast_spell",
        &[&guid.to_string(), "768", &guid.to_string()],
    );
    assert_effect(&shard, guid, 0);
}

fn partner(shard: &Standalone) -> u64 {
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]
        ["id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Equipfriend\"",
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let guid: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Equipfriend'")
        [0]["guid"]
        .parse()
        .unwrap();
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call("debug_set_money", &[&guid.to_string(), "1000"]);
    guid
}

fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn trade_and_mail_keep_effects_with_equipment_and_not_with_carried_items() {
    let (shard, mut connection, guid, _) = fixture("equip-trade-mail");
    let other = partner(&shard);
    let item = grant(&shard, guid, ITEM, 1, 23);
    equip(&mut connection, 23);
    shard.assert_call("gw_initiate_trade", &[&actor(guid), &other.to_string()]);
    shard.assert_call("gw_begin_trade", &[&actor(other)]);
    assert!(!shard
        .call("gw_set_trade_item", &[&actor(guid), "0", "10"])
        .status
        .success());
    assert_effect(&shard, guid, 1);
    move_item(&mut connection, 10, 23);
    assert_effect(&shard, guid, 0);
    shard.assert_call("gw_set_trade_item", &[&actor(guid), "0", "23"]);
    shard.assert_call("gw_accept_trade", &[&actor(guid)]);
    shard.assert_call("gw_accept_trade", &[&actor(other)]);
    assert_effect(&shard, guid, 0);
    assert_effect(&shard, other, 0);
    let slot = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE guid = {item}"
    ))[0]["slot"]
        .clone();
    shard.assert_call("debug_equip_item", &[&other.to_string(), &slot]);
    assert_effect(&shard, other, 1);
    shard.assert_call("debug_unequip_item", &[&other.to_string(), "10"]);
    assert_effect(&shard, other, 0);
    shard.assert_call(
        "realm_mail_send",
        &[
            &actor(other),
            &guid.to_string(),
            "\"Equip return\"",
            "\"\"",
            "0",
            "0",
            &item.to_string(),
            "true",
        ],
    );
    assert_effect(&shard, other, 0);
    let mail = shard.query_rows(&format!(
        "SELECT id FROM game_mail WHERE recipient_guid = {guid} AND subject = 'Equip return'"
    ))[0]["id"]
        .clone();
    shard.assert_call("realm_mail_take_item", &[&actor(guid), &mail]);
    assert_effect(&shard, guid, 0);
    let slot = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = {ITEM}"
    ))[0]["slot"]
        .clone();
    shard.assert_call("debug_equip_item", &[&guid.to_string(), &slot]);
    assert_effect(&shard, guid, 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn visible_equip_effects_keep_the_sources_duration_and_reapply_after_resurrection() {
    let (shard, mut connection, guid, _) = fixture("equip-visible-duration");
    shard.assert_sql(&format!("UPDATE game_spell SET attributes = 0, visual_id = 3580, duration_index = 1, duration_ms = 5000 WHERE spell_id = {EQUIP}"));
    grant(&shard, guid, ITEM, 1, 23);
    let packets = equip(&mut connection, 23);
    let rows = auras(&shard, guid);
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["slot"].parse::<u8>().unwrap() < 32);
    assert!(packets.iter().any(|(opcode, _)| *opcode == 0x137));
    shard.assert_call("debug_set_health", &[&guid.to_string(), "0"]);
    assert!(auras(&shard, guid).is_empty());
    shard.assert_call("debug_set_health", &[&guid.to_string(), "1"]);
    assert_eq!(auras(&shard, guid).len(), 1);
    shard.assert_call("debug_advance_time", &["5000"]);
    assert!(auras(&shard, guid).is_empty());
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT sheet_sta_bonus FROM game_world_entity WHERE guid = {guid}"
        ))[0]["sheet_sta_bonus"],
        "0"
    );
}
