use lyracore_test_support::{ReducerConnection, Standalone};

use super::buy_in_slot::{grant, inventory, money, request, stage};
use lyracore_package_delta::fixture_ids::FIXTURE_BLADE;

const FIXTURE_FACTION: u32 = 50900;
const VENDOR_FACTION_TEMPLATE: u32 = 35;
const BAG_ENTRY: u32 = lyracore_package_delta::fixture_ids::FIXTURE_REINS;

// Tempered Blade: item level 12, sword (multiplier column 7), uncommon (quality row 6),
// 70 durability. 20 lost points cost 20 x 10 x 1.5 = 300 copper; 70 lost points cost 1050.
pub(super) fn price_the_blade(shard: &Standalone) {
    shard.insert_row(
        "game_durability_cost",
        &[
            ("id", &(12 * 32 + 7)),
            ("item_level", &12),
            ("multiplier_id", &7),
            ("cost", &10),
        ],
    );
    shard.insert_row(
        "game_durability_quality",
        &[("id", &6), ("quality_mod", &1.5)],
    );
}

fn damaged_blade(shard: &Standalone, guid: u64, slot: u8, durability: u32) -> u64 {
    let blade = grant(shard, guid, FIXTURE_BLADE, 1, slot);
    set_durability(shard, blade, durability);
    blade
}

fn set_durability(shard: &Standalone, item: u64, durability: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET durability = {durability} WHERE guid = {item}"
    ));
}

fn durability(shard: &Standalone, item: u64) -> String {
    shard.query_rows(&format!(
        "SELECT durability FROM game_item_instance WHERE guid = {item}"
    ))[0]["durability"]
        .clone()
}

fn set_money(shard: &Standalone, guid: u64, copper: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = {copper} WHERE guid = {guid}"
    ));
}

fn repair(connection: &mut ReducerConnection, vendor: u64, item: u64) -> Vec<(u16, Vec<u8>)> {
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(item.to_le_bytes());
    request(connection, 0x2a8, body)
}

fn assert_no_chat_or_refusal(packets: &[(u16, Vec<u8>)]) {
    const SMSG_MESSAGECHAT: u16 = 0x96;
    const SMSG_INVENTORY_CHANGE_FAILURE: u16 = 0x112;
    assert!(
        packets
            .iter()
            .all(|(opcode, _)| ![SMSG_MESSAGECHAT, SMSG_INVENTORY_CHANGE_FAILURE].contains(opcode)),
        "a repair answers with no chat line and no refusal: {packets:?}"
    );
}

fn sell(connection: &mut ReducerConnection, vendor: u64, item: u64) -> Vec<(u16, Vec<u8>)> {
    let mut body = vendor.to_le_bytes().to_vec();
    body.extend(item.to_le_bytes());
    body.push(0);
    request(connection, 0x1a0, body)
}

fn equip_bag_in_slot_19(shard: &Standalone, guid: u64) {
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, container_slots = 4, max_stack = 1 WHERE entry = {BAG_ENTRY}"
    ));
    grant(shard, guid, BAG_ENTRY, 1, 19);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn repairing_one_item_charges_the_durability_cost_table_price() {
    let (shard, mut connection, guid, vendor) = stage("repair-one");
    price_the_blade(&shard);
    let blade = damaged_blade(&shard, guid, 23, 50);
    let spare = damaged_blade(&shard, guid, 24, 50);
    assert_no_chat_or_refusal(&repair(&mut connection, vendor, blade));
    assert_eq!(durability(&shard, blade), "70");
    assert_eq!(durability(&shard, spare), "50");
    assert_eq!(money(&shard, guid), "49700");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn repair_all_covers_equipped_backpack_and_bag_items() {
    let (shard, mut connection, guid, vendor) = stage("repair-all");
    price_the_blade(&shard);
    equip_bag_in_slot_19(&shard, guid);
    let equipped = damaged_blade(&shard, guid, 15, 50);
    let backpack = damaged_blade(&shard, guid, 23, 50);
    let in_bag = damaged_blade(&shard, guid, 120, 50);
    let banked = damaged_blade(&shard, guid, 39, 50);
    assert_no_chat_or_refusal(&repair(&mut connection, vendor, 0));
    for item in [equipped, backpack, in_bag] {
        assert_eq!(durability(&shard, item), "70");
    }
    assert_eq!(durability(&shard, banked), "50");
    assert_eq!(money(&shard, guid), "49100");
    assert_eq!(inventory(&shard, guid).len(), 5);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn repair_all_skips_what_the_character_cannot_pay_for_and_stays_silent() {
    let (shard, mut connection, guid, vendor) = stage("repair-all-short");
    price_the_blade(&shard);
    equip_bag_in_slot_19(&shard, guid);
    let broken = damaged_blade(&shard, guid, 15, 0);
    let backpack = damaged_blade(&shard, guid, 23, 50);
    let in_bag = damaged_blade(&shard, guid, 120, 50);
    set_money(&shard, guid, 700);
    assert_no_chat_or_refusal(&repair(&mut connection, vendor, 0));
    assert_eq!(durability(&shard, broken), "0");
    assert_eq!(durability(&shard, backpack), "70");
    assert_eq!(durability(&shard, in_bag), "70");
    assert_eq!(money(&shard, guid), "100");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn repair_all_pays_in_slot_order_so_the_backpack_comes_before_the_bag() {
    let (shard, mut connection, guid, vendor) = stage("repair-all-order");
    price_the_blade(&shard);
    equip_bag_in_slot_19(&shard, guid);
    let backpack = damaged_blade(&shard, guid, 23, 50);
    let in_bag = damaged_blade(&shard, guid, 120, 50);
    set_money(&shard, guid, 300);
    assert_no_chat_or_refusal(&repair(&mut connection, vendor, 0));
    assert_eq!(durability(&shard, backpack), "70");
    assert_eq!(durability(&shard, in_bag), "50");
    assert_eq!(money(&shard, guid), "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn repairing_a_broken_equipped_item_gives_its_stats_back() {
    let (shard, mut connection, guid, vendor) = stage("repair-broken-stats");
    price_the_blade(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET stat_stamina = 40 WHERE entry = {FIXTURE_BLADE}"
    ));
    let blade = damaged_blade(&shard, guid, 15, 0);
    let max_health = |shard: &Standalone| -> u32 {
        shard.query_rows(&format!(
            "SELECT max_health FROM game_world_entity WHERE guid = {guid}"
        ))[0]["max_health"]
            .parse()
            .unwrap()
    };
    let broken = max_health(&shard);
    assert_no_chat_or_refusal(&repair(&mut connection, vendor, blade));
    assert_eq!(durability(&shard, blade), "70");
    assert!(max_health(&shard) > broken);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_damaged_item_cannot_be_sold_when_its_repair_cost_is_unknown() {
    let (shard, mut connection, guid, vendor) = stage("sell-unpriced");
    let blade = damaged_blade(&shard, guid, 23, 50);
    sell(&mut connection, vendor, blade);
    assert_eq!(inventory(&shard, guid).len(), 1);
    assert_eq!(money(&shard, guid), "50000");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_single_repair_without_enough_money_changes_nothing_and_stays_silent() {
    let (shard, mut connection, guid, vendor) = stage("repair-one-short");
    price_the_blade(&shard);
    let blade = damaged_blade(&shard, guid, 23, 50);
    set_money(&shard, guid, 299);
    assert!(repair(&mut connection, vendor, blade).is_empty());
    assert_eq!(durability(&shard, blade), "50");
    assert_eq!(money(&shard, guid), "299");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_honored_character_pays_ten_percent_less_to_repair() {
    let (shard, mut connection, guid, vendor) = stage("repair-honored");
    price_the_blade(&shard);
    shard.assert_sql(&format!(
        "DELETE FROM game_faction_template WHERE id = {VENDOR_FACTION_TEMPLATE}"
    ));
    shard.insert_row(
        "game_faction_template",
        &[
            ("id", &VENDOR_FACTION_TEMPLATE),
            ("faction", &FIXTURE_FACTION),
        ],
    );
    shard.assert_call(
        "debug_grant_reputation",
        &[&guid.to_string(), &FIXTURE_FACTION.to_string(), "9000"],
    );
    let blade = damaged_blade(&shard, guid, 23, 50);
    assert_no_chat_or_refusal(&repair(&mut connection, vendor, blade));
    assert_eq!(durability(&shard, blade), "70");
    assert_eq!(money(&shard, guid), "49730");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_damaged_item_sells_for_its_price_minus_the_repair_cost() {
    let (shard, mut connection, guid, vendor) = stage("sell-damaged");
    price_the_blade(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET sell_price = 1000 WHERE entry = {FIXTURE_BLADE}"
    ));
    let blade = damaged_blade(&shard, guid, 23, 50);
    sell(&mut connection, vendor, blade);
    assert!(inventory(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), "50700");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_damaged_item_worth_less_than_its_repair_sells_for_one_copper() {
    let (shard, mut connection, guid, vendor) = stage("sell-ruined");
    price_the_blade(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET sell_price = 240 WHERE entry = {FIXTURE_BLADE}"
    ));
    let blade = damaged_blade(&shard, guid, 23, 50);
    sell(&mut connection, vendor, blade);
    assert!(inventory(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), "50001");
}
