use lyracore_test_support::{ReducerConnection, Standalone};
use serde_json::json;

use super::{enter, fixture, Packet};

const THREAD: u32 = 5090052;
const ARMOR: u32 = 5090050;

fn stage(name: &str) -> (Standalone, ReducerConnection, u64, u64) {
    stage_in(fixture(name))
}

fn stage_in(
    (shard, guid, _): (Standalone, u64, String),
) -> (Standalone, ReducerConnection, u64, u64) {
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
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(entry.to_le_bytes());
    body.extend(bag.to_le_bytes());
    body.extend([slot, count]);
    request(connection, 0x1a3, body)
}

fn request(connection: &mut ReducerConnection, opcode: u32, body: Vec<u8>) -> Vec<(u16, Vec<u8>)> {
    connection
        .call("native_world_receive", json!([0x1dcu32, vec![0u8; 8]]))
        .unwrap();
    connection.drain_packets();
    if let Err(reason) = connection.call("native_world_receive", json!([opcode, body])) {
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
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bag_family = 0 WHERE entry = {THREAD}"
    ));
    buy(&mut connection, vendor, THREAD, guid, 81, 1);
    let rows = inventory(&shard, guid);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["guid"], item.to_string());
    assert_eq!(rows[0]["stack_count"], "2");
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
    shard.assert_sql("INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (35,50900,0,0,0,0,0,0,0,0,0,0,0)");
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_two_hander_cannot_store_a_shield_in_a_quiver() {
    let (shard, mut connection, guid, vendor) = stage("purchase-quiver-offhand");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 2, subclass = 1, inventory_type = 17, required_level = 1, max_stack = 1 WHERE entry = {ARMOR}"));
    shard.assert_call("debug_learn_spell", &[&guid.to_string(), "197"]);
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 4, subclass = 6, inventory_type = 14, max_stack = 1 WHERE entry = {THREAD}"));
    shard.assert_sql("UPDATE game_item_template SET class = 11, subclass = 2, inventory_type = 18, container_slots = 4 WHERE entry = 5090054");
    grant(&shard, guid, 5090054, 1, 19);
    let shield = grant(&shard, guid, THREAD, 1, 16);
    for slot in 23..39 {
        grant(&shard, guid, ARMOR, 1, slot);
    }
    let before = inventory(&shard, guid);
    refusal(&buy(&mut connection, vendor, ARMOR, guid, 15, 1), 50);
    assert_eq!(inventory(&shard, guid), before);
    assert_eq!(money(&shard, guid), "50000");
    shard.assert_sql("UPDATE game_item_template SET class = 1, subclass = 0 WHERE entry = 5090054");
    buy(&mut connection, vendor, ARMOR, guid, 15, 1);
    assert!(inventory(&shard, guid)
        .iter()
        .any(|row| row["guid"] == shield.to_string() && row["slot"] == "120"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_quest_toast_precedes_item_updates_and_one_total_receipt() {
    let (shard, mut connection, guid, vendor) = stage("purchase-quest-order");
    shard.assert_sql(&format!("UPDATE game_quest_objective SET kind = 1, target_entry = {THREAD}, required_count = 20 WHERE id = 5090000"));
    shard.assert_call("debug_grant_quest", &[&guid.to_string(), "50900"]);
    grant(&shard, guid, THREAD, 19, 30);
    let packets = buy(&mut connection, vendor, THREAD, guid, 30, 3);
    let toast = packets
        .iter()
        .position(|(opcode, _)| *opcode == 0x19a)
        .expect("quest item toast");
    let update = packets
        .iter()
        .position(|(opcode, body)| {
            super::properties::updates(*opcode, body)
                .unwrap()
                .iter()
                .any(|object| object.guid != guid)
        })
        .expect("item update");
    let receipt = packets
        .iter()
        .position(|(opcode, _)| *opcode == 0x166)
        .unwrap();
    assert!(
        toast < update && update < receipt,
        "packet opcodes: {:?}",
        packets.iter().map(|p| p.0).collect::<Vec<_>>()
    );
    assert_eq!(
        packets
            .iter()
            .filter(|(opcode, _)| *opcode == 0x19a)
            .count(),
        1
    );
    assert_eq!(
        &packets[toast].1,
        &[THREAD.to_le_bytes(), 3u32.to_le_bytes()].concat()
    );
    assert_eq!(
        packets
            .iter()
            .filter(|(opcode, _)| *opcode == 0x166)
            .count(),
        1
    );
}

fn keyring_purchase(name: &str, slot: u8, count: u8) -> (Standalone, ReducerConnection, u64) {
    let (shard, mut connection, guid, vendor) = stage(name);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bag_family = 9 WHERE entry = {THREAD}"
    ));
    buy(&mut connection, vendor, THREAD, guid, slot, count);
    assert!(inventory(&shard, guid)
        .iter()
        .any(|row| row["slot"] == slot.to_string()));
    (shard, connection, guid)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_keyring_purchase_can_be_split() {
    let (shard, mut connection, guid) = keyring_purchase("purchase-keyring-split", 81, 4);
    let packets = request(&mut connection, 0x10e, vec![255, 81, 255, 82, 2]);
    assert!(
        !packets.iter().any(|(opcode, _)| *opcode == 0x112),
        "{packets:?}"
    );
    let rows = inventory(&shard, guid);
    assert_eq!(rows.len(), 2);
    for slot in [81, 82] {
        assert!(rows
            .iter()
            .any(|row| row["slot"] == slot.to_string() && row["stack_count"] == "2"));
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_keyring_purchase_can_be_destroyed() {
    let (shard, mut connection, guid) = keyring_purchase("purchase-keyring-destroy", 112, 4);
    request(&mut connection, 0x111, vec![255, 112, 1, 0, 0, 0]);
    assert_eq!(inventory(&shard, guid)[0]["stack_count"], "3");
    request(&mut connection, 0x111, vec![255, 112, 0, 0, 0, 0]);
    assert!(inventory(&shard, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_keyring_purchase_can_move_to_backpack_and_back() {
    let (shard, mut connection, guid) = keyring_purchase("purchase-keyring-move", 81, 4);
    for (source, destination) in [(81, 23), (23, 112), (112, 81)] {
        let packets = request(&mut connection, 0x10c, vec![255, destination, 255, source]);
        assert!(
            !packets.iter().any(|(opcode, _)| *opcode == 0x112),
            "{packets:?}"
        );
        assert_eq!(inventory(&shard, guid)[0]["slot"], destination.to_string());
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_ordinary_unknown_purchase_uses_a_zero_vendor_guid() {
    let (shard, mut connection, guid, vendor) = stage("purchase-ordinary-unknown");
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(u32::MAX.to_le_bytes());
    body.extend([1, 0]);
    let packets = request(&mut connection, 0x1a2, body);
    let mut expected = vec![0; 8];
    expected.extend(u32::MAX.to_le_bytes());
    expected.push(0);
    assert_eq!(packets, [(0x1a5, expected)]);
    assert_eq!(money(&shard, guid), "50000");
    assert!(inventory(&shard, guid).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_equipment_purchase_shares_ordinary_equip_effects_or_records_the_gap() {
    fn effects(shard: &Standalone, guid: u64, packets: &[(u16, Vec<u8>)]) -> (bool, bool, bool) {
        let aura = !shard
            .query_rows(&format!(
                "SELECT * FROM game_aura WHERE target_guid = {guid} AND spell_id = 168"
            ))
            .is_empty();
        let cooldown = !shard
            .query_rows(&format!(
                "SELECT * FROM game_spell_cd WHERE caster_guid = {guid} AND spell_id = 50072"
            ))
            .is_empty();
        (
            aura,
            cooldown,
            packets.iter().any(|(opcode, _)| *opcode == 0xb0),
        )
    }
    let (shard, mut connection, guid, vendor) = stage("purchase-shared-equip-effects");
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 1, required_level = 1, spellid_1 = 168, spelltrigger_1 = 1, spellid_2 = 50072, spelltrigger_2 = 0 WHERE entry = {ARMOR}"));
    grant(&shard, guid, ARMOR, 1, 23);
    let packets = request(&mut connection, 0x10a, vec![255, 23]);
    assert_eq!(inventory(&shard, guid)[0]["slot"], "0");
    let ordinary = effects(&shard, guid, &packets);
    request(&mut connection, 0x10c, vec![255, 23, 255, 0]);
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cd WHERE caster_guid = {guid}"
    ));
    let packets = buy(&mut connection, vendor, ARMOR, guid, 0, 1);
    let purchased = effects(&shard, guid, &packets);
    assert_eq!(
        ordinary, purchased,
        "on-equip aura, cooldown and cooldown packet"
    );
    if ordinary != (true, true, true) {
        let ledger = include_str!("../../../conformance/features/inventory/bag_management.toml");
        assert!(
            ledger.split("[[known_gaps]]").skip(1).any(|gap| {
                gap.contains("on-equip spells")
                    && gap.contains("30-second equip cooldown")
                    && gap.contains("symbol = \"apply_item_move\"")
            }),
            "ordinary equip also lacks these effects; record the shared gap on bag_management"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn review_purchase_into_an_offered_stack_updates_the_offer_and_clears_acceptance() {
    let (shard, mut connection, guid, vendor) = stage("purchase-trade-offer");
    let item = grant(&shard, guid, THREAD, 19, 30);
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
    shard.assert_call("gw_set_trade_item", &[&actor, "0", "30"]);
    shard.assert_call("gw_accept_trade", &[&actor]);
    assert_eq!(
        shard.query_rows("SELECT initiator_accepted FROM game_trade_session")[0]
            ["initiator_accepted"],
        "true"
    );
    let packets = buy(&mut connection, vendor, THREAD, guid, 30, 3);
    assert!(!packets.iter().any(|(opcode, _)| *opcode == 0x1a5));
    assert_eq!(
        shard.query_rows("SELECT item_guid FROM game_trade_slot")[0]["item_guid"],
        item.to_string()
    );
    assert_eq!(
        shard.query_rows("SELECT initiator_accepted FROM game_trade_session")[0]
            ["initiator_accepted"],
        "false"
    );
    assert!(inventory(&shard, guid)
        .iter()
        .any(|row| row["guid"] == item.to_string() && row["stack_count"] == "20"));
}

fn stock_time(shard: &Standalone, seconds: i64) {
    let micros = 1_800_000_000_000_000 + seconds * 1_000_000;
    shard.assert_sql(&format!(
        "UPDATE game_conformance_clock SET now_micros = {micros} WHERE id = 0"
    ));
}

fn stock_offer(shard: &Standalone) {
    shard.assert_sql(&format!("UPDATE game_npc_vendor SET max_count = 10, restock_seconds = 10, stock_flags = 0 WHERE item_entry = {THREAD}"));
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET buy_count = 2 WHERE entry = {THREAD}"
    ));
}

fn listed_stock(connection: &mut ReducerConnection, vendor: u64) -> u32 {
    let packets = request(connection, 0x19e, vendor.to_le_bytes().to_vec());
    let body = &packets
        .iter()
        .find(|(opcode, _)| *opcode == 0x19f)
        .unwrap()
        .1;
    body[9..]
        .chunks_exact(28)
        .find_map(|row| {
            (u32::from_le_bytes(row[4..8].try_into().unwrap()) == THREAD)
                .then(|| u32::from_le_bytes(row[12..16].try_into().unwrap()))
        })
        .expect("sold-out offers remain listed")
}

fn ordinary_buy(connection: &mut ReducerConnection, vendor: u64, lots: u8) -> Vec<(u16, Vec<u8>)> {
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(THREAD.to_le_bytes());
    body.extend([lots, 0]);
    request(connection, 0x1a2, body)
}

fn stock_ack(packets: &[(u16, Vec<u8>)], count: u32) {
    let body = &packets
        .iter()
        .find(|(opcode, _)| *opcode == 0x1a4)
        .unwrap()
        .1;
    assert_eq!(u32::from_le_bytes(body[12..16].try_into().unwrap()), count);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_both_buy_opcodes_share_stock_and_refuse_atomically() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-both");
    stock_offer(&shard);
    assert_eq!(listed_stock(&mut connection, vendor), 10);
    stock_ack(&ordinary_buy(&mut connection, vendor, 2), 6);
    stock_ack(&buy(&mut connection, vendor, THREAD, guid, 30, 2), 2);
    for in_slot in [false, true] {
        let before = inventory(&shard, guid);
        let balance = money(&shard, guid);
        let stock = shard.query_rows("SELECT * FROM game_vendor_stock");
        let packets = if in_slot {
            buy(&mut connection, vendor, THREAD, guid, 30, 2)
        } else {
            ordinary_buy(&mut connection, vendor, 2)
        };
        let mut expected = vendor.to_le_bytes().to_vec();
        expected.extend(THREAD.to_le_bytes());
        expected.push(1);
        assert_eq!(packets, [(0x1a5, expected)]);
        assert_eq!(inventory(&shard, guid), before);
        assert_eq!(money(&shard, guid), balance);
        assert_eq!(shard.query_rows("SELECT * FROM game_vendor_stock"), stock);
    }
    stock_ack(&ordinary_buy(&mut connection, vendor, 1), 0);
    assert_eq!(listed_stock(&mut connection, vendor), 0);
    assert_eq!(money(&shard, guid), "49950");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_lazy_restock_discards_partial_intervals_and_caps_at_full() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-time");
    stock_offer(&shard);
    stock_ack(&ordinary_buy(&mut connection, vendor, 5), 0);
    stock_time(&shard, 9);
    assert_eq!(listed_stock(&mut connection, vendor), 0);
    stock_time(&shard, 25);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    stock_time(&shard, 30);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    stock_ack(&buy(&mut connection, vendor, THREAD, guid, 30, 1), 2);
    stock_time(&shard, 39);
    assert_eq!(listed_stock(&mut connection, vendor), 2);
    stock_time(&shard, 40);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    stock_time(&shard, 1040);
    assert_eq!(listed_stock(&mut connection, vendor), 10);
    assert!(shard
        .query_rows("SELECT * FROM game_vendor_stock")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_failed_money_and_storage_leave_stock_unchanged() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-refuse");
    stock_offer(&shard);
    ordinary_buy(&mut connection, vendor, 1);
    let stock = shard.query_rows("SELECT * FROM game_vendor_stock");
    for in_slot in [false, true] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET money = 0 WHERE guid = {guid}"
        ));
        let packets = if in_slot {
            buy(&mut connection, vendor, THREAD, guid, 30, 1)
        } else {
            ordinary_buy(&mut connection, vendor, 1)
        };
        assert_eq!(
            packets
                .iter()
                .find(|(opcode, _)| *opcode == 0x1a5)
                .unwrap()
                .1[12],
            2
        );
        assert_eq!(shard.query_rows("SELECT * FROM game_vendor_stock"), stock);
    }
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_stack = 2 WHERE entry = {THREAD}"
    ));
    for slot in 24..=38 {
        grant(&shard, guid, ARMOR, 1, slot);
    }
    let before = inventory(&shard, guid);
    for in_slot in [false, true] {
        let packets = if in_slot {
            buy(&mut connection, vendor, THREAD, guid, 30, 1)
        } else {
            ordinary_buy(&mut connection, vendor, 1)
        };
        assert!(!packets.iter().any(|(opcode, _)| *opcode == 0x1a4));
        assert_eq!(inventory(&shard, guid), before);
        assert_eq!(money(&shard, guid), "50000");
        assert_eq!(shard.query_rows("SELECT * FROM game_vendor_stock"), stock);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_is_per_spawn_and_only_a_new_runtime_resets_it() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-lifetime");
    stock_offer(&shard);
    shard.assert_call("native_vendor_stock_runtime", &["123"]);
    ordinary_buy(&mut connection, vendor, 3);
    shard.assert_call("debug_spawn_at_feet", &[&guid.to_string(), "51004", "2.0"]);
    let other: u64 = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51004")
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    assert_ne!(vendor, other);
    assert_eq!(listed_stock(&mut connection, other), 10);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]
        ["id"]
        .clone();
    shard.assert_call(
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
    let shopper: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")[0]
        ["guid"]
        .parse()
        .unwrap();
    drop(connection);
    let mut connection = enter(&shard, shopper);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {shopper}"
    ));
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    stock_ack(&ordinary_buy(&mut connection, vendor, 1), 2);
    drop(connection);
    let mut connection = enter(&shard, guid);
    shard.assert_call("native_vendor_stock_runtime", &["123"]);
    assert_eq!(listed_stock(&mut connection, vendor), 2);
    shard.assert_call("native_vendor_stock_runtime", &["124"]);
    assert_eq!(listed_stock(&mut connection, vendor), 10);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_missing_import_keeps_legacy_purchases_unlimited() {
    let (shard, mut connection, _guid, vendor) = stage("vendor-stock-legacy");
    shard.assert_sql(&format!(
        "UPDATE game_npc_vendor SET max_count = 1 WHERE item_entry = {THREAD}"
    ));
    for _ in 0..2 {
        ordinary_buy(&mut connection, vendor, 2);
        assert_eq!(listed_stock(&mut connection, vendor), 1);
    }
    assert!(shard
        .query_rows("SELECT * FROM game_vendor_stock")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_unlimited_offers_keep_the_wire_sentinel() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-unlimited");
    assert_eq!(listed_stock(&mut connection, vendor), u32::MAX);
    stock_ack(&ordinary_buy(&mut connection, vendor, 2), u32::MAX);
    stock_ack(&buy(&mut connection, vendor, THREAD, guid, 30, 2), u32::MAX);
    assert_eq!(listed_stock(&mut connection, vendor), u32::MAX);
    assert!(shard
        .query_rows("SELECT * FROM game_vendor_stock")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_restart_replaces_the_runtime_without_spending_stock() {
    let fixture = super::fixture_in(Standalone::start_persistent("vendor-stock-restart"));
    let (mut shard, mut connection, guid, vendor) = stage_in(fixture);
    stock_offer(&shard);
    shard.assert_call("native_vendor_stock_runtime", &["123"]);
    ordinary_buy(&mut connection, vendor, 3);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    shard.restart_persistent();
    drop(connection);
    shard.assert_call("native_vendor_stock_runtime", &["124"]);
    let mut connection = enter(&shard, guid);
    assert_eq!(listed_stock(&mut connection, vendor), 10);
    assert!(shard
        .query_rows("SELECT * FROM game_vendor_stock")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_ordinary_respawn_keeps_the_counter_and_elapsed_time() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-respawn");
    stock_offer(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_npc_vendor SET restock_seconds = 1000 WHERE item_entry = {THREAD}"
    ));
    ordinary_buy(&mut connection, vendor, 3);
    shard.assert_call(
        "debug_kill_creature",
        &[&guid.to_string(), &vendor.to_string()],
    );
    stock_time(&shard, 61);
    shard.assert_call("tick_creatures", &[r#"{"scheduled_id":0,"scheduled_at":{"Interval":{"__time_duration_micros__":100000}},"instance_id":18446744073709551615}"#]);
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {vendor}"
        ))
        .is_empty());
    stock_time(&shard, 1000);
    shard.assert_call("tick_creatures", &[r#"{"scheduled_id":0,"scheduled_at":{"Interval":{"__time_duration_micros__":100000}},"instance_id":18446744073709551615}"#]);
    assert_eq!(listed_stock(&mut connection, vendor), 6);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_removing_the_spawn_discards_its_counter() {
    let (shard, mut connection, _guid, vendor) = stage("vendor-stock-remove");
    stock_offer(&shard);
    ordinary_buy(&mut connection, vendor, 3);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    shard.assert_call("operator_delete_creature_spawn", &[&vendor.to_string()]);
    assert!(shard
        .query_rows("SELECT * FROM game_vendor_stock")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_a_changed_entry_keeps_the_source_spawns_counter() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-entry");
    stock_offer(&shard);
    ordinary_buy(&mut connection, vendor, 3);
    let vendor_flag = lyracore_shared::constants::npc_flags::VENDOR;
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET npc_flags = {vendor_flag} WHERE entry = 51001"
    ));
    shard.assert_call("debug_spawn_at_feet", &[&guid.to_string(), "51001", "2.0"]);
    let replacement: u64 = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51001")
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_creature_spawn SET source_guid = {vendor} WHERE guid = {replacement}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {vendor}"
    ));
    shard.assert_sql(&format!("INSERT INTO game_npc_vendor (id,creature_entry,item_entry,slot,max_count,restock_seconds,stock_flags) VALUES (99999999,51001,{THREAD},0,10,10,0)"));
    assert_eq!(listed_stock(&mut connection, replacement), 4);
    stock_ack(&ordinary_buy(&mut connection, replacement, 1), 2);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_missing_restock_metadata_ignores_an_older_counter() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-legacy-counter");
    stock_offer(&shard);
    ordinary_buy(&mut connection, vendor, 3);
    shard.assert_sql(&format!(
        "UPDATE game_npc_vendor SET max_count = 1, restock_seconds = 0 WHERE item_entry = {THREAD}"
    ));
    assert_eq!(listed_stock(&mut connection, vendor), 1);
    ordinary_buy(&mut connection, vendor, 2);
    assert_eq!(listed_stock(&mut connection, vendor), 1);
    assert_eq!(money(&shard, guid), "49950");
    assert_eq!(
        shard.query_rows("SELECT count FROM game_vendor_stock")[0]["count"],
        "4"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_refused_purchases_preserve_partial_restock_observations() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-refused-restock");
    stock_offer(&shard);
    ordinary_buy(&mut connection, vendor, 5);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 0 WHERE guid = {guid}"
    ));
    let before = inventory(&shard, guid);
    stock_time(&shard, 25);
    let packets = ordinary_buy(&mut connection, vendor, 1);
    assert_eq!(
        packets
            .iter()
            .find(|(opcode, _)| *opcode == 0x1a5)
            .unwrap()
            .1[12],
        2
    );
    assert_eq!(money(&shard, guid), "0");
    assert_eq!(inventory(&shard, guid), before);
    stock_time(&shard, 30);
    assert_eq!(listed_stock(&mut connection, vendor), 4);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}"
    ));
    grant(&shard, guid, ARMOR, 1, 30);
    let before = inventory(&shard, guid);
    stock_time(&shard, 45);
    let packets = buy(&mut connection, vendor, THREAD, guid, 30, 1);
    assert!(packets.iter().any(|(opcode, _)| *opcode == 0x112));
    assert!(!packets.iter().any(|(opcode, _)| *opcode == 0x1a4));
    assert_eq!(money(&shard, guid), "50000");
    assert_eq!(inventory(&shard, guid), before);
    stock_time(&shard, 50);
    assert_eq!(listed_stock(&mut connection, vendor), 8);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_stock_commit_failure_rolls_back_payment_stock_and_acknowledgement() {
    let (shard, mut connection, guid, vendor) = stage("vendor-stock-commit");
    stock_offer(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_stack = 2 WHERE entry = {THREAD}"
    ));
    ordinary_buy(&mut connection, vendor, 1);
    let balance = money(&shard, guid);
    let before = inventory(&shard, guid);
    let stock = shard.query_rows("SELECT * FROM game_vendor_stock");
    shard.assert_sql("DELETE FROM game_guid_range WHERE id = 0");
    for in_slot in [false, true] {
        let packets = if in_slot {
            buy(&mut connection, vendor, THREAD, guid, 30, 1)
        } else {
            ordinary_buy(&mut connection, vendor, 1)
        };
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].0, 0x1a5);
        assert_eq!(packets[0].1[12], 0);
        assert_eq!(money(&shard, guid), balance);
        assert_eq!(inventory(&shard, guid), before);
        assert_eq!(shard.query_rows("SELECT * FROM game_vendor_stock"), stock);
    }
}
