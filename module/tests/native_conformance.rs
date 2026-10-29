use std::io::Cursor;
use std::time::Duration;

use lyracore_test_support::{ReducerConnection, Standalone};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

#[path = "conformance/taxi.rs"]
mod taxi;
#[path = "../../conformance/support/taxi_route.rs"]
mod taxi_route;
#[path = "conformance/tier1.rs"]
mod tier1;
#[path = "conformance/transport.rs"]
mod transport;

fn fixture(name: &str) -> (Standalone, u64, String) {
    let mut shard = Standalone::start(name);
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

#[derive(Deserialize)]
struct Packet(
    u64,
    (u8, Box<RawValue>),
    (u8, Box<RawValue>),
    String,
    bool,
    i64,
    bool,
);

fn packet(row: &RawValue, connection_id: u128, opcode: u16) -> wire_client::decode::Packet {
    let Packet(
        id,
        (audience, recipient),
        (option, encoded_opcode),
        encoded,
        close,
        not_before,
        authenticated,
    ) = serde_json::from_str(row.get()).unwrap();
    assert_ne!(id, 0);
    assert_eq!(audience, 0);
    assert_eq!(
        serde_json::from_str::<(u128,)>(recipient.get()).unwrap().0,
        connection_id
    );
    assert_eq!(option, 0);
    assert_eq!(
        serde_json::from_str::<u16>(encoded_opcode.get()).unwrap(),
        opcode
    );
    assert_eq!(encoded.len() % 2, 0);
    let body: Vec<u8> = (0..encoded.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&encoded[offset..offset + 2], 16).unwrap())
        .collect();
    assert!(!close);
    assert_eq!(not_before, 0);
    assert!(!authenticated);
    wire_client::decode::decode(opcode, &body).unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
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
    let rows = result.inserts("game_native_packet");
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
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
    let Packet(_, _, (option, opcode), body, close, _, _) =
        serde_json::from_str(rows.inserts("game_native_packet")[0].get()).unwrap();
    let bytes: Vec<u8> = (0..body.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&body[offset..offset + 2], 16).unwrap())
        .collect();
    assert_eq!(option, 0);
    assert_eq!(opcode.get(), "494");
    // SMSG_AUTH_RESPONSE carries AUTH_FAILED as the single result byte 13.
    assert_eq!(bytes, [13]);
    assert!(close);
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_later_transaction_delivers_packets_without_another_client_request() {
    let (shard, guid, _) = fixture("native-conformance-later");
    let mut connection = enter(&shard, guid);
    assert!(!connection.drain_packets().is_empty());
    shard.assert_call("debug_set_money", &[&guid.to_string(), "12345"]);
    let expected = lyracore_protocol::world::encode(
        lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(lyracore_protocol::codec::build_coinage_values(guid, 12345)),
        ),
    )
    .unwrap();
    let expected_body = expected
        .body
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut rows = Vec::new();
    while std::time::Instant::now() < deadline {
        rows.extend(connection.wait_for_packets(Duration::from_secs(1)).unwrap());
        if rows.iter().any(|row| {
            let Packet(_, (audience, _), (option, opcode), body, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            audience == 0 && option == 0 && opcode.get() == "169" && body == expected_body
        }) {
            return;
        }
    }
    panic!("expected coinage values from the later transaction: {rows:?}");
}

fn taxi_flight(name: &str, observer_x: i32) -> (Standalone, u64, u64) {
    let (shard, passenger, _) = fixture(name);
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
        "UPDATE game_character SET x = {observer_x} WHERE guid = {observer}"
    ));
    shard.assert_call("debug_spawn_player_entity", &[&passenger.to_string()]);
    shard.assert_sql("INSERT INTO game_taxi_node (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) VALUES (5090990,100,0,0,0,0,'Start',0,1147),(5090991,101,0,200,0,0,'End',0,1147)");
    shard.assert_sql("INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (5090992,5090990,5090991,0)");
    shard.assert_sql("INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) VALUES (5090993,5090992,0,0,0,0,0,0,0),(5090994,5090992,1,0,200,0,0,0,0)");
    shard.assert_sql(&format!(
        "INSERT INTO game_active_taxi_flight (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,current_node_index,started_micros,origin_x,origin_y,origin_z,origin_recorded) VALUES ({passenger},5090992,5090990,5090991,1147,0,0,0,0,0,0,false)"
    ));
    shard.assert_call(
        "gw_arm_taxi_flight",
        &[&format!("{{\"guid\":{passenger},\"ownership\":null}}")],
    );
    (shard, passenger, observer)
}

fn taxi_create(
    rows: &[Box<RawValue>],
    connection_id: u128,
    passenger: u64,
) -> Option<(
    wow_world_messages::vanilla::MovementBlock_MovementFlags_SplineEnabled,
    wow_world_messages::vanilla::Vector3d,
)> {
    use wow_world_messages::vanilla::{
        opcodes::ServerOpcodeMessage, MovementBlock_UpdateFlag_Living, Object,
    };
    rows.iter().find_map(|row| {
        let Packet(_, (audience, recipient), (option, opcode), body, _, _, _) =
            serde_json::from_str(row.get()).unwrap();
        if option != 0 || opcode.get() != "169" {
            return None;
        }
        let addressed = match audience {
            0 => serde_json::from_str::<(u128,)>(recipient.get()).unwrap().0 == connection_id,
            2 => serde_json::from_str::<((u128,), u8, u64)>(recipient.get())
                .is_ok_and(|((id,), _, guid)| id == connection_id && guid == passenger),
            _ => false,
        };
        if !addressed {
            return None;
        }
        let bytes: Vec<u8> = body
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
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
            let Object::CreateObject2 {
                guid3, movement2, ..
            } = object
            else {
                return None;
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

fn taxi_x(shard: &Standalone, guid: u64) -> f32 {
    shard.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {guid}"
    ))[0]["x"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_new_viewer_receives_the_current_taxi_spline() {
    let (shard, passenger, observer) = taxi_flight("taxi-new-viewer", 200);
    let mut viewer = enter(&shard, observer);
    viewer.drain_packets();

    let mut spline = None;
    let mut observed = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while spline.is_none() && std::time::Instant::now() < deadline {
        let rows = viewer.wait_for_packets(Duration::from_secs(1)).unwrap();
        spline = taxi_create(&rows, viewer.connection_id(), passenger);
        observed.extend(rows);
    }
    let (spline, position) = spline
        .unwrap_or_else(|| panic!("observer did not receive a taxi CREATE spline: {observed:?}"));
    let x = position.x;
    assert!((66.0..200.0).contains(&x), "obsolete spline start: {x}");
    assert_eq!(spline.nodes.len(), 4);
    assert_eq!((spline.nodes[1].x, spline.nodes[2].x), (0.0, 200.0));
    assert_eq!(spline.final_node.x, 200.0);
    assert!(spline.time_passed > 0);
    assert!(!observed.iter().any(|row| {
        let Packet(_, (audience, _), (option, opcode), _, _, _, _) =
            serde_json::from_str(row.get()).unwrap();
        audience == 0 && option == 0 && opcode.get() == "221"
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn login_near_a_flyer_replays_its_current_spline() {
    let (shard, passenger, observer) = taxi_flight("taxi-login-near-flyer", 40);
    assert!(lyracore_test_support::poll_until(
        Duration::from_secs(4),
        || { taxi_x(&shard, passenger) > 20.0 }
    ));

    let mut viewer = enter(&shard, observer);
    let (spline, position) =
        taxi_create(&viewer.drain_packets(), viewer.connection_id(), passenger)
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn flyer_reconnect_restores_position_and_replays_current_spline() {
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
    let (spline, position) =
        taxi_create(&second.drain_packets(), second.connection_id(), passenger)
            .expect("reconnect did not create the flyer with its spline");
    let replay_x = position.x;
    assert!(spline.time_passed > 0);
    assert!(
        world_x > saved_x + 15.0,
        "saved {saved_x}, reconnected {world_x}"
    );
    assert!(
        (replay_x - world_x).abs() < 5.0,
        "replayed {replay_x}, world {world_x}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
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
        rows.iter().any(|row| {
            let Packet(_, (audience, _), (option, opcode), body, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            audience == 0 && option == 0 && opcode.get() == "367" && body == "03000000"
        }),
        "permanent abort sent no successful dismount result: {rows:?}"
    );
}
fn start_taxi_flight(shard: &Standalone, passenger: u64, destination_x: u32) {
    shard.assert_sql(&format!("INSERT INTO game_taxi_node (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) VALUES (5090990,100,0,0,0,0,'Start',0,1147),(5090991,101,0,{destination_x},0,0,'End',0,1147)"));
    shard.assert_sql("INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (5090992,5090990,5090991,0)");
    shard.assert_sql(&format!("INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) VALUES (5090993,5090992,0,0,0,0,0,0,0),(5090994,5090992,1,0,{destination_x},0,0,0,0)"));
    shard.assert_sql(&format!(
        "INSERT INTO game_active_taxi_flight (character_guid,path_id,source_node_id,destination_node_id,mount_display_id,fare,current_node_index,started_micros,origin_x,origin_y,origin_z,origin_recorded) VALUES ({passenger},5090992,5090990,5090991,1147,0,0,0,0,0,0,false)"
    ));
    shard.assert_call(
        "gw_arm_taxi_flight",
        &[&format!("{{\"guid\":{passenger},\"ownership\":null}}")],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_reconnecting_passenger_keeps_both_taxi_flags() {
    use lyracore_shared::constants::unit_flags;

    let (shard, passenger, _) = fixture("taxi-reconnect");
    let first = enter(&shard, passenger);
    start_taxi_flight(&shard, passenger, 2000);
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
