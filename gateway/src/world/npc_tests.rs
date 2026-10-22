use super::*;

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
        required_skill: 43,
        required_skill_rank: 150,
        required_reputation_faction: 72,
        required_reputation_rank: 5,
        allowed_class: 0x01,
        allowed_race: 0x01,
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

    let client_can_use = |class: u32, race: u32, skill: Option<(u32, u32)>| {
        found.allowed_class.as_int() & class != 0
            && found.allowed_race.as_int() & race != 0
            && skill.is_some_and(|(id, rank)| {
                id == u32::from(found.required_skill.as_int()) && rank >= found.required_skill_rank
            })
    };
    assert!(client_can_use(0x01, 0x01, Some((43, 150))));
    assert!(!client_can_use(0x80, 0x01, Some((43, 150))));
    assert!(!client_can_use(0x01, 0x02, Some((43, 150))));
    assert!(!client_can_use(0x01, 0x01, None));

    drop(client);
    server.join().unwrap();
}

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

fn enter_world_unjoined(
    store: std::sync::Arc<WorldFake>,
    guid: u64,
) -> (
    std::os::unix::net::UnixStream,
    EncrypterHalf,
    DecrypterHalf,
    std::thread::JoinHandle<Result<()>>,
) {
    let (mut client, server_end) = world_session_socket_pair();
    let server = std::thread::spawn(move || run_world_session(server_end, store));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(guid),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    (client, c_enc, c_dec, server)
}

#[test]
fn inspect_transport_loss_ends_the_world_session() {
    let mut s = quest_store();
    s.npc.inspect_error =
        Some(|| crate::stdb::ReducerCallError::transport_lost("gw_inspect").into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world_unjoined(store, 1);
    CMSG_INSPECT {
        guid: Guid::new(55),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    while let Ok(message) = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
        assert!(
            !matches!(message, ServerOpcodeMessage::SMSG_INSPECT(_)),
            "a lost inspect is not an acknowledgement"
        );
    }
    assert!(
        server.join().unwrap().is_err(),
        "a Transport Loss ends the World Session"
    );
}

#[test]
fn a_say_lost_to_a_transport_loss_ends_the_world_session() {
    let mut s = quest_store();
    s.speech.send_chat_error =
        Some(|| crate::stdb::ReducerCallError::transport_lost("gw_send_chat").into());
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, _c_dec, server) = enter_world_unjoined(store, 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: "hello".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(
        server.join().unwrap().is_err(),
        "a Transport Loss ends the World Session"
    );
}

#[test]
fn a_gm_command_for_an_unknown_account_answers_a_system_line_and_keeps_the_session() {
    let mut s = quest_store();
    s.speech.gm_command_error = Some(|| {
        crate::stdb::ReducerCallError::refused(
            "gm_command",
            "no Account named TESTER on Realm-core",
        )
        .into()
    });
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world_unjoined(store, 1);
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Say,
        language: Language::Universal,
        message: ".speed 3".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_MESSAGECHAT(m) => {
            assert_eq!(m.message, "no Account named TESTER on Realm-core");
        }
        other => panic!("expected SMSG_MESSAGECHAT, got {other}"),
    }
    drop(client);
    assert!(server.join().unwrap().is_ok(), "the session goes on");
}

#[test]
fn inspect_refused_target_sends_no_reply() {
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
    CMSG_INSPECT { guid: Guid::new(0) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_PLAYED_TIME {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PLAYED_TIME(_) => {}
        other => {
            panic!("expected SMSG_PLAYED_TIME (no SMSG_INSPECT for refused target), got {other}")
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
fn gossip_select_on_a_spirit_healer_option_asks_for_confirmation() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Return me to life.", gossip_option::SPIRITHEALER)];
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
        ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(confirm) => {
            assert_eq!(confirm.guid.guid(), 90)
        }
        other => panic!("expected SMSG_SPIRIT_HEALER_CONFIRM, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_select_on_a_vendor_opens_the_inventory_window() {
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
    let ServerOpcodeMessage::SMSG_BINDER_CONFIRM(confirm) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected innkeeper confirmation");
    };
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    wow_world_messages::vanilla::CMSG_BINDER_ACTIVATE { guid: confirm.guid }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0158, vec![81, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0])
    );
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
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

#[test]
fn gossip_hello_renders_imported_options_verbatim() {
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
            assert_eq!(m.gossips.len(), 3, "3 imported options: {:?}", m.gossips);
            assert_eq!(m.gossips[0].message, "Well met, traveler.");
            assert_eq!(m.gossips[1].message, "I'd like to browse your goods.");
            assert_eq!(m.gossips[2].message, "I'd like to stay here a while.");
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
    s.npc.innkeeper = true;
    s.npc.gossip_opts = vec![
        opt(0, "Chat.", gossip_option::GOSSIP),
        opt(0, "Stay here.", gossip_option::INNKEEPER),
    ];
    s.npc.gossip_opts[0].row_id = 4001;
    s.npc.gossip_opts[1].row_id = 4002;
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
    let ServerOpcodeMessage::SMSG_BINDER_CONFIRM(confirm) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected innkeeper confirmation");
    };
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    wow_world_messages::vanilla::CMSG_BINDER_ACTIVATE { guid: confirm.guid }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0158, vec![90, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0])
    );
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
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
    let mut a = quest_store();
    a.npc.innkeeper = true;
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
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_BINDER_CONFIRM(_)
    ));
    drop(client);
    server.join().unwrap();

    let mut b = quest_store();
    b.npc.innkeeper = true;
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
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_BINDER_CONFIRM(_)
    ));
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
    s.npc.innkeeper = true;
    let mut gated = opt(0, "About that favor...", gossip_option::GOSSIP);
    gated.row_id = 4001;
    gated.cond_type = gossip_condition::QUEST_TAKEN;
    gated.cond_value1 = 60;
    let mut inn = opt(0, "Stay here.", gossip_option::INNKEEPER);
    inn.row_id = 4002;
    s.npc.gossip_opts = vec![gated, inn];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let rendered = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(rendered.gossips[0].message, "Stay here.");
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
    let ServerOpcodeMessage::SMSG_BINDER_CONFIRM(confirm) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected innkeeper confirmation");
    };
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    wow_world_messages::vanilla::CMSG_BINDER_ACTIVATE { guid: confirm.guid }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0158, vec![90, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0])
    );
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
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
    assert!(
        store.npc.gossip_selects.lock().unwrap().is_empty(),
        "a click outside the open menu must not fire a hook"
    );
}

#[test]
fn a_select_past_the_last_option_just_closes_the_window() {
    use lyracore_shared::constants::gossip_option;
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "Stay here.", gossip_option::INNKEEPER)];
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: menu.gossips.len() as u32,
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
    assert!(store.npc.gossip_selects.lock().unwrap().is_empty());
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
    assert_eq!(menu.gossips.len(), 2, "chat + browse goods");
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
    let ServerOpcodeMessage::SMSG_BINDER_CONFIRM(confirm) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected innkeeper confirmation");
    };
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    wow_world_messages::vanilla::CMSG_BINDER_ACTIVATE { guid: confirm.guid }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0158, vec![90, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0])
    );
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
    drop(client);
    server.join().unwrap();
    assert!(store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
}

fn quest_store_at_level(level: u8) -> WorldFake {
    let mut store = quest_store();
    store.characters[0].level = level;
    store.session.login_entity.as_mut().unwrap().level = u32::from(level);
    store
}

#[test]
fn gossip_hello_hides_unlearn_talents_below_level_10() {
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
                1,
                "training only, no unlearn option: {:?}",
                m.gossips
            );
            assert_eq!(m.gossips[0].message, "I require warrior training.");
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
    s.npc.gossip_opts[0].row_id = 4001;
    s.npc.gossip_opts[1].row_id = 4002;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 2, "training + unlearn: {:?}", m.gossips);
            assert_eq!(m.gossips[1].message, "I wish to unlearn my talents.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
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
    let ServerOpcodeMessage::MSG_TALENT_WIPE_CONFIRM(confirm) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected talent reset quote");
    };
    assert_eq!(confirm.cost_in_copper, 10_000);
    assert!(store.trainer.reset_talents_calls.lock().unwrap().is_empty());
    wow_world_messages::vanilla::MSG_TALENT_WIPE_CONFIRM_Client {
        wiping_npc: confirm.wiping_npc,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
    drop(client);
    server.join().unwrap();
    let calls = store.trainer.reset_talents_calls.lock().unwrap();
    assert_eq!(
        calls.as_slice(),
        &[(1, 90)],
        "reset_talents must have been called with (actor guid, trainer_guid)"
    );
}

#[test]
fn gossip_hello_shows_a_quest_gated_option_once_the_quest_is_taken() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.npc.gossip_opts = vec![opt(0, "About that favor...", gossip_option::GOSSIP)];
    s.npc.gossip_opts[0].cond_type = gossip_condition::QUEST_TAKEN;
    s.npc.gossip_opts[0].cond_value1 = 60;
    s.quest.quest_log = vec![(60, false)].into();
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 1, "{:?}", m.gossips);
            assert_eq!(m.gossips[0].message, "About that favor...");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hello_and_select_option_stay_position_aligned_under_a_hidden_option() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut s = quest_store();
    s.npc.innkeeper = true;
    s.npc.gossip_opts = vec![
        opt(0, "Chat.", gossip_option::GOSSIP),
        opt(0, "Hidden favor.", gossip_option::GOSSIP),
        opt(0, "Stay here.", gossip_option::INNKEEPER),
    ];
    s.npc.gossip_opts[0].row_id = 4001;
    s.npc.gossip_opts[1].row_id = 4002;
    s.npc.gossip_opts[2].row_id = 4003;
    s.npc.gossip_opts[1].cond_type = gossip_condition::QUEST_TAKEN;
    s.npc.gossip_opts[1].cond_value1 = 60;
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(m) => {
            assert_eq!(m.gossips.len(), 2, "2 visible options: {:?}", m.gossips);
            assert_eq!(m.gossips[0].message, "Chat.");
            assert_eq!(m.gossips[1].message, "Stay here.");
        }
        other => panic!("expected SMSG_GOSSIP_MESSAGE, got {other}"),
    }
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
    let ServerOpcodeMessage::SMSG_BINDER_CONFIRM(confirm) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected innkeeper confirmation");
    };
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    wow_world_messages::vanilla::CMSG_BINDER_ACTIVATE { guid: confirm.guid }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x0158, vec![90, 0, 0, 0, 0, 0, 0, 0, 12, 0, 0, 0])
    );
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
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
            assert_eq!(u.texts[1].probability, 0.0);
        }
        other => panic!("expected SMSG_NPC_TEXT_UPDATE, got {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_training_selection_sends_the_profession_list_type() {
    use lyracore_shared::constants::gossip_option;
    let mut store = quest_store();
    store.npc.gossip_opts = vec![opt(3, "Train me.", gossip_option::TRAINER)];
    store.trainer.spells = vec![codec::TrainerSpellView {
        spell_id: 4036,
        cost: 0,
        required_level: 1,
        player_level: 10,
        known: false,
        learn_skill_line: 202,
    }];
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(std::sync::Arc::new(store), 1);
    CMSG_GOSSIP_HELLO {
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(menu) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected a gossip menu");
    };
    assert_eq!(menu.gossips[0].item_icon, 3);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: menu.gossips[0].id,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let ServerOpcodeMessage::SMSG_TRAINER_LIST(list) =
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap()
    else {
        panic!("expected a trainer list");
    };
    assert_eq!(list.guid, Guid::new(90));
    assert_eq!(list.trainer_type, 2);
    assert_eq!(list.spells[0].spell, 4036);
    drop(client);
    server.join().unwrap();
}

#[test]
fn gossip_hides_the_train_and_unlearn_options_for_a_class_the_trainer_does_not_serve() {
    use lyracore_shared::constants::gossip_option;
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

#[test]
fn a_quest_rewarded_after_hello_refuses_the_vendor_click_before_its_action() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut fixture = quest_store();
    let mut option = opt(1, "Quest supplies.", gossip_option::VENDOR);
    option.row_id = 509_9001;
    option.cond_type = gossip_condition::QUEST_TAKEN;
    option.cond_value1 = 60;
    fixture.npc.gossip_opts = vec![option];
    fixture.quest.quest_log = vec![(60, false)].into();
    fixture.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        ..Default::default()
    }];
    let store = std::sync::Arc::new(fixture);
    let (mut client, mut c_enc, mut c_dec, world) = enter_world(store.clone(), 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(menu.gossips[0].message, "Quest supplies.");
    store.quest.quest_log.lock().unwrap()[0].1 = true;
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
    ));
    CMSG_NPC_TEXT_QUERY {
        text_id: 1,
        guid: Guid::new(90),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert!(
        matches!(
            ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
            ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(_)
        ),
        "the refused click must not queue a vendor window"
    );
    drop(client);
    world.join().unwrap();
    assert!(
        store.npc.gossip_selects.lock().unwrap().is_empty(),
        "the refused click must not fire the hook"
    );
}

#[test]
fn a_condition_hidden_service_option_is_not_recreated_by_synthesis() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut fixture = quest_store();
    fixture.npc.innkeeper = true;
    fixture.vendor.vendor_stock = vec![codec::VendorItemView {
        item_entry: 4540,
        ..Default::default()
    }];
    fixture.npc.gossip_opts = [gossip_option::VENDOR, gossip_option::INNKEEPER]
        .into_iter()
        .map(|action| {
            let mut option = opt(0, "Unavailable service.", action);
            option.cond_type = gossip_condition::REFERENCE;
            option.cond_value1 = 509_9001;
            option
        })
        .collect();
    let store = std::sync::Arc::new(fixture);
    let (mut client, mut c_enc, mut c_dec, world) = enter_world(store, 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert!(menu.gossips.is_empty(), "{:?}", menu.gossips);
    drop(client);
    world.join().unwrap();
}

#[test]
fn gossip_menu_distinguishes_complete_and_incomplete_quest_options() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    let mut fixture = quest_store();
    fixture.quest.quest_log = vec![(60, false)].into();
    fixture.quest.quest_log_slots = vec![codec::update_mask::QuestLogSlot {
        slot: 0,
        quest_id: 60,
        counts: vec![],
        state: 1,
        timer: 0,
    }];
    fixture.npc.gossip_opts = [(1, "Still working."), (2, "Ready to report.")]
        .into_iter()
        .map(|(state, text)| {
            let mut option = opt(0, text, gossip_option::GOSSIP);
            option.cond_type = gossip_condition::QUEST_TAKEN;
            option.cond_value1 = 60;
            option.cond_value2 = state;
            option
        })
        .collect();
    let store = std::sync::Arc::new(fixture);
    let (mut client, mut c_enc, mut c_dec, world) = enter_world(store, 1);
    let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    assert_eq!(menu.gossips.len(), 1);
    assert_eq!(menu.gossips[0].message, "Ready to report.");
    drop(client);
    world.join().unwrap();
}

#[test]
fn a_menu_source_switch_refuses_colliding_and_synthesized_clicks_before_service_actions() {
    use lyracore_shared::constants::gossip_option;
    use std::sync::atomic::Ordering;

    for (before, after) in [(0, 509_9001), (509_9001, 509_9002), (509_9001, 0)] {
        for synthesized in [false, true] {
            let mut fixture = quest_store();
            let mut option = opt(1, "Supplies.", gossip_option::VENDOR);
            option.row_id = 509_9001;
            let options = if synthesized {
                Vec::new()
            } else {
                vec![option]
            };
            fixture.npc.gossip_opts = options.clone();
            fixture.npc.gossip_profiles = [(509_9001, options.clone()), (509_9002, options)].into();
            fixture
                .npc
                .gossip_source_menu_id
                .store(before, Ordering::SeqCst);
            fixture.vendor.vendor_stock = vec![codec::VendorItemView {
                item_entry: 4540,
                ..Default::default()
            }];
            let store = std::sync::Arc::new(fixture);
            let (mut client, mut c_enc, mut c_dec, world) = enter_world(store.clone(), 1);
            let menu = gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
            assert_eq!(menu.gossips.len(), 1);
            store
                .npc
                .gossip_source_menu_id
                .store(after, Ordering::SeqCst);
            CMSG_GOSSIP_SELECT_OPTION {
                guid: Guid::new(90),
                gossip_list_id: 0,
                unknown: None,
            }
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
            assert!(matches!(
                ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
                ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE
            ));
            CMSG_NPC_TEXT_QUERY {
                text_id: 1,
                guid: Guid::new(90),
            }
            .write_encrypted_client(&mut client, &mut c_enc)
            .unwrap();
            assert!(
                matches!(
                    ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
                    ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(_)
                ),
                "menu {before} to {after} must not queue a vendor window"
            );
            drop(client);
            world.join().unwrap();
            assert!(
                store.npc.gossip_selects.lock().unwrap().is_empty(),
                "menu {before} to {after} must not fire the hook"
            );
        }
    }
}

#[test]
fn changed_actions_and_new_authored_services_close_the_stale_menu_without_a_service_reply() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};

    for source in [0, 509_9001] {
        for synthesized in [false, true] {
            let mut store = quest_store();
            let mut option = opt(1, "Supplies.", gossip_option::VENDOR);
            option.row_id = 509_9001;
            let options = if synthesized {
                Vec::new()
            } else {
                vec![option.clone()]
            };
            if source == 0 {
                store.npc.gossip_opts = options;
            } else {
                store.npc.gossip_profiles.insert(source, options);
                store
                    .npc
                    .gossip_source_menu_id
                    .store(source, std::sync::atomic::Ordering::SeqCst);
            }
            store.vendor.vendor_stock = vec![codec::VendorItemView {
                item_entry: 4540,
                ..Default::default()
            }];
            let mut session = ProtocolSession::in_world(7, 1);
            let hello = handlers::Query::handle(
                &store,
                &mut session,
                ProtocolRequest::Message(
                    CMSG_GOSSIP_HELLO {
                        guid: Guid::new(90),
                    }
                    .into(),
                ),
            )
            .unwrap();
            let messages = outbound_messages(hello.outbound);
            let [ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(menu)] = messages.as_slice() else {
                panic!("expected the service menu");
            };
            assert_eq!(menu.gossips.len(), 1);
            let current = if source == 0 {
                &mut store.npc.gossip_opts
            } else {
                store.npc.gossip_profiles.get_mut(&source).unwrap()
            };
            if synthesized {
                option.cond_type = gossip_condition::NEVER;
                current.push(option);
            } else {
                current[0].action = gossip_option::GOSSIP;
            }
            let reply = handlers::Query::handle(
                &store,
                &mut session,
                ProtocolRequest::Message(
                    CMSG_GOSSIP_SELECT_OPTION {
                        guid: Guid::new(90),
                        gossip_list_id: 0,
                        unknown: None,
                    }
                    .into(),
                ),
            )
            .unwrap();
            assert!(matches!(
                outbound_messages(reply.outbound).as_slice(),
                [ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE]
            ));
            assert!(store.npc.gossip_selects.lock().unwrap().is_empty());
            assert!(session.gossip_menu.is_none());
        }
    }
}

#[test]
fn rewarded_repeatable_quests_show_only_the_source_eligible_options() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    use lyracore_shared::gossip::Condition;

    let mut store = quest_store();
    store.quest.quest_log = vec![(60, true)].into();
    store.quest.repeatable_quests.insert(60);
    for (id, kind, flags, text) in [
        (1, 8, 0, "Rewarded"),
        (2, 8, 1, "Not rewarded"),
        (3, 22, 0, "No current or rewarded quest"),
        (4, 22, 1, "Current or rewarded quest"),
        (5, 9, 0, "Current quest"),
        (6, 9, 1, "No current quest"),
    ] {
        store.npc.gossip_conditions.insert(
            id,
            Condition {
                kind,
                values: [60, 0, 0, 0],
                flags,
            },
        );
        let mut option = opt(0, text, gossip_option::GOSSIP);
        option.row_id = id;
        option.cond_type = gossip_condition::REFERENCE;
        option.cond_value1 = id;
        store.npc.gossip_opts.push(option);
    }
    let reply = handlers::Query::handle(
        &store,
        &mut ProtocolSession::in_world(7, 1),
        ProtocolRequest::Message(
            CMSG_GOSSIP_HELLO {
                guid: Guid::new(90),
            }
            .into(),
        ),
    )
    .unwrap();
    let messages = outbound_messages(reply.outbound);
    let [ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(menu)] = messages.as_slice() else {
        panic!("expected the quest-conditioned menu");
    };
    assert_eq!(
        menu.gossips
            .iter()
            .map(|option| option.message.as_str())
            .collect::<Vec<_>>(),
        [
            "Not rewarded",
            "No current or rewarded quest",
            "No current quest"
        ]
    );
}

#[test]
fn gossip_selection_transport_loss_ends_the_world_session_without_a_service_reply() {
    use lyracore_shared::constants::gossip_option;
    use std::io::Read;

    let mut fixture = quest_store();
    fixture.npc.gossip_opts = vec![opt(1, "Supplies.", gossip_option::VENDOR)];
    fixture.npc.gossip_select_error =
        Some(|| crate::stdb::ReducerCallError::transport_lost("gw_gossip_select").into());
    let store = std::sync::Arc::new(fixture);
    let (mut client, mut c_enc, mut c_dec, world) = enter_world_unjoined(store.clone(), 1);
    gossip_hello(&mut client, &mut c_enc, &mut c_dec, 90);
    CMSG_GOSSIP_SELECT_OPTION {
        guid: Guid::new(90),
        gossip_list_id: 0,
        unknown: None,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    assert_eq!(client.read(&mut [0u8; 1]).unwrap(), 0);
    assert!(world.join().unwrap().is_err());
    assert!(store.npc.gossip_selects.lock().unwrap().is_empty());
}

#[test]
fn authored_services_that_become_unavailable_close_the_menu_without_a_service_reply() {
    use lyracore_shared::constants::gossip_option;

    for action in [gossip_option::VENDOR, gossip_option::INNKEEPER] {
        let mut store = quest_store();
        store.npc.innkeeper = true;
        store.npc.gossip_opts = vec![opt(0, "Service.", action)];
        store.vendor.vendor_stock = vec![codec::VendorItemView {
            item_entry: 4540,
            ..Default::default()
        }];
        let mut session = ProtocolSession::in_world(7, 1);
        handlers::Query::handle(
            &store,
            &mut session,
            ProtocolRequest::Message(
                CMSG_GOSSIP_HELLO {
                    guid: Guid::new(90),
                }
                .into(),
            ),
        )
        .unwrap();
        store.npc.innkeeper = false;
        store.vendor.vendor_stock.clear();
        let reply = handlers::Query::handle(
            &store,
            &mut session,
            ProtocolRequest::Message(
                CMSG_GOSSIP_SELECT_OPTION {
                    guid: Guid::new(90),
                    gossip_list_id: 0,
                    unknown: None,
                }
                .into(),
            ),
        )
        .unwrap();
        assert!(matches!(
            outbound_messages(reply.outbound).as_slice(),
            [ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE]
        ));
        assert!(store.npc.gossip_selects.lock().unwrap().is_empty());
        assert!(session.gossip_menu.is_none());
    }
}

#[test]
fn missing_quest_subjects_hide_options_even_when_progress_rows_remain() {
    use lyracore_shared::constants::{gossip_condition, gossip_option};
    use lyracore_shared::gossip::Condition;

    let mut store = quest_store();
    store.quest.quest_log = vec![(60, true)].into();
    store.quest.missing_quests.insert(60);
    for (id, kind, flags) in [
        (1, 8, 0),
        (2, 8, 1),
        (3, 9, 0),
        (4, 9, 1),
        (5, 22, 0),
        (6, 22, 1),
    ] {
        store.npc.gossip_conditions.insert(
            id,
            Condition {
                kind,
                values: [60, 0, 0, 0],
                flags,
            },
        );
        let mut option = opt(0, "Missing quest.", gossip_option::GOSSIP);
        option.row_id = id;
        option.cond_type = gossip_condition::REFERENCE;
        option.cond_value1 = id;
        store.npc.gossip_opts.push(option);
    }
    let reply = handlers::Query::handle(
        &store,
        &mut ProtocolSession::in_world(7, 1),
        ProtocolRequest::Message(
            CMSG_GOSSIP_HELLO {
                guid: Guid::new(90),
            }
            .into(),
        ),
    )
    .unwrap();
    let messages = outbound_messages(reply.outbound);
    let [ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(menu)] = messages.as_slice() else {
        panic!("expected the quest-conditioned menu");
    };
    assert!(menu.gossips.is_empty(), "{:?}", menu.gossips);
}
