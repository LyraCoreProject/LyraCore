mod support;

use lyracore_package_delta::fixture_ids::{SCENARIO_VENDOR_ENTRY, TEST_WOLF_ENTRY};
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
        &format!("UPDATE game_creature_template SET entry = 12861, display_id = 1825 WHERE entry = {TEST_WOLF_ENTRY}"),
    );
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
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell),
            ("name", &name),
            ("duration_ms", &4294967295u64),
            ("school_mask", &1),
        ],
    );
    for (index, (kind, amount, parameter, parameter_kind)) in effects.iter().copied().enumerate() {
        let id = (spell as u64) * 4 + index as u64;
        shard.insert_row(
            "game_spell_effect",
            &[
                ("id", &id),
                ("spell_id", &spell),
                ("effect_index", &index),
                ("kind", &kind),
                ("base_points", &amount),
                ("die_sides", &1),
                ("p_0", &parameter),
                ("p_0_kind", &parameter_kind),
            ],
        );
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
    let auras =
        shard.query_rows("SELECT eff_kind, eff_p0, amount FROM game_aura WHERE target_guid = 1");
    let strongest = |speed: u8| {
        auras
            .iter()
            .filter(|a| {
                number::<u8>(a, "eff_kind") == A_MOD_SPEED
                    && number::<i32>(a, "eff_p0") == i32::from(speed)
            })
            .map(|a| number::<i32>(a, "amount"))
            .max()
    };
    let bonus = if wisp { 50 } else { 25 };
    assert_eq!(strongest(SPEED_MOVE), Some(bonus));
    assert_eq!(strongest(SPEED_SWIM), Some(bonus));
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn corpse_reclaim_restores_native_form() {
    let shard = fixture("ghost-reclaim", 4);
    release(&shard);
    let ghost = entity(&shard);
    shard.assert_sql(&format!("UPDATE game_corpse SET x = {}, y = {}, z = {}, reclaim_delay_micros = 0 WHERE owner_guid = 1", ghost["x"], ghost["y"], ghost["z"]));
    shard.assert_call("gw_reclaim_corpse", &[&actor("1"), "1"]);
    assert_alive(&shard);
}

const CORPSE: &str = "17366161638117343233";

fn corpse_is_bones(shard: &Standalone) -> String {
    shard.query_rows("SELECT is_bones FROM game_corpse WHERE owner_guid = 1")[0]["is_bones"].clone()
}

fn move_corpse_to_ghost(shard: &Standalone) {
    let ghost = entity(shard);
    shard.assert_sql(&format!("UPDATE game_corpse SET x = {}, y = {}, z = {}, reclaim_delay_micros = 0 WHERE owner_guid = 1", ghost["x"], ghost["y"], ghost["z"]));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_reclaims_its_corpse_after_six_minutes() {
    let shard = fixture("ghost-reclaim-late", 4);
    release(&shard);
    shard.assert_call("debug_age_corpse", &[CORPSE, "360"]);
    assert_eq!(corpse_is_bones(&shard), "false");
    move_corpse_to_ghost(&shard);
    shard.assert_call("gw_reclaim_corpse", &[&actor("1"), "1"]);
    assert_alive(&shard);
    assert_eq!(corpse_is_bones(&shard), "true");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_corpse_expires_to_bones_after_three_days_and_a_ghost_cannot_reclaim_it() {
    let shard = fixture("ghost-reclaim-expired", 4);
    release(&shard);
    shard.assert_call("debug_age_corpse", &[CORPSE, "259201"]);
    assert_eq!(corpse_is_bones(&shard), "true");
    move_corpse_to_ghost(&shard);
    let refusal = shard.call("gw_reclaim_corpse", &[&actor("1"), "1"]);
    assert!(!refusal.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    );
    assert!(output.contains("corpse has decayed to bones"), "{output}");
    assert_ghost(&shard, true);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_that_logs_in_after_its_corpse_turned_to_bones_is_revived_at_half_health() {
    let shard = fixture("ghost-login-bones", 4);
    bind_session(&shard);
    release(&shard);
    shard.assert_call("debug_logout_character", &["1"]);
    shard.assert_call("debug_age_corpse", &[CORPSE, "259201"]);
    bind_session(&shard);
    shard.assert_call("gw_player_login", &["1", &actor("1")]);
    assert_alive(&shard);
    let row = entity(&shard);
    let (health, max_health): (u32, u32) = (number(&row, "health"), number(&row, "max_health"));
    assert!(
        (max_health / 2..max_health).contains(&health),
        "{health} of {max_health}"
    );
    assert_eq!(corpse_is_bones(&shard), "true");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn bones_of_a_revived_character_disappear_after_an_hour() {
    let shard = fixture("ghost-bones-expiry", 4);
    release(&shard);
    move_corpse_to_ghost(&shard);
    shard.assert_call("gw_reclaim_corpse", &[&actor("1"), "1"]);
    shard.assert_call("debug_age_corpse", &[CORPSE, "3500"]);
    assert_eq!(corpse_is_bones(&shard), "true");
    shard.assert_call("debug_age_corpse", &[CORPSE, "3601"]);
    assert!(shard
        .query_rows("SELECT guid FROM game_corpse WHERE owner_guid = 1")
        .is_empty());
}

fn power_after_reclaim(name: &str, class: u32, power_type: u32, max_power: u32, power: u32) -> u32 {
    let shard = fixture(name, 1);
    let bytes: u32 = number(&entity(&shard), "unit_bytes_0");
    let bytes = (bytes & 0x00FF_00FF) | (class << 8) | (power_type << 24);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_bytes_0 = {bytes}, \
         max_power = {max_power}, power = {power} WHERE guid = 1"
    ));
    release(&shard);
    let ghost = entity(&shard);
    shard.assert_sql(&format!("UPDATE game_corpse SET x = {}, y = {}, z = {}, reclaim_delay_micros = 0 WHERE owner_guid = 1", ghost["x"], ghost["y"], ghost["z"]));
    shard.assert_call("gw_reclaim_corpse", &[&actor("1"), "1"]);
    assert_alive(&shard);
    number(&entity(&shard), "power")
}

// The reclaimed Character regenerates at once, so a power that returns above 0 lands within a few ticks.
const REGEN_ALLOWANCE: u32 = 200;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn corpse_reclaim_returns_half_the_mana() {
    let mana = power_after_reclaim("reclaim-mana", 8, 0, 100_000, 100);
    assert!((50_000..50_000 + REGEN_ALLOWANCE).contains(&mana), "{mana}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn corpse_reclaim_returns_no_rage() {
    assert_eq!(power_after_reclaim("reclaim-rage", 1, 1, 1000, 600), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn corpse_reclaim_returns_half_the_energy() {
    let energy = power_after_reclaim("reclaim-energy", 4, 3, 10_000, 100);
    assert!(
        (5_000..5_000 + REGEN_ALLOWANCE).contains(&energy),
        "{energy}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_cannot_use_a_stale_self_resurrection_option() {
    let shard = fixture("ghost-self-res", 4);
    release(&shard);
    insert_spell(
        &shard,
        5090000,
        "Self resurrection",
        &[(E_SELF_RESURRECT, 50, 0, P_NONE)],
    );
    shard.insert_row(
        "game_self_resurrect_option",
        &[("character_guid", &1), ("spell_id", &5090000)],
    );
    let refusal = shard.call("gw_self_resurrect", &[&actor("1")]);
    assert!(!refusal.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    );
    assert!(output.contains("caller is a ghost"), "{output}");
    assert_ghost(&shard, true);
    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_corpse WHERE owner_guid = 1")
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
        .query_rows("SELECT guid FROM game_corpse WHERE owner_guid = 1 AND is_bones = false")
        .is_empty());
}

const SPIRIT_HEALER_OPTION: u32 = 509_6740;
const SPIRIT_HEALER_ENTRY: u32 = SCENARIO_VENDOR_ENTRY;
const RESURRECTION_SICKNESS: u32 = 15007;
const BLADE: &str = "5090050";

fn creature_guids(shard: &Standalone) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SPIRIT_HEALER_ENTRY}"
        ))
        .into_iter()
        .map(|row| row["guid"].clone())
        .collect()
}

fn spirit_healer(shard: &Standalone, offset: f32) -> String {
    let before = creature_guids(shard);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &SPIRIT_HEALER_ENTRY.to_string(), &offset.to_string()],
    );
    let healer = creature_guids(shard)
        .into_iter()
        .find(|guid| !before.contains(guid))
        .expect("a new Spirit Healer");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {} WHERE guid = {healer}",
        lyracore_shared::constants::npc_flags::SPIRITHEALER
    ));
    healer
}

fn select_spirit_healer(shard: &Standalone, healer: &str) -> bool {
    shard
        .call(
            "debug_gossip_select",
            &[
                "1",
                healer,
                "0",
                &SPIRIT_HEALER_OPTION.to_string(),
                "0",
                &lyracore_shared::constants::gossip_option::SPIRITHEALER.to_string(),
            ],
        )
        .status
        .success()
}

fn timestamp(value: &str) -> i64 {
    spacetimedb::Timestamp::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("invalid durable timestamp {value:?}: {error}"))
        .to_micros_since_unix_epoch()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_chooses_a_spirit_healer_in_reach_and_pays_its_penalties() {
    let shard = fixture("ghost-spirit-healer", 1);
    shard.insert_row(
        "game_gossip_option",
        &[
            ("row_id", &SPIRIT_HEALER_OPTION),
            ("entry", &SPIRIT_HEALER_ENTRY),
            ("text", &"Return me to life."),
            (
                "action",
                &lyracore_shared::constants::gossip_option::SPIRITHEALER,
            ),
        ],
    );
    shard.assert_call("debug_set_level", &["1", "15"]);
    let beside_the_living = spirit_healer(&shard, 2.0);
    assert!(
        !select_spirit_healer(&shard, &beside_the_living),
        "a living Character cannot choose the resurrection"
    );
    shard.assert_call("debug_grant_item", &["1", BLADE, "1"]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_durability = 100 WHERE entry = {BLADE}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = 15, durability = 100 WHERE owner_guid = 1 AND entry = {BLADE}"
    ));
    release(&shard);
    let healer = spirit_healer(&shard, 2.0);
    let distant = spirit_healer(&shard, 30.0);

    assert!(
        !select_spirit_healer(&shard, &distant),
        "the healer is out of reach"
    );
    assert!(
        !select_spirit_healer(&shard, "1"),
        "a Character is not a Spirit Healer"
    );
    assert!(select_spirit_healer(&shard, &healer));
    assert_ghost(&shard, false);

    shard.assert_call("debug_spirit_healer_res", &["1"]);
    assert_alive(&shard);
    let blade = shard.query_rows(&format!(
        "SELECT durability FROM game_item_instance WHERE owner_guid = 1 AND entry = {BLADE}"
    ));
    assert_eq!(
        blade[0]["durability"], "65",
        "ten percent at death, then twenty-five percent at the Spirit Healer"
    );
    let sickness = shard.query_rows(&format!(
        "SELECT applied_at, expires_at FROM game_aura WHERE target_guid = 1 AND spell_id = {RESURRECTION_SICKNESS}"
    ));
    assert_eq!(
        timestamp(&sickness[0]["expires_at"]) - timestamp(&sickness[0]["applied_at"]),
        5 * 60_000_000,
        "level 15 is sick for five minutes"
    );
    assert!(
        !shard
            .call("debug_spirit_healer_res", &["1"])
            .status
            .success(),
        "a living Character cannot be resurrected again"
    );
}
