use std::collections::BTreeMap;
use std::net::TcpStream;
use std::time::Instant;

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::{Message, WebSocket};

use crate::POLL_TIMEOUT;

pub struct ReducerConnection {
    socket: WebSocket<TcpStream>,
    connection_id: Value,
    next_request: u32,
}

#[derive(Debug, Default)]
pub struct ReducerRows {
    inserts: BTreeMap<String, Vec<Value>>,
}

impl ReducerRows {
    // Rows use SpacetimeDB's positional JSON encoding, in declared column order.
    pub fn inserts(&self, table: &str) -> &[Value] {
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
                        serde_json::from_str(row.as_str().ok_or("row is not JSON text")?)
                            .map_err(|error| error.to_string())?,
                    );
                }
            }
        }
        Ok(rows)
    }
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
            connection_id: Value::Null,
            next_request: 1,
        };
        let identity = client.receive(Instant::now() + POLL_TIMEOUT)?;
        client.connection_id = identity["IdentityToken"]["connection_id"].clone();
        if client.connection_id.is_null() {
            return Err("fixture did not issue a ConnectionId".into());
        }
        client.send(json!({"Subscribe": {"query_strings": queries, "request_id": 0}}))?;
        let deadline = Instant::now() + POLL_TIMEOUT;
        loop {
            let message = client.receive(deadline)?;
            if message["InitialSubscription"]["request_id"] == 0 {
                return Ok(client);
            }
        }
    }

    pub fn connection_id(&self) -> &Value {
        &self.connection_id
    }

    // Returns subscribed inserts from this call; unrelated transactions are discarded.
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
            let message = self.receive(deadline)?;
            let transaction = &message["TransactionUpdate"];
            if transaction["reducer_call"]["request_id"] != request_id
                || transaction["caller_connection_id"] != self.connection_id
            {
                continue;
            }
            let status = &transaction["status"];
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

    fn receive(&mut self, deadline: Instant) -> Result<Value, String> {
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|time| !time.is_zero())
                .ok_or("fixture WebSocket deadline elapsed")?;
            self.socket
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            match self.socket.read().map_err(|e| e.to_string())? {
                Message::Text(text) => {
                    let message: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                    if let Some(error) = message.get("SubscriptionError") {
                        return Err(format!("fixture subscription failed: {}", error["error"]));
                    }
                    return Ok(message);
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
        let row = &rows.inserts("game_native_packet")[0];
        assert_eq!(
            row[1][1][0].to_string(),
            "340282366920938463463374607431768211454"
        );
        assert_eq!(row[3], "78563412");
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
}
