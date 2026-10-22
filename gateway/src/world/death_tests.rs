use super::family::{ProtocolFamily, ProtocolSession};
use super::*;
use handle_loot_fake::{HandleLootFake, Life};

#[path = "handle_loot_fake.rs"]
pub(crate) mod handle_loot_fake;

pub(crate) const SELF_GUID: u64 = 1;

pub(crate) fn in_world_conn() -> ProtocolSession {
    ProtocolSession::in_world(7, SELF_GUID)
}

pub(crate) fn run(
    store: &HandleLootFake,
    session: &mut ProtocolSession,
    msg: ClientOpcodeMessage,
) -> Vec<Outbound> {
    handlers::Loot::handle(store, session, msg.into())
        .expect("the request ends the World Session")
        .outbound
}

#[test]
fn repop_revives_the_caller_and_no_one_else() {
    let store = HandleLootFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_life(2, Life::Dead);

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
    let store = HandleLootFake::default()
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
    let store = HandleLootFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_res_offer(SELF_GUID);

    run(&store, &mut in_world_conn(), resurrect_response(1));

    assert_eq!(store.life(SELF_GUID), Life::Alive);
}

#[test]
fn declining_a_resurrect_offer_leaves_the_caller_dead_and_spends_the_offer() {
    let store = HandleLootFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_res_offer(SELF_GUID);

    run(&store, &mut in_world_conn(), resurrect_response(0));

    assert_eq!(store.life(SELF_GUID), Life::Dead);
    assert!(!store.has_res_offer(SELF_GUID));
}

#[test]
fn self_res_revives_the_caller_and_spends_the_option() {
    let store = HandleLootFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_self_res_option(SELF_GUID);

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
    let store = HandleLootFake::default().with_life(SELF_GUID, Life::Dead);

    let sent = run(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_SELF_RES,
    );

    assert!(sent.is_empty());
    assert_eq!(store.life(SELF_GUID), Life::Dead);
}

#[test]
fn spirit_healer_activation_revives_the_ghost_without_a_second_confirmation() {
    let store = HandleLootFake::default()
        .with_life(SELF_GUID, Life::Ghost)
        .with_spirit_healer(888);

    let sent = run(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(CMSG_SPIRIT_HEALER_ACTIVATE {
            guid: Guid::new(888),
        }),
    );

    assert!(sent.is_empty());
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
    let store = HandleLootFake::default()
        .with_life(SELF_GUID, Life::Dead)
        .with_transport_loss();

    let result = handlers::Loot::handle(
        &store,
        &mut in_world_conn(),
        ClientOpcodeMessage::CMSG_REPOP_REQUEST.into(),
    );

    assert!(result.is_err());
}
