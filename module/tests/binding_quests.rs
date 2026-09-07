mod support;

use std::collections::BTreeMap;
use support::{poll_until, Standalone, POLL_TIMEOUT};

const PLAYER: &str = "1";
const FOCUS: u32 = 5_091_040;
const GIVER: u32 = 51_940;
const PET: u32 = 51_943;

fn copy_template(
    node: &Standalone,
    table: &str,
    source: u32,
    entry: u32,
    changes: &[(&str, String)],
) {
    let mut row = node
        .query_rows(&format!("SELECT * FROM {table} WHERE entry = {source}"))
        .remove(0);
    row.insert("entry".into(), entry.to_string());
    for (key, value) in changes {
        row.insert((*key).into(), value.clone());
    }
    let columns = match table {
        "game_creature_template" => "entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class",
        "game_item_template" => "entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property",
        _ => panic!("unsupported fixture catalogue {table}"),
    };
    let values = columns
        .split(',')
        .map(|key| {
            let value = &row[key];
            if matches!(key, "name" | "subname") {
                format!("'{}'", value.replace('\'', "''"))
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    node.assert_sql(&format!(
        "INSERT INTO {table} ({columns}) VALUES ({values})"
    ));
}

fn insert_spell(node: &Standalone, spell: u32, entry: u32, kind: u8) {
    node.assert_sql(&format!("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES ({spell},'Binding fixture',0,0,50,0,180000,10,360000,1,0,0,0,0,0,1,0,false,0,0,0,0,0,0,0)"));
    let id = u64::from(spell) << 2;
    node.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES ({id},{spell},0,{kind},1,0,0.0,0,0,0.0,0,0,0,{entry},9,83,0,false)"));
}

fn actor(node: &Standalone) -> BTreeMap<String, String> {
    node.query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .remove(0)
}

fn summoned(node: &Standalone, entry: u32) -> Vec<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT * FROM game_world_entity WHERE entry = {entry}"
    ))
}

fn assert_refused(node: &Standalone, reducer: &str, args: &[&str]) {
    let result = node.call(reducer, args);
    assert!(!result.status.success(), "{reducer} unexpectedly succeeded");
}

fn load_startup_rule(node: &Standalone, entry: u32, variant: u32) {
    let subject = format!("entry:{entry}");
    let event = if variant == 0 {
        "timer-generic:1000:1000"
    } else {
        "spawn:always"
    };
    let rules = format!("{entry},{event},100,4294967295,once,all,ordinary,any-posture,faction:14+attack-start:spawner~{},aggro,100,4294967295,repeat:0:0,all,ordinary,any-posture,phase-inc:7", entry + 100);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(format!("{subject}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
    let packed = serde_json::to_string(&format!("{subject}@{revision}@{rules}")).unwrap();
    node.assert_call("import_creature_ai_definitions", &[&packed]);
}

fn focus_loss_cancels_cast(node: &Standalone, spell: u32, creature: u32, slot: &str, focus: &str) {
    node.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = 2000 WHERE spell_id = {spell}"
    ));
    node.assert_call("debug_use_item", &[PLAYER, slot]);
    node.assert_sql(&format!(
        "UPDATE game_gameobject SET instance_id = 99 WHERE guid = {focus}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows("SELECT * FROM game_pending_cast WHERE caster_guid = 1")
        .is_empty()));
    assert!(summoned(node, creature).is_empty());
    node.assert_sql(&format!(
        "UPDATE game_gameobject SET instance_id = 0 WHERE guid = {focus}"
    ));
    node.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = 50 WHERE spell_id = {spell}"
    ));
}

fn abandon_and_reaccept(node: &Standalone, giver: &str, quest: u32, item: u32) {
    let quest_arg = quest.to_string();
    node.assert_call("gw_abandon_quest", &[PLAYER, &quest_arg]);
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE entry = {item}"
        ))
        .is_empty());
    node.assert_sql(&format!(
        "UPDATE game_quest_template SET src_item_count = 0 WHERE entry = {quest}"
    ));
    node.assert_call("debug_accept_quest", &[PLAYER, giver, &quest_arg]);
    node.assert_call("gw_abandon_quest", &[PLAYER, &quest_arg]);
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE entry = {item}"
        ))
        .is_empty());
    node.assert_call("debug_accept_quest", &[PLAYER, giver, &quest_arg]);
    node.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE entry = {item}"
    ));
    node.assert_call("gw_abandon_quest", &[PLAYER, &quest_arg]);
    node.assert_sql(&format!(
        "UPDATE game_quest_template SET src_item_count = 1 WHERE entry = {quest}"
    ));
    node.assert_call("debug_accept_quest", &[PLAYER, giver, &quest_arg]);
}

fn circle_radius_refuses_item_use(node: &Standalone, creature: u32, item: u32, slot: &str) {
    let inventory = node.query_rows(&format!(
        "SELECT * FROM game_item_instance WHERE entry = {item}"
    ));
    node.assert_sql(&format!(
        "UPDATE game_gameobject_template SET data1 = 2 WHERE entry = {FOCUS}"
    ));
    assert_refused(node, "debug_use_item", &[PLAYER, slot]);
    assert!(summoned(node, creature).is_empty());
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE entry = {item}"
        )),
        inventory
    );
    node.assert_sql(&format!(
        "UPDATE game_gameobject_template SET data1 = 10 WHERE entry = {FOCUS}"
    ));
}

fn assert_first_aggro(node: &Standalone, summon: &BTreeMap<String, String>) {
    assert_eq!(summon["target_guid"], PLAYER);
    node.assert_call("gw_attack", &[&summon["guid"], PLAYER]);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT phase FROM game_creature_ai_state WHERE creature_guid = {}",
            summon["guid"]
        ))[0]["phase"],
        "7"
    );
}

fn assert_lifetime_checks_continue(node: &Standalone, summon: &BTreeMap<String, String>) {
    let query = format!(
        "SELECT * FROM game_creature_ai_summon_expiry WHERE creature_guid = {}",
        summon["guid"]
    );
    let first = node
        .query_rows(&query)
        .into_iter()
        .next()
        .expect("living summon lost its expiry row");
    for _ in 0..2 {
        let previous = node.query_rows(&query).remove(0);
        assert!(
            poll_until(POLL_TIMEOUT, || node
                .query_rows(&query)
                .first()
                .is_some_and(|current| current["scheduled_id"]
                    != previous["scheduled_id"]
                    && current["life_seq"] == first["life_seq"])),
            "living summon stopped its lifetime checks: {:?}",
            node.query_rows(&query)
        );
        assert_eq!(summoned(node, summon["entry"].parse().unwrap()).len(), 1);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn binding_items_require_the_circle_and_complete_both_quest_variants_without_replacing_a_pet() {
    // Reserved fixtures copy the verified 1689/1739 item, objective and reward relationships.
    // Casts take 50 ms here; importer tests pin the source event mapping independently.
    let mut node = Standalone::start("binding-quests");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_sql("UPDATE game_character SET level = 20, class = 9, race = 1 WHERE guid = 1");
    node.assert_call("debug_spawn_player_entity", &[PLAYER]);
    node.assert_sql("UPDATE game_world_entity SET level = 20, health = 100000, max_health = 100000 WHERE guid = 1");
    copy_template(
        &node,
        "game_creature_template",
        51000,
        GIVER,
        &[("faction_template", "35".into()), ("npc_flags", "2".into())],
    );
    node.assert_call("debug_spawn_at_feet", &[PLAYER, &GIVER.to_string(), "1"]);
    let giver = summoned(&node, GIVER).remove(0)["guid"].clone();
    copy_template(&node, "game_creature_template", 51000, PET, &[]);
    insert_spell(&node, 50_943, PET, 0x15);
    node.assert_call("debug_force_cast", &[PLAYER, "50943"]);
    let pet = summoned(&node, PET).remove(0);
    assert_eq!(pet["owner_guid"], PLAYER);

    let location = actor(&node);
    let focus_x = (location["x"].parse::<f32>().unwrap() + 3.0).to_string();
    node.assert_call(
        "debug_spawn_gameobject",
        &[
            "--",
            &FOCUS.to_string(),
            "8",
            "1",
            "83",
            &location["map_id"],
            &focus_x,
            &location["y"],
            &location["z"],
            "10",
            "0",
            "0",
            "0",
        ],
    );
    let focus_guid = ((0xF110_u64 << 48) | u64::from(FOCUS)).to_string();

    for (variant, reward) in [(0u32, 50_944), (1, 50_945)] {
        let quest = 5_091_041 + variant;
        let item = 5_091_041 + variant;
        let creature = 51_941 + variant;
        let spell = 50_941 + variant;
        copy_template(
            &node,
            "game_creature_template",
            51000,
            creature,
            &[
                (
                    "faction_template",
                    if variant == 0 { "35" } else { "90" }.into(),
                ),
                (
                    "unit_flags",
                    if variant == 0 { "512" } else { "33280" }.into(),
                ),
                ("health", "100000".into()),
                ("max_level_health", "100000".into()),
            ],
        );
        copy_template(
            &node,
            "game_item_template",
            51,
            item,
            &[
                ("spellid_1", spell.to_string()),
                ("spelltrigger_1", "0".into()),
                ("required_level", "1".into()),
                ("max_count", "1".into()),
                ("max_stack", "1".into()),
            ],
        );
        load_startup_rule(&node, creature, variant);
        insert_spell(&node, spell, creature, 0x24);
        insert_spell(&node, reward, PET, 0x15);
        node.assert_sql(&format!("INSERT INTO game_spell_learn (id,parent_spell,learn_spell) VALUES ({quest},{reward},50946)"));
        node.assert_sql(&format!("INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,reward_money_max_level) VALUES ({quest},1,20,'Binding fixture',0,0,0,1,256,-61,0,0,0,0,{item},1,false,0,0,0)"));
        node.assert_sql(&format!("INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) VALUES ({quest},{quest},0,0,{creature},1)"));
        node.assert_sql(&format!("INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES ({},{GIVER},{quest},0),({},{GIVER},{quest},1)", quest * 2, quest * 2 + 1));
        node.assert_sql(&format!(
            "INSERT INTO game_quest_reward_spell (quest_entry,spell_id) VALUES ({quest},{reward})"
        ));
        node.assert_call("debug_accept_quest", &[PLAYER, &giver, &quest.to_string()]);
        abandon_and_reaccept(&node, &giver, quest, item);
        let inventory = node.query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE entry = {item}"
        ));
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0]["stack_count"], "1");
        let slot = inventory[0]["slot"].clone();

        circle_radius_refuses_item_use(&node, creature, item, &slot);

        node.assert_sql(&format!(
            "UPDATE game_gameobject SET instance_id = 99 WHERE guid = {focus_guid}"
        ));
        assert_refused(&node, "debug_use_item", &[PLAYER, &slot]);
        assert!(summoned(&node, creature).is_empty());
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_item_instance WHERE entry = {item}"
            )),
            inventory
        );
        node.assert_sql(&format!(
            "UPDATE game_gameobject SET instance_id = 0 WHERE guid = {focus_guid}"
        ));

        focus_loss_cancels_cast(&node, spell, creature, &slot, &focus_guid);
        node.assert_call("debug_use_item", &[PLAYER, &slot]);
        assert!(poll_until(POLL_TIMEOUT, || summoned(&node, creature)
            .first()
            .is_some_and(|summon| summon["faction_template"] == "14")));
        let summons = summoned(&node, creature);
        assert_eq!(summons.len(), 1);
        let summon = &summons[0];
        assert_eq!(summon["owner_guid"], "0");
        assert_first_aggro(&node, summon);
        assert_lifetime_checks_continue(&node, summon);
        assert_eq!(summon["faction_template"], "14");
        assert_eq!(summon["map_id"], location["map_id"]);
        assert_eq!(summon["instance_id"], location["instance_id"]);
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_item_instance WHERE entry = {item}"
            )),
            inventory
        );
        let melee = node.query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
            summon["guid"]
        ));
        assert_eq!(melee[0]["target_guid"], PLAYER);
        assert_eq!(summoned(&node, PET)[0]["guid"], pet["guid"]);
        assert_eq!(summoned(&node, PET)[0]["owner_guid"], PLAYER);

        assert_refused(&node, "debug_use_item", &[PLAYER, &slot]);
        assert_eq!(summoned(&node, creature).len(), 1);
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_item_instance WHERE entry = {item}"
            )),
            inventory
        );
        node.assert_sql(&format!(
            "UPDATE game_world_entity SET health = 1 WHERE guid = {}",
            summon["guid"]
        ));
        node.assert_call("gw_attack", &[PLAYER, &summon["guid"]]);
        let progress_query =
            format!("SELECT * FROM game_character_quest WHERE quest_entry = {quest}");
        assert!(poll_until(POLL_TIMEOUT, || node
            .query_rows(&progress_query)
            .first()
            .is_some_and(|row| row["counts"] == "1")));
        let progress = node.query_rows(&progress_query);
        assert_eq!(progress[0]["counts"], "1");
        assert!(poll_until(POLL_TIMEOUT, || summoned(&node, creature).is_empty()));
        assert_refused(&node, "debug_kill_creature", &[PLAYER, &summon["guid"]]);
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_character_quest WHERE quest_entry = {quest}"
            )),
            progress
        );
        node.assert_call(
            "debug_turn_in_quest",
            &[PLAYER, &giver, &quest.to_string(), "0"],
        );
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_item_instance WHERE entry = {item}"
            )),
            inventory
        );
        assert_eq!(
            node.query_rows(&format!(
                "SELECT spell_id FROM game_player_spell WHERE spell_id = {reward}"
            ))
            .len(),
            1
        );
        assert!(node
            .query_rows("SELECT spell_id FROM game_player_spell WHERE spell_id = 50946")
            .is_empty());
        assert_refused(
            &node,
            "debug_turn_in_quest",
            &[PLAYER, &giver, &quest.to_string(), "0"],
        );
        assert_eq!(
            node.query_rows(&format!(
                "SELECT spell_id FROM game_player_spell WHERE spell_id = {reward}"
            ))
            .len(),
            1
        );
    }
}
