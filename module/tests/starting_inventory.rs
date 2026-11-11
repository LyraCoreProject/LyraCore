use lyracore_test_support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn starting_items_fill_the_backpack_including_the_hearthstone() {
    let mut shard = Standalone::start("starting-inventory-placement");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_sql("DELETE FROM game_start_item WHERE race_class = 257");
    for (id, entry, count) in [(1, 52, 5), (2, 6948, 1)] {
        shard.insert_row(
            "game_start_item",
            &[
                ("id", &id),
                ("race_class", &257),
                ("item_entry", &entry),
                ("count", &count),
            ],
        );
    }
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"STARTITEMS\"", &credentials, &credentials],
    );
    let account =
        &shard.query_rows("SELECT id FROM game_account WHERE username = 'STARTITEMS'")[0]["id"];
    shard.assert_call(
        "create_character",
        &[
            account,
            "\"Startitems\"",
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
    let character =
        &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Startitems'")[0]["guid"];
    let items = shard.query_rows(&format!(
        "SELECT entry,slot,stack_count FROM game_item_instance WHERE owner_guid = {character}"
    ));
    assert_eq!(items.len(), 2);
    for (entry, slot, count) in [(52, 23, 5), (6948, 24, 1)] {
        let item = items
            .iter()
            .find(|item| item["entry"] == entry.to_string())
            .unwrap();
        assert_eq!(item["slot"], slot.to_string());
        assert_eq!(item["stack_count"], count.to_string());
    }
    shard.assert_call("debug_spawn_player_entity", &[character]);
    shard.assert_call("debug_grant_item", &[character, "51", "1"]);
    let granted = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = {character} AND entry = 51"
    ));
    assert_eq!(granted[0]["slot"], "25");
}
