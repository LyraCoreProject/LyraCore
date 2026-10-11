mod support;

use lyracore_shared::constants::{player_flags, unit_vis_flags};
use lyracore_shared::spell::taxonomy::*;
use support::{actor, number, Row, Standalone};

fn fixture(name: &str, race: u8) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET race = {race} WHERE guid = 1"
    ));
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql(
        "UPDATE game_creature_template SET entry = 12861, display_id = 1825 WHERE entry = 51000",
    );
    // Normalized build 5875 spell effects. Transform names a creature entry, not a display.
    for (spell, effects) in [
        (
            8326,
            [
                (A_FLAG, -20, 0, P_FLAG),
                (A_MOD_SPEED, 25, i32::from(SPEED_MOVE), P_SPEED_KIND),
                (A_MOD_SPEED, 25, i32::from(SPEED_SWIM), P_SPEED_KIND),
            ],
        ),
        (
            20584,
            [
                (A_MOD_SPEED, 50, i32::from(SPEED_MOVE), P_SPEED_KIND),
                (A_MOD_SPEED, 50, i32::from(SPEED_SWIM), P_SPEED_KIND),
                (A_TRANSFORM, 0, 12861, P_ENTRY),
            ],
        ),
    ] {
        insert_spell(&shard, spell, "Ghost", &effects);
    }
    shard
}

fn insert_spell(shard: &Standalone, spell: u32, name: &str, effects: &[(u8, i32, i32, u8)]) {
    shard.assert_sql(&format!(
            "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, family_name, family_flags, proc_flags, proc_chance, proc_charges) VALUES ({spell}, '{name}', 0, 0, 0, 0, 0, 0, 4294967295, 1, 0, 0, 0, 0, 0, 0, 0, false, 0, 0, 0, 0, 0, 0, 0)"
        ));
    for (index, (kind, amount, parameter, parameter_kind)) in effects.iter().copied().enumerate() {
        let id = (spell as u64) * 4 + index as u64;
        shard.assert_sql(&format!(
                "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, p0, p0_kind, p1, script_id, enters_combat) VALUES ({id}, {spell}, {index}, {kind}, {amount}, 1, 0.0, 0, 0, 0.0, 0, 0, 0, {parameter}, {parameter_kind}, 0, 0, false)"
            ));
    }
}

fn entity(shard: &Standalone) -> Row {
    shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .remove(0)
}

fn release(shard: &Standalone) {
    shard.assert_call("debug_set_health", &["1", "0"]);
    shard.assert_call("debug_repop", &["1"]);
}

fn assert_ghost(shard: &Standalone, wisp: bool) {
    let row = entity(shard);
    assert_eq!(row["dead"], "true");
    assert_ne!(number::<u32>(&row, "player_flags") & player_flags::GHOST, 0);
    assert_ne!(
        number::<u32>(&row, "unit_bytes_1") & unit_vis_flags::GHOST,
        0
    );
    assert_eq!(row["health"], "1");
    let amounts: Vec<_> = shard
        .query_rows("SELECT eff_kind, eff_p0, amount, stacks FROM game_aura WHERE target_guid = 1")
        .iter()
        .map(|a| lyracore_shared::aura::AuraAmount {
            kind: number(a, "eff_kind"),
            parameter: number(a, "eff_p0"),
            amount: number(a, "amount"),
            stacks: number(a, "stacks"),
        })
        .collect();
    assert_eq!(
        lyracore_shared::aura::run_speed(7.0, amounts.clone()),
        if wisp { 10.5 } else { 8.75 }
    );
    assert_eq!(
        lyracore_shared::aura::swim_speed(1.0, amounts),
        if wisp { 1.5 } else { 1.25 }
    );
    let native: u32 = number(&row, "native_display_id");
    assert_eq!(
        number::<u32>(&row, "display_id"),
        if wisp { 1825 } else { native }
    );
    let auras = shard.query_rows("SELECT spell_id FROM game_aura WHERE target_guid = 1");
    assert_eq!(auras.iter().filter(|a| a["spell_id"] == "8326").count(), 3);
    assert_eq!(
        auras.iter().filter(|a| a["spell_id"] == "20584").count(),
        if wisp { 3 } else { 0 }
    );
}

fn assert_alive(shard: &Standalone) {
    let row = entity(shard);
    assert_eq!(row["dead"], "false");
    assert_eq!(number::<u32>(&row, "player_flags") & player_flags::GHOST, 0);
    assert_eq!(
        number::<u32>(&row, "unit_bytes_1") & unit_vis_flags::GHOST,
        0
    );
    assert_eq!(row["display_id"], row["native_display_id"]);
    assert!(shard
        .query_rows("SELECT spell_id FROM game_aura WHERE target_guid = 1")
        .iter()
        .all(|a| a["spell_id"] != "8326" && a["spell_id"] != "20584"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn night_elf_release_applies_wisp_and_spirit_healer_restores_native_form() {
    let shard = fixture("ghost-wisp", 4);
    release(&shard);
    assert_ghost(&shard, true);
    shard.assert_call("debug_repop", &["1"]);
    assert_ghost(&shard, true);
    shard.assert_call("debug_spirit_healer_res", &["1"]);
    assert_alive(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn human_release_keeps_native_form() {
    let shard = fixture("ghost-human", 1);
    release(&shard);
    assert_ghost(&shard, false);
    shard.assert_call("debug_spirit_healer_res", &["1"]);
    assert_alive(&shard);
}

fn bind_session(shard: &Standalone) {
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    shard.assert_call("gw_heartbeat", &[]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn wisp_and_corpse_survive_logout_and_world_entry() {
    let shard = fixture("ghost-relog", 4);
    bind_session(&shard);
    release(&shard);
    shard.assert_call("debug_logout_character", &["1"]);
    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_corpse WHERE owner_guid = 1")
            .len(),
        1
    );
    bind_session(&shard);
    for entry in ["gw_player_login", "gw_player_world_port"] {
        shard.assert_call(entry, &["1", &actor("1")]);
        assert_ghost(&shard, true);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn corpse_reclaim_restores_native_form() {
    let shard = fixture("ghost-reclaim", 4);
    release(&shard);
    let ghost = entity(&shard);
    shard.assert_sql(&format!("UPDATE game_corpse SET x = {}, y = {}, z = {}, reclaim_delay_micros = 0 WHERE owner_guid = 1", ghost["x"], ghost["y"], ghost["z"]));
    shard.assert_call("gw_reclaim_corpse", &[&actor("1"), "1"]);
    assert_alive(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn self_resurrection_restores_native_form() {
    let shard = fixture("ghost-self-res", 4);
    release(&shard);
    insert_spell(
        &shard,
        5090000,
        "Self resurrection",
        &[(E_SELF_RESURRECT, 50, 0, P_NONE)],
    );
    shard.assert_sql(
        "INSERT INTO game_self_resurrect_option (character_guid, spell_id) VALUES (1, 5090000)",
    );
    shard.assert_call("debug_self_resurrect", &["1"]);
    assert_alive(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn ghost_effects_cannot_be_cancelled() {
    let shard = fixture("ghost-cancel", 4);
    release(&shard);
    for spell in ["8326", "20584"] {
        assert!(!shard
            .call("gw_cancel_aura", &[&actor("1"), spell])
            .status
            .success());
        assert_ghost(&shard, true);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn wisp_survives_cross_shard_transfer() {
    let source = fixture("ghost-transfer-source", 4);
    let destination = fixture("ghost-transfer-destination", 4);
    destination.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    release(&source);
    source.assert_call(
        "begin_transfer",
        &[
            "509675",
            &actor("1"),
            "0",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "true",
        ],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 509675");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    let system = actor("0");
    destination.assert_call("import_character_blob", &["509675", &blob, &system]);
    source.assert_call("confirm_import", &["509675", &system]);
    source.assert_call("finish_transfer", &["509675", &system]);
    destination.assert_call("release_transfer", &["509675", &system]);
    bind_session(&destination);
    destination.assert_call("gw_player_world_port", &["1", &actor("1")]);
    assert_ghost(&destination, true);
    destination.assert_call("debug_spirit_healer_res", &["1"]);
    assert_alive(&destination);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn damage_breaks_a_transform_and_restores_the_native_body() {
    let shard = fixture("transform-damage", 1);
    insert_spell(
        &shard,
        5090001,
        "Damage breaking transform",
        &[(A_TRANSFORM, 0, 12861, P_ENTRY)],
    );
    shard.assert_sql("UPDATE game_spell SET aura_interrupt = 1 WHERE spell_id = 5090001");
    shard.assert_call("debug_cast_at", &["1", "5090001", "1"]);
    assert_eq!(entity(&shard)["display_id"], "1825");
    shard.assert_call("debug_apply_damage", &["1", "1", "0"]);
    let row = entity(&shard);
    assert_eq!(row["display_id"], row["native_display_id"]);
    assert!(shard
        .query_rows("SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = 5090001")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_living_transform_survives_logout_and_world_entry() {
    let shard = fixture("transform-relog", 1);
    insert_spell(
        &shard,
        5090001,
        "Living transform",
        &[(A_TRANSFORM, 0, 12861, P_ENTRY)],
    );
    shard.assert_call("debug_cast_at", &["1", "5090001", "1"]);
    shard.assert_call("debug_logout_character", &["1"]);
    bind_session(&shard);
    shard.assert_call("gw_player_login", &["1", &actor("1")]);
    let row = entity(&shard);
    assert_eq!(row["dead"], "false");
    assert_eq!(row["display_id"], "1825");
    shard.assert_call("gw_cancel_aura", &[&actor("1"), "5090001"]);
    assert_eq!(entity(&shard)["display_id"], row["native_display_id"]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn accepting_resurrection_restores_native_form() {
    let shard = fixture("ghost-accept-res", 4);
    release(&shard);
    shard.assert_call(
        "debug_create_package_character",
        &[r#""ghost-test""#, r#""Healer""#, "1", "5"],
    );
    let healer = shard
        .query_rows("SELECT guid FROM game_character WHERE name = 'Healer'")
        .remove(0)["guid"]
        .clone();
    shard.assert_call("debug_spawn_player_entity", &[&healer]);
    let ghost = entity(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {}, z = {} WHERE guid = {healer}",
        ghost["x"], ghost["y"], ghost["z"]
    ));
    insert_spell(
        &shard,
        5090002,
        "Resurrection",
        &[(E_RESURRECT, 50, 0, P_NONE)],
    );
    shard.assert_sql("UPDATE game_spell SET range_yd = 40.0 WHERE spell_id = 5090002");
    shard.assert_sql("UPDATE game_spell_effect SET target = 2 WHERE spell_id = 5090002");
    shard.assert_call("debug_cast_at", &[&healer, "5090002", "1"]);
    shard.assert_call("debug_resurrect_response", &["1", "true"]);
    assert_alive(&shard);
    assert!(shard
        .query_rows("SELECT guid FROM game_corpse WHERE owner_guid = 1")
        .is_empty());
}
