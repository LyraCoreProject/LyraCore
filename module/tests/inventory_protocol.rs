mod support;

use lyracore_shared::trade::{decode_offer, event_kind};
use support::{number, poll_until, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_item_instance");
    shard
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn inventory_changes_preserve_accepted_trade_items() {
    let shard = fixture("inventory-trade-offer");
    shard.assert_call(
        "create_character",
        &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let other =
        shard.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")[0]["guid"].clone();
    shard.assert_call("debug_spawn_player_entity", &[&other]);
    shard.assert_sql("UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 100, y = 100, z = 100, money = 100 WHERE guid = {other}"
    ));
    shard.assert_sql("UPDATE game_world_entity SET money = 0 WHERE guid = 1");
    shard.assert_sql("UPDATE game_world_entity SET health = 1 WHERE guid = 1");
    shard.assert_call("debug_grant_item", &["1", "5090052", "15"]);
    shard.assert_call("debug_split_item", &["1", "23", "5", "24"]);
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    shard.assert_call("gw_move_item", &[ACTOR, "25", "26"]);
    shard.assert_call("debug_spawn_at_feet", &["1", "51003", "0"]);
    let vendor = shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51003")[0]
        ["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 4 WHERE guid = {vendor}"
    ));
    let buyer = format!(r#"{{"guid":{other},"ownership":null}}"#);
    shard.assert_call("gw_initiate_trade", &[ACTOR, &other]);
    shard.assert_call("gw_begin_trade", &[&buyer]);
    shard.assert_call("gw_set_trade_item", &[ACTOR, "0", "23"]);
    shard.assert_call("gw_set_trade_item", &[ACTOR, "1", "26"]);
    shard.assert_call("gw_set_trade_gold", &[&buyer, "100"]);
    shard.assert_call("gw_accept_trade", &[&buyer]);
    let items = shard.query_rows("SELECT * FROM game_item_instance");
    let session = shard.query_rows("SELECT * FROM game_trade_session");
    assert_eq!(session[0]["target_accepted"], "true");
    shard.assert_sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090171,'Fixture craft',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)");
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360684,5090171,0,7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false)");
    shard.assert_sql("INSERT INTO game_spell_reagent (id,spell_id,item_entry,count) VALUES (40721368,5090171,5090052,1)");
    for (verb, args) in [
        ("gw_split_item", vec![ACTOR, "23", "25", "9"]),
        ("gw_destroy_item", vec![ACTOR, "23", "9"]),
        ("gw_destroy_item", vec![ACTOR, "23", "0"]),
        ("gw_move_item", vec![ACTOR, "23", "25"]),
        ("gw_move_item", vec![ACTOR, "24", "23"]),
        ("gw_use_item", vec![ACTOR, "23"]),
        ("gw_sell_item", vec![ACTOR, &vendor, "23"]),
        ("gw_disenchant", vec![ACTOR, "26"]),
        ("debug_cast_at", vec!["1", "5090171", "1"]),
    ] {
        let result = shard.call(verb, &args);
        assert!(!result.status.success(), "{verb} changed an offered item");
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(output.contains("item:not_right_now"), "{output}");
        assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), items);
        assert_eq!(
            shard.query_rows("SELECT * FROM game_trade_session"),
            session
        );
    }
    shard.assert_call("debug_grant_item", &["1", "5090054", "1"]);
    shard.assert_sql("UPDATE game_spell SET cast_time_ms = 1000, cost = 10, power_type = 1 WHERE spell_id = 5090171");
    shard.assert_sql("UPDATE game_spell_reagent SET item_entry = 5090054 WHERE id = 40721368");
    shard.assert_sql("INSERT INTO game_spell_reagent (id,spell_id,item_entry,count) VALUES (40721369,5090171,5090052,1)");
    // Keep regeneration separate from the scheduled craft's power accounting.
    shard.assert_sql("DELETE FROM game_creature_move_schedule");
    shard.assert_sql("UPDATE game_world_entity SET power = 100 WHERE guid = 1");
    let before_craft = shard.query_rows("SELECT * FROM game_item_instance");
    shard.assert_call("debug_begin_cast", &["1", "5090171", "1"]);
    assert_eq!(
        shard
            .query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
            .len(),
        1
    );
    assert!(poll_until(std::time::Duration::from_secs(5), || shard
        .query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
        .is_empty()));
    assert_eq!(
        shard.query_rows("SELECT * FROM game_item_instance"),
        before_craft
    );
    assert_eq!(
        shard.query_rows("SELECT power FROM game_world_entity WHERE guid = 1")[0]["power"],
        "100"
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_trade_session"),
        session
    );
    shard.assert_sql("DELETE FROM game_spell_reagent WHERE id = 40721369");
    shard.assert_sql("UPDATE game_spell_effect SET p0 = 5090052 WHERE id = 20360684");
    shard.assert_call("debug_begin_cast", &["1", "5090171", "1"]);
    assert_eq!(
        shard
            .query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
            .len(),
        1
    );
    assert!(poll_until(std::time::Duration::from_secs(5), || shard
        .query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
        .is_empty()));
    assert_eq!(
        shard.query_rows("SELECT power FROM game_world_entity WHERE guid = 1")[0]["power"],
        "90"
    );
    assert_eq!(
        shard.query_rows(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = 1 AND slot = 23"
        )[0]["stack_count"],
        "11"
    );
    let changed = shard.query_rows("SELECT * FROM game_trade_session");
    assert_eq!(changed[0]["target_accepted"], "false");
    assert_eq!(changed[0]["initiator_accepted"], "false");
    shard.assert_call("debug_grant_item", &["1", "5090054", "1"]);
    shard.assert_call("gw_accept_trade", &[&buyer]);
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360685,5090171,1,7,1,0,0.0,0,0,0.0,0,0,0,5090999,8,0,0,false)");
    let before_failed_product = shard.query_rows("SELECT * FROM game_item_instance");
    // Retain offer events while the test inspects their final quantities.
    shard.assert_sql("DELETE FROM game_event_reaper_schedule");
    shard.assert_sql("DELETE FROM game_trade_event");
    shard.assert_call("debug_begin_cast", &["1", "5090171", "1"]);
    assert!(poll_until(std::time::Duration::from_secs(5), || shard
        .query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
        .is_empty()));
    assert_eq!(
        shard.query_rows("SELECT * FROM game_item_instance"),
        before_failed_product
    );
    let changed = shard.query_rows("SELECT * FROM game_trade_session");
    assert_eq!(changed[0]["target_accepted"], "false");
    assert_eq!(changed[0]["initiator_accepted"], "false");
    let events = shard.query_rows("SELECT * FROM game_trade_event");
    for kind in [event_kind::OFFER_SELF, event_kind::OFFER_PARTNER] {
        let last_offer = events
            .iter()
            .filter(|event| number::<u8>(event, "kind") == kind)
            .max_by_key(|event| number::<u64>(event, "id"))
            .unwrap();
        let (_, offer) = decode_offer(&last_offer["payload"]).unwrap();
        assert_eq!(
            offer
                .iter()
                .find(|slot| slot.trade_slot == 0)
                .unwrap()
                .stack_count,
            11
        );
    }
    shard.assert_call("gw_split_item", &[ACTOR, "24", "27", "1"]);
    shard.assert_call("gw_accept_trade", &[ACTOR]);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_trade_session").len(),
        1
    );
    shard.assert_call("gw_accept_trade", &[&buyer]);
    let received = shard.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {other} AND entry = 5090052"
    ));
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["stack_count"], "11");
    assert_eq!(shard.query_rows(&format!("SELECT stack_count FROM game_item_instance WHERE owner_guid = {other} AND entry = 5090050"))[0]["stack_count"], "1");
    assert_eq!(
        shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1")[0]["money"],
        "100"
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT money FROM game_world_entity WHERE guid = {other}"
        ))[0]["money"],
        "0"
    );
    assert!(shard
        .query_rows("SELECT * FROM game_trade_session")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn two_handed_weapons_and_offhand_items_cannot_be_equipped_together() {
    let shard = fixture("inventory-two-hand");
    shard.assert_sql("UPDATE game_item_template SET class = 2, subclass = 8, inventory_type = 17 WHERE entry = 5090050");
    shard.assert_sql("UPDATE game_item_template SET class = 4, subclass = 6, inventory_type = 14 WHERE entry = 5090054");
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    shard.assert_call("debug_grant_item", &["1", "5090054", "1"]);
    shard.assert_call("gw_move_item", &[ACTOR, "24", "16"]);
    let before = shard.query_rows("SELECT * FROM game_item_instance");
    assert!(!shard.call("gw_equip_item", &[ACTOR, "23"]).status.success());
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), before);
    shard.assert_call("gw_move_item", &[ACTOR, "16", "24"]);
    shard.assert_call("gw_equip_item", &[ACTOR, "23"]);
    let before = shard.query_rows("SELECT * FROM game_item_instance");
    assert!(!shard.call("gw_equip_item", &[ACTOR, "24"]).status.success());
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), before);
    shard.assert_call("gw_move_item", &[ACTOR, "15", "23"]);
    shard.assert_call("gw_equip_item", &[ACTOR, "24"]);
}
