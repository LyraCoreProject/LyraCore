//! Member Stats against the realm-wide party topology: the Relay tick for one viewer, and the
//! `CMSG_REQUEST_PARTY_MEMBER_STATS` answer. Party membership comes from the same Fake Realm-core
//! the routing tests use, so a mate on `instances` is read from that shard's cache.
//!
//! Expected bodies are written out by hand from cm:GroupHandler.cpp:585-630: packed guid, `u32`
//! mask, then the masked fields. Every guid here is one byte, so the packed guid is `01 <guid>`.

use super::party_tests::{form_split_party, party_topology, BOT, GINGER, TRIN, VIM};
use super::*;
use crate::world::handlers::{
    dispatch_member_stats, member_stats_tick, MemberSnapshot, MemberStatsOutcome, MemberStatsPlayer,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

type Snapshots = HashMap<u64, MemberSnapshot>;

/// A level 20 caster standing in Elwynn Forest.
fn caster() -> codec::MemberEntity {
    codec::MemberEntity {
        health: 1234,
        max_health: 1500,
        power: 800,
        max_power: 1000,
        unit_bytes_0: 0,
        level: 20,
        zone_id: 12,
        x: -8949.95,
        y: -132.49,
        dead: false,
        player_flags: 0,
    }
}

/// The caster's full body for guid `g`: every field, mask 0x1FF.
fn caster_full_body(guid: u8) -> Vec<u8> {
    vec![
        0x01, guid, // packed guid
        0xFF, 0x01, 0x00, 0x00, // mask 0x1FF
        0x01, // status ONLINE
        0xD2, 0x04, // current health 1234
        0xDC, 0x05, // max health 1500
        0x00, // power type mana
        0x20, 0x03, // current power 800
        0xE8, 0x03, // max power 1000
        0x14, 0x00, // level 20
        0x0C, 0x00, // zone 12
        0x0B, 0xDD, // x -8949
        0x7C, 0xFF, // y -132
    ]
}

fn place(shard: &InMemoryStore, guid: u64, entity: codec::MemberEntity) {
    let mut entities = shard.member_entities.lock().unwrap();
    entities.retain(|(g, _)| *g != guid);
    entities.push((guid, entity));
}

fn despawn(shard: &InMemoryStore, guid: u64) {
    shard
        .member_entities
        .lock()
        .unwrap()
        .retain(|(g, _)| *g != guid);
}

/// Ginger and Trin, both on `world`.
fn ginger_and_trin() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
) {
    let (realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("invite Trin");
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).expect("Trin accepts");
    place(&world, GINGER, caster());
    place(&world, TRIN, caster());
    (realm, world, instances)
}

/// One tick for Ginger, whose client has created only the guids in `created`.
fn tick(store: &InMemoryStore, created: &[u64], snapshots: &mut Snapshots) -> Vec<(u16, Vec<u8>)> {
    member_stats_tick(store, GINGER, |guid| created.contains(&guid), snapshots)
        .expect("the tick reads")
        .into_iter()
        .map(|packet| match packet {
            Outbound::Raw { opcode, body } => (opcode, body),
            _ => panic!("Member Stats are raw packets"),
        })
        .collect()
}

#[test]
fn a_live_mate_on_the_same_shard_gets_every_field_on_the_first_tick() {
    let (_realm, world, _instances) = ginger_and_trin();
    let packets = tick(&world, &[GINGER], &mut Snapshots::new());
    assert_eq!(packets, vec![(0x007E, caster_full_body(TRIN as u8))]);
}

#[test]
fn a_live_mate_on_another_shard_is_read_from_that_shards_cache() {
    let (_realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    let deadmines = codec::MemberEntity {
        zone_id: 1581,
        x: -16.4,
        y: -383.07,
        dead: true,
        ..caster()
    };
    place(&instances, VIM, deadmines);

    let packets = tick(&world, &[GINGER], &mut Snapshots::new());

    #[rustfmt::skip]
    let expected = vec![
        0x01, 0x02,             // packed guid 2
        0xFF, 0x01, 0x00, 0x00, // mask 0x1FF
        0x05,                   // status ONLINE | DEAD
        0xD2, 0x04, 0xDC, 0x05, 0x00, 0x20, 0x03, 0xE8, 0x03, 0x14, 0x00,
        0x2D, 0x06,             // zone 1581
        0xF0, 0xFF,             // x -16
        0x81, 0xFE,             // y -383
    ];
    assert_eq!(packets, vec![(0x007E, expected)]);
}

#[test]
fn a_mate_created_on_the_viewers_client_gets_nothing() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    assert!(tick(&world, &[GINGER, TRIN], &mut snapshots).is_empty());
    assert!(snapshots.is_empty());
}

#[test]
fn after_only_health_changes_the_next_tick_sends_current_health_alone() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots);
    place(
        &world,
        TRIN,
        codec::MemberEntity {
            health: 1000,
            ..caster()
        },
    );

    let packets = tick(&world, &[GINGER], &mut snapshots);

    let body = vec![0x01, 0x03, 0x02, 0x00, 0x00, 0x00, 0xE8, 0x03];
    assert_eq!(packets, vec![(0x007E, body)]);
    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());
}

#[test]
fn a_power_type_change_sends_the_type_and_both_power_values() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots);
    // Bear Form: the same numbers, now rage.
    place(
        &world,
        TRIN,
        codec::MemberEntity {
            unit_bytes_0: 0x0100_0000,
            ..caster()
        },
    );

    let packets = tick(&world, &[GINGER], &mut snapshots);

    let body = vec![
        0x01, 0x03, // packed guid 3
        0x38, 0x00, 0x00, 0x00, // POWER_TYPE | CUR_POWER | MAX_POWER
        0x01, // rage
        0x20, 0x03, // current power 800
        0xE8, 0x03, // max power 1000
    ];
    assert_eq!(packets, vec![(0x007E, body)]);
}

#[test]
fn a_mate_that_leaves_the_viewers_aoi_gets_every_field_again() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots);
    assert!(tick(&world, &[GINGER, TRIN], &mut snapshots).is_empty());

    let packets = tick(&world, &[GINGER], &mut snapshots);

    assert_eq!(packets, vec![(0x007E, caster_full_body(TRIN as u8))]);
}

#[test]
fn an_offline_mate_gets_one_status_zero_packet_then_nothing_until_it_changes() {
    let (_realm, world, _instances) = ginger_and_trin();
    despawn(&world, TRIN);
    let mut snapshots = Snapshots::new();

    let packets = tick(&world, &[GINGER], &mut snapshots);
    let offline = vec![0x01, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(packets, vec![(0x007E, offline)]);
    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());

    place(&world, TRIN, caster());
    let packets = tick(&world, &[GINGER], &mut snapshots);
    assert_eq!(packets, vec![(0x007E, caster_full_body(TRIN as u8))]);
}

#[test]
fn a_mate_in_transfer_gets_nothing_and_is_never_reported_offline() {
    let (realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    place(&instances, VIM, caster());
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots);

    despawn(&instances, VIM);
    realm.members_in_transit.lock().unwrap().push(VIM);
    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());

    // Arrival with nothing changed: the record survived the Transfer, so nothing is resent.
    realm.members_in_transit.lock().unwrap().clear();
    place(&world, VIM, caster());
    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());
}

#[test]
fn a_viewer_with_no_group_gets_nothing_and_reads_no_member() {
    let (_realm, world, instances, _) = party_topology();
    place(&world, TRIN, caster());
    let mut snapshots = Snapshots::from([(TRIN, MemberSnapshot::Offline)]);

    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());

    assert!(
        snapshots.is_empty(),
        "a viewer with no group keeps no records"
    );
    for shard in [&world, &instances] {
        assert_eq!(shard.member_presence_reads.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn a_mate_who_leaves_the_group_is_forgotten() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots);
    party::run(world.as_ref(), 9, TRIN, party::Op::Leave).expect("Trin leaves");

    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());
    assert!(!snapshots.contains_key(&TRIN));
}

fn request(store: &InMemoryStore, self_guid: Option<u64>, guid: u64) -> MemberStatsOutcome {
    dispatch_member_stats(
        store,
        MemberStatsPlayer { self_guid },
        ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(
            wow_world_messages::vanilla::CMSG_REQUEST_PARTY_MEMBER_STATS {
                guid: Guid::new(guid),
            },
        ),
    )
    .expect("the request reads")
}

fn answer(outcome: MemberStatsOutcome) -> Vec<(u16, Vec<u8>)> {
    let MemberStatsOutcome::Handled { outbound } = outcome else {
        panic!("an in-world stats request is handled");
    };
    outbound
        .into_iter()
        .map(|packet| match packet {
            Outbound::Raw { opcode, body } => (opcode, body),
            _ => panic!("Member Stats are raw packets"),
        })
        .collect()
}

#[test]
fn a_request_for_a_group_mate_returns_one_full_packet() {
    let (_realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    place(&instances, VIM, caster());

    let packets = answer(request(&world, Some(GINGER), VIM));

    assert_eq!(packets, vec![(0x02F2, caster_full_body(VIM as u8))]);
}

#[test]
fn a_request_for_anyone_else_returns_the_offline_full_packet() {
    let (realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    // The bot is live on `world` but in nobody's group: its position must not leak.
    place(&world, BOT, caster());
    let offline = |guid: u8| vec![(0x02F2, vec![0x01, guid, 0x01, 0x00, 0x00, 0x00, 0x00])];

    assert_eq!(
        answer(request(&world, Some(GINGER), BOT)),
        offline(BOT as u8)
    );
    // A mate with no entity anywhere.
    assert_eq!(
        answer(request(&world, Some(GINGER), VIM)),
        offline(VIM as u8)
    );
    // A mate in Transfer.
    realm.members_in_transit.lock().unwrap().push(VIM);
    assert_eq!(
        answer(request(&world, Some(GINGER), VIM)),
        offline(VIM as u8)
    );
}

#[test]
fn a_stats_request_outside_the_world_passes_through() {
    let (_realm, world, _instances) = ginger_and_trin();
    assert!(matches!(
        request(&world, None, TRIN),
        MemberStatsOutcome::PassThrough(ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(_))
    ));
}

/// The raw FULL body survives the session writer and the header cipher, on a single database
/// where the party lives in the shard's own tables.
#[test]
fn a_stats_request_is_answered_through_the_encrypted_session() {
    let store = InMemoryStore {
        mirror: std::sync::Mutex::new(vec![party::GroupRoster {
            group_id: 1,
            leader_guid: 1,
            members: vec![1, 2],
            ..Default::default()
        }]),
        ..quest_store()
    };
    place(&store, 2, caster());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(std::sync::Arc::new(store), 1);

    wow_world_messages::vanilla::CMSG_REQUEST_PARTY_MEMBER_STATS { guid: Guid::new(2) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();

    assert_eq!(
        read_raw_frame(&mut client, &mut c_dec),
        (0x02F2, caster_full_body(2))
    );
    drop(client);
    let _ = server.join();
}
