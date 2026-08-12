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

use super::party_tests::{character, DORMANT, GINGER, TRIN, VIM};
use super::*;

/// The Goldshire mailbox, as the client names it in every mail packet.
const MAILBOX: u64 = 0xF110_0000_0000_0042;
/// A gameobject guid the player is NOT standing at (another map, out of range, or not a mailbox).
const FAR_MAILBOX: u64 = 0xF110_0000_0000_0099;
/// The wire's "no item attached" — every letter in these fixtures unless it says otherwise.
const NO_ITEM: u64 = 0;

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

// ---------------------------------------------------------------------------------------------
//  Mark-as-read and delete — the routing tests, same shape as the read path above.
// ---------------------------------------------------------------------------------------------

/// **AC: `CMSG_MAIL_MARK_AS_READ` flips the row's read state, and the next list reflects it.**
#[test]
fn mark_read_flips_the_row_and_the_next_list_shows_it() {
    let (_realm, world, _calls) = sharded_mailbox();

    mail::mark_read(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger owns mail 1");

    let mails = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert!(mails[0].was_read, "the next list read must show the flip");
}

/// **AC: a read mail no longer counts toward the unread poll answer.**
#[test]
fn a_read_mail_no_longer_lights_the_unread_poll() {
    let (_realm, world, _calls) = sharded_mailbox();
    assert!(mail::has_unread(world.as_ref(), Some(GINGER)).unwrap());

    mail::mark_read(world.as_ref(), Some(GINGER), MAILBOX, 1).unwrap();

    assert!(
        !mail::has_unread(world.as_ref(), Some(GINGER)).unwrap(),
        "the poll must derive from the SAME read the list uses, so marking read is visible to it too"
    );
}

/// **AC: `CMSG_MAIL_DELETE` removes the row; the next list no longer shows it.**
#[test]
fn delete_removes_the_row_and_the_next_list_no_longer_shows_it() {
    let (_realm, world, _calls) = sharded_mailbox();

    mail::delete(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger owns mail 1");

    let mails = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert!(
        mails.is_empty(),
        "a deleted mail must be gone from the next list read"
    );
}

/// **AC: both are refused for a mail the caller is not the recipient of.** A crafted id naming
/// somebody else's mail must not be distinguishable from a nonexistent one — the same
/// non-distinction the letter-body read already takes.
#[test]
fn both_are_refused_for_a_mail_the_caller_does_not_own() {
    let (_realm, world, _calls) = sharded_mailbox();

    // Mail 2 is Trin's — Ginger names it anyway.
    let err = mail::mark_read(world.as_ref(), Some(GINGER), MAILBOX, 2)
        .expect_err("mail 2 is not Ginger's");
    assert!(
        err.to_string().contains("not addressed to you"),
        "got {err}"
    );
    let err =
        mail::delete(world.as_ref(), Some(GINGER), MAILBOX, 2).expect_err("mail 2 is not Ginger's");
    assert!(
        err.to_string().contains("not addressed to you"),
        "got {err}"
    );

    // Neither refusal touched the row: Trin's mail is untouched and still listed for Trin.
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(
        trins.len(),
        1,
        "a refused cross-owner action must not mutate the other mailbox"
    );
    assert!(!trins[0].was_read);
}

/// **AC: both work identically on the realm plane and the single-database fallback.** Same write,
/// same before/after list, through the shared core — not two code paths that happen to agree.
#[test]
fn both_write_ops_behave_identically_on_the_realm_plane_and_the_fallback() {
    let (_realm, world, _calls) = sharded_mailbox();
    let single = unsharded_mailbox();

    mail::mark_read(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("sharded plane");
    mail::mark_read(single.as_ref(), Some(GINGER), MAILBOX, 1).expect("fallback plane");
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).unwrap(),
        mail::open_mailbox(single.as_ref(), Some(GINGER), MAILBOX).unwrap(),
        "both planes must show the same mark-read result"
    );
}

/// **AC: mail write ops reach the AUTHORITY, never the player's own shard, on a sharded gateway** —
/// the write half of `a_sharded_mailbox_is_read_from_realm_core_and_never_from_the_players_own_shard`.
#[test]
fn a_sharded_mailbox_write_lands_on_realm_core_and_never_on_the_players_own_shard() {
    let (_realm, world, calls) = sharded_mailbox();

    mail::mark_read(world.as_ref(), Some(GINGER), MAILBOX, 1).unwrap();
    mail::delete(world.as_ref(), Some(TRIN), MAILBOX, 2).unwrap();

    let log = calls.lock().unwrap().clone();
    assert!(log.contains(&("lyracore-realm".to_string(), "mail_mark_read".to_string())));
    assert!(log.contains(&("lyracore-realm".to_string(), "mail_delete".to_string())));
    assert!(!log.iter().any(
        |(shard, call)| shard == "world" && (call == "mail_mark_read" || call == "mail_delete")
    ));
}

/// **AC: both are refused the same way the read path is — not in world, or not at the named
/// mailbox — before touching any row.** Mirrors `every_mail_read_is_refused_at_character_select`
/// and `a_mailbox_the_player_is_not_standing_at_is_refused`.
#[test]
fn both_write_ops_are_gated_like_the_read_path() {
    let (_realm, world, calls) = sharded_mailbox();

    let err = mail::mark_read(world.as_ref(), None, MAILBOX, 1).unwrap_err();
    assert!(err.to_string().contains("not in world"), "got {err}");
    let err = mail::delete(world.as_ref(), Some(GINGER), FAR_MAILBOX, 1).unwrap_err();
    assert!(err.to_string().contains("not at mailbox"), "got {err}");

    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|(_, call)| call == "mail_mark_read" || call == "mail_delete"),
        "a gate refusal must never reach the write; calls were {log:?}"
    );
}

// ---------------------------------------------------------------------------------------------
//  Dispatch: mark-as-read and delete over the real wire.
// ---------------------------------------------------------------------------------------------

/// **AC: `CMSG_MAIL_MARK_AS_READ` gets NO wire reply** (vanilla sends none — the client already
/// flipped its own display), but the session survives and the NEXT list read shows the flip.
#[test]
fn mark_as_read_sends_no_reply_but_the_next_list_shows_it() {
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

    wow_world_messages::vanilla::CMSG_MAIL_MARK_AS_READ {
        mailbox: Guid::new(MAILBOX),
        mail_id: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // No reply for mark-as-read — assert by racing the NEXT packet, which must be the poll answer
    // below, not a stray SMSG this opcode was never supposed to send.
    wow_world_messages::vanilla::CMSG_GET_MAIL_LIST {
        mailbox: Guid::new(MAILBOX),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MAIL_LIST_RESULT(m) => {
            assert!(m.mails[0].message_id == 1, "still the same mail, now read");
        }
        other => {
            panic!("expected the mail list (mark-as-read sends no reply of its own), got {other}")
        }
    }

    drop(client);
    server.join().unwrap();
}

/// **AC: `CMSG_MAIL_DELETE` removes the row and acks `SMSG_SEND_MAIL_RESULT`/Deleted.**
#[test]
fn delete_acks_with_send_mail_result_and_the_next_list_is_empty() {
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

    wow_world_messages::vanilla::CMSG_MAIL_DELETE {
        mailbox_id: Guid::new(MAILBOX),
        mail_id: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => {
            assert_eq!(m.mail_id, 1);
            match m.action {
                wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailAction::Deleted {
                    result2,
                } => {
                    assert_eq!(
                        result2,
                        wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok
                    );
                }
                other => panic!("expected the Deleted action, got {other:?}"),
            }
        }
        other => panic!("expected SMSG_SEND_MAIL_RESULT, got {other}"),
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

/// **AC: a refused delete (not the caller's mail) still acks — with the generic error — and never
/// tears the session down.**
#[test]
fn a_refused_delete_still_acks_and_never_kills_the_session() {
    let store = seated_store();
    // Mail 1 belongs to guid 1 (the seated warrior) in the base fixture; retarget it to someone
    // else so THIS session's delete is the not-your-mail refusal.
    store.mails.lock().unwrap()[0].0 = 999;

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

    wow_world_messages::vanilla::CMSG_MAIL_DELETE {
        mailbox_id: Guid::new(MAILBOX),
        mail_id: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => match m.action {
            wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailAction::Deleted { result2 } => {
                assert_eq!(
                    result2,
                    wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
                );
            }
            other => panic!("expected the Deleted action, got {other:?}"),
        },
        other => panic!("expected SMSG_SEND_MAIL_RESULT, got {other}"),
    }

    // The session survives: the next opcode is still answered.
    ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(_) => {}
        other => panic!("expected the mail-time poll, got {other}"),
    }

    drop(client);
    server.join().unwrap();
}

// ---------------------------------------------------------------------------------------------
//  Sending a text-only letter — the gates, the postage, and the cross-database delivery.
//
//  Everything the gateway decides about a send runs here: realm-core holds no characters, no live
//  entities and no gameobjects, so "may these two write to each other" is answered before any
//  reducer. The module answers affordability alone, which the fake store models faithfully — an
//  atomic debit that refuses for a purse that cannot pay.
// ---------------------------------------------------------------------------------------------

/// A Horde character. `party_tests::character` builds a Human, so this is the faction gate's other
/// side.
fn orc(guid: u64, name: &str) -> codec::CharacterView {
    codec::CharacterView {
        race: 2,
        ..character(guid, name)
    }
}

const GRUG: u64 = 20; // Horde, standing next to Ginger — the faction refusal
const ECHO_WORLD: u64 = 30; // "Echo" on the open world …
const ECHO_INSTANCES: u64 = 31; // … and "Echo" again on the instances shard: the homonym
/// Ginger's purse, comfortably above the 30-copper postage.
const PURSE: u32 = 500;

/// The sharded send topology: realm-core owns the mail rows, Ginger and the local cast live on the
/// open world, Vim lives on the instances shard — the boundary the plane decision exists to cross.
/// Only Ginger carries a purse, because only Ginger sends.
fn sharded_send() -> (
    std::sync::Arc<InMemoryStore>,
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
        characters: vec![
            character(GINGER, "Ginger"),
            character(TRIN, "Trin"),
            character(DORMANT, "Dormant"),
            character(ECHO_WORLD, "Echo"),
            orc(GRUG, "Grug"),
        ],
        live_guids: vec![GINGER, TRIN, GRUG],
        offline_guids: vec![DORMANT],
        mailboxes: vec![MAILBOX],
        ..Default::default()
    });
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(VIM, "Vim"), character(ECHO_INSTANCES, "Echo")],
        live_guids: vec![VIM],
        mailboxes: vec![MAILBOX],
        ..Default::default()
    });
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE)];
    for shard in [&world, &instances] {
        *shard.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    }
    (realm, world, instances, calls)
}

/// The same cast on one database — `lyracore dev up`'s gateway, where the purse and the mail row
/// live together.
fn unsharded_send() -> std::sync::Arc<InMemoryStore> {
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore".into(),
        characters: vec![character(GINGER, "Ginger"), character(TRIN, "Trin")],
        live_guids: vec![GINGER, TRIN],
        mailboxes: vec![MAILBOX],
        ..Default::default()
    });
    *store.purses.lock().unwrap() = vec![(GINGER, PURSE)];
    store
}

fn post<St: WorldStore + ?Sized>(
    store: &St,
    to: &str,
) -> std::result::Result<(), mail::SendRefusal> {
    post_money(store, to, 0)
}

/// The same letter with `money` copper attached — the coin the recipient takes out, on top of the
/// postage.
fn post_money<St: WorldStore + ?Sized>(
    store: &St,
    to: &str,
    money: u32,
) -> std::result::Result<(), mail::SendRefusal> {
    mail::send(
        store,
        Some(GINGER),
        MAILBOX,
        to,
        "Your sword".into(),
        "left it at the inn".into(),
        money,
        NO_ITEM,
    )
}

/// **AC: a letter to an online character on the same shard arrives in their list.**
#[test]
fn a_letter_to_a_character_on_the_same_shard_arrives_in_their_list() {
    let (_realm, world, _instances, _calls) = sharded_send();

    post(world.as_ref(), "Trin").expect("Trin is a Human on the same shard");

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(trins.len(), 1);
    assert_eq!(trins[0].subject, "Your sword");
    assert_eq!(trins[0].sender_guid, GINGER);
    assert!(!trins[0].was_read, "a freshly posted letter is unread");
}

/// **AC: a letter to an OFFLINE character is waiting for them.** Nothing about the send gates on
/// the recipient being logged in — that is the whole difference between mail and a whisper.
#[test]
fn a_letter_to_an_offline_character_is_waiting_in_their_mailbox() {
    let (_realm, world, _instances, _calls) = sharded_send();

    post(world.as_ref(), "Dormant").expect("an offline character is a valid recipient");

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(DORMANT), MAILBOX)
            .expect("the gate opens")
            .len(),
        1
    );
}

/// **AC: a letter to a character homed on ANOTHER database arrives.** The required cross-shard test:
/// Ginger is on the open world, Vim is inside the instance pool, and the letter still lands — read
/// back from Vim's own handle, which routes to the same authority. This case is why realm-core owns
/// the rows at all.
#[test]
fn a_letter_crosses_a_database_boundary_to_a_recipient_homed_on_another_shard() {
    let (_realm, world, instances, calls) = sharded_send();
    assert_eq!(
        world.character_guid_by_name("Vim").unwrap(),
        None,
        "fixture: Vim's row lives on the instances shard, as it did live"
    );

    post(world.as_ref(), "Vim").expect("the realm-wide name union finds Vim");

    let vims = mail::open_mailbox(instances.as_ref(), Some(VIM), MAILBOX).expect("the gate opens");
    assert_eq!(vims.len(), 1);
    assert_eq!(vims[0].sender_guid, GINGER);

    let log = calls.lock().unwrap().clone();
    assert!(
        log.contains(&("lyracore-realm".to_string(), "mail_commit".to_string())),
        "the row must be written on the authority; calls were {log:?}"
    );
    assert!(
        !log.iter()
            .any(|(shard, call)| shard == "world" && (call == "mail_commit" || call == "mail_send")),
        "a row written on the sender's own shard is invisible to a recipient homed elsewhere — the \
         exact bug the plane decision removes. Calls were {log:?}"
    );
}

/// **AC: postage is debited at send** — out of the purse on the SENDER's own shard, because
/// realm-core holds none.
///
/// Re-cut from #145's version, which asserted a bare `mail_charge_postage` call. That reducer is
/// gone: postage now rides the escrow fence alongside the attached coin, so the sender pays for the
/// whole letter in ONE debit instead of two calls that could half-land. What the test still pins is
/// the thing that mattered — WHICH database the purse is on.
#[test]
fn postage_is_debited_from_the_senders_own_shard_at_send() {
    let (_realm, world, _instances, calls) = sharded_send();

    post(world.as_ref(), "Trin").expect("the send goes through");

    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::postage()
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        log.contains(&("world".to_string(), "mail_fence".to_string())),
        "the purse is on the sender's shard; calls were {log:?}"
    );
    assert!(
        !log.contains(&("lyracore-realm".to_string(), "mail_fence".to_string())),
        "realm-core holds no characters and no purse; calls were {log:?}"
    );
}

/// **AC: a sender who cannot afford the postage is refused and charged NOTHING** — and no row is
/// written, so a failed send costs exactly nothing.
#[test]
fn a_sender_who_cannot_afford_the_postage_is_refused_and_charged_nothing() {
    let (_realm, world, _instances, calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, 10)];

    let refusal = post(world.as_ref(), "Trin").expect_err("10 copper does not cover the postage");
    assert!(matches!(refusal, mail::SendRefusal::NotEnoughMoney(_)));

    assert_eq!(world.purses.lock().unwrap()[0].1, 10, "charged nothing");
    assert!(mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
        .unwrap()
        .is_empty());
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_send"),
        "an unaffordable send must not write a row; calls were {log:?}"
    );
}

/// **AC: a non-existent recipient, yourself, and an opposing-faction recipient each produce their
/// OWN refusal.** The variant is the whole diagnosability story for a letter that did not go, so a
/// shared generic error would be the defect — none of these may collapse into another.
#[test]
fn each_refused_gate_produces_its_own_distinct_refusal() {
    let (_realm, world, _instances, calls) = sharded_send();

    assert!(matches!(
        post(world.as_ref(), "Nobody").unwrap_err(),
        mail::SendRefusal::RecipientNotFound(_)
    ));
    assert_eq!(
        post(world.as_ref(), "Ginger").unwrap_err(),
        mail::SendRefusal::CannotSendToSelf
    );
    assert_eq!(
        post(world.as_ref(), "Grug").unwrap_err(),
        mail::SendRefusal::NotYourTeam
    );

    assert_eq!(world.purses.lock().unwrap()[0].1, PURSE, "charged nothing");
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_send"),
        "every gate must refuse BEFORE the write; calls were {log:?}"
    );
}

/// **AC: the faction gate reads the SHARED race→team mapping**, not a second copy. Pinned by
/// driving a Horde SENDER: a mapping that had drifted to "everyone is Alliance" would let this
/// through, and would also have to be wrong in `graveyard`'s corpse release to stay consistent.
#[test]
fn the_faction_gate_refuses_in_both_directions() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GRUG, PURSE)];

    let refusal = mail::send(
        world.as_ref(),
        Some(GRUG),
        MAILBOX,
        "Trin",
        "Hail".into(),
        "".into(),
        0,
        NO_ITEM,
    )
    .expect_err("a Horde sender cannot write to an Alliance recipient either");
    assert_eq!(refusal, mail::SendRefusal::NotYourTeam);
    assert!(lyracore_shared::faction::same_team(2, 2), "fixture sanity");
}

/// **AC: a name that resolves to SEVERAL characters is refused, never guessed.**
///
/// Character names are unique per database, not per realm. Whisper picks the online candidate; mail
/// cannot, because reaching an offline character is the point of it. Refusing costs the sender a
/// retry — first-hit-wins would silently post the letter (and, once the attachment slices land, an
/// item and a purse of gold) to a stranger who can simply take it.
#[test]
fn a_homonym_recipient_is_refused_rather_than_guessed() {
    let (_realm, world, _instances, calls) = sharded_send();
    assert_eq!(
        party::resolve_all_by_name(world.as_ref(), "Echo").unwrap(),
        vec![ECHO_WORLD, ECHO_INSTANCES],
        "fixture: one name, two characters, on two databases"
    );

    let refusal = post(world.as_ref(), "Echo").expect_err("nothing can choose between them");
    assert!(
        matches!(refusal, mail::SendRefusal::RecipientNotFound(_)),
        "an ambiguous name is answered as 'no such recipient' — the client's closest text, and the \
         only honest one: got {refusal:?}"
    );
    assert!(!calls
        .lock()
        .unwrap()
        .iter()
        .any(|(_, call)| call == "mail_send"));
}

/// **AC: the sender's own list never shows the mail they sent.** A sent letter is the recipient's
/// row and nobody else's — there is no outbox to take it back out of.
#[test]
fn the_senders_own_list_never_shows_the_mail_they_sent() {
    let (_realm, world, _instances, _calls) = sharded_send();

    post(world.as_ref(), "Trin").expect("the send goes through");

    assert!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX)
            .expect("the gate opens")
            .is_empty(),
        "the sender's mailbox must stay empty"
    );
    assert!(
        !mail::has_unread(world.as_ref(), Some(GINGER)).unwrap(),
        "and their envelope must not light for their own letter"
    );
}

/// **AC: sending is refused from character select and away from a mailbox** — before any name is
/// resolved and before any purse is touched.
#[test]
fn sending_is_refused_at_character_select_and_away_from_a_mailbox() {
    let (_realm, world, _instances, calls) = sharded_send();

    let refusal = mail::send(
        world.as_ref(),
        None,
        MAILBOX,
        "Trin",
        "Hi".into(),
        "".into(),
        0,
        NO_ITEM,
    )
    .expect_err("character select drives no mailbox");
    assert!(
        refusal.to_string().contains("not in world"),
        "got {refusal}"
    );

    let refusal = mail::send(
        world.as_ref(),
        Some(GINGER),
        FAR_MAILBOX,
        "Trin",
        "Hi".into(),
        "".into(),
        0,
        NO_ITEM,
    )
    .expect_err("a mailbox out of reach refuses");
    assert!(
        refusal.to_string().contains("not at mailbox"),
        "got {refusal}"
    );

    assert_eq!(world.purses.lock().unwrap()[0].1, PURSE);
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|(_, call)| call == "mail_send" || call == "mail_charge_postage"),
        "a gate refusal must never reach the write; calls were {log:?}"
    );
}

/// **AC: both planes produce the same row for the same letter, through one shared core.** Same
/// sender, same recipient, same text; the only difference is whether a realm handle exists.
#[test]
fn both_planes_produce_the_same_row_for_the_same_letter() {
    let (_realm, world, _instances, _calls) = sharded_send();
    let single = unsharded_send();

    post(world.as_ref(), "Trin").expect("realm plane");
    post(single.as_ref(), "Trin").expect("fallback plane");

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap(),
        mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX).unwrap(),
        "the two planes must produce one letter"
    );
    assert_eq!(
        single.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::postage(),
        "and both must charge the same postage"
    );
}

/// **AC: the debit and the row insert are ONE transaction wherever they can be.**
///
/// Re-cut from #145's version, which pinned "the write call carries the postage on one database and
/// 0 when sharded". That sequence is gone with `mail_charge_postage`: the sharded plane now fences
/// the whole cost, so what the two planes share is the AMOUNT the sender pays, not the argument
/// list. The property worth pinning is unchanged and now stated directly — the purse goes down by
/// postage plus attached coin exactly once, whichever plane ran.
#[test]
fn the_whole_cost_leaves_the_purse_once_on_either_plane() {
    let (_realm, world, _instances, calls) = sharded_send();
    let single = unsharded_send();
    let attached = 100;
    let cost = lyracore_shared::mail::total_cost(attached);

    post_money(world.as_ref(), "Trin", attached).expect("realm plane");
    post_money(single.as_ref(), "Trin", attached).expect("fallback plane");

    assert_eq!(world.purses.lock().unwrap()[0].1, PURSE - cost);
    assert_eq!(single.purses.lock().unwrap()[0].1, PURSE - cost);
    // One database: one call does the debit and the insert together, and nothing is fenced.
    assert_eq!(single.sent_mail.lock().unwrap()[0].4, attached);
    assert!(single.mail_escrows.lock().unwrap().is_empty());
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_send"),
        "a sharded send has no one-transaction path to take; calls were {log:?}"
    );
}

/// **AC: a sent letter's body is readable by the recipient through the item-text query.** The whole
/// point of a text-only mail: the list packet carries only an `item_text_id`, and the client comes
/// back for the text with it.
#[test]
fn a_sent_letters_body_is_readable_through_the_item_text_query_path() {
    let (_realm, world, _instances, _calls) = sharded_send();

    post(world.as_ref(), "Trin").expect("the send goes through");

    let mails = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    let packet = codec::build_mail_list(&mails, 1_000);
    let text_id = packet.mails[0].item_text_id;
    assert_ne!(text_id, 0, "a letter WITH a body must advertise its own id");
    assert_eq!(
        mail::letter_body(world.as_ref(), Some(TRIN), u64::from(text_id)).unwrap(),
        Some("left it at the inn".to_string())
    );
}

// ---------------------------------------------------------------------------------------------
//  Dispatch: sending over the real wire.
// ---------------------------------------------------------------------------------------------

/// The seated warrior (guid 1), standing at a mailbox with a purse, and two people to write to.
fn seated_sender() -> std::sync::Arc<InMemoryStore> {
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        mailboxes: vec![MAILBOX],
        characters: vec![
            character(1, "Tester"),
            character(TRIN, "Trin"),
            orc(GRUG, "Grug"),
        ],
        live_guids: vec![1, TRIN, GRUG],
        ..tester_store(7)
    });
    *store.purses.lock().unwrap() = vec![(1, PURSE)];
    store
}

/// Drive a logged-in session and post one letter, returning the `MailResultTwo` the client is told.
fn send_over_the_wire(
    store: &std::sync::Arc<InMemoryStore>,
    receiver: &str,
) -> wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo {
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

    wow_world_messages::vanilla::CMSG_SEND_MAIL {
        mailbox: Guid::new(MAILBOX),
        receiver: receiver.into(),
        subject: "Your sword".into(),
        body: "left it at the inn".into(),
        ..Default::default()
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let result2 = match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => match m.action {
            wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailAction::Send { result2 } => {
                result2
            }
            other => panic!("expected the Send action, got {other:?}"),
        },
        other => panic!("expected SMSG_SEND_MAIL_RESULT, got {other}"),
    };

    // The session survives either way — a refused send costs a red line and nothing more.
    ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME(_) => {}
        other => panic!("expected the mail-time poll, got {other}"),
    }

    drop(client);
    server.join().unwrap();
    result2
}

/// **AC: `CMSG_SEND_MAIL` posts the letter and acks Ok**, and the row is the recipient's.
#[test]
fn a_letter_sent_over_the_wire_is_acked_ok_and_lands_in_the_recipients_mailbox() {
    let store = seated_sender();

    assert_eq!(
        send_over_the_wire(&store, "Trin"),
        wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok
    );

    let trins = mail::open_mailbox(store.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(trins.len(), 1);
    assert_eq!(trins[0].body, "left it at the inn");
    assert_eq!(store.purses.lock().unwrap()[0].1, PURSE - 30);
}

/// **AC: each refusal reaches the player as its OWN on-screen error**, over the real wire, and never
/// as a generic internal one. This is the packet the whole gate split exists to produce.
#[test]
fn each_refused_send_reaches_the_client_as_its_own_wire_error() {
    use wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo as R;

    for (receiver, want) in [
        ("Nobody", R::ErrRecipientNotFound),
        ("Tester", R::ErrCannotSendToSelf),
        ("Grug", R::ErrNotYourTeam),
    ] {
        let store = seated_sender();
        assert_eq!(send_over_the_wire(&store, receiver), want, "for {receiver}");
        assert_eq!(
            store.purses.lock().unwrap()[0].1,
            PURSE,
            "a refused send charges nothing"
        );
    }

    // And the affordability refusal, which the module owns rather than the gateway.
    let store = seated_sender();
    *store.purses.lock().unwrap() = vec![(1, 10)];
    assert_eq!(send_over_the_wire(&store, "Trin"), R::ErrNotEnoughMoney);
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

// ---------------------------------------------------------------------------------------------
//  Copper attachments: the debit at send, the take, and the drive that carries them across a
//  database boundary.
// ---------------------------------------------------------------------------------------------

/// The copper a fixture letter carries.
const ATTACHED: u32 = 100;

/// The escrow steps a drive takes, in order, on whichever database ran each — the sequence the
/// safety argument is made of.
fn escrow_steps(calls: &ShardCallLog) -> Vec<(String, String)> {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, call)| {
            matches!(
                call.as_str(),
                "mail_fence"
                    | "mail_commit"
                    | "mail_take_money_fence"
                    | "mail_payout"
                    | "mail_confirm_delivery"
                    | "mail_settle"
            )
        })
        .cloned()
        .collect()
}

/// **AC: copper attached to a mail is debited at send, alongside the postage** — and it arrives in
/// the recipient's row, ready to be taken.
#[test]
fn attached_copper_leaves_the_senders_purse_at_send_and_rides_the_letter() {
    let (_realm, world, _instances, _calls) = sharded_send();

    post_money(world.as_ref(), "Trin", ATTACHED).expect("the send goes through");

    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::total_cost(ATTACHED),
        "one debit for the postage AND the coin"
    );
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(trins[0].money, ATTACHED);
}

/// **AC: a sender who cannot afford postage plus attachment is refused and charged nothing.** The
/// affordability check is the fence itself, so there is no window where the postage went and the
/// coin did not.
#[test]
fn a_sender_who_cannot_afford_the_attachment_is_refused_and_charged_nothing() {
    let (_realm, world, _instances, calls) = sharded_send();
    let barely = lyracore_shared::mail::total_cost(ATTACHED) - 1;
    *world.purses.lock().unwrap() = vec![(GINGER, barely)];

    let refusal = post_money(world.as_ref(), "Trin", ATTACHED)
        .expect_err("the postage alone is affordable, the letter is not");
    assert!(matches!(refusal, mail::SendRefusal::NotEnoughMoney(_)));

    assert_eq!(world.purses.lock().unwrap()[0].1, barely, "charged nothing");
    assert!(mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
        .unwrap()
        .is_empty());
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_commit"),
        "a refused fence must never reach the commit; calls were {log:?}"
    );
}

/// **AC: `CMSG_MAIL_TAKE_MONEY` credits the recipient's purse and clears the amount from the row —
/// and the mail stays readable.**
#[test]
fn taking_a_mails_money_credits_the_purse_and_leaves_the_letter_readable() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    post_money(world.as_ref(), "Trin", ATTACHED).expect("the send goes through");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;

    mail::take_money(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("the take goes through");

    assert_eq!(world.purses.lock().unwrap()[1].1, ATTACHED);
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(trins.len(), 1, "a mail emptied of money is still a letter");
    assert_eq!(trins[0].money, 0);
    assert_eq!(
        mail::letter_body(world.as_ref(), Some(TRIN), mail_id).unwrap(),
        Some("left it at the inn".to_string())
    );
}

/// **AC: taking money twice from one mail credits it once.**
#[test]
fn taking_the_money_twice_credits_the_purse_once() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    post_money(world.as_ref(), "Trin", ATTACHED).expect("the send goes through");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;

    mail::take_money(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("the first take");
    let err = mail::take_money(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("there is nothing left in it");

    assert!(err.to_string().contains("nothing to take"), "got {err}");
    assert_eq!(world.purses.lock().unwrap()[1].1, ATTACHED);
}

/// **AC: taking is refused for a mail the caller is not the recipient of.** The mail id rides a
/// client packet, so this is an authorization boundary — and it answers the same as a mail that
/// does not exist, so a crafted id learns nothing.
#[test]
fn taking_money_from_somebody_elses_mail_is_refused() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    post_money(world.as_ref(), "Trin", ATTACHED).expect("the send goes through");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;

    let theft = mail::take_money(world.as_ref(), Some(GINGER), MAILBOX, mail_id)
        .expect_err("Ginger wrote it, they cannot empty it");
    let phantom = mail::take_money(world.as_ref(), Some(GINGER), MAILBOX, mail_id + 999)
        .expect_err("no such mail");

    assert_eq!(
        theft.to_string(),
        phantom.to_string(),
        "'not yours' and 'no such mail' must read the same, or a crafted id enumerates mailboxes"
    );
    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::total_cost(ATTACHED)
    );
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].money,
        ATTACHED,
        "and the copper is still in the letter"
    );
}

/// **AC: both planes behave identically, through one shared core.** Same letter, same take; the
/// only difference is whether a realm handle exists — and on one database none of the escrow is
/// touched, because there the whole thing is one transaction.
#[test]
fn both_planes_attach_and_take_the_same_copper() {
    let (_realm, world, _instances, calls) = sharded_send();
    let single = unsharded_send();
    for store in [&world, &single] {
        *store.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    }

    for store in [world.as_ref(), single.as_ref()] {
        post_money(store, "Trin", ATTACHED).expect("posted");
        let mail_id = mail::open_mailbox(store, Some(TRIN), MAILBOX).unwrap()[0].id;
        mail::take_money(store, Some(TRIN), MAILBOX, mail_id).expect("taken");
    }

    assert_eq!(
        world.purses.lock().unwrap().clone(),
        single.purses.lock().unwrap().clone(),
        "the two planes must move the same copper"
    );
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap(),
        mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX).unwrap(),
    );
    assert!(
        single.mail_escrows.lock().unwrap().is_empty()
            && single.mail_receipts.lock().unwrap().is_empty(),
        "the single-database plane must not route through the escrow — it HAS the transaction"
    );
    assert!(!escrow_steps(&calls).is_empty(), "and the sharded one must");
}

/// **The drive, in order.** Fence on the sender's shard, commit on realm-core, then attest and
/// settle back on the shard. Any other order either destroys a fence whose letter never arrived or
/// delivers one nobody paid for.
#[test]
fn a_sharded_send_drives_fence_then_commit_then_confirm_then_settle() {
    let (_realm, world, _instances, calls) = sharded_send();

    post_money(world.as_ref(), "Trin", ATTACHED).expect("the send goes through");

    assert_eq!(
        escrow_steps(&calls),
        vec![
            ("world".into(), "mail_fence".into()),
            ("lyracore-realm".into(), "mail_commit".into()),
            ("world".into(), "mail_confirm_delivery".into()),
            ("world".into(), "mail_settle".into()),
        ]
    );
}

/// **The take's drive, in order** — the same four steps with the databases swapped, because the
/// copper starts in a mail row on realm-core and ends in a purse on the shard.
#[test]
fn a_sharded_take_drives_the_same_four_steps_the_other_way() {
    let (_realm, world, _instances, calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    post_money(world.as_ref(), "Trin", ATTACHED).expect("posted");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    calls.lock().unwrap().clear();

    mail::take_money(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("taken");

    assert_eq!(
        escrow_steps(&calls),
        vec![
            ("lyracore-realm".into(), "mail_take_money_fence".into()),
            ("world".into(), "mail_payout".into()),
            ("lyracore-realm".into(), "mail_confirm_delivery".into()),
            ("lyracore-realm".into(), "mail_settle".into()),
        ]
    );
}

/// **Interrupted after the fence: the gold is with exactly one owner, and the next mailbox visit
/// rescues it.** The copper has left the purse and no letter exists, so it is held in the fence —
/// never refunded, because the shard cannot ask realm-core whether the letter arrived.
#[test]
fn a_send_killed_before_the_commit_is_re_driven_at_the_next_mailbox_visit() {
    let (realm, world, _instances, _calls) = sharded_send();
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());

    let refusal = post_money(world.as_ref(), "Trin", ATTACHED)
        .expect_err("realm-core never answered the commit");
    assert!(matches!(refusal, mail::SendRefusal::Internal(_)));
    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::total_cost(ATTACHED),
        "the sender has PAID, and nothing refunds a fence"
    );
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "held");
    assert!(mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
        .unwrap()
        .is_empty());

    // realm-core comes back, and Ginger opens their mailbox.
    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(trins.len(), 1, "the letter finally landed — exactly once");
    assert_eq!(trins[0].money, ATTACHED);
    assert!(world.mail_escrows.lock().unwrap().is_empty(), "and settled");
    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::total_cost(ATTACHED),
        "debited once across the whole episode"
    );
}

/// **Interrupted after the commit.** The letter is durable, the fence still stands — the re-drive's
/// commit replays into its receipt, so the recipient gets one letter and not two.
#[test]
fn a_send_killed_after_the_commit_re_drives_into_one_letter() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.mail_kill_at.lock().unwrap() = Some("mail_confirm_delivery".into());

    post_money(world.as_ref(), "Trin", ATTACHED).expect_err("the attestation never landed");
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "still fenced");

    *world.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
            .unwrap()
            .len(),
        1,
        "one letter — the replayed commit found its receipt"
    );
    assert!(world.mail_escrows.lock().unwrap().is_empty());
    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE - lyracore_shared::mail::total_cost(ATTACHED)
    );
}

/// **A take interrupted before the payout.** The copper is out of the mail row and held on
/// realm-core; the next mailbox visit pays it into the purse, once.
#[test]
fn a_take_killed_before_the_payout_is_re_driven_at_the_next_mailbox_visit() {
    let (realm, world, _instances, _calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    post_money(world.as_ref(), "Trin", ATTACHED).expect("posted");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    *world.mail_kill_at.lock().unwrap() = Some("mail_payout".into());

    mail::take_money(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("the purse was never credited");
    assert_eq!(world.purses.lock().unwrap()[1].1, 0);
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].money,
        0,
        "the copper has left the row"
    );
    assert_eq!(realm.mail_escrows.lock().unwrap().len(), 1, "and is held");

    *world.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(world.purses.lock().unwrap()[1].1, ATTACHED, "paid, once");
    assert!(realm.mail_escrows.lock().unwrap().is_empty(), "and settled");
}

/// **A take interrupted after the payout.** The purse holds the copper and the fence still stands;
/// the re-drive's payout replays into its receipt, so nobody is paid twice.
#[test]
fn a_take_killed_after_the_payout_re_drives_into_one_credit() {
    let (realm, world, _instances, _calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, 0)];
    post_money(world.as_ref(), "Trin", ATTACHED).expect("posted");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    *realm.mail_kill_at.lock().unwrap() = Some("mail_confirm_delivery".into());

    mail::take_money(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("the attestation never landed, so the click is reported as failed");
    assert_eq!(
        world.purses.lock().unwrap()[1].1,
        ATTACHED,
        "but the purse WAS credited — which is exactly why the re-drive must not credit it again"
    );
    assert_eq!(realm.mail_escrows.lock().unwrap().len(), 1, "still fenced");

    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(world.purses.lock().unwrap()[1].1, ATTACHED, "credited once");
    assert!(realm.mail_escrows.lock().unwrap().is_empty());
}

/// **Delete-last, from the gateway's side.** A settle driven while the delivery is unattested is
/// refused, so a drive that skipped the attestation cannot destroy a fence — the guard that makes
/// "the copper is nowhere" unreachable.
#[test]
fn a_settle_without_an_attestation_is_refused_and_the_fence_survives() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.mail_kill_at.lock().unwrap() = Some("mail_confirm_delivery".into());
    post_money(world.as_ref(), "Trin", ATTACHED).expect_err("no attestation");

    let escrow_id = world.mail_escrows.lock().unwrap()[0].1.escrow_id;
    let err = world.mail_settle(escrow_id).expect_err("not attested");

    assert!(err.to_string().contains("not attested"), "got {err}");
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "still held");
}

/// **A refused take never reaches the fence.** The mailbox gate and character select both stop it
/// before any copper moves.
#[test]
fn taking_money_is_refused_at_character_select_and_away_from_a_mailbox() {
    let (_realm, world, _instances, calls) = sharded_send();
    post_money(world.as_ref(), "Trin", ATTACHED).expect("posted");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    calls.lock().unwrap().clear();

    assert!(mail::take_money(world.as_ref(), None, MAILBOX, mail_id)
        .unwrap_err()
        .to_string()
        .contains("not in world"));
    assert!(
        mail::take_money(world.as_ref(), Some(TRIN), FAR_MAILBOX, mail_id)
            .unwrap_err()
            .to_string()
            .contains("not at mailbox")
    );

    assert!(escrow_steps(&calls).is_empty(), "no copper moved");
}

/// **AC over the real wire: `CMSG_MAIL_TAKE_MONEY` acks and the purse grows.** The one-database
/// dispatch, end to end.
#[test]
fn taking_money_over_the_wire_acks_and_credits_the_purse() {
    use wow_world_messages::vanilla::{
        SMSG_SEND_MAIL_RESULT_MailAction, SMSG_SEND_MAIL_RESULT_MailResultTwo,
    };
    let store = seated_sender();
    *store.mails.lock().unwrap() = vec![(
        1,
        codec::MailView {
            id: 7,
            sender_guid: TRIN,
            subject: "For you".into(),
            money: ATTACHED,
            created_at_secs: 1_000,
            ..Default::default()
        },
    )];

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

    wow_world_messages::vanilla::CMSG_MAIL_TAKE_MONEY {
        mailbox: Guid::new(MAILBOX),
        mail_id: 7,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => {
            assert_eq!(m.mail_id, 7);
            match m.action {
                SMSG_SEND_MAIL_RESULT_MailAction::MoneyTaken { result2 } => {
                    assert_eq!(result2, SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok)
                }
                other => panic!("expected the MoneyTaken action, got {other:?}"),
            }
        }
        other => panic!("expected SMSG_SEND_MAIL_RESULT, got {other}"),
    }

    drop(client);
    server.join().unwrap();
    assert_eq!(store.purses.lock().unwrap()[0].1, PURSE + ATTACHED);
    assert_eq!(store.mails.lock().unwrap()[0].1.money, 0);
}

// =============================================================================================
//  Item attachments
// =============================================================================================
//
// Coin can be minted back; an item cannot, so every routing test below also asserts WHERE the item
// is. `item_owners` counts the copies somebody could claim right now — bags plus mailbox — and it
// must never exceed one, however the drive is interrupted.

/// Ginger's item instance guid, as the client names it in `CMSG_SEND_MAIL`.
const SWORD_GUID: u64 = 0x4000_0000_0000_0011;

/// A DAMAGED, ENCHANTED weapon — every column mailing must not launder, set to something a
/// template-based re-grant would get wrong. Reserved entry, per the fixture-id rule.
fn sword() -> mail::AttachedItem {
    mail::AttachedItem {
        entry: 5_090_001,
        stack_count: 1,
        durability: 42,
        enchant_id: 7,
        soulbound: false,
    }
}

/// Put `item` in `owner`'s bags on `shard`, as `game_item_instance` would.
fn give_item(shard: &InMemoryStore, owner: u64, guid: u64, item: mail::AttachedItem) {
    shard.mail_items.lock().unwrap().push((guid, owner, item));
}

/// The attachments anyone could claim right now: in bags on either shard, or sitting in a mailbox.
/// A fenced item is deliberately not counted — it is in nobody's reach, which is the point.
fn claimable_swords(shards: &[&InMemoryStore], realm: &InMemoryStore) -> usize {
    let in_bags: usize = shards
        .iter()
        .map(|s| s.mail_items.lock().unwrap().len())
        .sum();
    let in_mail = realm
        .mails
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, m)| m.item_entry != 0)
        .count();
    in_bags + in_mail
}

/// Post the fixture letter with the sword attached.
fn post_item<St: WorldStore + ?Sized>(
    store: &St,
    to: &str,
) -> std::result::Result<(), mail::SendRefusal> {
    mail::send(
        store,
        Some(GINGER),
        MAILBOX,
        to,
        "Your sword".into(),
        "left it at the inn".into(),
        0,
        SWORD_GUID,
    )
}

/// **AC: an attached item leaves the sender's bags at send and appears in the recipient's list with
/// its stack, durability and enchant.** Every column is asserted, because the one that silently
/// resets is a free repair or a laundered enchant.
#[test]
fn a_mailed_item_leaves_the_senders_bags_and_lists_with_its_state_intact() {
    let (realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Trin").expect("Trin is a Human on the same shard");

    assert!(
        world.bags_of(GINGER).is_empty(),
        "the item leaves the bags at SEND — otherwise a send-and-logout duplicates it"
    );
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(trins.len(), 1);
    assert_eq!(trins[0].item_entry, sword().entry);
    assert_eq!(trins[0].item_stack_count, sword().stack_count);
    assert_eq!(
        trins[0].item_durability,
        sword().durability,
        "a damaged item must not arrive repaired"
    );
    assert_eq!(
        trins[0].item_enchant_id,
        sword().enchant_id,
        "an enchanted item must not arrive stripped"
    );
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

/// **AC: both planes deliver the same attachment, through one shared core.** Same letter, same
/// item; the only difference is whether a realm handle exists.
#[test]
fn the_realm_plane_and_the_single_database_fallback_deliver_the_same_attachment() {
    let (_realm, world, _instances, _calls) = sharded_send();
    let single = unsharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    give_item(&single, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Trin").expect("realm plane");
    post_item(single.as_ref(), "Trin").expect("fallback");

    let sharded = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    let unsharded = mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert_eq!(sharded, unsharded, "the two planes must deliver one letter");
    assert!(single.bags_of(GINGER).is_empty());
}

/// **AC: a soulbound instance is refused at send, with its own client error, and stays in the
/// bags.** Its own variant because "soulbound" and "not yours" are different mistakes.
#[test]
fn a_soulbound_attachment_is_refused_at_send_and_stays_in_the_senders_bags() {
    let (_realm, world, _instances, _calls) = sharded_send();
    give_item(
        &world,
        GINGER,
        SWORD_GUID,
        mail::AttachedItem {
            soulbound: true,
            ..sword()
        },
    );

    let refusal = post_item(world.as_ref(), "Trin").expect_err("a bound item is not mailable");

    assert!(matches!(refusal, mail::SendRefusal::AttachmentSoulbound(_)));
    assert_eq!(world.bags_of(GINGER).len(), 1, "still theirs");
    assert_eq!(
        world.purses.lock().unwrap()[0].1,
        PURSE,
        "and a refused send costs nothing"
    );
    assert!(world.mail_escrows.lock().unwrap().is_empty());
}

/// **AC: an unworn bind-on-equip item is mailable.** The verdict reads the INSTANCE's bind state,
/// so the unworn drop a player passes to an alt goes through.
#[test]
fn an_unworn_bind_on_equip_attachment_is_mailable() {
    let (_realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Trin").expect("an unbound instance mails");

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert!(!trins[0].item_soulbound, "it arrives as unbound as it left");
}

/// **AC: attaching an item the sender does not own is refused.** The item guid is client-supplied,
/// so this is the authorization boundary on a send — and a guid that names nothing reads the same.
#[test]
fn attaching_an_item_the_sender_does_not_own_is_refused() {
    let (_realm, world, _instances, _calls) = sharded_send();
    give_item(&world, TRIN, SWORD_GUID, sword());

    let refusal = post_item(world.as_ref(), "Trin").expect_err("it is not Ginger's");
    assert!(matches!(refusal, mail::SendRefusal::AttachmentInvalid(_)));

    world.mail_items.lock().unwrap().clear();
    let refusal = post_item(world.as_ref(), "Trin").expect_err("and nothing answers to that guid");
    assert!(matches!(refusal, mail::SendRefusal::AttachmentInvalid(_)));
    assert!(world.mail_escrows.lock().unwrap().is_empty());
}

/// **A fenced item is reachable by nothing.** Its instance row is gone, so a second letter's
/// attach finds no item — and every other path that reads a player's items resolves through that
/// same table, which is why nothing else had to learn an in-flight rule.
#[test]
fn an_item_in_flight_cannot_be_attached_to_a_second_letter() {
    let (realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());
    post_item(world.as_ref(), "Trin").expect_err("realm-core never answered");
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "held");

    let refusal =
        post_item(world.as_ref(), "Trin").expect_err("it is in flight, so it is nobody's");

    assert!(matches!(refusal, mail::SendRefusal::AttachmentInvalid(_)));
    assert_eq!(
        claimable_swords(&[&world], &realm),
        0,
        "in flight is claimable by nobody"
    );
}

// ---------------------------------------------------------------------------------------------
//  Taking it back out
// ---------------------------------------------------------------------------------------------

/// A letter with the sword, delivered to Trin. Returns the topology plus the mail id.
fn delivered_item() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    ShardCallLog,
    u64,
) {
    let (realm, world, _instances, calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    post_item(world.as_ref(), "Trin").expect("posted");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    calls.lock().unwrap().clear();
    (realm, world, calls, mail_id)
}

/// **AC: `CMSG_MAIL_TAKE_ITEM` places it in the recipient's bags with stack, durability, enchant
/// and bind state unchanged** — and the letter survives with nothing in it.
#[test]
fn taking_an_item_puts_it_in_the_takers_bags_with_its_state_unchanged() {
    let (realm, world, _calls, mail_id) = delivered_item();

    let taken =
        mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("the take completes");

    assert_eq!(
        taken,
        (sword().entry, sword().stack_count),
        "the wire's success arm names what the client just gained"
    );
    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert_eq!(
        trins.len(),
        1,
        "a mail emptied of its item is still a letter"
    );
    assert_eq!(trins[0].item_entry, 0);
    assert!(realm.mail_escrows.lock().unwrap().is_empty(), "settled");
}

/// **The sharded take drives the same four steps the other way**, and probes the taker's own shard
/// for room BEFORE it fences: past the fence the letter is empty, so a full bag found afterwards
/// would strand the item in an escrow rather than leave it in the mail.
#[test]
fn a_sharded_item_take_probes_for_room_before_it_fences_anything() {
    let (_realm, world, calls, mail_id) = delivered_item();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("taken");

    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, what)| what.starts_with("mail_item")
                || what.starts_with("mail_take_item")
                || what == "mail_confirm_delivery"
                || what == "mail_settle")
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            ("world".into(), "mail_item_room".into()),
            ("lyracore-realm".into(), "mail_take_item_fence".into()),
            ("world".into(), "mail_item_payout".into()),
            ("lyracore-realm".into(), "mail_confirm_delivery".into()),
            ("lyracore-realm".into(), "mail_settle".into()),
        ]
    );
}

/// **AC: a take into a full bag is refused with the equip-error variant and the item STAYS in the
/// mail.** The case where a naive implementation destroys it — asserted on BOTH planes, because
/// they reach the refusal by different routes (one transaction versus the pre-fence probe).
#[test]
fn a_take_into_a_full_bag_is_refused_and_the_item_stays_in_the_mail() {
    let (realm, world, _calls, mail_id) = delivered_item();
    world
        .bags_full
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let refusal = mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("there is nowhere to put it");

    assert!(
        matches!(refusal, mail::TakeItemRefusal::BagsFull(_)),
        "the client is told to make room, not handed a generic error: {refusal}"
    );
    assert!(world.bags_of(TRIN).is_empty());
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].item_entry,
        sword().entry,
        "the item is still in the letter — nothing was fenced"
    );
    assert!(realm.mail_escrows.lock().unwrap().is_empty());

    world
        .bags_full
        .store(false, std::sync::atomic::Ordering::Relaxed);
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("once there is room");
    assert_eq!(world.bags_of(TRIN), vec![sword()]);
}

/// The single-database plane reaches the same refusal through its ONE transaction: the grant fails,
/// so the clear never happens and the item is still in the letter.
#[test]
fn a_take_into_a_full_bag_on_one_database_leaves_the_item_in_the_mail() {
    let single = unsharded_send();
    give_item(&single, GINGER, SWORD_GUID, sword());
    post_item(single.as_ref(), "Trin").expect("posted");
    let mail_id = mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    single
        .bags_full
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let refusal = mail::take_item(single.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("there is nowhere to put it");

    assert!(matches!(refusal, mail::TakeItemRefusal::BagsFull(_)));
    assert_eq!(
        mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].item_entry,
        sword().entry
    );
    assert!(single.bags_of(TRIN).is_empty());
}

/// **AC: taking is refused for anyone who is not the mail's recipient.** The mail id is
/// client-supplied, so this is an authorization boundary — and it reads the same as a mail that
/// does not exist, so a crafted id learns nothing.
#[test]
fn taking_an_item_from_a_mail_the_caller_is_not_the_recipient_of_is_refused() {
    let (realm, world, _calls, mail_id) = delivered_item();

    for (who, why) in [(GINGER, "the sender cannot take it back"), (TRIN, "sanity")] {
        let outcome = mail::take_item(world.as_ref(), Some(who), MAILBOX, mail_id);
        if who == GINGER {
            let refusal = outcome.expect_err(why);
            assert!(matches!(refusal, mail::TakeItemRefusal::Other(_)));
            assert!(
                world.bags_of(GINGER).is_empty(),
                "and it is not in their bags"
            );
        } else {
            outcome.expect(why);
        }
    }
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

/// Taking twice grants once: the fence and the clear are one transaction, so the second click
/// finds a letter with nothing in it.
#[test]
fn taking_the_same_item_twice_grants_it_once() {
    let (realm, world, _calls, mail_id) = delivered_item();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("taken");
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("there is nothing left in it");

    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

// ---------------------------------------------------------------------------------------------
//  Interruption — executed at every step, both directions
// ---------------------------------------------------------------------------------------------

/// **A send killed before the commit.** The item has left the bags and no letter exists, so it is
/// held in the fence — never returned, because the shard cannot ask realm-core whether the letter
/// arrived. The next mailbox visit rolls it forward.
#[test]
fn an_item_send_killed_before_the_commit_is_re_driven_at_the_next_mailbox_visit() {
    let (realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());

    post_item(world.as_ref(), "Trin").expect_err("realm-core never answered the commit");
    assert!(world.bags_of(GINGER).is_empty());
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "held");
    assert_eq!(claimable_swords(&[&world], &realm), 0);

    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert_eq!(trins.len(), 1, "the letter finally landed — exactly once");
    assert_eq!(trins[0].item_durability, sword().durability);
    assert!(world.mail_escrows.lock().unwrap().is_empty(), "and settled");
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

/// **A send killed after the commit.** The letter is durable and the fence still stands; the
/// re-drive's commit replays into its receipt, so the item arrives once and not twice.
#[test]
fn an_item_send_killed_after_the_commit_re_drives_into_one_item() {
    let (realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    *world.mail_kill_at.lock().unwrap() = Some("mail_confirm_delivery".into());

    post_item(world.as_ref(), "Trin").expect_err("the attestation never landed");
    assert_eq!(
        claimable_swords(&[&world], &realm),
        1,
        "in the mailbox only"
    );
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "still fenced");

    *world.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
            .unwrap()
            .len(),
        1,
        "one letter — the replayed commit found its receipt"
    );
    assert_eq!(claimable_swords(&[&world], &realm), 1);
    assert!(world.mail_escrows.lock().unwrap().is_empty());
}

/// **A send killed after the attestation.** The fence is attested, so even the re-drive's settle is
/// all that is left to do — and the item is in the mailbox exactly once throughout.
#[test]
fn an_item_send_killed_after_the_attestation_settles_on_the_next_visit() {
    let (realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    *world.mail_kill_at.lock().unwrap() = Some("mail_settle".into());

    post_item(world.as_ref(), "Trin").expect_err("the settle never landed");
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1);
    assert_eq!(claimable_swords(&[&world], &realm), 1);

    *world.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    assert!(world.mail_escrows.lock().unwrap().is_empty(), "settled");
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

/// **A take killed before the payout.** The item is out of the letter and held on realm-core; the
/// next mailbox visit grants it into the bags, once.
#[test]
fn an_item_take_killed_before_the_payout_is_re_driven_at_the_next_mailbox_visit() {
    let (realm, world, _calls, mail_id) = delivered_item();
    *world.mail_kill_at.lock().unwrap() = Some("mail_item_payout".into());

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("the bags were never granted");
    assert!(world.bags_of(TRIN).is_empty());
    assert_eq!(claimable_swords(&[&world], &realm), 0, "held, not lost");
    assert_eq!(realm.mail_escrows.lock().unwrap().len(), 1);

    *world.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(world.bags_of(TRIN), vec![sword()], "granted, once");
    assert!(realm.mail_escrows.lock().unwrap().is_empty(), "and settled");
}

/// **A take killed after the payout.** The bags hold it and the fence still stands; the re-drive's
/// payout replays into its receipt, so the item is created once and not twice.
#[test]
fn an_item_take_killed_after_the_payout_re_drives_into_one_item() {
    let (realm, world, _calls, mail_id) = delivered_item();
    *realm.mail_kill_at.lock().unwrap() = Some("mail_confirm_delivery".into());

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("the attestation never landed");
    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    assert_eq!(realm.mail_escrows.lock().unwrap().len(), 1, "still fenced");

    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(
        world.bags_of(TRIN),
        vec![sword()],
        "granted once, not twice"
    );
    assert_eq!(claimable_swords(&[&world], &realm), 1);
    assert!(realm.mail_escrows.lock().unwrap().is_empty());
}
