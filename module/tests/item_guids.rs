mod support;

use support::Standalone;

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call(
        "create_character",
        &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    shard.assert_sql("UPDATE game_character SET guid = 16777217 WHERE name = 'Other'");
    shard.assert_call("debug_spawn_player_entity", &["16777217"]);
    shard.assert_sql("DELETE FROM game_item_instance");
    shard
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn characters_above_bit_24_can_receive_items_together() {
    let shard = fixture("item-guid-owners");
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    shard.assert_call("debug_grant_item", &["16777217", "5090050", "1"]);
    let rows = shard.query_rows("SELECT guid FROM game_item_instance");
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["guid"], rows[1]["guid"]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn legacy_items_keep_their_identity_and_no_longer_control_new_grants() {
    let shard = fixture("item-guid-legacy");
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let legacy = 0x4000_0000_0000_01ffu64;
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET guid = {legacy} WHERE owner_guid = 1"
    ));
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let rows = shard.query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["guid"] == legacy.to_string()));
    let fresh: u64 = rows
        .iter()
        .find(|row| row["guid"] != legacy.to_string())
        .unwrap()["guid"]
        .parse()
        .unwrap();
    assert_eq!(fresh, 0x4000_8000_0000_0800);
    shard.assert_call("debug_move_item", &["1", "23", "25"]);
    let moved =
        shard.query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND slot = 25");
    assert_eq!(moved[0]["guid"], legacy.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn split_and_debug_equipment_allocate_distinct_items_after_slot_moves() {
    let shard = fixture("item-guid-split");
    shard.assert_call("debug_grant_item", &["1", "5090052", "5"]);
    let before = shard.query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")[0]
        ["guid"]
        .clone();
    shard.assert_call("debug_split_item", &["1", "23", "2", "24"]);
    let rows =
        shard.query_rows("SELECT guid,stack_count FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row["guid"] == before && row["stack_count"] == "3"));
    assert!(rows
        .iter()
        .any(|row| row["guid"] != before && row["stack_count"] == "2"));
    shard.assert_call("debug_equip_weapon", &["1", "5090050"]);
    shard.assert_call("debug_unequip_item", &["1", "15"]);
    shard.assert_call("debug_equip_weapon", &["1", "5090050"]);
    let weapons = shard.query_rows(
        "SELECT guid,slot FROM game_item_instance WHERE owner_guid = 1 AND entry = 5090050",
    );
    assert_eq!(weapons.len(), 2);
    assert_ne!(weapons[0]["guid"], weapons[1]["guid"]);
    assert!(weapons.iter().any(|row| row["slot"] == "15"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn trade_remints_for_each_recipient_before_deleting_outgoing_items() {
    let shard = fixture("item-guid-trade");
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    shard.assert_call("debug_grant_item", &["16777217", "5090052", "5"]);
    let before = shard.query_rows("SELECT guid FROM game_item_instance");
    shard.assert_sql("UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = 1");
    shard
        .assert_sql("UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = 16777217");
    let first = r#"{"guid":1,"ownership":null}"#;
    let second = r#"{"guid":16777217,"ownership":null}"#;
    shard.assert_call("gw_initiate_trade", &[first, "16777217"]);
    shard.assert_call("gw_begin_trade", &[second]);
    shard.assert_call("gw_set_trade_item", &[first, "0", "23"]);
    shard.assert_call("gw_set_trade_item", &[second, "0", "23"]);
    shard.assert_call("gw_accept_trade", &[first]);
    shard.assert_call("gw_accept_trade", &[second]);
    let after = shard.query_rows("SELECT guid,owner_guid,entry FROM game_item_instance");
    assert_eq!(after.len(), 2);
    for row in &after {
        assert!(!before.iter().any(|old| old["guid"] == row["guid"]));
        let guid: u64 = row["guid"].parse().unwrap();
        let owner: u64 = row["owner_guid"].parse().unwrap();
        assert_eq!((guid & ((1 << 47) - 1)) >> 11, owner);
        assert_eq!(row["entry"], if owner == 1 { "5090052" } else { "5090050" });
    }
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let items = shard.query_rows("SELECT guid FROM game_item_instance");
    assert_eq!(items.len(), 3);
}

fn freeze(shard: &Standalone, id: &str) -> String {
    shard.assert_call(
        "begin_transfer",
        &[
            id,
            r#"{"guid":1,"ownership":null}"#,
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "true",
        ],
    );
    let rows = shard.query_rows(&format!(
        "SELECT blob FROM game_transfer_out WHERE transfer_id = {id}"
    ));
    serde_json::to_string(rows[0]["blob"].strip_prefix("0x").unwrap()).unwrap()
}

fn finish(source: &Standalone, destination: &Standalone, id: &str) {
    let operator = r#"{"guid":0,"ownership":null}"#;
    source.assert_call("confirm_import", &[id, operator]);
    source.assert_call("finish_transfer", &[id, operator]);
    destination.assert_call("release_transfer", &[id, operator]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn transfer_refuses_legacy_collision_then_preserves_both_formats_on_round_trip() {
    let source = fixture("item-guid-transfer-source");
    source.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let legacy = 0x4000_0000_0000_0117u64;
    source.assert_sql(&format!(
        "UPDATE game_item_instance SET guid = {legacy} WHERE owner_guid = 1"
    ));
    source.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let before =
        source.query_rows("SELECT guid,entry,slot FROM game_item_instance WHERE owner_guid = 1");
    let blob = freeze(&source, "5090171");
    let destination = fixture("item-guid-transfer-destination");
    destination.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    destination.assert_call("debug_grant_item", &["16777217", "5090050", "1"]);
    destination.assert_sql(&format!(
        "UPDATE game_item_instance SET guid = {legacy} WHERE owner_guid = 16777217"
    ));
    let foreign =
        destination.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 16777217");
    let operator = r#"{"guid":0,"ownership":null}"#;
    let refusal = destination.call("import_character_blob", &["5090171", &blob, operator]);
    assert!(!refusal.status.success());
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    )
    .contains("ITEM_GUID_CONFLICT"));
    assert_eq!(
        destination.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 16777217"),
        foreign
    );
    assert!(destination
        .query_rows("SELECT transfer_id FROM game_transfer_in")
        .is_empty());
    assert_eq!(
        source.query_rows("SELECT guid,entry,slot FROM game_item_instance WHERE owner_guid = 1"),
        before
    );
    destination.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 16777217");
    destination.assert_call("import_character_blob", &["5090171", &blob, operator]);
    destination.assert_call("import_character_blob", &["5090171", &blob, operator]);
    assert_eq!(
        destination
            .query_rows("SELECT guid,entry,slot FROM game_item_instance WHERE owner_guid = 1"),
        before
    );
    finish(&source, &destination, "5090171");
    let back = freeze(&destination, "5090172");
    source.assert_call("import_character_blob", &["5090172", &back, operator]);
    finish(&destination, &source, "5090172");
    assert_eq!(
        source.query_rows("SELECT guid,entry,slot FROM game_item_instance WHERE owner_guid = 1"),
        before
    );
    source.assert_call("debug_spawn_player_entity", &["1"]);
    source.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    assert_eq!(
        source
            .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")
            .len(),
        3
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn exhaustion_preserves_partial_stacks_and_freed_identities_can_be_reused() {
    let shard = fixture("item-guid-exhaustion");
    // Park rows outside carried inventory to fill the identity block independently of slot capacity.
    for _ in 0..128 {
        shard.assert_call("debug_grant_item", &["1", "5090050", "16"]);
        shard.assert_sql("UPDATE game_item_instance SET slot = 255 WHERE owner_guid = 1");
    }
    let base = 0x4000_8000_0000_0800u64;
    shard.assert_sql("UPDATE game_item_template SET max_stack = 2 WHERE entry = 5090050");
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = 23 WHERE guid = {base}"
    ));
    let before =
        shard.query_rows("SELECT guid,stack_count FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(before.len(), 2048);
    let refusal = shard.call("debug_grant_item", &["1", "5090050", "2"]);
    assert!(!refusal.status.success());
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    )
    .contains("ITEM_GUID_EXHAUSTED"));
    assert_eq!(
        shard.query_rows("SELECT guid,stack_count FROM game_item_instance WHERE owner_guid = 1"),
        before
    );
    let freed = base + 123;
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE guid = {freed}"
    ));
    shard.assert_call("debug_grant_item", &["1", "5090050", "2"]);
    let reused = shard.query_rows(&format!(
        "SELECT slot,stack_count FROM game_item_instance WHERE guid = {freed}"
    ));
    assert_eq!(reused[0]["slot"], "24");
    assert_eq!(reused[0]["stack_count"], "1");
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE guid = {base}"
        ))[0]["stack_count"],
        "2"
    );
}
