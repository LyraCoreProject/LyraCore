//! NPC and query opcodes over an encrypted World Session: item and NPC text queries, inspect, and
//! gossip menus.

use super::*;

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

/// `enter_world` for a test that needs to see how the session ended: the server thread hands back
/// its verdict instead of unwrapping it.
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

/// A Transport Loss leaves the inspect gate's outcome unknown, so it ends the World Session. A
/// Refusal (above) only drops the request.
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

/// Say, yell and `/e` lines go to the speaker's Home Shard. A lost transport leaves the line's
/// fate unknown, so it ends the World Session.
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

/// The Gateway finds a mistyped GM Account itself. The GM reads the reason as a system line and
/// the World Session goes on.
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
    s.npc.innkeeper = true;
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
    assert!(matches!(
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap(),
        ServerOpcodeMessage::SMSG_BINDER_CONFIRM(_)
    ));
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

/// A `quest_store()` fixture whose logged-in character (guid 1) reports `level`, so
/// `filtered_gossip_options`' level gate has something to read (`quest_store()` itself leaves
/// `characters` empty, which reads as level 0 — every below-10 test can lean on that default).
fn quest_store_at_level(level: u8) -> WorldFake {
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
    // Selecting the option quotes the cost without changing talents.
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
    s.npc.innkeeper = true;
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
            assert_eq!(u.texts[1].probability, 0.0); // untouched slot stays silent
        }
        other => panic!("expected SMSG_NPC_TEXT_UPDATE, got {other}"),
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
