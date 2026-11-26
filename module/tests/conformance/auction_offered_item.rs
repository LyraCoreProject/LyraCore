use lyracore_test_support::actor;

use super::buy_in_slot::request;
use super::{enter, fixture, Standalone};
use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, SCENARIO_VENDOR_ENTRY};

const CMSG_AUCTION_SELL_ITEM: u32 = 0x256;
const SMSG_AUCTION_COMMAND_RESULT: u16 = 0x25b;
const AUCTIONEER_AND_VENDOR: u32 = 4100;
// Auction 0, action Started, result ErrInventory, then InventoryResult ItemNotFound as one byte.
const NOT_TRADEABLE_REPLY: [u8; 13] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0x17];

/// A Character at an Auctioneer with an Auction House, 50,000 copper and an empty pack.
fn auction_fixture(name: &str) -> (Standalone, u64, String, u64) {
    let (shard, guid, account) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    let auctioneer: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    // Once any faction template exists, a trade needs both Characters' templates to be friendly.
    let player_faction = shard.query_rows(&format!(
        "SELECT faction_template FROM game_world_entity WHERE guid = {guid}"
    ))[0]["faction_template"]
        .clone();
    shard.assert_sql(&format!(
        "DELETE FROM game_faction_template WHERE id = {player_faction} OR id = 5090001"
    ));
    shard.insert_row(
        "game_faction_template",
        &[
            ("id", &player_faction),
            ("faction", &1),
            ("faction_group", &3),
            ("friend_group", &2),
            ("enemy_group", &12),
        ],
    );
    shard.insert_row("game_faction_template", &[("id", &5090001)]);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {AUCTIONEER_AND_VENDOR}, \
         faction_template = 5090001 WHERE guid = {auctioneer}"
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
                ("name", &"Offered item auction"),
            ],
        );
    }

    (shard, guid, account, auctioneer)
}

fn sell_item_body(auctioneer: u64, item: u64) -> Vec<u8> {
    let mut sell = auctioneer.to_le_bytes().to_vec();
    sell.extend(item.to_le_bytes());
    sell.extend(1u32.to_le_bytes());
    sell.extend(2u32.to_le_bytes());
    sell.extend(120u32.to_le_bytes());
    sell
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_offered_in_a_trade_cannot_be_listed_until_the_trade_ends() {
    let (shard, guid, account, auctioneer) = auction_fixture("auction-offered-item");
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Partner\"",
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
    let partner = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Partner'")[0]
        ["guid"]
        .clone();
    shard.assert_call("debug_spawn_player_entity", &[&partner]);
    let position = shard.query_rows(&format!(
        "SELECT x, y, z FROM game_world_entity WHERE guid = {guid}"
    ));
    let at = &position[0];
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {}, z = {} WHERE guid = {partner}",
        at["x"], at["y"], at["z"]
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}"
    ));

    let mut connection = enter(&shard, guid);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    connection.drain_packets();
    shard.assert_call(
        "debug_grant_item",
        &[&guid.to_string(), &FIXTURE_BLADE.to_string(), "1"],
    );
    let carried = shard.query_rows(&format!(
        "SELECT guid, slot FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    let blade = &carried[0];
    let (item, slot): (u64, String) = (blade["guid"].parse().unwrap(), blade["slot"].clone());

    shard.assert_call("gw_initiate_trade", &[&actor(guid), &partner]);
    shard.assert_call("gw_begin_trade", &[&actor(&partner)]);
    shard.assert_call("gw_set_trade_item", &[&actor(guid), "0", &slot]);
    let sessions =
        shard.query_rows("SELECT initiator_guid, target_guid, open FROM game_trade_session");
    assert_eq!(sessions.len(), 1, "the trade opened: {sessions:?}");
    assert_eq!(
        (
            sessions[0]["initiator_guid"].as_str(),
            sessions[0]["target_guid"].as_str(),
            sessions[0]["open"].as_str()
        ),
        (guid.to_string().as_str(), partner.as_str(), "true")
    );
    let offered = || {
        shard.query_rows(&format!(
            "SELECT trade_slot FROM game_trade_slot WHERE item_guid = {item} AND owner_guid = {guid}"
        ))
    };
    assert_eq!(offered().len(), 1, "the blade is offered");

    let sell = sell_item_body(auctioneer, item);
    let refused = request(&mut connection, CMSG_AUCTION_SELL_ITEM, sell.clone());
    assert!(
        refused.contains(&(SMSG_AUCTION_COMMAND_RESULT, NOT_TRADEABLE_REPLY.to_vec())),
        "{refused:?}"
    );
    assert_eq!(offered().len(), 1, "the blade is still offered");
    assert!(shard.query_rows("SELECT * FROM game_auction").is_empty());
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_item_instance WHERE guid = {item} AND owner_guid = {guid}"
            ))
            .len(),
        1,
        "the offered item stays with the seller"
    );

    shard.assert_call("gw_cancel_trade", &[&actor(guid)]);
    assert!(shard
        .query_rows("SELECT id FROM game_trade_session")
        .is_empty());
    request(&mut connection, CMSG_AUCTION_SELL_ITEM, sell);
    let listed = shard.query_rows("SELECT item_guid FROM game_auction");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["item_guid"], item.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_soulbound_quest_bonded_item_cannot_be_listed() {
    let (shard, guid, _account, auctioneer) = auction_fixture("auction-quest-bonded-item");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bonding = 4 WHERE entry = {FIXTURE_BLADE}"
    ));
    let mut connection = enter(&shard, guid);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    connection.drain_packets();
    shard.assert_call(
        "debug_grant_item",
        &[&guid.to_string(), &FIXTURE_BLADE.to_string(), "1"],
    );
    let carried = shard.query_rows(&format!(
        "SELECT guid, soulbound FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    assert_eq!(carried[0]["soulbound"], "true", "the grant bound it");
    let item: u64 = carried[0]["guid"].parse().unwrap();

    let refused = request(
        &mut connection,
        CMSG_AUCTION_SELL_ITEM,
        sell_item_body(auctioneer, item),
    );

    // The reply code is not pinned: a soulbound item answers ErrItemNotFound where vmangos
    // answers AUCTION_ERR_INVENTORY (a separate defect).
    assert!(
        refused
            .iter()
            .any(|(opcode, _)| *opcode == SMSG_AUCTION_COMMAND_RESULT),
        "{refused:?}"
    );
    assert!(shard.query_rows("SELECT * FROM game_auction").is_empty());
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_item_instance WHERE guid = {item} AND owner_guid = {guid}"
            ))
            .len(),
        1,
        "the item stays with the seller"
    );
}
