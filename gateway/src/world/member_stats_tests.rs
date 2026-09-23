//! Member Stats against the realm-wide party topology: the Relay tick for one viewer, and the
//! `CMSG_REQUEST_PARTY_MEMBER_STATS` answer. Party membership comes from the same Fake Realm-core
//! the routing tests use, so a mate on `instances` is read from that shard's cache.
//!
//! Expected bodies are written out by hand from cm:GroupHandler.cpp:585-630: packed guid, `u32`
//! mask, then the masked fields. Every guid here is one byte, so the packed guid is `01 <guid>`.

use super::party_tests::{
    character, form_split_party, party_members, party_topology, BOT, GINGER, TRIN, VIM,
};
use super::*;
use crate::world::handlers::{
    dispatch_member_stats, member_stats_tick, MemberSnapshot, MemberStatsOutcome,
    MemberStatsPlayer, MemberStatsRecord,
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
        ..codec::MemberEntity::default()
    }
}

/// The caster with two positive auras (slots 0 and 5) and one negative aura (slot 35).
fn caster_with_auras() -> codec::MemberEntity {
    codec::MemberEntity {
        auras: vec![
            codec::MemberAuraSlot {
                slot: 0,
                spell_id: 100,
            },
            codec::MemberAuraSlot {
                slot: 5,
                spell_id: 200,
            },
            codec::MemberAuraSlot {
                slot: 35,
                spell_id: 300,
            },
        ],
        ..caster()
    }
}

/// A Hunter's live pet: Fluffy, with one positive aura.
fn hunter_pet() -> codec::MemberPetEntity {
    codec::MemberPetEntity {
        guid: 85,
        name: "Fluffy".into(),
        display_id: 618,
        health: 50,
        max_health: 60,
        power: 40,
        max_power: 100,
        unit_bytes_0: 0x0200_0000, // power type 2, focus
        auras: vec![codec::MemberAuraSlot {
            slot: 2,
            spell_id: 400,
        }],
    }
}

/// The caster's aura block for [`caster_with_auras`]: `AURAS` (bits 0, 5) then
/// `AURAS_NEGATIVE` (bit 3, slot 35).
fn caster_aura_bytes() -> Vec<u8> {
    #[rustfmt::skip]
    let bytes = vec![
        0x21, 0x00, 0x00, 0x00, // AURAS mask: slots 0 and 5
        0x64, 0x00,             // spell 100
        0xC8, 0x00,             // spell 200
        0x08, 0x00,             // AURAS_NEGATIVE mask: slot 35 (bit 3)
        0x2C, 0x01,             // spell 300
    ];
    bytes
}

/// The caster's 9 base fields, header and aura blocks excluded.
fn caster_base_bytes() -> Vec<u8> {
    vec![
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

/// [`hunter_pet`]'s scalar pet fields: guid, name, display, health and power. Its aura blocks are
/// a separate concern; a caller appends them (occupied or empty) after this.
fn hunter_pet_bytes() -> Vec<u8> {
    #[rustfmt::skip]
    let bytes = vec![
        0x55, 0, 0, 0, 0, 0, 0, 0,       // PET_GUID 85
        b'F', b'l', b'u', b'f', b'f', b'y', 0x00, // PET_NAME "Fluffy"
        0x6A, 0x02,             // PET_MODEL_ID 618
        0x32, 0x00,             // PET_CUR_HP 50
        0x3C, 0x00,             // PET_MAX_HP 60
        0x02,                   // PET_POWER_TYPE 2 (focus)
        0x28, 0x00,             // PET_CUR_POWER 40
        0x64, 0x00,             // PET_MAX_POWER 100
    ];
    bytes
}

/// The wire bytes for an empty `AURAS`/`PET_AURAS` block where every slot rides the packet (a
/// `None` previous, cm:Group.cpp:306-307, 746-747): mask `0xFFFFFFFF`, then 32 zero values.
fn empty_positive_aura_bytes() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFF, 0xFF, 0xFF];
    bytes.extend(std::iter::repeat_n(0u8, 32 * 2));
    bytes
}

/// [`empty_positive_aura_bytes`] for the negative block: mask `0xFFFF`, then 16 zero values.
fn empty_negative_aura_bytes() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFF];
    bytes.extend(std::iter::repeat_n(0u8, 16 * 2));
    bytes
}

/// [`empty_positive_aura_bytes`] with `slot` overlaid to `spell_id`, for a first send where one
/// slot is occupied: mask `0xFFFFFFFF`, then a spell id per slot, `slot`'s the only nonzero one.
fn full_positive_aura_bytes(slot: usize, spell_id: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFF, 0xFF, 0xFF];
    for i in 0..32u16 {
        let value = if i as usize == slot { spell_id } else { 0 };
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// A zeroed pet block, PET_GUID through PET_AURAS_NEGATIVE: what a first send writes for a
/// pet-less member, and what a delta writes once for a member whose pet just vanished
/// (cm:GroupHandler.cpp:660-755's own "write zero when there is no charm" branches).
fn zeroed_pet_bytes() -> Vec<u8> {
    let mut bytes = vec![0u8; 8]; // PET_GUID
    bytes.push(0); // PET_NAME: an empty CString
    bytes.extend([0u8; 2]); // PET_MODEL_ID
    bytes.extend([0u8; 2]); // PET_CUR_HP
    bytes.extend([0u8; 2]); // PET_MAX_HP
    bytes.push(0); // PET_POWER_TYPE
    bytes.extend([0u8; 2]); // PET_CUR_POWER
    bytes.extend([0u8; 2]); // PET_MAX_POWER
    bytes.extend(empty_positive_aura_bytes()); // PET_AURAS
    bytes.extend(empty_negative_aura_bytes()); // PET_AURAS_NEGATIVE
    bytes
}

/// The pet-less caster's body for guid `g` answering `CMSG_REQUEST_PARTY_MEMBER_STATS`: every
/// base field, plus the two empty aura blocks. Mask 0x7FF, no pet bits: only the explicit FULL
/// answer strips `GROUP_UPDATE_PET` whole for a pet-less member (cm:GroupHandler.cpp:781-786).
fn caster_full_body(guid: u8) -> Vec<u8> {
    let mut body = vec![0x01, guid, 0xFF, 0x07, 0x00, 0x00]; // mask 0x7FF
    body.extend(caster_base_bytes());
    body.extend(empty_positive_aura_bytes());
    body.extend(empty_negative_aura_bytes());
    body
}

/// The pet-less caster's body for guid `g` on a Relay first send (opcode `0x007E`): every base
/// field, both aura blocks written in full, and every pet field zeroed. `None` marks
/// `GROUP_UPDATE_FULL` dirty unconditionally, pet fields included even with no pet
/// (cm:Group.cpp:306-307, 746-747), so a relogged member without their old pet still clears the
/// viewer's stale pet frame.
fn caster_first_send_body(guid: u8) -> Vec<u8> {
    let mut body = vec![0x01, guid, 0xFF, 0xFF, 0x1F, 0x00]; // mask 0x1FFFFF (GROUP_UPDATE_FULL)
    body.extend(caster_base_bytes());
    body.extend(empty_positive_aura_bytes());
    body.extend(empty_negative_aura_bytes());
    body.extend(zeroed_pet_bytes());
    body
}

/// The caster's wire values.
fn stats() -> codec::MemberStats {
    codec::MemberStats::from_entity(&caster())
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
    assert_eq!(packets, vec![(0x007E, caster_first_send_body(TRIN as u8))]);
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

    let mut expected = vec![0x01, 0x02, 0xFF, 0xFF, 0x1F, 0x00]; // mask 0x1FFFFF (GROUP_UPDATE_FULL)
    #[rustfmt::skip]
    expected.extend([
        0x05,                   // status ONLINE | DEAD
        0xD2, 0x04, 0xDC, 0x05, 0x00, 0x20, 0x03, 0xE8, 0x03, 0x14, 0x00,
        0x2D, 0x06,             // zone 1581
        0xF0, 0xFF,             // x -16
        0x81, 0xFE,             // y -383
    ]);
    expected.extend(empty_positive_aura_bytes());
    expected.extend(empty_negative_aura_bytes());
    expected.extend(zeroed_pet_bytes());
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

    assert_eq!(packets, vec![(0x007E, caster_first_send_body(TRIN as u8))]);
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
    assert_eq!(packets, vec![(0x007E, caster_first_send_body(TRIN as u8))]);
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

fn request_with(
    store: &InMemoryStore,
    self_guid: Option<u64>,
    record: Option<&MemberStatsRecord>,
    guid: u64,
) -> MemberStatsOutcome {
    dispatch_member_stats(
        store,
        MemberStatsPlayer { self_guid, record },
        ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(
            wow_world_messages::vanilla::CMSG_REQUEST_PARTY_MEMBER_STATS {
                guid: Guid::new(guid),
            },
        ),
    )
}

fn request(store: &InMemoryStore, self_guid: Option<u64>, guid: u64) -> MemberStatsOutcome {
    request_with(store, self_guid, None, guid)
}

/// Run the answer the way the session writer does: every job, in order.
fn answer(outcome: MemberStatsOutcome) -> Vec<(u16, Vec<u8>)> {
    let MemberStatsOutcome::Handled { outbound } = outcome else {
        panic!("an in-world stats request is handled");
    };
    outbound
        .into_iter()
        .flat_map(|packet| match packet {
            Outbound::Job(job) => job(),
            packet => vec![packet],
        })
        .map(|packet| match packet {
            Outbound::Raw { opcode, body } => (opcode, body),
            _ => panic!("Member Stats are raw packets"),
        })
        .collect()
}

fn full_offline(guid: u8) -> Vec<(u16, Vec<u8>)> {
    vec![(0x02F2, vec![0x01, guid, 0x01, 0x00, 0x00, 0x00, 0x00])]
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
    let (_realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    // The bot is live on `world` but in nobody's group: its position must not leak.
    place(&world, BOT, caster());

    assert_eq!(
        answer(request(&world, Some(GINGER), BOT)),
        full_offline(BOT as u8)
    );
    // A mate with no entity anywhere.
    assert_eq!(
        answer(request(&world, Some(GINGER), VIM)),
        full_offline(VIM as u8)
    );
}

/// cmangos answers a teleporting mate from the player object, flagged ZONE_OUT
/// (cm:GroupHandler.cpp:764, cm:Group.cpp:54-55). The entity is gone here, so the status goes
/// alone: online, zone out.
#[test]
fn a_request_for_a_mate_between_two_places_answers_online_and_zone_out() {
    let (realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    let online_zone_out = vec![(0x02F2, vec![0x01, 0x02, 0x01, 0x00, 0x00, 0x00, 0x21])];

    realm.members_in_transit.lock().unwrap().push(VIM);
    assert_eq!(answer(request(&world, Some(GINGER), VIM)), online_zone_out);

    realm.members_in_transit.lock().unwrap().clear();
    instances.members_between_places.lock().unwrap().push(VIM);
    assert_eq!(answer(request(&world, Some(GINGER), VIM)), online_zone_out);
}

/// A FULL answer rewrites the frame behind the Relay's back. The mate crossed while the record
/// said ONLINE, got the in-transit answer, then arrived with nothing else changed: the next tick
/// must still send every field, the online status among them.
#[test]
fn after_a_full_answer_the_next_tick_sends_every_field() {
    let (realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    place(&instances, VIM, caster());
    let record = MemberStatsRecord::default();
    tick(&world, &[GINGER], &mut record.lock());

    despawn(&instances, VIM);
    realm.members_in_transit.lock().unwrap().push(VIM);
    answer(request_with(&world, Some(GINGER), Some(&record), VIM));
    realm.members_in_transit.lock().unwrap().clear();
    place(&world, VIM, caster());

    let packets = tick(&world, &[GINGER], &mut record.lock());
    assert_eq!(packets, vec![(0x007E, caster_first_send_body(VIM as u8))]);
}

#[test]
fn a_failed_read_answers_nothing_and_is_never_offline() {
    let (realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    // Vim has no entity, and one World Shard cannot prove the absence.
    let unhealthy = InMemoryStore {
        shard: "world".into(),
        realm: Some(realm),
        world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
        ..Default::default()
    };
    *unhealthy.peers.lock().unwrap() = vec![world.clone(), instances.clone()];

    assert!(answer(request(&unhealthy, Some(GINGER), VIM)).is_empty());
    let mut snapshots = Snapshots::new();
    assert!(
        member_stats_tick(&unhealthy, GINGER, |_| false, &mut snapshots).is_err(),
        "an unproven absence is not Offline"
    );
}

#[test]
fn a_bot_crossing_between_shards_is_never_reported_offline() {
    let (_realm, world, _instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).expect("invite the bot");
    place(&world, BOT, caster());
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots);

    // The Package placed the bot and wrote its Transfer Intent; the Gateway has not claimed it.
    despawn(&world, BOT);
    world.members_between_places.lock().unwrap().push(BOT);

    assert!(tick(&world, &[GINGER], &mut snapshots).is_empty());
    assert_eq!(
        snapshots.get(&BOT).cloned(),
        Some(MemberSnapshot::Live(Box::new(stats())))
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
            members: party_members(&[1, 2]),
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

/// A read failure used to end the World Session. Now the first request goes unanswered and the
/// second, for a stranger, still gets its reply on the same socket.
#[test]
fn a_stats_request_that_cannot_be_read_does_not_end_the_session() {
    let store = InMemoryStore {
        mirror: std::sync::Mutex::new(vec![party::GroupRoster {
            group_id: 1,
            leader_guid: 1,
            members: party_members(&[1, 2]),
            ..Default::default()
        }]),
        world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
        ..quest_store()
    };
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(std::sync::Arc::new(store), 1);

    for guid in [2, 99] {
        wow_world_messages::vanilla::CMSG_REQUEST_PARTY_MEMBER_STATS {
            guid: Guid::new(guid),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    }

    assert_eq!(read_raw_frame(&mut client, &mut c_dec), full_offline(99)[0]);
    drop(client);
    let _ = server.join();
}

/// A Relay tick can reach the writer between viewer registration and the world-entry party frame.
/// The client does not know the party yet, so world entry forgets that tick behind the frame.
#[test]
fn world_entry_forgets_member_stats_sent_before_the_party_frame() {
    let view = std::sync::Arc::new(crate::stdb::world_view::WorldView::new(true));
    let (realm, _world, _instances, calls) = party_topology();
    let session_shard = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls,
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger"), character(VIM, "Vim")],
        live_guids: vec![GINGER, VIM],
        relay_view: Some(view.clone()),
        member_stats_before_party_frame: Some(VIM),
        ..Default::default()
    });
    *session_shard.peers.lock().unwrap() = vec![session_shard.clone()];
    place(&session_shard, VIM, caster());
    {
        let mut party = realm.party.lock().unwrap();
        party.next_group_id = 5;
        party.groups.push((5, GINGER, 3, 2, 0));
        party.members.push((5, GINGER));
        party.members.push((5, VIM));
    }
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = session_shard.clone();
    let server = std::thread::spawn(move || {
        let _ = run_world_session(server_end, server_store.as_ref());
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(GINGER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The answer for a stranger is the barrier: the writer runs its queue in order.
    wow_world_messages::vanilla::CMSG_REQUEST_PARTY_MEMBER_STATS {
        guid: Guid::new(99),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let mut saw_party_frame = false;
    loop {
        let (opcode, _) = read_raw_frame(&mut client, &mut c_dec);
        saw_party_frame |= opcode == 0x007D; // SMSG_GROUP_LIST
        if opcode == 0x02F2 {
            break;
        }
    }
    assert!(saw_party_frame, "world entry renders the party frame");

    let record = view
        .viewer_of_owner(crate::stdb::world_view::OwnerGuid(GINGER))
        .expect("world entry registers the viewer")
        .member_stats
        .clone();
    let packets = tick(&session_shard, &[GINGER], &mut record.lock());
    assert_eq!(packets, vec![(0x007E, caster_first_send_body(VIM as u8))]);
    drop(client);
    let _ = server.join();
}

/// AC1: a member with two buffs and one debuff sends `AURAS` with two ids and `AURAS_NEGATIVE`
/// with one id, at the right bits. Driven as a delta so the body carries only the aura fields.
#[test]
fn a_member_with_two_buffs_and_one_debuff_sends_both_aura_blocks() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots); // seed: the plain, aura-less caster

    place(&world, TRIN, caster_with_auras());
    let packets = tick(&world, &[GINGER], &mut snapshots);

    let mut expected = vec![0x01, TRIN as u8, 0x00, 0x06, 0x00, 0x00]; // mask 0x600
    expected.extend(caster_aura_bytes());
    assert_eq!(packets, vec![(0x007E, expected)]);
}

/// AC2: removing one buff, and only that one, sends only its bit, with id 0. The other buff and
/// the debuff stay in place and stay silent.
#[test]
fn removing_one_buff_sends_only_its_bit_with_id_zero() {
    let (_realm, world, _instances) = ginger_and_trin();
    place(&world, TRIN, caster_with_auras());
    let mut snapshots = Snapshots::new();
    tick(&world, &[GINGER], &mut snapshots); // seed with both buffs and the debuff

    let mut one_buff_left = caster_with_auras();
    one_buff_left.auras.retain(|aura| aura.slot != 0);
    place(&world, TRIN, one_buff_left);

    let packets = tick(&world, &[GINGER], &mut snapshots);
    let expected = vec![
        0x01, TRIN as u8, // packed guid
        0x00, 0x02, 0x00, 0x00, // mask 0x200 (AURAS only)
        0x01, 0x00, 0x00, 0x00, // AURAS mask: bit 0, the removed buff, alone
        0x00, 0x00, // spell id 0
    ];
    assert_eq!(packets, vec![(0x007E, expected)]);
}

/// AC3: a hunter with a live pet sends the pet block with guid, name, display, health, power and
/// auras. Dismissing the pet sends the pet bits zeroed once, then nothing more.
#[test]
fn a_live_pet_sends_its_block_then_dismissing_it_zeroes_the_bits_once() {
    let (_realm, world, _instances) = ginger_and_trin();
    let mut snapshots = Snapshots::new();
    place(
        &world,
        TRIN,
        codec::MemberEntity {
            pet: Some(hunter_pet()),
            ..caster()
        },
    );

    let mut first_send = vec![0x01, TRIN as u8, 0xFF, 0xFF, 0x1F, 0x00]; // mask FULL, live pet
    first_send.extend(caster_base_bytes());
    first_send.extend(empty_positive_aura_bytes()); // AURAS: the member has none
    first_send.extend(empty_negative_aura_bytes()); // AURAS_NEGATIVE: the member has none
    first_send.extend(hunter_pet_bytes());
    first_send.extend(full_positive_aura_bytes(2, 400)); // PET_AURAS: slot 2
    first_send.extend(empty_negative_aura_bytes()); // PET_AURAS_NEGATIVE: the pet has none
    assert_eq!(
        tick(&world, &[GINGER], &mut snapshots),
        vec![(0x007E, first_send)]
    );

    place(&world, TRIN, caster()); // the pet is dismissed
    let dismissed = vec![
        0x01, TRIN as u8, // packed guid
        0x00, 0xF8, 0x0F, 0x00, // mask 0xFF800: every pet bit but PET_AURAS_NEGATIVE
        0, 0, 0, 0, 0, 0, 0, 0,    // PET_GUID zeroed
        0x00, // PET_NAME: an empty CString
        0x00, 0x00, // PET_MODEL_ID zeroed
        0x00, 0x00, // PET_CUR_HP zeroed
        0x00, 0x00, // PET_MAX_HP zeroed
        0x00, // PET_POWER_TYPE zeroed
        0x00, 0x00, // PET_CUR_POWER zeroed
        0x00, 0x00, // PET_MAX_POWER zeroed
        0x04, 0x00, 0x00, 0x00, // PET_AURAS: slot 2 cleared, the one differing bit
        0x00, 0x00, // spell id 0
    ];
    assert_eq!(
        tick(&world, &[GINGER], &mut snapshots),
        vec![(0x007E, dismissed)]
    );

    assert!(
        tick(&world, &[GINGER], &mut snapshots).is_empty(),
        "a pet-less member that stays pet-less sends nothing more about it"
    );
}

/// AC4: a member without a pet gets a FULL packet with no pet bits (already the shape
/// `caster_full_body` carries; this test names the mask directly, for AC traceability).
#[test]
fn a_member_without_a_pet_gets_a_full_answer_with_no_pet_bits() {
    let (_realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    place(&instances, VIM, caster());

    let packets = answer(request(&world, Some(GINGER), VIM));

    assert_eq!(packets, vec![(0x02F2, caster_full_body(VIM as u8))]);
    let mask = u32::from_le_bytes(packets[0].1[3..7].try_into().unwrap());
    assert_eq!(
        mask & codec::GroupUpdateMask::PET.bits(),
        0,
        "no pet bit set"
    );
}

/// AC5: a member inside the viewer's AOI is already created on the client and gets nothing, auras
/// and a pet included.
#[test]
fn a_created_mate_with_auras_and_a_pet_still_gets_nothing() {
    let (_realm, world, _instances) = ginger_and_trin();
    place(
        &world,
        TRIN,
        codec::MemberEntity {
            pet: Some(hunter_pet()),
            ..caster_with_auras()
        },
    );
    let mut snapshots = Snapshots::new();

    assert!(tick(&world, &[GINGER, TRIN], &mut snapshots).is_empty());
    assert!(snapshots.is_empty());
}
