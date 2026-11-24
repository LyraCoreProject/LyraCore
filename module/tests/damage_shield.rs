mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{A_DAMAGE_SHIELD, T_SELF};
use lyracore_test_support::NativePacket;
use serde_json::json;
use std::time::{Duration, Instant};
use support::{
    event_rows, fence_subscription, subscribe_with_fence, Standalone, TableSubscription,
};

type SqlRow = std::collections::BTreeMap<String, String>;

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const THORNS: u32 = 5_091_000;
const NATURE: u8 = 8;
const REFLECTED: u32 = 7;
const OUT_OF_REACH_LEVEL: u32 = 60;
const AVOIDED_SWINGS: usize = 2;
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live";
const SMSG_SPELLORDAMAGE_IMMUNE: u16 = 0x263;
const WOLF_SWING_ATTEMPTS: usize = 20;
const PACKET_WAIT: Duration = Duration::from_secs(10);

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_connecting_white_swing_takes_the_bearers_shield_damage_and_an_avoided_one_does_not() {
    let mut standalone = Standalone::start("damage-shield-white-swing");
    let wolf = stage(&mut standalone);
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    let events = subscribe_with_fence(&standalone, &[SWING_EVENTS]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    for count in 1..=AVOIDED_SWINGS {
        swing(&standalone);
        fence_subscription(&standalone, &events);
        let outcomes = player_swings(&events);
        assert_eq!(outcomes.len(), count);
        assert_eq!(outcomes[count - 1]["hit_info"], "2");
    }
    assert_eq!(
        health(&standalone, PLAYER),
        PLAYER_HEALTH - wolf_damage(&events, wolf),
        "an avoided white swing triggers no damage shield"
    );

    set_level(&standalone, wolf, 1);
    standalone
        .assert_sql("UPDATE game_conformance_swing_random SET state = 1 WHERE character_guid = 1");
    swing(&standalone);
    fence_subscription(&standalone, &events);
    let outcomes = player_swings(&events);
    assert_eq!(outcomes.len(), AVOIDED_SWINGS + 1);
    assert!(outcomes[AVOIDED_SWINGS]["damage"].parse::<u32>().unwrap() > 0);
    assert_eq!(
        health(&standalone, PLAYER),
        PLAYER_HEALTH - REFLECTED - wolf_damage(&events, wolf),
        "a connecting white swing takes the shield's damage once"
    );
    assert_eq!(
        standalone
            .query_rows(&format!(
                "SELECT amount FROM game_aura WHERE target_guid = {wolf} AND spell_id = {THORNS}"
            ))
            .iter()
            .map(|row| row["amount"].clone())
            .collect::<Vec<_>>(),
        [REFLECTED.to_string()],
        "reflection does not consume the shield"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_whose_template_is_immune_to_the_shields_school_takes_no_shield_damage() {
    let mut standalone = Standalone::start("damage-shield-template-immunity");
    let wolf = stage(&mut standalone);
    let mut wire = standalone.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([PLAYER]))
        .unwrap();
    standalone.assert_call("debug_force_cast_at", &["1", &THORNS.to_string(), "1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_creature_template SET school_immune_mask = {NATURE} \
         WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    let events = subscribe_with_fence(&standalone, &[SWING_EVENTS]);
    wire.drain_packets();
    standalone.assert_call("debug_engage", &[&wolf.to_string(), "1"]);

    for _ in 0..WOLF_SWING_ATTEMPTS {
        if wolf_damage(&events, wolf) > 0 {
            break;
        }
        standalone.assert_sql(&format!(
            "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {wolf}"
        ));
        standalone.assert_call("debug_advance_roll_batch", &["4"]);
        fence_subscription(&standalone, &events);
    }
    assert!(wolf_damage(&events, wolf) > 0, "a wolf swing connects");
    assert_eq!(
        health(&standalone, wolf),
        WOLF_HEALTH,
        "the shield deals the immune wolf no damage"
    );
    let immune = [PLAYER.to_le_bytes(), wolf.to_le_bytes()].concat();
    let reports_immunity = |packet: &NativePacket| {
        packet.opcode == Some(SMSG_SPELLORDAMAGE_IMMUNE) && packet.body.starts_with(&immune)
    };
    let deadline = Instant::now() + PACKET_WAIT;
    let mut packets = wire.wait_for_packets(Duration::from_secs(1)).unwrap();
    while !packets.iter().any(reports_immunity) && Instant::now() < deadline {
        packets.extend(wire.wait_for_packets(Duration::from_secs(1)).unwrap());
    }
    assert!(
        packets.iter().any(reports_immunity),
        "the bearer reports the immunity with SMSG_SPELLORDAMAGE_IMMUNE"
    );
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
    ));
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &THORNS),
            ("name", &"Damage shield fixture"),
            ("school_mask", &NATURE),
            ("duration_ms", &600_000),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(THORNS, 0)),
            ("spell_id", &THORNS),
            ("kind", &A_DAMAGE_SHIELD),
            ("base_points", &REFLECTED),
            ("target", &T_SELF),
        ],
    );
    standalone.assert_call(
        "debug_force_cast_at",
        &[&wolf.to_string(), &THORNS.to_string(), &wolf.to_string()],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn swing(standalone: &Standalone) {
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &["4"]);
}

fn set_level(standalone: &Standalone, guid: u64, level: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = {level} WHERE guid = {guid}"
    ));
}

fn player_swings(events: &TableSubscription) -> Vec<SqlRow> {
    event_rows(events, "game_combat_event_live")
        .into_iter()
        .filter(|row| row["attacker_guid"] == PLAYER.to_string())
        .collect()
}

fn wolf_damage(events: &TableSubscription, wolf: u64) -> u32 {
    event_rows(events, "game_combat_event_live")
        .iter()
        .filter(|row| row["attacker_guid"] == wolf.to_string())
        .map(|row| row["damage"].parse::<u32>().unwrap())
        .sum()
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
