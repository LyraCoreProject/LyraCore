//! Realm Presence's multi-shard union — the routing tests for reads that moved out of `party.rs`.
//!
//! What EXECUTES here is production `world::presence`, against the same in-memory multi-database
//! topology `party_tests::party_topology` builds (Ginger in the open world on `world`, Vim inside
//! the dungeon on `instances`, plus an offline character and a playerbot on each side).

use super::party_tests::{character, party_topology, BOT, DORMANT, GINGER, VIM};
use super::*;

/// **AC 6 (part 1): `presence::of` answers for a Character on another Shard**, reporting
/// `session_online` from the character row and `in_world` from the live entity rather than from
/// wherever the asking handle happens to sit.
#[test]
fn presence_of_crosses_the_shard_boundary_and_tells_in_world_apart_from_session_online() {
    let (_realm, world, _instances, _calls) = party_topology();

    // Vim lives on `instances`; asked from `world`, the union crosses the boundary.
    let vim = presence::of(world.as_ref(), VIM)
        .unwrap()
        .expect("Vim resolves from the open-world handle");
    assert_eq!(vim.name, "Vim");
    assert!(vim.in_world, "Vim has a live entity on instances");
    assert!(vim.session_online, "Vim is not in offline_guids");

    // Dormant has a character row on `world` but no live entity and is session-offline —
    // existence alone still resolves, and both flags read false.
    let dormant = presence::of(world.as_ref(), DORMANT)
        .unwrap()
        .expect("a character row with no live entity still exists");
    assert!(!dormant.in_world);
    assert!(!dormant.session_online);

    // The playerbot is a live entity with no session (`game_character.online` never flips for
    // one) — `in_world` and `session_online` must disagree for it, in the direction the invite
    // gate needs (README Decision 26: bots stay listed and stay unwhisperable).
    let bot = presence::of(world.as_ref(), BOT)
        .unwrap()
        .expect("the playerbot resolves");
    assert!(bot.in_world, "a playerbot has a live entity");
    assert!(!bot.session_online, "a playerbot never runs player_login");
}

/// **AC 6 (part 2): reads AFK and DND from PLAYER_FLAGS.** A Character with no live entity reads
/// `AwayStatus::None`.
#[test]
fn presence_of_reads_away_status_from_the_live_entitys_player_flags() {
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        away_flags: [(GINGER, lyracore_shared::constants::player_flags::DND)].into(),
        ..Default::default()
    });
    let ginger = presence::of(store.as_ref(), GINGER).unwrap().unwrap();
    assert_eq!(ginger.away, presence::AwayStatus::Dnd);

    let no_entity = std::sync::Arc::new(InMemoryStore {
        characters: vec![character(DORMANT, "Dormant")],
        away_flags: [(DORMANT, lyracore_shared::constants::player_flags::AFK)].into(),
        ..Default::default()
    });
    assert_eq!(
        presence::of(no_entity.as_ref(), DORMANT)
            .unwrap()
            .unwrap()
            .away,
        presence::AwayStatus::None,
        "a Character with no live entity has AwayStatus::None, whatever `away_flags` says"
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
        // No `live_guids` entry: the frozen source row has no live entity.
        ..Default::default()
    });
    let destination = std::sync::Arc::new(InMemoryStore {
        shard: "destination".into(),
        calls,
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        ..Default::default()
    });
    for shard in [&source, &destination] {
        *shard.peers.lock().unwrap() = vec![source.clone(), destination.clone()];
    }

    let found = presence::of(source.as_ref(), GINGER).unwrap().unwrap();
    assert!(
        found.in_world,
        "the destination's live entity must win even though the source's own handle was asked \
         first and has no entity of its own"
    );
}
