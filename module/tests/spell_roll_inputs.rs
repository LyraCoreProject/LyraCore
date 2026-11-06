mod support;

use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn only_creatures_with_a_character_owner_can_crit_with_spells() {
    use lyracore_shared::spell::taxonomy::SPELL_ATTR_IGNORE_RESISTANCES;

    let (shard, caster) = stage("creature-spell-crit-owner");
    shard.assert_sql("UPDATE game_world_entity SET godmode = false, health = 10000, max_health = 10000 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {caster}"
    ));
    shard.assert_sql(&format!("UPDATE game_spell SET cast_flags = {SPELL_ATTR_IGNORE_RESISTANCES}, range_yd = 0, gcd_ms = 0, cooldown_ms = 0 WHERE spell_id = 133"));
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 400, die_sides = 0, per_level = 0.0 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES ({caster},1,2)"));
    let events = subscribe_with_fence(&shard, &["SELECT * FROM game_spell_cast_event_live"]);
    for (index, (owner, crit)) in [("0", false), (caster.as_str(), false), ("1", true)]
        .into_iter()
        .enumerate()
    {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET owner_guid = {owner} WHERE guid = {caster}"
        ));
        shard.assert_sql(&format!(
            "UPDATE game_conformance_spell_random SET state = 2 WHERE character_guid = {caster}"
        ));
        shard.assert_call("debug_force_cast_at", &[&caster, "133", "1"]);
        fence_subscription(&shard, &events);
        let casts: Vec<_> = event_rows(&events, "game_spell_cast_event_live")
            .into_iter()
            .filter(|row| row["caster_guid"] == caster && row["kind"] == "2")
            .collect();
        assert_eq!(casts.len(), index + 1);
        assert_eq!(casts[index]["is_crit"], crit.to_string(), "owner {owner}");
        assert_eq!(casts[index]["damage"], if crit { "600" } else { "400" });
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn binary_periodic_damage_and_slow_share_one_resist_roll_at_cast() {
    use lyracore_shared::spell::taxonomy::{
        A_MOD_RESISTANCE, A_MOD_SPEED, A_PERIODIC_DAMAGE, SPELL_ATTR_BINARY,
    };

    let (shard, target) = stage("binary-aura-hit");
    shard.assert_sql(&format!("UPDATE game_world_entity SET level = 60, health = 10000, max_health = 10000 WHERE guid = {target}"));
    shard.assert_sql(&format!("UPDATE game_spell SET cast_flags = {SPELL_ATTR_BINARY}, duration_ms = 10000, gcd_ms = 0, cooldown_ms = 0, is_negative = true WHERE spell_id = 133"));
    shard.assert_sql(&format!("UPDATE game_spell_effect SET kind = {A_PERIODIC_DAMAGE}, base_points = 10, die_sides = 0, per_level = 0.0, period_ms = 1000 WHERE spell_id = 133"));
    shard.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES (533,133,1,{A_MOD_SPEED},-50,0,0.0,0,1,0.0,0,0,0,0,0,0,0,false,0.0)"));
    shard.assert_call("debug_fill_aura_slots", &[&target, "1", "false", "1"]);
    shard.assert_sql(&format!("UPDATE game_aura SET eff_kind = {A_MOD_RESISTANCE}, amount = 300, eff_p0 = 4 WHERE target_guid = {target}"));
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},0)"));
    let events = subscribe_with_fence(&shard, &["SELECT * FROM game_spell_cast_event_live"]);
    for (index, (resistance, seed, miss)) in [
        (300, 10161, "2"),
        (300, 21988, "0"),
        (300, 26747, "0"),
        (0, 10161, "0"),
    ]
    .into_iter()
    .enumerate()
    {
        shard.assert_sql(&format!("UPDATE game_aura SET amount = {resistance} WHERE target_guid = {target} AND eff_kind = {A_MOD_RESISTANCE}"));
        shard.assert_sql(&format!(
            "UPDATE game_conformance_spell_random SET state = {seed} WHERE character_guid = 1"
        ));
        shard.assert_call("debug_force_cast_at", &["1", "133", &target]);
        fence_subscription(&shard, &events);
        let casts: Vec<_> = event_rows(&events, "game_spell_cast_event_live")
            .into_iter()
            .filter(|row| row["caster_guid"] == "1" && row["kind"] == "2")
            .collect();
        assert_eq!(casts.len(), index + 1);
        assert_eq!(
            casts[index]["miss_info"], miss,
            "resistance {resistance}, seed {seed}"
        );
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT id FROM game_aura WHERE target_guid = {target} AND spell_id = 133"
                ))
                .len(),
            if miss == "0" { 2 } else { 0 }
        );
        shard.assert_sql(&format!(
            "DELETE FROM game_aura WHERE target_guid = {target} AND spell_id = 133"
        ));
    }
}

fn stage(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_0 = 2049, level = 60, intellect = 125, godmode = true WHERE guid = 1");
    shard.assert_call("debug_spawn_at_feet", &["1", "51000", "2"]);
    let target = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .into_iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap()
        .to_string();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 63, simulation_indexed = false WHERE guid = {target}"
    ));
    (shard, target)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn mage_spell_crit_uses_its_class_and_level() {
    let (shard, target) = stage("mage-spell-crit");
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_crit_bp FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_crit_bp"],
        "602"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn creature_levels_add_partial_resistance_without_gear() {
    let (shard, target) = stage("innate-spell-resistance");
    shard.assert_sql("UPDATE game_world_entity SET intellect = 0 WHERE guid = 1");
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 400 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},6)"));
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_hit_normal FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_hit_normal"],
        "100"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn binary_damage_skips_the_innate_partial_resistance_roll() {
    use lyracore_shared::spell::taxonomy::SPELL_ATTR_BINARY;

    let (shard, target) = stage("binary-spell-resistance");
    shard.assert_sql("UPDATE game_world_entity SET intellect = 0 WHERE guid = 1");
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 400 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},6)"));
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = {SPELL_ATTR_BINARY} WHERE spell_id = 133"
    ));
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_hit_normal FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_hit_normal"],
        "400"
    );
    shard.assert_sql("UPDATE game_spell SET cast_flags = 0 WHERE spell_id = 133");
    shard.assert_sql("UPDATE game_conformance_spell_random SET state = 6 WHERE character_guid = 1");
    shard.assert_call("debug_compute_spell", &["1", &target, "133"]);
    assert_eq!(
        shard.query_rows("SELECT spell_hit_normal FROM game_debug_readout WHERE key = 'spell'")[0]
            ["spell_hit_normal"],
        "100"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_partial_resist_that_removes_a_small_area_tick_still_reports_its_log() {
    let (shard, target) = stage("resisted-area-log");
    shard.assert_sql("INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (5090300,5090300,1,1,2,0,0,0,0,0,0,0,0),(5090301,5090301,2,2,1,0,0,0,0,0,0,0,0)");
    shard.assert_sql("UPDATE game_world_entity SET faction_template = 5090300 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = 5090301 WHERE guid = {target}"
    ));
    shard.assert_sql("UPDATE game_spell SET duration_ms = 2000 WHERE spell_id = 133");
    shard.assert_sql("UPDATE game_spell_effect SET base_points = 1, period_ms = 500, radius_yd = 20.0 WHERE spell_id = 133");
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},149)"));
    let events = subscribe_with_fence(&shard, &["SELECT * FROM game_spell_cast_event_live"]);
    shard.assert_call("debug_spawn_ground_area", &["1", "133"]);
    shard.assert_call("debug_advance_roll_batch", &["10"]);
    fence_subscription(&shard, &events);
    assert_ne!(
        shard
            .query_rows("SELECT state FROM game_conformance_spell_random WHERE character_guid = 1")
            [0]["state"],
        "149"
    );
    let logs: Vec<_> = event_rows(&events, "game_spell_cast_event_live")
        .into_iter()
        .filter(|row| row["spell_id"] == "133" && row["target_guid"] == target)
        .collect();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["damage"], "0");
    assert_eq!(logs[0]["resisted"], "1");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn aura_cast_hit_testing_uses_damage_class_and_hostility() {
    use lyracore_shared::spell::taxonomy::{A_MOD_SPEED, SPELL_ATTR_MAGIC_HIT};
    let (shard, target) = stage("aura-damage-class");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {target}"
    ));
    shard.assert_sql("UPDATE game_spell SET duration_ms = 10000, gcd_ms = 0, cooldown_ms = 0 WHERE spell_id = 133");
    shard.assert_sql(&format!("UPDATE game_spell_effect SET kind = {A_MOD_SPEED}, base_points = -50, die_sides = 0, per_level = 0.0 WHERE spell_id = 133"));
    shard.assert_sql(&format!("INSERT INTO game_conformance_spell_random (character_guid,target_guid,state) VALUES (1,{target},20554)"));
    let events = subscribe_with_fence(&shard, &["SELECT * FROM game_spell_cast_event_live"]);
    for (index, (flags, school, negative, miss)) in [
        (0, 4, true, "0"),
        (SPELL_ATTR_MAGIC_HIT, 4, true, "2"),
        (SPELL_ATTR_MAGIC_HIT, 4, false, "0"),
        (SPELL_ATTR_MAGIC_HIT, 1, true, "2"),
    ]
    .into_iter()
    .enumerate()
    {
        shard.assert_sql(&format!("UPDATE game_spell SET cast_flags = {flags}, school_mask = {school}, is_negative = {negative} WHERE spell_id = 133"));
        shard.assert_sql(
            "UPDATE game_conformance_spell_random SET state = 20554 WHERE character_guid = 1",
        );
        shard.assert_call("debug_force_cast_at", &["1", "133", &target]);
        fence_subscription(&shard, &events);
        let casts: Vec<_> = event_rows(&events, "game_spell_cast_event_live")
            .into_iter()
            .filter(|row| row["caster_guid"] == "1" && row["kind"] == "2")
            .collect();
        assert_eq!(casts.len(), index + 1);
        assert_eq!(
            casts[index]["miss_info"], miss,
            "flags {flags}, negative {negative}, integer roll 399"
        );
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT id FROM game_aura WHERE spell_id = 133 AND target_guid = {target}"
                ))
                .len(),
            usize::from(miss == "0")
        );
        shard.assert_sql(&format!(
            "DELETE FROM game_aura WHERE spell_id = 133 AND target_guid = {target}"
        ));
    }
}
