//! The mailbox read path — the routing tests.
//!
//! What EXECUTES here is production `world::mail`, against the same in-memory multi-database
//! topology the party, whisper and transfer tests use. What the fakes stand in for is named at each
//! seam: a handle's `mails` is the `game_mail` rows on THAT database, and `mailboxes` is the answer
//! the gameobject PK lookup would give on the shard the player is standing on.
//!
//! The load-bearing shape: the SAME fixture mail is seeded on realm-core in the sharded topology and
//! on the single handle in the unsharded one, and both must render the same list — that is the
//! two-plane equivalence, not two tests that happen to agree.

use super::party_tests::{character, GINGER, TRIN, VIM};
use super::*;

/// The Goldshire mailbox, as the client names it in every mail packet.
const MAILBOX: u64 = 0xF110_0000_0000_0042;
/// A gameobject guid the player is NOT standing at (another map, out of range, or not a mailbox).
const FAR_MAILBOX: u64 = 0xF110_0000_0000_0099;

fn mail(id: u64, from: u64, subject: &str, body: &str) -> codec::MailView {
    codec::MailView {
        id,
        sender_guid: from,
        subject: subject.into(),
        body: body.into(),
        created_at_secs: 1_000,
        ..Default::default()
    }
}

/// The sharded topology: realm-core holds every mail, the open-world shard holds the mailbox
/// Ginger is standing at, and the instance shard holds Vim. Returns `(realm, world, calls)`.
fn sharded_mailbox() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    ShardCallLog,
) {
    let calls: ShardCallLog = Default::default();
    let realm = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore-realm".into(),
        calls: calls.clone(),
        is_realm: true,
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger"), character(TRIN, "Trin")],
        live_guids: vec![GINGER, TRIN],
        mailboxes: vec![MAILBOX],
        ..Default::default()
    });
    *realm.mails.lock().unwrap() = seeded_mail();
    (realm, world, calls)
}

/// The single-database gateway: no realm handle, the same rows and the same mailbox on the one
/// database `lyracore dev up` publishes.
fn unsharded_mailbox() -> std::sync::Arc<InMemoryStore> {
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore".into(),
        characters: vec![character(GINGER, "Ginger"), character(TRIN, "Trin")],
        live_guids: vec![GINGER, TRIN],
        mailboxes: vec![MAILBOX],
        ..Default::default()
    });
    *store.mails.lock().unwrap() = seeded_mail();
    store
}

/// One mail to Ginger and one to somebody else, so "listed for its recipient" and "listed for
/// nobody else" are the same fixture.
fn seeded_mail() -> Vec<(u64, codec::MailView)> {
    vec![
        (GINGER, mail(1, VIM, "Your sword", "left it at the inn")),
        (TRIN, mail(2, VIM, "Not yours", "for Trin only")),
    ]
}

/// **AC: a seeded mail appears in the list for its recipient, and for nobody else.**
#[test]
fn a_seeded_mail_is_listed_for_its_recipient_and_for_nobody_else() {
    let (_realm, world, _calls) = sharded_mailbox();

    let mails = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].subject, "Your sword");
    assert_eq!(mails[0].sender_guid, VIM);

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(
        trins.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![2],
        "a mailbox read is scoped to the reader — one player must never see another's mail"
    );
}

/// **AC: a player with no mail gets an empty list rather than no response.** An `Ok(vec![])` is
/// what makes the handler send a packet; an `Err` would be silence, and silence is
/// indistinguishable from a server that ignored the click.
#[test]
fn a_player_with_no_mail_gets_an_empty_list_and_not_a_refusal() {
    let (realm, world, _calls) = sharded_mailbox();
    realm.mails.lock().unwrap().clear();

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("an empty mailbox opens"),
        vec![]
    );
}

/// **AC: the mailbox read reaches the AUTHORITY.** Realm-core owns the rows on a sharded gateway;
/// a read that quietly went shard-local would work for a character homed where the row happens to
/// be and lose the mailbox for everyone else.
#[test]
fn a_sharded_mailbox_is_read_from_realm_core_and_never_from_the_players_own_shard() {
    let (_realm, world, calls) = sharded_mailbox();

    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    let log = calls.lock().unwrap().clone();
    assert!(
        log.contains(&("lyracore-realm".to_string(), "mail_list".to_string())),
        "the mail read must land on realm-core; calls were {log:?}"
    );
    assert!(
        !log.contains(&("world".to_string(), "mail_list".to_string())),
        "a multi-database gateway must not read the mailbox from the player's own shard — that is \
         exactly the shard-local behaviour the plane decision removes. Calls were {log:?}"
    );
}

/// **AC: the proximity gate runs on the shard the player is standing on.** Realm-core holds no
/// gameobjects, so asking it would refuse every mailbox on a sharded realm.
#[test]
fn the_mailbox_proximity_gate_is_asked_of_the_players_own_shard() {
    let (_realm, world, calls) = sharded_mailbox();

    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    let log = calls.lock().unwrap().clone();
    assert!(
        log.contains(&("world".to_string(), "mailbox_in_range".to_string())),
        "the mailbox is a gameobject on the player's own shard; calls were {log:?}"
    );
    assert!(
        !log.contains(&("lyracore-realm".to_string(), "mailbox_in_range".to_string())),
        "realm-core holds no gameobjects — asking it would refuse every mailbox. Calls were {log:?}"
    );
}

/// **AC: the same read produces the same list on both planes, through one shared core.** Same
/// fixture, same reader; the only difference is whether a realm handle exists.
#[test]
fn the_realm_plane_and_the_single_database_fallback_render_the_same_mailbox() {
    let (_realm, world, calls) = sharded_mailbox();
    let single = unsharded_mailbox();

    let sharded = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("realm plane");
    let unsharded = mail::open_mailbox(single.as_ref(), Some(GINGER), MAILBOX).expect("fallback");

    assert_eq!(sharded, unsharded, "the two planes must render one mailbox");
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(shard, _)| shard == "lyracore"),
        "the unsharded store shares no call log with the sharded topology — fixture drift"
    );
}

/// **AC: the single-database fallback reads its OWN database.** With no realm handle there is
/// nothing to route to, and a read that still looked for one would answer an empty mailbox on
/// `lyracore dev up`.
#[test]
fn an_unsharded_gateway_reads_the_mailbox_on_its_own_database() {
    let single = unsharded_mailbox();
    assert!(
        single.realm_store().is_none(),
        "fixture: this is the single-database gateway"
    );

    let mails = mail::open_mailbox(single.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].id, 1);
}

/// **AC: every mail opcode is refused when the player is not in world.** Character select drives no
/// mailbox — and the refusal must not be a mailbox read that happens to find nothing, because a
/// guid of 0 is a real recipient key.
#[test]
fn every_mail_read_is_refused_at_character_select() {
    let (_realm, world, calls) = sharded_mailbox();

    for err in [
        mail::open_mailbox(world.as_ref(), None, MAILBOX).unwrap_err(),
        mail::has_unread(world.as_ref(), None).unwrap_err(),
        mail::letter_body(world.as_ref(), None, 1).unwrap_err(),
    ] {
        assert!(
            err.to_string().contains("not in world"),
            "the refusal must name the gate that failed, got {err}"
        );
    }
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_list"),
        "a refused session must not read anyone's mailbox; calls were {log:?}"
    );
}

/// **AC: every mailbox-addressed opcode is refused away from the named mailbox.** The client passes
/// the guid, so a crafted packet naming a gameobject the player is not standing at must not open
/// the mailbox.
#[test]
fn a_mailbox_the_player_is_not_standing_at_is_refused() {
    let (_realm, world, calls) = sharded_mailbox();

    let err = mail::open_mailbox(world.as_ref(), Some(GINGER), FAR_MAILBOX)
        .expect_err("a mailbox out of reach refuses");
    assert!(err.to_string().contains("not at mailbox"), "got {err}");
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_list"),
        "the gate must refuse BEFORE the read, so a crafted packet cannot enumerate a mailbox from \
         anywhere on the map; calls were {log:?}"
    );
}

/// **AC: the unread poll answers from the same read as the list.** The envelope and the window are
/// one projection, so they cannot disagree — and the poll names no mailbox, which is why it is
/// gated on being in world alone.
#[test]
fn the_unread_poll_follows_the_same_mailbox_the_window_lists() {
    let (realm, world, _calls) = sharded_mailbox();
    assert!(mail::has_unread(world.as_ref(), Some(GINGER)).unwrap());

    realm.mails.lock().unwrap()[0].1.was_read = true;
    assert!(
        !mail::has_unread(world.as_ref(), Some(GINGER)).unwrap(),
        "a mailbox holding only READ mail must not light the envelope"
    );

    realm.mails.lock().unwrap().clear();
    assert!(!mail::has_unread(world.as_ref(), Some(GINGER)).unwrap());
}

/// **AC: `CMSG_ITEM_TEXT_QUERY` returns the letter's body** — and only to the character the mail is
/// addressed to. The body rides this query rather than the list packet, so this read is the whole
/// letter-reading path.
#[test]
fn a_letter_body_is_readable_by_its_recipient_and_by_nobody_else() {
    let (_realm, world, _calls) = sharded_mailbox();

    assert_eq!(
        mail::letter_body(world.as_ref(), Some(GINGER), 1).unwrap(),
        Some("left it at the inn".to_string())
    );
    assert_eq!(
        mail::letter_body(world.as_ref(), Some(GINGER), 2).unwrap(),
        None,
        "mail 2 is addressed to Trin — a crafted id must not read another player's letter"
    );
    assert_eq!(
        mail::letter_body(world.as_ref(), Some(GINGER), 999).unwrap(),
        None
    );
}

/// **AC: an empty body is never queried** — the list advertises text id 0 for it, which is the
/// signal the client acts on. Pinned here (rather than only in the codec) because the id and the
/// body come from the same row and a later slice must not split them.
#[test]
fn a_mail_with_no_body_advertises_text_id_zero() {
    let (realm, world, _calls) = sharded_mailbox();
    realm
        .mails
        .lock()
        .unwrap()
        .push((GINGER, mail(3, VIM, "No letter", "")));

    let mails = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    let packet = codec::build_mail_list(&mails, 1_000);
    let ids: Vec<u32> = packet.mails.iter().map(|m| m.item_text_id).collect();
    assert_eq!(ids, vec![1, 0]);
}

// ---------------------------------------------------------------------------------------------
//  Dispatch: the packets a real session actually gets back. `run_world_session` over a socket
//  pair, driven with real encrypted opcodes — the routing tests above pin WHAT is read, these pin
//  that the client is answered at all.
// ---------------------------------------------------------------------------------------------

/// The logged-in fixture: the warrior of the login tests (guid 1), standing at the mailbox, with
/// one unread mail waiting.
fn seated_store() -> std::sync::Arc<InMemoryStore> {
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        mailboxes: vec![MAILBOX],
        ..tester_store(7)
    });
    *store.mails.lock().unwrap() = vec![(1, mail(1, VIM, "Your sword", "left it at the inn"))];
    store
}

/// **AC: a player with no mail gets an empty list rather than no response.** The client opened the
/// mail frame itself (there is no `SMSG_SHOW_MAILBOX` in vanilla) and is waiting on this packet;
/// silence and "you have no mail" would look identical.
#[test]
fn an_empty_mailbox_is_answered_with_an_empty_list_packet() {
    let store = seated_store();
    store.mails.lock().unwrap().clear();

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    wow_world_messages::vanilla::CMSG_GET_MAIL_LIST {
        mailbox: Guid::new(MAILBOX),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MAIL_LIST_RESULT(m) => assert!(m.mails.is_empty()),
        other => panic!("expected the mail list, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

/// **AC: a seeded mail reaches its recipient over the wire**, subject and sender intact, with the
/// mail's own id doubling as the `item_text_id` the client queries the body with.
#[test]
fn a_seeded_mail_reaches_the_client_as_a_mail_list_row() {
    let store = seated_store();

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    wow_world_messages::vanilla::CMSG_GET_MAIL_LIST {
        mailbox: Guid::new(MAILBOX),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let text_id = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MAIL_LIST_RESULT(m) => {
            assert_eq!(m.mails.len(), 1);
            assert_eq!(m.mails[0].subject, "Your sword");
            assert_eq!(m.mails[0].message_id, 1);
            m.mails[0].item_text_id
        }
        other => panic!("expected the mail list, got {other}"),
    };

    // The body rides the follow-up query the client sends on OPENING the letter, keyed by the id
    // the list just advertised.
    wow_world_messages::vanilla::CMSG_ITEM_TEXT_QUERY {
        item_text_id: text_id,
        mail_id: 1,
        unknown1: 0,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_TEXT_QUERY_RESPONSE(m) => {
            assert_eq!(m.item_text_id, text_id);
            assert_eq!(m.text, "left it at the inn");
        }
        other => panic!("expected the letter body, got {other}"),
    }

    // And the poll behind the minimap envelope answers 0.0 while that mail is unread.
    ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(m) => assert_eq!(m.unread_mails, 0.0),
        other => panic!("expected the mail-time poll, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

/// **AC: a refused mail opcode is never session-fatal.** A mail poll at character select, and a
/// mailbox the player is not standing at: the session survives both, and the poll is still
/// answered (a dropped reply leaves a stale envelope lit).
#[test]
fn a_refused_mail_opcode_costs_a_packet_and_never_the_session() {
    let store = seated_store();

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    // Still at character select — no self guid, so every gate refuses.
    ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(m) => assert!(
            m.unread_mails < 0.0,
            "a refused poll must answer 'no mail', not light the envelope"
        ),
        other => panic!("expected the mail-time poll, got {other}"),
    }

    // The session is still alive: log in, then click a mailbox that is not there.
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..10 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    wow_world_messages::vanilla::CMSG_GET_MAIL_LIST {
        mailbox: Guid::new(FAR_MAILBOX),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // No reply for a refused mailbox (vanilla has no mailbox-refusal packet) — but the session
    // still answers the next opcode, which is what "per-action, never session-fatal" means.
    ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(m) => assert_eq!(m.unread_mails, 0.0),
        other => panic!("expected the mail-time poll, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}
