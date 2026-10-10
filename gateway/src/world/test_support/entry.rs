//! World entry as a socket test reads it.

use super::*;
use std::os::unix::net::UnixStream;
use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION, CMSG_PLAYER_LOGIN};
use wow_world_messages::Guid;

/// More frames than any world entry batch carries. Reaching it means the batch never ended.
const WORLD_ENTRY_FRAME_LIMIT: usize = 64;

/// Read one world entry batch: the login sequence, any item CREATEs and the self CREATE, up to and
/// including the zone's `SMSG_WEATHER`, which always closes the batch. A fresh entry and a
/// world-port re-entry end the same way. Panics when no `SMSG_WEATHER` arrives within
/// [`WORLD_ENTRY_FRAME_LIMIT`] frames.
pub(crate) fn drain_world_entry<S: Read>(
    client: &mut S,
    dec: &mut DecrypterHalf,
) -> Vec<ServerOpcodeMessage> {
    let mut frames = Vec::new();
    while frames.len() < WORLD_ENTRY_FRAME_LIMIT {
        let message = ServerOpcodeMessage::read_encrypted(&mut *client, dec)
            .unwrap_or_else(|e| panic!("world entry frame {} did not arrive: {e}", frames.len()));
        let ends_batch = matches!(message, ServerOpcodeMessage::SMSG_WEATHER(_));
        frames.push(message);
        if ends_batch {
            return frames;
        }
    }
    panic!("no SMSG_WEATHER within {WORLD_ENTRY_FRAME_LIMIT} frames: the world entry batch never ended");
}

/// The client side of every real world-session test has a bounded read. A missing server packet is
/// a test failure, never an indefinitely blocked test process.
pub(crate) const WORLD_SESSION_READ_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(5);

pub(crate) fn world_session_socket_pair() -> (UnixStream, UnixStream) {
    let (client, server) = UnixStream::pair().expect("world-session socket pair must be created");
    client
        .set_read_timeout(Some(WORLD_SESSION_READ_DEADLINE))
        .expect("world-session client read deadline must be configured");
    (client, server)
}

pub(crate) const K: [u8; 40] = [
    0x2E, 0xFE, 0xE7, 0xB0, 0xC1, 0x77, 0xEB, 0xBD, 0xFF, 0x66, 0x76, 0xC5, 0x6E, 0xFC, 0x23, 0x39,
    0xBE, 0x9C, 0xAD, 0x14, 0xBF, 0x8B, 0x54, 0xBB, 0x5A, 0x86, 0xFB, 0xF8, 0x1F, 0x6D, 0x42, 0x4A,
    0xA2, 0x3C, 0xC9, 0xA3, 0x14, 0x9F, 0xB1, 0x75,
];

pub(crate) fn ns(s: &str) -> NormalizedString {
    NormalizedString::new(s).unwrap()
}

/// Drive the shared prefix of every world handshake: read the plaintext `SMSG_AUTH_CHALLENGE`,
/// derive the client-side proof + cipher pair for `key` with `wow_srp`, and send
/// `CMSG_AUTH_SESSION`. Lower-level than [`client_handshake`] — it does not read whatever comes
/// back, so a call site can assert on that itself (an `AuthOk`, an `AuthWaitQueue`, a plaintext
/// rejection...). Returns the cipher pair `into_client_header_crypto` derived, split; a `key` that
/// does not match what the server holds still produces a (mismatched, useless) pair here — the
/// send happens regardless — so a rejection-path call site is free to bind them as `_`.
pub(crate) fn drive_auth<S: Read + Write>(
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
pub(crate) fn client_handshake<S: Read + Write>(
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

pub(crate) fn auth_session(
    username: &str,
    client_seed: u32,
    client_proof: [u8; 20],
) -> CMSG_AUTH_SESSION {
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
pub(crate) fn tester_store(account_id: u64) -> WorldFake {
    WorldFake {
        trainer: TrainerState {
            talent_reset_quote: Some(10_000),
            ..Default::default()
        },
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

/// Human/Warrior entity matching the seed, as the gateway would read it back after
/// `player_login`.
pub(crate) fn warrior_entity() -> codec::EntityView {
    codec::EntityView {
        watched_faction_index: None,
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

/// A store configured for an in-world TESTER (account 7, char guid 1) with a login entity — the
/// base fixture every socket test that needs a logged-in player builds on, quest or not.
pub(crate) fn quest_store() -> WorldFake {
    let base = tester_store(7);
    WorldFake {
        session: SessionState {
            login_entity: Some(warrior_entity()),
            ..base.session
        },
        ..base
    }
}

/// Spin up a world session over a socket pair, handshake as TESTER, enter the world as `guid`, and
/// drain the world entry batch. Draining one
/// message too few manifests as the CLIENT closing with unread bytes still queued — which the kernel
/// reports back to the SERVER thread's next read as ECONNRESET, not a clean EOF.
/// Returns the client socket + encrypted halves + the server join handle for the test to drive.
pub(crate) fn enter_world(
    store: std::sync::Arc<WorldFake>,
    guid: u64,
) -> (
    UnixStream,
    EncrypterHalf,
    DecrypterHalf,
    std::thread::JoinHandle<()>,
) {
    let (mut client, server_end) = world_session_socket_pair();
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
    (client, c_enc, c_dec, server)
}
