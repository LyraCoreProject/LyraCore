//! Death and resurrection requests, handled against a Fake that models only who is alive.

use super::*;
use death_fake::{DeathFake, Life};

#[path = "death_fake.rs"]
mod death_fake;

const SELF_GUID: u64 = 1;

/// A World Session connection in the world as `SELF_GUID`. `handle_death` never reads the
/// connection's Store, but `WorldConn` needs one.
fn in_world_conn() -> WorldConn {
    let (_, crypto) = ProofSeed::new().into_client_header_crypto(&ns("TESTER"), K, 0);
    let (_, decrypt) = crypto.split();
    WorldConn {
        session_claim: None,
        account_id: 7,
        account_name: "TESTER".into(),
        decrypt,
        state: WorldState::InWorld(InWorld {
            self_guid: SELF_GUID,
            subs: PlayerSubscriptions::empty(),
            attacking_target: None,
            open_loot: OpenLootState::default(),
            ranged_repeat: false,
        }),
        move_coalesce: Default::default(),
        gossip_menu: None,
        unavailable_notices: Default::default(),
        store: RoutedStore::new(std::sync::Arc::new(WorldFake::default())),
        session_key: None,
        guild_signed_on: None,
        move_desync_drops: 0,
        who_throttled_until: None,
        group_broadcast_cooldowns: Default::default(),
        chat_flood: Default::default(),
    }
}

/// Handle one request and return what the client is sent. Fails when the handler ends the session
/// or passes the request on.
fn run(store: &DeathFake, conn: &mut WorldConn, msg: ClientOpcodeMessage) -> Vec<Outbound> {
    let (tx, rx) = SessionTx::with_depth(0);
    let passed_on = handle_death(&tx, store, conn, msg).expect("the request ends the session");
    assert!(passed_on.is_none(), "the request was passed on");
    drop(tx);
    rx.try_iter().collect()
}

#[test]
fn repop_revives_the_caller_and_no_one_else() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_life(2, Life::Dead);

    // The revive reaches the client through the entity relay, not as a direct reply.
    let sent = run(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_REPOP_REQUEST,
    );

    assert!(sent.is_empty());
    assert_eq!(store.life(SELF_GUID), Life::Alive);
    assert_eq!(store.life(2), Life::Dead);
}

#[test]
fn reclaim_corpse_uses_the_corpse_guid_from_the_wire() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Ghost)
        .with_corpse(SELF_GUID, 777);
    let mut conn = in_world_conn();
    let reclaim = |guid| {
        ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(CMSG_RECLAIM_CORPSE {
            guid: Guid::new(guid),
        })
    };

    run(&store, &mut conn, reclaim(778));
    assert_eq!(store.life(SELF_GUID), Life::Ghost, "another corpse");

    run(&store, &mut conn, reclaim(777));
    assert_eq!(store.life(SELF_GUID), Life::Alive);
}

#[test]
fn accepting_a_resurrect_offer_revives_the_caller() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_res_offer(SELF_GUID);

    run(&store, &mut in_world_conn(), resurrect_response(1));

    assert_eq!(store.life(SELF_GUID), Life::Alive);
}

#[test]
fn declining_a_resurrect_offer_leaves_the_caller_dead_and_spends_the_offer() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_res_offer(SELF_GUID);

    run(&store, &mut in_world_conn(), resurrect_response(0));

    assert_eq!(store.life(SELF_GUID), Life::Dead);
    assert!(!store.has_res_offer(SELF_GUID));
}

#[test]
fn self_res_revives_the_caller_and_spends_the_option() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_self_res_option(SELF_GUID);

    // The revive reaches the client through the entity relay, not as a direct reply.
    let sent = run(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_SELF_RES,
    );

    assert!(sent.is_empty());
    assert_eq!(store.life(SELF_GUID), Life::Alive);
    assert!(!store.has_self_res_option(SELF_GUID));
}

#[test]
fn a_refused_self_res_sends_nothing_and_keeps_the_session() {
    let store = DeathFake::default().with_life(SELF_GUID, Life::Dead);

    // `run` fails the test when the handler ends the session.
    let sent = run(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_SELF_RES,
    );

    assert!(sent.is_empty());
    assert_eq!(store.life(SELF_GUID), Life::Dead);
}

#[test]
fn spirit_healer_revives_the_ghost_and_confirms_the_healer_guid() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Ghost)
        .with_spirit_healer(888);

    let sent = run(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(CMSG_SPIRIT_HEALER_ACTIVATE {
            guid: Guid::new(888),
        }),
    );

    let [Outbound::One(ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(confirm))] = sent.as_slice()
    else {
        panic!("expected one SMSG_SPIRIT_HEALER_CONFIRM");
    };
    assert_eq!(confirm.guid, Guid::new(888), "echoes the healer's own guid");
    assert_eq!(store.life(SELF_GUID), Life::Alive);
}

fn resurrect_response(status: u8) -> ClientOpcodeMessage {
    ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(Box::new(CMSG_RESURRECT_RESPONSE {
        guid: Guid::new(42),
        status,
    }))
}

#[test]
fn transport_loss_ends_the_world_session() {
    let store = DeathFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_transport_loss();
    let (tx, _rx) = SessionTx::with_depth(0);

    let result = handle_death(
        &tx,
        &store,
        &in_world_conn(),
        ClientOpcodeMessage::CMSG_REPOP_REQUEST,
    );

    assert!(result.is_err());
}
