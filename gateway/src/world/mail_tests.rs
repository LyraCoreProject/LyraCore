use super::party_tests::{character, DORMANT, GINGER, TRIN, VIM};
use super::*;
const MAILBOX: u64 = 0xF110_0000_0000_0042;
const FAR_MAILBOX: u64 = 0xF110_0000_0000_0099;
const NO_ITEM: u64 = 0;
const NO_COD: u32 = 0;

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
fn seeded_mail() -> Vec<(u64, codec::MailView)> {
    vec![
        (GINGER, mail(1, VIM, "Your sword", "left it at the inn")),
        (TRIN, mail(2, VIM, "Not yours", "for Trin only")),
    ]
}

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

#[test]
fn a_player_with_no_mail_gets_an_empty_list_and_not_a_refusal() {
    let (realm, world, _calls) = sharded_mailbox();
    realm.mails.lock().unwrap().clear();

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("an empty mailbox opens"),
        vec![]
    );
}

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
    let text_ids: Vec<(u32, u32)> = packet
        .mails
        .iter()
        .map(|m| (m.message_id, m.item_text_id))
        .collect();
    assert_eq!(text_ids, vec![(3, 0), (1, 1)]);
}
fn seated_store() -> std::sync::Arc<InMemoryStore> {
    let store = std::sync::Arc::new(InMemoryStore {
        login_entity: Some(warrior_entity()),
        mailboxes: vec![MAILBOX],
        ..tester_store(7)
    });
    *store.mails.lock().unwrap() = vec![(1, mail(1, VIM, "Your sword", "left it at the inn"))];
    store
}

#[test]
fn an_empty_mailbox_is_answered_with_an_empty_list_packet() {
    let store = seated_store();
    store.mails.lock().unwrap().clear();

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..WORLD_ENTRY_PACKETS {
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

#[test]
fn a_seeded_mail_reaches_the_client_as_a_mail_list_row() {
    let store = seated_store();

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..WORLD_ENTRY_PACKETS {
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

#[test]
fn mark_read_flips_the_row_and_the_next_list_shows_it() {
    let (_realm, world, _calls) = sharded_mailbox();

    mail::mark_read(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger owns mail 1");

    let mails = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert!(mails[0].was_read, "the next list read must show the flip");
}

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

#[test]
fn both_are_refused_for_a_mail_the_caller_does_not_own() {
    let (_realm, world, _calls) = sharded_mailbox();
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
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(
        trins.len(),
        1,
        "a refused cross-owner action must not mutate the other mailbox"
    );
    assert!(!trins[0].was_read);
}

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

#[test]
fn returning_a_mail_moves_it_from_the_recipients_list_to_the_senders() {
    let (_realm, world, _calls) = sharded_mailbox();

    mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger owns mail 1");

    let gingers =
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert!(
        gingers.is_empty(),
        "the mail must leave the recipient's list"
    );
    let vims = mail::mail_of(world.as_ref(), VIM).expect("read whatever plane owns the rows");
    assert_eq!(vims.len(), 1);
    assert_eq!(
        vims[0].sender_guid, GINGER,
        "the original recipient is now the sender"
    );
}

#[test]
fn returning_a_mail_carries_its_attachment_and_copper_unchanged() {
    let world = unsharded_mailbox();
    *world.mails.lock().unwrap() = vec![(
        GINGER,
        codec::MailView {
            id: 1,
            sender_guid: VIM,
            subject: "A gift".into(),
            body: "enjoy".into(),
            item_entry: 5_090_001,
            item_stack_count: 3,
            item_durability: 40,
            item_enchant_id: 7,
            money: 250,
            created_at_secs: 1_000,
            ..Default::default()
        },
    )];
    // Vim is Ginger's alt here, so the item comes back at once.
    *world.realm_accounts.lock().unwrap() = alts(&[GINGER, VIM]);

    mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger owns mail 1");

    let vims = mail::mail_of(world.as_ref(), VIM).unwrap();
    assert_eq!(vims.len(), 1);
    assert_eq!(vims[0].sender_guid, GINGER);
    assert_eq!(vims[0].item_entry, 5_090_001);
    assert_eq!(vims[0].item_stack_count, 3);
    assert_eq!(vims[0].item_durability, 40);
    assert_eq!(vims[0].item_enchant_id, 7);
    assert_eq!(
        vims[0].money, 250,
        "the attached copper must ride the return unchanged"
    );
}

#[test]
fn returning_an_already_taken_mail_does_not_duplicate_the_attachment() {
    let world = unsharded_mailbox();
    *world.mails.lock().unwrap() = vec![(
        GINGER,
        codec::MailView {
            id: 1,
            sender_guid: VIM,
            subject: "A gift".into(),
            item_entry: 5_090_001,
            item_stack_count: 1,
            money: 100,
            created_at_secs: 1_000,
            ..Default::default()
        },
    )];

    mail::take_item(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger takes the item");
    assert_eq!(
        world.bags_of(GINGER).len(),
        1,
        "exactly one copy landed in the taker's bags"
    );

    mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("still Ginger's mail");

    let vims = mail::mail_of(world.as_ref(), VIM).unwrap();
    assert_eq!(vims.len(), 1);
    assert_eq!(
        vims[0].item_entry, 0,
        "the already-taken item must not come back as a second copy"
    );
    assert_eq!(
        vims[0].money, 100,
        "the untouched copper still rides the return"
    );
    assert_eq!(
        world.bags_of(GINGER).len(),
        1,
        "the return must not put a second copy in the taker's own bags either"
    );
}

#[test]
fn returning_a_mail_is_refused_for_a_caller_who_does_not_own_it() {
    let (_realm, world, _calls) = sharded_mailbox();
    let err = mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 2)
        .expect_err("mail 2 is not Ginger's");
    assert!(
        err.to_string().contains("not addressed to you"),
        "got {err}"
    );

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");
    assert_eq!(
        trins.len(),
        1,
        "a refused return must not touch a row it does not own"
    );
}

#[test]
fn returning_a_mail_behaves_identically_on_the_realm_plane_and_the_fallback() {
    let (_realm, world, _calls) = sharded_mailbox();
    let single = unsharded_mailbox();

    mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("sharded plane");
    mail::return_to_sender(single.as_ref(), Some(GINGER), MAILBOX, 1).expect("fallback plane");

    assert_eq!(
        mail::mail_of(world.as_ref(), VIM).unwrap(),
        mail::mail_of(single.as_ref(), VIM).unwrap(),
        "both planes must produce the same returned row"
    );
}

#[test]
fn a_sharded_return_lands_on_realm_core_and_never_on_the_players_own_shard() {
    let (_realm, world, calls) = sharded_mailbox();

    mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 1).unwrap();

    let log = calls.lock().unwrap().clone();
    assert!(log.contains(&("lyracore-realm".to_string(), "mail_return".to_string())));
    assert!(
        !log.iter()
            .any(|(shard, call)| shard == "world" && call == "mail_return"),
        "the write must never land on the player's own shard; calls were {log:?}"
    );
}

#[test]
fn returning_a_mail_is_gated_like_the_read_path() {
    let (_realm, world, calls) = sharded_mailbox();

    let err = mail::return_to_sender(world.as_ref(), None, MAILBOX, 1).unwrap_err();
    assert!(err.to_string().contains("not in world"), "got {err}");
    let err = mail::return_to_sender(world.as_ref(), Some(GINGER), FAR_MAILBOX, 1).unwrap_err();
    assert!(err.to_string().contains("not at mailbox"), "got {err}");

    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_return"),
        "a gate refusal must never reach the write; calls were {log:?}"
    );
}

#[test]
fn return_acks_with_send_mail_result_and_the_next_list_is_empty() {
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
    for _ in 0..WORLD_ENTRY_PACKETS {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    wow_world_messages::vanilla::CMSG_MAIL_RETURN_TO_SENDER {
        mailbox_id: Guid::new(MAILBOX),
        mail_id: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => {
            assert_eq!(m.mail_id, 1);
            match m.action {
                wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailAction::ReturnedToSender {
                    result2,
                } => {
                    assert_eq!(
                        result2,
                        wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok
                    );
                }
                other => panic!("expected the ReturnedToSender action, got {other:?}"),
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

#[test]
fn a_refused_return_still_acks_and_never_kills_the_session() {
    let store = seated_store();
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
    for _ in 0..WORLD_ENTRY_PACKETS {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    wow_world_messages::vanilla::CMSG_MAIL_RETURN_TO_SENDER {
        mailbox_id: Guid::new(MAILBOX),
        mail_id: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => match m.action {
            wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailAction::ReturnedToSender {
                result2,
            } => {
                assert_eq!(
                    result2,
                    wow_world_messages::vanilla::SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
                );
            }
            other => panic!("expected the ReturnedToSender action, got {other:?}"),
        },
        other => panic!("expected SMSG_SEND_MAIL_RESULT, got {other}"),
    }
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
    for _ in 0..WORLD_ENTRY_PACKETS {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    wow_world_messages::vanilla::CMSG_MAIL_MARK_AS_READ {
        mailbox: Guid::new(MAILBOX),
        mail_id: 1,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
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
    for _ in 0..WORLD_ENTRY_PACKETS {
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

#[test]
fn a_refused_delete_still_acks_and_never_kills_the_session() {
    let store = seated_store();
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
    for _ in 0..WORLD_ENTRY_PACKETS {
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
fn orc(guid: u64, name: &str) -> codec::CharacterView {
    codec::CharacterView {
        race: 2,
        ..character(guid, name)
    }
}

const GRUG: u64 = 20; // Horde, standing next to Ginger — the faction refusal
const ECHO_WORLD: u64 = 30; // "Echo" on the open world …
const ECHO_INSTANCES: u64 = 31; // … and "Echo" again on the instances shard: the homonym
const PURSE: u32 = 500;
/// Ginger, Trin and Dormant are one player's alts, so an item between them arrives at once, as the
/// take and cash on delivery tests need. Vim plays on another Realm Account.
const ALTS: &str = "GINGER";
const VIMS_ACCOUNT: &str = "VIM";
fn alts(guids: &[u64]) -> Vec<(u64, String)> {
    guids.iter().map(|guid| (*guid, ALTS.to_string())).collect()
}
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
    *world.realm_accounts.lock().unwrap() = alts(&[GINGER, TRIN, DORMANT]);
    *instances.realm_accounts.lock().unwrap() = vec![(VIM, VIMS_ACCOUNT.to_string())];
    for shard in [&world, &instances] {
        *shard.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    }
    (realm, world, instances, calls)
}
fn unsharded_send() -> std::sync::Arc<InMemoryStore> {
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore".into(),
        characters: vec![character(GINGER, "Ginger"), character(TRIN, "Trin")],
        live_guids: vec![GINGER, TRIN],
        mailboxes: vec![MAILBOX],
        ..Default::default()
    });
    *store.purses.lock().unwrap() = vec![(GINGER, PURSE)];
    *store.realm_accounts.lock().unwrap() = alts(&[GINGER, TRIN]);
    store
}

fn post<St: WorldStore + ?Sized>(
    store: &St,
    to: &str,
) -> std::result::Result<(), mail::SendRefusal> {
    post_money(store, to, 0)
}
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
        NO_COD,
        NO_ITEM,
    )
}

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
        NO_COD,
        NO_ITEM,
    )
    .expect_err("a Horde sender cannot write to an Alliance recipient either");
    assert_eq!(refusal, mail::SendRefusal::NotYourTeam);
    assert!(lyracore_shared::faction::same_team(2, 2), "fixture sanity");
}

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
        NO_COD,
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
        NO_COD,
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
    assert_eq!(single.sent_mail.lock().unwrap()[0].4, attached);
    assert!(single.mail_escrows.lock().unwrap().is_empty());
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, call)| call == "mail_send"),
        "a sharded send has no one-transaction path to take; calls were {log:?}"
    );
}

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
    for _ in 0..WORLD_ENTRY_PACKETS {
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
    let store = seated_sender();
    *store.purses.lock().unwrap() = vec![(1, 10)];
    assert_eq!(send_over_the_wire(&store, "Trin"), R::ErrNotEnoughMoney);
}

#[test]
fn a_refused_mail_opcode_costs_a_packet_and_never_the_session() {
    let store = seated_store();

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
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
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..WORLD_ENTRY_PACKETS {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    wow_world_messages::vanilla::CMSG_GET_MAIL_LIST {
        mailbox: Guid::new(FAR_MAILBOX),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
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
const ATTACHED: u32 = 100;
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
    for _ in 0..WORLD_ENTRY_PACKETS {
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
const SWORD_GUID: u64 = 0x4000_0000_0000_0011;
fn sword() -> mail::AttachedItem {
    mail::AttachedItem {
        entry: 5_090_001,
        stack_count: 1,
        durability: 42,
        enchant_id: 7,
        soulbound: false,
        random_property_id: 0,
    }
}
fn give_item(shard: &InMemoryStore, owner: u64, guid: u64, item: mail::AttachedItem) {
    shard.mail_items.lock().unwrap().push((guid, owner, item));
}
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
        NO_COD,
        SWORD_GUID,
    )
}

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

#[test]
fn a_soulbound_attachment_is_refused_at_send_and_stays_in_the_senders_bags() {
    let (_realm, world, _instances, _calls) = sharded_send();
    give_item(
        &world,
        GINGER,
        SWORD_GUID,
        mail::AttachedItem {
            soulbound: true,
            random_property_id: 0,
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

#[test]
fn an_unworn_bind_on_equip_attachment_is_mailable() {
    let (_realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Trin").expect("an unbound instance mails");

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert!(!trins[0].item_soulbound, "it arrives as unbound as it left");
}

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

#[test]
fn taking_the_same_item_twice_grants_it_once() {
    let (realm, world, _calls, mail_id) = delivered_item();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("taken");
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("there is nothing left in it");

    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

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
const COD: u32 = 250;
fn post_cod<St: WorldStore + ?Sized>(
    store: &St,
    to: &str,
    cod: u32,
) -> std::result::Result<(), mail::SendRefusal> {
    mail::send(
        store,
        Some(GINGER),
        MAILBOX,
        to,
        "Your sword".into(),
        "250 and it is yours".into(),
        0,
        cod,
        SWORD_GUID,
    )
}
fn delivered_cod() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    ShardCallLog,
    u64,
) {
    let (realm, world, _instances, calls) = sharded_send();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, PURSE)];
    give_item(&world, GINGER, SWORD_GUID, sword());
    post_cod(world.as_ref(), "Trin", COD).expect("posted");
    let mail_id = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    calls.lock().unwrap().clear();
    (realm, world, calls, mail_id)
}
fn delivered_cod_unsharded() -> (std::sync::Arc<InMemoryStore>, u64) {
    let single = unsharded_send();
    *single.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, PURSE)];
    give_item(&single, GINGER, SWORD_GUID, sword());
    post_cod(single.as_ref(), "Trin", COD).expect("posted");
    let mail_id = mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].id;
    (single, mail_id)
}
fn purse_of(store: &InMemoryStore, guid: u64) -> u32 {
    store
        .purses
        .lock()
        .unwrap()
        .iter()
        .find(|(g, _)| *g == guid)
        .map(|(_, m)| *m)
        .unwrap_or(0)
}

#[test]
fn a_cod_price_is_listed_before_the_recipient_takes_anything() {
    let (_realm, world, _calls, _mail_id) = delivered_cod();

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(trins[0].cod, COD);
    assert_eq!(
        trins[0].item_entry,
        sword().entry,
        "and it is still theirs to buy"
    );
    assert_eq!(purse_of(&world, TRIN), PURSE, "looking costs nothing");
}

#[test]
fn taking_a_priced_item_debits_the_buyer_and_posts_the_copper_to_the_seller() {
    let (realm, world, _calls, mail_id) = delivered_cod();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("Trin can afford it");

    assert_eq!(world.bags_of(TRIN), vec![sword()], "the buyer has the item");
    assert_eq!(
        purse_of(&world, TRIN),
        PURSE - COD,
        "and paid exactly the price"
    );
    let sellers =
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");
    assert_eq!(sellers.len(), 1, "one payment mail");
    assert_eq!(sellers[0].money, COD, "carrying the price");
    assert_eq!(sellers[0].sender_guid, TRIN, "from the buyer");
    mail::take_money(world.as_ref(), Some(GINGER), MAILBOX, sellers[0].id).expect("paid");
    assert_eq!(
        purse_of(&world, GINGER),
        PURSE - lyracore_shared::mail::total_cost(0) + COD,
        "the seller is up the price, less the postage they paid to post the sword"
    );
    assert!(realm.mail_escrows.lock().unwrap().is_empty(), "settled");
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_buyer_who_cannot_afford_the_price_is_refused_and_nothing_moves() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, COD - 1)];

    let refusal = mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("one copper short");

    assert!(
        matches!(refusal, mail::TakeItemRefusal::CannotAffordCod(_)),
        "the buyer must be told to bring gold: {refusal}"
    );
    assert_eq!(purse_of(&world, TRIN), COD - 1, "charged nothing");
    assert!(world.bags_of(TRIN).is_empty());
    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert_eq!(
        trins[0].item_entry,
        sword().entry,
        "the item is still in the mail"
    );
    assert_eq!(trins[0].cod, COD, "and still owed");
    assert!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX)
            .unwrap()
            .is_empty(),
        "and the seller was paid nothing"
    );
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_buyer_who_refuses_the_price_can_return_the_mail_instead() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    *world.purses.lock().unwrap() = vec![(GINGER, PURSE), (TRIN, COD - 1)];
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect_err("cannot pay");

    mail::return_to_sender(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("declined");

    assert!(mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
        .unwrap()
        .is_empty());
    let back = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].item_entry, sword().entry, "the sword went home");
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_returned_priced_mail_does_not_charge_its_own_sender() {
    let (_realm, world, _calls, mail_id) = delivered_cod();
    mail::return_to_sender(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("declined");
    let back = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).unwrap();
    assert_eq!(
        back[0].cod, 0,
        "the price does not travel home with the item"
    );
    let before = purse_of(&world, GINGER);

    mail::take_item(world.as_ref(), Some(GINGER), MAILBOX, back[0].id).expect("their own sword");

    assert_eq!(purse_of(&world, GINGER), before, "and nobody was charged");
    assert_eq!(
        purse_of(&world, TRIN),
        PURSE,
        "least of all paid to the buyer who refused it"
    );
    assert_eq!(world.bags_of(GINGER), vec![sword()]);
}

#[test]
fn taking_a_priced_mail_twice_charges_once() {
    let (realm, world, _calls, mail_id) = delivered_cod();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("bought");
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("there is nothing left in it");

    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "debited once");
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX)
            .unwrap()
            .len(),
        1,
        "and the seller was paid once"
    );
    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn both_planes_settle_the_same_cod() {
    let (_realm, world, _calls, sharded_id) = delivered_cod();
    let (single, single_id) = delivered_cod_unsharded();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, sharded_id).expect("sharded");
    mail::take_item(single.as_ref(), Some(TRIN), MAILBOX, single_id).expect("one database");

    assert_eq!(
        purse_of(&world, TRIN),
        purse_of(&single, TRIN),
        "the two planes must charge the same buyer the same price"
    );
    assert_eq!(world.bags_of(TRIN), single.bags_of(TRIN));
    let sharded_seller = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).unwrap();
    let single_seller = mail::open_mailbox(single.as_ref(), Some(GINGER), MAILBOX).unwrap();
    assert_eq!(sharded_seller.len(), single_seller.len());
    assert_eq!(sharded_seller[0].money, single_seller[0].money);
    assert_eq!(sharded_seller[0].subject, single_seller[0].subject);
    assert!(
        single.mail_escrows.lock().unwrap().is_empty()
            && single.mail_receipts.lock().unwrap().is_empty(),
        "the single-database plane must not route through the escrow — it HAS the transaction"
    );
}

#[test]
fn a_sharded_cod_take_pays_before_it_fences_the_item() {
    let (_realm, world, calls, mail_id) = delivered_cod();

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("bought");

    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, what)| what.starts_with("mail_fence")
                || what.starts_with("mail_commit")
                || what.starts_with("mail_take_item")
                || what.starts_with("mail_item_payout"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            ("world".into(), "mail_fence".into()),
            ("lyracore-realm".into(), "mail_commit".into()),
            ("lyracore-realm".into(), "mail_take_item_fence".into()),
            ("world".into(), "mail_item_payout".into()),
        ],
        "the payment's four steps run to completion BEFORE the item's first one"
    );
}

#[test]
fn a_full_bag_refuses_a_priced_take_before_any_copper_moves() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    world
        .bags_full
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let refusal = mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("nowhere to put it");

    assert!(matches!(refusal, mail::TakeItemRefusal::BagsFull(_)));
    assert_eq!(purse_of(&world, TRIN), PURSE, "charged nothing");
    assert!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX)
            .unwrap()
            .is_empty(),
        "and the seller was paid nothing"
    );
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].cod,
        COD,
        "still owed"
    );
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}
fn assert_the_price_is_in_exactly_one_place(world: &InMemoryStore, realm: &InMemoryStore) {
    let purse = purse_of(world, TRIN);
    assert!(
        purse == PURSE || purse == PURSE - COD,
        "the buyer's purse went somewhere it should not ({purse}): the payment debits it once and \
         nothing credits it back"
    );
    let fenced: u32 = world
        .mail_escrows
        .lock()
        .unwrap()
        .iter()
        .map(|(_, e)| e.money)
        .sum();
    let paid: u32 = realm
        .mails
        .lock()
        .unwrap()
        .iter()
        .filter(|(to, _)| *to == GINGER)
        .map(|(_, m)| m.money)
        .sum();
    if purse == PURSE - COD {
        assert!(
            fenced == COD || paid == COD,
            "the buyer has paid, nothing is fenced and the seller has nothing — the copper is \
             nowhere, which is the one unrecoverable outcome"
        );
    }
    assert!(
        paid <= COD,
        "the seller was paid {paid} for one sale priced at {COD} — a replayed commit must pay once"
    );
}

#[test]
fn a_cod_payment_killed_before_the_commit_is_re_driven_at_the_next_mailbox_visit() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("realm-core never answered the payment");
    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "the buyer has PAID");
    assert_eq!(world.mail_escrows.lock().unwrap().len(), 1, "held");
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].item_entry,
        sword().entry,
        "and the item is still in the letter"
    );
    assert_the_price_is_in_exactly_one_place(&world, &realm);

    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).unwrap()[0].money,
        COD,
        "the seller is paid, once"
    );
    assert!(world.mail_escrows.lock().unwrap().is_empty(), "and settled");
    assert_the_price_is_in_exactly_one_place(&world, &realm);
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_second_click_resumes_a_held_payment_rather_than_charging_twice() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect_err("never committed");
    *realm.mail_kill_at.lock().unwrap() = None;

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("the second click lands");

    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "charged once");
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX)
            .unwrap()
            .len(),
        1,
        "and the seller was paid once"
    );
    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    assert!(world.mail_escrows.lock().unwrap().is_empty());
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_cod_take_killed_after_the_payment_hands_the_item_over_for_free_on_the_next_click() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    *realm.mail_kill_at.lock().unwrap() = Some("mail_take_item_fence".into());

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("the item was never fenced");
    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "paid");
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].cod,
        0,
        "and the price is settled, so a retry must not charge again"
    );
    assert_the_price_is_in_exactly_one_place(&world, &realm);

    *realm.mail_kill_at.lock().unwrap() = None;
    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("the retry lands");

    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "charged once in total");
    assert_eq!(world.bags_of(TRIN), vec![sword()]);
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_cod_take_killed_before_the_item_payout_is_re_driven_at_the_next_mailbox_visit() {
    let (realm, world, _calls, mail_id) = delivered_cod();
    *world.mail_kill_at.lock().unwrap() = Some("mail_item_payout".into());

    mail::take_item(world.as_ref(), Some(TRIN), MAILBOX, mail_id)
        .expect_err("the bags were never granted");
    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "paid");
    assert_eq!(claimable_swords(&[&world], &realm), 0, "held, not lost");
    assert_eq!(realm.mail_escrows.lock().unwrap().len(), 1);
    assert_the_price_is_in_exactly_one_place(&world, &realm);

    *world.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).expect("the gate opens");

    assert_eq!(world.bags_of(TRIN), vec![sword()], "granted, once");
    assert_eq!(purse_of(&world, TRIN), PURSE - COD, "and charged once");
    assert!(realm.mail_escrows.lock().unwrap().is_empty(), "settled");
    assert_eq!(claimable_swords(&[&world], &realm), 1);
}

#[test]
fn a_priced_send_killed_before_the_commit_re_drives_with_its_price_intact() {
    let (realm, world, _instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());

    post_cod(world.as_ref(), "Trin", COD).expect_err("realm-core never answered");
    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    let trins = mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap();
    assert_eq!(trins.len(), 1);
    assert_eq!(trins[0].cod, COD, "the price survived the re-drive");
    assert_eq!(trins[0].item_entry, sword().entry);
}

#[test]
fn a_price_on_a_letter_with_no_attachment_is_dropped() {
    let (_realm, world, _instances, _calls) = sharded_send();

    mail::send(
        world.as_ref(),
        Some(GINGER),
        MAILBOX,
        "Trin",
        "Nothing".into(),
        "".into(),
        0,
        COD,
        NO_ITEM,
    )
    .expect("posted");

    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX).unwrap()[0].cod,
        0
    );
}

#[test]
fn a_refused_priced_take_reaches_the_client_as_not_enough_money() {
    use wow_world_messages::vanilla::{
        SMSG_SEND_MAIL_RESULT_MailAction, SMSG_SEND_MAIL_RESULT_MailResult,
    };
    let store = seated_sender();
    *store.mails.lock().unwrap() = vec![(
        1,
        codec::MailView {
            id: 7,
            sender_guid: TRIN,
            subject: "Your sword".into(),
            item_entry: sword().entry,
            item_stack_count: 1,
            cod: PURSE + 1,
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
    for _ in 0..WORLD_ENTRY_PACKETS {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }

    wow_world_messages::vanilla::CMSG_MAIL_TAKE_ITEM {
        mailbox: Guid::new(MAILBOX),
        mail_id: 7,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_SEND_MAIL_RESULT(m) => match m.action {
            SMSG_SEND_MAIL_RESULT_MailAction::ItemTaken { result } => assert_eq!(
                result,
                SMSG_SEND_MAIL_RESULT_MailResult::ErrNotEnoughMoney {
                    item: 0,
                    item_count: 0
                }
            ),
            other => panic!("expected the ItemTaken action, got {other:?}"),
        },
        other => panic!("expected SMSG_SEND_MAIL_RESULT, got {other}"),
    }

    drop(client);
    server.join().unwrap();
    assert_eq!(store.purses.lock().unwrap()[0].1, PURSE, "charged nothing");
    assert_eq!(store.mails.lock().unwrap()[0].1.item_entry, sword().entry);
}

/// The same mail rows on both planes, as `(the session's store, the store that holds the rows)`:
/// a sharded world handle whose rows live on realm-core, and a single-database store.
fn both_planes(
    mails: Vec<(u64, codec::MailView)>,
) -> [(std::sync::Arc<InMemoryStore>, std::sync::Arc<InMemoryStore>); 2] {
    let (realm, world, _calls) = sharded_mailbox();
    *realm.mails.lock().unwrap() = mails.clone();
    let single = unsharded_mailbox();
    *single.mails.lock().unwrap() = mails;
    [(world, realm), (single.clone(), single)]
}
fn listed(store: &InMemoryStore, guid: u64) -> Vec<wow_world_messages::vanilla::Mail> {
    let mails = mail::open_mailbox(store, Some(guid), MAILBOX).expect("the gate opens");
    codec::build_mail_list(&mails, mail::now_secs()).mails
}
fn listed_ids(store: &InMemoryStore, guid: u64) -> Vec<u32> {
    listed(store, guid).iter().map(|m| m.message_id).collect()
}

#[test]
fn a_read_mail_lists_with_the_read_bit_on_both_planes() {
    for (store, _) in both_planes(seeded_mail()) {
        assert_eq!(
            listed(&store, GINGER)[0].checked_timestamp,
            0,
            "a legacy unread row lists as before"
        );

        mail::mark_read(store.as_ref(), Some(GINGER), MAILBOX, 1).expect("Ginger owns mail 1");

        assert_eq!(listed(&store, GINGER)[0].checked_timestamp, 0x01);
    }
}

#[test]
fn a_posted_letter_lists_with_its_body_bit_and_an_empty_one_as_already_copied() {
    let (_realm, world, _instances, _calls) = sharded_send();
    for store in [world, unsharded_send()] {
        post(store.as_ref(), "Trin").expect("a letter with a body");
        mail::send(
            store.as_ref(),
            Some(GINGER),
            MAILBOX,
            "Trin",
            "No letter".into(),
            String::new(),
            0,
            NO_COD,
            NO_ITEM,
        )
        .expect("a letter without one");

        let checked: Vec<(String, u32)> = listed(&store, TRIN)
            .into_iter()
            .map(|m| (m.subject, m.checked_timestamp))
            .collect();
        assert_eq!(
            checked,
            vec![("No letter".into(), 0x04), ("Your sword".into(), 0x10)],
            "COPIED without a body, HAS_BODY with one"
        );
    }
}

#[test]
fn a_cod_payment_lists_under_the_letters_own_subject_with_the_payment_bit_on_both_planes() {
    let (_realm, world, _calls, sharded_id) = delivered_cod();
    let (single, single_id) = delivered_cod_unsharded();
    for (store, mail_id) in [(world, sharded_id), (single, single_id)] {
        mail::take_item(store.as_ref(), Some(TRIN), MAILBOX, mail_id).expect("Trin buys it");

        let payment = &listed(&store, GINGER)[0];
        assert_eq!(
            payment.subject, "Your sword",
            "the client adds \"COD Payment: \" itself"
        );
        assert_eq!(
            payment.checked_timestamp, 0x08,
            "COD_PAYMENT and nothing else"
        );
        assert_eq!(payment.money.as_int(), COD);
    }
}

#[test]
fn the_mailbox_lists_newest_first_on_both_planes() {
    let at = |id, created_at_secs| {
        (
            GINGER,
            codec::MailView {
                created_at_secs,
                ..mail(id, VIM, "Hello", "")
            },
        )
    };
    for (store, _) in both_planes(vec![at(1, 1_000), at(3, 3_000), at(4, 2_000)]) {
        assert_eq!(listed_ids(&store, GINGER), vec![3, 4, 1]);
    }
}

#[test]
fn the_mailbox_lists_the_fifty_newest_mails_and_the_poll_still_sees_the_rest() {
    let mails = (1..=60)
        .map(|id| {
            (
                GINGER,
                codec::MailView {
                    created_at_secs: 1_000 + id as i64,
                    was_read: id > 10,
                    ..mail(id, VIM, "Hello", "")
                },
            )
        })
        .collect();
    for (store, _) in both_planes(mails) {
        let ids = listed_ids(&store, GINGER);
        assert_eq!(ids.len(), 50);
        assert_eq!((ids[0], ids[49]), (60, 11));
        assert!(
            mail::has_unread(store.as_ref(), Some(GINGER)).unwrap(),
            "the ten oldest mails are unread and past the cap, and they still light the envelope"
        );
    }
}

fn arriving_in(secs: i64, id: u64) -> (u64, codec::MailView) {
    (
        GINGER,
        codec::MailView {
            deliver_secs: mail::now_secs() + secs,
            money: 100,
            item_entry: sword().entry,
            item_stack_count: 1,
            ..mail(id, VIM, "On its way", "see you soon")
        },
    )
}

#[test]
fn a_mail_not_yet_delivered_is_absent_from_the_list_the_poll_and_the_body_read() {
    for (store, _) in both_planes(vec![arriving_in(3_600, 1), arriving_in(-1, 5)]) {
        assert_eq!(
            listed_ids(&store, GINGER),
            vec![5],
            "only the mail whose delivery instant has passed"
        );
        assert_eq!(
            mail::letter_body(store.as_ref(), Some(GINGER), 1).unwrap(),
            None
        );

        mail::mark_read(store.as_ref(), Some(GINGER), MAILBOX, 5).unwrap();
        assert!(
            !mail::has_unread(store.as_ref(), Some(GINGER)).unwrap(),
            "an unread mail still on its way does not light the envelope"
        );
    }
}

#[test]
fn a_mail_not_yet_delivered_cannot_be_read_deleted_taken_from_or_returned() {
    let rows = vec![arriving_in(3_600, 1)];
    for (store, holder) in both_planes(rows.clone()) {
        mail::mark_read(store.as_ref(), Some(GINGER), MAILBOX, 1).expect_err("nothing to read yet");
        mail::delete(store.as_ref(), Some(GINGER), MAILBOX, 1).expect_err("nothing to delete yet");
        mail::take_money(store.as_ref(), Some(GINGER), MAILBOX, 1).expect_err("no copper yet");
        mail::take_item(store.as_ref(), Some(GINGER), MAILBOX, 1).expect_err("no item yet");
        mail::return_to_sender(store.as_ref(), Some(GINGER), MAILBOX, 1)
            .expect_err("nothing to return yet");

        assert_eq!(
            *holder.mails.lock().unwrap(),
            rows,
            "every refusal left the mail as it was"
        );
        assert!(store.bags_of(GINGER).is_empty());
    }
}

#[test]
fn a_mail_with_nobody_to_take_it_back_cannot_be_returned_on_either_plane() {
    use lyracore_shared::mail::MailSender;
    let from = |id, sender: MailSender| {
        let (sender_kind, sender_guid, sender_entry) = sender.columns();
        (
            GINGER,
            codec::MailView {
                sender_kind,
                sender_guid,
                sender_entry,
                money: 100,
                ..mail(id, 0, "Auction outbid", "")
            },
        )
    };
    let rows = vec![
        from(1, MailSender::Character(0)),
        from(3, MailSender::AuctionHouse(7)),
        from(4, MailSender::Creature(11_811)),
    ];
    for (store, holder) in both_planes(rows.clone()) {
        for id in [1, 3, 4] {
            let err = mail::return_to_sender(store.as_ref(), Some(GINGER), MAILBOX, id)
                .expect_err("nobody to return it to");
            assert!(
                err.to_string()
                    .contains(lyracore_shared::mail::NO_SENDER_TO_RETURN_TO),
                "mail {id}: {err}"
            );
        }
        assert_eq!(
            *holder.mails.lock().unwrap(),
            rows,
            "the copper stays with Ginger"
        );
    }
}

#[test]
fn a_returned_mail_cannot_be_returned_again() {
    for (store, _) in both_planes(seeded_mail()) {
        mail::return_to_sender(store.as_ref(), Some(GINGER), MAILBOX, 1).expect("back to Vim");

        let err = mail::return_to_sender(store.as_ref(), Some(VIM), MAILBOX, 1)
            .expect_err("a mail goes back once");

        assert!(
            err.to_string()
                .contains(lyracore_shared::mail::ALREADY_RETURNED),
            "{err}"
        );
        assert_eq!(mail::mail_of(store.as_ref(), VIM).unwrap().len(), 1);
    }
}

#[test]
fn a_returned_mail_lists_as_returned_and_unread_with_no_price_and_a_fresh_countdown() {
    let priced = (
        GINGER,
        codec::MailView {
            was_read: true,
            cod: COD,
            item_entry: sword().entry,
            item_stack_count: 1,
            check_flags: lyracore_shared::mail::CHECK_MASK_HAS_BODY,
            ..mail(1, VIM, "Your sword", "250 and it is yours")
        },
    );
    for (store, _) in both_planes(vec![priced]) {
        // Vim is Ginger's alt here, so the item comes back at once.
        *store.realm_accounts.lock().unwrap() = alts(&[GINGER, VIM]);
        mail::return_to_sender(store.as_ref(), Some(GINGER), MAILBOX, 1).expect("declined");

        let back = &listed(&store, VIM)[0];
        assert_eq!(back.checked_timestamp, 0x02, "RETURNED, and unread");
        assert_eq!(back.cash_on_delivery_amount, 0);
        assert!(
            back.expiration_time > 29.99,
            "the 30-day clock restarts at the return, got {}",
            back.expiration_time
        );
    }
}

#[test]
fn a_priced_mail_cannot_be_deleted_on_either_plane() {
    let (_realm, world, _calls, sharded_id) = delivered_cod();
    let (single, single_id) = delivered_cod_unsharded();
    for (store, mail_id) in [(world, sharded_id), (single, single_id)] {
        let err = mail::delete(store.as_ref(), Some(TRIN), MAILBOX, mail_id)
            .expect_err("a price is owed");

        assert!(
            err.to_string()
                .contains(lyracore_shared::mail::COD_MAIL_UNDELETABLE),
            "{err}"
        );
        assert_eq!(listed(&store, TRIN)[0].cash_on_delivery_amount, COD);
    }
}

#[test]
fn deleting_a_priced_mail_answers_the_internal_error_and_keeps_the_mail() {
    let store = seated_store();
    store.mails.lock().unwrap()[0].1.cod = COD;

    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..WORLD_ENTRY_PACKETS {
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

    drop(client);
    server.join().unwrap();
    assert_eq!(
        store.mails.lock().unwrap().len(),
        1,
        "the priced mail stays"
    );
}

/// Every row `recipient` holds on `plane`, delivered or not.
fn held_for(plane: &InMemoryStore, recipient: u64) -> Vec<codec::MailView> {
    plane
        .mails
        .lock()
        .unwrap()
        .iter()
        .filter(|(to, _)| *to == recipient)
        .map(|(_, m)| m.clone())
        .collect()
}
/// The `same_account` answer each mail send, fence and return on `store` carried.
fn same_account_seen(store: &InMemoryStore) -> Vec<(&'static str, bool)> {
    store.same_account_seen.lock().unwrap().clone()
}

#[test]
fn an_item_to_another_account_is_hidden_for_its_delivery_delay() {
    let (realm, world, instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Vim").expect("posted");

    assert_eq!(same_account_seen(&world), [("mail_fence", false)]);
    assert!(
        mail::open_mailbox(instances.as_ref(), Some(VIM), MAILBOX)
            .unwrap()
            .is_empty(),
        "Vim sees nothing for the hour"
    );
    let held = held_for(&realm, VIM);
    assert!(held[0].deliver_secs > mail::now_secs(), "{held:?}");
    mail::take_item(instances.as_ref(), Some(VIM), MAILBOX, held[0].id)
        .expect_err("nothing to take yet");
}

#[test]
fn copper_to_another_account_arrives_at_once() {
    let (_realm, world, instances, _calls) = sharded_send();

    post_money(world.as_ref(), "Vim", ATTACHED).expect("posted");

    assert_eq!(same_account_seen(&world), [("mail_fence", false)]);
    assert_eq!(
        mail::open_mailbox(instances.as_ref(), Some(VIM), MAILBOX)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn an_item_to_an_alt_on_another_shard_arrives_at_once() {
    let (_realm, world, instances, _calls) = sharded_send();
    // Vim is Ginger's alt. Only the instances Shard, where Vim lives, names his Realm Account.
    *instances.realm_accounts.lock().unwrap() = alts(&[VIM]);
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Vim").expect("posted");

    assert_eq!(same_account_seen(&world), [("mail_fence", true)]);
    assert_eq!(
        mail::open_mailbox(instances.as_ref(), Some(VIM), MAILBOX)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_shard_that_holds_a_character_on_a_shadow_account_defers_to_another_shard() {
    let (_realm, world, instances, _calls) = sharded_send();
    // Ginger came over from the instances Shard. The world Shard holds her on a shadow Account and
    // cannot name her Realm Account, and the instances Shard kept her Account Character Owner.
    *world.realm_accounts.lock().unwrap() = alts(&[TRIN]);
    *instances.realm_accounts.lock().unwrap() = alts(&[GINGER]);
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Trin").expect("posted");

    assert_eq!(same_account_seen(&world), [("mail_fence", true)]);
    assert_eq!(
        mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_character_no_shard_can_name_counts_as_another_account() {
    let (_realm, world, _instances, _calls) = sharded_send();
    *world.realm_accounts.lock().unwrap() = alts(&[GINGER]);
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_item(world.as_ref(), "Trin").expect("posted");

    assert_eq!(same_account_seen(&world), [("mail_fence", false)]);
    assert!(mail::open_mailbox(world.as_ref(), Some(TRIN), MAILBOX)
        .unwrap()
        .is_empty());
}

#[test]
fn the_single_database_send_carries_the_same_account_answer() {
    for (accounts, same_account, listed) in [
        (alts(&[GINGER, TRIN]), true, 1),
        (vec![(GINGER, ALTS.to_string())], false, 0),
    ] {
        let single = unsharded_send();
        *single.realm_accounts.lock().unwrap() = accounts;
        give_item(&single, GINGER, SWORD_GUID, sword());

        post_item(single.as_ref(), "Trin").expect("posted");

        assert_eq!(same_account_seen(&single), [("mail_send", same_account)]);
        assert_eq!(
            mail::open_mailbox(single.as_ref(), Some(TRIN), MAILBOX)
                .unwrap()
                .len(),
            listed
        );
    }
}

#[test]
fn an_item_returned_to_another_account_waits_and_to_an_alt_does_not() {
    for (vims_account, same_account, listed) in [(VIMS_ACCOUNT, false, 0), (ALTS, true, 1)] {
        let (realm, world, instances, _calls) = sharded_send();
        *instances.realm_accounts.lock().unwrap() = vec![(VIM, vims_account.to_string())];
        *realm.mails.lock().unwrap() = vec![(
            GINGER,
            codec::MailView {
                item_entry: sword().entry,
                item_stack_count: 1,
                ..mail(1, VIM, "A gift", "enjoy")
            },
        )];

        mail::return_to_sender(world.as_ref(), Some(GINGER), MAILBOX, 1).expect("returned");

        assert_eq!(same_account_seen(&realm), [("mail_return", same_account)]);
        assert_eq!(
            mail::open_mailbox(instances.as_ref(), Some(VIM), MAILBOX)
                .unwrap()
                .len(),
            listed,
            "Vim on {vims_account}"
        );
        assert_eq!(
            held_for(&realm, VIM).len(),
            1,
            "the sword is on its way back"
        );
    }
}

#[test]
fn an_item_send_re_driven_after_a_restart_keeps_its_delivery_delay() {
    let (realm, world, instances, _calls) = sharded_send();
    give_item(&world, GINGER, SWORD_GUID, sword());
    *realm.mail_kill_at.lock().unwrap() = Some("mail_commit".into());

    post_item(world.as_ref(), "Vim").expect_err("realm-core never answered the commit");
    assert_eq!(
        world.mail_escrows.lock().unwrap()[0].1.delivery_delay_secs,
        3_600,
        "the fence holds the hour"
    );

    *realm.mail_kill_at.lock().unwrap() = None;
    mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).expect("the gate opens");

    assert!(world.mail_escrows.lock().unwrap().is_empty(), "settled");
    assert_eq!(held_for(&realm, VIM).len(), 1, "committed once");
    assert!(
        mail::open_mailbox(instances.as_ref(), Some(VIM), MAILBOX)
            .unwrap()
            .is_empty(),
        "the re-driven letter still waits its hour"
    );
}

#[test]
fn a_cod_letter_to_another_account_waits_and_its_payment_arrives_at_once() {
    let (realm, world, instances, _calls) = sharded_send();
    *instances.purses.lock().unwrap() = vec![(VIM, PURSE)];
    give_item(&world, GINGER, SWORD_GUID, sword());

    post_cod(world.as_ref(), "Vim", COD).expect("posted");

    let id = held_for(&realm, VIM)[0].id;
    mail::take_item(instances.as_ref(), Some(VIM), MAILBOX, id).expect_err("nothing to buy yet");
    assert_eq!(purse_of(&instances, VIM), PURSE, "and nothing is charged");

    // An hour later.
    for (_, m) in realm.mails.lock().unwrap().iter_mut() {
        m.deliver_secs = mail::now_secs();
    }
    mail::take_item(instances.as_ref(), Some(VIM), MAILBOX, id).expect("bought");

    assert_eq!(purse_of(&instances, VIM), PURSE - COD);
    let gingers = mail::open_mailbox(world.as_ref(), Some(GINGER), MAILBOX).unwrap();
    assert_eq!(
        (gingers.len(), gingers[0].money),
        (1, COD),
        "the payment arrives at once (cmangos MailHandler.cpp:475-477)"
    );
}
