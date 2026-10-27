use std::io::Cursor;
use std::time::Duration;

use lyracore_test_support::{ReducerConnection, Standalone};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

#[path = "conformance/taxi.rs"]
mod taxi;
#[path = "conformance/taxi_route.rs"]
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
    assert!(connection
        .wait_for_packets(Duration::from_millis(50))
        .unwrap()
        .is_empty());
    shard.assert_call("debug_set_money", &[&guid.to_string(), "12345"]);
    let rows = connection.wait_for_packets(Duration::from_secs(5)).unwrap();
    assert!(
        !rows.is_empty(),
        "the later transaction must reach the subscription"
    );
    assert!(
        rows.iter().any(|row| {
            let Packet(_, (audience, _), (option, opcode), _, _, _, _) =
                serde_json::from_str(row.get()).unwrap();
            audience == 0 && option == 0 && opcode.get() == "169"
        }),
        "expected SMSG_UPDATE_OBJECT from the money change"
    );
    assert!(connection.drain_packets().is_empty());
}
