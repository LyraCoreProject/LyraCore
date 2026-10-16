use std::cell::{Cell, RefCell};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use lyracore_test_support::Standalone;
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
    pub refusal: bool,
}

impl Outcome {
    pub fn assert_accepted(&self) {
        assert!(
            self.accepted,
            "Durable Request of {} bytes returned {} after {} ns encoding and {} ns round trip: {}",
            self.request_bytes,
            if self.refusal {
                "a Refusal"
            } else {
                "an internal failure"
            },
            self.encode_ns,
            self.round_trip_ns,
            self.message
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
        let (accepted, message, refusal) = match response.result {
            ws::ReducerOutcome::Ok(_) | ws::ReducerOutcome::OkEmpty => (true, String::new(), false),
            ws::ReducerOutcome::Err(bytes) => (
                false,
                bsatn::from_slice::<String>(&bytes).expect("reducer Refusal is a string"),
                true,
            ),
            ws::ReducerOutcome::InternalError(message) => (false, message.into(), false),
        };
        Outcome {
            accepted,
            message,
            request_bytes,
            encode_ns,
            round_trip_ns,
            refusal,
        }
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
