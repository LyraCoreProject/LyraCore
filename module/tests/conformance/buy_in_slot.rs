use lyracore_test_support::{ReducerConnection, Standalone};
use serde_json::json;

use super::{enter, fixture, Packet};

const THREAD: u32 = 5090052;
const ARMOR: u32 = 5090050;

fn stage(name: &str) -> (Standalone, ReducerConnection, u64, u64) {
    let (shard, guid, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call("debug_spawn_at_feet", &[&guid.to_string(), "51004", "2.0"]);
    let vendor = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51004")
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    let mut connection = enter(&shard, guid);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}"
    ));
    shard.assert_sql(&format!("UPDATE game_item_template SET buy_price = 10, buy_count = 1, max_stack = 20 WHERE entry = {THREAD}"));
    connection.drain_packets();
    (shard, connection, guid, vendor)
}

fn buy(
    connection: &mut ReducerConnection,
    vendor: u64,
    entry: u32,
    bag: u64,
    slot: u8,
    count: u8,
) -> Vec<(u16, Vec<u8>)> {
    connection
        .call("native_world_receive", json!([0x1dcu32, vec![0u8; 8]]))
        .unwrap();
    connection.drain_packets();
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(entry.to_le_bytes());
    body.extend(bag.to_le_bytes());
    body.extend([slot, count]);
    if let Err(reason) = connection.call("native_world_receive", json!([0x1a3u32, body])) {
        let replies: Vec<(u16, Vec<u8>)> =
            serde_json::from_str(reason.strip_prefix("native_reply:").expect(&reason)).unwrap();
        connection
            .call("native_world_reply", json!([replies]))
            .unwrap();
    }
    connection
        .drain_packets()
        .into_iter()
        .filter_map(|row| {
            let Packet(_, _, (tag, opcode), body, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            (tag == 0).then(|| {
                (
                    serde_json::from_str(opcode.get()).unwrap(),
                    (0..body.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&body[i..i + 2], 16).unwrap())
                        .collect(),
                )
            })
        })
        .collect()
}

fn grant(shard: &Standalone, guid: u64, entry: u32, count: u32, slot: u8) -> u64 {
    shard.assert_call(
        "debug_grant_item",
        &[&guid.to_string(), &entry.to_string(), &count.to_string()],
    );
    let item: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
        ))
        .iter()
        .map(|row| row["guid"].parse().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {slot} WHERE guid = {item}"
    ));
    item
}

fn inventory(shard: &Standalone, guid: u64) -> Vec<std::collections::BTreeMap<String, String>> {
    shard.query_rows(&format!(
        "SELECT * FROM game_item_instance WHERE owner_guid = {guid}"
    ))
}

fn money(shard: &Standalone, guid: u64) -> String {
    shard.query_rows(&format!(
        "SELECT money FROM game_world_entity WHERE guid = {guid}"
    ))[0]["money"]
        .clone()
}

fn refusal(packets: &[(u16, Vec<u8>)], result: u8) {
    let mut expected = vec![result];
    expected.extend([0; 17]);
    assert_eq!(packets, &[(0x112, expected)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn backpack_purchase_uses_the_requested_slot_and_acknowledges_before_one_receipt() {
    let (shard, mut connection, guid, vendor) = stage("purchase-backpack");
    let packets = buy(&mut connection, vendor, THREAD, guid, 30, 0);
    let rows = inventory(&shard, guid);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["slot"], "30");
    assert_eq!(rows[0]["stack_count"], "1");
    assert_eq!(money(&shard, guid), "49990");
    let replies: Vec<_> = packets
        .iter()
        .filter(|(opcode, _)| [0x1a4, 0x166].contains(opcode))
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0].0, 0x1a4);
    assert_eq!(replies[1].0, 0x166);
    assert_eq!(
        u32::from_le_bytes(replies[1].1[21..25].try_into().unwrap()),
        30
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn carried_bag_guids_resolve_all_four_destinations() {
    let (shard, mut connection, guid, vendor) = stage("purchase-bags");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, container_slots = 4, max_stack = 1 WHERE entry = {ARMOR}"));
    for slot in 19..=22 {
        let bag = grant(&shard, guid, ARMOR, 1, slot);
        buy(&mut connection, vendor, THREAD, bag, 2, 1);
        assert!(inventory(&shard, guid).iter().any(|row| row["slot"]
            == (120 + (slot - 19) * 18 + 2).to_string()
            && row["entry"] == THREAD.to_string()));
    }
    assert_eq!(money(&shard, guid), "49960");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn unresolved_bag_guids_are_silent_and_leave_state_unchanged() {
    let (shard, mut connection, guid, vendor) = stage("purchase-unknown-bag");
    let carried = grant(&shard, guid, ARMOR, 1, 23);
    let bank_bag = grant(&shard, guid, ARMOR, 1, 63);
    let foreign = grant(&shard, guid, ARMOR, 1, 19);
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET owner_guid = {} WHERE guid = {foreign}",
        guid + 1
    ));
    let before = inventory(&shard, guid);
    for bag in [0, u64::MAX, carried, bank_bag, foreign] {
        assert!(buy(&mut connection, vendor, THREAD, bag, 0, 1).is_empty());
        assert_eq!(inventory(&shard, guid), before);
        assert_eq!(money(&shard, guid), "50000");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn bank_and_invalid_character_slots_are_refused_without_a_debit() {
    let (shard, mut connection, guid, vendor) = stage("purchase-bank-refusal");
    for slot in [39, 62, 63, 68, 69, 113, 120, 254] {
        refusal(&buy(&mut connection, vendor, THREAD, guid, slot, 1), 3);
        assert!(inventory(&shard, guid).is_empty());
        assert_eq!(money(&shard, guid), "50000");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn bag_capacity_and_family_refusals_are_atomic() {
    let (shard, mut connection, guid, vendor) = stage("purchase-bag-refusal");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 1, subclass = 0, container_slots = 4 WHERE entry = {ARMOR}"));
    let bag = grant(&shard, guid, ARMOR, 1, 19);
    refusal(&buy(&mut connection, vendor, THREAD, bag, 4, 1), 40);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET subclass = 1, bag_family = 4 WHERE entry = {ARMOR}"
    ));
    refusal(&buy(&mut connection, vendor, THREAD, bag, 0, 1), 16);
    assert_eq!(inventory(&shard, guid).len(), 1);
    assert_eq!(money(&shard, guid), "50000");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bag_family = 3 WHERE entry = {THREAD}"
    ));
    buy(&mut connection, vendor, THREAD, bag, 0, 1);
    assert!(inventory(&shard, guid)
        .iter()
        .any(|row| row["entry"] == THREAD.to_string() && row["slot"] == "120"));
    assert_eq!(money(&shard, guid), "49990");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn occupied_slots_only_accept_a_matching_partial_stack() {
    let (shard, mut connection, guid, vendor) = stage("purchase-stack-refusal");
    grant(&shard, guid, ARMOR, 1, 30);
    grant(&shard, guid, THREAD, 20, 31);
    let before = inventory(&shard, guid);
    for slot in [30, 31] {
        refusal(&buy(&mut connection, vendor, THREAD, guid, slot, 1), 19);
        assert_eq!(inventory(&shard, guid), before);
        assert_eq!(money(&shard, guid), "50000");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_purchase_merges_at_the_destination_and_spills_with_one_total_receipt() {
    let (shard, mut connection, guid, vendor) = stage("purchase-merge");
    let item = grant(&shard, guid, THREAD, 19, 30);
    let packets = buy(&mut connection, vendor, THREAD, guid, 30, 3);
    let rows = inventory(&shard, guid);
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row["guid"] == item.to_string() && row["stack_count"] == "20"));
    assert!(rows
        .iter()
        .any(|row| row["slot"] == "23" && row["stack_count"] == "2"));
    let receipts: Vec<_> = packets
        .iter()
        .filter(|(opcode, _)| *opcode == 0x166)
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        u32::from_le_bytes(receipts[0].1[21..25].try_into().unwrap()),
        u32::MAX
    );
    assert_eq!(
        u32::from_le_bytes(receipts[0].1[37..41].try_into().unwrap()),
        3
    );
    assert_eq!(money(&shard, guid), "49970");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn null_slots_search_the_selected_bag_before_other_carried_storage() {
    let (shard, mut connection, guid, vendor) = stage("purchase-auto-slot");
    buy(&mut connection, vendor, THREAD, guid, 255, 1);
    assert_eq!(inventory(&shard, guid)[0]["slot"], "23");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 1, subclass = 0, container_slots = 4 WHERE entry = {ARMOR}"));
    let bag = grant(&shard, guid, ARMOR, 1, 20);
    buy(&mut connection, vendor, THREAD, bag, 255, 1);
    assert!(inventory(&shard, guid)
        .iter()
        .any(|row| row["slot"] == "138"));
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET stack_count = 20 WHERE owner_guid = {guid} AND slot = 138"
    ));
    for slot in 139..142 {
        grant(&shard, guid, ARMOR, 1, slot);
    }
    buy(&mut connection, vendor, THREAD, bag, 255, 1);
    assert!(inventory(&shard, guid)
        .iter()
        .any(|row| row["slot"] == "23" && row["stack_count"] == "2"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn insufficient_money_precedes_destination_validation_and_changes_nothing() {
    let (shard, mut connection, guid, vendor) = stage("purchase-money");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 9 WHERE guid = {guid}"
    ));
    for slot in [30, 39, 0] {
        let packets = buy(&mut connection, vendor, THREAD, guid, slot, 1);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].0, 0x1a5);
        assert_eq!(*packets[0].1.last().unwrap(), 2);
        assert!(inventory(&shard, guid).is_empty());
        assert_eq!(money(&shard, guid), "9");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equipment_purchases_keep_the_ordinary_equip_stats_and_wire_values() {
    let (shard, mut connection, guid, vendor) = stage("purchase-equip");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 1, required_level = 1, stat_armor = 43, fire_res = 7, stat_strength = 5, bonding = 2, max_stack = 1 WHERE entry = {ARMOR}"));
    let before = shard.query_rows(&format!(
        "SELECT sheet_str_bonus FROM game_world_entity WHERE guid = {guid}"
    ))[0]["sheet_str_bonus"]
        .parse::<u32>()
        .unwrap();
    let packets = buy(&mut connection, vendor, ARMOR, guid, 0, 1);
    let rows = inventory(&shard, guid);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["slot"], "0");
    assert_eq!(rows[0]["soulbound"], "true");
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT sheet_str_bonus FROM game_world_entity WHERE guid = {guid}"
        ))[0]["sheet_str_bonus"]
            .parse::<u32>()
            .unwrap(),
        before + 5
    );
    let fields: std::collections::BTreeMap<_, _> = packets
        .iter()
        .flat_map(|(opcode, body)| super::properties::updates(*opcode, body).unwrap())
        .filter(|update| update.guid == guid)
        .flat_map(|update| update.fields)
        .collect();
    assert_eq!(fields[&155], 43);
    assert_eq!(fields[&157], 7);
    assert_eq!(fields[&260], ARMOR);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equipment_requires_one_item_a_fitting_slot_and_no_replacement() {
    let (shard, mut connection, guid, vendor) = stage("purchase-equip-refusals");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 1, required_level = 1, max_stack = 1 WHERE entry = {ARMOR}"));
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 0, 2), 20);
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 4, 1), 20);
    grant(&shard, guid, ARMOR, 1, 0);
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 0, 1), 20);
    assert_eq!(money(&shard, guid), "50000");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_vendor_can_equip_a_bag_but_cannot_equip_it_in_the_bank() {
    let (shard, mut connection, guid, vendor) = stage("purchase-equip-bag");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, container_slots = 4, required_level = 1, max_stack = 1 WHERE entry = {ARMOR}"));
    buy(&mut connection, vendor, ARMOR, guid, 19, 1);
    assert_eq!(inventory(&shard, guid)[0]["slot"], "19");
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 63, 1), 3);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn equipment_checks_class_skill_and_required_level() {
    let (shard, mut connection, guid, vendor) = stage("purchase-equip-eligibility");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 1, required_level = 2, max_stack = 1, allowed_class = 0 WHERE entry = {ARMOR}"));
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 0, 1), 10);
    shard.assert_sql(&format!("UPDATE game_item_template SET allowed_class = 1503, required_skill = 164, required_skill_rank = 10 WHERE entry = {ARMOR}"));
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 0, 1), 8);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET required_skill = 0 WHERE entry = {ARMOR}"
    ));
    let packets = buy(&mut connection, vendor, ARMOR, guid, 0, 1);
    let mut expected = vec![1];
    expected.extend(2u32.to_le_bytes());
    expected.extend([0; 17]);
    assert_eq!(packets, [(0x112, expected)]);
    assert!(inventory(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), "50000");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn storage_failure_and_unique_limits_leave_money_and_items_unchanged() {
    let (shard, mut connection, guid, vendor) = stage("purchase-storage-atomicity");
    grant(&shard, guid, THREAD, 1, 39);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_count = 1 WHERE entry = {THREAD}"
    ));
    let before = inventory(&shard, guid);
    refusal(&buy(&mut connection, vendor, THREAD, guid, 30, 1), 17);
    assert_eq!(inventory(&shard, guid), before);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_count = 0, max_stack = 1 WHERE entry = {THREAD}"
    ));
    for slot in 23..39 {
        grant(&shard, guid, ARMOR, 1, slot);
    }
    let before = inventory(&shard, guid);
    refusal(&buy(&mut connection, vendor, THREAD, guid, 255, 1), 50);
    assert_eq!(inventory(&shard, guid), before);
    assert_eq!(money(&shard, guid), "50000");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn buying_a_two_hander_stores_the_offhand_or_refuses_atomically() {
    let (shard, mut connection, guid, vendor) = stage("purchase-twohand");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 2, subclass = 1, inventory_type = 17, required_level = 1, max_stack = 1 WHERE entry = {ARMOR}"));
    shard.assert_call("debug_learn_spell", &[&guid.to_string(), "197"]);
    let offhand = grant(&shard, guid, THREAD, 1, 16);
    for slot in 23..39 {
        grant(&shard, guid, THREAD, 20, slot);
    }
    let before = inventory(&shard, guid);
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 15, 1), 50);
    assert_eq!(inventory(&shard, guid), before);
    assert_eq!(money(&shard, guid), "50000");
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot = 30"
    ));
    buy(&mut connection, vendor, ARMOR, guid, 15, 1);
    let rows = inventory(&shard, guid);
    assert!(rows
        .iter()
        .any(|row| row["guid"] == offhand.to_string() && row["slot"] == "30"));
    assert!(rows
        .iter()
        .any(|row| row["entry"] == ARMOR.to_string() && row["slot"] == "15"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn dead_characters_cannot_purchase_and_receive_no_reply() {
    let (shard, mut connection, guid, vendor) = stage("purchase-dead");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true, health = 0 WHERE guid = {guid}"
    ));
    assert!(buy(&mut connection, vendor, THREAD, guid, 30, 1).is_empty());
    assert!(inventory(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), "50000");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn keyring_destinations_check_the_item_family_and_publish_the_link() {
    let (shard, mut connection, guid, vendor) = stage("purchase-keyring");
    refusal(&buy(&mut connection, vendor, THREAD, guid, 81, 1), 16);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bag_family = 9 WHERE entry = {THREAD}"
    ));
    let packets = buy(&mut connection, vendor, THREAD, guid, 81, 1);
    let rows = inventory(&shard, guid);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["slot"], "81");
    let fields: std::collections::BTreeMap<_, _> = packets
        .iter()
        .flat_map(|(opcode, body)| super::properties::updates(*opcode, body).unwrap())
        .filter(|update| update.guid == guid)
        .flat_map(|update| update.fields)
        .collect();
    let item: u64 = rows[0]["guid"].parse().unwrap();
    assert_eq!(fields[&648], item as u32);
    assert_eq!(fields[&649], (item >> 32) as u32);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn purchase_admission_checks_items_vendors_and_reputation_before_storage() {
    let (shard, mut connection, guid, vendor) = stage("purchase-admission");
    for (npc, entry, expected) in [(vendor, u32::MAX, 0), (0, THREAD, 5)] {
        let packets = buy(&mut connection, npc, entry, guid, 30, 1);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].0, 0x1a5);
        assert_eq!(&packets[0].1[..8], &[0; 8]);
        assert_eq!(*packets[0].1.last().unwrap(), expected);
    }
    shard.assert_sql(&format!("UPDATE game_item_template SET required_reputation_faction = 50900, required_reputation_rank = 4 WHERE entry = {THREAD}"));
    let packets = buy(&mut connection, vendor, THREAD, guid, 0, 1);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].0, 0x1a5);
    assert_eq!(*packets[0].1.last().unwrap(), 12);
    assert!(inventory(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), "50000");
    shard.assert_call(
        "debug_grant_reputation",
        &[&guid.to_string(), "50900", "9000"],
    );
    shard.assert_sql("UPDATE game_faction_template SET faction = 50900 WHERE id = 35");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET buy_price = 11 WHERE entry = {THREAD}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 10 WHERE guid = {guid}"
    ));
    buy(&mut connection, vendor, THREAD, guid, 30, 1);
    assert_eq!(money(&shard, guid), "0");
    assert_eq!(inventory(&shard, guid)[0]["slot"], "30");
}
