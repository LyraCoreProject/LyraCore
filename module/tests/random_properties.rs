mod support;

use support::Standalone;

const ITEM: &str = "5090050";
const POOL: &str = "5090100";
const PROPERTY: &str = "5090101";

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    shard.assert_sql("INSERT INTO game_item_random_property (property_id,enchant_id_1,enchant_id_2,enchant_id_3,suffix) VALUES (5090101,5090103,0,0,'of the Fixture'),(5090102,0,0,0,'of the Other Fixture')");
    shard.assert_sql("INSERT INTO game_item_enchantment (id,enchant_id,effect_index,kind,amount,spell_id,school_mask) VALUES (1303066368,5090103,0,3,7,0,0),(1303066369,5090103,1,5,9,0,0)");
    shard.assert_sql("INSERT INTO game_item_property_weight (id,pool_id,property_id,weight) VALUES (5090100,5090100,5090101,10000)");
    shard.assert_sql(&format!("UPDATE game_item_template SET random_property = {POOL}, stat_stamina = 0, bonding = 0 WHERE entry = {ITEM}"));
    shard
}

fn max_health(shard: &Standalone) -> u32 {
    shard.query_rows("SELECT max_health FROM game_world_entity WHERE guid = 1")[0]["max_health"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn loot_keeps_its_property_after_the_pool_changes_and_equipped_stats_follow_it() {
    let shard = fixture("property-loot");
    shard.assert_sql(&format!("INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090100,51000,{ITEM},10000,1,0,false)"));
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf =
        shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND dead = false");
    shard.assert_call("debug_apply_damage", &[&wolf[0]["guid"], "100", "1"]);
    assert_eq!(
        shard
            .query_rows("SELECT character_guid FROM game_creature_quest_tap")
            .len(),
        1,
        "damage must acquire the Loot Tag"
    );
    shard.assert_call("debug_kill_nearest", &["1", "51000"]);
    let loot = shard.query_rows(&format!(
        "SELECT * FROM game_corpse_loot WHERE item_entry = {ITEM}"
    ));
    assert_eq!(loot.len(), 1);
    assert_eq!(loot[0]["random_property_id"], PROPERTY);
    shard.assert_sql(
        "UPDATE game_item_property_weight SET property_id = 5090102 WHERE id = 5090100",
    );
    shard.assert_call(
        "debug_take_loot",
        &["1", &loot[0]["corpse_guid"], &loot[0]["slot"]],
    );
    let item = shard.query_rows(&format!(
        "SELECT * FROM game_item_instance WHERE owner_guid = 1 AND entry = {ITEM}"
    ));
    assert_eq!(item.len(), 1);
    assert_eq!(item[0]["random_property_id"], PROPERTY);
    shard.assert_sql("UPDATE game_world_entity SET stamina = 22 WHERE guid = 1");
    let base = max_health(&shard);
    let base_spirit: u32 = shard.query_rows("SELECT spirit FROM game_world_entity WHERE guid = 1")
        [0]["spirit"]
        .parse()
        .unwrap();
    shard.assert_call("debug_equip_item", &["1", &item[0]["slot"]]);
    assert_eq!(
        max_health(&shard),
        base + 70,
        "seven Stamina above the base curve adds 70 health"
    );
    let spirit: u32 = shard.query_rows("SELECT spirit FROM game_world_entity WHERE guid = 1")[0]
        ["spirit"]
        .parse()
        .unwrap();
    assert_eq!(spirit, base_spirit + 9);
    shard.assert_sql(
        "UPDATE game_item_instance SET enchant_id = 7748 WHERE owner_guid = 1 AND slot = 15",
    );
    shard.assert_call("debug_unequip_item", &["1", "15"]);
    shard.assert_call("debug_equip_item", &["1", "23"]);
    assert_eq!(
        max_health(&shard),
        base + 100,
        "the existing +3 Stamina enchant adds to the property"
    );
    shard.assert_sql(
        "UPDATE game_item_instance SET durability = 0 WHERE owner_guid = 1 AND slot = 15",
    );
    shard.assert_call("debug_unequip_item", &["1", "15"]);
    shard.assert_call("debug_equip_item", &["1", "23"]);
    assert_eq!(
        max_health(&shard),
        base,
        "broken items grant neither overlay"
    );
    let spirit: u32 = shard.query_rows("SELECT spirit FROM game_world_entity WHERE guid = 1")[0]
        ["spirit"]
        .parse()
        .unwrap();
    assert_eq!(spirit, base_spirit);
}

fn create_spell(shard: &Standalone) {
    shard.assert_sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090100,'Fixture craft',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)");
    shard.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360400,5090100,0,7,2,0,0.0,0,0,0.0,0,0,0,{ITEM},8,0,0,false)"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_missing_property_pool_does_not_leave_an_empty_loot_cursor() {
    let shard = fixture("property-missing-loot-pool");
    shard.assert_sql(
        "UPDATE game_creature_template SET money_min = 0, money_max = 0 WHERE entry = 51000",
    );
    shard.assert_sql("DELETE FROM game_item_property_weight WHERE id = 5090100");
    shard.assert_sql("DELETE FROM game_creature_loot WHERE creature_entry = 51000");
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    shard.assert_sql(&format!("INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090100,51000,{ITEM},10000,1,0,false)"));
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf = shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000");
    let guid = &wolf[0]["guid"];
    shard.assert_call("debug_apply_damage", &[guid, "10", "1"]);
    shard.assert_call("debug_kill_creature", &["1", guid]);
    assert!(shard
        .query_rows("SELECT id FROM game_corpse_loot")
        .is_empty());
    let corpse =
        shard.query_rows("SELECT dynamic_flags FROM game_world_entity WHERE entry = 51000");
    let flags: u32 = corpse[0]["dynamic_flags"].parse().unwrap();
    assert_eq!(flags & 1, 0, "an omitted drop must not set LOOTABLE");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn invalid_creature_drops_leave_valid_loot_at_slot_zero() {
    let shard = fixture("property-mixed-creature");
    shard.assert_sql("DELETE FROM game_item_property_weight WHERE id = 5090100");
    shard.assert_sql("DELETE FROM game_creature_spawn WHERE entry = 51000");
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    shard.assert_sql("DELETE FROM game_creature_loot WHERE creature_entry = 51000");
    shard.assert_sql(&format!("INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090100,51000,{ITEM},10000,1,0,false),(5090101,51000,5090052,10000,2,0,false)"));
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "1"]);
    let wolf = shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000");
    assert_eq!(wolf.len(), 1);
    shard.assert_call("debug_apply_damage", &[&wolf[0]["guid"], "10", "1"]);
    shard.assert_call("debug_kill_creature", &["1", &wolf[0]["guid"]]);
    let loot =
        shard.query_rows("SELECT slot,item_entry,count,random_property_id FROM game_corpse_loot");
    assert_eq!(loot.len(), 1);
    assert_eq!(loot[0]["slot"], "0");
    assert_eq!(loot[0]["item_entry"], "5090052");
    assert_eq!(loot[0]["count"], "2");
    assert_eq!(loot[0]["random_property_id"], "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn invalid_chest_properties_preserve_valid_drops_and_finish_empty_fallbacks() {
    let shard = fixture("property-mixed-chest");
    shard.assert_sql("UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = 1");
    shard.assert_sql("DELETE FROM game_item_property_weight WHERE id = 5090100");
    shard.assert_sql(&format!("INSERT INTO game_gameobject_loot (id,loot_id,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090100,5090100,{ITEM},10000,1,0,false),(5090101,5090100,5090052,10000,2,0,false)"));
    for (entry, loot_id) in [("5090110", "5090100"), ("5090111", "0")] {
        shard.assert_call(
            "debug_spawn_gameobject",
            &[
                entry, "3", "0", ITEM, "0", "100", "100", "100", loot_id, "0", "0", "0",
            ],
        );
        shard.assert_call("debug_use_gameobject_entry", &["1", entry]);
        let chest = shard.query_rows(&format!(
            "SELECT guid,state FROM game_gameobject WHERE template_entry = {entry}"
        ));
        assert_eq!(chest[0]["state"], "1");
        let loot = shard.query_rows(&format!("SELECT slot,item_entry,count,random_property_id FROM game_corpse_loot WHERE corpse_guid = {}", chest[0]["guid"]));
        if loot_id == "0" {
            assert!(loot.is_empty());
        } else {
            assert_eq!(loot.len(), 1);
            assert_eq!(loot[0]["slot"], "0");
            assert_eq!(loot[0]["item_entry"], "5090052");
            assert_eq!(loot[0]["count"], "2");
            assert_eq!(loot[0]["random_property_id"], "0");
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn large_imported_spirit_contributions_do_not_overflow_equipping() {
    let shard = fixture("property-spirit-bound");
    let base: u32 = shard.query_rows("SELECT spirit FROM game_world_entity WHERE guid = 1")[0]
        ["spirit"]
        .parse()
        .unwrap();
    shard.assert_sql("UPDATE game_item_enchantment SET amount = 2147483647 WHERE id = 1303066369");
    shard.assert_call("debug_grant_item", &["1", ITEM, "1"]);
    shard.assert_call("debug_equip_item", &["1", "23"]);
    let spirit: u32 = shard.query_rows("SELECT spirit FROM game_world_entity WHERE guid = 1")[0]
        ["spirit"]
        .parse()
        .unwrap();
    assert_eq!(spirit, base + 2_147_483_647);
    shard.assert_call("debug_unequip_item", &["1", "15"]);
    let spirit: u32 = shard.query_rows("SELECT spirit FROM game_world_entity WHERE guid = 1")[0]
        ["spirit"]
        .parse()
        .unwrap();
    assert_eq!(spirit, base);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn creation_is_atomic_even_when_the_spell_handles_a_full_inventory() {
    let shard = fixture("property-create");
    create_spell(&shard);
    shard.assert_call("debug_grant_item", &["1", ITEM, "15"]);
    let before = shard.query_rows(
        "SELECT guid,stack_count,random_property_id FROM game_item_instance WHERE owner_guid = 1",
    );
    assert_eq!(before.len(), 15);
    shard.assert_call("debug_cast_at", &["1", "5090100", "1"]);
    assert_eq!(shard.query_rows("SELECT guid,stack_count,random_property_id FROM game_item_instance WHERE owner_guid = 1"), before);
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    shard.assert_call("debug_cast_at", &["1", "5090100", "1"]);
    let created =
        shard.query_rows("SELECT random_property_id FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(created.len(), 2);
    assert!(created
        .iter()
        .all(|item| item["random_property_id"] == PROPERTY));
    shard.assert_sql("DELETE FROM game_item_property_weight WHERE id = 5090100");
    let refused = shard.call("debug_grant_item", &["1", ITEM, "1"]);
    assert!(!refused.status.success());
    assert_eq!(
        shard.query_rows("SELECT random_property_id FROM game_item_instance WHERE owner_guid = 1"),
        created
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn splitting_and_moving_stacks_never_mix_properties() {
    let shard = fixture("property-stacks");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_stack = 20 WHERE entry = {ITEM}"
    ));
    shard.assert_call("debug_grant_item", &["1", ITEM, "5"]);
    shard.assert_call("debug_split_item", &["1", "23", "2", "24"]);
    let split = shard.query_rows(
        "SELECT stack_count,random_property_id FROM game_item_instance WHERE owner_guid = 1",
    );
    assert_eq!(split.len(), 2);
    assert!(split
        .iter()
        .all(|item| item["random_property_id"] == PROPERTY));
    shard.assert_sql(
        "UPDATE game_item_property_weight SET property_id = 5090102 WHERE id = 5090100",
    );
    shard.assert_call("debug_grant_item", &["1", ITEM, "4"]);
    shard.assert_call("debug_move_item", &["1", "25", "24"]);
    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")
            .len(),
        3
    );
    shard.assert_call("debug_move_item", &["1", "25", "23"]);
    let merged = shard.query_rows(
        "SELECT stack_count,random_property_id FROM game_item_instance WHERE owner_guid = 1",
    );
    assert_eq!(merged.len(), 2);
    assert!(merged
        .iter()
        .any(|item| item["random_property_id"] == PROPERTY && item["stack_count"] == "5"));
    assert!(merged
        .iter()
        .any(|item| item["random_property_id"] == "5090102" && item["stack_count"] == "4"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn mail_escrow_and_replayed_payout_preserve_both_plain_and_random_items() {
    let shard = fixture("property-mail");
    for (escrow_id, property) in [(5090110, PROPERTY), (5090120, "0")] {
        let send = escrow_id.to_string();
        shard.assert_call(
            "realm_mail_commit",
            &[
                &send,
                r#"{"guid":2,"ownership":null}"#,
                "1",
                "\"Fixture\"",
                "\"\"",
                "0",
                ITEM,
                "1",
                "42",
                "7748",
                "false",
                property,
                "0",
                "0",
            ],
        );
        let mails = shard.query_rows("SELECT id,random_property_id FROM game_mail WHERE recipient_guid = 1 AND item_entry = 5090050");
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0]["random_property_id"], property);
        let payout = (escrow_id + 1).to_string();
        shard.assert_call(
            "realm_mail_take_item_fence",
            &[
                &payout,
                r#"{"guid":1,"ownership":null}"#,
                &mails[0]["id"],
                ITEM,
            ],
        );
        let fenced = shard.query_rows(&format!(
            "SELECT random_property_id FROM game_mail_escrow WHERE escrow_id = {payout}"
        ));
        assert_eq!(fenced[0]["random_property_id"], property);
        let args = [
            &payout,
            r#"{"guid":1,"ownership":null}"#,
            &mails[0]["id"],
            ITEM,
            "1",
            "42",
            "7748",
            "false",
            property,
        ];
        shard.assert_call("realm_mail_item_payout", &args);
        shard.assert_call("realm_mail_item_payout", &args);
        let items = shard.query_rows("SELECT random_property_id,enchant_id,durability FROM game_item_instance WHERE owner_guid = 1");
        assert_eq!(items.len(), 1, "payout replay must not duplicate the item");
        assert_eq!(items[0]["random_property_id"], property);
        assert_eq!(items[0]["enchant_id"], "7748");
        assert_eq!(items[0]["durability"], "42");
        shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn buyback_restores_the_saved_property_without_reading_the_current_pool() {
    let shard = fixture("property-buyback");
    shard.assert_call("debug_spawn_at_feet", &["1", "51004", "1"]);
    let vendor = shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51004");
    for property in [PROPERTY, "0"] {
        shard.assert_call("debug_grant_item", &["1", ITEM, "1"]);
        shard.assert_sql(&format!(
            "UPDATE game_item_instance SET random_property_id = {property} WHERE owner_guid = 1"
        ));
        shard.assert_call("debug_sell_item", &["1", &vendor[0]["guid"], "23"]);
        let sold = shard.query_rows(
            "SELECT random_property_id FROM game_character_buyback WHERE player_guid = 1",
        );
        assert_eq!(sold.len(), 1);
        assert_eq!(sold[0]["random_property_id"], property);
        shard.assert_sql(&format!(
            "UPDATE game_item_template SET random_property = 5090199 WHERE entry = {ITEM}"
        ));
        shard.assert_call(
            "gw_buyback_item",
            &[r#"{"guid":1,"ownership":null}"#, &vendor[0]["guid"], "0"],
        );
        let restored = shard
            .query_rows("SELECT random_property_id FROM game_item_instance WHERE owner_guid = 1");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0]["random_property_id"], property);
        assert!(shard
            .query_rows("SELECT id FROM game_character_buyback WHERE player_guid = 1")
            .is_empty());
        shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1");
        shard.assert_sql(&format!(
            "UPDATE game_item_template SET random_property = {POOL} WHERE entry = {ITEM}"
        ));
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn auction_refund_replay_compares_the_saved_property() {
    let shard = fixture("property-auction-refund");
    let mut args = [
        "5090140",
        r#"{"guid":1,"ownership":null}"#,
        "5090141",
        ITEM,
        "1",
        "42",
        "7748",
        "false",
        PROPERTY,
        "1",
        "5",
        "5",
        "100",
        "500",
        "120",
        "10",
        "1",
        "7200000001",
    ];
    shard.assert_call("realm_auction_refund_listing", &args);
    shard.assert_call("realm_auction_refund_listing", &args);
    let mails = shard.query_rows("SELECT random_property_id,item_enchant_id,item_durability,money FROM game_mail WHERE recipient_guid = 1");
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0]["random_property_id"], PROPERTY);
    assert_eq!(mails[0]["item_enchant_id"], "7748");
    assert_eq!(mails[0]["item_durability"], "42");
    assert_eq!(mails[0]["money"], "10");
    args[8] = "0";
    assert!(!shard
        .call("realm_auction_refund_listing", &args)
        .status
        .success());
    assert_eq!(shard.query_rows("SELECT random_property_id,item_enchant_id,item_durability,money FROM game_mail WHERE recipient_guid = 1"), mails);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn cross_shard_transfer_imports_the_saved_item_property() {
    let source = fixture("property-transfer-source");
    source.assert_call("debug_grant_item", &["1", ITEM, "1"]);
    source.assert_call(
        "begin_transfer",
        &[
            "5090150",
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
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 5090150");
    let mut destination = Standalone::start("property-transfer-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    destination.assert_call(
        "import_character_blob",
        &["5090150", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    destination.assert_call(
        "import_character_blob",
        &["5090150", &blob, r#"{"guid":0,"ownership":null}"#],
    );
    let items = destination
        .query_rows("SELECT random_property_id FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["random_property_id"], PROPERTY);
    source.assert_call(
        "confirm_import",
        &["5090150", r#"{"guid":0,"ownership":null}"#],
    );
    source.assert_call(
        "finish_transfer",
        &["5090150", r#"{"guid":0,"ownership":null}"#],
    );
    destination.assert_call(
        "release_transfer",
        &["5090150", r#"{"guid":0,"ownership":null}"#],
    );
    assert!(source
        .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")
        .is_empty());
    assert_eq!(
        destination
            .query_rows("SELECT random_property_id FROM game_item_instance WHERE owner_guid = 1"),
        items
    );
}
