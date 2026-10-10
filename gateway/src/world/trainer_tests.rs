//! Trainer and talent opcodes over an encrypted World Session.

use super::*;

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
