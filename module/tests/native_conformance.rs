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
    assert!(query().inserts("game_native_packet").is_empty());
    std::thread::sleep(Duration::from_secs(21));
    let response = query();
    assert!(response.inserts("game_native_packet").iter().any(|row| {
        let Packet(_, _, (_, opcode), _, _, _, _) = serde_json::from_str(row.get()).unwrap();
        opcode.get() == "463"
    }));
    let has_heartbeat = |row: &RawValue| {
        let Packet(_, _, (_, opcode), _, _, _, _) = serde_json::from_str(row.get()).unwrap();
        opcode.get() == "238"
    };
    assert!(
        response
            .inserts("game_native_packet")
            .iter()
            .any(|row| has_heartbeat(row))
            || connection
                .drain_packets()
                .iter()
                .any(|row| has_heartbeat(row))
    );
    assert_eq!(
        shard.query_rows("SELECT teleport FROM game_native_player")[0]["teleport"],
        "(none = ())"
    );
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
    taxi_flight_on_route(name, observer_x, 0, &[(0.0, 0.0, 0.0), (200.0, 0.0, 0.0)])
}

fn taxi_flight_on_route(
    name: &str,
    observer_x: i32,
    observer_y: i32,
    route: &[(f32, f32, f32)],
) -> (Standalone, u64, u64) {
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
        "UPDATE game_character SET x = {observer_x}, y = {observer_y} WHERE guid = {observer}"
    ));
    shard.assert_call("debug_spawn_player_entity", &[&passenger.to_string()]);
    start_taxi_flight_on_route(&shard, passenger, route);
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
    viewer.drain_packets();
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
            &viewer.wait_for_packets(Duration::from_secs(1)).unwrap(),
            viewer.connection_id(),
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

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
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
            let Packet(_, (audience, _), (option, opcode), _, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            if audience == 0 && option == 0 {
                let opcode = serde_json::from_str::<u16>(opcode.get()).unwrap();
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn taxi_crosses_capital_and_zone_boundaries_with_area_data() {
    use std::collections::BTreeMap;
    use wire_client::decode::Field;

    let (shard, passenger, _) = fixture("taxi-zone-boundaries");
    shard.assert_sql("INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES (1519,0,0,1,256,0,2,'Stormwind'),(12,0,0,2,0,0,2,'Elwynn Forest'),(40,0,0,3,0,0,2,'Westfall')");
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
            let Packet(_, (audience, _), (option, opcode), body, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            if option != 0 {
                continue;
            }
            let opcode = serde_json::from_str::<u16>(opcode.get()).unwrap();
            if audience == 0 && opcode == 706 {
                let decoded = packet(&row, connection.connection_id(), opcode);
                if let Some(Field::Unsigned(zone)) = decoded.fields.get("zone") {
                    zones.push(*zone);
                }
            } else if audience == 0 && opcode == 756 {
                weather += 1;
            } else if opcode == 169 {
                let bytes = body
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect::<Vec<_>>();
                let decoded = wire_client::decode::decode(opcode, &bytes).unwrap();
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
    shard.assert_sql(&format!("INSERT INTO game_taxi_node (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) VALUES (5090990,100,0,{start_x},{start_y},{start_z},'Start',0,1147),(5090991,101,0,{end_x},{end_y},{end_z},'End',0,1147)"));
    shard.assert_sql("INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (5090992,5090990,5090991,0)");
    let values = route
        .iter()
        .enumerate()
        .map(|(index, &(x, y, z))| {
            format!("({},5090992,{index},0,{x},{y},{z},0,0)", 5090993 + index)
        })
        .collect::<Vec<_>>()
        .join(",");
    shard.assert_sql(&format!("INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) VALUES {values}"));
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
