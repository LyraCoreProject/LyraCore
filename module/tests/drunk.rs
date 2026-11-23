mod support;

use std::time::{Duration, Instant};

use lyracore_package_delta::fixture_ids::FIXTURE_JERKY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage;
use lyracore_shared::spell::taxonomy::{E_INEBRIATE, T_SELF};
use serde_json::json;
use support::{number, Standalone};

const PLAYER: u64 = 1;
const ALE: u32 = 5_116_301;
// vmangos Spell::EffectInebriate adds 256 per point: 50 points reach 12800, the drunk tier.
const ALE_POINTS: i32 = 50;
const DRUNK: u32 = 12_800;
// vmangos Player::HandleSobering: 256 every 10 s, 200 ticks of the 50 ms virtual clock.
const SOBERING_STEP: u32 = 256;
const TEN_SECONDS_TICKS: &str = "200";
const HALF_SECOND_TICKS: &str = "10";
const SMSG_UPDATE_OBJECT: u64 = 0xA9;
const PACKET_WAIT: Duration = Duration::from_secs(10);

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_drink_makes_the_character_drunk_and_player_bytes_3_carries_it() {
    let shard = stage("drunk-drink");
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([PLAYER]))
        .unwrap();
    wire.drain_packets();

    drink(&shard);

    assert_eq!(
        player_bytes_3(&shard),
        DRUNK,
        "gender 0 and drunk value 12800"
    );
    let expected =
        lyracore_protocol::world::encode(&ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            lyracore_protocol::codec::build_player_bytes_3_values(PLAYER, DRUNK),
        )))
        .unwrap()
        .body
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let carries_drunk_value = |row: &str| {
        let packet: serde_json::Value = serde_json::from_str(row).unwrap();
        packet[2][1] == SMSG_UPDATE_OBJECT && packet[3].as_str() == Some(expected.as_str())
    };
    let deadline = Instant::now() + PACKET_WAIT;
    let mut packets = wire.wait_for_packets(Duration::from_secs(1)).unwrap();
    while !packets.iter().any(|row| carries_drunk_value(row.get())) && Instant::now() < deadline {
        packets.extend(wire.wait_for_packets(Duration::from_secs(1)).unwrap());
    }
    assert!(
        packets.iter().any(|row| carries_drunk_value(row.get())),
        "the Character's VALUES update carries PLAYER_BYTES_3 with the drunk value"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_drunk_character_sobers_by_256_every_ten_seconds() {
    let shard = stage("drunk-sobering");
    drink(&shard);
    assert_eq!(player_bytes_3(&shard), DRUNK);

    advance(&shard, TEN_SECONDS_TICKS);
    advance(&shard, HALF_SECOND_TICKS);
    assert_eq!(player_bytes_3(&shard), DRUNK - SOBERING_STEP);
    advance(&shard, TEN_SECONDS_TICKS);
    assert_eq!(player_bytes_3(&shard), DRUNK - 2 * SOBERING_STEP);

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET player_bytes_3 = {SOBERING_STEP} WHERE guid = {PLAYER}"
    ));
    advance(&shard, TEN_SECONDS_TICKS);
    assert_eq!(
        player_bytes_3(&shard),
        0,
        "the last step leaves the Character sober"
    );
    assert!(
        shard
            .query_rows(&format!(
                "SELECT character_guid FROM game_drunk_sobering WHERE character_guid = {PLAYER}"
            ))
            .is_empty(),
        "a sober Character keeps no sobering timer"
    );
}

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.insert_row(
        "game_spell",
        &[("spell_id", &ALE), ("name", &"Ale fixture")],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(ALE, 0)),
            ("spell_id", &ALE),
            ("kind", &E_INEBRIATE),
            ("base_points", &ALE_POINTS),
            ("target", &T_SELF),
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET spellid_1 = {ALE}, spelltrigger_1 = 0 \
         WHERE entry = {FIXTURE_JERKY}"
    ));
    shard.assert_call(
        "debug_grant_item",
        &[&PLAYER.to_string(), &FIXTURE_JERKY.to_string(), "1"],
    );
    shard
}

fn drink(shard: &Standalone) {
    let slot = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_JERKY}"
    ))[0]["slot"]
        .clone();
    shard.assert_call("debug_use_item", &[&PLAYER.to_string(), &slot]);
}

fn player_bytes_3(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT player_bytes_3 FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0],
        "player_bytes_3",
    )
}

fn advance(shard: &Standalone, ticks: &str) {
    shard.assert_call("debug_advance_roll_batch", &[ticks]);
}
