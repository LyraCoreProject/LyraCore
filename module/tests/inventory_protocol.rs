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
    shard.assert_sql(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 1 WHERE guid = 1",
    );
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
    shard.assert_sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES (5090171,'Fixture craft',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0)");
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES (20360684,5090171,0,7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false,0.0)");
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
    shard.assert_sql("DELETE FROM game_creature_move_schedule");
    shard.assert_sql(
        "UPDATE game_world_entity SET simulation_indexed = false, power = 100 WHERE guid = 1",
    );
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
    shard.assert_call("debug_grant_item", &["1", "5090054", "1"]);
    shard.assert_call("gw_accept_trade", &[&buyer]);
    shard.assert_sql("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES (20360685,5090171,1,7,1,0,0.0,0,0,0.0,0,0,0,5090999,8,0,0,false,0.0)");
    let before_failed_product = shard.query_rows("SELECT * FROM game_item_instance");
    let accepted = shard.query_rows("SELECT * FROM game_trade_session");
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
    assert_eq!(
        shard.query_rows("SELECT * FROM game_trade_session"),
        accepted
    );
    assert!(shard
        .query_rows("SELECT * FROM game_trade_event")
        .is_empty());
    shard.assert_call("gw_split_item", &[ACTOR, "24", "27", "1"]);
    shard.assert_call("gw_accept_trade", &[ACTOR]);
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

fn assert_refused(shard: &Standalone, verb: &str, args: &[&str], refusal: &str) {
    let before = shard.query_rows("SELECT * FROM game_item_instance");
    let result = shard.call(verb, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.status.success(), "{verb} {args:?} succeeded");
    assert!(output.contains(refusal), "{verb} {args:?}: {output}");
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), before);
}

fn slot_of(shard: &Standalone, entry: u32) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = {entry}"
        ))
        .into_iter()
        .map(|row| row["slot"].clone())
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_two_handed_weapon_moves_the_off_hand_item_to_a_free_slot() {
    const TWO_HANDER: u32 = 5090050;
    const ONE_HANDER: u32 = 5090052;
    const SHIELD: u32 = 5090054;
    let shard = fixture("inventory-two-hand");
    shard.assert_call("debug_learn_spell", &["1", "201"]);
    shard.assert_call("debug_learn_spell", &["1", "202"]);
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 2, subclass = 8, inventory_type = 17 WHERE entry = {TWO_HANDER}"));
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 2, subclass = 7, inventory_type = 13, max_stack = 1 WHERE entry = {ONE_HANDER}"));
    shard.assert_sql(&format!("UPDATE game_item_template SET class = 4, subclass = 6, inventory_type = 14 WHERE entry = {SHIELD}"));
    shard.assert_call("debug_grant_item", &["1", &TWO_HANDER.to_string(), "1"]);
    shard.assert_call("debug_grant_item", &["1", &SHIELD.to_string(), "1"]);
    shard.assert_call("gw_move_item", &[ACTOR, "24", "16"]);

    shard.assert_call("gw_equip_item", &[ACTOR, "23"]);
    assert_eq!(slot_of(&shard, TWO_HANDER), ["15"]);
    assert_eq!(slot_of(&shard, SHIELD), ["23"]);

    assert_refused(
        &shard,
        "gw_equip_item",
        &[ACTOR, "23"],
        "item:two_handed_weapon_equipped",
    );
    assert_refused(
        &shard,
        "gw_move_item",
        &[ACTOR, "23", "16"],
        "item:two_handed_weapon_equipped",
    );

    shard.assert_call("gw_move_item", &[ACTOR, "15", "24"]);
    shard.assert_call("gw_move_item", &[ACTOR, "23", "16"]);
    shard.assert_call("debug_grant_item", &["1", &ONE_HANDER.to_string(), "15"]);
    shard.assert_call("gw_move_item", &[ACTOR, "38", "15"]);
    shard.assert_call("debug_grant_item", &["1", &ONE_HANDER.to_string(), "1"]);
    assert_refused(
        &shard,
        "gw_move_item",
        &[ACTOR, "24", "15"],
        "item:inventory_full",
    );
    assert_refused(
        &shard,
        "gw_equip_item",
        &[ACTOR, "24"],
        "item:inventory_full",
    );

    shard.assert_call("gw_destroy_item", &[ACTOR, "30", "0"]);
    shard.assert_call("gw_move_item", &[ACTOR, "24", "15"]);
    assert_eq!(slot_of(&shard, TWO_HANDER), ["15"]);
    assert_eq!(slot_of(&shard, SHIELD), ["30"]);
    assert_eq!(
        shard.query_rows("SELECT entry FROM game_item_instance WHERE owner_guid = 1 AND slot = 24")
            [0]["entry"],
        ONE_HANDER.to_string()
    );
}
