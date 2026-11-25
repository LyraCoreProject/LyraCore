mod support;

use std::io::Cursor;
use std::time::Duration;

use serde_json::json;
use support::{module_bytes, poll_until, ReducerConnection, Standalone};
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

// SHA-1 for CONFORMANCE, client seed 0, server seed 42 and key [0x5a; 40].
const PROOF: [u8; 20] = [
    0, 219, 190, 39, 108, 63, 12, 6, 223, 186, 9, 97, 43, 173, 114, 133, 133, 42, 164, 178,
];
const CMSG_AUTH_SESSION_OPCODE: u32 = 0x1ed;
const AWAY_CONNECTION: u128 = 5;

fn database_identity(shard: &Standalone, database: &str) -> String {
    let decimal = &shard.query_database_rows(database, "SELECT database_identity FROM st_module")
        [0]["database_identity"];
    let mut limbs = [0u64; 4];
    for digit in decimal.bytes() {
        let mut carry = u128::from(digit - b'0');
        for limb in limbs.iter_mut().rev() {
            let wide = u128::from(*limb) * 10 + carry;
            *limb = wide as u64;
            carry = wide >> 64;
        }
    }
    let hex: String = limbs.iter().map(|limb| format!("{limb:016x}")).collect();
    format!(r#"{{"__identity__":"0x{hex}"}}"#)
}

/// A Realm Shard and a World Shard that does not yet list the Realm Shard, so it refuses every
/// call from it.
struct Split {
    shard: Standalone,
    world: String,
    realm_identity: String,
    world_identity: String,
}

fn split(name: &str) -> Split {
    let mut shard = Standalone::start_persistent(name);
    shard.publish_module();
    let world = format!("{}-world", shard.shard_name());
    shard.publish_named_module_bytes(&world, module_bytes());
    shard.assert_call("claim_operator", &[]);
    shard.assert_call_database(&world, "claim_operator", &[]);
    let realm_identity = database_identity(&shard, shard.shard_name());
    let world_identity = database_identity(&shard, &world);
    shard.assert_call("operator_add_peer_shard", &[&world_identity, "\"world\""]);
    shard.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"realm":{world_identity}}}}}"#)],
    );
    Split {
        shard,
        world,
        realm_identity,
        world_identity,
    }
}

impl Split {
    fn accept_the_realm(&self) {
        self.shard.assert_call_database(
            &self.world,
            "operator_add_peer_shard",
            &[&self.realm_identity, "\"realm\""],
        );
        self.shard.assert_call_database(
            &self.world,
            "operator_set_shard_role",
            &[&format!(
                r#"{{"some":{{"world":{}}}}}"#,
                self.realm_identity
            )],
        );
    }

    fn realm_rows(&self, table: &str) -> usize {
        self.shard
            .query_rows(&format!("SELECT scheduled_id FROM {table}"))
            .len()
    }

    fn away_connections(&self) -> Vec<String> {
        self.shard
            .query_rows("SELECT connection_id FROM game_native_away")
            .into_iter()
            .map(|row| row["connection_id"].clone())
            .collect()
    }
}

fn provision(shard: &Standalone, database: &str) -> String {
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call_database(
        database,
        "provision_account",
        &["\"CONFORMANCE\"", &credentials, &credentials],
    );
    shard.query_database_rows(
        database,
        "SELECT id FROM game_account WHERE username = 'CONFORMANCE'",
    )[0]["id"]
        .clone()
}

/// Authenticates a new world connection for the CONFORMANCE Account on the Realm Shard.
fn authenticate(shard: &Standalone, account: &str) -> ReducerConnection {
    shard.assert_call(
        "establish_session",
        &[
            account,
            &serde_json::to_string(&vec![0x5au8; 40]).unwrap(),
            r#"{"__identity__":"0x1234"}"#,
        ],
    );
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    let mut frame = Vec::new();
    CMSG_AUTH_SESSION {
        build: 5875,
        server_id: 0,
        username: "CONFORMANCE".into(),
        client_seed: 0,
        client_proof: PROOF,
        addon_info: vec![],
    }
    .write_unencrypted_client(&mut Cursor::new(&mut frame))
    .unwrap();
    wire.call(
        "native_world_receive",
        json!([CMSG_AUTH_SESSION_OPCODE, &frame[6..]]),
    )
    .unwrap();
    wire
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_end_its_world_shard_refuses_stays_owed_until_the_world_shard_accepts_it() {
    let split = split("owed-end-accepted");
    let account = provision(&split.shard, &split.world);
    let connection = format!(r#"{{"__connection_id__":{AWAY_CONNECTION}}}"#);
    split
        .shard
        .assert_call("debug_end_world_session", &[&connection, &account]);
    assert!(
        poll_until(Duration::from_secs(20), || split
            .shard
            .query_rows("SELECT id FROM game_world_session_end")
            .is_empty()),
        "the World Shard did not answer the end"
    );
    assert_eq!(
        split.realm_rows("game_world_session_owed_end"),
        1,
        "a refused end is no longer owed"
    );

    split.accept_the_realm();
    split.shard.assert_call_database(
        &split.world,
        "debug_native_expect_handoff",
        &[&connection, "0", "\"CONFORMANCE\"", "0", "[]"],
    );
    let ended = || {
        split
            .shard
            .query_database_rows(
                &split.world,
                "SELECT connection_id FROM game_native_handoff",
            )
            .is_empty()
            && split.realm_rows("game_world_session_owed_end") == 0
    };
    assert!(
        poll_until(Duration::from_secs(45), ended),
        "the owed end did not reach the World Shard once it accepted the Realm Shard"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_bound_that_passes_while_no_ticket_waits_leaves_the_away_world_session_recorded() {
    let split = split("closed-ticket-keeps-away");
    let account = provision(&split.shard, split.shard.shard_name());
    split.shard.assert_call(
        "debug_native_away",
        &[
            &account,
            &AWAY_CONNECTION.to_string(),
            &split.world_identity,
            "1",
        ],
    );
    let away = split.away_connections();
    let wire = authenticate(&split.shard, &account);
    assert!(
        poll_until(Duration::from_secs(10), || split
            .realm_rows("game_world_session_end_retry")
            == 1
            && split.realm_rows("game_world_session_owed_end") == 1),
        "the newer ticket did not wait for the away World Session"
    );

    drop(wire);
    assert!(
        poll_until(Duration::from_secs(100), || split
            .realm_rows("game_world_session_end_retry")
            == 0),
        "the bound did not end"
    );
    assert_eq!(split.away_connections(), away);
    assert_eq!(
        split.realm_rows("game_world_session_owed_end"),
        1,
        "the end of the away World Session is still owed"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_accepted_end_drops_the_owed_end_and_the_away_record_of_its_arrival() {
    let split = split("accepted-end-drops-away");
    split.accept_the_realm();
    let account = provision(&split.shard, split.shard.shard_name());
    split.shard.assert_call(
        "debug_native_away",
        &[
            &account,
            &AWAY_CONNECTION.to_string(),
            &split.world_identity,
            "1",
        ],
    );
    let connection = format!(r#"{{"__connection_id__":{AWAY_CONNECTION}}}"#);
    split
        .shard
        .assert_call("debug_end_world_session", &[&connection, &account]);
    assert!(
        poll_until(Duration::from_secs(20), || split
            .away_connections()
            .is_empty()
            && split.realm_rows("game_world_session_owed_end") == 0),
        "the World Shard's answer did not end the away World Session it holds nothing of"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_answer_to_an_earlier_send_leaves_the_owed_end_and_the_away_record() {
    let split = split("late-end-answer");
    let account = provision(&split.shard, split.shard.shard_name());
    split.shard.assert_call(
        "debug_native_away",
        &[
            &account,
            &AWAY_CONNECTION.to_string(),
            &split.world_identity,
            "1",
        ],
    );
    let away = split.away_connections();
    let connection = format!(r#"{{"__connection_id__":{AWAY_CONNECTION}}}"#);
    let answered = || {
        split
            .shard
            .query_rows("SELECT id FROM game_world_session_end")
            .is_empty()
    };
    split
        .shard
        .assert_call("debug_end_world_session", &[&connection, &account]);
    assert!(
        poll_until(Duration::from_secs(20), answered),
        "the World Shard did not refuse the end"
    );

    split.accept_the_realm();
    split
        .shard
        .assert_call("debug_end_world_session", &[&connection, &account]);
    assert!(
        poll_until(Duration::from_secs(20), answered),
        "the World Shard did not answer the late send"
    );
    assert_eq!(split.away_connections(), away);
    assert_eq!(
        split.realm_rows("game_world_session_owed_end"),
        1,
        "an answer to a send the owed end no longer counts ended it"
    );
}
