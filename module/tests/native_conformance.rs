use std::io::Cursor;

use lyracore_test_support::Standalone;
use serde_json::{json, Value};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

// Tier 1 replaces this decoder with the harness library; see docs/adr/0005-1-12-conformance.md.
fn decode_packet(opcode: u16, body: &[u8]) -> ServerOpcodeMessage {
    let mut frame = Vec::new();
    frame.extend_from_slice(&u16::try_from(body.len() + 2).unwrap().to_be_bytes());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(body);
    let mut reader = Cursor::new(frame);
    let message = ServerOpcodeMessage::read_unencrypted(&mut reader).unwrap();
    assert_eq!(reader.position() as usize, reader.get_ref().len());
    message
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn operator_connection_enters_world_and_receives_pong() {
    let mut shard = Standalone::start("native-conformance");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"CONFORMANCE\"", &credentials, &credentials],
    );
    let account =
        &shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]["id"];
    shard.assert_call(
        "create_character",
        &[
            account,
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

    let mut anonymous = shard.connect_reducers_anonymous(&[]);
    let refused = anonymous
        .call("debug_native_enter_world", json!([guid]))
        .unwrap_err();
    assert!(refused.contains("operator"), "{refused}");
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([guid]))
        .unwrap();
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
    let row = &rows[0];
    assert_eq!(
        row[1],
        json!([0, [connection.connection_id()["__connection_id__"]]])
    );
    assert_eq!(row[2], json!([0, 0x1dd]));
    let encoded = row[3].as_str().expect("packet bytes are hexadecimal");
    assert_eq!(encoded.len() % 2, 0);
    let body: Vec<u8> = (0..encoded.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&encoded[offset..offset + 2], 16).unwrap())
        .collect();
    let ServerOpcodeMessage::SMSG_PONG(pong) = decode_packet(0x1dd, &body) else {
        panic!("expected SMSG_PONG");
    };
    assert_eq!(pong.sequence_id, sequence);
    assert_eq!(row[4], Value::Bool(false));
    assert!(shard
        .query_rows("SELECT * FROM game_native_packet")
        .is_empty());
}
