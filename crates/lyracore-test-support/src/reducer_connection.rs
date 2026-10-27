use std::collections::BTreeMap;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::{Message, WebSocket};

use crate::POLL_TIMEOUT;

pub struct ReducerConnection {
    socket: WebSocket<TcpStream>,
    connection_id: ConnectionId,
    packets: Vec<Box<RawValue>>,
    next_request: u32,
}

#[derive(Debug, Default)]
pub struct ReducerRows {
    inserts: BTreeMap<String, Vec<Box<RawValue>>>,
}

impl ReducerRows {
    // Decode positional rows directly into typed fields to preserve integers wider than u64.
    pub fn inserts(&self, table: &str) -> &[Box<RawValue>] {
        self.inserts.get(table).map(Vec::as_slice).unwrap_or(&[])
    }

    fn decode(update: &Value) -> Result<Self, String> {
        let mut rows = Self::default();
        for table in update["tables"].as_array().ok_or("missing update tables")? {
            let name = table["table_name"].as_str().ok_or("missing table name")?;
            let inserts = rows.inserts.entry(name.to_owned()).or_default();
            for query in table["updates"].as_array().ok_or("missing query updates")? {
                for row in query["inserts"].as_array().ok_or("missing inserted rows")? {
                    inserts.push(
                        RawValue::from_string(
                            row.as_str().ok_or("row is not JSON text")?.to_owned(),
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
            }
        }
        Ok(rows)
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ConnectionId {
    __connection_id__: u128,
}

#[derive(Deserialize)]
struct ReducerCall {
    request_id: u32,
}

#[derive(Deserialize)]
enum ServerMessage {
    IdentityToken {
        connection_id: ConnectionId,
    },
    InitialSubscription {
        request_id: u32,
        database_update: Value,
    },
    TransactionUpdate {
        caller_connection_id: ConnectionId,
        reducer_call: ReducerCall,
        status: Value,
    },
    TransactionUpdateLight {
        update: Value,
    },
    SubscriptionError {
        error: String,
    },
    #[serde(other)]
    Other,
}

impl ReducerConnection {
    pub(crate) fn connect(
        address: &str,
        shard: &str,
        token: Option<&str>,
        queries: &[&str],
    ) -> Result<Self, String> {
        let stream = TcpStream::connect_timeout(
            &address.parse().map_err(|_| "invalid fixture address")?,
            POLL_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(POLL_TIMEOUT))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(POLL_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let mut request = format!("ws://{address}/v1/database/{shard}/subscribe?compression=None")
            .into_client_request()
            .map_err(|error| error.to_string())?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("v1.json.spacetimedb"),
        );
        if let Some(token) = token {
            let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "invalid fixture Owner Token")?;
            authorization.set_sensitive(true);
            request.headers_mut().insert("Authorization", authorization);
        }
        let (socket, _) =
            tungstenite::client(request, stream).map_err(|error| error.to_string())?;
        let mut client = Self {
            socket,
            connection_id: ConnectionId {
                __connection_id__: 0,
            },
            packets: Vec::new(),
            next_request: 1,
        };
        let Some(ServerMessage::IdentityToken { connection_id }) =
            client.receive(Instant::now() + POLL_TIMEOUT)?
        else {
            return Err("fixture did not issue a ConnectionId".into());
        };
        client.connection_id = connection_id;
        client.send(json!({"Subscribe": {"query_strings": queries, "request_id": 0}}))?;
        let deadline = Instant::now() + POLL_TIMEOUT;
        loop {
            match client.receive(deadline)? {
                Some(ServerMessage::InitialSubscription { request_id: 0, .. }) => {
                    return Ok(client)
                }
                None => return Err("fixture subscription deadline elapsed".into()),
                _ => {}
            }
        }
    }

    pub fn connection_id(&self) -> u128 {
        self.connection_id.__connection_id__
    }

    // Drains received inserts; wait_for_packets reads transactions still pending on the socket.
    pub fn drain_packets(&mut self) -> Vec<Box<RawValue>> {
        std::mem::take(&mut self.packets)
    }

    // Returns buffered packet rows or waits for the first batch; an empty result means timeout.
    pub fn wait_for_packets(&mut self, timeout: Duration) -> Result<Vec<Box<RawValue>>, String> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or("invalid packet timeout")?;
        while self.packets.is_empty() {
            if self.receive(deadline)?.is_none() {
                break;
            }
        }
        Ok(self.drain_packets())
    }

    // Returns this call's inserts; all subscribed packet inserts also remain in drain_packets.
    pub fn call(&mut self, reducer: &str, args: Value) -> Result<ReducerRows, String> {
        if !args.is_array() {
            return Err("reducer arguments must be an array".into());
        }
        let request_id = self.next_request;
        self.next_request = request_id.checked_add(1).ok_or("request ids exhausted")?;
        self.send(json!({"CallReducer": {
            "reducer": reducer, "args": args.to_string(), "request_id": request_id, "flags": 0
        }}))?;
        let deadline = Instant::now() + POLL_TIMEOUT;
        loop {
            let message = self
                .receive(deadline)?
                .ok_or("fixture reducer deadline elapsed")?;
            let ServerMessage::TransactionUpdate {
                caller_connection_id,
                reducer_call,
                status,
            } = message
            else {
                continue;
            };
            if reducer_call.request_id != request_id || caller_connection_id != self.connection_id {
                continue;
            }
            if let Some(update) = status.get("Committed") {
                return ReducerRows::decode(update);
            }
            if let Some(error) = status["Failed"].as_str() {
                return Err(error.to_owned());
            }
            return Err(format!("reducer {reducer} did not commit: {status}"));
        }
    }

    fn send(&mut self, message: Value) -> Result<(), String> {
        self.socket
            .send(Message::Text(message.to_string().into()))
            .map_err(|e| e.to_string())
    }

    fn receive(&mut self, deadline: Instant) -> Result<Option<ServerMessage>, String> {
        loop {
            let Some(remaining) = deadline
                .checked_duration_since(Instant::now())
                .filter(|time| !time.is_zero())
            else {
                return Ok(None);
            };
            self.socket
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            let frame = match self.socket.read() {
                Ok(frame) => frame,
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(error) => return Err(error.to_string()),
            };
            match frame {
                Message::Text(text) => {
                    let message: ServerMessage =
                        serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    let update = match &message {
                        ServerMessage::InitialSubscription {
                            database_update, ..
                        } => Some(database_update),
                        ServerMessage::TransactionUpdate { status, .. } => status.get("Committed"),
                        ServerMessage::TransactionUpdateLight { update } => Some(update),
                        ServerMessage::SubscriptionError { error } => {
                            return Err(format!("fixture subscription failed: {error}"));
                        }
                        _ => None,
                    };
                    if let Some(update) = update {
                        let mut rows = ReducerRows::decode(update)?;
                        self.packets.extend(
                            rows.inserts
                                .remove("game_native_packet")
                                .unwrap_or_default(),
                        );
                    }
                    return Ok(Some(message));
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => return Err("fixture WebSocket closed".into()),
                _ => return Err("unexpected fixture WebSocket frame".into()),
            }
        }
    }
}

impl Drop for ReducerConnection {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribed_rows_preserve_connection_ids_and_packet_bytes() {
        let rows = ReducerRows::decode(&json!({"tables": [{
            "table_name": "game_native_packet", "updates": [{
                "inserts": ["[1,[0,[340282366920938463463374607431768211454]],[0,477],\"78563412\",false,0,false]"],
                "deletes": []
            }]
        }]})).unwrap();
        type Packet = (u64, (u8, (u128,)), (u8, u16), String, bool, i64, bool);
        let (_, (_, (connection_id,)), _, body, _, _, _): Packet =
            serde_json::from_str(rows.inserts("game_native_packet")[0].get()).unwrap();
        assert_eq!(connection_id, u128::MAX - 1);
        assert_eq!(body, "78563412");
        assert!(rows.inserts("absent").is_empty());
    }

    #[test]
    fn malformed_subscription_updates_fail_instead_of_becoming_empty_evidence() {
        assert!(ReducerRows::decode(&json!({})).is_err());
        assert!(ReducerRows::decode(&json!({"tables": [{
            "table_name": "game_native_packet", "updates": [{"inserts": ["[truncated"]}]
        }]}))
        .is_err());
    }

    fn peer(
        run: impl FnOnce(WebSocket<TcpStream>) + Send + 'static,
    ) -> (ReducerConnection, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(POLL_TIMEOUT)).unwrap();
            let mut socket = tungstenite::accept_hdr(
                stream,
                |_: &tungstenite::handshake::server::Request,
                 mut response: tungstenite::handshake::server::Response| {
                    response.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        HeaderValue::from_static("v1.json.spacetimedb"),
                    );
                    Ok(response)
                },
            )
            .unwrap();
            socket.send(Message::text(
                r#"{"IdentityToken":{"connection_id":{"__connection_id__":340282366920938463463374607431768211454}}}"#
            )).unwrap();
            socket.read().unwrap();
            socket
                .send(Message::text(
                    r#"{"InitialSubscription":{"request_id":0,"database_update":{"tables":[]}}}"#,
                ))
                .unwrap();
            run(socket);
        });
        let client = ReducerConnection::connect(
            &address,
            "fixture",
            None,
            &["SELECT * FROM game_native_packet"],
        )
        .unwrap();
        (client, handle)
    }

    fn update(row: &str) -> Value {
        json!({"tables": [{"table_name": "game_native_packet", "updates": [{"inserts": [row]}]}]})
    }

    #[test]
    fn calls_keep_other_transactions_and_their_own_packets_in_order() {
        let (mut client, handle) = peer(|mut socket| {
            socket.read().unwrap();
            for (caller, row) in [
                ("1", "[1]"),
                ("340282366920938463463374607431768211454", "[2]"),
            ] {
                let update = update(row);
                socket.send(Message::text(format!(
                    r#"{{"TransactionUpdate":{{"caller_connection_id":{{"__connection_id__":{caller}}},"reducer_call":{{"request_id":1}},"status":{{"Committed":{update}}}}}}}"#
                ))).unwrap();
            }
        });
        assert_eq!(client.connection_id(), u128::MAX - 1);
        let rows = client.call("example", json!([])).unwrap();
        assert_eq!(rows.inserts("game_native_packet")[0].get(), "[2]");
        let packets = client.drain_packets();
        assert_eq!(
            packets.iter().map(|row| row.get()).collect::<Vec<_>>(),
            ["[1]", "[2]"]
        );
        assert!(client.drain_packets().is_empty());
        handle.join().unwrap();
    }

    #[test]
    fn waits_receive_later_transactions_without_a_reducer_call() {
        let (mut client, handle) = peer(|mut socket| {
            let update = update("[3]");
            socket
                .send(Message::text(format!(
                    r#"{{"TransactionUpdateLight":{{"request_id":0,"update":{update}}}}}"#
                )))
                .unwrap();
        });
        assert!(client.drain_packets().is_empty());
        let packets = client.wait_for_packets(Duration::from_secs(2)).unwrap();
        assert_eq!(packets[0].get(), "[3]");
        handle.join().unwrap();
    }

    #[test]
    fn a_packet_wait_can_time_out_and_then_receive_more_packets() {
        let (send, receive) = std::sync::mpsc::channel();
        let (mut client, handle) = peer(move |mut socket| {
            receive.recv_timeout(POLL_TIMEOUT).unwrap();
            let update = update("[4]");
            socket.send(Message::text(format!(
                r#"{{"TransactionUpdate":{{"caller_connection_id":{{"__connection_id__":0}},"reducer_call":{{"request_id":0}},"status":{{"Committed":{update}}}}}}}"#
            ))).unwrap();
        });
        assert!(client.wait_for_packets(Duration::ZERO).unwrap().is_empty());
        assert!(client
            .wait_for_packets(Duration::from_millis(20))
            .unwrap()
            .is_empty());
        send.send(()).unwrap();
        assert_eq!(
            client.wait_for_packets(Duration::from_secs(2)).unwrap()[0].get(),
            "[4]"
        );
        handle.join().unwrap();
    }

    #[test]
    fn packet_waits_report_transport_and_subscription_failures() {
        for frame in [
            Message::Close(None),
            Message::text(r#"{"SubscriptionError":{"error":"subscription ended"}}"#),
        ] {
            let (mut client, handle) = peer(move |mut socket| {
                socket.send(frame).unwrap();
            });
            assert!(client.wait_for_packets(Duration::from_secs(2)).is_err());
            handle.join().unwrap();
        }
    }
}
