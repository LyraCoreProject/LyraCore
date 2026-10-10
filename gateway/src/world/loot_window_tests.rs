//! GameObject use: its routing by GameObject type over a World Session, and the general-use path.

use super::death_tests::handle_loot_fake::HandleLootFake;
use super::death_tests::{in_world_conn, run, SELF_GUID};
use super::*;

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
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store, 1);

    CMSG_GAMEOBJ_USE {
        guid: Guid::new(68),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_QUESTGIVER_STATUS_QUERY {
        guid: Guid::new(50),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(details) => {
            assert_eq!(details.quest_id, 1234)
        }
        other => panic!("expected quest details from a questgiver GameObject, got {other}"),
    }
    // The loot window would have answered before the status query's reply.
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_QUESTGIVER_STATUS(_) => {}
        other => panic!("a questgiver GameObject opened a loot window: {other}"),
    }
    drop(client);
    server.join().unwrap();
}

#[test]
fn gameobject_use_outside_the_chest_path_opens_the_window_for_its_loot() {
    let store = HandleLootFake::default().with_loot(SELF_GUID, vec![(4, 117, 2, 321, 0)]);
    let mut conn = in_world_conn();

    let sent = run(
        &store,
        &mut conn,
        ClientOpcodeMessage::CMSG_GAMEOBJ_USE(CMSG_GAMEOBJ_USE {
            guid: Guid::new(91),
        }),
    );

    let [Outbound::Raw { opcode, body }] = sent.as_slice() else {
        panic!("expected one loot window");
    };
    assert_eq!(*opcode, OP_LOOT_RESPONSE);
    assert_eq!(&body[0..8], &91u64.to_le_bytes());
    let WorldState::InWorld(iw) = &conn.state else {
        panic!("the connection left the world");
    };
    assert_eq!(iw.open_loot.target_guid, Some(91));
}
