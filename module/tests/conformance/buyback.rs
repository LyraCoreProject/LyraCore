use lyracore_test_support::{Field, ReducerConnection, Standalone};
use serde_json::json;

use super::buy_in_slot::{delivered, money, request};
use super::{enter, fixture, Packet};
use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, SCENARIO_VENDOR_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;

#[path = "buyback_handoff.rs"]
mod handoff;

fn stage(name: &str) -> (Standalone, ReducerConnection, u64, u64) {
    stage_in(fixture(name))
}

fn stage_in(
    (shard, guid, _): (Standalone, u64, String),
) -> (Standalone, ReducerConnection, u64, u64) {
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET sell_price = 13 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    let vendor = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}"
    ));
    let mut connection = enter(&shard, guid);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    connection.drain_packets();
    (shard, connection, guid, vendor)
}

fn grant(shard: &Standalone, guid: u64, entry: u32, count: u32) -> u64 {
    shard.assert_call(
        "debug_grant_item",
        &[&guid.to_string(), &entry.to_string(), &count.to_string()],
    );
    shard
        .query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
        ))
        .iter()
        .map(|row| row["guid"].parse().unwrap())
        .max()
        .unwrap()
}

fn values(packets: &[(u16, Vec<u8>)], guid: u64) -> std::collections::BTreeMap<u16, u32> {
    packets
        .iter()
        .flat_map(|(opcode, body)| super::properties::updates(*opcode, body).unwrap())
        .filter(|update| update.guid == guid)
        .flat_map(|update| update.fields)
        .collect()
}

fn sold_rows(shard: &Standalone, guid: u64) -> Vec<std::collections::BTreeMap<String, String>> {
    shard.query_rows(&format!(
        "SELECT * FROM game_buyback_item WHERE character_guid = {guid}"
    ))
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_partial_sale_clones_and_buyback_merges_without_an_item_receipt() {
    let (shard, mut connection, guid, vendor) = stage("buyback-partial");
    let item = grant(&shard, guid, FIXTURE_JERKY, 5);
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET enchant_id = 2564, soulbound = true WHERE guid = {item}"
    ));
    let sold = sell(&mut connection, vendor, item, 2);
    let rows = sold_rows(&shard, guid);
    assert_eq!(rows.len(), 1);
    let clone: u64 = rows[0]["item_guid"].parse().unwrap();
    assert_ne!(clone, item);
    assert_eq!(rows[0]["price"], "4");
    assert_eq!(values(&sold, item)[&14], 3);
    assert_eq!(values(&sold, clone).get(&22).copied().unwrap_or(0), 0);
    assert_eq!(values(&sold, clone)[&21], 1);
    let returned = buyback(&mut connection, vendor, 69);
    let inventory = shard.query_rows(&format!(
        "SELECT guid,stack_count,enchant_id FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0]["guid"], item.to_string());
    assert_eq!(inventory[0]["stack_count"], "5");
    assert_eq!(inventory[0]["enchant_id"], "2564");
    assert!(returned
        .iter()
        .any(|(opcode, body)| *opcode == 0xaa && body == &clone.to_le_bytes()));
    assert!(returned.iter().all(|(opcode, _)| *opcode != 0x166));
    assert!(sold_rows(&shard, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn ring_slots_timestamps_and_full_ring_eviction_follow_the_reference() {
    let (shard, mut connection, guid, vendor) = stage("buyback-ring");
    let mut items = Vec::new();
    for _ in 0..12 {
        let item = grant(&shard, guid, FIXTURE_BLADE, 1);
        items.push(item);
        sell(&mut connection, vendor, item, 0);
    }
    let rows = sold_rows(&shard, guid);
    assert_eq!(rows.len(), 12);
    for (index, item) in items.iter().enumerate() {
        let row = rows
            .iter()
            .find(|row| row["slot"] == (69 + index).to_string())
            .unwrap();
        assert_eq!(row["item_guid"], item.to_string());
        assert_eq!(row["timestamp"], "108000");
    }
    for _ in 0..2 {
        let item = grant(&shard, guid, FIXTURE_BLADE, 1);
        let packets = sell(&mut connection, vendor, item, 0);
        assert_eq!(values(&packets, guid)[&624], item as u32);
        assert!(packets.iter().any(|(opcode, _)| *opcode == 0xaa));
        assert_eq!(sold_rows(&shard, guid).len(), 12);
    }
    assert_eq!(
        sold_rows(&shard, guid)
            .iter()
            .find(|row| row["slot"] == "70")
            .unwrap()["item_guid"],
        items[1].to_string()
    );
    buyback(&mut connection, vendor, 70);
    shard
        .assert_sql("UPDATE game_conformance_clock SET now_micros = 1800000005000000 WHERE id = 0");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    let packets = sell(&mut connection, vendor, item, 0);
    assert_eq!(values(&packets, guid)[&626], item as u32);
    assert_eq!(values(&packets, guid)[&1239], 108005);
    shard
        .assert_sql("UPDATE game_conformance_clock SET now_micros = 1800108006000000 WHERE id = 0");
    assert_eq!(
        sold_rows(&shard, guid).len(),
        12,
        "the timestamp is not a server expiry timer"
    );
    buyback(&mut connection, vendor, 70);
    assert_eq!(sold_rows(&shard, guid).len(), 11);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn empty_and_invalid_vendor_buyback_return_the_reference_refusals() {
    let (shard, mut connection, guid, vendor) = stage("buyback-refusals");
    for slot in [69, 80, 81] {
        let packets = buyback(&mut connection, vendor, slot);
        let mut expected = vendor.to_le_bytes().to_vec();
        expected.extend([0; 5]);
        assert_eq!(packets, vec![(0x1a5, expected)]);
    }
    let packets = buyback(&mut connection, 0, 69);
    let mut expected = vec![0; 16];
    expected.push(3);
    assert_eq!(packets, vec![(0x1a1, expected)]);
    assert!(sold_rows(&shard, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn money_is_checked_before_capacity_and_any_item_or_ring_change() {
    let (shard, mut connection, guid, vendor) = stage("buyback-money");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    grant(&shard, guid, FIXTURE_BLADE, 16);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 0 WHERE guid = {guid}"
    ));
    let before = sold_rows(&shard, guid);
    let inventory = shard.query_rows(&format!(
        "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    let packets = buyback(&mut connection, vendor, 69);
    let mut expected = vendor.to_le_bytes().to_vec();
    expected.extend(FIXTURE_BLADE.to_le_bytes());
    expected.push(2);
    assert_eq!(packets, vec![(0x1a5, expected)]);
    assert_eq!(sold_rows(&shard, guid), before);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
        )),
        inventory
    );
    assert_eq!(money(&shard, guid), "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn full_inventory_buyback_does_not_debit_or_release_the_item() {
    let (shard, mut connection, guid, vendor) = stage("buyback-full");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    grant(&shard, guid, FIXTURE_BLADE, 16);
    let before = sold_rows(&shard, guid);
    let copper = money(&shard, guid);
    let packets = buyback(&mut connection, vendor, 69);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].0, 0x112);
    assert_eq!(&packets[0].1[1..9], &item.to_le_bytes());
    assert_eq!(sold_rows(&shard, guid), before);
    assert_eq!(money(&shard, guid), copper);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn sold_items_do_not_count_toward_unique_grants_but_restoration_does() {
    let (shard, mut connection, guid, vendor) = stage("buyback-unique");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_count = 1 WHERE entry = {FIXTURE_BLADE}"
    ));
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    grant(&shard, guid, FIXTURE_BLADE, 1);
    let before = sold_rows(&shard, guid);
    let copper = money(&shard, guid);
    let packets = buyback(&mut connection, vendor, 69);
    assert_eq!(packets[0].0, 0x112);
    assert_eq!(packets[0].1[0], 17);
    assert_eq!(sold_rows(&shard, guid), before);
    assert_eq!(money(&shard, guid), copper);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn granting_another_stack_cannot_merge_into_buyback() {
    let (shard, mut connection, guid, vendor) = stage("buyback-storage");
    let item = grant(&shard, guid, FIXTURE_JERKY, 2);
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    let fresh = grant(&shard, guid, FIXTURE_JERKY, 3);
    assert_ne!(fresh, item);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE guid = {fresh}"
        ))[0]["stack_count"],
        "3"
    );
    assert_eq!(sold_rows(&shard, guid), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equipment_swap_bank_destroy_and_repair_cannot_reach_a_sold_item() {
    let (shard, mut connection, guid, vendor) = stage("buyback-inventory-gates");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 16644 WHERE guid = {vendor}"
    ));
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    for (reducer, arguments) in [
        ("debug_equip_item", vec![guid.to_string(), "23".into()]),
        (
            "debug_move_item",
            vec![guid.to_string(), "23".into(), "24".into()],
        ),
        ("gw_auto_bank_item", vec![actor.clone(), "23".into()]),
        ("debug_repair_item", vec![guid.to_string(), "23".into()]),
        (
            "gw_destroy_item",
            vec![actor.clone(), "23".into(), "0".into()],
        ),
    ] {
        let args: Vec<_> = arguments.iter().map(String::as_str).collect();
        let output = shard.call(reducer, &args);
        assert!(!output.status.success(), "{reducer} reached buyback");
        assert_eq!(sold_rows(&shard, guid), before);
    }
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE guid = {item}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn logout_destroys_buyback_and_relogin_starts_an_empty_ring() {
    let (shard, mut connection, guid, vendor) = stage("buyback-logout");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    request(&mut connection, 0x4b, Vec::new());
    assert!(sold_rows(&shard, guid).is_empty());
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_buyback_state WHERE character_guid = {guid}"
        ))
        .is_empty());
    drop(connection);
    let mut connection = enter(&shard, guid);
    assert!(sold_rows(&shard, guid).is_empty());
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    let packets = sell(&mut connection, vendor, item, 0);
    assert_eq!(values(&packets, guid)[&624], item as u32);
}

fn sell(
    connection: &mut ReducerConnection,
    vendor: u64,
    item: u64,
    count: u8,
) -> Vec<(u16, Vec<u8>)> {
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(item.to_le_bytes());
    body.push(count);
    request(connection, 0x1a0, body)
}

fn buyback(connection: &mut ReducerConnection, vendor: u64, slot: u32) -> Vec<(u16, Vec<u8>)> {
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(slot.to_le_bytes());
    request(connection, 0x290, body)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_full_sale_returns_the_same_item_with_its_instance_state() {
    let (shard, mut connection, guid, vendor) = stage("buyback-identity");
    shard.assert_call(
        "debug_grant_item",
        &[&guid.to_string(), &FIXTURE_BLADE.to_string(), "1"],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET durability = 7, enchant_id = 2564, soulbound = true WHERE owner_guid = {guid}"
    ));
    shard.assert_call("debug_equip_item", &[&guid.to_string(), "23"]);
    let mut before = shard.query_rows(&format!(
        "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    assert_eq!(before[0]["slot"], "15");
    before[0].insert("slot".into(), "23".into());
    let item = before[0]["guid"].parse().unwrap();
    connection.drain_packets();
    let sold = sell(&mut connection, vendor, item, 0);
    assert_eq!(values(&sold, guid)[&516], 0);
    let returned = buyback(&mut connection, vendor, 69);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
        )),
        before,
        "buyback preserves the Item GUID, age, binding, durability and enchantment"
    );
    assert!(sold
        .iter()
        .chain(&returned)
        .all(|(opcode, _)| !matches!(opcode, 0xaa | 0x166)));
    sell(&mut connection, vendor, item, 0);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn crash_recovery_discards_buyback_before_the_next_login() {
    let fixture = super::fixture_in(Standalone::start_persistent("buyback-crash"));
    let (mut shard, mut connection, guid, vendor) = stage_in(fixture);
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    assert_eq!(sold_rows(&shard, guid).len(), 1);
    shard.restart_persistent();
    drop(connection);
    let mut connection = enter(&shard, guid);
    assert!(sold_rows(&shard, guid).is_empty());
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE guid = {item}"
        ))
        .is_empty());
    let packets = buyback(&mut connection, vendor, 69);
    assert_eq!(packets[0].0, 0x1a5);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn transport_loss_discards_buyback_without_a_logout_request() {
    let (shard, mut connection, guid, vendor) = stage("buyback-disconnect");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    drop(connection);
    assert!(lyracore_test_support::poll_until(
        lyracore_test_support::POLL_TIMEOUT,
        || sold_rows(&shard, guid).is_empty()
    ));
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE guid = {item}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn item_guid_allocation_cannot_collide_with_buyback() {
    let (shard, mut connection, guid, vendor) = stage("buyback-guid-conflict");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    let low = (item & ((1 << 47) - 1)) - 1;
    shard.assert_sql(&format!(
        "UPDATE game_guid_allocator SET high_water = {low} WHERE id = 0"
    ));
    let refusal = shard.call(
        "debug_grant_item",
        &[&guid.to_string(), &FIXTURE_BLADE.to_string(), "1"],
    );
    assert!(!refusal.status.success());
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("ITEM_GUID_CONFLICT"),
        "{:?}",
        refusal
    );
    assert_eq!(sold_rows(&shard, guid), before);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE guid = {item}"
        ))
        .is_empty());
}

fn poison(shard: &Standalone, connection: &mut ReducerConnection, guid: u64, item: u64) {
    use lyracore_shared::spell::taxonomy::{E_ENCHANT_ITEM_TEMPORARY, P_ENCHANT_ID, T_SELF};
    let spell: u32 = 5090200;
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell),
            ("name", &"Buyback poison"),
            ("range_yd", &5),
            ("school_mask", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell, 0)),
            ("spell_id", &spell),
            ("kind", &E_ENCHANT_ITEM_TEMPORARY),
            ("base_points", &60),
            ("target", &T_SELF),
            ("p_0", &323),
            ("p_0_kind", &P_ENCHANT_ID),
        ],
    );
    shard.insert_row(
        "game_item_enchantment",
        &[("id", &82688), ("enchant_id", &323), ("kind", &1)],
    );
    shard.insert_row(
        "game_spell_enchant_charges",
        &[("spell_id", &spell), ("charges", &40)],
    );
    shard.assert_sql(&format!("UPDATE game_item_template SET spellid_1 = {spell}, spelltrigger_1 = 0 WHERE entry = {FIXTURE_JERKY}"));
    grant(shard, guid, FIXTURE_JERKY, 1);
    shard.assert_call("debug_use_item_on_item", &[&guid.to_string(), "24", "23"]);
    for _ in 0..20 {
        if !shard
            .query_rows(&format!(
                "SELECT charges FROM game_temporary_enchantment WHERE item_guid = {item}"
            ))
            .is_empty()
        {
            break;
        }
        connection
            .call("debug_advance_virtual_clock", json!([50000]))
            .unwrap();
    }
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT charges FROM game_temporary_enchantment WHERE item_guid = {item}"
        ))[0]["charges"],
        "40"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn temporary_enchantment_duration_pauses_in_buyback() {
    let (shard, mut connection, guid, vendor) = stage("buyback-enchantment");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    poison(&shard, &mut connection, guid, item);
    sell(&mut connection, vendor, item, 0);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_temporary_enchantment WHERE item_guid = {item}"
        ))
        .is_empty());
    shard
        .assert_sql("UPDATE game_conformance_clock SET now_micros = 1800000120000000 WHERE id = 0");
    let returned = buyback(&mut connection, vendor, 69);
    let enchantment_fields = values(&returned, item);
    assert_eq!(enchantment_fields[&25], 323);
    assert_eq!(enchantment_fields[&26], 60_000);
    assert_eq!(enchantment_fields[&27], 40);
    let enchantment = shard.query_rows(&format!(
        "SELECT charges,expires_at FROM game_temporary_enchantment WHERE item_guid = {item}"
    ));
    assert_eq!(enchantment[0]["charges"], "40");
    assert!(
        enchantment[0]["expires_at"].starts_with("2027-01-15T08:03:00"),
        "{enchantment:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn mail_cannot_detach_a_buyback_item() {
    let (shard, mut connection, guid, vendor) = stage("buyback-detach");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    let mail = shard.call(
        "realm_mail_send",
        &[
            &actor,
            "1",
            "\"Sold item\"",
            "\"\"",
            "0",
            "0",
            &item.to_string(),
            "false",
        ],
    );
    assert!(!mail.status.success());
    assert!(
        String::from_utf8_lossy(&mail.stderr).contains("mail:"),
        "{:?}",
        mail
    );
    assert_eq!(sold_rows(&shard, guid), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn auction_cannot_list_a_buyback_item() {
    let (shard, mut connection, guid, vendor) = stage("buyback-auction");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 4100 WHERE guid = {vendor}"
    ));
    shard.assert_sql("INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (5090001,0,0,0,0,0,0,0,0,0,0,0,0)");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = 5090001 WHERE guid = {vendor}"
    ));
    if shard
        .query_rows("SELECT * FROM game_auction_house WHERE id = 7")
        .is_empty()
    {
        shard.insert_row(
            "game_auction_house",
            &[
                ("id", &7),
                ("deposit_rate", &15),
                ("consignment_rate", &5),
                ("name", &"Buyback auction"),
            ],
        );
    }
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(item.to_le_bytes());
    body.extend(1u32.to_le_bytes());
    body.extend(2u32.to_le_bytes());
    body.extend(120u32.to_le_bytes());
    let packets = request(&mut connection, 0x256, body);
    assert!(packets.iter().any(|(opcode, _)| *opcode == 0x25b));
    assert_eq!(sold_rows(&shard, guid), before);
    assert!(shard.query_rows("SELECT * FROM game_auction").is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn character_deletion_releases_buyback_items_and_letter_text() {
    let (shard, mut connection, guid, vendor) = stage("buyback-character-delete");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    shard.insert_row(
        "game_item_text",
        &[("id", &5090001), ("text", &"Kept text"), ("letters", &1)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET item_text_id = 5090001 WHERE guid = {item}"
    ));
    sell(&mut connection, vendor, item, 0);
    shard.assert_call("debug_delete_character", &[&guid.to_string()]);
    assert!(sold_rows(&shard, guid).is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_item_text WHERE id = 5090001")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn login_discards_orphaned_buyback_without_any_previous_world_presence() {
    let (shard, mut connection, guid, vendor) = stage("buyback-orphaned-login");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    shard.assert_sql(&format!(
        "DELETE FROM game_native_player WHERE character_guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    let _new_connection = enter(&shard, guid);
    assert!(sold_rows(&shard, guid).is_empty());
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE guid = {item}"
        ))
        .is_empty());
}

fn sessionless(name: &str) -> (Standalone, u64, u64) {
    let (shard, guid, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET sell_price = 13 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    let vendor = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    (shard, guid, vendor)
}

fn freeze(shard: &Standalone, guid: u64, transfer: u64) -> String {
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    shard.assert_call(
        "begin_transfer",
        &[
            &transfer.to_string(),
            &actor,
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "true",
        ],
    );
    let blob = shard.query_rows(&format!(
        "SELECT blob FROM game_transfer_out WHERE transfer_id = {transfer}"
    ))[0]["blob"]
        .clone();
    serde_json::to_string(blob.strip_prefix("0x").unwrap()).unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn transfer_without_a_world_session_carries_inventory_and_discards_buyback() {
    let (source, guid, vendor) = sessionless("buyback-transfer-source");
    let sold = grant(&source, guid, FIXTURE_BLADE, 1);
    source.assert_call(
        "debug_sell_item",
        &[&guid.to_string(), &vendor.to_string(), "23"],
    );
    let carried = grant(&source, guid, FIXTURE_JERKY, 3);
    assert!(source
        .query_rows("SELECT * FROM game_native_player")
        .is_empty());
    let blob = freeze(&source, guid, 5090071);
    let (destination, _, _) = sessionless("buyback-transfer-destination");
    destination.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    destination.assert_call(
        "import_character_blob",
        &["5090071", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    assert_eq!(sold_rows(&destination, guid).len(), 1);
    assert!(destination
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE guid = {sold}"
        ))
        .is_empty());
    assert_eq!(
        destination.query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE guid = {carried}"
        ))[0]["stack_count"],
        "3"
    );
    source.assert_call(
        "confirm_import",
        &["5090071", r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "finish_transfer",
        &["5090071", r#"{"guid":0,"ownership":null}"#],
    );
    assert!(sold_rows(&source, guid).is_empty());
    assert!(source
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
        ))
        .is_empty());
    destination.assert_call(
        "release_transfer",
        &["5090071", r#"{"guid":0,"ownership":null}"#],
    );
    let _login = enter(&destination, guid);
    assert!(sold_rows(&destination, guid).is_empty());
    assert!(sold_rows(&destination, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn transfer_refuses_an_incoming_inventory_guid_already_in_buyback() {
    let (destination, _, _) = sessionless("buyback-import-destination");
    let account = destination
        .query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]["id"]
        .clone();
    destination.assert_call(
        "create_character",
        &[
            &account,
            "\"Other\"",
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
    let other: u64 = destination.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")
        [0]["guid"]
        .parse()
        .unwrap();
    destination.assert_call("debug_spawn_player_entity", &[&other.to_string()]);
    destination.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {other}"
    ));
    destination.assert_call(
        "debug_spawn_at_feet",
        &[
            &other.to_string(),
            &SCENARIO_VENDOR_ENTRY.to_string(),
            "2.0",
        ],
    );
    let vendor: u64 = destination
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    let sold = grant(&destination, other, FIXTURE_BLADE, 1);
    destination.assert_call(
        "debug_sell_item",
        &[&other.to_string(), &vendor.to_string(), "23"],
    );
    let before = sold_rows(&destination, other);
    let (source, guid, _) = sessionless("buyback-import-source");
    let carried = grant(&source, guid, FIXTURE_JERKY, 3);
    source.assert_sql(&format!(
        "UPDATE game_item_instance SET guid = {sold} WHERE guid = {carried}"
    ));
    let blob = freeze(&source, guid, 5090072);
    destination.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    let refusal = destination.call(
        "import_character_blob",
        &["5090072", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    assert!(!refusal.status.success());
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("ITEM_GUID_CONFLICT"),
        "{:?}",
        refusal
    );
    assert_eq!(sold_rows(&destination, other), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn package_teardown_clears_sessionless_buyback_while_keeping_the_character() {
    let (shard, _, _) = sessionless("buyback-package-teardown");
    shard.assert_call(
        "debug_create_package_character",
        &["\"test_fixture\"", "\"Sleeper\"", "1", "1"],
    );
    let guid: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Sleeper'")[0]
        ["guid"]
        .parse()
        .unwrap();
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    let vendor: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    grant(&shard, guid, FIXTURE_BLADE, 1);
    shard.assert_call(
        "debug_sell_item",
        &[&guid.to_string(), &vendor.to_string(), "23"],
    );
    assert_eq!(sold_rows(&shard, guid).len(), 1);
    shard.assert_call("teardown_package", &["\"test_fixture\""]);
    assert!(sold_rows(&shard, guid).is_empty());
    assert_eq!(
        shard
            .query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}"))
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn trade_cannot_offer_a_buyback_item() {
    let (shard, mut connection, guid, vendor) = stage("buyback-trade");
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]
        ["id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Trader\"",
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
    let other = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Trader'")[0]
        ["guid"]
        .clone();
    shard.assert_call("debug_spawn_player_entity", &[&other]);
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    let partner = format!(r#"{{"guid":{other},"ownership":null}}"#);
    shard.assert_call("gw_initiate_trade", &[&actor, &other]);
    shard.assert_call("gw_begin_trade", &[&partner]);
    assert!(!shard
        .call("gw_set_trade_item", &[&actor, "0", "23"])
        .status
        .success());
    assert!(shard.query_rows("SELECT * FROM game_trade_slot").is_empty());
    assert_eq!(sold_rows(&shard, guid), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn package_teardown_also_clears_buyback_after_sessionless_presence_is_lost() {
    let (shard, _, _) = sessionless("buyback-dormant-teardown");
    shard.assert_call(
        "debug_create_package_character",
        &["\"test_fixture\"", "\"Dormant\"", "1", "1"],
    );
    let guid: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Dormant'")[0]
        ["guid"]
        .parse()
        .unwrap();
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    let vendor: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    grant(&shard, guid, FIXTURE_BLADE, 1);
    shard.assert_call(
        "debug_sell_item",
        &[&guid.to_string(), &vendor.to_string(), "23"],
    );
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {guid}"
    ));
    shard.assert_call("teardown_package", &["\"test_fixture\""]);
    assert!(sold_rows(&shard, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn publish_repair_ends_legacy_world_sessions_before_buyback_resumes() {
    use lyracore_protocol::codec;

    let (mut shard, mut connection, guid, vendor) = stage("buyback-legacy");
    let inventory = grant(&shard, guid, FIXTURE_BLADE, 1);
    shard.assert_sql(&format!(
        "DELETE FROM game_buyback_state WHERE character_guid = {guid}"
    ));
    shard.insert_row(
        "game_character_buyback",
        &[
            ("id", &5090001),
            ("player_guid", &guid),
            ("item_entry", &FIXTURE_BLADE),
            ("stack_count", &1),
            ("price", &13),
        ],
    );
    let synthetic = 0x4090_0000_0000_0000;
    let create = lyracore_protocol::world::encode(
        &lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_item_create_object(&codec::ItemInstanceView {
                guid: synthetic,
                entry: FIXTURE_BLADE,
                owner_guid: guid,
                slot: 69,
                stack_count: 1,
                ..Default::default()
            })),
        ),
    )
    .unwrap();
    let mut fields = codec::update_mask::UpdateMaskValues::new();
    fields.set_u64(624, synthetic);
    fields.set_u32(1226, 13);
    fields.set_u32(1238, 0);
    let links = codec::build_values_update_raw(guid, &fields);
    connection
        .call(
            "native_world_reply",
            json!([[(create.opcode, create.body), links]]),
        )
        .unwrap();
    let legacy = delivered(&mut connection);
    assert_eq!(values(&legacy, synthetic)[&14], 1);
    assert_eq!(values(&legacy, guid)[&624], synthetic as u32);
    assert_eq!(values(&legacy, guid)[&1226], 13);
    assert_eq!(values(&legacy, guid)[&1238], 0);
    shard
        .assert_sql("UPDATE game_conformance_clock SET now_micros = 1800003600000000 WHERE id = 0");

    shard.publish_module();
    connection
        .call("debug_repair_after_publish", json!([]))
        .unwrap();
    assert!(
        connection.drain_packets().iter().any(|row| {
            let Packet(_, _, (tag, _), _, close_after, _, _) =
                serde_json::from_str(row.get()).unwrap();
            tag != 0 && close_after
        }),
        "legacy client must discard its synthetic objects and buyback fields on disconnect"
    );
    assert!(shard
        .query_rows("SELECT * FROM game_native_player")
        .is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_native_world_connection")
        .is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_character_buyback")
        .is_empty());
    assert!(sold_rows(&shard, guid).is_empty());
    let mut connection = enter(&shard, guid);
    shard
        .assert_sql("UPDATE game_conformance_clock SET now_micros = 1800007200000000 WHERE id = 0");
    assert_eq!(
        values(&sell(&mut connection, vendor, inventory, 0), guid)[&1238],
        111600
    );
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_native_player").len(),
        1
    );
    assert_eq!(sold_rows(&shard, guid).len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn selling_equipped_gear_relays_client_armor_and_every_resistance() {
    let (shard, mut connection, guid, vendor) = stage("buyback-equipped-resistances");
    shard.assert_sql(&format!("UPDATE game_item_template SET stat_armor = 100, holy_res = 11, fire_res = 12, nature_res = 13, frost_res = 14, shadow_res = 15, arcane_res = 16 WHERE entry = {FIXTURE_BLADE}"));
    let item = grant(&shard, guid, FIXTURE_BLADE, 1);
    let base: u32 = shard.query_rows(&format!(
        "SELECT armor FROM game_world_entity WHERE guid = {guid}"
    ))[0]["armor"]
        .parse()
        .unwrap();
    connection
        .call("debug_equip_item", json!([guid, 23]))
        .unwrap();
    let equipped = values(&delivered(&mut connection), guid);
    for (index, expected) in [base + 100, 11, 12, 13, 14, 15, 16].into_iter().enumerate() {
        assert_eq!(equipped.get(&(155 + index as u16)), Some(&expected));
    }
    let sold = values(&sell(&mut connection, vendor, item, 0), guid);
    for (index, expected) in [base, 0, 0, 0, 0, 0, 0].into_iter().enumerate() {
        assert_eq!(sold.get(&(155 + index as u16)), Some(&expected));
    }
    assert_eq!(sold[&516], 0);
    let returned = values(&buyback(&mut connection, vendor, 69), guid);
    assert_eq!(returned[&532], item as u32);
    let mut client = sold.clone();
    client.extend(returned);
    for (index, expected) in [base, 0, 0, 0, 0, 0, 0].into_iter().enumerate() {
        assert_eq!(client.get(&(155 + index as u16)), Some(&expected));
    }
    shard.assert_call("debug_equip_item", &[&guid.to_string(), "23"]);
    let sold_again = values(&sell(&mut connection, vendor, item, 0), guid);
    for index in 0..7 {
        assert_eq!(sold_again.get(&(155 + index)), sold.get(&(155 + index)));
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn selling_a_guild_charter_preserves_its_petition_for_buyback() {
    let (shard, mut connection, guid, vendor) = stage("buyback-charter");
    let mut template = shard
        .query_rows(&format!(
            "SELECT * FROM game_item_template WHERE entry = {FIXTURE_BLADE}"
        ))
        .remove(0);
    template.insert("entry".into(), "5863".into());
    template.insert("name".into(), "Guild Charter".into());
    let columns = "entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property,spellppmrate_1,spellppmrate_2,spellppmrate_3,spellppmrate_4,spellppmrate_5";
    let fields: Vec<Field> = columns
        .split(',')
        .map(|key| (key, &template[key] as _))
        .collect();
    shard.insert_row("game_item_template", &fields);
    let charter = grant(&shard, guid, 5863, 1);
    shard.assert_sql(&format!("INSERT INTO game_guild_petition (petition_id,charter_item_guid,owner_guid,owner_name,team,name,created_micros) VALUES (123,{charter},{guid},'Conformance',469,'Buyback Charter',1800000000000000)"));
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET enchant_id = 123 WHERE guid = {charter}"
    ));
    let realm = format!(r#"{{"__identity__":"0x{}"}}"#, "11".repeat(32));
    shard.assert_call(
        "operator_add_peer_shard",
        &[&realm, "\"unavailable-realm\""],
    );
    shard.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"world":{realm}}}}}"#)],
    );
    let petition = shard.query_rows("SELECT * FROM game_guild_petition");
    sell(&mut connection, vendor, charter, 0);
    assert!(
        shard
            .query_rows("SELECT * FROM game_guild_transaction")
            .is_empty(),
        "sale must not send the destruction-only Petition close"
    );
    buyback(&mut connection, vendor, 69);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_guild_petition"),
        petition
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT enchant_id FROM game_item_instance WHERE guid = {charter}"
        ))[0]["enchant_id"],
        "123"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn buyback_merging_keeps_the_destination_property_and_binding() {
    let (shard, mut connection, guid, vendor) = stage("buyback-merge-state");
    let destination = grant(&shard, guid, FIXTURE_JERKY, 2);
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET random_property_id = 1 WHERE guid = {destination}"
    ));
    let sold = grant(&shard, guid, FIXTURE_JERKY, 2);
    shard.assert_sql(&format!("UPDATE game_item_instance SET random_property_id = 2, soulbound = true WHERE guid = {sold}"));
    sell(&mut connection, vendor, sold, 0);
    buyback(&mut connection, vendor, 69);
    let rows = shard.query_rows(&format!("SELECT guid,stack_count,random_property_id,soulbound FROM game_item_instance WHERE owner_guid = {guid}"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["guid"], destination.to_string());
    assert_eq!(rows[0]["stack_count"], "4");
    assert_eq!(rows[0]["random_property_id"], "1");
    assert_eq!(rows[0]["soulbound"], "false");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn buyback_price_is_the_sale_payment_even_after_the_template_price_changes() {
    let (shard, mut connection, guid, vendor) = stage("buyback-price");
    let item = grant(&shard, guid, FIXTURE_JERKY, 2);
    let sold = sell(&mut connection, vendor, item, 0);
    assert_eq!(values(&sold, guid)[&1226], 4);
    assert_eq!(money(&shard, guid), "50004");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET sell_price = 999 WHERE entry = {FIXTURE_JERKY}"
    ));
    let returned = buyback(&mut connection, vendor, 69);
    assert_eq!(money(&shard, guid), "50000");
    assert_eq!(values(&returned, guid)[&1226], 0);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE guid = {item}"
        ))[0]["stack_count"],
        "2"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_letter_grant_retry_cannot_duplicate_a_copy_in_buyback() {
    let (shard, mut connection, guid, vendor) = stage("buyback-letter-retry");
    shard.insert_row(
        "game_item_template",
        &[
            ("entry", &8383),
            ("name", &"Plain Letter"),
            ("sell_price", &13),
            ("max_stack", &1),
        ],
    );
    shard.insert_row(
        "game_item_text",
        &[("id", &5090001), ("text", &"Retry once"), ("letters", &1)],
    );
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    shard.assert_call("gw_mail_grant_letter", &[&actor, "5090001"]);
    let item = shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND item_text_id = 5090001"
    ))[0]["guid"]
        .parse()
        .unwrap();
    sell(&mut connection, vendor, item, 0);
    let before = sold_rows(&shard, guid);
    shard.assert_call("gw_mail_grant_letter", &[&actor, "5090001"]);
    assert_eq!(sold_rows(&shard, guid), before);
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND item_text_id = 5090001"
        ))
        .is_empty());
    buyback(&mut connection, vendor, 69);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND item_text_id = 5090001"
        ))[0]["guid"],
        item.to_string()
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_nonempty_equipped_bag_cannot_be_sold_into_buyback() {
    let (shard, mut connection, guid, vendor) = stage("buyback-filled-bag");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, container_slots = 4, max_durability = 0 WHERE entry = {FIXTURE_BLADE}"));
    let bag = grant(&shard, guid, FIXTURE_BLADE, 1);
    shard.assert_call("debug_move_item", &[&guid.to_string(), "23", "19"]);
    grant(&shard, guid, FIXTURE_JERKY, 1);
    shard.assert_call("debug_move_item", &[&guid.to_string(), "23", "120"]);
    let inventory = shard.query_rows(&format!(
        "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    let copper = money(&shard, guid);
    let packets = sell(&mut connection, vendor, bag, 0);
    let mut expected = vendor.to_le_bytes().to_vec();
    expected.extend(bag.to_le_bytes());
    expected.push(2);
    assert_eq!(packets, vec![(0x1a1, expected)]);
    assert!(sold_rows(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), copper);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
        )),
        inventory
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn selling_from_an_equipped_bag_clears_the_container_link_and_keeps_identity() {
    use super::transport::OperatorTransport;
    use std::collections::BTreeMap;
    use wire_client::runner::Transport;

    let (shard, _connection, guid, vendor) = stage("buyback-bag-sale");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, container_slots = 4, max_durability = 0 WHERE entry = {FIXTURE_BLADE}"));
    let bag = grant(&shard, guid, FIXTURE_BLADE, 1);
    shard.assert_call("debug_move_item", &[&guid.to_string(), "23", "19"]);
    let item = grant(&shard, guid, FIXTURE_JERKY, 1);
    shard.assert_call("debug_move_item", &[&guid.to_string(), "23", "120"]);
    let mut transport = OperatorTransport::connect(&shard, guid, BTreeMap::new()).unwrap();
    let opening = &transport.batches().last().unwrap().projections;
    assert_eq!(opening[&bag].fields[&50], item as u32);
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(item.to_le_bytes());
    body.push(0);
    transport.send(0x1a0, &body).unwrap();
    let projections = &transport.batches().last().unwrap().projections;
    assert_eq!(projections[&bag].object_type, 2);
    assert_eq!(projections[&bag].fields.get(&50).copied().unwrap_or(0), 0);
    assert_eq!(projections[&guid].fields[&624], item as u32);
    assert_eq!(sold_rows(&shard, guid)[0]["item_guid"], item.to_string());
    assert!(super::properties::run(transport.batches())
        .unwrap()
        .is_empty());
}
