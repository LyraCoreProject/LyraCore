mod support;

use support::Standalone;

fn fixture(name: &str) -> Standalone {
    fixture_in_range(name, "0")
}

fn fixture_in_range(name: &str, base: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &[base]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
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
    assert_eq!(fresh >> 47, 0x8001);
    assert!(fresh & ((1 << 47) - 1) < 1_000_000_000);
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
    let items_before = shard.query_rows("SELECT * FROM game_item_instance");
    let session_before = shard.query_rows("SELECT * FROM game_trade_session");
    let mark =
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"].clone();
    shard.assert_sql("UPDATE game_guid_allocator SET high_water = 999999998 WHERE id = 0");
    assert!(!shard.call("gw_accept_trade", &[second]).status.success());
    assert_eq!(
        shard.query_rows("SELECT * FROM game_item_instance"),
        items_before
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_trade_session"),
        session_before
    );
    assert_eq!(
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        "999999998"
    );
    shard.assert_sql(&format!(
        "UPDATE game_guid_allocator SET high_water = {mark} WHERE id = 0"
    ));
    shard.assert_call("gw_accept_trade", &[second]);
    let after = shard.query_rows("SELECT guid,owner_guid,entry FROM game_item_instance");
    assert_eq!(after.len(), 2);
    for row in &after {
        assert!(!before.iter().any(|old| old["guid"] == row["guid"]));
        let guid: u64 = row["guid"].parse().unwrap();
        let owner: u64 = row["owner_guid"].parse().unwrap();
        assert_eq!(guid >> 47, 0x8001);
        assert_eq!(row["entry"], if owner == 1 { "5090052" } else { "5090050" });
    }
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let items = shard.query_rows("SELECT guid FROM game_item_instance");
    assert_eq!(items.len(), 3);
}

fn freeze(shard: &Standalone, id: &str) -> String {
    freeze_character(shard, id, "1")
}

fn freeze_character(shard: &Standalone, id: &str, guid: &str) -> String {
    let actor = format!(r#"{{"guid":{guid},"ownership":null}}"#);
    shard.assert_call(
        "begin_transfer",
        &[id, &actor, "0", "0", "0", "0", "0", "0", "true"],
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
    let destination = fixture_in_range("item-guid-transfer-destination", "1000000000");
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
    source.assert_call("debug_grant_item", &["16777217", "5090050", "1"]);
    let next = source.query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 16777217");
    assert!(next.iter().all(|row| before
        .iter()
        .all(|departed| row["guid"] != departed["guid"])));
    destination.assert_call("debug_spawn_player_entity", &["1"]);
    destination.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    let before = destination
        .query_rows("SELECT guid,entry,slot FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(before.len(), 3);
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
        4
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn item_and_character_creation_share_exhaustion_without_changing_partial_stacks() {
    let shard = fixture("item-guid-exhaustion");
    shard.assert_call("debug_grant_item", &["1", "5090052", "1"]);
    shard.assert_sql("UPDATE game_item_template SET max_stack = 2 WHERE entry = 5090052");
    shard.assert_sql("UPDATE game_guid_allocator SET high_water = 999999998 WHERE id = 0");
    let before =
        shard.query_rows("SELECT guid,stack_count FROM game_item_instance WHERE owner_guid = 1");
    let refusal = shard.call("debug_grant_item", &["1", "5090052", "4"]);
    assert!(!refusal.status.success());
    assert_eq!(
        shard.query_rows("SELECT guid,stack_count FROM game_item_instance WHERE owner_guid = 1"),
        before
    );
    assert_eq!(
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        "999999998"
    );
    shard.assert_call("debug_grant_item", &["1", "5090052", "2"]);
    let issued =
        shard.query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND slot = 24");
    assert_eq!(
        issued[0]["guid"],
        (0x4000_8000_0000_0000u64 | 999_999_999).to_string()
    );
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    assert!(!shard
        .call("debug_grant_item", &["1", "5090050", "1"])
        .status
        .success());
    assert!(!shard
        .call(
            "create_character",
            &["1", "\"Full\"", "1", "1", "0", "0", "0", "0", "0", "0"]
        )
        .status
        .success());
    shard.assert_call("install_guid_range", &["0"]);
    assert_eq!(
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        "999999999"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn crafting_does_not_reuse_the_consumed_reagents_identity() {
    let shard = fixture("item-guid-craft");
    shard.assert_call("debug_grant_item", &["1", "5090052", "1"]);
    let old_guid = shard.query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")[0]
        ["guid"]
        .clone();
    seed_craft(&shard);
    shard.assert_call("debug_cast_at", &["1", "5090171", "1"]);
    let created =
        shard.query_rows("SELECT guid,entry FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["entry"], "5090050");
    assert_ne!(created[0]["guid"], old_guid);
}

fn seed_craft(shard: &Standalone) {
    shard.assert_sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090171,'Fixture craft',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)");
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360684,5090171,0,7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false)");
    shard.assert_sql("INSERT INTO game_spell_reagent (id,spell_id,item_entry,count) VALUES (40721368,5090171,5090052,1)");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_failed_later_craft_product_restores_reagents_without_skill_or_partial_products() {
    let shard = fixture("item-guid-craft-refund");
    seed_craft(&shard);
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360685,5090171,1,7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false)");
    shard.assert_sql("INSERT INTO game_skill_ability (id,spell_id,skill_line,race_mask,class_mask,min_skill,acquire_method,gray,green) VALUES (5090171,5090171,185,0,0,0,0,0,0)");
    shard.assert_call("debug_set_skill", &["1", "185", "1"]);
    shard.assert_call("debug_grant_item", &["1", "5090052", "1"]);
    shard.assert_sql("UPDATE game_guid_allocator SET high_water = 999999998 WHERE id = 0");
    let items = shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1");
    let skills = shard.query_rows("SELECT * FROM game_player_skill WHERE character_guid = 1");
    let spells = shard.query_rows("SELECT * FROM game_player_spell WHERE character_guid = 1");
    shard.assert_call("debug_cast_at", &["1", "5090171", "1"]);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1"),
        items
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_player_skill WHERE character_guid = 1"),
        skills
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_player_spell WHERE character_guid = 1"),
        spells
    );
    assert_eq!(
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        "999999999"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_high_range_character_arrives_and_receives_items_from_the_destinations_own_range() {
    let source = fixture_in_range("item-guid-high-source", "1000000000");
    source.assert_call(
        "create_character",
        &["1", "\"Traveler\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let traveler = source.query_rows("SELECT guid FROM game_character WHERE name = 'Traveler'")[0]
        ["guid"]
        .clone();
    assert!(traveler.parse::<u64>().unwrap() > 1_000_000_000);
    let before = source.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = {traveler}"
    ));
    let blob = freeze_character(&source, "5090173", &traveler);
    let destination = fixture("item-guid-low-destination");
    let mark: u64 = destination.query_rows("SELECT high_water FROM game_guid_allocator")[0]
        ["high_water"]
        .parse()
        .unwrap();
    destination.assert_call(
        "import_character_blob",
        &["5090173", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    finish(&source, &destination, "5090173");
    assert_eq!(
        destination.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        mark.to_string()
    );
    destination.assert_call("debug_spawn_player_entity", &[&traveler]);
    destination.assert_call("debug_grant_item", &[&traveler, "5090050", "1"]);
    assert_eq!(
        destination.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        (mark + 1).to_string()
    );
    let after = destination.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = {traveler}"
    ));
    assert_eq!(after.len(), before.len() + 1);
    assert!(before.iter().all(|row| after.contains(row)));
    let fresh = (0x4000_8000_0000_0000u64 | (mark + 1)).to_string();
    assert!(after.iter().any(|row| row["guid"] == fresh));
    let back = freeze_character(&destination, "5090174", &traveler);
    source.assert_call(
        "import_character_blob",
        &["5090174", &back, r#"{"guid":0,"ownership":null}"#],
    );
    finish(&destination, &source, "5090174");
    assert_eq!(
        destination.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        (mark + 1).to_string()
    );
    destination.assert_call("debug_grant_item", &["16777217", "5090050", "1"]);
    assert_eq!(
        destination.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        (mark + 2).to_string()
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn item_format_exhaustion_does_not_advance_the_range() {
    let shard = fixture_in_range("item-guid-format-limit", "140737000000000");
    shard.assert_sql("UPDATE game_guid_allocator SET high_water = 140737488355327 WHERE id = 0");
    let refusal = shard.call("debug_grant_item", &["1", "5090050", "1"]);
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("ITEM_GUID_EXHAUSTED"));
    assert_eq!(
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        "140737488355327"
    );
    assert!(shard
        .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_local_legacy_collision_refuses_without_replacing_either_owners_items() {
    let shard = fixture("item-guid-local-conflict");
    shard.assert_call("debug_grant_item", &["16777217", "5090050", "1"]);
    let mark: u64 = shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"]
        .parse()
        .unwrap();
    let collision = 0x4000_8000_0000_0000u64 | (mark + 1);
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET guid = {collision} WHERE owner_guid = 16777217"
    ));
    let before = shard.query_rows("SELECT * FROM game_item_instance");
    let refusal = shard.call("debug_grant_item", &["1", "5090050", "1"]);
    assert!(!refusal.status.success());
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    )
    .contains("ITEM_GUID_CONFLICT"));
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), before);
    assert_eq!(
        shard.query_rows("SELECT high_water FROM game_guid_allocator")[0]["high_water"],
        mark.to_string()
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn legacy_deletion_preserves_its_floor_before_range_installation() {
    let mut shard = Standalone::start("item-guid-legacy-floor");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_delete_character", &["1"]);
    let marks = shard.query_rows("SELECT high_water FROM game_guid_allocator");
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0]["high_water"], "1");
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call(
        "create_character",
        &[
            "1",
            "\"AfterDelete\"",
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
    let created = shard.query_rows("SELECT guid FROM game_character WHERE name = 'AfterDelete'");
    assert_eq!(created[0]["guid"], "2");
}
