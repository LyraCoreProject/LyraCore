mod support;

use std::time::Duration;
use support::{poll_until, Standalone};

const SPELL: u32 = 5_090_701;

fn stage(duration_ms: u32) -> (Standalone, String) {
    let mut shard = Standalone::start("spell-guardians");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_level", &["1", "60"]);
    for (id, faction, group, friends, enemies) in [(1, 1, 3, 2, 12), (14, 16, 8, 0, 1)] {
        shard.assert_sql(&format!(
            "DELETE FROM game_faction_template WHERE id = {id}"
        ));
        shard.assert_sql(&format!(
            "INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,\
             enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) \
             VALUES ({id},{faction},{group},{friends},{enemies},0,0,0,0,0,0,0,0)"
        ));
    }
    shard.assert_call("debug_spawn_at_feet", &["1", "51006", "2"]);
    let owner = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51006")
        .into_iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap()
        .to_string();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = 14 WHERE guid = {owner}"
    ));
    shard.assert_sql("UPDATE game_creature_template SET faction_template = 35, level = 19, max_level = 20 WHERE entry = 51000");
    let rules = "1,spawn,100,4294967295,once,all,ordinary,npc-flags:gossip-and-quest~2,aggro,100,4294967295,once,all,ordinary,phase:2";
    let mut hash = blake3::Hasher::new();
    hash.update(b"lyracore-eventai-definition-v1");
    hash.update(format!("entry:51000@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hash.finalize().as_bytes()[..8].try_into().unwrap());
    shard.assert_call(
        "import_creature_ai_definitions",
        &[&serde_json::to_string(&format!("entry:51000@{revision}@{rules}")).unwrap()],
    );
    shard.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,\
         range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,\
         attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,\
         proc_flags,proc_chance,proc_charges) \
         VALUES ({SPELL},'Guardian fixture',0,0,0,0,0,0,{duration_ms},1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,\
         period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) \
         VALUES ({},{SPELL},0,39,2,1,0.0,0,0,0.0,0,0,0,51000,9,0,0,false)", u64::from(SPELL)<<2
    ));
    shard.assert_call(
        "debug_cast_spell_at",
        &[&owner, &SPELL.to_string(), &owner, "0", "0", "0"],
    );
    (shard, owner)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn guardians_attack_players_survive_owner_death_and_expire_in_combat() {
    let (shard, owner) = stage(12_000);
    let rows = shard.query_rows(&format!(
        "SELECT guid,faction_template,level,npc_flags FROM game_world_entity WHERE owner_guid = {owner}"
    ));
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row["faction_template"], "14");
        assert!(matches!(row["level"].as_str(), "19" | "20"));
        assert_eq!(row["level"], rows[0]["level"]);
        assert_eq!(row["npc_flags"], "3");
        assert!(poll_until(Duration::from_secs(4), || {
            !shard.query_rows(&format!(
            "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {} AND target_guid = 1", row["guid"]
        )).is_empty()
        }));
    }
    for row in &rows {
        assert_eq!(
            shard.query_rows(&format!(
                "SELECT phase FROM game_creature_ai_state WHERE creature_guid = {}",
                row["guid"]
            ))[0]["phase"],
            "2"
        );
    }
    shard.assert_call("debug_kill_creature", &["1", &owner]);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE owner_guid = {owner}"
            ))
            .len(),
        2
    );
    assert!(poll_until(Duration::from_secs(15), || shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE owner_guid = {owner}"
        ))
        .is_empty()));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn fifteen_guardians_allow_a_full_cast_and_sixteen_prevent_another() {
    let (shard, owner) = stage(0);
    shard.assert_sql(
        "UPDATE game_world_entity SET health = 100000, max_health = 100000 WHERE guid = 1",
    );
    for (amount, expected) in [(13, 15), (2, 17), (2, 17)] {
        shard.assert_sql(&format!(
            "UPDATE game_spell_effect SET base_points = {amount} WHERE spell_id = {SPELL}"
        ));
        shard.assert_call(
            "debug_cast_spell_at",
            &[&owner, &SPELL.to_string(), &owner, "0", "0", "0"],
        );
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE owner_guid = {owner}"
                ))
                .len(),
            expected,
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_guardian_leaves_a_non_lootable_corpse_for_fifteen_seconds() {
    let (shard, owner) = stage(0);
    let guardian = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE owner_guid = {owner}"
        ))
        .remove(0)["guid"]
        .clone();
    shard.assert_call("debug_kill_creature", &["1", &guardian]);
    std::thread::sleep(Duration::from_secs(1));
    let corpse = shard.query_rows(&format!(
        "SELECT health,dead,dynamic_flags FROM game_world_entity WHERE guid = {guardian}"
    ));
    assert_eq!(corpse.len(), 1);
    assert_eq!(corpse[0]["health"], "0");
    assert_eq!(corpse[0]["dead"], "true");
    assert_eq!(corpse[0]["dynamic_flags"], "0");
    assert!(poll_until(Duration::from_secs(18), || shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {guardian}"
        ))
        .is_empty()));
}
