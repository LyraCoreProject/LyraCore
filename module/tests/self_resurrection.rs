mod support;

use lyracore_shared::spell::taxonomy::{
    A_SELF_RESURRECT, E_SELF_RESURRECT, P_FLAT_MANA, P_SPELL_ID, T_SELF, T_TARGET_ALLY,
};

use std::collections::BTreeMap;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::{player_flags, unit_flags, unit_vis_flags};
use serde_json::{json, value::RawValue};
use support::{ReducerConnection, Standalone};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;

const SOULSTONE_AURA: u32 = 20707;
const SOULSTONE_SELF_RES: u32 = 3026;
const SOULSTONE_HEALTH: u32 = 400;
const SOULSTONE_MANA: u32 = 700;
const RESURRECTION_SICKNESS: u32 = 15007;

const WARRIOR: u32 = 1;
const ROGUE: u32 = 4;
const WARLOCK: u32 = 9;
const MANA: u32 = 0;
const RAGE: u32 = 1;
const ENERGY: u32 = 3;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_soulstone_death_holds_the_option_and_using_it_revives_in_place() {
    let standalone = start("self-res-soulstone");
    set_class(&standalone, WARLOCK, MANA, 2000);
    soulstone(&standalone);

    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);

    assert_eq!(option_spell(&standalone), Some(SOULSTONE_SELF_RES));
    assert!(
        !has_aura(&standalone, SOULSTONE_AURA),
        "the Soulstone aura does not survive death"
    );

    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);

    let player = entity(&standalone);
    assert_alive_in_place(&player);
    assert_eq!(number(&player, "health"), SOULSTONE_HEALTH);
    assert_eq!(number(&player, "power"), SOULSTONE_MANA);
    assert_eq!(
        option_spell(&standalone),
        None,
        "using the option spends it"
    );
    assert!(
        !has_aura(&standalone, RESURRECTION_SICKNESS),
        "self-resurrection applies no Resurrection Sickness"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn without_an_option_self_resurrection_is_refused_and_changes_nothing() {
    let standalone = start("self-res-refused");
    soulstone(&standalone);
    let alive = standalone.call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert!(
        !alive.status.success(),
        "a living Character holds no option to use"
    );

    standalone.assert_sql(&format!(
        "DELETE FROM game_aura WHERE target_guid = {PLAYER} AND spell_id = {SOULSTONE_AURA}"
    ));
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    assert_eq!(option_spell(&standalone), None);

    let dead = standalone.call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert!(
        !dead.status.success(),
        "a death without a Soulstone gives no option"
    );
    let player = entity(&standalone);
    assert_eq!(player["dead"], "true");
    assert_eq!(number(&player, "health"), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn release_spirit_clears_the_option_and_self_resurrection_is_refused() {
    let standalone = start("self-res-ghost");
    set_class(&standalone, WARLOCK, MANA, 2000);
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    assert_eq!(option_spell(&standalone), Some(SOULSTONE_SELF_RES));
    standalone.assert_call("debug_repop", &[&PLAYER.to_string()]);

    let ghost = entity(&standalone);
    assert_ne!(number(&ghost, "player_flags") & player_flags::GHOST, 0);
    assert_eq!(option_spell(&standalone), None);
    assert_eq!(corpse_count(&standalone), 1);

    assert!(!standalone
        .call("debug_self_resurrect", &[&PLAYER.to_string()])
        .status
        .success());

    assert_eq!(entity(&standalone), ghost);
    assert_eq!(corpse_count(&standalone), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_cannot_spend_a_stale_reincarnation_option_through_the_native_protocol() {
    let shard = start("self-res-ghost-stale");
    let mut connection = enter_native(&shard);
    reincarnation(&shard);
    shard.assert_call("debug_set_health", &["1", "0"]);
    shard.assert_call("debug_repop", &["1"]);
    let counter = number(
        &shard
            .query_rows("SELECT teleport_counter FROM game_native_player WHERE character_guid = 1")
            [0],
        "teleport_counter",
    );
    let mut acknowledgement = PLAYER.to_le_bytes().to_vec();
    acknowledgement.extend_from_slice(&counter.to_le_bytes());
    acknowledgement.extend_from_slice(&0u32.to_le_bytes());
    connection
        .call("native_world_receive", json!([0x00c7u32, acknowledgement]))
        .unwrap();
    shard.assert_sql("DELETE FROM game_self_resurrect_option WHERE character_guid = 1");
    shard.insert_row(
        "game_self_resurrect_option",
        &[("character_guid", &1), ("spell_id", &21169)],
    );
    let ghost = entity(&shard);
    let corpses = shard.query_rows("SELECT * FROM game_corpse WHERE owner_guid = 1");
    let inventory = shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1");

    let refusal = connection
        .call("native_world_receive", json!([0x2b3u32, Vec::<u8>::new()]))
        .unwrap_err();
    assert!(refusal.contains("native_reply:[]"), "{refusal}");
    assert_eq!(entity(&shard), ghost);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_corpse WHERE owner_guid = 1"),
        corpses
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1"),
        inventory
    );
    assert_eq!(option_spell(&shard), Some(21169));
    assert!(shard
        .query_rows("SELECT id FROM game_spell_cd WHERE caster_guid = 1")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn beginning_a_transfer_clears_the_option_even_without_a_live_entity() {
    for live in [true, false] {
        let shard = start(&format!("self-res-transfer-{live}"));
        shard.assert_call("claim_operator", &[]);
        soulstone(&shard);
        shard.assert_call("debug_set_health", &["1", "0"]);
        assert_eq!(option_spell(&shard), Some(SOULSTONE_SELF_RES));
        if !live {
            shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
        }

        shard.assert_call(
            "begin_transfer",
            &[
                "619",
                &support::actor("1"),
                "0",
                "0",
                "1200",
                "1200",
                "50",
                "0",
                "true",
            ],
        );

        assert_eq!(option_spell(&shard), None);
        assert!(shard
            .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
            .is_empty());
        assert_eq!(
            shard
                .query_rows("SELECT transfer_id FROM game_transfer_out WHERE transfer_id = 619")
                .len(),
            1
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spirit_healer_resurrection_or_a_logout_spends_the_option() {
    let standalone = start("self-res-other-paths");
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    standalone.assert_call("debug_repop", &[&PLAYER.to_string()]);
    standalone.insert_row(
        "game_self_resurrect_option",
        &[
            ("character_guid", &PLAYER),
            ("spell_id", &SOULSTONE_SELF_RES),
        ],
    );
    standalone.assert_call("debug_spirit_healer_res", &[&PLAYER.to_string()]);
    assert_eq!(
        option_spell(&standalone),
        None,
        "a Spirit Healer res spends it"
    );

    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    assert_eq!(option_spell(&standalone), Some(SOULSTONE_SELF_RES));
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_logout_character", &[&PLAYER.to_string()]);
    assert_eq!(option_spell(&standalone), None, "a logout spends it");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn rage_comes_back_empty_and_energy_comes_back_full() {
    let standalone = start("self-res-power-types");
    set_class(&standalone, WARRIOR, RAGE, 1000);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "500"]);
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert_eq!(number(&entity(&standalone), "power"), 0);

    set_class(&standalone, ROGUE, ENERGY, 100);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "10"]);
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert_eq!(number(&entity(&standalone), "power"), 100);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn reincarnation_revives_with_an_ankh_and_starts_its_cooldown() {
    let shard = start("self-res-reincarnation");
    reincarnation(&shard);
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), Some(21169));
    hold_vitals(&shard);
    shard.assert_call("debug_self_resurrect", &["1"]);
    let character = entity(&shard);
    assert_alive_in_place(&character);
    assert_eq!(number(&character, "health"), 200);
    assert_eq!(number(&character, "power"), 400);
    assert_eq!(option_spell(&shard), None);
    assert!(shard
        .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = 17030")
        .is_empty());
    let cooldowns =
        shard.query_rows("SELECT spell_id, ready_at FROM game_spell_cd WHERE caster_guid = 1");
    assert_eq!(cooldowns.len(), 1);
    assert_eq!(number(&cooldowns[0], "spell_id"), 21169);
    let ready_at = spacetimedb::Timestamp::parse_from_rfc3339(&cooldowns[0]["ready_at"])
        .unwrap()
        .to_micros_since_unix_epoch();
    assert_eq!(ready_at, 1_003_600_000_000);

    ankh(&shard);
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn native_reincarnation_reports_the_configured_cooldown() {
    let shard = start("self-res-native-cooldown");
    let mut connection = enter_native(&shard);
    reincarnation(&shard);
    for duration_ms in [3_600_000, 120_000, 0] {
        shard.assert_sql("DELETE FROM game_spell_cd WHERE caster_guid = 1");
        shard.assert_sql(&format!(
            "UPDATE game_spell SET cooldown_ms = {duration_ms} WHERE spell_id = 21169"
        ));
        ankh(&shard);
        shard.assert_call("debug_set_health", &["1", "0"]);
        assert_eq!(option_spell(&shard), Some(21169));
        let result = connection
            .call("native_world_receive", json!([0x2b3u32, Vec::<u8>::new()]))
            .unwrap();
        assert_alive_in_place(&entity(&shard));
        let cooldowns = cooldown_packets(
            result.inserts("game_native_packet"),
            connection.connection_id(),
        );
        let expected = if duration_ms == 0 {
            vec![]
        } else {
            vec![(PLAYER, 21169, duration_ms)]
        };
        assert_eq!(cooldowns, expected);
        if duration_ms == 0 {
            assert!(shard
                .query_rows("SELECT id FROM game_spell_cd WHERE caster_guid = 1")
                .is_empty());
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn reincarnation_requires_a_learned_spell_and_a_carried_ankh() {
    for (name, change) in [
        (
            "unlearned",
            "DELETE FROM game_player_spell WHERE character_guid = 1 AND spell_id = 20608",
        ),
        (
            "missing-ankh",
            "DELETE FROM game_item_instance WHERE owner_guid = 1 AND entry = 17030",
        ),
        (
            "banked-ankh",
            "UPDATE game_item_instance SET slot = 39 WHERE owner_guid = 1 AND entry = 17030",
        ),
    ] {
        let shard = start(&format!("self-res-{name}"));
        reincarnation(&shard);
        shard.assert_sql(change);
        shard.assert_call("debug_set_health", &["1", "0"]);
        assert_eq!(option_spell(&shard), None, "{name}");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn soulstone_takes_priority_without_spending_reincarnation() {
    let shard = start("self-res-priority");
    let mut connection = enter_native(&shard);
    reincarnation(&shard);
    soulstone(&shard);
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), Some(SOULSTONE_SELF_RES));
    let result = connection
        .call("native_world_receive", json!([0x2b3u32, Vec::<u8>::new()]))
        .unwrap();
    assert!(cooldown_packets(
        result.inserts("game_native_packet"),
        connection.connection_id(),
    )
    .is_empty());
    assert_eq!(
        shard
            .query_rows(
                "SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = 17030"
            )
            .len(),
        1
    );
    assert!(shard
        .query_rows("SELECT id FROM game_spell_cd WHERE caster_guid = 1 AND spell_id = 21169")
        .is_empty());
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), Some(21169));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn reincarnation_rechecks_its_requirements_before_spending_the_option() {
    let shard = start("self-res-recheck");
    let mut connection = enter_native(&shard);
    reincarnation(&shard);
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), Some(21169));
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1 AND entry = 17030");
    connection.drain_packets();
    let refusal = connection
        .call("native_world_receive", json!([0x2b3u32, Vec::<u8>::new()]))
        .unwrap_err();
    assert!(refusal.contains("native_reply:[]"), "{refusal}");
    assert!(cooldown_packets(&connection.drain_packets(), connection.connection_id()).is_empty());
    assert_eq!(entity(&shard)["dead"], "true");
    assert_eq!(option_spell(&shard), Some(21169));
    assert!(shard
        .query_rows("SELECT id FROM game_spell_cd WHERE caster_guid = 1 AND spell_id = 21169")
        .is_empty());
    ankh(&shard);
    shard.assert_call("debug_self_resurrect", &["1"]);
    assert_alive_in_place(&entity(&shard));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn reincarnation_uses_edited_reagents_and_cooldown_without_partial_consumption() {
    let shard = start("self-res-edited-cost");
    reincarnation(&shard);
    shard.assert_sql("UPDATE game_spell SET cooldown_ms = 120000 WHERE spell_id = 21169");
    shard.assert_sql(
        "UPDATE game_spell_reagent SET item_entry = 6948, count = 2 WHERE spell_id = 21169",
    );
    shard.insert_row(
        "game_spell_reagent",
        &[
            ("id", &169353),
            ("spell_id", &21169),
            ("item_entry", &17030),
            ("count", &3),
        ],
    );
    shard.assert_call("debug_grant_item", &["1", "6948", "1"]);
    shard.assert_sql(
        "UPDATE game_item_instance SET stack_count = 2 WHERE owner_guid = 1 AND entry = 6948",
    );
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), None);

    shard.assert_call("debug_set_health", &["1", "1000"]);
    shard.assert_sql(
        "UPDATE game_item_instance SET stack_count = 3 WHERE owner_guid = 1 AND entry = 17030",
    );
    shard.assert_call("debug_set_health", &["1", "0"]);
    assert_eq!(option_spell(&shard), Some(21169));
    shard.assert_sql(
        "UPDATE game_item_instance SET stack_count = 1 WHERE owner_guid = 1 AND entry = 17030",
    );
    let inventory = shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1");
    assert!(!shard.call("debug_self_resurrect", &["1"]).status.success());
    assert_eq!(
        shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1"),
        inventory
    );
    assert_eq!(entity(&shard)["dead"], "true");
    assert_eq!(option_spell(&shard), Some(21169));
    assert!(shard
        .query_rows("SELECT id FROM game_spell_cd WHERE caster_guid = 1")
        .is_empty());

    shard.assert_sql(
        "UPDATE game_item_instance SET stack_count = 3 WHERE owner_guid = 1 AND entry = 17030",
    );
    shard.assert_call("debug_self_resurrect", &["1"]);
    assert_alive_in_place(&entity(&shard));
    assert!(shard
        .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1")
        .is_empty());
    let cooldowns = shard.query_rows(
        "SELECT ready_at FROM game_spell_cd WHERE caster_guid = 1 AND spell_id = 21169",
    );
    assert_eq!(cooldowns.len(), 1);
    let ready_at = spacetimedb::Timestamp::parse_from_rfc3339(&cooldowns[0]["ready_at"])
        .unwrap()
        .to_micros_since_unix_epoch();
    assert_eq!(ready_at, 1_000_120_000_000);
}

fn enter_native(shard: &Standalone) -> ReducerConnection {
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([PLAYER]))
        .unwrap();
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1 AND entry = 6948");
    connection
}

fn cooldown_packets(rows: &[Box<RawValue>], connection_id: u128) -> Vec<(u64, u32, u32)> {
    #[derive(serde::Deserialize)]
    struct Packet(
        u64,
        (u8, Box<RawValue>),
        (u8, Box<RawValue>),
        String,
        bool,
        i64,
        bool,
    );

    rows.iter()
        .filter_map(|row| {
            let Packet(id, (audience, recipient), (option, opcode), body, close, delay, auth) =
                serde_json::from_str(row.get()).unwrap();
            if option != 0 || opcode.get() != "308" {
                return None;
            }
            assert_ne!(id, 0);
            assert_eq!(audience, 0);
            assert_eq!(
                serde_json::from_str::<(u128,)>(recipient.get()).unwrap().0,
                connection_id
            );
            assert!(!close);
            assert_eq!(delay, 0);
            assert!(!auth);
            let body: Vec<u8> = (0..body.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&body[i..i + 2], 16).unwrap())
                .collect();
            assert_eq!(body.len(), 16);
            Some((
                u64::from_le_bytes(body[0..8].try_into().unwrap()),
                u32::from_le_bytes(body[8..12].try_into().unwrap()),
                u32::from_le_bytes(body[12..16].try_into().unwrap()),
            ))
        })
        .collect()
}

fn reincarnation(shard: &Standalone) {
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.insert_row(
        "game_conformance_clock",
        &[("id", &0), ("now_micros", &1000000000000u64)],
    );
    set_class(shard, 7, MANA, 2000);
    insert_spell(shard, 21169, "Reincarnation", 0);
    shard.assert_sql("UPDATE game_spell SET cooldown_ms = 3600000 WHERE spell_id = 21169");
    shard.insert_row(
        "game_spell_reagent",
        &[
            ("id", &169352),
            ("spell_id", &21169),
            ("item_entry", &17030),
            ("count", &1),
        ],
    );
    insert_effect(
        shard,
        21169,
        u32::from(E_SELF_RESURRECT),
        20,
        u32::from(T_SELF),
        0,
        0,
    );
    shard.assert_call("debug_learn_spell", &["1", "20608"]);
    ankh(shard);
}

fn ankh(shard: &Standalone) {
    shard.assert_call("debug_grant_item", &["1", "6948", "1"]);
    shard.assert_sql(
        "UPDATE game_item_instance SET entry = 17030 WHERE owner_guid = 1 AND entry = 6948",
    );
}

fn start(name: &str) -> Standalone {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, max_health = 1000, health = 1000 WHERE guid = {PLAYER}"
    ));
    insert_spell(
        &standalone,
        SOULSTONE_AURA,
        "Soulstone Resurrection",
        1_800_000,
    );
    insert_effect(
        &standalone,
        SOULSTONE_AURA,
        u32::from(A_SELF_RESURRECT),
        0,
        u32::from(T_TARGET_ALLY),
        SOULSTONE_SELF_RES as i32,
        u32::from(P_SPELL_ID),
    );
    insert_spell(&standalone, SOULSTONE_SELF_RES, "Use Soulstone", 0);
    insert_effect(
        &standalone,
        SOULSTONE_SELF_RES,
        u32::from(E_SELF_RESURRECT),
        -(SOULSTONE_HEALTH as i32),
        u32::from(T_SELF),
        SOULSTONE_MANA as i32,
        u32::from(P_FLAT_MANA),
    );
    standalone
}

fn soulstone(standalone: &Standalone) {
    standalone.assert_call(
        "debug_cast_at",
        &[
            &PLAYER.to_string(),
            &SOULSTONE_AURA.to_string(),
            &PLAYER.to_string(),
        ],
    );
    assert!(has_aura(standalone, SOULSTONE_AURA));
}

fn set_class(standalone: &Standalone, class: u32, power_type: u32, max_power: u32) {
    let bytes = number(&entity(standalone), "unit_bytes_0");
    let bytes = (bytes & 0x00FF_00FF) | (class << 8) | (power_type << 24);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_bytes_0 = {bytes}, max_power = {max_power}, \
         power = {max_power} WHERE guid = {PLAYER}"
    ));
}

fn hold_vitals(standalone: &Standalone) {
    let flags = number(&entity(standalone), "unit_flags");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_flags = {}, combat_until_ms = {}, \
         mana_regen_paused_until_ms = {} WHERE guid = {PLAYER}",
        flags | unit_flags::IN_COMBAT,
        u64::MAX,
        u64::MAX,
    ));
}

fn assert_alive_in_place(player: &SqlRow) {
    assert_eq!(player["dead"], "false");
    assert_eq!(number(player, "player_flags") & player_flags::GHOST, 0);
    assert_eq!(number(player, "unit_bytes_1") & unit_vis_flags::GHOST, 0);
}

fn insert_spell(standalone: &Standalone, spell_id: u32, name: &str, duration_ms: u32) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("duration_ms", &duration_ms),
            ("school_mask", &32),
        ],
    );
}

fn insert_effect(
    standalone: &Standalone,
    spell_id: u32,
    kind: u32,
    base_points: i32,
    target: u32,
    p0: i32,
    p0_kind: u32,
) {
    let id = packed_spell_effect_id(spell_id, 0);
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &id),
            ("spell_id", &spell_id),
            ("kind", &kind),
            ("base_points", &base_points),
            ("die_sides", &1),
            ("target", &target),
            ("p_0", &p0),
            ("p_0_kind", &p0_kind),
        ],
    );
}

fn entity(standalone: &Standalone) -> SqlRow {
    standalone
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .pop()
        .expect("the Character must be live")
}

fn number(row: &SqlRow, column: &str) -> u32 {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is a number: {}", row[column]))
}

fn option_spell(standalone: &Standalone) -> Option<u32> {
    standalone
        .query_rows(&format!(
            "SELECT spell_id FROM game_self_resurrect_option WHERE character_guid = {PLAYER}"
        ))
        .pop()
        .map(|row| number(&row, "spell_id"))
}

fn has_aura(standalone: &Standalone, spell_id: u32) -> bool {
    !standalone
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {PLAYER} AND spell_id = {spell_id}"
        ))
        .is_empty()
}

fn corpse_count(standalone: &Standalone) -> usize {
    standalone
        .query_rows(&format!(
            "SELECT guid FROM game_corpse WHERE owner_guid = {PLAYER}"
        ))
        .len()
}
