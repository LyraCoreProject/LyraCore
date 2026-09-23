//! Realm Presence's multi-shard union — the routing tests for reads that moved out of `party.rs`
//! and out of `handlers/member_stats.rs`'s own `locate_member`.
//!
//! What EXECUTES here is production `world::presence`, against the same in-memory multi-database
//! topology `party_tests::party_topology` builds (Ginger in the open world on `world`, Vim inside
//! the dungeon on `instances`, plus an offline character and a playerbot on each side), and small
//! bespoke topologies where a test needs to set a field `party_topology` does not expose.
//!
//! A live entity is `member_entities` alone (`place`, below) — not `live_guids`, which models a
//! different, party-eligibility presence and stays independent of it on purpose (see `live_entity`
//! in `tests.rs`).

use super::party_tests::{character, party_topology, BOT, DORMANT, GINGER, VIM};
use super::*;

/// Give `shard` a live Member Stats entity for `guid` — the one signal `live_entity` reads.
fn place(shard: &InMemoryStore, guid: u64, entity: codec::MemberEntity) {
    shard.member_entities.lock().unwrap().push((guid, entity));
}

/// **AC 6 (part 1): `presence::of` answers for a Character on another Shard**, reporting
/// `session_online` from the character row and `Whereabouts::InWorld` from the live entity rather
/// than from wherever the asking handle happens to sit.
#[test]
fn presence_of_crosses_the_shard_boundary_and_tells_in_world_apart_from_session_online() {
    let (_realm, world, instances, _calls) = party_topology();
    place(&instances, VIM, codec::MemberEntity::default());
    place(&world, BOT, codec::MemberEntity::default());

    // Vim lives on `instances`; asked from `world`, the union crosses the boundary.
    let vim = presence::of(world.as_ref(), VIM)
        .unwrap()
        .expect("Vim resolves from the open-world handle");
    assert_eq!(vim.name, "Vim");
    assert!(
        matches!(vim.whereabouts, presence::Whereabouts::InWorld { .. }),
        "Vim has a live entity on instances: {:?}",
        vim.whereabouts
    );
    assert!(vim.session_online, "Vim is not in offline_guids");

    // Dormant has a character row on `world` but no live entity and is session-offline —
    // existence alone still resolves, and it reads Offline with session_online false.
    let dormant = presence::of(world.as_ref(), DORMANT)
        .unwrap()
        .expect("a character row with no live entity still exists");
    assert_eq!(dormant.whereabouts, presence::Whereabouts::Offline);
    assert!(!dormant.session_online);

    // The playerbot is a live entity with no session (`game_character.online` never flips for
    // one) — `Whereabouts::InWorld` and `session_online` must disagree for it, in the direction
    // the invite gate needs (README Decision 26: bots stay listed and stay unwhisperable).
    let bot = presence::of(world.as_ref(), BOT)
        .unwrap()
        .expect("the playerbot resolves");
    assert!(
        matches!(bot.whereabouts, presence::Whereabouts::InWorld { .. }),
        "a playerbot has a live entity: {:?}",
        bot.whereabouts
    );
    assert!(!bot.session_online, "a playerbot never runs player_login");
}

/// **AC 6 (part 2): reads AFK and DND from PLAYER_FLAGS.** A Character with no live entity reads
/// no Away Status at all.
#[test]
fn presence_of_reads_away_status_from_the_live_entitys_player_flags() {
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![character(GINGER, "Ginger")],
        ..Default::default()
    });
    place(
        &store,
        GINGER,
        codec::MemberEntity {
            player_flags: lyracore_shared::constants::player_flags::DND,
            ..Default::default()
        },
    );
    let ginger = presence::of(store.as_ref(), GINGER).unwrap().unwrap();
    match ginger.whereabouts {
        presence::Whereabouts::InWorld { away, .. } => {
            assert_eq!(away, presence::AwayStatus::Dnd)
        }
        other => panic!("expected InWorld, got {other:?}"),
    }

    let no_entity = std::sync::Arc::new(InMemoryStore {
        characters: vec![character(DORMANT, "Dormant")],
        ..Default::default()
    });
    let dormant = presence::of(no_entity.as_ref(), DORMANT).unwrap().unwrap();
    assert_eq!(
        dormant.whereabouts,
        presence::Whereabouts::Offline,
        "a Character with no live entity has no Away Status to read"
    );
}

/// A hit with a live entity beats a hit without one, so a Character caught between the two halves
/// of a Transfer reads as in world — the source Shard's frozen row (no entity) and the
/// destination's fresh one (a live entity) both exist for the same guid at once.
#[test]
fn a_live_hit_beats_a_hit_without_one_regardless_of_shard_order() {
    let calls: ShardCallLog = Default::default();
    let source = std::sync::Arc::new(InMemoryStore {
        shard: "source".into(),
        calls: calls.clone(),
        characters: vec![character(GINGER, "Ginger")],
        // No live entity: the frozen source row has none.
        ..Default::default()
    });
    let destination = std::sync::Arc::new(InMemoryStore {
        shard: "destination".into(),
        calls,
        characters: vec![character(GINGER, "Ginger")],
        ..Default::default()
    });
    place(&destination, GINGER, codec::MemberEntity::default());
    for shard in [&source, &destination] {
        *shard.peers.lock().unwrap() = vec![source.clone(), destination.clone()];
    }

    let found = presence::of(source.as_ref(), GINGER).unwrap().unwrap();
    assert!(
        matches!(found.whereabouts, presence::Whereabouts::InWorld { .. }),
        "the destination's live entity must win even though the source's own handle was asked \
         first and has no entity of its own: {:?}",
        found.whereabouts
    );
}

/// A minimal two-shard topology this file builds directly (rather than `party_topology`, which
/// does not expose `world_shard_set_error`): a realm handle, and one world Shard holding `guid`.
fn one_shard_topology(
    guid: u64,
    live: bool,
) -> (std::sync::Arc<InMemoryStore>, std::sync::Arc<InMemoryStore>) {
    let realm = std::sync::Arc::new(InMemoryStore {
        shard: "realm".into(),
        is_realm: true,
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        realm: Some(realm.clone()),
        characters: vec![character(guid, "Named")],
        ..Default::default()
    });
    if live {
        place(&world, guid, codec::MemberEntity::default());
    }
    (realm, world)
}

/// A live entity on any connected Shard is live even while another Shard is down — the positive
/// InWorld signal needs no absence proof, unlike Offline. Migrated from
/// `handlers::member_stats::tests`, which drove this through `locate_member` directly before it
/// folded into `presence::of`.
#[test]
fn a_live_entity_on_any_connected_shard_is_live_even_while_another_shard_is_down() {
    let (_realm, world) = one_shard_topology(GINGER, true);
    let down = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), down.clone()];
    *down.peers.lock().unwrap() = vec![world.clone(), down.clone()];

    let presence = presence::of(world.as_ref(), GINGER).unwrap().unwrap();
    assert!(
        matches!(presence.whereabouts, presence::Whereabouts::InWorld { .. }),
        "Ginger's live entity must win even though `instances` is down: {:?}",
        presence.whereabouts
    );
}

/// A pending Transfer on Realm-core is in transit, even when the World Shards cannot vouch for
/// absence — `realm_transfer_pending` is checked before the health-gated absence path.
#[test]
fn a_pending_transfer_on_realm_core_is_in_transit() {
    let (realm, world) = one_shard_topology(GINGER, false);
    *realm.members_in_transit.lock().unwrap() = vec![GINGER];
    let down = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        world_shard_set_error: Some("down".into()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), down.clone()];

    let presence = presence::of(world.as_ref(), GINGER).unwrap().unwrap();
    assert_eq!(presence.whereabouts, presence::Whereabouts::InTransit);
}

/// A Character between places on any connected Shard is in transit — the session-less bot
/// Transfer Intent case `character_in_transit` models, here on a PEER Shard rather than the
/// asking one.
#[test]
fn a_character_between_places_on_any_shard_is_in_transit() {
    let (_realm, world) = one_shard_topology(BOT, false);
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        members_between_places: vec![BOT].into(),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    *instances.peers.lock().unwrap() = vec![world.clone(), instances.clone()];

    let presence = presence::of(world.as_ref(), BOT).unwrap().unwrap();
    assert_eq!(presence.whereabouts, presence::Whereabouts::InTransit);
}

/// Offline needs every configured Shard healthy — an unreadable Shard or Realm-core is an `Err`,
/// never a false Offline.
#[test]
fn offline_needs_every_configured_shard_healthy() {
    let (_realm, world) = one_shard_topology(DORMANT, false);
    let down = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), down.clone()];
    assert!(
        presence::of(world.as_ref(), DORMANT).is_err(),
        "an unhealthy configured Shard must refuse to let Offline through"
    );

    let healthy = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), healthy.clone()];
    let presence = presence::of(world.as_ref(), DORMANT).unwrap().unwrap();
    assert_eq!(presence.whereabouts, presence::Whereabouts::Offline);
}

/// A frozen pre-Transfer copy (`begin_transfer` persists with `set_offline: false`) keeps
/// `session_online` true with no live entity anywhere. It must not read as present
/// (`Whereabouts::InWorld`): the Shard's own between-places signal — modelling its online row with
/// no entity — reads it as `InTransit` instead, decoupled from `session_online`.
#[test]
fn a_frozen_pre_transfer_copy_reads_online_but_not_in_world() {
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![character(GINGER, "Ginger")],
        // Not in `offline_guids`: session-online. No live entity. `members_between_places` is this
        // Shard's own signal for it, the same one `character_in_transit` reads in production from
        // `game_character.online` with no entity present — exactly the frozen source row
        // `begin_transfer` leaves behind.
        members_between_places: vec![GINGER].into(),
        ..Default::default()
    });
    let presence = presence::of(store.as_ref(), GINGER).unwrap().unwrap();
    assert!(presence.session_online, "the frozen row keeps online true");
    assert!(
        !matches!(presence.whereabouts, presence::Whereabouts::InWorld { .. }),
        "a frozen copy with no live entity must never read as present: {:?}",
        presence.whereabouts
    );
    assert_eq!(presence.whereabouts, presence::Whereabouts::InTransit);
}
