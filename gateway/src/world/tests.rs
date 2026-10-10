use super::handlers::{
    AuctionInteraction, ChannelOutcome, ChatOutcome, LootWindowRefusal, RealmChatRequest,
    SpeakerFacts, WhisperRequest, WhisperTargetFacts,
};
use super::party::PartyOutcome;
use super::test_support::*;
use super::*;
use crate::read_deadline::{DeadlineClock, PreAuthDeadline};
use lyracore_shared::group::{GroupKind, GroupRefusal, RaidSlot};
use lyracore_shared::item::ItemRefusal;
use lyracore_shared::loot::LootRefusal;
use lyracore_shared::social::ContactRefusal;
use std::cell::Cell;
use std::io::Cursor;
use std::os::unix::net::UnixStream;
use std::rc::Rc;

#[derive(Clone)]
struct ManualClock(Rc<Cell<Instant>>);

impl DeadlineClock for ManualClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
}

struct AdvancingStream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    clock: ManualClock,
    per_read: Duration,
    per_write: Duration,
    write_timeout_calls: Rc<Cell<usize>>,
}

impl Read for AdvancingStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let one_byte = buf.len().min(1);
        let read = self.input.read(&mut buf[..one_byte])?;
        self.clock.0.set(self.clock.0.get() + self.per_read);
        Ok(read)
    }
}

impl Write for AdvancingStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(buf);
        self.clock.0.set(self.clock.0.get() + self.per_write);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl IoDeadline for AdvancingStream {
    fn set_read_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }

    fn set_write_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
        self.write_timeout_calls
            .set(self.write_timeout_calls.get() + 1);
        Ok(())
    }
}

/// The client side of every real world-session test has a bounded read. A missing server packet is
/// a test failure, never an indefinitely blocked test process.
const WORLD_SESSION_READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

fn world_session_socket_pair() -> (UnixStream, UnixStream) {
    let (client, server) = UnixStream::pair().expect("world-session socket pair must be created");
    client
        .set_read_timeout(Some(WORLD_SESSION_READ_DEADLINE))
        .expect("world-session client read deadline must be configured");
    (client, server)
}

#[test]
fn world_session_socket_pair_times_out_when_the_server_writes_nothing() {
    let (mut client, _server) = world_session_socket_pair();
    let mut byte = [0];
    let error = client
        .read(&mut byte)
        .expect_err("a silent server must hit the world-session read deadline");
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        "a silent server must time out, got {error}"
    );
}

/// The realm-wide party routing tests. A child module so they can reach
/// `WorldFake` and its fake realm-core topology without widening anything, kept in their own
/// file because this one is already the largest in the tree.
#[path = "party_tests.rs"]
mod party_tests;

/// The Roster Revision Relay against the party topology. A sibling of `party_tests` so it reaches
/// `WorldFake` and that topology without widening anything.
#[path = "party_mirror_tests.rs"]
mod party_mirror_tests;

/// Member Stats: the Relay tick and the stats request against the party topology. A sibling of
/// `party_tests` so it reaches `WorldFake` and that topology without widening anything.
#[path = "member_stats_tests.rs"]
mod member_stats_tests;

/// The realm-wide whisper routing tests. A sibling of `party_tests` for the
/// same reason — it reaches `WorldFake` (and `party_tests`' live topology) without widening
/// anything.
#[path = "whisper_tests.rs"]
mod whisper_tests;

/// Realm Presence's multi-shard union (`presence::of`, `presence::in_world_characters`) — reads
/// that moved out of `party.rs`. A sibling of `party_tests`/`whisper_tests` for the same reason: it
/// reaches `WorldFake` and `party_tests`' fixture characters without widening anything.
#[path = "presence_tests.rs"]
mod presence_tests;

/// `/who`'s Store-Fake tests: the multi-shard listing and the 49-cap/oversized-request rules that
/// need real presence rows rather than the hand-written filter inputs `who.rs`'s own unit tests
/// use. A sibling of `presence_tests` for the same reason.
#[path = "who_tests.rs"]
mod who_tests;

/// The realm-wide loot-roll routing/relay tests. A sibling of `party_tests`/`whisper_tests` for
/// the same reason — it reaches `WorldFake` without widening anything.
#[path = "loot_tests.rs"]
mod loot_tests;

/// The mailbox read-path routing tests. A sibling of the modules above for the same reason — it
/// reaches `WorldFake` (and `party_tests`' fixture characters) without widening anything.
#[path = "mail_tests.rs"]
mod mail_tests;

/// The inbound FRAMING boundary — malformed, truncated, oversized and unsupported packets
/// driven as raw bytes over a real cipher. A sibling of the modules above for the same reason (it
/// reaches `WorldFake` and `client_handshake`), kept separate because it is the only file here
/// that writes headers no typed builder can produce.
#[path = "framing_tests.rs"]
mod framing_tests;

/// Account-owned Alpha Test Tools at the Headless Client seam. This stays separate from the Module
/// command Gate tests: it proves dot-Say dispatch, current authority, and client-visible results.
#[path = "alpha_test_tools_tests.rs"]
mod alpha_test_tools_tests;

/// Multi-shard routing — reducer calls and subscriptions never target a shard other than the
/// player's home shard. A sibling of the modules above for the same reason. `ShardCallLog` is
/// `pub(super)` here and re-exported below because this file's own pre-shard-routing tests (and
/// `transfer_tests`) still share the one call-log type.
#[path = "shard_routing_tests.rs"]
mod shard_routing_tests;
use shard_routing_tests::ShardCallLog;

/// Cross-database Transfer tests share the Character and Escrow Fakes defined here.
#[path = "transfer_tests.rs"]
mod transfer_tests;

#[path = "trade_tests.rs"]
mod trade_tests;
/// `SMSG_COMPRESSED_MOVES` corruption regressions, driven through the real `spawn_writer` +
/// `wow_srp` cipher pair. A sibling of the modules above for the same reason.
#[path = "wire_corruption_tests.rs"]
mod wire_corruption_tests;

use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    BuyBankSlotResult,
    BuyResult,
    BuybackSlot,
    Class,
    ClientMessage,
    Gender,
    GroupLootSetting,
    ItemQuality,
    Language,
    Level,
    MSG_AUCTION_HELLO_Client,
    Map,
    Object,
    Race,
    RollVote,
    SheathState,
    SpellCastTargets,
    SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_Unit,
    Talent,
    TrainingFailureReason,
    WeatherType,
    WorldResult,
    CMSG_ACTIVATETAXI,
    CMSG_ADD_FRIEND,
    CMSG_ADD_IGNORE,
    CMSG_ATTACKSTOP,
    CMSG_ATTACKSWING,
    CMSG_AUCTION_LIST_BIDDER_ITEMS,
    CMSG_AUCTION_LIST_ITEMS,
    CMSG_AUCTION_LIST_OWNER_ITEMS,
    CMSG_AUTH_SESSION,
    CMSG_AUTOBANK_ITEM,
    CMSG_AUTOEQUIP_ITEM,
    CMSG_AUTOSTORE_BANK_ITEM,
    CMSG_AUTOSTORE_LOOT_ITEM,
    CMSG_BANKER_ACTIVATE,
    CMSG_BUYBACK_ITEM,
    CMSG_BUY_BANK_SLOT,
    CMSG_BUY_ITEM,
    CMSG_CANCEL_AURA,
    CMSG_CANCEL_AUTO_REPEAT_SPELL,
    CMSG_CANCEL_CAST,
    CMSG_CAST_SPELL,
    CMSG_CHAR_CREATE,
    CMSG_CHAR_DELETE,
    CMSG_CHAR_ENUM,
    CMSG_DEL_FRIEND,
    CMSG_DEL_IGNORE,
    CMSG_FRIEND_LIST,
    CMSG_GAMEOBJ_USE,
    CMSG_GOSSIP_HELLO,
    CMSG_GOSSIP_SELECT_OPTION,
    CMSG_GUILD_ACCEPT,
    CMSG_GUILD_DECLINE,
    CMSG_GUILD_DEMOTE,
    CMSG_GUILD_DISBAND,
    CMSG_GUILD_INVITE,
    CMSG_GUILD_LEADER,
    CMSG_GUILD_LEAVE,
    CMSG_GUILD_PROMOTE,
    CMSG_GUILD_REMOVE,
    CMSG_INSPECT,
    CMSG_ITEM_QUERY_SINGLE,
    CMSG_LEARN_TALENT,
    CMSG_LIST_INVENTORY,
    CMSG_LOGOUT_REQUEST,
    CMSG_LOOT,
    CMSG_LOOT_MASTER_GIVE,
    CMSG_LOOT_METHOD,
    CMSG_LOOT_MONEY,
    CMSG_LOOT_RELEASE,
    CMSG_LOOT_ROLL,
    CMSG_NPC_TEXT_QUERY,
    CMSG_PLAYED_TIME,
    CMSG_PLAYER_LOGIN,
    CMSG_QUESTGIVER_CHOOSE_REWARD,
    CMSG_QUESTGIVER_HELLO,
    CMSG_QUESTGIVER_STATUS_QUERY,
    CMSG_QUEST_QUERY,
    // Item guid → slot resolution (vendor sell / armorer repair).
    CMSG_RECLAIM_CORPSE,
    CMSG_REPOP_REQUEST,
    CMSG_RESURRECT_RESPONSE,
    CMSG_SELF_RES,
    CMSG_SETSHEATHED,
    CMSG_SET_SELECTION,
    CMSG_SPIRIT_HEALER_ACTIVATE,
    CMSG_TAXINODE_STATUS_QUERY,
    CMSG_TAXIQUERYAVAILABLENODES,
    CMSG_TRAINER_BUY_SPELL,
    CMSG_TRAINER_LIST,
    CMSG_WHO,
    // Cross-map teleport: the client's world-port-finished ack.
    MSG_MOVE_WORLDPORT_ACK,
    SMSG_WEATHER,
};
use wow_world_messages::Guid;

const K: [u8; 40] = [
    0x2E, 0xFE, 0xE7, 0xB0, 0xC1, 0x77, 0xEB, 0xBD, 0xFF, 0x66, 0x76, 0xC5, 0x6E, 0xFC, 0x23, 0x39,
    0xBE, 0x9C, 0xAD, 0x14, 0xBF, 0x8B, 0x54, 0xBB, 0x5A, 0x86, 0xFB, 0xF8, 0x1F, 0x6D, 0x42, 0x4A,
    0xA2, 0x3C, 0xC9, 0xA3, 0x14, 0x9F, 0xB1, 0x75,
];

fn ns(s: &str) -> NormalizedString {
    NormalizedString::new(s).unwrap()
}

/// Drive the shared prefix of every world handshake: read the plaintext `SMSG_AUTH_CHALLENGE`,
/// derive the client-side proof + cipher pair for `key` with `wow_srp`, and send
/// `CMSG_AUTH_SESSION`. Lower-level than [`client_handshake`] — it does not read whatever comes
/// back, so a call site can assert on that itself (an `AuthOk`, an `AuthWaitQueue`, a plaintext
/// rejection...). Returns the cipher pair `into_client_header_crypto` derived, split; a `key` that
/// does not match what the server holds still produces a (mismatched, useless) pair here — the
/// send happens regardless — so a rejection-path call site is free to bind them as `_`.
fn drive_auth<S: Read + Write>(
    client: &mut S,
    username: &str,
    key: [u8; 40],
) -> (EncrypterHalf, DecrypterHalf) {
    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut *client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, crypto) =
        client_seed.into_client_header_crypto(&ns(username), key, server_seed);
    let (enc, dec) = crypto.split();

    auth_session(username, client_seed_value, client_proof)
        .write_unencrypted_client(&mut *client)
        .unwrap();

    (enc, dec)
}

/// Drive the client side of the world handshake against a server running `run_world_session`
/// (or `world_handshake`): read the plaintext challenge, send `CMSG_AUTH_SESSION` with a
/// valid proof, read the encrypted AUTH_OK. Returns the client's cipher halves for the
/// post-handshake encrypted traffic.
fn client_handshake<S: Read + Write>(
    client: &mut S,
    username: &str,
    key: [u8; 40],
) -> (EncrypterHalf, DecrypterHalf) {
    let (enc, mut dec) = drive_auth(client, username, key);

    match ServerOpcodeMessage::read_encrypted(&mut *client, &mut dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }
    (enc, dec)
}

fn auth_session(username: &str, client_seed: u32, client_proof: [u8; 20]) -> CMSG_AUTH_SESSION {
    CMSG_AUTH_SESSION {
        build: 5875,
        server_id: 1,
        username: username.to_string(),
        client_seed,
        client_proof,
        addon_info: vec![],
    }
}

/// A store for a logged-in TESTER (account `account_id`), with no character/scenario state beyond
/// the session itself — the shape every handshake/login/logout/movement test overlays with its own
/// fields via `..`. `quest_store()` is the sibling for tests that also need a login entity.
fn tester_store(account_id: u64) -> WorldFake {
    WorldFake {
        session: SessionState {
            entity_in_world: true,
            username: "TESTER".into(),
            session: Some(WorldSession {
                account_id,
                session_key: K,
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn handshake_succeeds_and_traffic_is_encrypted_both_ways() {
    let store = std::sync::Arc::new(tester_store(42));

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let (mut conn, _encrypt) = world_handshake(&mut s, server_store.clone())
            .unwrap()
            .expect("handshake should succeed");
        assert_eq!(conn.account_id, 42);
        assert_eq!(conn.account_name, "TESTER");
        // Prove the inbound cipher works: read one encrypted client message.
        match ClientOpcodeMessage::read_encrypted(&mut s, &mut conn.decrypt).unwrap() {
            ClientOpcodeMessage::CMSG_CHAR_ENUM => {}
            other => panic!("expected encrypted CMSG_CHAR_ENUM, got {other}"),
        }
    });

    // --- client: drive the shared challenge→proof→AUTH_SESSION prefix (`drive_auth`) ---
    let (mut c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // --- client: the AUTH_OK response is the first ENCRYPTED packet ---
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    // --- client: send an encrypted CMSG the server decrypts ---
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
}

/// The typed decoder would size a buffer from the addon field and unwrap the zlib. The gateway
/// never reads the addon list, so a 4 GiB claim over garbage bytes changes nothing: the proof is
/// checked and AUTH_OK goes out.
#[test]
fn an_auth_session_with_an_absurd_addon_size_still_completes_the_handshake() {
    let store = std::sync::Arc::new(tester_store(42));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), K, server_seed);
    let (_c_enc, mut c_dec) = crypto.split();

    // Hand-built frame: the fixed fields, then a 4 GiB decompressed-size claim over garbage.
    let mut body = Vec::new();
    body.extend_from_slice(&5875u32.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(b"TESTER\0");
    body.extend_from_slice(&client_seed_value.to_le_bytes());
    body.extend_from_slice(&client_proof);
    body.extend_from_slice(&u32::MAX.to_le_bytes());
    body.extend_from_slice(&[0xFF; 32]);
    let mut frame = Vec::new();
    frame.extend_from_slice(&((body.len() + 4) as u16).to_be_bytes());
    frame.extend_from_slice(&CMSG_AUTH_SESSION_OPCODE.to_le_bytes());
    frame.extend_from_slice(&body);
    client.write_all(&frame).unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }));
        }
        other => panic!("expected encrypted AUTH_OK, got {other}"),
    }
    drop(client);
    server.join().unwrap().unwrap();
}

/// A peer that reads the challenge and never answers must give its blocking thread back at the
/// total pre-auth deadline.
#[test]
fn a_silent_world_connection_is_closed_at_the_pre_auth_deadline() {
    let store = tester_store(42);
    let (mut client, server_end) = world_session_socket_pair();
    let mut deadline = PreAuthDeadline::after(Duration::from_millis(200));
    let server = std::thread::spawn(move || {
        run_world_session_with_queue_and_deadline(
            server_end,
            std::sync::Arc::new(store),
            &LoginQueue::unlimited(),
            &mut deadline,
        )
    });

    ServerOpcodeMessage::read_unencrypted(&mut client).unwrap();
    let err = server
        .join()
        .unwrap()
        .expect_err("a silent peer must be cut at the deadline");
    assert!(
        err.to_string().contains("total pre-auth deadline"),
        "{err:#}"
    );
    let mut byte = [0u8];
    assert_eq!(
        client.read(&mut byte).unwrap(),
        0,
        "the socket must be closed"
    );
}

#[test]
fn slow_auth_session_bytes_cannot_extend_the_pre_auth_deadline() {
    let store = tester_store(42);
    let mut input = Vec::new();
    auth_session("TESTER", 1, [0; 20])
        .write_unencrypted_client(&mut input)
        .unwrap();
    let start = Instant::now();
    let clock = ManualClock(Rc::new(Cell::new(start)));
    let mut deadline = PreAuthDeadline::with_clock(start + Duration::from_millis(8), clock.clone());
    let mut stream = AdvancingStream {
        input: Cursor::new(input),
        output: Vec::new(),
        clock,
        per_read: Duration::from_millis(1),
        per_write: Duration::ZERO,
        write_timeout_calls: Rc::new(Cell::new(0)),
    };

    let result = world_handshake_with_queue_and_deadline(
        &mut stream,
        std::sync::Arc::new(store),
        &LoginQueue::unlimited(),
        &mut deadline,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a slow auth frame must share one total deadline"),
    };

    assert!(
        error.to_string().contains("total pre-auth deadline"),
        "{error:#}"
    );
    assert!(
        !stream.output.is_empty(),
        "the server challenge must be written before the slow client frame times out"
    );
}

#[test]
fn world_challenge_write_uses_the_absolute_pre_auth_deadline() {
    let store = tester_store(42);
    let start = Instant::now();
    let clock = ManualClock(Rc::new(Cell::new(start)));
    let write_timeout_calls = Rc::new(Cell::new(0));
    let mut deadline =
        PreAuthDeadline::with_clock(start + Duration::from_millis(10), clock.clone());
    let mut stream = AdvancingStream {
        input: Cursor::new(Vec::new()),
        output: Vec::new(),
        clock,
        per_read: Duration::ZERO,
        per_write: Duration::from_millis(20),
        write_timeout_calls: write_timeout_calls.clone(),
    };

    let result = world_handshake_with_queue_and_deadline(
        &mut stream,
        std::sync::Arc::new(store),
        &LoginQueue::unlimited(),
        &mut deadline,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("the world challenge must not write past the total deadline"),
    };

    assert_eq!(
        write_timeout_calls.get(),
        1,
        "the world challenge must write through the pre-auth I/O wrapper"
    );
    assert!(
        error.to_string().contains("pre-auth I/O deadline"),
        "{error:#}"
    );
    assert!(
        !stream.output.is_empty(),
        "the fake advances time after accepting the world challenge bytes"
    );
}

/// Once client proof succeeds, authenticated World Session traffic has no pre-auth deadline.
#[test]
fn post_auth_world_traffic_has_no_pre_auth_deadline() {
    let store = std::sync::Arc::new(tester_store(42));
    let (mut client, server_end) = world_session_socket_pair();
    let mut deadline = PreAuthDeadline::after(Duration::from_millis(200));
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session_with_queue_and_deadline(
            server_end,
            server_store.clone(),
            &LoginQueue::unlimited(),
            &mut deadline,
        )
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    std::thread::sleep(Duration::from_millis(500));
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }
    drop(client);
    server.join().unwrap().unwrap();
}

#[test]
fn queued_handshake_sends_wait_queue_then_admits_once_a_seat_frees() {
    let store = std::sync::Arc::new(tester_store(42));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    // Occupy the only seat directly — exactly what an already-connected world session holds.
    assert_eq!(queue.request(), Admission::Admitted);

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        // Blocks in the queue's wait loop until `queue.depart()` (below) frees the one seat.
        let (conn, _encrypt) =
            world_handshake_with_queue(&mut s, server_store.clone(), &server_queue)
                .unwrap()
                .expect("handshake should eventually succeed once admitted");
        assert_eq!(conn.account_id, 42);
    });

    // --- client: drive the shared challenge→proof→AUTH_SESSION prefix (`drive_auth`) ---
    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // --- client: the FIRST encrypted response must be AuthWaitQueue at position 1, not AuthOk —
    // proving the handshake actually queued instead of bypassing the gate ---
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { queue_position: 1 }),
                "expected AuthWaitQueue at position 1, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    // Free the one seat — mirrors what `run_world_session_with_queue`'s teardown does on a real
    // disconnect. The queued handshake must notice on its next poll and proceed to admission.
    queue.depart();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(
                matches!(*r, SMSG_AUTH_RESPONSE::AuthOk { .. }),
                "expected AuthOk, got {r:?}"
            );
        }
        other => panic!("expected encrypted SMSG_AUTH_RESPONSE, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn disconnecting_while_queued_leaves_the_line_without_taking_a_seat() {
    let store = std::sync::Arc::new(tester_store(7));
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    assert_eq!(queue.request(), Admission::Admitted); // occupy the only seat

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let result =
            world_handshake_with_queue(&mut s, server_store.clone(), &server_queue).unwrap();
        assert!(
            result.is_none(),
            "a hangup while queued must end the session cleanly, not error"
        );
    });

    let (_c_enc, mut c_dec) = drive_auth(&mut client, "TESTER", K);

    // Read (and discard) the first AuthWaitQueue so we know the server has actually queued us
    // before hanging up — a hangup racing the very first send would prove nothing about `cancel`.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthWaitQueue { .. }));
        }
        other => panic!("expected AuthWaitQueue, got {other}"),
    }

    drop(client); // hang up while still queued
    server.join().unwrap();

    // The seat is still held by the ORIGINAL occupant (never touched); the queued-then-cancelled
    // connection must have left NO trace in the line.
    assert_eq!(
        queue.depth(),
        0,
        "the cancelled ticket must not linger in the queue"
    );
    assert_eq!(
        queue.active(),
        1,
        "cancelling a waiter must never grant or free a seat"
    );
}

#[test]
fn a_restarted_gateway_completes_the_handshake_from_realm_state_alone() {
    // The stateless-gateway invariant, now realm-scoped. The session key K lives in
    // `game_session` — on realm-core when it is configured, on the world DB when it is not — and
    // the gateway keeps NOTHING about it. So killing the gateway mid-session and starting a fresh
    // one must let the same account re-handshake with no re-logon, against a brand-new process
    // that has never seen this client.
    //
    // Modelled here as two handshakes against two INDEPENDENT store instances that share only the
    // realm-held session row: `store_before` is the gateway that gets killed, `store_after` is the
    // replacement. Each handshake mints its own server seed and its own client seed, so nothing
    // from the first run can be smuggled into the second — if any handshake input were
    // gateway-local rather than realm state, the second run could not succeed.
    let realm_session = || WorldSession {
        account_id: 42,
        session_key: K,
    };
    let handshake_once = |store: WorldFake| {
        let (mut client, server_end) = world_session_socket_pair();
        let server = std::thread::spawn(move || {
            let mut s = server_end;
            let established = world_handshake(&mut s, std::sync::Arc::new(store))
                .unwrap()
                .expect("handshake should succeed");
            established.0.account_id
        });
        client_handshake(&mut client, "TESTER", K);
        drop(client);
        server.join().unwrap()
    };

    let store_before = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(realm_session()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(handshake_once(store_before), 42);

    // ---- the gateway is killed here; every byte of its in-process state is gone ----

    let store_after = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(realm_session()), // re-READ from the realm, not carried over
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        handshake_once(store_after),
        42,
        "a fresh gateway must re-establish the session from realm-held state alone"
    );
}

#[test]
fn a_gateway_that_cannot_reach_the_session_store_rejects_rather_than_guessing() {
    // The other half of the stateless-gateway invariant: "resume from realm state" must not
    // degrade into "resume from anything". A store that cannot answer (realm-core unreachable →
    // `lookup_session` yields no session) rejects the handshake plaintext instead of establishing
    // a session on an unverified key. `CoordinatorStore` reaches this state by way of
    // `Coordinator::realm_core()`'s Err.
    let store = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(
            world_handshake(&mut s, std::sync::Arc::new(store))
                .unwrap()
                .is_none(),
            "no session material ⇒ no session, never a best-effort one"
        );
    });

    let (_enc, _dec) = drive_auth(&mut client, "TESTER", K);
    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthUnknownAccount));
        }
        other => panic!("expected a plaintext rejection, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn bad_proof_is_rejected() {
    // The store hands out a session key, but the client computes its proof against a
    // DIFFERENT key, so the server's digest check must fail.
    let store = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: Some(WorldSession {
                account_id: 1,
                session_key: K,
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        let conn = world_handshake(&mut s, std::sync::Arc::new(store)).unwrap();
        assert!(conn.is_none(), "bad proof must not establish a connection");
    });

    let server_seed = match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_CHALLENGE(c) => c.server_seed,
        other => panic!("expected SMSG_AUTH_CHALLENGE, got {other}"),
    };

    let wrong_key = [0x11u8; 40];
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, _crypto) =
        client_seed.into_client_header_crypto(&ns("TESTER"), wrong_key, server_seed);

    auth_session("TESTER", client_seed_value, client_proof)
        .write_unencrypted_client(&mut client)
        .unwrap();

    // The failure is sent plaintext (no cipher was established).
    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthFailed));
        }
        other => panic!("expected plaintext SMSG_AUTH_RESPONSE failure, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn unknown_account_is_rejected_cleanly() {
    let store = WorldFake {
        session: SessionState {
            username: "TESTER".into(),
            session: None, // account exists nowhere / no session
            ..Default::default()
        },
        ..Default::default()
    };

    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        let mut s = server_end;
        assert!(world_handshake(&mut s, std::sync::Arc::new(store))
            .unwrap()
            .is_none());
    });

    let (_enc, _dec) = drive_auth(&mut client, "NOBODY", K);

    match ServerOpcodeMessage::read_unencrypted(&mut client).unwrap() {
        ServerOpcodeMessage::SMSG_AUTH_RESPONSE(r) => {
            assert!(matches!(*r, SMSG_AUTH_RESPONSE::AuthUnknownAccount));
        }
        other => panic!("expected SMSG_AUTH_RESPONSE unknown-account, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_enum_returns_the_seeded_character() {
    // The pre-seeded Human Warrior "Tester" must appear on the character-select screen.
    let tester = codec::CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,   // Human
        class: 1,  // Warrior
        gender: 0, // Male
        level: 1,
        map_id: 0,   // Eastern Kingdoms
        zone_id: 12, // Elwynn Forest
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        first_login: true,
        ..Default::default()
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            characters: vec![tester],
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    // Full handshake, then request the character list over the encrypted channel.
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(e) => {
            assert_eq!(e.characters.len(), 1);
            let ch = &e.characters[0];
            assert_eq!(ch.name, "Tester");
            assert_eq!(ch.guid.guid(), 1);
            assert_eq!(ch.race, Race::Human);
            assert_eq!(ch.class, Class::Warrior);
            assert_eq!(ch.level, Level::new(1));
            assert_eq!(ch.map, Map::EasternKingdoms);
        }
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn guild_query_answers_at_character_select() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            guild: GuildState {
                guilds: vec![codec::GuildView {
                    guild_id: 7,
                    name: "Tracer Guild".into(),
                    ranks: vec![codec::GuildRankView {
                        rank_id: 0,
                        name: "Guild Master".into(),
                        rights: lyracore_shared::guild::rights::ALL,
                    }],
                    ..Default::default()
                }],
                ..base.guild
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    wow_world_messages::vanilla::CMSG_GUILD_QUERY { guild_id: 7 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(response) => {
            assert_eq!(response.id, 7);
            assert_eq!(response.name, "Tracer Guild");
            assert_eq!(response.rank_names[0], "Guild Master");
            assert_eq!(response.rank_names[1], "");
        }
        other => panic!("expected SMSG_GUILD_QUERY_RESPONSE, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

/// Write one request that answers its actor nothing, then a sentinel request with a guaranteed
/// reply, and block for that reply. This is more than pacing: `enter_world` drains the world entry
/// batch, which ends before the Guild MOTD event `guild_world_entry` sends a fresh-login
/// Guild member (see its own doc comment), so one packet can still be unread in the client's
/// kernel buffer. Reading for a sentinel discards it along the way. Dropping the client with it
/// still queued would close with unread bytes, which the kernel reports to the server as a reset,
/// not a clean EOF (`enter_world`'s doc comment names this exact failure shape). The sentinel is
/// CMSG_PLAYED_TIME, which replies only when the store has a Character row for the caller.
fn sync(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    write: impl FnOnce(&mut UnixStream, &mut EncrypterHalf),
) {
    write(&mut *client, &mut *enc);
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut *client, &mut *enc)
        .unwrap();
    loop {
        if let ServerOpcodeMessage::SMSG_PLAYED_TIME(_) =
            ServerOpcodeMessage::read_encrypted(&mut *client, &mut *dec).unwrap()
        {
            break;
        }
    }
}

/// Character 1 as a member of Guild 7 at rank 3.
fn guild_member_store() -> WorldFake {
    {
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            guild: GuildState {
                guild_memberships: vec![codec::GuildMemberView {
                    character_guid: 1,
                    guild_id: 7,
                    rank_id: 3,
                    name: "Warrior".into(),
                    ..Default::default()
                }],
                ..base.guild
            },
            ..base
        }
    }
}

/// The recorded operations, in call order, without their Shard.
fn recorded(store: &WorldFake) -> Vec<String> {
    store
        .topology
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, what)| what.clone())
        .collect()
}

fn position(calls: &[String], what: &str) -> usize {
    calls
        .iter()
        .position(|call| call == what)
        .unwrap_or_else(|| panic!("`{what}` never ran: {calls:?}"))
}

#[test]
fn a_member_enters_the_world_with_its_guild_on_the_self_create_and_signs_on() {
    let store = std::sync::Arc::new(guild_member_store());
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let mut guild = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) = message {
            if let [Object::CreateObject2 {
                mask2: wow_world_messages::vanilla::UpdateMask::Player(player),
                ..
            }] = update.objects.as_slice()
            {
                guild = Some((player.player_guildid(), player.player_guildrank()));
            }
        }
    }
    assert_eq!(guild, Some((Some(7), Some(3))));
    drop(client);
    server.join().unwrap();
    assert!(recorded(&store).contains(&"guild_op:SignOn".to_string()));
}

#[test]
fn every_membership_opcode_reaches_its_durable_request() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            guild: GuildState {
                guild_memberships: vec![codec::GuildMemberView {
                    character_guid: 1,
                    guild_id: 7,
                    rank_id: 3,
                    name: "Warrior".into(),
                    ..Default::default()
                }],
                guilds: vec![codec::GuildView {
                    guild_id: 7,
                    name: "Tracer Guild".into(),
                    leader_guid: 1,
                    ..Default::default()
                }],
                ..base.guild
            },
            characters: vec![
                codec::CharacterView {
                    guid: 1,
                    name: "Warrior".into(),
                    ..Default::default()
                },
                codec::CharacterView {
                    guid: 2,
                    name: "Target".into(),
                    ..Default::default()
                },
            ],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_INVITE {
            invited_player: "Target".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_ACCEPT {}.write_encrypted_client(c, e).unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_DECLINE {}.write_encrypted_client(c, e).unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_REMOVE {
            player_name: "Target".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_PROMOTE {
            player_name: "Target".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_DEMOTE {
            player_name: "Target".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_LEADER {
            new_guild_leader_name: "Target".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_LEAVE {}.write_encrypted_client(c, e).unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_GUILD_DISBAND {}.write_encrypted_client(c, e).unwrap();
    });

    drop(client);
    server.join().unwrap();
    let calls = recorded(&store);
    for op in [
        "guild_op:Invite",
        "guild_op:Accept",
        "guild_op:Decline",
        "guild_op:Remove",
        "guild_op:Promote",
        "guild_op:Demote",
        "guild_op:SetLeader",
        "guild_op:Leave",
        "guild_op:Disband",
    ] {
        assert!(
            calls.contains(&op.to_string()),
            "{op} never reached the Durable Request: {calls:?}"
        );
    }
}

#[test]
fn logout_signs_the_member_off_before_the_account_claim_is_released() {
    let store = std::sync::Arc::new(guild_member_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..2 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    drop(client);
    server.join().unwrap();
    let calls = recorded(&store);
    assert!(
        position(&calls, "guild_op:SignOff") < position(&calls, "logout"),
        "Realm-core refuses a sign-off after the Account Claim closes: {calls:?}"
    );
}

#[test]
fn world_entry_finishes_a_leftover_fee_hold_on_the_home_shard() {
    let store = std::sync::Arc::new({
        let base = guild_member_store();
        WorldFake {
            guild: GuildState {
                guild_fee_hold: std::sync::Mutex::new(Some(crate::world::guild_fee::FeeHold {
                    operation_id: 5_090_401,
                    terms: crate::world::guild_fee::FeeTerms::Emblem(Default::default()),
                })),
                ..base.guild
            },
            ..base
        }
    });
    let (client, _c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    drop(client);
    server.join().unwrap();
    let calls = recorded(&store);
    assert!(
        position(&calls, "player_login") < position(&calls, "guild_fee_decide")
            && position(&calls, "guild_fee_decide") < position(&calls, "guild_fee_finish"),
        "{calls:?}"
    );
    assert_eq!(*store.guild.guild_fee_hold.lock().unwrap(), None);
}

#[test]
fn the_tabard_designer_window_opens_over_the_socket() {
    let store = std::sync::Arc::new(guild_member_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    wow_world_messages::vanilla::MSG_TABARDVENDOR_ACTIVATE {
        guid: Guid::new(5_090_010),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(window) => {
            assert_eq!(window.guid, Guid::new(5_090_010));
        }
        other => panic!("expected MSG_TABARDVENDOR_ACTIVATE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn a_member_who_is_not_the_leader_saves_no_emblem_over_the_socket() {
    let store = std::sync::Arc::new(guild_member_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    wow_world_messages::vanilla::MSG_SAVE_GUILD_EMBLEM_Client {
        vendor: Guid::new(5_090_010),
        emblem_style: 1,
        emblem_color: 2,
        border_style: 3,
        border_color: 4,
        background_color: 5,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(saved) => assert_eq!(
            saved.result,
            wow_world_messages::vanilla::GuildEmblemResult::NotGuildMaster
        ),
        other => panic!("expected MSG_SAVE_GUILD_EMBLEM, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(!recorded(&store).contains(&"guild_fee_hold".to_string()));
}

#[test]
fn the_settings_opcodes_reach_their_dispatch_entries_over_the_socket() {
    let store = std::sync::Arc::new({
        let base = guild_member_store();
        WorldFake {
            guild: GuildState {
                guilds: vec![codec::GuildView {
                    guild_id: 7,
                    name: "Tracer Guild".into(),
                    ranks: vec![codec::GuildRankView {
                        rank_id: 0,
                        name: "Guild Master".into(),
                        rights: lyracore_shared::guild::rights::ALL,
                    }],
                    ..Default::default()
                }],
                ..base.guild
            },
            // The sentinel below is CMSG_PLAYED_TIME, which only replies once `character_by_guid`
            // resolves the caller's row (`char.rs`'s own doc comment); without one here, the reply
            // never comes and the sentinel read blocks until the test's socket timeout fires.
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Warrior".into(),
                ..Default::default()
            }],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_MOTD {
            message_of_the_day: "Assemble!".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_INFO_TEXT {
            guild_info: "About us".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_SET_PUBLIC_NOTE {
            player_name: "Dave".into(),
            note: "reliable".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_SET_OFFICER_NOTE {
            player_name: "Dave".into(),
            note: "watch closely".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_RANK {
            rank_id: 2,
            rights: 0x43,
            rank_name: "Veteran+".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_ADD_RANK {
            rank_name: "Recruit".into(),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        wow_world_messages::vanilla::CMSG_GUILD_DEL_RANK {}
            .write_encrypted_client(c, e)
            .unwrap();
    });

    drop(client);
    server.join().unwrap();
    let calls = recorded(&store);
    for op in [
        "guild_op:SetMotd",
        "guild_op:SetInfo",
        "guild_op:SetPublicNote",
        "guild_op:SetOfficerNote",
        "guild_op:EditRank",
        "guild_op:AddRank",
        "guild_op:DeleteRank",
    ] {
        assert!(
            calls.contains(&op.to_string()),
            "{op} never reached guild_op: {calls:?}"
        );
    }
}

#[test]
fn a_world_port_that_fails_after_sign_on_still_signs_off() {
    let store = std::sync::Arc::new({
        let base = guild_member_store();
        WorldFake {
            session: SessionState {
                // The world-port ack is only actionable after teleport_player removed the old entity.
                entity_in_world: false,
                worldport_login_error: Some("character 1 is stranded on map 36".into()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    while ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_ok() {}
    drop(client);
    assert!(
        server.join().unwrap().is_err(),
        "the failed entry ends the session"
    );
    let calls = recorded(&store);
    assert!(
        position(&calls, "guild_op:SignOn") < position(&calls, "guild_op:SignOff"),
        "{calls:?}"
    );
    assert!(position(&calls, "guild_op:SignOff") < position(&calls, "logout"));
}

#[test]
fn char_create_replies_success_then_name_in_use() {
    // The fake store reports a name already among its characters as in-use, else success.
    let tester = codec::CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            characters: vec![tester],
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    let mk = |name: &str| CMSG_CHAR_CREATE {
        name: name.to_string(),
        race: Race::Human,
        class: Class::Warrior,
        gender: Gender::Male,
        skin_color: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
    };

    // A fresh name → CharCreateSuccess.
    mk("Newbie")
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateSuccess)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    // An existing name → CharCreateNameInUse, and the session stays alive.
    mk("Tester")
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_CREATE(m) => {
            assert_eq!(m.result, WorldResult::CharCreateNameInUse)
        }
        other => panic!("expected SMSG_CHAR_CREATE, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_delete_replies_success_and_dispatches_owned_guid() {
    let tester = codec::CharacterView {
        guid: 5,
        name: "Tester".into(),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            characters: vec![tester],
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE { guid: Guid::new(5) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result, WorldResult::CharDeleteSuccess)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }
    assert_eq!(*store.character.deleted.lock().unwrap(), vec![(7, 5)]);

    drop(client);
    server.join().unwrap();
}

#[test]
fn char_delete_failure_replies_failed_and_keeps_session_alive() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            character: CharacterState {
                delete_outcome: Some(codec::CharDeleteOutcome::Failed),
                ..base.character
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE {
        guid: Guid::new(999),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result, WorldResult::CharDeleteFailed)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }

    // Session should still be alive: CMSG_CHAR_ENUM gets a normal reply, not a dropped socket.
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

/// Guild state on Realm-core for the deleted-Character reconciliation: Leader 5 and member 6 in
/// one Guild, and a Petition owned by 7 with a Signature by 8. Characters 5 and 7 still exist on
/// the `instances` peer; 6 and 8 exist on no World Shard.
fn guild_cleanup_topology() -> std::sync::Arc<WorldFake> {
    guild_cleanup_topology_failing_lookup_for(None)
}

fn guild_cleanup_topology_failing_lookup_for(
    guild_lookup_error_for: Option<u64>,
) -> std::sync::Arc<WorldFake> {
    let instances = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "instances".into(),
            ..Default::default()
        },
        characters: [5, 7]
            .into_iter()
            .map(|guid| codec::CharacterView {
                guid,
                name: format!("Kept{guid}"),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let world = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "world".into(),
            ..Default::default()
        },
        guild: GuildState {
            guild_memberships: [(5, 0), (6, 4)]
                .into_iter()
                .map(|(character_guid, rank_id)| codec::GuildMemberView {
                    character_guid,
                    guild_id: 7,
                    rank_id,
                    ..Default::default()
                })
                .collect(),
            guild_petitions: vec![codec::PetitionView {
                petition_id: 3,
                charter_item_guid: 90,
                owner_guid: 7,
                name: "Boundary Test".into(),
                signers: vec![8],
            }],
            guild_lookup_error_for,
            ..Default::default()
        },
        ..Default::default()
    });
    *world.topology.peers.lock().unwrap() = vec![world.clone(), instances];
    world
}

fn forgotten(store: &WorldFake) -> Vec<String> {
    store
        .topology
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, call)| call.clone())
        .filter(|call| call.starts_with("guild_op:ForgetDeletedCharacter"))
        .collect()
}

fn sweep() -> crate::world::GuildCleanup {
    crate::world::GuildCleanup {
        sweep: true,
        ..Default::default()
    }
}

fn deleted(guids: &[u64]) -> crate::world::GuildCleanup {
    crate::world::GuildCleanup {
        sweep: false,
        deleted: guids.iter().copied().collect(),
    }
}

fn durable_absence_checks(store: &WorldFake) -> usize {
    store
        .character
        .durable_absence_checks
        .load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn only_characters_absent_from_every_world_shard_are_forgotten() {
    let world = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(world.as_ref(), &sweep()).unwrap();
    assert_eq!(
        forgotten(&world),
        [
            "guild_op:ForgetDeletedCharacter:6",
            "guild_op:ForgetDeletedCharacter:8"
        ]
    );
}

/// A Character row in any Shard's cache proves the Character exists, so the pass takes no durable
/// snapshot for it. Only 6 and 8, gone from every cache, pay for the absence check.
#[test]
fn a_character_still_in_a_shard_cache_takes_no_durable_snapshot() {
    let world = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(world.as_ref(), &sweep()).unwrap();
    assert_eq!(durable_absence_checks(&world), 2);

    let kept = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(kept.as_ref(), &deleted(&[5, 7])).unwrap();
    assert_eq!(durable_absence_checks(&kept), 0);
    assert!(forgotten(&kept).is_empty());
}

/// A `game_character` delete owes cleanup for that Character only, and none at all for a
/// Character that no Guild, Petition or Signature names.
#[test]
fn a_single_delete_checks_only_its_own_character() {
    let world = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(world.as_ref(), &deleted(&[8, 40])).unwrap();
    assert_eq!(forgotten(&world), ["guild_op:ForgetDeletedCharacter:8"]);
    assert_eq!(durable_absence_checks(&world), 1);
}

/// A lookup that fails for one deleted Character does not hold up the others. The pass still
/// fails, so the worker keeps the work and retries it.
#[test]
fn a_failed_lookup_still_cleans_the_other_deleted_characters() {
    let world = guild_cleanup_topology_failing_lookup_for(Some(6));
    let error = crate::world::reconcile_deleted_guild_characters(world.as_ref(), &deleted(&[6, 8]))
        .expect_err("the failed lookup is owed a retry");
    assert!(error.to_string().contains("lookup for 6"));
    assert_eq!(forgotten(&world), ["guild_op:ForgetDeletedCharacter:8"]);
}

#[test]
fn an_unavailable_world_shard_defers_every_guild_cleanup() {
    let world = guild_cleanup_topology();
    let incomplete = WorldFake {
        topology: TopologyState {
            shard: "world".into(),
            calls: world.topology.calls.clone(),
            world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
            ..Default::default()
        },
        guild: GuildState {
            guild_memberships: world.guild.guild_memberships.clone(),
            guild_petitions: world.guild.guild_petitions.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = crate::world::reconcile_deleted_guild_characters(&incomplete, &sweep())
        .expect_err("an incomplete Shard set cannot prove a deletion");
    assert!(error.to_string().contains("no healthy Coordinator"));
    assert!(forgotten(&incomplete).is_empty());
}

/// A Guild Leader's delete answers 0x3A, FAILED_GUILD_LEADER in mangos, and never reaches the
/// Home Shard (`cm:CharacterHandler.cpp:540-546`).
#[test]
fn char_delete_of_a_guild_leader_replies_failed_and_deletes_nothing() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            guild: GuildState {
                guild_memberships: vec![codec::GuildMemberView {
                    character_guid: 5,
                    guild_id: 7,
                    rank_id: 0,
                    name: "Tester".into(),
                    ..Default::default()
                }],
                guilds: vec![codec::GuildView {
                    guild_id: 7,
                    name: "Boundary Test".into(),
                    leader_guid: 5,
                    ..Default::default()
                }],
                ..base.guild
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE { guid: Guid::new(5) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result.as_int(), 0x3A)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }
    assert!(store.character.deleted.lock().unwrap().is_empty());

    drop(client);
    server.join().unwrap();
}

/// Human/Warrior entity matching the seed, as the gateway would read it back after
/// `player_login`.
fn warrior_entity() -> codec::EntityView {
    codec::EntityView {
        guid: 1,
        map_id: 0,
        instance_id: 0, // the open world
        zone_id: 12,
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        orientation: 0.0,
        last_move_ms: 0,
        movement_flags: 0,
        run_speed_mult_bp: 10_000,
        type_mask: 0x19,
        entry: 0,
        scale_x: 1.0,
        health: 60,
        max_health: 60,
        power: 0,
        max_power: 1000,
        level: 1,
        faction_template: 1,
        target_guid: 0,
        unit_bytes_0: 1 | (1 << 8) | (1 << 24), // human(1) warrior(1) male(0) rage(1)
        display_id: 49,
        native_display_id: 49,
        unit_flags: 0,
        mount_display_id: 0,
        base_attack_time_ms: 2000,
        dynamic_flags: 0,
        player_bytes: 0,
        player_bytes_2: 0,
        player_bytes_3: 0,
        player_flags: 0,
        xp: 0,
        next_level_xp: 0,
        money: 0,
        unit_bytes_1: 0,
        unit_bytes_2: 0,
        // L1 Human Warrior base attributes (cmangos curve) — non-zero so the CREATE exercises them.
        strength: 23,
        agility: 20,
        stamina: 22,
        intellect: 20,
        spirit: 21,
        sheet_ap_base: 29,
        sheet_ap_mods: 0,
        sheet_dmg_min: 5,
        sheet_dmg_max: 7,
        sheet_ranged_ap: 0,
        sheet_ranged_dmg_min: 0,
        sheet_ranged_dmg_max: 0,
        npc_flags: 0,        // a player is not an NPC
        owner_guid: 0,       // not a summon
        effective_armor: 40, // agility 20 * 2 (base; no gear in the fixture → effective == base)
        magic_resistances: [0; 6],
        // No hearthstone bind recorded for the test entity; fall back to login position.
        home_map: 0,
        home_zone: 0,
        home_x: 0.0,
        home_y: 0.0,
        home_z: 0.0,
        guild_id: 0,
        guild_rank: 0,
    }
}

#[test]
fn login_replays_a_pending_package_system_message() {
    // A Package `on_login` hook emits its System Message INSIDE `player_login` — before the
    // session is in the viewer registry, so the live insert relay has nobody to address. World
    // entry must replay the parked row right after the entry sequence.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                pending_system_messages: vec!["Example Package is active.".to_string()],
                ..base.session
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let replay = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    let ServerOpcodeMessage::SMSG_MESSAGECHAT(actual) = replay else {
        panic!("expected the parked System Message after the entry sequence, got {replay:?}");
    };
    assert_eq!(
        *actual,
        codec::build_gm_system_message("Example Package is active.".to_string())
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn player_login_emits_sequence_then_self_create() {
    // CMSG_PLAYER_LOGIN must yield the full login sequence (in order), then the self
    // CREATE_OBJECT2 at the correct position/guid, then the current zone's weather — the clock
    // (SMSG_LOGIN_SETTIMESPEED) before the sky, so the client has both before it renders.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            weather: WeatherState {
                // Elwynn Forest (the fixture's zone) is raining hard.
                zone_weather: vec![(
                    12,
                    codec::ZoneWeatherView {
                        weather_type: 1,
                        intensity: 0.8,
                    },
                )],
                ..base.weather
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // The 11-message login sequence, the self CREATE_OBJECT2, then the zone's weather.
    let mut tags = Vec::new();
    let mut create_guid = None;
    let mut weather = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        match message {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(m) => {
                tags.push("verify_world");
                assert_eq!(m.map, Map::EasternKingdoms);
                assert!((m.position.x - (-8949.95)).abs() < 0.01);
            }
            ServerOpcodeMessage::SMSG_ACCOUNT_DATA_TIMES(_) => tags.push("account_data_times"),
            ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(_) => tags.push("settimespeed"),
            ServerOpcodeMessage::SMSG_TUTORIAL_FLAGS(_) => tags.push("tutorial_flags"),
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(_) => tags.push("initial_spells"),
            ServerOpcodeMessage::SMSG_SET_PROFICIENCY(m) => tags.push(match m.class.as_int() {
                2 => "weapon_proficiency",
                4 => "armor_proficiency",
                other => panic!("unexpected proficiency item class {other}"),
            }),
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(_) => tags.push("action_buttons"),
            ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(_) => tags.push("init_factions"),
            ServerOpcodeMessage::SMSG_SET_REST_START(_) => tags.push("set_rest_start"),
            ServerOpcodeMessage::SMSG_BINDPOINTUPDATE(_) => tags.push("bindpoint"),
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(m) => {
                tags.push("update_object");
                if let Object::CreateObject2 { guid3, .. } = &m.objects[0] {
                    create_guid = Some(guid3.guid());
                } else {
                    panic!("expected CreateObject2 in self-spawn");
                }
            }
            ServerOpcodeMessage::SMSG_WEATHER(m) => {
                tags.push("weather");
                weather = Some(*m);
            }
            other => panic!("unexpected message in login sequence: {other}"),
        }
    }

    assert_eq!(
        tags,
        vec![
            "verify_world",
            "account_data_times",
            "settimespeed",
            "tutorial_flags",
            "initial_spells",
            "weapon_proficiency",
            "armor_proficiency",
            "action_buttons",
            "init_factions",
            "set_rest_start",
            "bindpoint",
            "update_object",
            "weather",
        ],
    );
    assert_eq!(create_guid, Some(1));
    // Elwynn's stored rain, synchronized rather than faded in: an arriving client must not spend
    // a transition rendering the sky it happened to have last.
    let weather = weather.expect("world entry sends the zone's weather");
    assert_eq!(weather.weather_type, WeatherType::Rain);
    assert_eq!(weather.grade, 0.8);
    assert_eq!(weather.sound_id, 8535);
    assert_eq!(weather.change, WeatherChangeType::Instant);

    drop(client);
    server.join().unwrap();
}

/// Drive one login to completion and hand back the world-entry weather packet.
fn world_entry_weather(store: std::sync::Arc<WorldFake>) -> SMSG_WEATHER {
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let mut weather = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_WEATHER(m) = message {
            weather = Some(*m);
        }
    }
    drop(client);
    server.join().unwrap();
    weather.expect("every world entry sends the zone's weather")
}

/// Story 10: a zone the Module has no weather row for is fine weather. World entry still sends the
/// packet — a client told nothing keeps rendering whatever sky it arrived with.
#[test]
fn world_entry_into_a_zone_with_no_weather_row_sends_fine_weather() {
    let weather = world_entry_weather(std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            ..base
        }
    }));
    assert_eq!(weather.weather_type, WeatherType::Fine);
    assert_eq!(weather.grade, 0.0);
    assert_eq!(weather.sound_id, 0);
    assert_eq!(weather.change, WeatherChangeType::Instant);
}

/// A Store that cannot answer the weather question is a degraded sky, never a failed login: the
/// session completes world entry and the player lands under clear skies.
#[test]
fn a_weather_read_failure_still_completes_world_entry() {
    let weather = world_entry_weather(std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            weather: WeatherState {
                weather_error: Some("shard cache unavailable".into()),
                ..base.weather
            },
            ..base
        }
    }));
    assert_eq!(weather.weather_type, WeatherType::Fine);
    assert_eq!(weather.grade, 0.0);
}

/// Drive one World Session through the handshake and world entry, and hand back the client end
/// parked on the first packet that comes AFTER world entry.
fn world_session_in_world(
    store: std::sync::Arc<WorldFake>,
    character_guid: u64,
) -> (UnixStream, DecrypterHalf, std::thread::JoinHandle<()>) {
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(character_guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    (client, c_dec, server)
}

/// One `game_zone_weather` row, as the Module writes it and the relay reads it.
fn zone_weather_row(
    zone_id: u32,
    weather_type: u8,
    intensity: f32,
) -> crate::stdb::bindings::ZoneWeather {
    crate::stdb::bindings::ZoneWeather {
        zone_id,
        weather_type,
        intensity,
        changed_at_micros: 0,
    }
}

/// Stories 5 and 6 end to end, over the real cipher: two World Sessions on real socket pairs share
/// one shard's relay, one standing in Elwynn Forest and one in Westfall. Forced Elwynn rain must
/// decode as valid rain on the Elwynn client and must not reach the Westfall client at all.
///
/// Both characters stand on the SAME position, so nothing but the zone can separate them — the
/// spatial index has no say in weather.
///
/// Westfall's own snow is a sentinel rather than a bare timeout. A session's writer is FIFO, so an
/// Elwynn row that leaked into Westfall's routing would arrive on that socket BEFORE the snow; the
/// assertion therefore fails deterministically if the zone filter goes away, instead of resting on
/// a socket that happened to stay quiet. The bounded silence afterwards closes the mirror case:
/// Westfall's snow must not reach Elwynn either.
#[test]
fn forced_elwynn_rain_reaches_only_the_client_standing_in_elwynn() {
    const ELWYNN: u32 = 12;
    const WESTFALL: u32 = 40;
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));

    let mut westfall_entity = warrior_entity();
    westfall_entity.guid = 2;
    westfall_entity.zone_id = WESTFALL;
    let session_store = |entity: codec::EntityView| {
        std::sync::Arc::new({
            let base = tester_store(7);
            WorldFake {
                session: SessionState {
                    login_entity: Some(entity),
                    relay_view: Some(view.clone()),
                    ..base.session
                },
                ..base
            }
        })
    };

    let (mut elwynn_client, mut elwynn_dec, elwynn_server) =
        world_session_in_world(session_store(warrior_entity()), 1);
    let (mut westfall_client, mut westfall_dec, westfall_server) =
        world_session_in_world(session_store(westfall_entity), 2);

    // Elwynn is forced to heavy rain; Westfall drifts into light snow of its own.
    crate::stdb::world_view::relay_zone_weather(&view, 0, &zone_weather_row(ELWYNN, 1, 0.75));
    crate::stdb::world_view::relay_zone_weather(&view, 0, &zone_weather_row(WESTFALL, 2, 0.30));

    match ServerOpcodeMessage::read_encrypted(&mut elwynn_client, &mut elwynn_dec).unwrap() {
        ServerOpcodeMessage::SMSG_WEATHER(m) => {
            assert_eq!(m.weather_type, WeatherType::Rain);
            assert_eq!(m.grade, 0.75);
            assert_eq!(m.sound_id, 8535, "0.75 is the heavy rain band");
            assert_eq!(m.change, WeatherChangeType::Smooth);
        }
        other => panic!("the Elwynn client must receive Elwynn's rain, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut westfall_client, &mut westfall_dec).unwrap() {
        ServerOpcodeMessage::SMSG_WEATHER(m) => {
            assert_eq!(
                m.weather_type,
                WeatherType::Snow,
                "the first weather the Westfall client sees must be Westfall's own — rain here \
                 means Elwynn's row leaked across the zone boundary"
            );
            assert_eq!(m.grade, 0.30);
            assert_eq!(m.sound_id, 8536, "0.30 is the light snow band");
        }
        other => panic!("the Westfall client must receive Westfall's snow, got {other}"),
    }

    // Neither zone's sky is echoed to the other. A short deadline is enough: both sockets have
    // already delivered a packet enqueued after the one being tested for.
    let quiet = std::time::Duration::from_millis(250);
    for (name, client, dec) in [
        ("Elwynn", &mut elwynn_client, &mut elwynn_dec),
        ("Westfall", &mut westfall_client, &mut westfall_dec),
    ] {
        client.set_read_timeout(Some(quiet)).unwrap();
        assert!(
            ServerOpcodeMessage::read_encrypted(client, dec).is_err(),
            "the {name} client must receive exactly one weather packet, its own zone's"
        );
    }

    drop(elwynn_client);
    drop(westfall_client);
    elwynn_server.join().unwrap();
    westfall_server.join().unwrap();
}

#[test]
fn worldport_ack_reenters_with_fresh_subscription_and_empty_loot_state() {
    // MSG_MOVE_WORLDPORT_ACK must re-run the SAME enter_world path as
    // CMSG_PLAYER_LOGIN — rebuilding the entity (now on the NEW map the module's teleport_player
    // durably wrote to the character row) and re-subscribing with a FRESH `created` dedup set — a
    // reused dedup set from the old map would suppress the initial sweep of pre-existing entities
    // through the CREATE path, leaving the new map looking empty until something moved or spawned —
    // a stale created-set is exactly what would leave entities invisible on cross-map arrival.
    let mut ported = warrior_entity();
    ported.map_id = 1; // Kalimdor — simulates teleport_player's durable cross-map write
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                // A real cross-map teleport has despawned the old-map entity before its ack arrives.
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                worldport_entity: Some(ported),
                ..base.session
            },
            loot_window: LootWindowState {
                corpse_money: 25,
                ..base.loot_window
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Drain world entry (map 0 — not the point of this test).
    drain_world_entry(&mut client, &mut c_dec);

    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);

    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // Re-entry skips SMSG_LOGIN_VERIFY_WORLD because resending it reloads the current map.
    let mut create_guid = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        match message {
            ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_) => {
                panic!(
                    "the re-entry sequence must NOT resend SMSG_LOGIN_VERIFY_WORLD — it makes the \
                     client reload the map it just loaded (a second loading screen)"
                );
            }
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(m) => {
                if let Object::CreateObject2 { guid3, .. } = &m.objects[0] {
                    create_guid = Some(guid3.guid());
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        create_guid,
        Some(1),
        "the self entity is re-created (despawn+rebuild), not a stale row"
    );

    // subscribe_player_events must have fired TWICE: once at login (map 0), once at the world-port
    // re-entry (map 1) — a fresh `created` set is built for the new map each time, never reused.
    let calls = store.session.subscribed.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "subscribe_player_events must run again on WORLDPORT_ACK"
    );
    assert_eq!(calls[0].1, 0, "the first subscription is for the login map");
    assert_eq!(
        calls[1].1, 1,
        "the second subscription is for the NEW (post-teleport) map"
    );
    assert!(
        (calls[1].2 - 100.0).abs() < 0.01,
        "the re-entry must use the NEW position (verify-world no longer carries it — the \
         subscription placement is the observable)"
    );

    // The old map's open window cannot authorize a targetless request after re-entry.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "world-port re-entry must start with no open loot target"
    );
    // The login ends the Away Status; the world-port rides the reducer that keeps it.
    assert_eq!(
        store.session.login_entries.lock().unwrap().as_slice(),
        [codec::WorldEntry::FreshLogin, codec::WorldEntry::WorldPort]
    );
}

#[test]
fn worldport_removes_the_source_viewer_before_routing_and_registers_a_replacement() {
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));
    let mut ported = warrior_entity();
    ported.map_id = 1;
    ported.x = 100.0;
    ported.y = 200.0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                worldport_entity: Some(ported),
                relay_view: Some(view.clone()),
                ..base.session
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    let source_session = view
        .viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
        .expect("login registers the source viewer")
        .session;

    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);

    assert_eq!(
        store
            .topology
            .viewer_present_at_settle
            .lock()
            .unwrap()
            .as_slice(),
        &[false, false],
        "neither initial routing nor world-port routing may see a registered source viewer; the \
         second observation is the transfer-cascade safety boundary"
    );
    let destination_session = view
        .viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
        .expect("destination world entry registers its viewer")
        .session;
    assert_ne!(
        destination_session, source_session,
        "world-port must replace, not reuse, the source viewer lifetime"
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_initialize_factions_carries_persisted_standing_at_its_reputation_index() {
    // A persisted `game_player_reputation` row must land in the login
    // SMSG_INITIALIZE_FACTIONS at its STORED reputation_index slot (0..63), never faction_id — the
    // guardrail that also gates the live SET_FACTION_STANDING relay (McBride ERROR).
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            character: CharacterState {
                // Stormwind's rep-index is 19 (Faction.dbc ReputationListID), NOT its faction id (72) —
                // exercising the exact index/id distinction the guardrail protects.
                reputations: vec![(19, 3175, false)],
                ..base.character
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // Drain the whole world entry so the server side doesn't see a broken pipe.
    let mut factions = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(m) = message {
            factions = Some(m.factions);
        }
    }
    let factions = factions.expect("SMSG_INITIALIZE_FACTIONS not found in login sequence");
    assert_eq!(factions.len(), 64);
    assert_eq!(
        factions[19].standing, 3175,
        "slot 19 (the stored reputation_index) must carry the standing"
    );
    // Stormwind's faction_id (72) is itself past the 64-slot array — proof that indexing by faction_id
    // (the McBride bug) would panic/crash here rather than silently landing on the wrong slot.
    for (i, f) in factions.iter().enumerate() {
        if i != 19 {
            assert_eq!(f.standing, 0, "slot {i} should remain the Neutral/0 stub");
        }
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_with_resident_items_and_reputation_emits_no_gain_feedback() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            cast: CastState {
                player_items_fixture: vec![codec::ItemInstanceView {
                    guid: 0x4000_0000_0000_0001,
                    entry: 25,
                    owner_guid: 1,
                    slot: 23,
                    stack_count: 1,
                    durability: 20,
                    max_durability: 20,
                    container_slots: 0,
                    random_property_id: 0,
                    random_property_enchant_ids: [0; 3],
                    item_text_id: 0,
                    enchantment: 0,
                }],
                ..base.cast
            },
            character: CharacterState {
                reputations: vec![(19, 3175, false)],
                ..base.character
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // World entry, which carries the resident item's CREATE. The item and standing are snapshots
    // in those frames, not live insert callbacks, so neither feedback packet is lawful.
    for message in drain_world_entry(&mut client, &mut c_dec) {
        assert!(
            !matches!(
                message,
                ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(_)
                    | ServerOpcodeMessage::SMSG_SET_FACTION_STANDING(_)
            ),
            "resident login state must not look like a newly gained item or reputation change"
        );
    }
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(50)))
        .unwrap();
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "login queued an unexpected post-snapshot feedback frame"
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn login_fills_a_resident_suffix_items_enchantment_slots_after_the_entry_batch() {
    let item_guid = 0x4000_0000_0000_0001;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            cast: CastState {
                player_items_fixture: vec![codec::ItemInstanceView {
                    guid: item_guid,
                    entry: 25,
                    owner_guid: 1,
                    slot: 23,
                    stack_count: 1,
                    random_property_id: 22,
                    random_property_enchant_ids: [73, 0, 0],
                    ..Default::default()
                }],
                ..base.cast
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // World entry, which carries the item's CREATE, then the raw update gtker's typed reader
    // rejects.
    drain_world_entry(&mut client, &mut c_dec);
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, 0x00A9);
    let updates = lyracore_shared::values_mask::parse_values_updates(&body);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].guid, item_guid);
    assert_eq!(updates[0].fields, vec![(31, 73)]);

    drop(client);
    server.join().unwrap();
}

#[test]
fn inbound_movement_is_recorded_under_its_opcode() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: true,
                ..base.session
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 12345,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    MSG_MOVE_HEARTBEAT_Client { info }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client); // server reads the heartbeat, then EOF
    server.join().unwrap();

    let moves = store.session.moves.lock().unwrap();
    assert_eq!(moves.len(), 1);
    let (opcode, x, _, _, o, t) = moves[0];
    assert_eq!(
        opcode,
        lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT
    );
    assert!((x - (-8950.0)).abs() < 0.01);
    assert!((o - 1.5).abs() < 0.001);
    assert_eq!(t, 12345);
}

#[test]
fn a_movement_packet_for_a_despawned_entity_never_kills_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let calls: ShardCallLog = Default::default();
    let entity_presence = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            topology: TopologyState {
                calls: calls.clone(),
                ..base.topology
            },
            session: SessionState {
                entity_presence: Some(entity_presence.clone()),
                ..base.session
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));

    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let beat = |t: u32| MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: t,
            position: Vector3d {
                x: -8950.0,
                y: -130.0,
                z: 83.0,
            },
            orientation: 1.5,
            fall_time: 0.0,
        },
    };
    // The first packet is a transfer tail. Once the coordinator cache sees the entity again, the
    // next state transition must resume normal batched submission.
    beat(1)
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..100 {
        if store
            .session
            .entity_presence_checks
            .load(std::sync::atomic::Ordering::SeqCst)
            != 0
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_ne!(
        store
            .session
            .entity_presence_checks
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the encrypted movement packet must reach the coordinator presence gate"
    );
    entity_presence.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_START_FORWARD_Client { info: beat(2).info }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);

    server.join().unwrap().expect(
        "a movement packet for a despawned entity must be DROPPED, not session-fatal — \
                 closing the socket here strands the client on a loading screen with no error",
    );
    assert_eq!(
        calls
.lock()
.unwrap()
.iter()
.filter(|(_, c)| c == "movement_update")
.count(),
        1,
        "the transfer-tail packet must not enter the batch, while movement resumes when presence returns"
    );
}

#[test]
fn a_reappearing_entity_resets_the_movement_desync_tolerance() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let present = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_presence: Some(present.clone()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = |t| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    let send_tail = |start: u32, end: u32, client: &mut UnixStream, enc: &mut EncrypterHalf| {
        for t in start..end {
            let sent = if t % 2 == 0 {
                MSG_MOVE_START_FORWARD_Client { info: info(t) }
                    .write_encrypted_client(&mut *client, enc)
            } else {
                MSG_MOVE_STOP_Client { info: info(t) }.write_encrypted_client(&mut *client, enc)
            };
            sent.unwrap();
        }
    };
    let wait_for_checks = |n| {
        for _ in 0..100 {
            if store
                .session
                .entity_presence_checks
                .load(std::sync::atomic::Ordering::SeqCst)
                >= n
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the gateway did not check entity presence {n} times");
    };

    send_tail(0, MOVE_DESYNC_TOLERANCE, &mut client, &mut c_enc);
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize);
    present.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_START_FORWARD_Client {
        info: info(MOVE_DESYNC_TOLERANCE),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize + 1);
    present.store(false, std::sync::atomic::Ordering::SeqCst);
    send_tail(
        MOVE_DESYNC_TOLERANCE + 1,
        MOVE_DESYNC_TOLERANCE * 2 + 1,
        &mut client,
        &mut c_enc,
    );
    wait_for_checks(MOVE_DESYNC_TOLERANCE as usize * 2 + 1);
    present.store(true, std::sync::atomic::Ordering::SeqCst);
    MSG_MOVE_STOP_Client {
        info: info(MOVE_DESYNC_TOLERANCE * 2 + 1),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server
        .join()
        .unwrap()
        .expect("a restored entity must reset the tolerance before a later transfer tail arrives");
    assert_eq!(
        store.session.moves.lock().unwrap().len(),
        2,
        "only movements sent while the entity was present may reach the shared batch"
    );
}

#[test]
fn a_movement_failure_that_is_not_a_desync_is_still_session_fatal() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                entity_in_world: true,
                movement_error: Some("timed out after 10s".into()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    MSG_MOVE_HEARTBEAT_Client {
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 1,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        },
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a non-desync movement failure must still end the session");
    assert!(format!("{err:#}").contains("timed out"), "{err:#}");
}

#[test]
fn a_movement_desync_that_never_heals_still_ends_the_session() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_START_FORWARD_Client, MSG_MOVE_STOP_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                // The entity is gone and is NEVER coming back — not a teleport tail, a real desync.
                entity_in_world: false,
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);
    let info = |t: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: t,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.0,
        },
        orientation: 1.5,
        fall_time: 0.0,
    };
    // State transitions, so every one forwards immediately (never coalesced). Far more than a
    // cross-map port's in-flight tail, and spread over more time than any loading screen's worth of
    // queued packets — a session still serving these has stopped detecting desyncs altogether.
    // Writes are best-effort: the socket SHOULD close partway through, which is the whole point.
    for i in 0..200u32 {
        let ok = if i % 2 == 0 {
            MSG_MOVE_START_FORWARD_Client { info: info(i) }
                .write_encrypted_client(&mut client, &mut c_enc)
                .is_ok()
        } else {
            MSG_MOVE_STOP_Client { info: info(i) }
                .write_encrypted_client(&mut client, &mut c_enc)
                .is_ok()
        };
        if !ok {
            break;
        }
    }
    drop(client);
    let err = server.join().unwrap().expect_err(
        "a movement desync that never heals must STILL end the session: tolerating the tail of a \
         cross-map port is one thing, serving a permanently desynced client a frozen world forever \
         is the hang a stuck cross-map transfer causes, with no loading screen to blame it on",
    );
    assert!(format!("{err:#}").contains("not in world"), "{err:#}");
}

#[test]
fn a_world_port_whose_transfer_cannot_be_driven_aborts_the_clients_loading_screen() {
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));
    let xdb = FakeShardDb::with_character(
        1,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                // The world-port ack is only actionable after teleport_player removed the old entity.
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                relay_view: Some(view.clone()),
                ..base.session
            },
            characters: vec![],
            transfer: TransferState {
                xdb: Some(xdb),
                ..base.transfer
            },
            topology: TopologyState {
                settle_error: Some("instances shard unreachable".into()),
                settle_ok_calls: 1, // the LOGIN routes fine; the world-port's settle is the one that fails
                ..base.topology
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    // The client finished loading the dungeon map and acks — this is where the transfer runs.
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    let aborted = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).expect(
        "the client must receive SMSG_TRANSFER_ABORTED — silence here is the loading-screen hang this fixes",
    );
    match aborted {
        ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(m) => {
            assert_eq!(
                m.map,
                Map::Deadmines,
                "the abort must name the map the client is loading"
            );
        }
        other => panic!("expected SMSG_TRANSFER_ABORTED, got {other}"),
    }
    drop(client);
    let err = server.join().unwrap().expect_err(
        "a half-driven transfer must still end the session rather than enter the world",
    );
    assert!(
        format!("{err:#}").contains("instances shard unreachable"),
        "{err:#}"
    );
    assert!(
        view.viewer_of_owner(crate::stdb::world_view::OwnerGuid(1))
            .is_none(),
        "a failed transfer terminates the session and must not restore its source viewer"
    );
    assert!(
        store
            .session
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "early viewer removal keeps InWorld intact so the existing abort teardown still logs out"
    );
}

#[test]
fn a_world_port_whose_world_entry_fails_also_aborts_the_clients_loading_screen() {
    let xdb = FakeShardDb::with_character(
        1,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                // The world-port ack is only actionable after teleport_player removed the old entity.
                entity_in_world: false,
                login_entity: Some(warrior_entity()),
                // Routing succeeds; the world entry on the far side is what fails.
                worldport_login_error: Some("character 1 is stranded on map 36".into()),
                ..base.session
            },
            characters: vec![],
            transfer: TransferState {
                xdb: Some(xdb),
                ..base.transfer
            },
            ..base
        }
    });

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // Drain whatever the (partial) re-entry emitted and require an abort somewhere in it — the
    // client must not be left loading. `read_encrypted` returns Err once the socket closes.
    let mut aborted = None;
    while let Ok(msg) = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(m) = msg {
            aborted = Some(m);
            break;
        }
    }
    let m = aborted.expect(
        "a world-port whose ENTRY fails must still abort the client's loading screen — silence \
         here is the infinite loading bar this abort exists to kill",
    );
    assert_eq!(
        m.map,
        Map::Deadmines,
        "the abort must name the map the client is loading"
    );
    drop(client);
    let err = server
        .join()
        .unwrap()
        .expect_err("a half-entered world-port must still end the session");
    assert!(format!("{err:#}").contains("stranded"), "{err:#}");
}

#[test]
fn movement_state_changes_forward_immediately_and_in_order_never_delayed() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MSG_MOVE_START_TURN_LEFT_Client,
        MSG_MOVE_STOP_Client, MovementInfo, MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);

    let mk = |flags: u32, orientation: f32, x: f32, timestamp: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::new(flags, None, None, None, None),
        timestamp,
        position: Vector3d { x, y: 0.0, z: 0.0 },
        orientation,
        fall_time: 0.0,
    };
    const RUN: u32 = 0x1;
    const TURN: u32 = 0x1 | 0x10;
    const STOPPED: u32 = 0x0;

    MSG_MOVE_START_FORWARD_Client {
        info: mk(RUN, 0.0, 0.0, 100),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Same flags + heading as the run-start -> a pure heartbeat -> held pending (nowhere near the
    // 150ms window in real wall-clock terms, since these all write back-to-back with no sleep).
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 5.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A second same-vector heartbeat SUPERSEDES the first (rule 3: the pending slot IS the drop
    // mechanism) — x=5.0 must never reach the module.
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(RUN, 0.0, 10.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A turn: different flags -> a STATE CHANGE. Must flush the pending x=10.0 heartbeat FIRST,
    // then forward the turn itself, both undelayed.
    MSG_MOVE_START_TURN_LEFT_Client {
        info: mk(TURN, 5.0, 10.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // A stop: another STATE CHANGE, no pending left to flush — forwards immediately alone.
    MSG_MOVE_STOP_Client {
        info: mk(STOPPED, 5.0, 10.0, 500),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    drop(client);
    server.join().unwrap();

    let moves = store.session.moves.lock().unwrap();
    let opcodes: Vec<u32> = moves.iter().map(|(op, ..)| *op).collect();
    assert_eq!(
        opcodes,
        vec![
            lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD,
            lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT, // the FLUSHED x=10.0 heartbeat
            lyracore_shared::opcodes::movement::MSG_MOVE_START_TURN_LEFT,
            lyracore_shared::opcodes::movement::MSG_MOVE_STOP,
        ],
        "run-start, [flushed heartbeat], turn, stop — in that exact order; the turn and stop must \
         never be held, and the superseded x=5.0 intermediate must never appear at all"
    );
    // The flushed heartbeat carries the LATEST pending position (x=10.0), not the dropped x=5.0 one.
    assert!(
        (moves[1].1 - 10.0).abs() < 0.01,
        "flushed heartbeat must carry the superseding x=10.0, not the dropped x=5.0"
    );
    assert_eq!(
        moves.len(),
        4,
        "exactly one heartbeat survives coalescing out of the two sent"
    );
}

#[test]
fn non_movement_opcode_flushes_pending_heartbeat_before_being_handled() {
    use wow_world_messages::vanilla::{
        MSG_MOVE_HEARTBEAT_Client, MSG_MOVE_START_FORWARD_Client, MovementInfo,
        MovementInfo_MovementFlags, Vector3d,
    };

    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    let mk = |x: f32, timestamp: u32| MovementInfo {
        flags: MovementInfo_MovementFlags::new(0x1, None, None, None, None),
        timestamp,
        position: Vector3d { x, y: 0.0, z: 0.0 },
        orientation: 0.0,
        fall_time: 0.0,
    };

    MSG_MOVE_START_FORWARD_Client { info: mk(0.0, 100) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Three same-vector heartbeats, sent back-to-back (well inside the 150ms window): each
    // supersedes the last in the pending slot, so only the FINAL one (x=30.0) may ever ship.
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(10.0, 200),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(20.0, 300),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    MSG_MOVE_HEARTBEAT_Client {
        info: mk(30.0, 400),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // the query itself was still answered normally
        other => panic!("expected SMSG_QUESTGIVER_STATUS, got {other}"),
    }
    // Safe to inspect now: the reply we just read could only have been produced AFTER the query's
    // dispatch ran (flush, then the query handler) on the single-threaded reader/dispatch loop.
    {
        let moves = store.session.moves.lock().unwrap();
        assert_eq!(
            moves.len(),
            2,
            "baseline + exactly ONE flushed heartbeat — if coalescing weren't happening, all 3 \
             heartbeats would have forwarded individually (4 total), not 2"
        );
        assert!(
            (moves[1].1 - 30.0).abs() < 0.01,
            "the flushed heartbeat must carry the LATEST superseding position, not an earlier dropped one"
        );
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn desync_error_classifies_entity_missing_as_fatal_but_not_transient() {
    // The module's `entity_by_owner` failures (the player's entity is gone — a desync, e.g. a
    // schema-change publish dropped the gateway's read subscription) must be session-FATAL so the
    // handler forces a CLEAN disconnect instead of leaving a silent zombie (can't attack / can't logout).
    assert!(is_desync_error(&anyhow!("attacker not in world")));
    assert!(is_desync_error(&anyhow!("caster not in world")));
    assert!(is_desync_error(&anyhow!("no live entity for guid 5")));
    assert!(is_desync_error(&anyhow!("Player NOT IN WORLD"))); // case-insensitive
                                                               // TRANSIENT per-action failures are NOT desync — they stay swallowed (the player keeps playing,
                                                               // never disconnected). A false positive here would drop players on every dead-target swing.
    assert!(!is_desync_error(&anyhow!("target is dead")));
    assert!(!is_desync_error(&anyhow!(
        "target out of range (35.0 yd > 30 yd)"
    )));
    assert!(!is_desync_error(&anyhow!(
        "not enough power: have 0, need 30"
    )));
    assert!(!is_desync_error(&anyhow!(
        "spell not ready (global cooldown)"
    )));
    assert!(!is_desync_error(&anyhow!("cannot attack self")));
}

// ===========================================================================================
//  Quest traffic over the world session. Which screen a quest opens, and which durable request it
//  makes, is decided and proved at the `dispatch_quest_action` seam (`handlers/quest.rs`). What is
//  left here is only what the seam cannot see: that a quest opcode reaches the seam through the
//  full handshake + login + cipher, and that the bodies it answers with survive the encrypted
//  frame. Five tests carry that contract, each naming it in its own comment.
// ===========================================================================================

/// A store configured for an in-world TESTER (account 7, char guid 1) with a login entity — the
/// base fixture every socket test that needs a logged-in player builds on, quest or not.
fn quest_store() -> WorldFake {
    {
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            ..base
        }
    }
}

/// A minimal quest detail (everything zero/empty but the id + title) — enough for the build_* codecs.
fn detail_view(quest_id: u32, title: &str) -> codec::QuestDetailView {
    codec::QuestDetailView {
        quest_id,
        quest_level: 1,
        zone_or_sort: 12,
        title: title.into(),
        details: String::new(),
        objectives_text: String::new(),
        offer_reward_text: String::new(),
        request_items_text: String::new(),
        money_reward: 0,
        reward_xp: 0,
        next_quest_id: 0,
        max_level_money_reward: 0,
        rewards: Vec::new(),
        choice_rewards: Vec::new(),
        objectives: Vec::new(),
    }
}

/// One giver↔quest eval with the four booleans the menu/status + complete-branch read.
fn eval(quest_id: u32, role: u8, active: bool, complete: bool) -> codec::GiverQuestEval {
    codec::GiverQuestEval {
        quest_id,
        title: "Q".into(),
        level: 1,
        role,
        startable: role == codec::ROLE_START,
        active,
        complete,
    }
}

/// Spin up a world session over a socket pair, handshake as TESTER, enter the world as `guid`, and
/// drain the world entry batch, then the quest-log update when the player has quests. Draining one
/// message too few manifests as the CLIENT closing with unread bytes still queued — which the kernel
/// reports back to the SERVER thread's next read as ECONNRESET, not a clean EOF.
/// Returns the client socket + encrypted halves + the server join handle for the test to drive.
fn enter_world(
    store: std::sync::Arc<WorldFake>,
    guid: u64,
) -> (
    UnixStream,
    EncrypterHalf,
    DecrypterHalf,
    std::thread::JoinHandle<()>,
) {
    let (mut client, server_end) = world_session_socket_pair();
    // The login sequence ends with the quest-log VALUES packet IFF the player has quests (mirrors
    // `send_quest_log`'s skip-when-empty). Checked before `store` is moved into the server thread.
    let has_quest_log = !store.quest.quest_log_slots.is_empty();
    let server_store = store;
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    if has_quest_log {
        // The quest-log packet is a PARTIAL VALUES update with OBJECT_FIELD_TYPE stripped (so the real
        // 5875 client doesn't crash — see the health-VALUES note). gtker's DECODER rejects that ("Missing
        // object TYPE"), but the frame bytes are consumed, so drain it tolerantly rather than unwrap.
        let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    }
    (client, c_enc, c_dec, server)
}

/// A restrictive imported equipment template. The query response is cached by the client and is
/// shared by inventory, vendor, and quest-reward screens, so one socket test pins that definition.
fn human_warrior_sword_template() -> codec::ItemTemplateView {
    codec::ItemTemplateView {
        entry: 910_261,
        class: 2,
        subclass: 7,
        name: "Quartermaster's Practice Sword".into(),
        display_id: 1542,
        quality: 2,
        inventory_type: 13,
        item_level: 20,
        required_level: 15,
        max_durability: 40,
        buy_price: 4_000,
        sell_price: 800,
        max_stack: 1,
        damage_min: 9.0,
        damage_max: 17.0,
        delay_ms: 2_400,
        required_skill: 43, // Swords
        required_skill_rank: 150,
        required_reputation_faction: 72,
        required_reputation_rank: 5,
        allowed_class: 0x01, // Warrior
        allowed_race: 0x01,  // Human
        ..Default::default()
    }
}

#[test]
fn item_query_preserves_imported_eligibility_through_the_encrypted_world_session() {
    let template = human_warrior_sword_template();
    let mut fixture = quest_store();
    fixture.cast.item_templates = vec![template.clone()];
    let store = std::sync::Arc::new(fixture);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    CMSG_ITEM_QUERY_SINGLE {
        item: template.entry,
        guid: Guid::new(0),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let found = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(reply) => {
            assert_eq!(reply.item, template.entry);
            reply
                .found
                .expect("the imported template must reach the item query reply")
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    };

    assert_eq!(found.allowed_class.as_int(), template.allowed_class);
    assert_eq!(found.allowed_race.as_int(), template.allowed_race);
    assert_eq!(
        u32::from(found.required_skill.as_int()),
        template.required_skill
    );
    assert_eq!(found.required_skill_rank, template.required_skill_rank);
    assert_eq!(
        u32::from(found.required_faction.as_int()),
        template.required_reputation_faction
    );
    assert_eq!(
        found.required_faction_rank,
        template.required_reputation_rank
    );

    // These are client-local eligibility terms, deliberately limited to the fields this cached
    // definition contains. They prove the fixture is restrictive rather than an all-bits mask.
    let client_can_use = |class: u32, race: u32, skill: Option<(u32, u32)>| {
        found.allowed_class.as_int() & class != 0
            && found.allowed_race.as_int() & race != 0
            && skill.is_some_and(|(id, rank)| {
                id == u32::from(found.required_skill.as_int()) && rank >= found.required_skill_rank
            })
    };
    assert!(client_can_use(0x01, 0x01, Some((43, 150)))); // Human Warrior with Swords 150
    assert!(!client_can_use(0x80, 0x01, Some((43, 150)))); // excluded Mage
    assert!(!client_can_use(0x01, 0x02, Some((43, 150)))); // excluded Orc
    assert!(!client_can_use(0x01, 0x01, None)); // missing sword proficiency

    drop(client);
    server.join().unwrap();
}

#[test]
fn the_writer_drains_the_egress_depth_back_to_zero_b2() {
    let store = std::sync::Arc::new(quest_store());
    let (client, _c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    let depth = store
        .session
        .session_depth
        .lock()
        .unwrap()
        .clone()
        .expect("the session must have handed its egress handle to subscribe_player_events");
    assert_eq!(
        depth.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "the login sequence has been fully read by the client, so the writer must have decremented \
         the egress depth for every item it wrote — a depth that only climbs makes every session \
         shed peer movement forever once it passes EGRESS_SHED_DEPTH"
    );
    drop(client);
    let _ = server.join();
}

#[test]
fn group_invite_by_name_replies_party_command_result_success() {
    // CMSG_GROUP_INVITE "Buddy" resolves the name, calls the store, and echoes
    // SMSG_PARTY_COMMAND_RESULT(Invite, "Buddy", Success); the store recorded the resolved guid.
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        class: 1,
        level: 10,
        zone_id: 12,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "buddy".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(r.result, wow_world_messages::vanilla::PartyResult::Success);
            assert_eq!(
                r.operation,
                wow_world_messages::vanilla::PartyOperation::Invite
            );
            assert_eq!(r.member, "buddy");
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
    }
    assert_eq!(store.party.group_invites.lock().unwrap().as_slice(), &[2]);
    drop(client);
    let _ = server.join();
}

/// Every party Refusal the Module can send reaches the client as exactly one `PartyResult`, through
/// the real store seam rather than the mapping function alone.
#[test]
fn every_group_refusal_reaches_the_client_as_one_party_result() {
    use wow_world_messages::vanilla::PartyResult;
    for refusal in GroupRefusal::ALL {
        let want = match refusal {
            GroupRefusal::AlreadyInGroup => PartyResult::AlreadyInGroup,
            GroupRefusal::GroupFull => PartyResult::GroupFull,
            GroupRefusal::NotLeader => PartyResult::NotLeader,
            GroupRefusal::NotInGroup => PartyResult::NotInGroup,
            GroupRefusal::TargetNotInGroup => PartyResult::TargetNotInGroup,
            GroupRefusal::WrongFaction => PartyResult::PlayerWrongFaction,
            _ => PartyResult::BadPlayerName,
        };
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 2,
            name: "Buddy".into(),
            ..Default::default()
        }];
        s.trade_error = Some(refusal.as_tag().to_string());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        wow_world_messages::vanilla::CMSG_GROUP_INVITE {
            name: "Buddy".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
                assert_eq!(r.result, want, "{refusal:?} must map to {want:?}")
            }
            other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
        }
        drop(client);
        let _ = server.join();
    }
}

/// A reducer that timed out left the party in an unknown state, so it must end the session rather
/// than pose as a gameplay answer the client renders.
#[test]
fn a_group_invite_timeout_is_not_answered_as_a_refusal() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        ..Default::default()
    }];
    s.session.login_entity = Some(warrior_entity());
    s.trade_error = Some("gw_group_invite reducer timed out after 10s".into());
    let store = std::sync::Arc::new(s);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        result_tx
            .send(run_world_session(server_end, server_store.clone()))
            .unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "Buddy".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("an unknown party outcome must end the session promptly")
        .expect_err("a timed-out party reducer must be session-fatal");
    assert!(format!("{error:#}").contains("timed out"));
}

#[test]
fn group_invite_unknown_name_replies_bad_player_name() {
    // An unresolvable name never reaches the store — the reply is BadPlayerName ("player not found").
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(
                r.result,
                wow_world_messages::vanilla::PartyResult::BadPlayerName
            );
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
    }
    assert!(store.party.group_invites.lock().unwrap().is_empty());
    drop(client);
    let _ = server.join();
}

/// Read one `SMSG_FRIEND_STATUS` frame — RAW always, now that `ONLINE`/`ADDED_ONLINE` carry
/// trailing fields gtker 0.3 cannot type (see `codec::build_friend_status_raw`). Returns the
/// result, the other party's guid, and the trailing bytes (empty unless online).
fn read_friend_status(
    client: &mut UnixStream,
    dec: &mut DecrypterHalf,
) -> (FriendResult, u64, Vec<u8>) {
    let (opcode, body) = read_raw_frame(client, dec);
    assert_eq!(
        opcode,
        codec::social::SMSG_FRIEND_STATUS_OPCODE,
        "expected SMSG_FRIEND_STATUS"
    );
    let result = FriendResult::try_from(body[0]).expect("a known FriendResult byte");
    let guid = u64::from_le_bytes(body[1..9].try_into().unwrap());
    (result, guid, body[9..].to_vec())
}

#[test]
fn add_friend_by_name_then_friend_list_carries_online_presence() {
    // CMSG_ADD_FRIEND "Buddy" -> SMSG_FRIEND_STATUS AddedOnline (guid 2, resolved by
    // name); a follow-up CMSG_FRIEND_LIST then carries them online with their level/class/zone.
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        class: 1,
        level: 10,
        zone_id: 12,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ADD_FRIEND {
        name: "buddy".into(),
    } // case-insensitive match
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::AddedOnline);
    assert_eq!(guid, 2);
    assert_eq!(trailer[0], 1, "status ONLINE");

    CMSG_FRIEND_LIST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_LIST(l) => {
            assert_eq!(l.friends.len(), 1);
            assert_eq!(l.friends[0].guid.guid(), 2);
            assert!(matches!(
                l.friends[0].status,
                wow_world_messages::vanilla::Friend_FriendStatus::Online { .. }
            ));
        }
        other => panic!("expected SMSG_FRIEND_LIST, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_IGNORE_LIST(l) => assert!(l.ignored.is_empty()),
        other => panic!("expected SMSG_IGNORE_LIST, got {other}"),
    }

    // Removing it replies Removed and the friend list empties out.
    CMSG_DEL_FRIEND { guid: Guid::new(2) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, _, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::Removed);
    assert!(trailer.is_empty(), "a remove carries no trailing fields");
    drop(client);
    server.join().unwrap();
}

/// A friend on ANOTHER Shard resolves. Vim lives on `instances`; Ginger (this Gateway's own
/// Shard) adds them by name and gets ADDED_ONLINE with Vim's real presence, byte-exact against
/// the raw encoder.
#[test]
fn add_friend_on_another_shard_answers_added_online_with_presence() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 1,
        name: "Ginger".into(),
        race: 1,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let peer = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "instances".into(),
            ..Default::default()
        },
        characters: vec![codec::CharacterView {
            guid: 2,
            name: "Vim".into(),
            race: 1, // same team as Ginger
            class: 4,
            ..Default::default()
        }],
        social: SocialState {
            // Vim standing IN the instance: a live entity, not just a durable row, so the ADDED_ONLINE
            // fields below come off `Whereabouts::InWorld`.
            member_entities: std::sync::Mutex::new(vec![(
                2,
                codec::MemberEntity {
                    level: 22,
                    zone_id: 33,
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    });
    *store.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];
    *peer.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];

    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND { name: "Vim".into() }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, guid, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::AddedOnline);
    assert_eq!(guid, 2);
    let mut expected = vec![1u8]; // status ONLINE
    expected.extend(33u32.to_le_bytes()); // area
    expected.extend(22u32.to_le_bytes()); // level
    expected.extend(4u32.to_le_bytes()); // class
    assert_eq!(trailer, expected);
    drop(client);
    server.join().unwrap();
}

/// A friend standing in another Shard's instance shows ONLINE with real level/class/zone through
/// `CMSG_FRIEND_LIST`, and AFK/DND status follow the live entity's `PLAYER_FLAGS` — exercised
/// through the actual socket dispatch, so it runs the SAME `world::social::friend_views`
/// composition a Coordinator serves, not a Store Fake's own reimplementation of it.
#[test]
fn friend_list_shows_a_friend_in_another_shards_instance_online_afk_or_dnd() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 1,
        name: "Ginger".into(),
        race: 1,
        ..Default::default()
    }];
    s.social.contacts = std::sync::Mutex::new(vec![(1, 2, false)]); // Ginger already friends Vim
    let store = std::sync::Arc::new(s);
    let peer = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "instances".into(),
            ..Default::default()
        },
        characters: vec![codec::CharacterView {
            guid: 2,
            name: "Vim".into(),
            race: 1, // same team as Ginger
            class: 4,
            ..Default::default()
        }],
        social: SocialState {
            member_entities: std::sync::Mutex::new(vec![(
                2,
                codec::MemberEntity {
                    level: 22,
                    zone_id: 33,
                    player_flags: lyracore_shared::constants::player_flags::DND,
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        ..Default::default()
    });
    *store.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];
    *peer.topology.peers.lock().unwrap() = vec![store.clone(), peer.clone()];

    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_FRIEND_LIST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_FRIEND_LIST(l) => {
            assert_eq!(l.friends.len(), 1);
            assert_eq!(l.friends[0].guid.guid(), 2);
            match l.friends[0].status {
                wow_world_messages::vanilla::Friend_FriendStatus::Dnd { area, class, level } => {
                    assert_eq!(area.as_int(), 33);
                    assert_eq!(level.as_int(), 22);
                    assert_eq!(class, wow_world_messages::vanilla::Class::Rogue);
                }
                other => panic!("expected Dnd, got {other:?}"),
            }
        }
        other => panic!("expected SMSG_FRIEND_LIST, got {other}"),
    }
    // CMSG_FRIEND_LIST always answers with BOTH lists; leaving the SMSG_IGNORE_LIST reply unread
    // and dropping the client closes the socket with a queued write still in flight, which Linux
    // reports to the server as a reset rather than a clean EOF.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_IGNORE_LIST(l) => assert!(l.ignored.is_empty()),
        other => panic!("expected SMSG_IGNORE_LIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// An enemy-faction friend answers FRIEND_ENEMY. The companion half — the same target is a legal
/// ignore — is a Module rule this Store Fake cannot express; it is pinned durably in
/// `module/tests/friends.rs`.
#[test]
fn add_friend_enemy_faction_answers_friend_enemy() {
    let mut s = quest_store();
    s.characters = vec![
        codec::CharacterView {
            guid: 1,
            name: "Ginger".into(),
            race: 1, // Human, Alliance
            ..Default::default()
        },
        codec::CharacterView {
            guid: 2,
            name: "Grunt".into(),
            race: 2, // Orc, Horde
            ..Default::default()
        },
    ];
    s.trade_error = Some(ContactRefusal::Enemy.as_tag().to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND {
        name: "Grunt".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, trailer) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::Enemy);
    assert_eq!(guid, 2);
    assert!(trailer.is_empty());
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_friend_unknown_name_replies_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_FRIEND {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, _) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::NotFound);
    assert_eq!(guid, 0);
    drop(client);
    server.join().unwrap();
}

/// The ignore list has its own "unknown name" code.
#[test]
fn add_ignore_unknown_name_replies_ignore_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_IGNORE {
        name: "Nobody".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, _) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreNotFound);
    assert_eq!(guid, 0);
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_ignore_by_name_replies_ignore_added() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 3,
        name: "Pest".into(),
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ADD_IGNORE {
        name: "Pest".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, guid, _) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreAdded);
    assert_eq!(guid, 3);
    drop(client);
    server.join().unwrap();
}

#[test]
fn add_friend_maps_self_already_and_full_errors() {
    for (refusal, want) in [
        (ContactRefusal::AddSelf, FriendResult::SelfX),
        (ContactRefusal::AlreadyOnList, FriendResult::Already),
        (ContactRefusal::ListFull, FriendResult::ListFull),
    ] {
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 2,
            name: "Buddy".into(),
            ..Default::default()
        }];
        s.trade_error = Some(refusal.as_tag().to_string());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_ADD_FRIEND {
            name: "Buddy".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (result, ..) = read_friend_status(&mut client, &mut c_dec);
        assert_eq!(result, want, "{refusal:?} must map to {want:?}");
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn add_ignore_maps_already_and_full_errors_to_the_ignore_variants() {
    for (refusal, want) in [
        (ContactRefusal::AddSelf, FriendResult::IgnoreSelf),
        (ContactRefusal::AlreadyOnList, FriendResult::IgnoreAlready),
        (ContactRefusal::ListFull, FriendResult::IgnoreFull),
    ] {
        let mut s = quest_store();
        s.characters = vec![codec::CharacterView {
            guid: 3,
            name: "Pest".into(),
            ..Default::default()
        }];
        s.trade_error = Some(refusal.as_tag().to_string());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_ADD_IGNORE {
            name: "Pest".into(),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (result, ..) = read_friend_status(&mut client, &mut c_dec);
        assert_eq!(result, want, "{refusal:?} must map to {want:?}");
        drop(client);
        server.join().unwrap();
    }
}

/// Every contact Refusal the Module can send reaches the client as exactly one `FriendResult`, and
/// the friends and ignore lists get their own code family for the same condition.
#[test]
fn every_contact_refusal_reaches_the_client_as_one_friend_result() {
    for refusal in ContactRefusal::ALL {
        let (friend, ignore) = match refusal {
            ContactRefusal::AddSelf => (FriendResult::SelfX, FriendResult::IgnoreSelf),
            ContactRefusal::AlreadyOnList => (FriendResult::Already, FriendResult::IgnoreAlready),
            ContactRefusal::ListFull => (FriendResult::ListFull, FriendResult::IgnoreFull),
            ContactRefusal::NotOnList => (FriendResult::NotFound, FriendResult::IgnoreNotFound),
            ContactRefusal::ActorUnavailable => (FriendResult::NotFound, FriendResult::NotFound),
            ContactRefusal::Enemy => (FriendResult::Enemy, FriendResult::Enemy),
        };
        for (is_ignore, want) in [(false, friend), (true, ignore)] {
            let mut s = quest_store();
            s.characters = vec![codec::CharacterView {
                guid: 2,
                name: "Buddy".into(),
                ..Default::default()
            }];
            s.trade_error = Some(refusal.as_tag().to_string());
            let store = std::sync::Arc::new(s);
            let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
            if is_ignore {
                CMSG_ADD_IGNORE {
                    name: "Buddy".into(),
                }
                .write_encrypted_client(&mut client, &mut c_enc)
                .unwrap();
            } else {
                CMSG_ADD_FRIEND {
                    name: "Buddy".into(),
                }
                .write_encrypted_client(&mut client, &mut c_enc)
                .unwrap();
            }
            let (result, ..) = read_friend_status(&mut client, &mut c_dec);
            assert_eq!(result, want, "{refusal:?} on ignore={is_ignore}");
            drop(client);
            server.join().unwrap();
        }
    }
}

/// The contact half of the same rule: a timed-out add left the list in an unknown state.
#[test]
fn an_add_friend_timeout_is_not_answered_as_a_refusal() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 2,
        name: "Buddy".into(),
        ..Default::default()
    }];
    s.session.login_entity = Some(warrior_entity());
    s.trade_error = Some("gw_add_friend reducer timed out after 10s".into());
    let store = std::sync::Arc::new(s);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        result_tx
            .send(run_world_session(server_end, server_store.clone()))
            .unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    CMSG_ADD_FRIEND {
        name: "Buddy".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("an unknown contact outcome must end the session promptly")
        .expect_err("a timed-out contact reducer must be session-fatal");
    assert!(format!("{error:#}").contains("timed out"));
}

#[test]
fn del_friend_unknown_contact_replies_not_found() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_DEL_FRIEND {
        guid: Guid::new(404),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::NotFound);
    drop(client);
    server.join().unwrap();
}

#[test]
fn del_ignore_round_trips_added_then_unknown_is_ignore_not_found() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 3,
        name: "Pest".into(),
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    CMSG_ADD_IGNORE {
        name: "Pest".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreAdded);

    // Removing the just-added contact: IgnoreRemoved.
    CMSG_DEL_IGNORE { guid: Guid::new(3) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreRemoved);
    // Removing it again: no longer on the list -> IgnoreNotFound.
    CMSG_DEL_IGNORE { guid: Guid::new(3) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (result, ..) = read_friend_status(&mut client, &mut c_dec);
    assert_eq!(result, FriendResult::IgnoreNotFound);
    drop(client);
    server.join().unwrap();
}

#[test]
fn quest_choose_reward_relays_inventory_before_completion_over_the_cipher() {
    // The socket-level contract: the subscribed reducer callback has already queued the complete
    // item insertion when turn_in_quest returns, so the session's completion presentation must sit
    // behind CREATE, its inventory pointer and gain feedback on the one writer queue.
    let mut s = quest_store();
    let detail = detail_view(1234, "A Threat Within");
    s.quest.quest_details = vec![detail.clone()];
    let reward_item = codec::ItemInstanceView {
        guid: 0x4000_0000_0000_0042,
        entry: 25,
        owner_guid: 1,
        slot: 23,
        stack_count: 2,
        durability: 20,
        max_durability: 20,
        container_slots: 0,
        random_property_id: 0,
        random_property_enchant_ids: [0; 3],
        item_text_id: 0,
        enchantment: 0,
    };
    s.session.turn_in_reward_item = Some(reward_item.clone());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_QUESTGIVER_CHOOSE_REWARD {
        guid: Guid::new(50),
        quest_id: 1234,
        reward: 2,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let framed = |message: ServerOpcodeMessage| {
        let mut bytes = Vec::new();
        message.write_unencrypted_server(&mut bytes).unwrap();
        (
            u16::from_le_bytes([bytes[2], bytes[3]]),
            bytes[4..].to_vec(),
        )
    };
    let expected = [
        framed(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_item_create_object(&reward_item),
        ))),
        framed(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_inv_slot_values(
                reward_item.owner_guid,
                reward_item.slot,
                reward_item.guid,
            )
            .unwrap(),
        ))),
        framed(ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(Box::new(
            codec::build_item_push_result(
                reward_item.owner_guid,
                255,
                reward_item.slot as u32,
                reward_item.entry,
                reward_item.stack_count,
                false,
                0,
            ),
        ))),
        framed(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_COMPLETE(
            Box::new(codec::build_quest_complete(&detail)),
        )),
    ];
    let actual = std::array::from_fn(|_| read_raw_frame(&mut client, &mut c_dec));
    assert_eq!(
        actual, expected,
        "inventory visibility must precede completion"
    );
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.quest.turned_in.lock().unwrap().as_slice(),
        &[(7, 50, 1234, 2)]
    );
}

#[test]
fn login_sends_the_quest_log_descriptor_raw_update_after_the_create_packet() {
    let slots = vec![codec::update_mask::QuestLogSlot {
        slot: 3,
        quest_id: 777,
        counts: Vec::new(),
        state: 0,
        timer: 0,
    }];
    let mut s = quest_store();
    s.quest.quest_log_slots = slots.clone();
    let store = std::sync::Arc::new(s);

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // World entry — discarded, this test is about what comes right after.
    drain_world_entry(&mut client, &mut c_dec);
    // gtker's typed reader rejects this raw partial VALUES body (no OBJECT_FIELD_TYPE), so read it
    // RAW and compare it against the same builder the seam's `quest_log_update` calls.
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    let mask = codec::update_mask::full_quest_log_mask(&slots);
    assert_eq!((opcode, body), codec::build_values_update_raw(1, &mask));

    drop(client);
    server.join().unwrap();
}

// ── Inspect ───────────────────────────────────────────────────────────────────────────────────────

#[test]
fn inspect_in_range_friendly_target_replies_smsg_inspect_with_the_target_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_INSPECT {
        guid: Guid::new(55),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INSPECT(r) => assert_eq!(r.guid.guid(), 55),
        other => panic!("expected SMSG_INSPECT, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn inspect_refused_target_sends_no_reply() {
    // CMSG_PLAYED_TIME (below) always replies as long as `character_by_guid` resolves the caller's
    // own guid, so give the store a character row for guid 1 (quest_store() has none).
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            characters: vec![codec::CharacterView {
                guid: 1,
                ..Default::default()
            }],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    // guid 0 is the mock store's stand-in for "out of range / no such target" — the gate rejects it
    // and the handler drops the request silently (mirrors CMSG_GAMEOBJ_USE/CMSG_AREATRIGGER).
    CMSG_INSPECT { guid: Guid::new(0) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // Sentinel: a follow-up request with a guaranteed reply. If the refused CMSG_INSPECT had
    // wrongly produced an SMSG_INSPECT, it would arrive first and this match would fail.
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {} // no SMSG_INSPECT was sent for the refused target
        other => {
            panic!("expected SMSG_PLAYED_TIME (no SMSG_INSPECT for refused target), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
}

// ── Vendor / buy-failed ─────────────────────────────────────────────────────────

#[test]
fn buy_item_err_sends_smsg_buy_failed() {
    // When `buy_item` returns Err (e.g. "not enough money"), the gateway must send SMSG_BUY_FAILED
    // with the matching BuyResult code so the player gets an on-screen error.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some("not enough money to buy that item".into()),
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BUY_ITEM {
        vendor: Guid::new(99),
        item: 1234,
        amount: 1,
        unknown1: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_BUY_FAILED(p) => {
            assert_eq!(p.guid.guid(), 99, "vendor guid echoed back");
            assert_eq!(p.item, 1234, "item entry echoed back");
            assert!(
                matches!(p.result, BuyResult::NotEnoughMoney),
                "BuyResult maps to NotEnoughMoney"
            );
        }
        other => panic!("expected SMSG_BUY_FAILED, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

// ── Bank ──────────────────────────────────────────────────────────────────────────

#[test]
fn banker_activate_sends_smsg_show_bank_with_the_banker_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BANKER_ACTIVATE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOW_BANK(p) => assert_eq!(p.guid.guid(), 77),
        other => panic!("expected SMSG_SHOW_BANK, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn banker_activate_on_a_standing_refusing_banker_sends_no_reply() {
    // CMSG_PLAYED_TIME (the sentinel below) only replies once `character_by_guid` resolves the
    // caller's own guid, so give the store a character row for guid 1 (quest_store() has none) —
    // same setup as `inspect_refused_target_sends_no_reply`.
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            npc_refuses: true,
            characters: vec![codec::CharacterView {
                guid: 1,
                ..Default::default()
            }],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_BANKER_ACTIVATE {
        guid: Guid::new(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // Sentinel: a follow-up request with a guaranteed reply. If the refused activate had wrongly
    // produced an SMSG_SHOW_BANK, it would arrive first and this match would fail.
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {} // no SMSG_SHOW_BANK for the refused banker
        other => {
            panic!("expected SMSG_PLAYED_TIME (no SMSG_SHOW_BANK for refused banker), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_an_imported_banker_option_opens_the_bank_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(
        0,
        "I would like to check my deposit box.",
        gossip_option::BANKER,
    )];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOW_BANK(p) => assert_eq!(p.guid.guid(), 90),
        other => panic!("expected SMSG_SHOW_BANK, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_a_petitioner_option_opens_the_charter_list() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "How do I form a guild?", gossip_option::PETITIONER)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PETITION_SHOWLIST(list) => {
            assert_eq!(list.npc.guid(), 90);
            assert_eq!(list.petitions[0].charter_entry, 5863);
            assert_eq!(list.petitions[0].guild_charter_cost, 1000);
        }
        other => panic!("expected SMSG_PETITION_SHOWLIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autobank_item_from_the_main_bag_dispatches_auto_bank_item() {
    // Right-click a bag item with the bank open (CMSG_AUTOBANK_ITEM) → the gateway names the source
    // slot and lets the module resolve the free bank slot; deposit and withdraw share one store method.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 255,
        slot_index: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bank.auto_banked_items.lock().unwrap().as_slice(),
        &[23]
    );
}

#[test]
fn autostore_bank_item_from_the_main_bag_dispatches_auto_bank_item() {
    // Right-click a banked item (CMSG_AUTOSTORE_BANK_ITEM) → withdraw, same store method as deposit.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BANK_ITEM {
        bag_index: 255,
        slot_index: 39,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bank.auto_banked_items.lock().unwrap().as_slice(),
        &[39]
    );
}

#[test]
fn autobank_item_err_sends_smsg_inventory_change_failure() {
    // A full destination (bank full, or carry space full) is a per-action error relayed as the
    // existing inventory-change-failure reply, never session-fatal.
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            trade_error: Some("bank full".into()),
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 255,
        slot_index: 23,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn autobank_item_from_a_sub_bag_is_unsupported_and_does_not_dispatch() {
    // Only the main pseudo-bag (255) is addressed, matching the item handler's restriction — a
    // sub-bag index is logged and ignored, never fatal.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOBANK_ITEM {
        bag_index: 19,
        slot_index: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.bank.auto_banked_items.lock().unwrap().is_empty(),
        "a sub-bag source must not be routed through auto_bank_item"
    );
}

#[test]
fn autostore_bank_item_from_a_sub_bag_is_unsupported_and_does_not_dispatch() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_AUTOSTORE_BANK_ITEM {
        bag_index: 19,
        slot_index: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.bank.auto_banked_items.lock().unwrap().is_empty(),
        "a sub-bag source must not be routed through auto_bank_item"
    );
}

#[test]
fn buy_bank_slot_success_sends_ok_and_reaches_the_named_banker() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_BUY_BANK_SLOT {
        guid: Guid::new(88),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p) => {
            assert_eq!(p.result, BuyBankSlotResult::Ok);
        }
        other => panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.bank.bought_bank_slots.lock().unwrap().as_slice(),
        &[88]
    );
}

#[test]
fn buy_bank_slot_failure_maps_the_bracketed_code_to_the_matching_result() {
    // The module tags a refusal with its `SMSG_BUY_BANK_SLOT_RESULT` code in brackets — parsed by
    // code, not by matching the prose.
    for (err, want) in [
        (
            "[0] no bank bag slots left to buy",
            BuyBankSlotResult::FailedTooMany,
        ),
        (
            "[1] not enough money (need 1000)",
            BuyBankSlotResult::InsufficientFunds,
        ),
        ("[2] target is not a banker", BuyBankSlotResult::NotBanker),
    ] {
        let mut s = quest_store();
        s.trade_error = Some(err.into());
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_BUY_BANK_SLOT {
            guid: Guid::new(88),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_BUY_BANK_SLOT_RESULT(p) => {
                assert_eq!(p.result, want, "store error {err:?} must map to {want:?}");
            }
            other => panic!("expected SMSG_BUY_BANK_SLOT_RESULT, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

// ── Inventory change failure ─────────────────────────────────────────────────────

#[test]
fn equip_item_err_sends_smsg_inventory_change_failure() {
    // Socket contract: a gameplay refusal reaches the client as an encrypted
    // SMSG_INVENTORY_CHANGE_FAILURE frame and the session keeps serving the next action.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some(ItemRefusal::CannotEquip.as_tag().into()),
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    // Slot 24 is a backpack slot (>= 23); bag 255 = INVENTORY_SLOT_BAG_0 (main bag).
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {} // correct feedback packet
        other => panic!("expected SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 25,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(_) => {}
        other => panic!("expected a second SMSG_INVENTORY_CHANGE_FAILURE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn item_action_before_player_login_is_handled_without_panicking() {
    // Socket contract: an item frame arriving after the handshake but before CMSG_PLAYER_LOGIN
    // (no selected player) must not panic or error the session thread.
    let store = std::sync::Arc::new(tester_store(7));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("an item action without a selected player must not panic")
        .expect("the legacy zero-actor fallback remains a handled gameplay context");
}

#[test]
fn item_reducer_transport_loss_ends_the_world_session() {
    // Socket contract: reducer transport loss ends the session with an error and closes the
    // socket instead of being translated into gameplay feedback.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some("equip_item reducer transport disconnected: channel closed".into()),
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        result_tx
            .send(run_world_session(server_end, server_store.clone()))
            .unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);

    CMSG_AUTOEQUIP_ITEM {
        source_bag: 255,
        source_slot: 24,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("transport loss must end the session promptly")
        .expect_err("a disconnected item reducer transport must be session-fatal");
    assert!(format!("{error:#}").contains("reducer transport disconnected"));
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the socket closes instead of translating transport loss into gameplay feedback"
    );
}

// ===========================================================================
// Logout gate tests (blocking logout while in combat)
// ===========================================================================

#[test]
fn logout_while_out_of_combat_succeeds_and_clears_open_loot() {
    // combat_until_ms=0 (default, never in combat) → CMSG_LOGOUT_REQUEST must reply
    // Success/Instant + LOGOUT_COMPLETE and the Store must release Account ownership.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            loot_window: LootWindowState {
                corpse_money: 25,
                ..base.loot_window
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);

    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // First message: SMSG_LOGOUT_RESPONSE(Success, Instant)
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::{LogoutResult, LogoutSpeed};
            assert_eq!(
                r.result,
                LogoutResult::Success,
                "expected Success, got {:?}",
                r.result
            );
            assert_eq!(r.speed, LogoutSpeed::Instant);
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE, got {other}"),
    }
    // Second message: SMSG_LOGOUT_COMPLETE
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE => {}
        other => panic!("expected SMSG_LOGOUT_COMPLETE, got {other}"),
    }

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    drop(client);
    server.join().unwrap();

    // Releasing Account ownership removes the live Character.
    assert!(
        store
            .session
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "Account ownership must be released after successful logout"
    );
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "logout must discard the open loot target"
    );
}

#[test]
fn logout_while_in_combat_is_denied() {
    // combat_until_ms=u64::MAX → CMSG_LOGOUT_REQUEST must reply FailureInCombat. The session
    // stays alive (verified by sending a second request and getting a second denial), meaning the
    // entity was NOT removed during the handler — the player cannot escape combat by logging out.
    // Note: socket teardown (drop below) still calls leave_world/logout as cleanup; that is correct
    // and separate from the CMSG gate.
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                combat_until_ms: u64::MAX, // always in combat
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    // First request: must be denied.
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::LogoutResult;
            assert_eq!(
                r.result,
                LogoutResult::FailureInCombat,
                "expected FailureInCombat on first request, got {:?}",
                r.result
            );
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE(denial) on first request, got {other}"),
    }

    // Second request: still denied (session is still alive, entity still in-world).
    // If the first denial had accidentally removed the entity / transitioned to CharSelect,
    // this second request would either hang or produce a different message.
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
            use wow_world_messages::vanilla::LogoutResult;
            assert_eq!(
                r.result,
                LogoutResult::FailureInCombat,
                "expected FailureInCombat on second request (session still alive), got {:?}",
                r.result
            );
        }
        other => panic!("expected SMSG_LOGOUT_RESPONSE(denial) on second request, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn played_time_replies_with_the_durable_total_plus_the_live_session_span() {
    // CMSG_PLAYED_TIME -> SMSG_PLAYED_TIME. The character row carries a durable
    // 3600s total plus a session_start_micros stamped in the recent past, so the reply must be
    // strictly greater than the durable floor (the live span gets folded in) and sane (not absurdly
    // large — bounds the test against a unit mixup, e.g. treating micros as millis).
    let durable_secs: u32 = 3600;
    let session_started_secs_ago: u64 = 5;
    let now_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    let session_start_micros = now_micros - session_started_secs_ago * 1_000_000;

    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Tester".into(),
                played_total_secs: durable_secs,
                session_start_micros,
                ..Default::default()
            }],
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(p) => {
            assert!(
                p.total_played_time >= durable_secs,
                "reply {} must be at least the durable floor {durable_secs}",
                p.total_played_time
            );
            assert!(
                p.total_played_time < durable_secs + 60,
                "reply {} should only add a few seconds of live session, not run away",
                p.total_played_time
            );
            assert_eq!(
                p.total_played_time, p.level_played_time,
                "level time mirrors total (untracked per-level in this slice)"
            );
        }
        other => panic!("expected SMSG_PLAYED_TIME, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

fn imported_auction_interaction() -> AuctionInteraction {
    AuctionInteraction {
        house: super::handlers::AuctionHousePolicy {
            id: 1,
            deposit_rate: 5,
            consignment_rate: 5,
        },
        refuses_interaction: false,
    }
}

#[test]
fn auction_house_round_trip_stays_typed_and_ordered_over_an_encrypted_session() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            auction: AuctionState {
                auction_interaction: Some(imported_auction_interaction()),
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    let auctioneer = Guid::new(42);

    MSG_AUCTION_HELLO_Client { auctioneer }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_AUCTION_HELLO(message) => {
            assert_eq!(message.auctioneer, auctioneer);
            assert_eq!(message.auction_house.as_int(), 1);
        }
        other => panic!("expected auction hello first, got {other}"),
    }

    CMSG_AUCTION_LIST_ITEMS {
        auctioneer,
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty browse view second, got {other}"),
    }

    CMSG_AUCTION_LIST_OWNER_ITEMS {
        auctioneer,
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty owner view third, got {other}"),
    }

    CMSG_AUCTION_LIST_BIDDER_ITEMS {
        auctioneer,
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(result) => {
            assert!(result.auctions.is_empty());
            assert_eq!(result.total_amount_of_auctions, 0);
        }
        other => panic!("expected empty bidder view fourth, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

#[test]
fn refused_auctioneer_interaction_keeps_the_encrypted_world_session_alive() {
    // A resolvable requester with no seeded in-world Characters (`entity_in_world: false`
    // overrides `quest_store`'s blanket flag), so the WHO answer this test cares about is the
    // empty-but-present reply, not "no answer for an unknown requester" (a different rule, pinned
    // in `social.rs`'s own WHO tests).
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Tester".into(),
                race: 1,
                ..Default::default()
            }],
            session: SessionState {
                entity_in_world: false,
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    MSG_AUCTION_HELLO_Client {
        auctioneer: Guid::new(999),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_WHO::default()
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    // RAW-encoded (codec::build_who_response_raw); the auction refusal must be silent and leave
    // WHO's own empty-roster reply next.
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, codec::social::SMSG_WHO_OPCODE);
    assert_eq!(
        &body[0..8],
        &[0u8; 8],
        "no in-world Characters: listed and online both 0"
    );

    drop(client);
    server.join().unwrap();
}

// ===========================================================================================
//  The handler-level tests — CMSG_CAST_SPELL routing, quest instant
//  routing, the loot window state machine, melee attack over the socket, and the smaller mappings.
//  Each drives the full encrypted session (enter_world) and pins wire replies + store dispatches.
// ===========================================================================================

/// Vanilla opcodes for the frames the cast path emits (raw + typed) — pinned as numbers because the
/// synchronous ORDER across `Outbound::Raw` and typed sends is the contract under test.
const OP_CAST_RESULT: u16 = 0x0130;
const OP_SPELL_START: u16 = 0x0131;
const OP_SPELL_GO: u16 = 0x0132;
const OP_LOOT_RESPONSE: u16 = 0x0160;

/// Read one encrypted server frame RAW: decrypt the 4-byte header via the client's `DecrypterHalf`,
/// return `(opcode, body)`. Needed where the wire contract is an `Outbound::Raw` packet (the 5-byte
/// CAST_RESULT ack, the loot/vendor windows) or where the exact opcode ORDER across raw+typed sends
/// is the assertion — gtker's typed reader rejects some of the hand-rolled bodies (it would consume
/// the frame but error), so the bytes are read and pinned directly.
fn read_raw_frame<S: Read>(client: &mut S, dec: &mut DecrypterHalf) -> (u16, Vec<u8>) {
    let h = dec.read_and_decrypt_server_header(&mut *client).unwrap();
    let mut body = vec![0u8; (h.size as usize).saturating_sub(2)];
    client.read_exact(&mut body).unwrap();
    (h.opcode, body)
}

fn open_loot_window(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    target_guid: u64,
) -> Vec<u8> {
    CMSG_LOOT {
        guid: Guid::new(target_guid),
    }
    .write_encrypted_client(&mut *client, enc)
    .unwrap();
    let (opcode, body) = read_raw_frame(client, dec);
    assert_eq!(opcode, OP_LOOT_RESPONSE);
    body
}

/// `SpellCastTargets` carrying a UNIT target (the client's selected mob).
fn unit_targets(guid: u64) -> SpellCastTargets {
    SpellCastTargets {
        target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
            SpellCastTargets_SpellCastTargetFlags_Unit {
                unit_target: Guid::new(guid),
            },
        ),
    }
}

// ── Cast dispatch through the encrypted session ────────────────────────────────────────────────
//
// Route selection, target decoding, durable dispatch and message ORDER are covered per route by the
// focused seam tests in `handlers/cast`. What only a socket can show survives here: that a cast
// opcode reaches the seam through the real dispatch chain, that the ordered batch leaves the writer
// as those frames in that order, and that the session supplies the caller's identity.

#[test]
fn instant_cast_sends_start_then_raw_cast_result_ok_then_go_and_threads_the_target() {
    // Root-cause client-wedge fix: an INSTANT cast must emit START(0) → raw CAST_RESULT(OK,
    // opcode 0x0130, 5-byte body) → GO synchronously, IN THAT ORDER, and the cast must reach the
    // store with the client's unit target. Raw and typed sends interleave here, so the frame order
    // is a property of the writer, not just of the batch the seam returns.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_CAST_SPELL {
        spell: 100,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let (op1, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op1, OP_SPELL_START, "first frame must be SMSG_SPELL_START");
    let (op2, body2) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op2, OP_CAST_RESULT,
        "second frame must be the raw CAST_RESULT ack"
    );
    let mut want = 100u32.to_le_bytes().to_vec();
    want.push(0x00); // SPELL_RESULT_STATUS_OKAY — 5 bytes, NO trailing reason byte
    assert_eq!(
        body2, want,
        "CAST_RESULT body is spell_id(u32 LE) + OKAY(0x00)"
    );
    let (op3, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op3, OP_SPELL_GO, "third frame must be SMSG_SPELL_GO");

    drop(client);
    server.join().unwrap();
    // The unit target rode CMSG → handler → store unchanged (target-keyed effects need it).
    assert_eq!(store.cast.casts.lock().unwrap().as_slice(), &[(100, 77)]);
}

#[test]
fn set_sheathed_routes_the_clients_z_press_to_the_store() {
    for (sent, expect) in [
        (SheathState::Unarmed, 0u8),
        (SheathState::Melee, 1),
        (SheathState::Ranged, 2),
    ] {
        let store = std::sync::Arc::new(quest_store());
        let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
        CMSG_SETSHEATHED { sheathed: sent }
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
        drop(client);
        server.join().unwrap();
        assert_eq!(
            store.combat.sheathed.lock().unwrap().as_slice(),
            &[(1, expect)],
            "{sent:?} must reach set_sheathed as byte {expect}"
        );
    }
}

#[test]
fn auto_shot_intercept_starts_the_ranged_attack_instead_of_casting() {
    // Vanilla shape: Auto Shot (75) and wand Shoot (5019) are auto-repeat ranged attacks —
    // the handler arms start_ranged_attack with the cast's unit target, then the activation ack is
    // SMSG_SPELL_START ALONE with timer 0 (no CAST_RESULT, no GO — the cast parks in the client's
    // AUTOREPEAT slot and never resolves; each shot's GO comes from the swing-tick relay).
    // cast_spell must NOT run.
    for spell in [75u32, 5019] {
        let store = std::sync::Arc::new(quest_store());
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
        CMSG_CAST_SPELL {
            spell,
            targets: unit_targets(88),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (op, body) = read_raw_frame(&mut client, &mut c_dec);
        assert_eq!(
            op, OP_SPELL_START,
            "spell {spell}: activation ack is SPELL_START alone"
        );
        // timer (u32 LE) is the LAST 4 body bytes before... layout: cast_item(packed) caster(packed)
        // spell(4) flags(2) timer(4) targets(..). Cheap pin: the timer bytes right after the u16
        // flags must be 0 — locate spell id then skip flags. spell sits at a packed-guid-dependent
        // offset; both packed self-guids here are 2 bytes (guid 1 -> [0x01, 0x01]).
        let spell_pos = 4; // two 2-byte packed guids
        assert_eq!(
            &body[spell_pos..spell_pos + 4],
            &spell.to_le_bytes(),
            "spell id in START"
        );
        assert_eq!(
            &body[spell_pos + 6..spell_pos + 10],
            &0u32.to_le_bytes(),
            "spell {spell}: START timer must be 0 (the 0.5s wind-up is an attack-timer, not a cast bar)"
        );
        // Nothing else may follow on the activation path (the old phantom GO fired the shoot
        // animation instantly).
        let mut probe = [0u8; 1];
        assert!(
            client.read(&mut probe).map(|n| n == 0).unwrap_or(true),
            "spell {spell}: no packet may follow the activation START"
        );
        drop(client);
        server.join().unwrap();
        assert_eq!(
            store.cast.ranged_attacks.lock().unwrap().as_slice(),
            &[(88, spell)]
        );
        assert!(
            store.cast.casts.lock().unwrap().is_empty(),
            "spell {spell} must not reach cast_spell"
        );
    }
}

#[test]
fn a_cast_before_entering_the_world_answers_nothing_and_keeps_the_session_alive() {
    // A cast can arrive while the session is still at character select — a stale addon macro, a
    // reconnect race. The seam answers no frames and names a zero caster; the session must serve the
    // next opcode instead of panicking, which the CHAR_ENUM sentinel proves.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_CAST_SPELL {
        spell: 100,
        targets: unit_targets(77),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected SMSG_CHAR_ENUM (the cast sent no frames), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cast.casts.lock().unwrap().as_slice(), &[(100, 77)]);
}

#[test]
fn cancelling_auto_repeat_still_tears_the_ranged_loop_down_through_stop_attack() {
    // The cancel tears the loop down only when one is armed (the `was_repeat` gate). Arm Auto Shot,
    // then cancel, and pin that the teardown reaches the durable disengage for this attacker.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CAST_SPELL {
        spell: 75,
        targets: unit_targets(88),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_SPELL_START, "the activation ack arms the loop");
    CMSG_CANCEL_AUTO_REPEAT_SPELL {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // The cancel sends no ack of its own (the on_delete relay does), so a sentinel proves the
    // server got that far before we assert.
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (the cancel acks nothing), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.melee.stop_attacks.lock().unwrap().as_slice(), &[1]);
}

// ── Quest giver routing (CMSG_QUESTGIVER_HELLO) ──────────────────────────────────────────────────

#[test]
fn quest_hello_reaches_the_quest_module_and_its_raw_details_body_survives_the_cipher() {
    // The socket-level contract: dispatch routes HELLO to the quest module, and the raw-encoded
    // DETAILS screen it returns crosses the encrypted frame intact. Which screen a giver opens is
    // decided at the `dispatch_quest_action` seam and proved there.
    const OP_QUEST_DETAILS: u16 = 0x0188;
    let mut s = quest_store();
    s.quest.quest_evals = vec![eval(1234, codec::ROLE_START, false, false)];
    s.quest.quest_details = vec![detail_view(1234, "A Threat Within")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_QUESTGIVER_HELLO {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_QUEST_DETAILS);
    assert_eq!(
        &body[..12],
        &codec::build_quest_details_raw(50, &detail_view(1234, "A Threat Within")).1[..12],
        "giver guid + quest id reached the wire unchanged"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn quest_query_answers_the_raw_definition_body_through_the_cipher() {
    // The socket-level contract for the client's cold-cache definition query: the hand-rolled 5875
    // body crosses the encrypted frame intact. Which quests answer at all is proved at the seam.
    const OP_QUEST_QUERY_RESPONSE: u16 = 0x005D;
    let mut s = quest_store();
    s.quest.quest_details = vec![detail_view(1234, "A Threat Within")];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_QUEST_QUERY { quest_id: 1234 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_QUEST_QUERY_RESPONSE);
    assert_eq!(
        body,
        codec::build_quest_query_response_raw(&detail_view(1234, "A Threat Within")).1
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn questgiver_gameobject_bypasses_the_chest_lifecycle() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(lyracore_shared::constants::go_type::QUESTGIVER);
    s.quest.quest_evals = vec![eval(1234, codec::ROLE_START, false, false)];
    s.quest.quest_details = vec![detail_view(1234, "A Threat Within")];
    s.loot_window
        .corpse_loot_by_viewer
        .insert(1, vec![(0, 2589, 1, 200, 0)]);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(68),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(details) => {
            assert_eq!(details.quest_id, 1234)
        }
        other => panic!("expected quest details from a questgiver GameObject, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store
        .loot_window
        .gameobjects_used
        .lock()
        .unwrap()
        .is_empty());
    assert!(store
        .loot_window
        .corpse_loot_reads
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn non_chest_gameobject_preserves_the_general_use_path() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(0);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(91),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store
            .loot_window
            .gameobjects_used
            .lock()
            .unwrap()
            .as_slice(),
        &[91]
    );
    assert_eq!(
        store
            .loot_window
            .corpse_loot_reads
            .lock()
            .unwrap()
            .as_slice(),
        &[(91, 1)]
    );
}

// ── Loot window state machine ────────────────────────────────────────────────────────────────────

#[test]
fn chest_dispatch_opens_the_shared_window_and_tracks_its_target() {
    let mut s = quest_store();
    s.npc.gameobject_type = Some(lyracore_shared::constants::go_type::CHEST);
    s.loot_window
        .corpse_loot_by_viewer
        .insert(1, vec![(4, 117, 2, 321, 0)]);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, OP_LOOT_RESPONSE);
    assert_eq!(&body[0..8], &90u64.to_le_bytes());
    assert_eq!(loot_item_bytes(&body, 0), (4, 117, 2, 321));

    CMSG_AUTOSTORE_LOOT_ITEM { item_slot: 4 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_REMOVED(removed) => assert_eq!(removed.slot, 4),
        other => panic!("expected SMSG_LOOT_REMOVED, got {other}"),
    }

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store
            .loot_window
            .gameobjects_used
            .lock()
            .unwrap()
            .as_slice(),
        &[90]
    );
    assert_eq!(
        store
            .loot_window
            .corpse_loot_reads
            .lock()
            .unwrap()
            .as_slice(),
        &[(90, 1)]
    );
    assert_eq!(
        store.loot_window.items_looted.lock().unwrap().as_slice(),
        &[(90, 4)]
    );
}

#[test]
fn loot_before_player_login_is_handled_without_panicking() {
    let store = std::sync::Arc::new(tester_store(7));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, _c_dec) = client_handshake(&mut client, "TESTER", K);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("loot without a selected player must not panic")
        .expect("loot without a selected player remains a handled no-op");
    assert!(store
        .loot_window
        .corpse_loot_reads
        .lock()
        .unwrap()
        .is_empty());
    assert!(store.loot_window.skinned.lock().unwrap().is_empty());
}

#[test]
fn loot_with_a_zero_player_guid_is_handled_without_panicking() {
    let mut entity = warrior_entity();
    entity.guid = 0;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(entity),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 0);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    server
        .join()
        .expect("loot with a zero player guid must not panic");
    assert!(store
        .loot_window
        .corpse_loot_reads
        .lock()
        .unwrap()
        .is_empty());
    assert!(store.loot_window.skinned.lock().unwrap().is_empty());
}

#[test]
fn skinning_refusal_keeps_the_world_session_alive() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            loot_window: LootWindowState {
                skinning_refusal: Some(LootWindowRefusal::Unanswered),
                ..base.loot_window
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    for target_guid in [60, 61] {
        CMSG_LOOT {
            guid: Guid::new(target_guid),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
        assert_eq!(opcode, OP_LOOT_RESPONSE);
        assert_eq!(&body[0..8], &target_guid.to_le_bytes());
    }

    drop(client);
    server.join().unwrap();
    assert!(store.loot_window.skinned.lock().unwrap().is_empty());
}

#[test]
fn skinning_infrastructure_failure_ends_the_world_session() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            loot_window: LootWindowState {
                skinning_failure: Some("gw_skin reducer timed out after 10s".to_string()),
                ..base.loot_window
            },
            ..base
        }
    });
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store, 1);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    assert!(
        server.join().is_err(),
        "a skinning timeout must end the World Session"
    );
}

#[test]
fn loot_opens_the_window_and_loot_money_drives_the_tracked_guid() {
    // CMSG_LOOT arms the open-loot state and replies the RAW loot window (guid + money in the body);
    // CMSG_LOOT_MONEY (which carries NO guid) must then hit the TRACKED corpse. A
    // SOLO money loot sends ONLY SMSG_LOOT_CLEAR_MONEY — the unconditional SMSG_LOOT_MONEY_NOTIFY
    // is gone (vanilla never sends it to a solo looter; the client prints its own local "You loot X
    // copper" line). A corpse with money is NOT skinned.
    let mut s = quest_store();
    s.loot_window.corpse_money = 25;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, OP_LOOT_RESPONSE);
    assert_eq!(
        &body[0..8],
        &60u64.to_le_bytes(),
        "loot window names the corpse guid"
    );
    assert_eq!(
        &body[9..13],
        &25u32.to_le_bytes(),
        "loot window shows the corpse's copper"
    );

    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY => {}
        other => panic!(
            "expected SMSG_LOOT_CLEAR_MONEY directly (no notify for a solo looter), got {other}"
        ),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_window.money_looted.lock().unwrap().as_slice(),
        &[60],
        "the TRACKED guid was looted"
    );
    assert!(
        store.loot_window.skinned.lock().unwrap().is_empty(),
        "a corpse with money is not skinned"
    );
}

#[test]
fn loot_money_with_zero_copper_still_clears_with_no_notify() {
    // amount == 0: the same no-notify contract as any solo loot — CLEAR_MONEY still
    // goes out so the client's loot window drops its money row.
    let store = std::sync::Arc::new(quest_store()); // corpse_money = 0
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = read_raw_frame(&mut client, &mut c_dec); // the loot window
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_CLEAR_MONEY => {} // and NOT a NOTIFY first
        other => {
            panic!("expected SMSG_LOOT_CLEAR_MONEY directly (no notify for 0 copper), got {other}")
        }
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_window.money_looted.lock().unwrap().as_slice(),
        &[60]
    );
}

#[test]
fn loot_release_clears_the_tracked_target_so_take_requests_are_noops() {
    let mut s = quest_store();
    s.loot_window.corpse_money = 25; // non-empty so the skin fallback stays out of the picture
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let _ = open_loot_window(&mut client, &mut c_enc, &mut c_dec, 60);
    CMSG_LOOT_RELEASE {
        guid: Guid::new(60),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LOOT_RELEASE_RESPONSE(r) => assert_eq!(r.guid.guid(), 60),
        other => panic!("expected SMSG_LOOT_RELEASE_RESPONSE, got {other}"),
    }
    // The window is closed — stray targetless take requests must not reach the store.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_AUTOSTORE_LOOT_ITEM { item_slot: 3 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert!(
        store.loot_window.money_looted.lock().unwrap().is_empty(),
        "release cleared the tracked target"
    );
    assert!(
        store.loot_window.items_looted.lock().unwrap().is_empty(),
        "release cleared the tracked target before an item take"
    );
}

// ── Group loot methods ───────────────────────────────────────────────────────────────────────────

#[test]
fn loot_method_dispatches_the_decoded_setting_threshold_and_master() {
    // CMSG_LOOT_METHOD (leader sets MASTER LOOT, Epic threshold, master guid 7) must reach the
    // store with the gateway-decoded wire bytes — a direct pass-through (module adopted the wire
    // ordering verbatim), no separate ack packet (vanilla sends none for this opcode either).
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_METHOD {
        loot_setting: GroupLootSetting::MasterLoot,
        loot_master: Guid::new(7),
        loot_threshold: ItemQuality::Epic,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // No reply packet for this opcode — send a harmless follow-up (CMSG_LOOT_MONEY, a no-op here
    // with no tracked target) and confirm it's the NEXT thing the server processes, proving the
    // method call didn't hang the dispatch loop waiting to send something.
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.party.group_loot_methods.lock().unwrap().as_slice(),
        &[(
            GroupLootSetting::MasterLoot.as_int(),
            7,
            ItemQuality::Epic.as_int()
        )]
    );
}

#[test]
fn loot_roll_dispatches_the_corpse_slot_and_vote() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_ROLL {
        item: Guid::new(60),
        item_slot: 2,
        vote: RollVote::Need,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_roll.loot_rolls.lock().unwrap().as_slice(),
        &[(60, 2, RollVote::Need.as_int())]
    );
}

#[test]
fn loot_master_give_dispatches_the_corpse_slot_and_target() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_MASTER_GIVE {
        loot: Guid::new(60),
        slot_id: 3,
        player: Guid::new(9),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_LOOT_MONEY {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.loot_roll.loot_master_gives.lock().unwrap().as_slice(),
        &[(60, 3, 9)]
    );
}

#[test]
fn loot_roll_rejection_is_logged_and_ignored_not_session_fatal() {
    // A rejection (no roll open / already voted / not eligible) must not tear the connection down —
    // the SAME session keeps working afterward (mirrors take_loot's per-action ignore discipline).
    let mut s = quest_store();
    s.loot_roll.loot_action_refusal = Some(LootRefusal::RollUnavailable);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOOT_ROLL {
        item: Guid::new(60),
        item_slot: 2,
        vote: RollVote::Greed,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The session survives: a subsequent CMSG_LOOT still gets a normal reply.
    CMSG_LOOT {
        guid: Guid::new(61),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, _) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(
        op, OP_LOOT_RESPONSE,
        "the session must survive a rejected loot_roll"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn loot_master_give_refusals_keep_the_world_session_alive() {
    for refusal in [
        LootRefusal::NotMasterLooter,
        LootRefusal::RecipientUnavailable,
        LootRefusal::RecipientInventoryFull,
    ] {
        let mut s = quest_store();
        s.loot_roll.loot_action_refusal = Some(refusal);
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_LOOT_MASTER_GIVE {
            loot: Guid::new(60),
            slot_id: 3,
            player: Guid::new(9),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        CMSG_LOOT {
            guid: Guid::new(61),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

        let (op, _) = read_raw_frame(&mut client, &mut c_dec);
        assert_eq!(op, OP_LOOT_RESPONSE, "{refusal:?}");
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn loot_roll_timeout_ends_the_world_session() {
    let mut s = quest_store();
    s.loot_roll.loot_action_failure = Some("gw_loot_roll reducer timed out after 10s".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store, 1);
    CMSG_LOOT_ROLL {
        item: Guid::new(60),
        item_slot: 2,
        vote: RollVote::Greed,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    assert!(
        server.join().is_err(),
        "an unknown vote result must end the World Session"
    );
}

#[test]
fn loot_master_give_transport_failure_ends_the_world_session() {
    let mut s = quest_store();
    s.loot_roll.loot_action_failure =
        Some("gw_loot_master_give reducer transport disconnected".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store, 1);
    CMSG_LOOT_MASTER_GIVE {
        loot: Guid::new(60),
        slot_id: 3,
        player: Guid::new(9),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);

    assert!(
        server.join().is_err(),
        "an unknown master-loot result must end the World Session"
    );
}

fn loot_item_bytes(body: &[u8], index: usize) -> (u8, u32, u32, u32) {
    // Item N starts at byte 14 (8 guid + 1 method + 4 money + 1 count), 22 bytes each.
    let base = 14 + index * 22;
    let slot = body[base];
    let item_id = u32::from_le_bytes(body[base + 1..base + 5].try_into().unwrap());
    let count = u32::from_le_bytes(body[base + 5..base + 9].try_into().unwrap());
    let display_id = u32::from_le_bytes(body[base + 9..base + 13].try_into().unwrap());
    (slot, item_id, count, display_id)
}

// ── CMSG_ATTACKSWING error split + happy path (combat C1) ───────────────────────────────────────

#[test]
fn attackswing_ok_replies_attackstart_and_stop_echoes_then_clears() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTART(a) => {
            assert_eq!(a.attacker.guid(), 1, "self guid");
            assert_eq!(a.victim.guid(), 90);
        }
        other => panic!("expected SMSG_ATTACKSTART, got {other}"),
    }
    // Stop echoes the armed target and clears it.
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTOP(a) => {
            assert_eq!(a.player.guid(), 1);
            assert_eq!(a.enemy.guid(), 90);
        }
        other => panic!("expected SMSG_ATTACKSTOP, got {other}"),
    }
    // A second stop finds no armed target → NO second echo (sentinel replies first).
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (no ATTACKSTOP echo once cleared), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn melee_opcodes_at_character_select_answer_nothing_and_keep_the_session_alive() {
    // No WorldEntity yet, so the seam has no attacker guid to name and no combat state to change.
    // The durable calls still go out under the legacy zero actor and the client gets no attack
    // message. The sentinel can only arrive if neither opcode replied or panicked.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_ATTACKSTOP {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_CHAR_ENUM {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_ENUM(_) => {}
        other => panic!("expected the sentinel (no melee reply at character select), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.melee.stop_attacks.lock().unwrap().as_slice(),
        &[0],
        "the stop still reaches the durable seam under the legacy zero actor"
    );
}

#[test]
fn attackswing_desync_error_is_session_fatal() {
    // A desync-classified start_attack failure (the player's OWN entity is gone) must PROPAGATE
    // as session-fatal: run_world_session returns Err and the socket tears down for a clean relog.
    // Only the socket half is proved here; which failures are fatal, and which answer a refusal
    // instead, belongs to the melee seam's own tests.
    let mut s = quest_store();
    s.melee.start_attack_error = Some("no live entity for guid 1".into());
    let store = std::sync::Arc::new(s);
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    // Roll enter_world by hand: the server thread must RETURN the session result (not unwrap it).
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    CMSG_ATTACKSWING {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let result = server.join().unwrap();
    let err = result.expect_err("a desync on attackswing must end the session with an error");
    assert!(
        format!("{err:#}").contains("desync"),
        "the error should carry the desync context, got: {err:#}"
    );
    drop(client);
}

// ── Smaller mappings: WHO, buyback slots, trainer buy, talents, gossip select, chat ─────

#[test]
fn who_reply_lists_every_online_player_with_guild_level_and_zone() {
    let mut s = quest_store();
    s.guild.guild_memberships = vec![codec::GuildMemberView {
        character_guid: 2,
        guild_id: 7,
        name: "Alpha".into(),
        ..Default::default()
    }];
    s.guild.guilds = vec![codec::GuildView {
        guild_id: 7,
        name: "Boundary Test".into(),
        ..Default::default()
    }];
    s.characters = vec![
        // The requester. Human like Alpha/Bravo (so the team gate passes them), but a class
        // outside the request's `class_mask` — the requester is not exempt from its own filters,
        // so this keeps the assertions below at exactly the two matches.
        codec::CharacterView {
            guid: 1,
            name: "Tester".into(),
            race: 1,
            class: 4,
            level: 10,
            zone_id: 12,
            ..Default::default()
        },
        codec::CharacterView {
            guid: 2,
            name: "Alpha".into(),
            race: 1,
            class: 1,
            level: 5,
            zone_id: 12,
            ..Default::default()
        },
        codec::CharacterView {
            guid: 3,
            name: "Bravo".into(),
            race: 1,
            class: 1,
            level: 60,
            zone_id: 12,
            ..Default::default()
        },
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_WHO {
        minimum_level: Level::new(1),
        maximum_level: Level::new(60),
        player_name: String::new(),
        guild_name: String::new(),
        race_mask: 1 << 1,  // Human
        class_mask: 1 << 1, // Warrior
        zones: Vec::new(),
        search_strings: Vec::new(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // RAW-encoded (codec::build_who_response_raw): gtker's typed reader assumes the wrong 5875
    // layout (see that builder's doc comment), so this reads the cmangos body by hand.
    let (opcode, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(opcode, codec::social::SMSG_WHO_OPCODE);
    let online_players = u32::from_le_bytes(body[4..8].try_into().unwrap());
    assert_eq!(online_players, 2);
    let mut rest = &body[8..];
    for (name, guild, level) in [("Alpha", "Boundary Test", 5u32), ("Bravo", "", 60)] {
        let name_end = rest.iter().position(|&b| b == 0).unwrap();
        assert_eq!(std::str::from_utf8(&rest[..name_end]).unwrap(), name);
        rest = &rest[name_end + 1..];
        let guild_end = rest.iter().position(|&b| b == 0).unwrap();
        assert_eq!(std::str::from_utf8(&rest[..guild_end]).unwrap(), guild);
        rest = &rest[guild_end + 1..];
        assert_eq!(u32::from_le_bytes(rest[0..4].try_into().unwrap()), level);
        rest = &rest[16..]; // level, class, race, zone: u32 each
    }
    assert!(rest.is_empty(), "exactly two listed rows");
    drop(client);
    server.join().unwrap();
}

#[test]
fn login_replays_a_persisted_buyback_ring_after_the_login_sequence() {
    // The ring survives logout, so world entry rebuilds the tab: one fabricated item CREATE per
    // entry, then the raw descriptor update. (An EMPTY ring emits nothing — every other login test
    // reads the login sequence and then EOF, which is that case.)
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            vendor: VendorState {
                buyback_ring: vec![(2589, 5, 120, 0), (4540, 1, 30, 0)],
                ..base.vendor
            },
            ..base
        }
    });
    let (mut client, _c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    for _ in 0..2 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_) => {}
            other => panic!("expected a fabricated buyback item CREATE, got {other}"),
        }
    }
    // The descriptor update is a hand-rolled partial VALUES mask gtker cannot decode; the frame is
    // consumed either way, and EOF after it proves nothing else was sent.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    assert!(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err());
    drop(client);
    server.join().unwrap();
}

#[test]
fn buyback_maps_the_wire_slot_enum_to_zero_based_ring_slots() {
    // BuybackSlot rides as 69..=81 on the wire; the store reducer takes 0-based ring slots —
    // Slot1 (69) → 0, Slot13 (81) → 12.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_BUYBACK_ITEM {
        guid: Guid::new(99),
        slot: BuybackSlot::Slot1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_BUYBACK_ITEM {
        guid: Guid::new(99),
        slot: BuybackSlot::Slot13,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // 248: a successful buyback now pushes the refreshed tab view (one raw VALUES per call —
    // the mock ring is empty, so no item CREATEs). Consume both frames before EOF; gtker cannot
    // DECODE a hand-rolled partial VALUES mask (no OBJECT_FIELD_TYPE — the raw path's whole
    // reason to exist), so tolerate the parse error: the frame bytes are consumed either way.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.vendor.bought_back.lock().unwrap().as_slice(),
        &[(99, 0), (99, 12)]
    );
}

#[test]
fn trainer_buy_success_replies_succeeded_then_pushes_the_learned_spell() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: 1234,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(m) => {
            assert_eq!(m.guid.guid(), 70);
            assert_eq!(m.id, 1234);
        }
        other => panic!("expected SMSG_TRAINER_BUY_SUCCEEDED, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 1234),
        other => panic!("expected SMSG_LEARNED_SPELL, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// A RIDING purchase teaches a skill, not a spell. Its trainer-list id is a marker with no Spell.dbc row,
/// so the buy confirms and stops — pushing it as a learned spell would hand the client an id it cannot
/// resolve. The skill pane still moves, from the `game_player_skill` relay.
#[test]
fn a_riding_buy_confirms_without_echoing_the_offering_as_a_learned_spell() {
    let mut s = quest_store();
    s.trainer
        .trainer_offer_skill_lines
        .insert(50132, lyracore_shared::trainer::RIDING_SKILL_LINE);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: 50132,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(m) => assert_eq!(m.id, 50132),
        other => panic!("expected SMSG_TRAINER_BUY_SUCCEEDED, got {other}"),
    }
    // The follow-up whose reply we DO expect, proving the buy emitted nothing further.
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(_) => {}
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => {
            panic!(
                "a riding buy must not echo marker {} as a learned spell",
                m.id
            )
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// A Warrior buying Plate Mail at 40. The book update alone leaves the client tinting every plate
/// piece red, so the buy also refreshes the ARMOR mask from the post-buy spellbook.
#[test]
fn a_proficiency_buy_pushes_the_refreshed_armor_mask_after_the_learned_spell() {
    let mut s = quest_store();
    // The passive the buy granted, and the class the mask is derived for.
    s.character.learned_spells =
        vec![lyracore_shared::constants::armor_proficiency::PLATE_PASSIVE_SPELL_ID];
    s.characters = vec![codec::CharacterView {
        guid: 1,
        class: 1,
        level: 40,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: lyracore_shared::constants::armor_proficiency::PLATE_TRAINER_SPELL_ID,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(m) => assert_eq!(m.id, 7109),
        other => panic!("expected SMSG_TRAINER_BUY_SUCCEEDED, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 7109),
        other => panic!("expected SMSG_LEARNED_SPELL, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SET_PROFICIENCY(m) => {
            assert_eq!(
                m.class.as_int(),
                4,
                "a proficiency buy refreshes ARMOR only"
            );
            assert!(
                m.item_sub_class_mask & (1 << lyracore_shared::item::armor_subclass::PLATE) != 0,
                "the trained plate bit must be set: {:#x}",
                m.item_sub_class_mask
            );
        }
        other => panic!("expected SMSG_SET_PROFICIENCY, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// An ordinary ability purchase changes nothing about what the Character may wear, so it must not
/// resend a proficiency mask.
#[test]
fn an_ordinary_trainer_buy_pushes_no_proficiency_mask() {
    let mut s = quest_store();
    s.characters = vec![codec::CharacterView {
        guid: 1,
        class: 1,
        level: 40,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: 1234,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(_) => {}
        other => panic!("expected SMSG_TRAINER_BUY_SUCCEEDED, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 1234),
        other => panic!("expected SMSG_LEARNED_SPELL, got {other}"),
    }
    // A follow-up whose reply we DO expect, proving the buy emitted nothing further.
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(_) => {}
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn trainer_buy_rank_upgrade_supersedes_the_previous_rank_spell() {
    // A trainer buy whose chain prev is already known sends SMSG_SUPERCEDED_SPELL, not
    // SMSG_LEARNED_SPELL — the client REPLACES the old rank's book entry (vanilla), mirroring the
    // talent rank-upgrade path's cmangos wire order (OLD rides the first u16 slot).
    let mut s = quest_store();
    s.trainer.trainer_superseded = Some(1233); // rank 1 known; buying 1234 (rank 2) supersedes it
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: 1234,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(m) => {
            assert_eq!(m.guid.guid(), 70);
            assert_eq!(m.id, 1234);
        }
        other => panic!("expected SMSG_TRAINER_BUY_SUCCEEDED, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(m) => {
            assert_eq!(
                m.new_spell_id, 1233,
                "first wire slot carries the OLD rank (cmangos order)"
            );
            assert_eq!(
                m.old_spell_id, 1234,
                "second wire slot carries the NEW rank"
            );
        }
        other => panic!("expected SMSG_SUPERCEDED_SPELL, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn every_module_refusal_has_one_client_failure_reason() {
    use lyracore_shared::trainer::TrainerRefusal;
    let expected = |refusal| match refusal {
        TrainerRefusal::NotEnoughMoney => TrainingFailureReason::NotEnoughMoney,
        TrainerRefusal::LevelTooLow | TrainerRefusal::PreviousRankMissing => {
            TrainingFailureReason::NotEnoughSkill
        }
        TrainerRefusal::Unavailable | TrainerRefusal::NotOffered | TrainerRefusal::AlreadyKnown => {
            TrainingFailureReason::Unavailable
        }
    };
    for refusal in TrainerRefusal::ALL {
        let mut s = quest_store();
        s.trainer.trainer_buy_refusal = Some(refusal);
        let store = std::sync::Arc::new(s);
        let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
        CMSG_TRAINER_BUY_SPELL {
            guid: Guid::new(70),
            id: 1234,
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(m) => {
                assert_eq!(m.error, expected(refusal), "{refusal:?}");
                assert_eq!(m.id, 1234);
            }
            other => panic!("expected SMSG_TRAINER_BUY_FAILED, got {other}"),
        }
        drop(client);
        server.join().unwrap();
    }
}

/// A reducer timeout leaves the durable result unknown, so it must not reach the client as a
/// gameplay Refusal that says the purchase did not happen.
#[test]
fn a_trainer_reducer_timeout_is_not_answered_as_a_refusal() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            trade_error: Some("gw_trainer_buy reducer timed out after 10s".into()),
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        result_tx
            .send(run_world_session(server_end, server_store.clone()))
            .unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);

    CMSG_TRAINER_BUY_SPELL {
        guid: Guid::new(70),
        id: 1234,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let error = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("an unknown buy outcome must end the session promptly")
        .expect_err("a timed-out trainer reducer must be session-fatal");
    assert!(format!("{error:#}").contains("timed out"));
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the socket closes instead of claiming the purchase failed"
    );
}

#[test]
fn learn_talent_with_a_grant_spell_pushes_learned_spell() {
    // An ability talent (grant_spell_id != 0) + a successful learn → SMSG_LEARNED_SPELL(grant) so
    // the new button is usable without a relog.
    let mut s = quest_store();
    s.trainer.talent_grant = 2098;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 2098),
        other => panic!("expected SMSG_LEARNED_SPELL, got {other}"),
    }
    // The CHARACTER_POINTS1 VALUES push follows (raw read — the dirty_reset partial deliberately
    // omits OBJECT_FIELD_TYPE, which gtker's typed reader refuses).
    assert_eq!(read_raw_frame(&mut client, &mut c_dec).0, 0x00A9);
    drop(client);
    server.join().unwrap();
}

#[test]
fn learn_talent_passive_pushes_rank_spell_and_points() {
    // A PASSIVE pick must still refresh the 1.12 TalentFrame live: SMSG_LEARNED_SPELL for the
    // RANK-SPELL the module taught (the pane derives shown ranks from known rank-spells —
    // SPELLS_CHANGED) followed by the PLAYER_CHARACTER_POINTS1 partial VALUES
    // (CHARACTER_POINTS_CHANGED). The old behavior sent NOTHING → pane frozen until relog.
    let mut s = quest_store(); // talent_grant = 0
    s.trainer.talent_pane = (7777, 0, 2); // rank-spell 7777 taught, no superseded prev, 2 points left
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_LEARNED_SPELL(m) => assert_eq!(m.id, 7777),
        other => panic!("expected SMSG_LEARNED_SPELL(rank-spell), got {other}"),
    }
    // Raw read: the dirty_reset partial VALUES omits OBJECT_FIELD_TYPE (gtker's typed reader refuses).
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec).0,
        0x00A9,
        "the CHARACTER_POINTS1 VALUES push"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn learn_talent_rank_upgrade_supersedes_the_previous_rank_spell() {
    // Rank N>1: the previous rank's spell is REPLACED in the book — SMSG_SUPERCEDED_SPELL with the
    // cmangos wire order (OLD rides the first u16 slot), mirroring the trainer rank-upgrade path.
    let mut s = quest_store();
    s.trainer.talent_pane = (7778, 7777, 1); // new rank-spell 7778 supersedes 7777, 1 point left
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LEARN_TALENT {
        talent: Talent::BurningSoul,
        requested_rank: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(m) => {
            assert_eq!(
                m.new_spell_id, 7777,
                "first wire slot carries the OLD rank (cmangos order)"
            );
            assert_eq!(
                m.old_spell_id, 7778,
                "second wire slot carries the NEW rank"
            );
        }
        other => panic!("expected SMSG_SUPERCEDED_SPELL, got {other}"),
    }
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec).0,
        0x00A9,
        "the CHARACTER_POINTS1 VALUES push"
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn list_inventory_opens_the_vendor_window_over_the_socket() {
    let mut s = quest_store();
    s.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_LIST_INVENTORY {
        guid: Guid::new(80),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(&body[0..8], &80u64.to_le_bytes());
    assert_eq!(body[8], 1, "one stocked item");
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_a_vendor_opens_the_inventory_window() {
    // Option 0 on a stocked NPC is "browse goods" → the RAW SMSG_LIST_INVENTORY, same as the
    // direct CMSG_LIST_INVENTORY path.
    let mut s = quest_store();
    s.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 80);
    assert_eq!(menu.gossips[0].message, "I'd like to browse your goods.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(80),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(
        &body[0..8],
        &80u64.to_le_bytes(),
        "the vendor window names the NPC"
    );
    drop(client);
    server.join().unwrap();
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn gossip_select_on_an_innkeeper_binds_home_and_completes() {
    // A non-vendor innkeeper's "Make this inn your home." is option 0 → bind_home + GOSSIP_COMPLETE.
    let mut s = quest_store();
    s.npc.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 81);
    assert_eq!(menu.gossips[0].message, "Make this inn your home.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(81),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store
            .npc
            .home_bound
            .load(std::sync::atomic::Ordering::SeqCst),
        "bind_home must have run"
    );
}

#[test]
fn gossip_select_of_any_other_option_completes_without_binding() {
    // Farewell (option 1 on an innkeeper NPC) → GOSSIP_COMPLETE only; no bind, no vendor window.
    let mut s = quest_store();
    s.npc.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 81);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(81),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
}

// --- Imported gossip menu options + multi-slot npc_text -------------------------------------------

/// Open `npc`'s gossip window and drain the menu, so a following `CMSG_GOSSIP_SELECT_OPTION` has the
/// snapshot it resolves against — a click with nothing open selects nothing.
fn gossip_hello(
    client: &mut UnixStream,
    enc: &mut EncrypterHalf,
    dec: &mut DecrypterHalf,
    npc: u64,
) -> wow_world_messages::vanilla::SMSG_GOSSIP_MESSAGE {
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(npc),
    }
    .write_encrypted_client(&mut *client, enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut *client, dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => *m,
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
}

/// A shorthand imported option builder for the gossip mock tests.
fn opt(icon: u32, text: &str, action: u32) -> codec::GossipOptionView {
    codec::GossipOptionView {
        icon,
        text: text.to_string(),
        action,
        ..Default::default()
    }
}

#[test]
fn taxi_status_query_returns_the_persisted_bit_without_opening() {
    let mut s = quest_store();
    s.taxi.taxi_status = Some(codec::TaxiNodeStatusView {
        npc_guid: 90,
        known: false,
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_TAXINODE_STATUS_QUERY {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TAXINODE_STATUS(status) => {
            assert_eq!(status.guid.guid(), 90);
            assert!(!status.taxi_mask_node_known);
        }
        other => panic!("expected SMSG_TAXINODE_STATUS, got {other}"),
    }
    assert_eq!(
        *store.taxi.taxi_calls.lock().unwrap(),
        vec![("status", 1, 90)]
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn direct_taxi_query_and_taxi_gossip_share_one_open_operation() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Show me your flight routes.", gossip_option::TAXI)];
    s.taxi.taxi_map = Some(codec::TaxiMapView {
        npc_guid: 90,
        source_client_node_id: 255,
        available_client_node_ids: vec![255, 256],
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_TAXIQUERYAVAILABLENODES {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOWTAXINODES(map) => {
            assert_eq!(map.guid.guid(), 90);
            assert_eq!(map.nearest_node, 255);
            assert_eq!(map.nodes.len(), 8);
            assert_eq!(map.nodes[7], 0xC000_0000);
        }
        other => panic!("expected SMSG_SHOWTAXINODES, got {other}"),
    }

    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SHOWTAXINODES(map) => {
            assert_eq!(map.nearest_node, 255);
            assert_eq!(map.nodes[7], 0xC000_0000);
        }
        other => panic!("expected SMSG_SHOWTAXINODES, got {other}"),
    }

    assert_eq!(
        *store.taxi.taxi_calls.lock().unwrap(),
        vec![("open", 1, 90), ("open", 1, 90)]
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn activate_taxi_gameplay_refusal_replies_and_keeps_the_socket_alive() {
    let mut s = quest_store();
    s.taxi.taxi_activation = codec::TaxiActivationResult {
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
    };
    s.taxi.taxi_status = Some(codec::TaxiNodeStatusView {
        npc_guid: 90,
        known: true,
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ACTIVATETAXI {
        guid: Guid::new(90),
        source_node: 255,
        destination_node: 256,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply) => assert_eq!(
            reply.reply,
            wow_world_messages::vanilla::ActivateTaxiReply::NotEnoughMoney
        ),
        other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
    }
    assert_eq!(
        *store.taxi.taxi_activation_inputs.lock().unwrap(),
        vec![(1, 90, 255, 256)]
    );

    // A gameplay refusal is a normal packet result, not a world-session failure.
    CMSG_TAXINODE_STATUS_QUERY {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_TAXINODE_STATUS(_)
    ));
    drop(client);
    server.join().unwrap();
}

#[test]
fn activate_taxi_success_round_trips_over_the_encrypted_socket() {
    let mut s = quest_store();
    s.taxi.taxi_activation = codec::TaxiActivationResult {
        result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
    };
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_ACTIVATETAXI {
        guid: Guid::new(90),
        source_node: 255,
        destination_node: 256,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(reply) => assert_eq!(
            reply.reply,
            wow_world_messages::vanilla::ActivateTaxiReply::Ok
        ),
        other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
    }
    assert_eq!(
        *store.taxi.taxi_activation_inputs.lock().unwrap(),
        vec![(1, 90, 255, 256)]
    );

    drop(client);
    server.join().unwrap();
}

#[test]
fn taxi_gossip_transport_failure_ends_the_world_session() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Show me your flight routes.", gossip_option::TAXI)];
    s.taxi.taxi_error = Some("taxi reducer transport disconnected: channel closed".into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    server
        .join()
        .expect_err("a broken taxi reducer transport must end the world session");
}

#[test]
fn gossip_hello_renders_imported_options_verbatim_with_a_trailing_farewell() {
    // The 217 acceptance criterion: an Elwynn-innkeeper-shaped NPC with 3 imported options (chat,
    // browse goods, make-home) renders them VERBATIM (real dump text, not the hardcoded fallback
    // strings) — the vendor/innkeeper flags are ignored entirely once options are imported.
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![
        opt(0, "Well met, traveler.", gossip_option::GOSSIP),
        opt(1, "I'd like to browse your goods.", gossip_option::VENDOR),
        opt(
            0,
            "I'd like to stay here a while.",
            gossip_option::INNKEEPER,
        ),
    ];
    // Fallback signals present too — must be ignored while options are imported.
    s.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 1,
        ..Default::default()
    }];
    s.npc.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                4,
                "3 imported + a trailing Farewell: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[0].message, "Well met, traveler.");
            assert_eq!(m.gossips[1].message, "I'd like to browse your goods.");
            assert_eq!(m.gossips[2].message, "I'd like to stay here a while.");
            assert_eq!(m.gossips[3].message, "Farewell.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_an_imported_vendor_option_opens_the_inventory_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(1, "Browse.", gossip_option::VENDOR)];
    s.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(
        &body[0..8],
        &90u64.to_le_bytes(),
        "the vendor window names the NPC"
    );
    drop(client);
    server.join().unwrap();
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn gossip_select_on_an_imported_innkeeper_option_binds_home() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![
        opt(0, "Chat.", gossip_option::GOSSIP),
        opt(0, "Stay here.", gossip_option::INNKEEPER),
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store
            .npc
            .home_bound
            .load(std::sync::atomic::Ordering::SeqCst),
        "bind_home must have run"
    );
}

#[test]
fn the_same_option_row_reaches_the_module_by_row_id_from_either_viewer() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let menu = || {
        let mut gated = opt(0, "About that favor...", gossip_option::GOSSIP);
        gated.row_id = 4001;
        gated.cond_type = gossip_condition::QUEST_TAKEN;
        gated.cond_value1 = 60;
        let mut always = opt(0, "Stay here.", gossip_option::INNKEEPER);
        always.row_id = 4002;
        vec![gated, always]
    };
    // Viewer A has not taken quest 60 → the gated row is hidden, so "Stay here." renders at 0.
    let mut a = quest_store();
    a.npc.gossip_opts = menu();
    let a = std::sync::Arc::new(a);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(a.clone(), 1);
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[0].message, "Stay here.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    drop(client);
    server.join().unwrap();

    // Viewer B HAS taken it → the gated row renders first and pushes "Stay here." to position 1.
    let mut b = quest_store();
    b.npc.gossip_opts = menu();
    b.quest.quest_log = vec![(60, false)].into();
    let b = std::sync::Arc::new(b);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(b.clone(), 1);
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[1].message, "Stay here.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    drop(client);
    server.join().unwrap();

    let (a_pos, a_row) = a.npc.gossip_selects.lock().unwrap()[0];
    let (b_pos, b_row) = b.npc.gossip_selects.lock().unwrap()[0];
    assert_ne!(a_pos, b_pos, "the POSITION differs between the two viewers");
    assert_eq!(
        (a_row, b_row),
        (4002, 4002),
        "the row_id is the same option for both"
    );
}

#[test]
fn a_quest_taken_while_the_window_is_open_does_not_shift_the_click() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    let mut gated = opt(0, "About that favor...", gossip_option::GOSSIP);
    gated.row_id = 4001;
    gated.cond_type = gossip_condition::QUEST_TAKEN;
    gated.cond_value1 = 60;
    let mut inn = opt(0, "Stay here.", gossip_option::INNKEEPER);
    inn.row_id = 4002;
    s.npc.gossip_opts = vec![gated, inn];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    // Rendered while the quest is untaken: the gated line is hidden, "Stay here." is position 0.
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[0].message, "Stay here.");
    // The player accepts quest 60 elsewhere (another window, a party member's turn-in) — a fresh
    // filter would now put the gated line at 0 and push "Stay here." to 1.
    store.quest.quest_log.lock().unwrap().push((60, false));
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store
            .npc
            .home_bound
            .load(std::sync::atomic::Ordering::SeqCst),
        "the click must still select the innkeeper line the player was shown"
    );
    assert_eq!(
        store.npc.gossip_selects.lock().unwrap()[0],
        (0, 4002),
        "and the module hears the row the player saw, not the one that moved into that slot"
    );
}

#[test]
fn a_select_with_no_open_menu_just_closes_the_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Stay here.", gossip_option::INNKEEPER)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    // Same position, a DIFFERENT npc — the open menu is 90's, so this selects nothing.
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(91),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        store.npc.gossip_selects.lock().unwrap()[0].1,
        codec::SYNTHESIZED_ROW_ID,
        "no imported row was selected"
    );
}

#[test]
fn an_imported_menu_missing_its_vendor_row_still_reaches_the_stock() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "What is Children's Week?", gossip_option::GOSSIP)];
    s.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        display_id: 6353,
        buy_price: 25,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(menu.gossips.len(), 3, "chat + browse goods + Farewell");
    assert_eq!(menu.gossips[0].message, "What is Children's Week?");
    assert_eq!(menu.gossips[1].message, "I'd like to browse your goods.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let (op, body) = read_raw_frame(&mut client, &mut c_dec);
    assert_eq!(op, codec::SMSG_LIST_INVENTORY_OPCODE);
    assert_eq!(&body[0..8], &90u64.to_le_bytes());
    drop(client);
    server.join().unwrap();
}

#[test]
fn an_imported_menu_missing_its_bind_row_still_offers_the_hearth() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Tell me about the inn.", gossip_option::GOSSIP)];
    s.npc.innkeeper = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(menu.gossips[1].message, "Make this inn your home.");
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
}

/// A `quest_store()` fixture whose logged-in character (guid 1) reports `level`, so
/// `filtered_gossip_options`' level gate has something to read (`quest_store()` itself leaves
/// `characters` empty, which reads as level 0 — every below-10 test can lean on that default).
fn quest_store_at_level(level: u8) -> WorldFake {
    {
        let base = quest_store();
        WorldFake {
            characters: vec![codec::CharacterView {
                guid: 1,
                level,
                ..Default::default()
            }],
            ..base
        }
    }
}

#[test]
fn gossip_hello_hides_unlearn_talents_below_level_10() {
    // The imported "I wish to unlearn my talents." row (reclassified by the importer to
    // `UNLEARNTALENTS`, since the raw dump column never carries it) must not render for a character
    // who cannot yet have a talent point.
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store_at_level(5);
    s.npc.gossip_opts = vec![
        opt(0, "I require warrior training.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                2,
                "training + trailing Farewell only, no unlearn option: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[0].message, "I require warrior training.");
            assert_eq!(m.gossips[1].message, "Farewell.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hello_shows_unlearn_talents_at_level_10_and_select_routes_to_reset_talents() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store_at_level(10);
    s.npc.gossip_opts = vec![
        opt(0, "I require warrior training.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
    ];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(
                m.gossips.len(),
                3,
                "training + unlearn + trailing Farewell: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[1].message, "I wish to unlearn my talents.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    // Click it (index 1, same list HELLO just rendered) — must route to reset_talents, not just
    // close the window inert.
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let calls = store.trainer.reset_talents_calls.lock().unwrap();
    assert_eq!(
        calls.as_slice(),
        &[(7, 1, 90)],
        "reset_talents must have been called with (account_id, self_guid, trainer_guid)"
    );
}

#[test]
fn gossip_hello_shows_a_quest_gated_option_once_the_quest_is_taken() {
    // The socket-level contract: the player's quest state reaches the gossip filter through the
    // quest seam's `quest_gate_state`, and the gated row is assembled into the gossip message the
    // client actually receives. The hidden case is covered by the position-alignment test below.
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "About that favor...", gossip_option::GOSSIP)];
    s.npc.gossip_opts[0].cond_type = gossip_condition::QUEST_TAKEN;
    s.npc.gossip_opts[0].cond_value1 = 60;
    s.quest.quest_log = vec![(60, false)].into(); // taken, not yet turned in
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 2, "{:?}", m.gossips);
            assert_eq!(m.gossips[0].message, "About that favor...");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hello_and_select_option_stay_position_aligned_under_a_hidden_option() {
    // The sharpest trap (danger-zones-adjacent): 3 imported options where the MIDDLE one is
    // quest-gated and hidden. HELLO sends only 2 lines (positions 0,1); a SELECT of position 1 MUST
    // route to the THIRD raw option (innkeeper), not the hidden middle one — proving
    // `filtered_gossip_options` re-derives the IDENTICAL list rather than indexing the raw rows.
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.npc.gossip_opts = vec![
        opt(0, "Chat.", gossip_option::GOSSIP), // raw index 0 -> rendered index 0
        opt(0, "Hidden favor.", gossip_option::GOSSIP), // raw index 1 -> HIDDEN (quest-gated)
        opt(0, "Stay here.", gossip_option::INNKEEPER), // raw index 2 -> rendered index 1
    ];
    s.npc.gossip_opts[1].cond_type = gossip_condition::QUEST_TAKEN;
    s.npc.gossip_opts[1].cond_value1 = 60; // never taken in this store
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 3, "2 visible + Farewell: {:?}", m.gossips); // hidden option excluded
            assert_eq!(m.gossips[0].message, "Chat.");
            assert_eq!(m.gossips[1].message, "Stay here.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    // Click rendered position 1 ("Stay here.") — must bind home, NOT be swallowed by the hidden
    // middle option that was never actually sent to the client.
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 1,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE => {}
        other => panic!("expected SMSG_GOSSIP_COMPLETE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(
        store
            .npc
            .home_bound
            .load(std::sync::atomic::Ordering::SeqCst),
        "position 1 must resolve to the innkeeper option, not the hidden one"
    );
}

#[test]
fn npc_text_query_ships_the_imported_8_slot_view() {
    let mut view = codec::NpcTextView::default();
    view.slots[0] = (
        "Well met.".to_string(),
        "Well met, traveler.".to_string(),
        0.6,
    );
    view.slots[3] = (
        "Watch yourself.".to_string(),
        "Watch yourself.".to_string(),
        0.4,
    );
    let mut s = quest_store();
    s.npc.npc_text_view = Some(view);
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_NPC_TEXT_QUERY {
        text_id: 77,
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(u) => {
            assert_eq!(u.text_id, 77);
            assert_eq!(u.texts[0].texts[0], "Well met.");
            assert_eq!(u.texts[0].probability, 0.6);
            assert_eq!(u.texts[3].texts[0], "Watch yourself.");
            assert_eq!(u.texts[3].probability, 0.4);
            assert_eq!(u.texts[1].probability, 0.0); // untouched slot stays silent
        }
        other => panic!("expected SMSG_NPC_TEXT_UPDATE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn messagechat_say_and_yell_route_to_chat_types_0_and_1() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hi".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Yell,
        language: Language::Universal,
        message: "HEY".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // no reply on success — the speaker sees their line via the broadcast relay
    server.join().unwrap();
    assert_eq!(
        store.speech.chats.lock().unwrap().as_slice(),
        &[(0, 0, "hi".to_string()), (1, 0, "HEY".to_string())],
        "Say → type 0, Yell → type 1, language threaded"
    );
}

#[test]
fn messagechat_emote_routes_to_chat_type_3() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Emote,
        language: Language::Common,
        message: "waves wildly.".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client); // no reply on success — the speaker sees their line via the broadcast relay
    server.join().unwrap();
    assert_eq!(
        store.speech.chats.lock().unwrap().as_slice(),
        &[(3, 7, "waves wildly.".to_string())],
        "/e → type 3, language threaded to the Module (which stores it as Universal)"
    );
}

#[test]
fn messagechat_dot_say_diverts_to_gm_command_never_touching_chat() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".heal".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful dot-command
        other => panic!("expected the sentinel (no reply on gm_command success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.speech.gm_commands.lock().unwrap().as_slice(),
        &[("TESTER".to_string(), ".heal".to_string())],
        "the proof-validated Account name and raw dot-command must reach the Store together"
    );
    assert!(
        store.speech.chats.lock().unwrap().is_empty(),
        "a dot-command must NEVER reach send_chat"
    );
}

#[test]
fn messagechat_non_dot_say_is_byte_identical_to_before_223() {
    // The 223 divert must be a no-op for ordinary chat: a Say line NOT starting with '.' still routes
    // to send_chat exactly as before, and never touches gm_command.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hi".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.speech.chats.lock().unwrap().as_slice(),
        &[(0u8, 0u8, "hi".to_string())]
    );
    assert!(
        store.speech.gm_commands.lock().unwrap().is_empty(),
        "a plain Say line must never reach gm_command"
    );
}

#[test]
fn messagechat_dot_say_error_relays_a_system_chat_line_to_the_sender_only() {
    // A rejected dot-command (bad gm_level, unknown command, bad args) is relayed back
    // to the SENDER as a System SMSG_MESSAGECHAT carrying the module's raw message VERBATIM — no
    // "reducer failed" wrapper prefix, no broadcast, no game_chat_event row.
    let mut s = quest_store();
    s.speech.gm_command_error = Some("permission denied".to_string());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".god".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(m) => {
            assert_eq!(m.message, "permission denied");
            assert!(
                matches!(
                    m.chat_type,
                    wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::System { .. }
                ),
                "expected a System chat line, got {:?}",
                m.chat_type
            );
        }
        other => panic!("expected SMSG_MESSAGECHAT System, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store.speech.chats.lock().unwrap().is_empty());
}

#[test]
fn force_run_speed_change_ack_is_swallowed_with_no_reply_and_no_session_teardown() {
    // The client's ack to our `.speed`-triggered SMSG_FORCE_RUN_SPEED_CHANGE must be
    // consumed cleanly (no reply, no desync/disconnect) — proven by a sentinel opcode right after it
    // still getting its normal reply on the SAME session.
    use wow_world_messages::vanilla::{
        MovementInfo, MovementInfo_MovementFlags, Vector3d, CMSG_FORCE_RUN_SPEED_CHANGE_ACK,
    };
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_FORCE_RUN_SPEED_CHANGE_ACK {
        guid: Guid::new(1),
        counter: 1,
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 0,
            position: Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: 0.0,
            fall_time: 0.0,
        },
        new_speed: 21.0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // the ack produced no reply of its own
        other => panic!("expected the sentinel (ack swallowed), got {other}"),
    }
    drop(client);
    server.join().unwrap(); // the session ran to a clean close, not a desync teardown
}

#[test]
fn messagechat_whisper_to_an_unknown_player_replies_player_not_found() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Whisper {
            target_player: "Ghost".into(),
        },
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec)
        .expect("a whisper to nobody must answer SMSG_CHAT_PLAYER_NOT_FOUND, and nothing arrived")
    {
        ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(m) => assert_eq!(m.name, "Ghost"),
        other => panic!("expected SMSG_CHAT_PLAYER_NOT_FOUND, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert!(store.chat.realm_whispers.lock().unwrap().is_empty());
}

/// `quest_store` plus the session's own Character row, which the `sync` sentinel needs.
fn chat_session_store() -> WorldFake {
    {
        let base = quest_store();
        WorldFake {
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Warrior".into(),
                ..Default::default()
            }],
            ..base
        }
    }
}

/// `/afk Brb` becomes one `set_away` request for the session's own Character, with no reply: the
/// client prints its own notice and observers see PLAYER_FLAGS on the entity Relay.
#[test]
fn messagechat_afk_and_dnd_become_set_away_requests_from_the_sessions_character() {
    let store = std::sync::Arc::new(chat_session_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    sync(&mut client, &mut c_enc, &mut c_dec, |client, enc| {
        for (chat_type, message) in [
            (CMSG_MESSAGECHAT_ChatType::Afk, "Brb"),
            (CMSG_MESSAGECHAT_ChatType::Dnd, ""),
        ] {
            CMSG_MESSAGECHAT {
                chat_type,
                language: Language::Universal,
                message: message.into(),
            }
            .write_encrypted_client(&mut *client, &mut *enc)
            .unwrap();
        }
    });
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.chat.away_requests.lock().unwrap().clone(),
        vec![(1, 0x14, "Brb".to_string()), (1, 0x15, String::new())]
    );
}

/// cm:ChatHandler.cpp:801-815: `CMSG_CHAT_IGNORED` sends IGNORED to the Character whose line the
/// client dropped, carrying the ignorer's own name.
#[test]
fn chat_ignored_becomes_an_ignored_line_to_the_dropped_speaker() {
    let mut s = chat_session_store();
    s.chat.speaker_facts = Some(SpeakerFacts {
        race: 1,
        chat_tag: 1,
        name: "Warrior".to_string(),
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    sync(&mut client, &mut c_enc, &mut c_dec, |client, enc| {
        wow_world_messages::vanilla::CMSG_CHAT_IGNORED {
            guid: Guid::new(42),
        }
        .write_encrypted_client(client, enc)
        .unwrap();
    });
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(
        requests.as_slice(),
        &[(
            1,
            RealmChatRequest {
                kind: 0x16,
                language: 0,
                channel_name: String::new(),
                target_guid: 42,
                message: "Warrior".to_string(),
                speaker: SpeakerFacts {
                    race: 1,
                    chat_tag: 1,
                    name: "Warrior".to_string(),
                },
            }
        )]
    );
}

#[test]
fn messagechat_guild_becomes_one_realm_chat_request_from_the_sessions_character() {
    // `/g` reaches the Realm Chat path with the guid it entered the world with, the same shape as
    // `/p`. No reply on success: the speaker hears the line through the Relay like every member.
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Guild,
        language: Language::Common,
        message: "hello guild".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /g
        other => panic!("expected the sentinel (no reply on /g success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (speaker_guid, request) = &requests[0];
    assert_eq!(*speaker_guid, 1);
    assert_eq!(request.kind, lyracore_shared::chat::chat_kind::GUILD);
    assert_eq!(request.message, "hello guild");
    assert!(
        store.speech.chats.lock().unwrap().is_empty(),
        "a guild line never becomes a say line"
    );
}

#[test]
fn messagechat_officer_becomes_one_realm_chat_request_from_the_sessions_character() {
    // `/o` follows the same path as `/g` with its own Chat Kind.
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Officer,
        language: Language::Common,
        message: "officers only".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /o
        other => panic!("expected the sentinel (no reply on /o success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (speaker_guid, request) = &requests[0];
    assert_eq!(*speaker_guid, 1);
    assert_eq!(request.kind, lyracore_shared::chat::chat_kind::OFFICER);
    assert_eq!(request.message, "officers only");
}

fn human_speaker() -> SpeakerFacts {
    SpeakerFacts {
        race: 1,
        chat_tag: 0,
        name: String::new(),
    }
}

#[test]
fn messagechat_party_becomes_one_realm_chat_request_from_the_sessions_character() {
    // The session's `/p` reaches the Realm Chat path with the guid it entered the world with. No
    // reply on success: the speaker hears the line through the Relay like every other member.
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Common,
        message: "form up".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // nothing was sent for a successful /p
        other => panic!("expected the sentinel (no reply on /p success), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (speaker_guid, request) = &requests[0];
    assert_eq!(*speaker_guid, 1);
    assert_eq!(request.kind, lyracore_shared::chat::chat_kind::PARTY);
    assert_eq!(request.language, 7);
    assert_eq!(request.message, "form up");
    assert!(
        store.speech.chats.lock().unwrap().is_empty(),
        "a party line never becomes a say line"
    );
}

#[test]
fn messagechat_party_from_an_ungrouped_caller_replies_not_in_group() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    s.chat.realm_chat_outcome = Some(ChatOutcome::Refused(
        lyracore_shared::chat::ChatRefusal::NotInGroup,
    ));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(
                r.result,
                wow_world_messages::vanilla::PartyResult::NotInGroup
            );
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT(NotInGroup), got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

/// Character 1 in the world, with a Character row so `sync`'s sentinel is answered.
fn chat_store() -> WorldFake {
    {
        let base = quest_store();
        WorldFake {
            chat: ChatState {
                speaker_facts: Some(human_speaker()),
                ..base.chat
            },
            characters: vec![codec::CharacterView {
                guid: 1,
                name: "Tester".into(),
                ..Default::default()
            }],
            ..base
        }
    }
}

fn say_line(message: &str) -> CMSG_MESSAGECHAT {
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Common,
        message: message.into(),
    }
}

fn notification(message: ServerOpcodeMessage) -> String {
    match message {
        ServerOpcodeMessage::SMSG_NOTIFICATION(notice) => notice.notification,
        other => panic!("expected SMSG_NOTIFICATION, got {other}"),
    }
}

#[test]
fn a_flooding_session_is_muted_before_any_chat_durable_request() {
    // cm:Player.cpp:16344-16377: eleven fast lines mute the speaker for ten seconds. The twelfth
    // line and the party line after it answer the cmangos notice and reach no reducer.
    let store = std::sync::Arc::new(chat_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    for n in 0..12 {
        say_line(&format!("line {n}"))
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
    }
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Party,
        language: Language::Common,
        message: "form up".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            notification(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()),
            "You must wait 10 Second(s). before speaking again."
        );
    }
    sync(&mut client, &mut c_enc, &mut c_dec, |_, _| {});
    drop(client);
    server.join().unwrap();
    assert_eq!(store.speech.chats.lock().unwrap().len(), 11);
    assert!(store.chat.realm_chats.lock().unwrap().is_empty());
}

#[test]
fn a_muted_session_still_sends_addon_lines() {
    // cm:ChatHandler.cpp:113-119: the addon language is never flood-controlled.
    let store = std::sync::Arc::new(chat_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    for n in 0..12 {
        say_line(&format!("line {n}"))
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
    }
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Guild,
        language: Language::Addon,
        message: "LCTEST\tping".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    notification(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap());
    sync(&mut client, &mut c_enc, &mut c_dec, |_, _| {});
    drop(client);
    server.join().unwrap();
    let requests = store.chat.realm_chats.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1.language, 0xFFFF_FFFF);
}

#[test]
fn a_game_master_is_never_muted_for_flooding() {
    // cm:Player.cpp:16346-16348 skips the flood count for any account above SEC_PLAYER.
    let mut s = chat_store();
    s.chat.gm_level = 1;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        for n in 0..15 {
            say_line(&format!("line {n}"))
                .write_encrypted_client(&mut *c, &mut *e)
                .unwrap();
        }
    });
    drop(client);
    server.join().unwrap();
    assert_eq!(store.speech.chats.lock().unwrap().len(), 15);
}

#[test]
fn a_say_line_in_a_language_the_speaker_does_not_know_answers_the_vanilla_notice() {
    // cm:ChatHandler.cpp:107-110 with cm mangos.sql:4044.
    let mut s = chat_store();
    s.speech.send_chat_outcome = Some(ChatOutcome::Refused(
        lyracore_shared::chat::ChatRefusal::UnknownLanguage,
    ));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Orcish,
        message: "zug zug".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(
        notification(ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()),
        "You don't know that language"
    );
    sync(&mut client, &mut c_enc, &mut c_dec, |_, _| {});
    drop(client);
    server.join().unwrap();
}

#[test]
fn join_channel_runs_the_channel_op_as_the_sessions_character() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    wow_world_messages::vanilla::CMSG_JOIN_CHANNEL {
        channel_name: "Trade - City".into(),
        channel_password: String::new(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {} // YOU_JOINED returns on the Relay
        other => panic!("expected the sentinel (no reply on a successful join), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    let ops = store.channel.channel_ops.lock().unwrap();
    assert_eq!(ops.len(), 1);
    let (actor_guid, op, request) = &ops[0];
    assert_eq!((*actor_guid, *op), (1, 0));
    assert_eq!(request.channel_name, "Trade - City");
}

/// A refused join answers WRONG_PASSWORD 0x04 on the session's own socket.
#[test]
fn a_refused_join_answers_the_notice_on_the_session() {
    let mut s = quest_store();
    s.chat.speaker_facts = Some(human_speaker());
    s.channel.channel_outcome = Some(ChannelOutcome::Refused(
        lyracore_shared::channel::ChannelRefusal::WrongPassword,
    ));
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    wow_world_messages::vanilla::CMSG_JOIN_CHANNEL {
        channel_name: "Rx".into(),
        channel_password: "guess".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(notify) => {
            assert_eq!(
                notify.notify_type,
                wow_world_messages::vanilla::ChatNotify::WrongPasswordNotice
            );
            assert_eq!(notify.channel_name, "Rx");
        }
        other => panic!("expected SMSG_CHANNEL_NOTIFY, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

// ── Trainer dispatch (CMSG_TRAINER_LIST) ─────────────────────────────────────────────────────────

#[test]
fn trainer_list_replies_smsg_trainer_list_with_the_fixture_spells() {
    let mut s = quest_store();
    s.trainer.trainer_spells = vec![codec::TrainerSpellView {
        spell_id: 100,
        cost: 10,
        required_level: 1,
        player_level: 1,
        known: false,
        profession: false,
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_LIST {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TRAINER_LIST(list) => {
            assert_eq!(list.guid, Guid::new(70));
            assert_eq!(list.spells.len(), 1, "the fixture's one spell row");
            assert_eq!(list.spells[0].spell, 100);
        }
        other => panic!("expected SMSG_TRAINER_LIST, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn trainer_list_is_silently_dropped_for_a_player_the_trainer_does_not_serve() {
    let mut s = quest_store();
    s.trainer.trainer_refuses_class = true;
    s.trainer.trainer_spells = vec![codec::TrainerSpellView {
        spell_id: 100,
        cost: 10,
        required_level: 1,
        player_level: 1,
        known: false,
        profession: false,
    }];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_TRAINER_LIST {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The follow-up whose reply we DO expect. Gossip always answers, so reading it back proves the
    // trainer request emitted nothing — and it doubles as the "the NPC still talks to you" check:
    // the class gate removes the training service, not the creature.
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(70),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(_) => {}
        ServerOpcodeMessage::SMSG_TRAINER_LIST(_) => {
            panic!("a trainer that does not serve this class must send NO window")
        }
        other => panic!("expected only the gossip reply, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hides_the_train_and_unlearn_options_for_a_class_the_trainer_does_not_serve() {
    use lyracore_shared::constants::gossip_option;
    // Level 20 matters: the respec option is independently hidden below level 10, so at the default
    // fixture level this would pass without the class gate doing any work.
    let mut s = quest_store_at_level(20);
    s.npc.gossip_opts = vec![
        opt(0, "Well met, traveler.", gossip_option::GOSSIP),
        opt(1, "I would like to train.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
        opt(1, "I'd like to browse your goods.", gossip_option::VENDOR),
    ];
    s.trainer.trainer_refuses_class = true;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            let lines: Vec<&str> = m.gossips.iter().map(|g| g.message.as_str()).collect();
            assert!(
                !lines.contains(&"I would like to train."),
                "the train option must be hidden: {lines:?}"
            );
            assert!(
                !lines.contains(&"I wish to unlearn my talents."),
                "the respec option must be hidden too: {lines:?}"
            );
            // The NPC is not silenced — it still talks, and still sells.
            assert!(
                lines.contains(&"Well met, traveler."),
                "plain gossip lines survive: {lines:?}"
            );
            assert!(
                lines.contains(&"I'd like to browse your goods."),
                "the vendor line on the same NPC survives: {lines:?}"
            );
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_keeps_the_train_and_unlearn_options_for_a_class_the_trainer_serves() {
    use lyracore_shared::constants::gossip_option;
    // Same level as its counterpart, so the only difference between the two tests is the gate.
    let mut s = quest_store_at_level(20);
    s.npc.gossip_opts = vec![
        opt(0, "Well met, traveler.", gossip_option::GOSSIP),
        opt(1, "I would like to train.", gossip_option::TRAINER),
        opt(
            0,
            "I wish to unlearn my talents.",
            gossip_option::UNLEARNTALENTS,
        ),
        opt(1, "I'd like to browse your goods.", gossip_option::VENDOR),
    ];
    let store = std::sync::Arc::new(s); // trainer_refuses_class stays false (derive-Default)
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            let lines: Vec<&str> = m.gossips.iter().map(|g| g.message.as_str()).collect();
            assert!(
                lines.contains(&"I would like to train."),
                "a served class still gets the train option: {lines:?}"
            );
            assert!(
                lines.contains(&"I wish to unlearn my talents."),
                "and the respec option: {lines:?}"
            );
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

// ── Death/resurrection dispatch (CMSG_REPOP_REQUEST / CMSG_RECLAIM_CORPSE /
// CMSG_RESURRECT_RESPONSE / CMSG_SELF_RES / CMSG_SPIRIT_HEALER_ACTIVATE) ───────────────────────────────────────────

#[test]
fn repop_request_dispatches_repop_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_REPOP_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // repop's revive replicates via the entity VALUES relay, not a direct SMSG here
    server.join().unwrap();
    assert_eq!(store.death.repopped.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn reclaim_corpse_dispatches_with_the_wire_corpse_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RECLAIM_CORPSE {
        guid: Guid::new(777),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.death.reclaimed_corpses.lock().unwrap().as_slice(),
        &[(1, 777)]
    );
}

#[test]
fn resurrect_response_accept_maps_status_byte_to_true() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.death.resurrect_responses.lock().unwrap().as_slice(),
        &[(1, true)]
    );
}

#[test]
fn resurrect_response_decline_maps_status_byte_to_false() {
    // Proves the `status != 0` mapping actually distinguishes decline from accept, not just that
    // SOME boolean reaches the store.
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.death.resurrect_responses.lock().unwrap().as_slice(),
        &[(1, false)]
    );
}

#[test]
fn self_res_dispatches_self_resurrect_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SELF_RES {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client); // the revive replicates via the entity VALUES relay, not a direct SMSG here
    server.join().unwrap();
    assert_eq!(store.death.self_resurrects.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn a_refused_self_res_sends_nothing_and_keeps_the_session() {
    let store = std::sync::Arc::new({
        let base = quest_store();
        WorldFake {
            death: DeathState {
                self_resurrect_error: Some("no Self-Resurrection Option".into()),
                ..base.death
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SELF_RES {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("expected the sentinel (the Refusal sends nothing), got {other}"),
    }
    drop(client);
    server.join().unwrap();
    assert_eq!(store.death.self_resurrects.lock().unwrap().as_slice(), &[1]);
}

#[test]
fn spirit_healer_activate_dispatches_and_confirms_the_healer_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SPIRIT_HEALER_ACTIVATE {
        guid: Guid::new(888),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(p) => {
            assert_eq!(p.guid, Guid::new(888), "echoes the healer's own guid");
        }
        other => panic!("expected SMSG_SPIRIT_HEALER_CONFIRM, got {other}"),
    }
    drop(client);
    server.join().unwrap();
    // The SMSG above echoes the WIRE guid verbatim, so it alone can't catch a swapped-argument bug —
    // this pins that the STORE call also got (self_guid, healer_guid) in the right order.
    assert_eq!(
        store.death.spirit_healer_calls.lock().unwrap().as_slice(),
        &[(1, 888)]
    );
}

// ── Targeting/aura dispatch (CMSG_SET_SELECTION / CMSG_CANCEL_AURA / CMSG_CANCEL_CAST) ─────────────

#[test]
fn reducer_transport_loss_ends_an_admitted_session_and_frees_one_queue_seat() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                // The same dead transport makes leave-world cleanup unreachable. Teardown is best-effort,
                // but the client session and its admission seat must not wait for that reducer.
                logout_error: Some("transport disconnected".into()),
                ..base.session
            },
            combat: CombatState {
                set_target_error: Some("transport disconnected".into()),
                ..base.combat
            },
            ..base
        }
    });
    let queue = std::sync::Arc::new(LoginQueue::new(1, 0));
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server_queue = queue.clone();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result =
            run_world_session_with_queue(server_end, server_store.clone(), server_queue.as_ref());
        result_tx.send(result).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    assert_eq!(
        queue.active(),
        1,
        "the admitted session holds the only seat"
    );

    CMSG_SET_SELECTION {
        target: Guid::new(321),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    let err = result_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("transport loss must end the session promptly")
        .expect_err("a disconnected reducer transport must end the world session");
    assert!(format!("{err:#}").contains("transport disconnected"));
    assert!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err(),
        "the world socket closes after the fatal reducer result"
    );
    assert!(
        store
            .session
            .logout_called
            .load(std::sync::atomic::Ordering::SeqCst),
        "teardown still attempts leave-world cleanup"
    );
    assert_eq!(queue.active(), 0, "the ended session released its seat");
    assert_eq!(
        queue.request(),
        Admission::Admitted,
        "one replacement session is admitted"
    );
    assert!(
        matches!(queue.request(), Admission::Queued(_)),
        "only one seat was released"
    );
    drop(client);
}

#[test]
fn set_selection_dispatches_set_target_with_the_wire_guid() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_SET_SELECTION {
        target: Guid::new(321),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.combat.selected_targets.lock().unwrap().as_slice(),
        &[321]
    );
}

#[test]
fn cancel_aura_dispatches_with_the_wire_spell_id() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_AURA { id: 5555 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.cast.cancelled_auras.lock().unwrap().as_slice(),
        &[5555]
    );
}

#[test]
fn cancel_cast_dispatches_for_the_caller() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    CMSG_CANCEL_CAST { id: 133 }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drop(client);
    server.join().unwrap();
    assert_eq!(store.cast.cancelled_casts.lock().unwrap().as_slice(), &[1]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn competing_world_sessions_close_the_old_socket_without_removing_the_winner() {
    use crate::accept::BlockingTaskCapacity;
    use crate::config::GatewayConfig;
    use crate::durable_test_support::Standalone;
    use crate::stdb::Coordinator;

    fn start(
        coord: Coordinator,
        runtime: tokio::runtime::Handle,
    ) -> (UnixStream, std::sync::mpsc::Receiver<Result<()>>) {
        let (client, server) = world_session_socket_pair();
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _entered = runtime.enter();
            let _ = done.send(run_world_session(server, std::sync::Arc::new(coord)));
        });
        (client, result)
    }

    fn login(client: &mut UnixStream) -> (EncrypterHalf, DecrypterHalf) {
        let (mut encrypt, mut decrypt) = client_handshake(client, "TEST", [7; 40]);
        CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
            .write_encrypted_client(&mut *client, &mut encrypt)
            .unwrap();
        for _ in 0..100 {
            if matches!(
                ServerOpcodeMessage::read_encrypted(&mut *client, &mut decrypt).unwrap(),
                ServerOpcodeMessage::SMSG_WEATHER(_)
            ) {
                return (encrypt, decrypt);
            }
        }
        panic!("World Session did not complete its entry batch");
    }

    for name in [
        "LYRACORE_SHARD_MAP",
        "LYRACORE_SHARD_MAP_FILE",
        "LYRACORE_REALM_CORE",
    ] {
        assert!(
            std::env::var_os(name).is_none(),
            "unset {name} for this private fixture"
        );
    }
    let mut fixture = Standalone::start("account-world-sessions");
    fixture.publish_module();
    fixture.assert_call("claim_operator", &[]);
    fixture.assert_call("install_guid_range", &["0"]);
    fixture.assert_call("gw_heartbeat", &[]);
    let cfg = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: fixture.server().into(),
        module_name: fixture.shard_name().into(),
        coordinator_token: Some(fixture.owner_token()),
        gateway_id: "world-owner-a".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(2),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let a = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    let b = runtime
        .block_on(Coordinator::connect(&GatewayConfig {
            gateway_id: "world-owner-b".into(),
            ..cfg
        }))
        .unwrap();
    let (release_pool, hold_pool) = std::sync::mpsc::channel();
    let (pool_started, pool_ready) = std::sync::mpsc::channel();
    runtime.spawn_blocking(move || {
        pool_started.send(()).unwrap();
        let _ = hold_pool.recv_timeout(Duration::from_secs(40));
    });
    pool_ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let account = a.account_by_username("TEST").unwrap().unwrap().id;
    a.establish_session(account, &[7; 40], a.bound_identity(account).unwrap())
        .unwrap();
    let (mut first, first_done) = start(a, runtime.handle().clone());
    let _first_cipher = login(&mut first);

    let (mut refused, refused_done) = start(b.clone(), runtime.handle().clone());
    let (mut encrypt, _) = client_handshake(&mut refused, "TEST", [7; 40]);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut refused, &mut encrypt)
        .unwrap();
    let refusal = refused_done
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap_err();
    assert!(
        refusal.to_string().contains("ACCOUNT_IN_USE"),
        "{refusal:#}"
    );
    assert!(first_done.try_recv().is_err());

    fixture.assert_sql("UPDATE game_account_claim SET expires_micros = 0");
    let (mut winner, winner_done) = start(b, runtime.handle().clone());
    let (mut winner_encrypt, mut winner_decrypt) = login(&mut winner);
    let first_deadline = fixture.query_rows("SELECT expires_micros FROM game_account_claim")[0]
        ["expires_micros"]
        .clone();
    first_done
        .recv_timeout(Duration::from_secs(25))
        .expect("lost Account ownership must close the old World Session")
        .unwrap();
    assert_eq!(
        fixture
            .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
            .len(),
        1
    );
    assert!(winner_done.try_recv().is_err());
    assert!(
        crate::durable_test_support::poll_until(Duration::from_secs(5), || fixture
            .query_rows("SELECT expires_micros FROM game_account_claim")[0]["expires_micros"]
            != first_deadline),
        "the winning Account Claim must renew while the Tokio blocking pool is full"
    );
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut winner, &mut winner_encrypt)
        .unwrap();
    let mut logged_out = false;
    for _ in 0..100 {
        if read_raw_frame(&mut winner, &mut winner_decrypt).0
            == lyracore_shared::opcodes::world::SMSG_LOGOUT_COMPLETE
        {
            logged_out = true;
            break;
        }
    }
    assert!(
        logged_out,
        "winning World Session must still process logout"
    );
    winner.shutdown(std::net::Shutdown::Write).unwrap();
    winner_done
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    drop(winner);
    release_pool.send(()).unwrap();
    assert!(fixture
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn closing_a_world_session_interrupts_a_full_socket_without_draining_its_queue() {
    use std::os::fd::AsRawFd;

    let (_client, mut socket) = world_session_socket_pair();
    socket.set_nonblocking(true).unwrap();
    loop {
        match socket.write(&[0; 65536]) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("fill owned socket: {error}"),
        }
    }
    socket.set_nonblocking(false).unwrap();
    let (mut tx, queued, _) = session_channel();
    tx.bind_socket(socket.try_clone().unwrap());
    tx.send(Outbound::Raw {
        opcode: 0,
        body: vec![],
    })
    .unwrap();
    let socket_fd = socket.as_raw_fd();
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, result) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let task = std::fs::read_link("/proc/thread-self")
            .expect("this Linux socket fixture requires /proc/thread-self");
        started.send(task).unwrap();
        finished.send(socket.write_all(&[1; 65536])).unwrap();
    });
    let task = ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let syscall_path = std::path::Path::new("/proc").join(task).join("syscall");
    assert!(
        crate::durable_test_support::poll_until(Duration::from_secs(2), || {
            let syscall = std::fs::read_to_string(&syscall_path)
                .expect("this Linux socket fixture requires visibility of its writer's syscall");
            let mut fields = syscall.split_whitespace();
            let number = fields
                .next()
                .and_then(|value| value.parse::<libc::c_long>().ok());
            let fd = fields
                .next()
                .and_then(|value| value.strip_prefix("0x"))
                .and_then(|value| i32::from_str_radix(value, 16).ok());
            matches!(number, Some(n) if n == libc::SYS_sendto || n == libc::SYS_write)
                && fd == Some(socket_fd)
        }),
        "writer did not enter its blocked socket write"
    );
    tx.close();
    assert!(result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .is_err());
    assert!(matches!(queued.try_recv().unwrap(), Outbound::Raw { .. }));
    writer.join().unwrap();
}

/// Every petition opcode reaches its dispatch entry over a real encrypted socket. Character 1,
/// outside any Guild, owns Petition 42 and holds its Guild Charter; Character 2 is live.
#[test]
fn every_petition_opcode_reaches_its_dispatch_entry_over_the_socket() {
    use wow_world_messages::vanilla::{
        CMSG_OFFER_PETITION, CMSG_PETITION_BUY, CMSG_PETITION_QUERY, CMSG_PETITION_SHOWLIST,
        CMSG_PETITION_SHOW_SIGNATURES, CMSG_PETITION_SIGN, CMSG_TURN_IN_PETITION,
        MSG_PETITION_DECLINE, MSG_PETITION_RENAME,
    };
    const CHARTER: u64 = 0x4000_0000_0000_0101;
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            session: SessionState {
                login_entity: Some(warrior_entity()),
                ..base.session
            },
            characters: vec![
                codec::CharacterView {
                    guid: 1,
                    name: "Warrior".into(),
                    ..Default::default()
                },
                codec::CharacterView {
                    guid: 2,
                    name: "Target".into(),
                    ..Default::default()
                },
            ],
            guild: GuildState {
                guild_petitions: vec![codec::PetitionView {
                    petition_id: 42,
                    charter_item_guid: CHARTER,
                    owner_guid: 1,
                    name: "Night Watch".into(),
                    signers: vec![2],
                }],
                held_charters: vec![CHARTER],
                ..base.guild
            },
            ..base
        }
    });
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let mut next =
        |client: &mut UnixStream| ServerOpcodeMessage::read_encrypted(client, &mut c_dec).unwrap();

    CMSG_PETITION_SHOWLIST {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match next(&mut client) {
        ServerOpcodeMessage::SMSG_PETITION_SHOWLIST(list) => assert_eq!(list.npc.guid(), 90),
        other => panic!("expected SMSG_PETITION_SHOWLIST, got {other}"),
    }
    CMSG_PETITION_QUERY {
        guild_id: 42,
        petition: Guid::new(CHARTER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match next(&mut client) {
        ServerOpcodeMessage::SMSG_PETITION_QUERY_RESPONSE(query) => {
            assert_eq!(
                (query.petition_id, query.guild_name.as_str()),
                (42, "Night Watch")
            );
        }
        other => panic!("expected SMSG_PETITION_QUERY_RESPONSE, got {other}"),
    }
    CMSG_PETITION_SHOW_SIGNATURES {
        item: Guid::new(CHARTER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match next(&mut client) {
        ServerOpcodeMessage::SMSG_PETITION_SHOW_SIGNATURES(window) => {
            assert_eq!(window.signatures.len(), 1);
            assert_eq!(window.signatures[0].signer, Guid::new(2));
        }
        other => panic!("expected SMSG_PETITION_SHOW_SIGNATURES, got {other}"),
    }
    MSG_PETITION_RENAME {
        petition: Guid::new(CHARTER),
        new_name: "Day Watch".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match next(&mut client) {
        ServerOpcodeMessage::MSG_PETITION_RENAME(rename) => {
            assert_eq!(rename.new_name, "Day Watch")
        }
        other => panic!("expected MSG_PETITION_RENAME, got {other}"),
    }

    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_OFFER_PETITION {
            petition: Guid::new(CHARTER),
            target: Guid::new(2),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_PETITION_SIGN {
            petition: Guid::new(CHARTER),
            unknown1: 0,
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        MSG_PETITION_DECLINE {
            petition: Guid::new(CHARTER),
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });
    // The buyer still holds the Charter of its open Petition. mangos is silent; no copper moves.
    sync(&mut client, &mut c_enc, &mut c_dec, |c, e| {
        CMSG_PETITION_BUY {
            npc: Guid::new(90),
            name: "Night Watch".into(),
            ..Default::default()
        }
        .write_encrypted_client(c, e)
        .unwrap();
    });

    CMSG_TURN_IN_PETITION {
        petition: Guid::new(CHARTER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result) => {
            assert_eq!(result.string, "Night Watch");
        }
        other => panic!("expected SMSG_GUILD_COMMAND_RESULT, got {other}"),
    }
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_TURN_IN_PETITION_RESULTS(result) => assert_eq!(
            result.result,
            wow_world_messages::vanilla::PetitionResult::Ok
        ),
        other => panic!("expected SMSG_TURN_IN_PETITION_RESULTS, got {other}"),
    }
    drop(client);
    server.join().unwrap();

    let calls = recorded(&store);
    for op in [
        "guild_op:RenamePetition",
        "guild_op:OfferPetition",
        "guild_op:SignPetition",
        "guild_op:DeclinePetition",
        "guild_op:TurnInPetition",
    ] {
        assert!(calls.contains(&op.to_string()), "{op} never ran: {calls:?}");
    }
    assert!(
        position(&calls, "guild_op:TurnInPetition") < position(&calls, "guild_destroy_charter"),
        "the Guild is founded before the Charter is destroyed"
    );
    assert!(!calls.contains(&"guild_fee_hold".to_string()));
}
