use std::io::Cursor;
use std::time::Duration;

use lyracore_package_delta::fixture_ids::{FIXTURE_JERKY, SCENARIO_VENDOR_ENTRY, TEST_WOLF_ENTRY};
use lyracore_test_support::{
    Field, NativeAudience, NativePacket, ReducerConnection, Standalone, WorldSessionObserver,
};
use serde_json::json;
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

#[path = "conformance/buy_in_slot.rs"]
mod buy_in_slot;
#[path = "conformance/buyback.rs"]
mod buyback;
#[path = "../../conformance/support/character_world.rs"]
mod character_world;
#[path = "conformance/item_equip.rs"]
mod item_equip;
#[path = "conformance/item_learn.rs"]
mod item_learn;
#[path = "conformance/item_loot.rs"]
mod item_loot;
#[path = "conformance/loot_refusals.rs"]
mod loot_refusals;
#[path = "conformance/passives.rs"]
mod passives;
#[path = "../../conformance/support/properties/mod.rs"]
mod properties;
#[path = "conformance/quest_availability.rs"]
mod quest_availability;
#[path = "conformance/quest_reward_spell.rs"]
mod quest_reward_spell;
#[path = "conformance/quest_turn_in.rs"]
mod quest_turn_in;
#[path = "../../conformance/support/quest_world.rs"]
mod quest_world;
#[path = "../../conformance/support/queued_strike_rolls.rs"]
mod queued_strike_spec;
#[path = "conformance/queued_strikes.rs"]
mod queued_strikes;
#[path = "../../conformance/support/ranged_swing_rolls.rs"]
mod ranged_swing_spec;
#[path = "conformance/ranged_swings.rs"]
mod ranged_swings;
#[path = "conformance/repair.rs"]
mod repair;
#[path = "conformance/reputation.rs"]
mod reputation;
#[path = "conformance/rolls.rs"]
mod rolls;
#[path = "conformance/skills.rs"]
mod skills;
#[path = "conformance/spell_rolls.rs"]
mod spell_rolls;
#[path = "../../conformance/support/spell_world.rs"]
mod spell_world;
#[path = "conformance/strike_collector.rs"]
mod strike_collector;
#[path = "../../conformance/support/strike_rolls.rs"]
mod strike_rolls;
#[path = "conformance/taxi.rs"]
mod taxi;
#[path = "../../conformance/support/taxi_route.rs"]
mod taxi_route;
#[path = "conformance/tier1.rs"]
mod tier1;
#[path = "../../conformance/support/transaction_world.rs"]
mod transaction_world;
#[path = "conformance/transport.rs"]
mod transport;
#[path = "../../conformance/support/weapon_strike_rolls.rs"]
mod weapon_strike_spec;
#[path = "conformance/weapon_strikes.rs"]
mod weapon_strikes;

fn fixture(name: &str) -> (Standalone, u64, String) {
    fixture_in(Standalone::start(name))
}

fn fixture_in(mut shard: Standalone) -> (Standalone, u64, String) {
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"CONFORMANCE\"", &credentials, &credentials],
    );
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]
        ["id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Conformance\"",
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let guid: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Conformance'")
        [0]["guid"]
        .parse()
        .unwrap();

    (shard, guid, account)
}

fn enter(shard: &Standalone, guid: u64) -> ReducerConnection {
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([guid]))
        .unwrap();
    connection
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn native_login_calls_nearby_pack_assistance() {
    use lyracore_shared::constants::unit_flags;

    let (shard, character, _) = fixture("native-login-assistance");
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("DELETE FROM game_faction_template WHERE id = 1 OR id = 14");
    shard.insert_rows(
        "game_faction_template",
        &[
            &[
                ("id", &1),
                ("faction", &1),
                ("faction_group", &3),
                ("friend_group", &2),
                ("enemy_group", &12),
            ],
            &[
                ("id", &14),
                ("faction", &14),
                ("faction_group", &8),
                ("enemy_group", &1),
                ("flags", &1),
            ],
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0, orientation = 0, level = 1 WHERE guid = {character}"
    ));
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    for distance in [15, 23] {
        shard.assert_call(
            "debug_spawn_at_feet",
            &[
                &character.to_string(),
                &TEST_WOLF_ENTRY.to_string(),
                &distance.to_string(),
            ],
        );
    }
    let creatures = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} AND (x = 15 OR x = 23)"
    ));
    assert_eq!(creatures.len(), 2);
    assert!(shard
        .query_rows("SELECT * FROM game_melee_attack")
        .is_empty());

    let engaged = || {
        let mut engaged = shard
            .query_rows(&format!(
                "SELECT attacker_guid FROM game_melee_attack WHERE target_guid = {character}"
            ))
            .into_iter()
            .map(|row| row["attacker_guid"].parse::<u64>().unwrap())
            .collect::<Vec<_>>();
        engaged.sort_unstable();
        engaged
    };
    let near = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} AND x = 15"
    ))[0]["guid"]
        .parse::<u64>()
        .unwrap();
    let _connection = enter(&shard, character);
    assert_eq!(
        engaged(),
        [near],
        "the nearer wolf notices the Character on login"
    );
    shard.assert_call("debug_advance_roll_batch", &["30"]);
    let mut expected = creatures
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .collect::<Vec<_>>();
    expected.sort_unstable();
    assert_eq!(
        engaged(),
        expected,
        "its pack mate answers the call for assistance 1.5 seconds later"
    );
    for guid in expected.into_iter().chain([character]) {
        let flags = shard.query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {guid}"
        ))[0]["unit_flags"]
            .parse::<u32>()
            .unwrap();
        assert_ne!(flags & unit_flags::IN_COMBAT, 0, "{guid} must be in combat");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_purchases_acknowledge_the_offer_before_the_npc_receipt() {
    let (shard, guid, _) = fixture("vendor-purchase-replies");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET buy_count = 5 WHERE entry = {FIXTURE_JERKY}"
    ));
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&guid.to_string(), &SCENARIO_VENDOR_ENTRY.to_string(), "2.0"],
    );
    let vendor: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_VENDOR_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}"
    ));
    let mut connection = enter(&shard, guid);
    connection.drain_packets();
    for (count, expected_slot) in [(2u8, None), (1, Some(u32::MAX)), (0, Some(u32::MAX))] {
        let mut body = vendor.to_le_bytes().to_vec();
        body.extend(FIXTURE_JERKY.to_le_bytes());
        body.extend([count, 1]);
        connection
            .call("native_world_receive", json!([0x1a2u32, body]))
            .unwrap();
        let packets: Vec<_> = connection
            .drain_packets()
            .into_iter()
            .filter_map(|packet| {
                let opcode = packet.opcode?;
                matches!(opcode, 0x1a4 | 0x166).then_some((opcode, packet.body))
            })
            .collect();
        assert_eq!(
            packets.iter().map(|p| p.0).collect::<Vec<_>>(),
            [0x1a4, 0x166]
        );
        let mut acknowledgement = vendor.to_le_bytes().to_vec();
        acknowledgement.extend(2u32.to_le_bytes());
        acknowledgement.extend(u32::MAX.to_le_bytes());
        acknowledgement.extend(u32::from(count.max(1)).to_le_bytes());
        assert_eq!(packets[0].1, acknowledgement);
        let receipt = &packets[1].1;
        assert_eq!(u32::from_le_bytes(receipt[8..12].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(receipt[25..29].try_into().unwrap()),
            FIXTURE_JERKY
        );
        assert_eq!(
            u32::from_le_bytes(receipt[37..41].try_into().unwrap()),
            u32::from(count.max(1)) * 5
        );
        if let Some(slot) = expected_slot {
            assert_eq!(
                u32::from_le_bytes(receipt[21..25].try_into().unwrap()),
                slot
            );
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn quest_slots_survive_reconnect_and_reuse_only_the_abandoned_slot() {
    use std::collections::BTreeMap;

    fn fields(packets: Vec<NativePacket>, guid: u64) -> BTreeMap<u16, u32> {
        let mut fields = BTreeMap::new();
        for packet in packets {
            let Some(opcode) = packet.opcode else {
                continue;
            };
            for update in properties::updates(opcode, &packet.body).unwrap() {
                if update.guid == guid {
                    fields.extend(update.fields);
                }
            }
        }
        fields
    }

    let (shard, guid, _) = fixture("quest-slot-reconnect");
    for statement in quest_world::STATEMENTS {
        shard.assert_sql(statement);
    }
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    for quest in [15, 7] {
        shard.assert_call(
            "debug_grant_quest",
            &[&guid.to_string(), &quest.to_string()],
        );
    }
    let mut first = enter(&shard, guid);
    first.drain_packets();
    first
        .call("native_world_receive", json!([0x193u32, [0u8, 19]]))
        .unwrap();
    let swapped = fields(first.drain_packets(), guid);
    assert_eq!(swapped.get(&198), Some(&0));
    assert_eq!(swapped.get(&255), Some(&15));
    drop(first);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || {
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {guid}"
                ))
                .is_empty()
        }
    ));
    let mut second = enter(&shard, guid);
    let restored = fields(second.drain_packets(), guid);
    assert_eq!(restored.get(&201), Some(&7));
    assert_eq!(restored.get(&255), Some(&15));
    second
        .call("native_world_receive", json!([0x194u32, [19u8]]))
        .unwrap();
    assert_eq!(fields(second.drain_packets(), guid).get(&255), Some(&0));
    shard.assert_call("debug_grant_quest", &[&guid.to_string(), "433"]);
    second
        .call("native_world_receive", json!([0x1dcu32, vec![0u8; 8]]))
        .unwrap();
    let accepted = fields(second.drain_packets(), guid);
    assert_eq!(accepted.get(&198), Some(&433));
    assert!(!accepted.contains_key(&201));
    drop(second);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || {
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {guid}"
                ))
                .is_empty()
        }
    ));
    shard.assert_sql("DELETE FROM game_character_quest_log");
    let mut legacy = enter(&shard, guid);
    let projected = fields(legacy.drain_packets(), guid);
    assert_eq!(projected.get(&198), Some(&7));
    assert_eq!(projected.get(&201), Some(&433));
    legacy
        .call("native_world_receive", json!([0x194u32, [0u8]]))
        .unwrap();
    assert_eq!(fields(legacy.drain_packets(), guid).get(&198), Some(&0));
    let rows = shard.query_rows(&format!(
        "SELECT entries FROM game_character_quest_log WHERE character_guid = {guid}"
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["entries"], "0433000000000000000000");
}

fn packet(packet: &NativePacket, connection_id: u128, opcode: u16) -> wire_client::decode::Packet {
    assert_ne!(packet.id, 0);
    assert_eq!(packet.audience, NativeAudience::Connection(connection_id));
    assert_eq!(packet.opcode, Some(opcode));
    assert!(!packet.close_after);
    assert_eq!(packet.not_before_micros, 0);
    assert!(!packet.authentication_complete);
    wire_client::decode::decode(opcode, &packet.body).unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn operator_connection_enters_world_and_receives_pong() {
    let (shard, guid, _) = fixture("native-conformance");
    let accounts = shard.query_rows("SELECT * FROM game_account");
    let mut anonymous = shard.connect_reducers_anonymous(&[]);
    let refused = anonymous
        .call("debug_native_enter_world", json!([guid]))
        .unwrap_err();
    assert!(refused.contains("operator"), "{refused}");
    let mut connection = enter(&shard, guid);
    assert_eq!(shard.query_rows("SELECT * FROM game_account"), accounts);
    assert!(shard.query_rows("SELECT * FROM game_session").is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_native_cipher")
        .is_empty());
    assert_eq!(
        shard.query_rows("SELECT * FROM game_native_player").len(),
        1
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT * FROM game_world_entity WHERE guid = {guid}"
            ))
            .len(),
        1
    );
    let sequence = 0x12345678u32;
    let body: Vec<u8> = sequence
        .to_le_bytes()
        .into_iter()
        .chain(25u32.to_le_bytes())
        .collect();
    let result = connection
        .call("native_world_receive", json!([0x1dcu32, body]))
        .unwrap();
    let rows = result.packets();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let pong = packet(&rows[0], connection.connection_id(), 0x1dd);
    assert_eq!(
        pong.fields["sequence"],
        wire_client::decode::Field::Unsigned(sequence.into())
    );
    assert!(shard
        .query_rows("SELECT * FROM game_native_packet")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missing_near_teleport_ack_expires_and_world_requests_resume() {
    let (shard, guid, _) = fixture("native-teleport-expiry");
    let mut connection = enter(&shard, guid);
    connection.drain_packets();
    shard.assert_call(
        "debug_teleport",
        &[&guid.to_string(), "0", "20", "0", "0", "0"],
    );
    connection.drain_packets();

    let mut query = || {
        connection
            .call("native_world_receive", json!([0x1ceu32, Vec::<u8>::new()]))
            .unwrap()
    };
    assert!(query().packets().is_empty());
    std::thread::sleep(Duration::from_secs(21));
    let response = query();
    assert!(response
        .packets()
        .iter()
        .any(|packet| packet.opcode == Some(463)));
    let has_heartbeat = |packet: &NativePacket| packet.opcode == Some(238);
    assert!(
        response.packets().iter().any(has_heartbeat)
            || connection.drain_packets().iter().any(has_heartbeat)
    );
    assert_eq!(
        shard.query_rows("SELECT teleport FROM game_native_player")[0]["teleport"],
        "(none = ())"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn fixture_entry_preserves_login_credentials_and_refuses_the_old_fixture_key() {
    let (shard, guid, account) = fixture("native-conformance-session");
    let key = serde_json::to_string(&vec![0x37u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &[&account, &key, r#"{"__identity__":"0x1234"}"#],
    );
    let sessions = shard.query_rows("SELECT * FROM game_session");
    let accounts = shard.query_rows("SELECT * FROM game_account");
    let connection = enter(&shard, guid);
    assert_eq!(shard.query_rows("SELECT * FROM game_session"), sessions);
    assert_eq!(shard.query_rows("SELECT * FROM game_account"), accounts);
    assert!(shard
        .query_rows("SELECT * FROM game_native_cipher")
        .is_empty());

    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    let mut frame = Vec::new();
    CMSG_AUTH_SESSION {
        build: 5875,
        server_id: 0,
        username: "CONFORMANCE".into(),
        client_seed: 0,
        // SHA-1 for CONFORMANCE, client seed 0, server seed 42 and key [0x5a; 40].
        client_proof: [
            0, 219, 190, 39, 108, 63, 12, 6, 223, 186, 9, 97, 43, 173, 114, 133, 133, 42, 164, 178,
        ],
        addon_info: vec![],
    }
    .write_unencrypted_client(&mut Cursor::new(&mut frame))
    .unwrap();
    let rows = wire
        .call("native_world_receive", json!([0x1edu32, &frame[6..]]))
        .unwrap();
    let response = &rows.packets()[0];
    assert_eq!(response.opcode, Some(494));
    // SMSG_AUTH_RESPONSE carries AUTH_FAILED as the single result byte 13.
    assert_eq!(response.body, [13]);
    assert!(response.close_after);
    drop(connection);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || {
            shard
                .query_rows("SELECT * FROM game_native_conformance_connection")
                .is_empty()
        }
    ));
    assert_eq!(shard.query_rows("SELECT * FROM game_session"), sessions);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_later_transaction_delivers_packets_without_another_client_request() {
    let (shard, guid, _) = fixture("native-conformance-later");
    let mut connection = enter(&shard, guid);
    assert!(!connection.drain_packets().is_empty());
    shard.assert_call("debug_set_money", &[&guid.to_string(), "12345"]);
    let expected = lyracore_protocol::world::encode(
        &lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(lyracore_protocol::codec::build_coinage_values(guid, 12345)),
        ),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut rows = Vec::new();
    while std::time::Instant::now() < deadline {
        rows.extend(connection.wait_for_packets(Duration::from_secs(1)).unwrap());
        if rows.iter().any(|packet| {
            matches!(packet.audience, NativeAudience::Connection(_))
                && packet.opcode == Some(169)
                && packet.body == expected.body
        }) {
            return;
        }
    }
    panic!("expected coinage values from the later transaction: {rows:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn character_health_reaches_strangers_as_a_percent_and_the_party_in_full() {
    use lyracore_shared::spell::taxonomy::{A_MOD_STAT, STAT_STA, T_SELF};
    const STAMINA_BUFF: u32 = 5_090_410;

    let (shard, subject, _) = fixture("native-health-visibility");
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {subject}"
    ));
    let viewer = add_observer(&shard, 5, 0);
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &STAMINA_BUFF),
            ("name", &"Stamina fixture"),
            ("school_mask", &1),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &(STAMINA_BUFF * 4)),
            ("spell_id", &STAMINA_BUFF),
            ("kind", &A_MOD_STAT),
            ("base_points", &10),
            ("target", &T_SELF),
            ("p_0", &STAT_STA),
            ("p_0_kind", &1),
        ],
    );
    let mut own = enter(&shard, subject);
    let other = enter(&shard, viewer);
    let (own_id, other_id) = (own.connection_id(), other.connection_id());
    let units = [subject, viewer];
    let solo = |unit: u64, connection: u128| {
        let (health, max_health) = vitals(&shard, unit);
        vec![
            (
                unit,
                HealthDelivery::aoi_except(connection),
                hidden(health, max_health),
            ),
            (
                unit,
                HealthDelivery::Connection(connection),
                (Some(health), Some(max_health)),
            ),
        ]
    };
    let party = |unit: u64| {
        let (health, max_health) = vitals(&shard, unit);
        vec![
            (
                unit,
                HealthDelivery::aoi_except(if unit == subject { own_id } else { other_id }),
                hidden(health, max_health),
            ),
            (
                unit,
                HealthDelivery::Connection(own_id),
                (Some(health), Some(max_health)),
            ),
            (
                unit,
                HealthDelivery::Connection(other_id),
                (Some(health), Some(max_health)),
            ),
        ]
    };

    let max_health = vitals(&shard, subject).1;
    let sent = health_after(&mut own, &units, || {
        set_health(&shard, subject, max_health / 2)
    });
    assert_eq!(
        sent,
        sorted(solo(subject, own_id)),
        "a stranger sees a percent"
    );

    let sent = health_after(&mut own, &units, || {
        shard.assert_call(
            "debug_force_cast",
            &[&subject.to_string(), &STAMINA_BUFF.to_string()],
        );
    });
    assert_ne!(
        vitals(&shard, subject).1,
        max_health,
        "Stamina must raise maximum health"
    );
    assert_eq!(
        sent,
        sorted(solo(subject, own_id)),
        "vmangos Unit::SetMaxHealth sends health with the new maximum"
    );

    group_op(&shard, 0, subject, viewer);
    let sent = health_after(&mut own, &units, || {
        group_op(&shard, 1, viewer, 0);
    });
    assert_eq!(
        sent,
        sorted(party(subject).into_iter().chain(party(viewer))),
        "vmangos Group::BroadcastGroupUpdate shows each member's health to the other"
    );

    let health = vitals(&shard, subject).1 / 3;
    let sent = health_after(&mut own, &units, || set_health(&shard, subject, health));
    assert_eq!(
        sent,
        sorted(party(subject)),
        "a party member sees the health itself"
    );

    let sent = health_after(&mut own, &units, || {
        group_op(&shard, 3, viewer, 0);
    });
    assert_eq!(
        sent,
        sorted(
            solo(subject, own_id)
                .into_iter()
                .chain(solo(viewer, other_id))
        ),
        "a former member sees a percent again"
    );

    let sent = health_after(&mut own, &units, || {
        mirror_group(&shard, 900, &[(subject, 100), (viewer, 100)]);
    });
    assert_eq!(
        sent,
        sorted(party(subject).into_iter().chain(party(viewer))),
        "a mirrored roster shows each member's health to the other"
    );
    let sent = health_after(&mut own, &units, || {
        mirror_group(&shard, 901, &[(viewer, 101)])
    });
    assert_eq!(
        sent,
        sorted(
            solo(subject, own_id)
                .into_iter()
                .chain(solo(viewer, other_id))
        ),
        "the member left behind in the old group sees a percent again"
    );
}

/// Pushes a Realm roster for `group_id` whose members carry their membership revisions.
fn mirror_group(shard: &Standalone, group_id: u64, members: &[(u64, u64)]) {
    let guids: Vec<u64> = members.iter().map(|(guid, _)| *guid).collect();
    let partitions: Vec<_> = members
        .iter()
        .map(|&(guid, revision)| {
            json!({
                "character_guid": guid, "group_id": group_id,
                "membership_revision": revision, "member_active": true,
                "map_id": 0, "instance_id": 0, "locator_revision": 0,
                "state": {"unknown": []},
            })
        })
        .collect();
    let args = [
        group_id.to_string(),
        guids[0].to_string(),
        "3".into(),
        "2".into(),
        "0".into(),
        json!(guids).to_string(),
        lyracore_test_support::actor(0),
        json!(partitions).to_string(),
        "1".into(),
        "0".into(),
        json!(vec![0u8; guids.len()]).to_string(),
    ];
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    shard.assert_call("sync_group_mirror", &args);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn creature_health_is_a_percent_and_a_pets_owner_sees_it_in_full() {
    let (shard, owner, _) = fixture("native-creature-health");
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {owner}"
    ));
    let mut own = enter(&shard, owner);
    let own_id = own.connection_id();
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&owner.to_string(), &TEST_WOLF_ENTRY.to_string(), "3"],
    );
    let wolf: u64 = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} AND x = 3"
    ))[0]["guid"]
        .parse()
        .unwrap();
    let max_health = vitals(&shard, wolf).1;

    let sent = health_after(&mut own, &[wolf], || {
        set_health(&shard, wolf, max_health / 3)
    });
    assert_eq!(
        sent,
        [(
            wolf,
            HealthDelivery::Aoi { except: None },
            hidden(max_health / 3, max_health)
        )],
        "every viewer sees a creature's health as a percent"
    );

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET owner_guid = {owner} WHERE guid = {wolf}"
    ));
    let health = max_health / 2;
    let sent = health_after(&mut own, &[wolf], || set_health(&shard, wolf, health));
    assert_eq!(
        sent,
        sorted(vec![
            (
                wolf,
                HealthDelivery::aoi_except(own_id),
                hidden(health, max_health)
            ),
            (
                wolf,
                HealthDelivery::Connection(own_id),
                (Some(health), Some(max_health))
            ),
        ]),
        "a pet's owner sees its health; a stranger sees a percent"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_auras_stacks_and_charges_reach_viewers_as_its_applications() {
    use lyracore_shared::spell::taxonomy::{
        A_MOD_RESISTANCE, A_MOD_STAT, P_SCHOOL_MASK, RESIST_ARMOR, STAT_STA, T_SELF,
    };
    const STACKING: u32 = 5_090_420;
    const CHARGED: u32 = 5_090_421;
    const PROC_FLAG_TAKEN_MELEE_HIT: u32 = 0x8;
    const NATURE: u8 = 8;

    let (shard, subject, _) = fixture("native-aura-applications");
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {subject}"
    ));
    shard.insert_rows(
        "game_spell",
        &[
            &[
                ("spell_id", &STACKING),
                ("name", &"Stacking fixture"),
                ("school_mask", &1),
                ("duration_ms", &600_000),
                ("max_stacks", &5),
                ("spell_level", &1),
            ],
            &[
                ("spell_id", &CHARGED),
                ("name", &"Charged fixture"),
                ("school_mask", &NATURE),
                ("duration_ms", &600_000),
                ("spell_level", &1),
                ("proc_flags", &PROC_FLAG_TAKEN_MELEE_HIT),
                ("proc_chance", &100),
                ("proc_charges", &3),
            ],
        ],
    );
    shard.insert_rows(
        "game_spell_effect",
        &[
            &[
                ("id", &(STACKING * 4)),
                ("spell_id", &STACKING),
                ("kind", &A_MOD_STAT),
                ("base_points", &10),
                ("target", &T_SELF),
                ("p_0", &STAT_STA),
                ("p_0_kind", &1),
            ],
            &[
                ("id", &(CHARGED * 4)),
                ("spell_id", &CHARGED),
                ("kind", &A_MOD_RESISTANCE),
                ("base_points", &10),
                ("target", &T_SELF),
                ("p_0", &RESIST_ARMOR),
                ("p_0_kind", &P_SCHOOL_MASK),
            ],
        ],
    );
    let mut own = enter(&shard, subject);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&subject.to_string(), &TEST_WOLF_ENTRY.to_string(), "3"],
    );
    let wolf = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} AND x = 3"
    ))[0]["guid"]
        .clone();
    let cast = |spell: u32| {
        shard.assert_call(
            "debug_force_cast",
            &[&subject.to_string(), &spell.to_string()],
        )
    };
    let slot = |spell: u32| -> u8 {
        shard.query_rows(&format!(
            "SELECT slot FROM game_aura WHERE target_guid = {subject} AND spell_id = {spell}"
        ))[0]["slot"]
            .parse()
            .unwrap()
    };

    applications_after(&mut own, subject, || cast(STACKING));
    let words = applications_after(&mut own, subject, || cast(STACKING));
    assert_eq!(
        applications(&words, slot(STACKING)),
        1,
        "vmangos SpellAuraHolder::SetStackAmount sends two stacks as 1"
    );

    let words = applications_after(&mut own, subject, || cast(CHARGED));
    assert_eq!(
        applications(&words, slot(CHARGED)),
        2,
        "vmangos SpellAuraHolder::_AddSpellAuraHolder sends three charges as 2"
    );

    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    let words = applications_after(&mut own, subject, || {
        shard.assert_call("debug_apply_damage", &[&subject.to_string(), "1", &wolf]);
    });
    assert_eq!(
        applications(&words, slot(CHARGED)),
        1,
        "vmangos SpellAuraHolder::DropAuraCharge sends two charges left as 1"
    );
    assert_eq!(
        applications(&words, slot(STACKING)),
        1,
        "the stacking Aura keeps its count"
    );
}

/// The UNIT_FIELD_AURAAPPLICATIONS words of the last VALUES `action` sends to viewers of `unit`.
fn applications_after(
    connection: &mut ReducerConnection,
    unit: u64,
    action: impl FnOnce(),
) -> Vec<u32> {
    use wire_client::decode::Field;

    connection.drain_packets();
    action();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let rows = connection.wait_for_packets(Duration::from_secs(1)).unwrap();
        let mut sent = None;
        for packet in &rows {
            if !matches!(packet.audience, NativeAudience::Aoi { .. }) || packet.opcode != Some(169)
            {
                continue;
            }
            let decoded = wire_client::decode::decode(0x00a9, &packet.body).unwrap();
            let Some(Field::List(objects)) = decoded.fields.get("objects") else {
                continue;
            };
            for object in objects {
                let Field::Object(object) = object else {
                    continue;
                };
                let (Some(&Field::Guid(guid)), Some(Field::Object(values))) =
                    (object.get("guid"), object.get("values"))
                else {
                    continue;
                };
                let words: Option<Vec<u32>> = (113..125)
                    .map(|index| match values.get(&format!("field_{index}")) {
                        Some(Field::Unsigned(value)) => Some(u32::try_from(*value).unwrap()),
                        _ => None,
                    })
                    .collect();
                if guid == unit && words.is_some() {
                    sent = words;
                }
            }
        }
        if let Some(words) = sent {
            return words;
        }
    }
    panic!("no Aura VALUES reached the viewers of {unit}");
}

// Build 5875 packs one byte a slot, four slots a word.
fn applications(words: &[u32], slot: u8) -> u8 {
    (words[usize::from(slot / 4)] >> (slot % 4 * 8)) as u8
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum HealthDelivery {
    Connection(u128),
    Aoi { except: Option<u128> },
}

impl HealthDelivery {
    fn aoi_except(connection: u128) -> Self {
        Self::Aoi {
            except: Some(connection),
        }
    }
}

type SentVitals = (Option<u32>, Option<u32>);

// vmangos Object::BuildValuesUpdate sends health * 100 / maximum, truncated, and 100 maximum.
fn hidden(health: u32, max_health: u32) -> SentVitals {
    (Some(health * 100 / max_health), Some(100))
}

fn sorted<T: Ord>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut items: Vec<T> = items.into_iter().collect();
    items.sort_unstable();
    items
}

fn vitals(shard: &Standalone, unit: u64) -> (u32, u32) {
    let row = &shard.query_rows(&format!(
        "SELECT health, max_health FROM game_world_entity WHERE guid = {unit}"
    ))[0];
    (
        row["health"].parse().unwrap(),
        row["max_health"].parse().unwrap(),
    )
}

fn set_health(shard: &Standalone, unit: u64, health: u32) {
    shard.assert_call(
        "debug_set_health",
        &[&unit.to_string(), &health.to_string()],
    );
}

fn group_op(shard: &Standalone, op: u8, actor: u64, target: u64) {
    let actor = lyracore_test_support::actor(actor);
    let (op, target) = (op.to_string(), target.to_string());
    shard.assert_call("realm_group_op", &[&op, &actor, &target, "0", "0", "0"]);
}

/// The health and maximum health VALUES that `action` sends for `units`, sorted.
fn health_after(
    connection: &mut ReducerConnection,
    units: &[u64],
    action: impl FnOnce(),
) -> Vec<(u64, HealthDelivery, SentVitals)> {
    connection.drain_packets();
    action();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let rows = connection.wait_for_packets(Duration::from_secs(1)).unwrap();
        let sent = decoded_health(&rows, units);
        if !sent.is_empty() {
            for (aoi_id, unit, _, _) in sent
                .iter()
                .filter(|(_, _, delivery, _)| matches!(delivery, HealthDelivery::Aoi { .. }))
            {
                assert!(
                    sent.iter().all(|(id, other, delivery, _)| other != unit
                        || !matches!(delivery, HealthDelivery::Connection(_))
                        || id > aoi_id),
                    "the raw health of {unit} must follow its AOI percent: {sent:?}"
                );
            }
            return sorted(
                sent.into_iter()
                    .map(|(_, unit, delivery, vitals)| (unit, delivery, vitals)),
            );
        }
    }
    Vec::new()
}

fn decoded_health(
    packets: &[NativePacket],
    units: &[u64],
) -> Vec<(u64, u64, HealthDelivery, SentVitals)> {
    use wire_client::decode::Field;

    let field =
        |values: &std::collections::BTreeMap<String, Field>, name: &str| match values.get(name) {
            Some(Field::Unsigned(value)) => Some(u32::try_from(*value).unwrap()),
            _ => None,
        };
    let mut sent = Vec::new();
    for packet in packets {
        if packet.opcode != Some(169)
            || !matches!(
                packet.audience,
                NativeAudience::Connection(_) | NativeAudience::Aoi { .. }
            )
        {
            continue;
        }
        let decoded = wire_client::decode::decode(0x00a9, &packet.body).unwrap();
        let Some(Field::List(objects)) = decoded.fields.get("objects") else {
            continue;
        };
        for object in objects {
            let Field::Object(object) = object else {
                continue;
            };
            let Some(&Field::Guid(unit)) = object.get("guid") else {
                continue;
            };
            let Some(Field::Object(values)) = object.get("values") else {
                continue;
            };
            let vitals = (field(values, "field_22"), field(values, "field_28"));
            if object.get("kind") != Some(&Field::Unsigned(0))
                || !units.contains(&unit)
                || vitals == (None, None)
            {
                continue;
            }
            let delivery = match packet.audience {
                NativeAudience::Connection(connection) => HealthDelivery::Connection(connection),
                NativeAudience::Aoi {
                    entity_guid,
                    exclude_connection,
                    ..
                } => {
                    assert_eq!(entity_guid, unit, "{:?}", packet.audience);
                    HealthDelivery::Aoi {
                        except: exclude_connection,
                    }
                }
                _ => unreachable!(),
            };
            sent.push((packet.id, unit, delivery, vitals));
        }
    }
    sent
}

fn taxi_flight(name: &str, observer_x: i32) -> (Standalone, u64, u64) {
    taxi_flight_on_route(name, observer_x, 0, &[(0.0, 0.0, 0.0), (200.0, 0.0, 0.0)])
}

fn taxi_flight_on_route(
    name: &str,
    observer_x: i32,
    observer_y: i32,
    route: &[(f32, f32, f32)],
) -> (Standalone, u64, u64) {
    let (shard, passenger, _) = fixture(name);
    let &(x, y, z) = route.first().expect("taxi fixture needs a launch point");
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = {x}, y = {y}, z = {z} WHERE guid = {passenger}"
    ));
    let observer = add_observer(&shard, observer_x, observer_y);
    shard.assert_call("debug_spawn_player_entity", &[&passenger.to_string()]);
    start_taxi_flight_on_route(&shard, passenger, route);
    (shard, passenger, observer)
}

fn add_observer(shard: &Standalone, x: i32, y: i32) -> u64 {
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"OBSERVER\"", &credentials, &credentials],
    );
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'OBSERVER'")[0]
        ["id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Observer\"",
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let observer: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Observer'")
        [0]["guid"]
        .parse()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = {x}, y = {y} WHERE guid = {observer}"
    ));
    observer
}

fn taxi_create(
    observer: &mut WorldSessionObserver,
    packets: &[NativePacket],
    passenger: u64,
) -> Option<(
    wow_world_messages::vanilla::MovementBlock_MovementFlags_SplineEnabled,
    wow_world_messages::vanilla::Vector3d,
)> {
    use wow_world_messages::vanilla::{
        opcodes::ServerOpcodeMessage, MovementBlock_UpdateFlag_Living, Object,
    };
    observer.received(packets).iter().find_map(|packet| {
        if packet.opcode != Some(169) {
            return None;
        }
        let mut bytes = packet.body.clone();
        if !matches!(bytes.get(5), Some(2 | 3)) {
            return None;
        }
        let type_index = 7 + bytes[6].count_ones() as usize;
        if bytes.get(type_index) != Some(&4) {
            return None;
        }
        let flags_index = type_index + 1;
        assert_eq!(bytes[flags_index] & 0x70, 0x70);
        // gtker reads HAS_POSITION after LIVING, but build 5875 writes only LIVING.
        bytes[flags_index] &= !0x40;
        let mut frame = Vec::with_capacity(bytes.len() + 4);
        frame.extend_from_slice(&u16::try_from(bytes.len() + 2).unwrap().to_be_bytes());
        frame.extend_from_slice(&0x00a9u16.to_le_bytes());
        frame.extend_from_slice(&bytes);
        let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) =
            ServerOpcodeMessage::read_unencrypted(&mut frame.as_slice()).unwrap()
        else {
            unreachable!();
        };
        update.objects.into_iter().find_map(|object| {
            let (guid3, movement2) = match object {
                Object::CreateObject {
                    guid3, movement2, ..
                }
                | Object::CreateObject2 {
                    guid3, movement2, ..
                } => (guid3, movement2),
                _ => return None,
            };
            if guid3.guid() != passenger {
                return None;
            }
            let MovementBlock_UpdateFlag_Living::Living {
                flags,
                living_position,
                ..
            } = movement2.update_flag.get_living()?
            else {
                return None;
            };
            Some((flags.get_spline_enabled()?.clone(), *living_position))
        })
    })
}

fn client_spline_position(
    spline: &wow_world_messages::vanilla::MovementBlock_MovementFlags_SplineEnabled,
    elapsed_ms: u32,
) -> [f32; 3] {
    let curve = |segment: usize, t: f32| {
        let points = &spline.nodes[segment..segment + 4];
        std::array::from_fn(|axis| {
            let value = |index: usize| match axis {
                0 => points[index].x,
                1 => points[index].y,
                _ => points[index].z,
            };
            let (a, b, c, d) = (value(0), value(1), value(2), value(3));
            b + 0.5
                * t
                * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
        })
    };
    let mut from_ms = 0u32;
    let mut to_ms = 1u32;
    for segment in 0..spline.nodes.len() - 3 {
        let mut previous = curve(segment, 0.0);
        let mut length = 0.0f64;
        for step in 1..=3 {
            let current = curve(segment, step as f32 / 3.0);
            length += f64::from(
                current
                    .iter()
                    .zip(previous)
                    .map(|(next, old)| (next - old).powi(2))
                    .sum::<f32>()
                    .sqrt(),
            );
            previous = current;
        }
        to_ms = (to_ms as f32 + length as f32 * 31.25) as u32;
        if elapsed_ms < to_ms {
            let t = (elapsed_ms.saturating_sub(from_ms) as f32 / (to_ms - from_ms) as f32)
                .clamp(0.0, 1.0);
            return curve(segment, t);
        }
        from_ms = to_ms;
    }
    let last = spline.final_node;
    [last.x, last.y, last.z]
}

fn taxi_x(shard: &Standalone, guid: u64) -> f32 {
    shard.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {guid}"
    ))[0]["x"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_new_viewer_receives_the_current_taxi_spline() {
    let (shard, passenger, observer) = taxi_flight("taxi-new-viewer", 200);
    let mut viewer = enter(&shard, observer);
    let mut session = WorldSessionObserver::new(viewer.connection_id());
    session.received(&viewer.drain_packets());

    let mut spline = None;
    let mut observed = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while spline.is_none() && std::time::Instant::now() < deadline {
        let packets = viewer.wait_for_packets(Duration::from_secs(1)).unwrap();
        spline = taxi_create(&mut session, &packets, passenger);
        observed.extend(packets);
    }
    let (spline, position) = spline
        .unwrap_or_else(|| panic!("observer did not receive a taxi CREATE spline: {observed:?}"));
    let x = position.x;
    assert!((66.0..200.0).contains(&x), "obsolete spline start: {x}");
    assert_eq!(spline.nodes.len(), 4);
    assert_eq!((spline.nodes[1].x, spline.nodes[2].x), (0.0, 200.0));
    assert_eq!(spline.final_node.x, 200.0);
    assert!(spline.time_passed > 0);
    assert!(!observed.iter().any(|packet| {
        matches!(packet.audience, NativeAudience::Connection(_)) && packet.opcode == Some(221)
    }));
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT * FROM game_aoi_grant WHERE viewer_guid = {observer} AND entity_guid = {passenger}"
            ))
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_late_viewers_create_follows_the_authoritative_curved_flight() {
    let route = [
        (0.0, 0.0, 0.0),
        (250.0, 40.0, 0.0),
        (500.0, -40.0, 0.0),
        (750.0, 40.0, 0.0),
        (1000.0, -40.0, 0.0),
        (1250.0, 40.0, 0.0),
        (1400.0, -40.0, 0.0),
        (1550.0, 40.0, 0.0),
        (1700.0, -40.0, 0.0),
        (1850.0, 40.0, 0.0),
        (2000.0, 0.0, 0.0),
    ];
    let (shard, passenger, observer) = taxi_flight_on_route("taxi-curved-replay", 1550, 40, &route);
    let mut viewer = enter(&shard, observer);
    let mut session = WorldSessionObserver::new(viewer.connection_id());
    session.received(&viewer.drain_packets());
    let original_start: i64 = shard.query_rows(&format!(
        "SELECT started_micros FROM game_active_taxi_flight WHERE character_guid = {passenger}"
    ))[0]["started_micros"]
        .parse()
        .unwrap();
    let total_ms: u32 = shard.query_rows(&format!(
        "SELECT duration_ms FROM game_taxi_passenger_spline WHERE character_guid = {passenger}"
    ))[0]["duration_ms"]
        .parse()
        .unwrap();
    let accelerated_start = original_start - i64::from(total_ms) * 600;
    shard.assert_sql(&format!(
        "UPDATE game_active_taxi_flight SET started_micros = {accelerated_start} WHERE character_guid = {passenger}"
    ));

    let mut replay = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while replay.is_none() && std::time::Instant::now() < deadline {
        replay = taxi_create(
            &mut session,
            &viewer.wait_for_packets(Duration::from_secs(1)).unwrap(),
            passenger,
        );
    }
    let (spline, position) = replay.expect("late viewer received no flying CREATE");
    assert!(
        position.x < 1550.0,
        "viewer saw flight too late: {}",
        position.x
    );
    assert_eq!(spline.nodes.len(), route.len() + 2);
    assert_eq!(spline.nodes[1].x, 0.0);
    assert_eq!(spline.final_node.x, 2000.0);
    assert!(spline.duration >= total_ms);
    assert!(spline.time_passed > 0);

    for threshold in [1550.0, 1700.0, 1850.0] {
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        loop {
            let row = &shard.query_rows(&format!(
                "SELECT start_x,start_y,start_z,duration_ms FROM game_taxi_passenger_spline WHERE character_guid = {passenger}"
            ))[0];
            let authoritative: [f32; 3] =
                ["start_x", "start_y", "start_z"].map(|field| row[field].parse().unwrap());
            if authoritative[0] >= threshold {
                assert!(authoritative[0] < threshold + 60.0);
                let remaining_ms: u32 = row["duration_ms"].parse().unwrap();
                let elapsed_ms = spline.duration - remaining_ms;
                assert!(elapsed_ms > spline.time_passed);
                let client = client_spline_position(&spline, elapsed_ms);
                let distance = client
                    .iter()
                    .zip(authoritative)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt();
                assert!(
                    distance <= 1.0,
                    "at {threshold} yd: client {client:?}, flight {authoritative:?}, separation {distance} yd"
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "flight never reached {threshold}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn login_near_a_flyer_replays_its_current_spline() {
    let (shard, passenger, observer) = taxi_flight("taxi-login-near-flyer", 40);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(4),
        || { taxi_x(&shard, passenger) > 20.0 }
    ));

    let mut viewer = enter(&shard, observer);
    let mut session = WorldSessionObserver::new(viewer.connection_id());
    let (spline, position) = taxi_create(&mut session, &viewer.drain_packets(), passenger)
        .expect("login did not create the flyer with its spline");
    assert!(
        position.x > 20.0,
        "login CREATE placed flyer at {}",
        position.x
    );
    assert!(spline.time_passed > 0);
    assert!((position.x - taxi_x(&shard, passenger)).abs() < 5.0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn flyer_reconnect_resumes_from_its_saved_position() {
    let (shard, passenger, _) = taxi_flight("taxi-flyer-reconnect", 40);
    let mut first = enter(&shard, passenger);
    first.drain_packets();
    drop(first);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || {
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {passenger}"
                ))
                .is_empty()
        }
    ));
    let saved_x: f32 = shard.query_rows(&format!(
        "SELECT x FROM game_character WHERE guid = {passenger}"
    ))[0]["x"]
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_millis(900));

    let mut second = enter(&shard, passenger);
    let world_x = taxi_x(&shard, passenger);
    let rows = second.drain_packets();
    let mut session = WorldSessionObserver::new(second.connection_id());
    assert!(taxi_create(&mut session, &rows, passenger).is_none());
    let packets = decoded_taxi_entry_packets(&rows);
    assert!(packets.iter().any(|packet| packet.opcode == 0x016e));
    assert!(packets.iter().any(|packet| packet.opcode == 0x00dd));
    assert!(
        (world_x - saved_x).abs() < 5.0,
        "saved {saved_x}, reconnected {world_x}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn permanent_taxi_abort_sends_dismount_result() {
    let (shard, passenger, _) = taxi_flight("taxi-abort-dismount", 40);
    let mut connection = enter(&shard, passenger);
    connection.drain_packets();
    shard.assert_sql("DELETE FROM game_taxi_path_node WHERE id = 5090994");
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || shard
            .query_rows(&format!(
            "SELECT character_guid FROM game_active_taxi_flight WHERE character_guid = {passenger}"
        ))
            .is_empty()
    ));
    let rows = connection.wait_for_packets(Duration::from_secs(5)).unwrap();
    assert!(
        rows.iter().any(|packet| {
            matches!(packet.audience, NativeAudience::Connection(_))
                && packet.opcode == Some(367)
                && packet.body == [3, 0, 0, 0]
        }),
        "permanent abort sent no successful dismount result: {rows:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn taxi_landing_sends_a_position_correction_after_dismount() {
    use wire_client::decode::Field;

    let (shard, passenger, _) = fixture("taxi-landing-correction");
    shard.assert_call("debug_spawn_player_entity", &[&passenger.to_string()]);
    let mut connection = enter(&shard, passenger);
    connection.drain_packets();
    start_taxi_flight(&shard, passenger, 32);

    let mut direct = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !direct.iter().any(|(opcode, _)| *opcode == 199) && std::time::Instant::now() < deadline {
        for row in connection.wait_for_packets(Duration::from_secs(1)).unwrap() {
            if let (NativeAudience::Connection(_), Some(opcode)) = (row.audience, row.opcode) {
                let decoded =
                    (opcode == 199).then(|| packet(&row, connection.connection_id(), opcode));
                direct.push((opcode, decoded));
            }
        }
    }

    let dismount = direct
        .iter()
        .position(|(opcode, _)| *opcode == 367)
        .unwrap();
    let correction = direct
        .iter()
        .position(|(opcode, _)| *opcode == 199)
        .unwrap();
    assert!(
        dismount < correction,
        "landing correction preceded dismount"
    );
    let fields = &direct[correction].1.as_ref().unwrap().fields;
    assert_eq!(fields.get("guid"), Some(&Field::Guid(passenger)));
    assert_eq!(fields.get("flags"), Some(&Field::Unsigned(0)));
    let Some(Field::Object(position)) = fields.get("position") else {
        panic!("landing correction has no position");
    };
    assert_eq!(position.get("x"), Some(&Field::Float(32.0)));
    assert_eq!(position.get("y"), Some(&Field::Float(0.0)));
    assert_eq!(position.get("z"), Some(&Field::Float(0.0)));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn taxi_crosses_capital_and_zone_boundaries_with_area_data() {
    use std::collections::BTreeMap;
    use wire_client::decode::Field;

    let (shard, passenger, _) = fixture("taxi-zone-boundaries");
    shard.insert_rows(
        "game_area",
        &[
            &[
                ("id", &1519),
                ("area_bit", &1),
                ("flags", &256),
                ("faction_group", &2),
                ("name", &"Stormwind"),
            ],
            &[
                ("id", &12),
                ("area_bit", &2),
                ("faction_group", &2),
                ("name", &"Elwynn Forest"),
            ],
            &[
                ("id", &40),
                ("area_bit", &3),
                ("faction_group", &2),
                ("name", &"Westfall"),
            ],
        ],
    );
    let y_cell = lyracore_shared::terrain::cell_index(0.0).unwrap();
    let mut areas = BTreeMap::new();
    for x in (0..=200).step_by(5) {
        let cell = lyracore_shared::terrain::cell_index(x as f32).unwrap();
        let area = if x < 50 {
            1519
        } else if x < 125 {
            12
        } else {
            40
        };
        areas.entry(cell).or_insert(area);
    }
    let heights = vec!["0"; 145].join(":");
    let chunks = areas
        .into_iter()
        .map(|(cell, area)| format!("0,{cell},{y_cell},0,0,0,{area},{heights}"))
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call("import_terrain_chunks", &[&chunks]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0, zone_id = 0 WHERE guid = {passenger}"
    ));

    let mut connection = enter(&shard, passenger);
    connection.drain_packets();
    let starting = shard.query_rows(&format!(
        "SELECT zone_id,resting,player_flags FROM game_world_entity WHERE guid = {passenger}"
    ));
    assert_eq!(starting[0]["zone_id"], "1519");
    assert_eq!(starting[0]["resting"], "true");
    assert_eq!(starting[0]["player_flags"], "32");

    start_taxi_flight(&shard, passenger, 200);
    let mut zones = Vec::new();
    let mut weather = 0;
    let mut rest_flags = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while std::time::Instant::now() < deadline
        && (zones.len() < 2 || weather < 2 || rest_flags.is_empty())
    {
        for row in connection.wait_for_packets(Duration::from_secs(1)).unwrap() {
            let Some(opcode) = row.opcode else {
                continue;
            };
            let connection_bound = matches!(row.audience, NativeAudience::Connection(_));
            if connection_bound && opcode == 706 {
                let decoded = packet(&row, connection.connection_id(), opcode);
                if let Some(Field::Unsigned(zone)) = decoded.fields.get("zone") {
                    zones.push(*zone);
                }
            } else if connection_bound && opcode == 756 {
                weather += 1;
            } else if opcode == 169 {
                let decoded = wire_client::decode::decode(opcode, &row.body).unwrap();
                if let Some(Field::List(objects)) = decoded.fields.get("objects") {
                    for object in objects {
                        if let Field::Object(object) = object {
                            if let Some(Field::Object(values)) = object.get("values") {
                                if let Some(Field::Unsigned(flags)) = values.get("field_190") {
                                    rest_flags.push(*flags);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    assert_eq!(zones, [12, 40]);
    assert_eq!(weather, 2);
    assert_eq!(rest_flags, [0]);
    let ending = shard.query_rows(&format!(
        "SELECT zone_id,resting,player_flags FROM game_world_entity WHERE guid = {passenger}"
    ));
    assert_eq!(ending[0]["zone_id"], "40");
    assert_eq!(ending[0]["resting"], "false");
    assert_eq!(ending[0]["player_flags"], "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn rest_area_rejects_hostile_characters_in_an_enemy_capital() {
    let (shard, alliance, account) = fixture("capital-rest-teams");
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Horde\"",
            "2",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let horde: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Horde'")[0]
        ["guid"]
        .parse()
        .unwrap();
    shard.insert_row(
        "game_area",
        &[
            ("id", &1519),
            ("area_bit", &1),
            ("flags", &256),
            ("faction_group", &2),
            ("name", &"Stormwind"),
        ],
    );
    let cell = lyracore_shared::terrain::cell_index(0.0).unwrap();
    let heights = vec!["0"; 145].join(":");
    shard.assert_call(
        "import_terrain_chunks",
        &[&format!("0,{cell},{cell},0,0,0,1519,{heights}")],
    );

    let rest = |guid| {
        shard.query_rows(&format!(
            "SELECT resting,player_flags,unit_flags FROM game_world_entity WHERE guid = {guid}"
        ))[0]
            .clone()
    };
    let unit_flags = |guid| rest(guid)["unit_flags"].parse::<u32>().unwrap();
    let mut spawned_flags = Vec::new();
    for guid in [alliance, horde] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
        spawned_flags.push(unit_flags(guid));
        shard.assert_call("debug_check_rest_at", &[&guid.to_string(), "0", "0", "0"]);
    }
    assert_eq!(rest(alliance)["resting"], "true");
    assert_eq!(rest(alliance)["player_flags"], "32");
    assert_eq!(unit_flags(alliance), spawned_flags[0]);
    assert_eq!(rest(horde)["resting"], "false");
    assert_eq!(rest(horde)["player_flags"], "0");
    // vmangos Player::UpdateZone flags a Character in the enemy team's capital.
    assert_eq!(
        unit_flags(horde),
        spawned_flags[1] | lyracore_shared::constants::unit_flags::PVP
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn rest_area_rejects_a_flyer_above_the_goldshire_inn() {
    use std::collections::BTreeSet;

    let (shard, passenger, _) = fixture("inn-overflight");
    shard.insert_row(
        "game_area_trigger",
        &[
            ("id", &5090902),
            ("x", &-9464),
            ("y", &42),
            ("z", &56),
            ("radius", &20),
        ],
    );
    shard.insert_row("game_tavern_area_trigger", &[("trigger_id", &5090902)]);
    shard.insert_row(
        "game_area",
        &[
            ("id", &12),
            ("area_bit", &1),
            ("faction_group", &2),
            ("name", &"Elwynn Forest"),
        ],
    );
    let y_cell = lyracore_shared::terrain::cell_index(42.0).unwrap();
    let heights = vec!["0"; 145].join(":");
    let chunks = (-9500..=-9440)
        .step_by(5)
        .map(|x| lyracore_shared::terrain::cell_index(x as f32).unwrap())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|x_cell| format!("0,{x_cell},{y_cell},0,0,0,12,{heights}"))
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call("import_terrain_chunks", &[&chunks]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET map_id = 0, x = -9500, y = 42, z = 1000 WHERE guid = {passenger}"
    ));
    let _connection = enter(&shard, passenger);
    let route = std::iter::once((-9500.0, 42.0, 1000.0))
        .chain((0..20).map(|index| {
            if index % 2 == 0 {
                (-9470.0, 42.0, 1000.0)
            } else {
                (-9458.0, 42.0, 1000.0)
            }
        }))
        .collect::<Vec<_>>();
    start_taxi_flight_on_route(&shard, passenger, &route);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || {
            let row = &shard.query_rows(&format!(
                "SELECT x,z FROM game_world_entity WHERE guid = {passenger}"
            ))[0];
            (row["x"].parse::<f32>().unwrap() + 9464.0).abs() < 10.0
        }
    ));
    let flyer = &shard.query_rows(&format!(
        "SELECT z,resting,player_flags FROM game_world_entity WHERE guid = {passenger}"
    ))[0];
    assert!((flyer["z"].parse::<f32>().unwrap() - 1000.0).abs() < 0.01);
    assert_eq!(flyer["resting"], "false");
    assert_eq!(flyer["player_flags"], "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn tavern_rest_uses_imported_trigger_geometry() {
    let (shard, character, _) = fixture("tavern-rest");
    shard.insert_row(
        "game_area_trigger",
        &[
            ("id", &5090902),
            ("x", &-9464),
            ("y", &42),
            ("z", &56),
            ("box_length", &8),
            ("box_width", &6),
            ("box_height", &4),
        ],
    );
    shard.insert_row("game_tavern_area_trigger", &[("trigger_id", &5090902)]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET map_id = 0, x = -9459, y = 42, z = 56 WHERE guid = {character}"
    ));
    let _connection = enter(&shard, character);
    shard.assert_call(
        "debug_check_rest_at",
        &[&character.to_string(), "0", "-9464", "42"],
    );
    let rest = || {
        shard.query_rows(&format!(
            "SELECT resting,player_bytes_2 FROM game_world_entity WHERE guid = {character}"
        ))[0]
            .clone()
    };
    let state = rest();
    assert_eq!(state["resting"], "true");
    assert_eq!(state["player_bytes_2"].parse::<u32>().unwrap() >> 24, 2);
    shard.assert_call(
        "debug_check_rest_at",
        &[&character.to_string(), "0", "-9459", "42"],
    );
    let state = rest();
    assert_eq!(state["resting"], "false");
    assert_eq!(state["player_bytes_2"].parse::<u32>().unwrap() >> 24, 2);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn world_entry_resets_rest_state_before_bonus_threshold() {
    let (shard, character, _) = fixture("rest-state-login");
    shard.assert_sql(&format!(
        "UPDATE game_character SET rested_xp = 5, rest_state = 1 WHERE guid = {character}"
    ));
    let _connection = enter(&shard, character);
    let entity = &shard.query_rows(&format!(
        "SELECT player_bytes_2 FROM game_world_entity WHERE guid = {character}"
    ))[0];
    assert_eq!(entity["player_bytes_2"].parse::<u32>().unwrap() >> 24, 2);
    let saved = &shard.query_rows(&format!(
        "SELECT rest_state FROM game_character WHERE guid = {character}"
    ))[0];
    assert_eq!(saved["rest_state"], "2");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn neutral_capital_does_not_rest_on_ffa_pvp_realm() {
    let (shard, character, _) = fixture("ffa-capital-rest");
    shard.assert_sql("UPDATE game_realm SET realm_type = 16 WHERE id = 1");
    shard.insert_row(
        "game_area",
        &[
            ("id", &5090901),
            ("area_bit", &1),
            ("flags", &256),
            ("name", &"Neutral capital"),
        ],
    );
    let cell = lyracore_shared::terrain::cell_index(0.0).unwrap();
    let heights = vec!["0"; 145].join(":");
    shard.assert_call(
        "import_terrain_chunks",
        &[&format!("0,{cell},{cell},0,0,0,5090901,{heights}")],
    );
    let _connection = enter(&shard, character);
    shard.assert_call(
        "debug_check_rest_at",
        &[&character.to_string(), "0", "0", "0"],
    );
    let row = &shard.query_rows(&format!(
        "SELECT resting,player_flags FROM game_world_entity WHERE guid = {character}"
    ))[0];
    assert_eq!(row["resting"], "false");
    assert_eq!(row["player_flags"], "0");
}

fn start_taxi_flight(shard: &Standalone, passenger: u64, destination_x: u32) {
    start_taxi_flight_on_route(
        shard,
        passenger,
        &[(0.0, 0.0, 0.0), (destination_x as f32, 0.0, 0.0)],
    );
}

fn start_taxi_flight_on_route(shard: &Standalone, passenger: u64, route: &[(f32, f32, f32)]) {
    let &(start_x, start_y, start_z) = route.first().expect("taxi route has a start");
    let &(end_x, end_y, end_z) = route.last().expect("taxi route has an end");
    shard.insert_rows(
        "game_taxi_node",
        &[
            &[
                ("id", &5090990),
                ("client_node_id", &100),
                ("x", &start_x),
                ("y", &start_y),
                ("z", &start_z),
                ("name", &"Start"),
                ("mount_display_alliance", &1147),
            ],
            &[
                ("id", &5090991),
                ("client_node_id", &101),
                ("x", &end_x),
                ("y", &end_y),
                ("z", &end_z),
                ("name", &"End"),
                ("mount_display_alliance", &1147),
            ],
        ],
    );
    shard.insert_row(
        "game_taxi_path",
        &[
            ("id", &5090992),
            ("source_node_id", &5090990),
            ("destination_node_id", &5090991),
        ],
    );
    let ids: Vec<_> = (0..route.len())
        .map(|index| (5090993 + index, index))
        .collect();
    let mut nodes: Vec<[Field; 6]> = Vec::new();
    for ((id, index), (x, y, z)) in ids.iter().zip(route) {
        nodes.push([
            ("id", id),
            ("path_id", &5090992),
            ("node_index", index),
            ("x", x),
            ("y", y),
            ("z", z),
        ]);
    }
    let nodes: Vec<&[Field]> = nodes.iter().map(|node| &node[..]).collect();
    shard.insert_rows("game_taxi_path_node", &nodes);
    shard.insert_row(
        "game_active_taxi_flight",
        &[
            ("character_guid", &passenger),
            ("path_id", &5090992),
            ("source_node_id", &5090990),
            ("destination_node_id", &5090991),
            ("mount_display_id", &1147),
        ],
    );
    shard.assert_call(
        "gw_arm_taxi_flight",
        &[&format!("{{\"guid\":{passenger},\"ownership\":null}}")],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reconnecting_passenger_applies_class_specific_starting_passives() {
    use lyracore_shared::spell::taxonomy::{A_MOD_STAT_PCT, SPELL_ATTR_PASSIVE, STAT_SPI};

    let (shard, passenger, _) = fixture("taxi-reconnect-passives");
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {passenger}"
    ));
    shard.assert_sql("DELETE FROM game_level_stats WHERE race = 1 AND class = 1 AND level = 1");
    shard.insert_row(
        "game_level_stats",
        &[
            ("race_class_level", &65793),
            ("race", &1),
            ("class", &1),
            ("level", &1),
            ("strength", &23),
            ("agility", &20),
            ("stamina", &22),
            ("intellect", &20),
            ("spirit", &20),
        ],
    );
    shard.assert_sql("DELETE FROM game_createinfo_spell WHERE spell_id = 20598");
    shard.insert_row(
        "game_createinfo_spell",
        &[
            ("id", &5090908),
            ("race", &1),
            ("class", &1),
            ("spell_id", &20598),
        ],
    );
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &20598),
            ("name", &"Spirit passive fixture"),
            ("school_mask", &1),
            ("max_stacks", &1),
            ("attributes", &SPELL_ATTR_PASSIVE),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &82392),
            ("spell_id", &20598),
            ("kind", &A_MOD_STAT_PCT),
            ("base_points", &5),
            ("p_0", &STAT_SPI),
        ],
    );
    let spirit = || {
        shard.query_rows(&format!(
            "SELECT spirit FROM game_world_entity WHERE guid = {passenger}"
        ))[0]["spirit"]
            .clone()
    };
    let first = enter(&shard, passenger);
    assert_eq!(
        spirit(),
        "21",
        "starting passives must apply before login CREATE"
    );
    start_taxi_flight_on_route(
        &shard,
        passenger,
        &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (2000.0, 0.0, 0.0)],
    );
    drop(first);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || shard
            .query_rows("SELECT * FROM game_native_player")
            .is_empty()
    ));
    let _second = enter(&shard, passenger);
    assert_eq!(
        spirit(),
        "21",
        "reconnect must retain the five-percent Spirit bonus"
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_aura WHERE target_guid = {passenger} AND spell_id = 20598"
            ))
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reconnecting_passenger_keeps_both_taxi_flags() {
    use lyracore_shared::constants::unit_flags;

    let (shard, passenger, _) = fixture("taxi-reconnect");
    let first = enter(&shard, passenger);
    start_taxi_flight_on_route(
        &shard,
        passenger,
        &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (2000.0, 0.0, 0.0)],
    );
    let flags = |shard: &Standalone| -> u32 {
        shard.query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {passenger}"
        ))[0]["unit_flags"]
            .parse()
            .unwrap()
    };
    let taxi_flags = unit_flags::TAXI_FLIGHT | unit_flags::REMOVE_CLIENT_CONTROL;
    assert_eq!(flags(&shard) & taxi_flags, taxi_flags);
    drop(first);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || shard
            .query_rows("SELECT * FROM game_native_player")
            .is_empty()
    ));
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT * FROM game_active_taxi_flight WHERE character_guid = {passenger}"
            ))
            .len(),
        1
    );
    let _second = enter(&shard, passenger);
    assert_eq!(flags(&shard) & taxi_flags, taxi_flags);
}

fn decoded_taxi_entry_packets(packets: &[NativePacket]) -> Vec<wire_client::decode::Packet> {
    packets
        .iter()
        .filter_map(|packet| {
            let opcode = packet.opcode?;
            if ![0x00a9, 0x00dd, 0x0143, 0x0144, 0x016e].contains(&opcode) {
                return None;
            }
            Some(wire_client::decode::decode(opcode, &packet.body).unwrap())
        })
        .collect()
}

fn assert_taxi_resumes_after_world_entry(packets: &[wire_client::decode::Packet], passenger: u64) {
    use lyracore_shared::constants::unit_flags;
    use wire_client::decode::Field;

    let flags: Vec<_> = packets
        .iter()
        .flat_map(|packet| match packet.fields.get("objects") {
            Some(Field::List(objects)) => objects.iter(),
            _ => [].iter(),
        })
        .filter_map(|object| {
            let Field::Object(object) = object else {
                return None;
            };
            if object.get("guid") != Some(&Field::Guid(passenger)) {
                return None;
            }
            let Some(Field::Object(values)) = object.get("values") else {
                return None;
            };
            let Some(Field::Unsigned(flags)) = values.get("field_46") else {
                return None;
            };
            Some((object.get("kind").cloned().unwrap(), *flags as u32))
        })
        .collect();
    let controlled = unit_flags::PLAYER_CONTROLLED;
    let flying = controlled | unit_flags::TAXI_FLIGHT | unit_flags::REMOVE_CLIENT_CONTROL;
    assert_eq!(
        flags,
        vec![
            (Field::Unsigned(2), controlled | unit_flags::IN_COMBAT),
            (Field::Unsigned(0), flying | unit_flags::IN_COMBAT)
        ]
    );
    let sequence: Vec<_> = packets
        .iter()
        .filter(|packet| [0x0143, 0x0144, 0x016e, 0x00dd].contains(&packet.opcode))
        .map(|packet| packet.opcode)
        .collect();
    assert_eq!(sequence, vec![0x0144, 0x016e, 0x00dd]);
    let resumed_spline = packets
        .iter()
        .find(|packet| packet.opcode == 0x00dd)
        .unwrap();
    let Some(Field::List(points)) = resumed_spline.fields.get("points") else {
        panic!("resumed spline has no point list");
    };
    assert_eq!(points.len(), 1, "the selected resume node is replaced");
    assert!(packets.iter().any(|packet| packet.opcode == 0x0144
        && packet.fields.get("victim") == Some(&Field::Guid(passenger))));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reconnecting_passenger_enters_the_world_before_resuming_taxi() {
    use lyracore_shared::constants::unit_flags;

    let (shard, passenger, _) = fixture("taxi-entry-order");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("DELETE FROM game_faction_template WHERE id = 1 OR id = 14");
    shard.insert_rows(
        "game_faction_template",
        &[
            &[
                ("id", &1),
                ("faction", &1),
                ("faction_group", &3),
                ("friend_group", &2),
                ("enemy_group", &12),
            ],
            &[
                ("id", &14),
                ("faction", &14),
                ("faction_group", &8),
                ("enemy_group", &1),
            ],
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {passenger}"
    ));
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let first = enter(&shard, passenger);
    start_taxi_flight_on_route(
        &shard,
        passenger,
        &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (2000.0, 0.0, 0.0)],
    );
    for _ in 0..20 {
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    }
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&passenger.to_string(), &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    drop(first);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || shard
            .query_rows("SELECT * FROM game_native_player")
            .is_empty()
    ));
    let mut second = enter(&shard, passenger);
    let packets = decoded_taxi_entry_packets(&second.drain_packets());
    assert_taxi_resumes_after_world_entry(&packets, passenger);
    for _ in 0..24 {
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    }
    let entity = &shard.query_rows(&format!(
        "SELECT unit_flags FROM game_world_entity WHERE guid = {passenger}"
    ))[0];
    let flying =
        unit_flags::PLAYER_CONTROLLED | unit_flags::TAXI_FLIGHT | unit_flags::REMOVE_CLIENT_CONTROL;
    assert_eq!(entity["unit_flags"].parse::<u32>().unwrap(), flying);
    assert!(shard
        .query_rows(&format!(
            "SELECT attacker_guid FROM game_melee_attack WHERE target_guid = {passenger}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reconnect_on_the_last_taxi_segment_lands_on_the_next_tick() {
    let (shard, passenger, _) = fixture("taxi-last-segment-reconnect");
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {passenger}"
    ));
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let first = enter(&shard, passenger);
    start_taxi_flight_on_route(
        &shard,
        passenger,
        &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (2000.0, 0.0, 0.0)],
    );
    drop(first);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(5),
        || shard
            .query_rows("SELECT * FROM game_native_player")
            .is_empty()
    ));
    shard.assert_sql(&format!(
        "UPDATE game_character SET x = 1500, orientation = 0.5 WHERE guid = {passenger}"
    ));
    let mut second = enter(&shard, passenger);
    let packets = decoded_taxi_entry_packets(&second.drain_packets());
    assert!(packets.iter().any(|packet| packet.opcode == 0x016e));
    assert!(!packets.iter().any(|packet| packet.opcode == 0x00dd));
    let movement = &shard.query_rows(&format!(
        "SELECT movement_flags FROM game_world_entity WHERE guid = {passenger}"
    ))[0];
    assert_eq!(movement["movement_flags"], "0");
    for _ in 0..2 {
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    }
    let entity = &shard.query_rows(&format!(
        "SELECT x, orientation, mount_display_id FROM game_world_entity WHERE guid = {passenger}"
    ))[0];
    assert_eq!(entity["x"].parse::<f32>().unwrap(), 2000.0);
    assert_eq!(entity["orientation"].parse::<f32>().unwrap(), 0.5);
    assert_eq!(entity["mount_display_id"], "0");
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_active_taxi_flight WHERE character_guid = {passenger}"
        ))
        .is_empty());
}
