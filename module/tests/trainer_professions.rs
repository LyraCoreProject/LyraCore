mod support;

use support::{actor, Standalone};

fn fixture(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_level", &["1", "60"]);
    shard.assert_call("debug_set_money", &["1", "1000000"]);
    shard.assert_call("debug_spawn_at_feet", &["1", "51001", "2"]);
    let npc = shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51001")[0]["guid"]
        .clone();
    (shard, npc)
}

fn offer(shard: &Standalone, id: u32, spell: u32, line: u32, cap: u32) {
    shard.assert_sql(&format!("INSERT INTO game_trainer_spell (id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap) VALUES ({id},51001,{spell},10,1,{line},{cap})"));
}

fn wrapper(shard: &Standalone, wrapper: u32, target: u32) {
    let id = u64::from(wrapper) << 2;
    let kind = lyracore_shared::spell::taxonomy::E_SCRIPTED;
    shard.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES ({id},{wrapper},0,{kind},0,0,0.0,0,0,0.0,0,{target},0,0,0,0,0,false)"));
}

fn assert_learned(shard: &Standalone, spell: u32, line: u32, cap: u32) {
    assert_eq!(shard.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = 1 AND spell_id = {spell}")).len(), 1);
    let skill = shard.query_rows(&format!("SELECT current,max_rank FROM game_player_skill WHERE character_guid = 1 AND skill_line = {line}"));
    assert_eq!(skill.len(), 1);
    assert_eq!(skill[0]["current"], "1");
    assert_eq!(skill[0]["max_rank"], cap.to_string());
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.7.1 CLI and debug Module"]
fn profession_wrappers_teach_the_spell_at_every_skill_tier() {
    let (shard, npc) = fixture("trainer-profession-wrappers");
    for (profession, (line, spells)) in [
        (171, [2259, 3101, 3464, 11611]),
        (356, [7620, 7731, 7732, 18248]),
        (186, [2575, 2576, 3564, 10248]),
    ]
    .into_iter()
    .enumerate()
    {
        for (tier, spell) in spells.into_iter().enumerate() {
            let id = 5_090_800 + (profession * 4 + tier) as u32;
            let cap = (tier as u32 + 1) * 75;
            wrapper(&shard, id, spell);
            offer(&shard, id, id, line, cap);
            shard.assert_sql(&format!("INSERT INTO game_skill_ability (id,spell_id,skill_line,race_mask,class_mask,min_skill,acquire_method,gray,green) VALUES ({id},{spell},{line},0,0,0,0,0,0)"));
            shard.assert_call("gw_trainer_buy", &[&actor("1"), &npc, &id.to_string()]);
            assert_learned(&shard, spell, line, cap);
            assert!(shard.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = 1 AND spell_id = {id}")).is_empty());
        }
    }
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.7.1 CLI and debug Module"]
fn classic_wrapper_and_synthesized_profession_offers_keep_working_in_either_order() {
    let (shard, npc) = fixture("trainer-classic-professions");
    for (tier, spell) in [2259, 3101, 3464, 11611].into_iter().enumerate() {
        let id = 5_090_900 + tier as u32;
        let cap = (tier as u32 + 1) * 75;
        wrapper(&shard, id, spell);
        offer(&shard, id, id, 0, 75);
        offer(&shard, id + 10, spell, 171, cap);
        if tier % 2 == 0 {
            shard.assert_call("gw_trainer_buy", &[&actor("1"), &npc, &id.to_string()]);
        }
        shard.assert_call("gw_trainer_buy", &[&actor("1"), &npc, &spell.to_string()]);
        assert_learned(&shard, spell, 171, cap);
        let money = shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1");
        assert!(!shard
            .call("gw_trainer_buy", &[&actor("1"), &npc, &id.to_string()])
            .status
            .success());
        assert_eq!(
            shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1"),
            money
        );
    }
}
