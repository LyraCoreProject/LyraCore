use std::cell::{Cell, RefCell};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lyracore_module::native_prototype::commit::{
    Batch, GAMEPLAY_TABLES, SNAPSHOT_PREFIX, STATE_TABLE,
};
use lyracore_test_support::{poll_until, Standalone, TableSubscription};
use serde_json::Value;
use spacetimedb::sats::{bsatn, ser::Serialize};
use spacetimedb_client_api_messages::websocket::{common, v2 as ws};
use tungstenite::{client::IntoClientRequest, Message, WebSocket};

pub struct Calls {
    socket: RefCell<WebSocket<TcpStream>>,
    next_request: Cell<u32>,
}

pub struct Outcome {
    pub accepted: bool,
    pub message: String,
    pub request_bytes: usize,
    pub encode_ns: u128,
    pub round_trip_ns: u128,
}

impl Outcome {
    pub fn assert_accepted(&self) {
        assert!(
            self.accepted,
            "Durable Request of {} bytes failed after {} ns encoding and {} ns round trip: {}",
            self.request_bytes, self.encode_ns, self.round_trip_ns, self.message
        );
    }
}

impl Calls {
    pub fn new(shard: &Standalone) -> Self {
        let address = shard
            .server()
            .strip_prefix("http://")
            .expect("fixture uses loopback HTTP");
        let stream = TcpStream::connect(address).expect("fixture connection failed");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream.set_nodelay(true).unwrap();
        let mut request = format!(
            "ws://{address}/v1/database/{}/subscribe?compression=None&confirmed=true",
            shard.shard_name(),
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", ws::BIN_PROTOCOL.parse().unwrap());
        let mut auth =
            tungstenite::http::HeaderValue::from_str(&format!("Bearer {}", shard.owner_token()))
                .unwrap();
        auth.set_sensitive(true);
        request.headers_mut().insert("Authorization", auth);
        let (mut socket, response) = tungstenite::client(request, stream)
            .unwrap_or_else(|_| panic!("fixture handshake failed"));
        assert_eq!(
            response.headers()["Sec-WebSocket-Protocol"],
            ws::BIN_PROTOCOL
        );
        assert!(matches!(
            receive(&mut socket),
            ws::ServerMessage::InitialConnection(_)
        ));
        Self {
            socket: RefCell::new(socket),
            next_request: Cell::new(1),
        }
    }

    pub fn call<T: Serialize>(&self, reducer: &str, args: &T) -> Outcome {
        let encoded_at = Instant::now();
        let request_id = self.next_request.get();
        self.next_request.set(request_id.checked_add(1).unwrap());
        let body = bsatn::to_vec(&ws::ClientMessage::CallReducer(ws::CallReducer {
            request_id,
            flags: ws::CallReducerFlags::Default,
            reducer: reducer.into(),
            args: bsatn::to_vec(args).unwrap().into(),
        }))
        .unwrap();
        let request_bytes = body.len();
        let encode_ns = encoded_at.elapsed().as_nanos();
        let sent_at = Instant::now();
        let mut socket = self.socket.borrow_mut();
        socket
            .send(Message::Binary(body.into()))
            .expect("benchmark request failed");
        let ws::ServerMessage::ReducerResult(response) = receive(&mut socket) else {
            panic!("unexpected message on the benchmark call connection");
        };
        let round_trip_ns = sent_at.elapsed().as_nanos();
        assert_eq!(response.request_id, request_id);
        let (accepted, message) = match response.result {
            ws::ReducerOutcome::Ok(_) | ws::ReducerOutcome::OkEmpty => (true, String::new()),
            ws::ReducerOutcome::Err(bytes) => (
                false,
                bsatn::from_slice::<String>(&bytes).expect("reducer Refusal is a string"),
            ),
            ws::ReducerOutcome::InternalError(message) => (false, message.into()),
        };
        Outcome {
            accepted,
            message,
            request_bytes,
            encode_ns,
            round_trip_ns,
        }
    }

    pub fn commit(&self, batch: &Batch) -> Outcome {
        let encoded_at = Instant::now();
        let payload = bsatn::to_vec(batch).unwrap();
        let encode_ns = encoded_at.elapsed().as_nanos();
        let mut outcome = self.call("debug_native_committed_apply", &(payload,));
        outcome.encode_ns += encode_ns;
        outcome
    }
}

fn receive(socket: &mut WebSocket<TcpStream>) -> ws::ServerMessage {
    loop {
        match socket.read().expect("benchmark connection closed") {
            Message::Binary(bytes) => {
                assert_eq!(
                    bytes.first(),
                    Some(&common::SERVER_MSG_COMPRESSION_TAG_NONE)
                );
                let mut remaining = &bytes[1..];
                let message = bsatn::from_reader(&mut remaining).expect("invalid binary response");
                assert!(remaining.is_empty());
                return message;
            }
            Message::Ping(_) | Message::Pong(_) => {}
            _ => panic!("unexpected WebSocket frame on the benchmark call connection"),
        }
    }
}

pub fn start() -> Standalone {
    let artifact = std::env::var_os("LYRACORE_NATIVE_REFERENCE_WASM")
        .expect("set LYRACORE_NATIVE_REFERENCE_WASM to the prototype Module artifact");
    let mut shard = Standalone::start_persistent("native-committed");
    shard.publish_module_bytes(&std::fs::read(artifact).unwrap());
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_clear_creatures", &["0"]);
    shard
}

pub fn setup(
    shard: &Standalone,
    calls: &Calls,
    scenario: &str,
    creatures: u32,
    native: bool,
) -> u64 {
    calls
        .call(
            "debug_native_committed_setup",
            &(scenario.to_owned(), creatures, native),
        )
        .assert_accepted();
    let rows = shard.query_rows(&format!("SELECT run FROM {STATE_TABLE}"));
    rows[0]["run"].parse().unwrap()
}

pub fn subscribe(shard: &Standalone) -> TableSubscription {
    let queries: Vec<_> = GAMEPLAY_TABLES
        .iter()
        .chain([&STATE_TABLE])
        .map(|table| format!("SELECT * FROM {table}"))
        .collect();
    shard
        .subscribe(&queries.iter().map(String::as_str).collect::<Vec<_>>())
        .0
}

pub fn gameplay_update(mut update: Value) -> Value {
    let tables = update
        .as_object_mut()
        .expect("subscription update is a table map");
    tables.remove(STATE_TABLE);
    tables.retain(|name, table| {
        assert!(
            GAMEPLAY_TABLES.contains(&name.as_str()),
            "unexpected subscribed table: {name}"
        );
        for operation in ["inserts", "deletes"] {
            table[operation]
                .as_array_mut()
                .unwrap()
                .sort_by_cached_key(|row| row.to_string());
        }
        !table["inserts"].as_array().unwrap().is_empty()
            || !table["deletes"].as_array().unwrap().is_empty()
    });
    update
}

pub fn snapshot(shard: &Standalone, calls: &Calls, run: u64, sequence: u64, trace: bool) -> Value {
    static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
    let nonce = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
    calls
        .call("debug_native_committed_snapshot", &(trace, nonce))
        .assert_accepted();
    let mut found = None;
    assert!(
        poll_until(Duration::from_secs(5), || {
            let logs = shard.module_logs(80);
            assert!(logs.status.success());
            found = String::from_utf8(logs.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|line| {
                    line["message"]
                        .as_str()?
                        .strip_prefix(SNAPSHOT_PREFIX)
                        .map(|message| serde_json::from_str::<Value>(message).unwrap())
                })
                .find(|frame| frame["nonce"] == nonce);
            found.is_some()
        }),
        "committed snapshot did not arrive"
    );
    let found = found.unwrap();
    assert_eq!(found["run"], run);
    assert_eq!(found["sequence"], sequence);
    found
}
