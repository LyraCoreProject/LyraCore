//! `/who`'s Store-Fake tests: the multi-shard listing and the 49-cap/oversized-request rules that
//! need real Realm Presence rows. `who.rs`'s own `#[cfg(test)]` module covers every filter rule
//! against hand-written candidates; this file covers what only a Store can exercise.
//!
//! What EXECUTES here is production `world::who`, against the same in-memory multi-database
//! topology `party_tests::party_topology` builds, and a plain single-shard `InMemoryStore` for the
//! unsharded case.

use super::party_tests::{character, party_topology};
use super::*;
use wow_world_messages::vanilla::{Level, CMSG_WHO};

/// Every filter wide open — level 0 through "no upper bound", every race and class bit set, no
/// zone or name restriction. What a client sends with nothing filtered in the `/who` panel.
fn wide_open_who() -> CMSG_WHO {
    CMSG_WHO {
        minimum_level: Level::new(0),
        maximum_level: Level::new(100),
        player_name: String::new(),
        guild_name: String::new(),
        race_mask: u32::MAX,
        class_mask: u32::MAX,
        zones: Vec::new(),
        search_strings: Vec::new(),
    }
}

/// **AC 1: a requester on a World Shard sees a Character standing in an instance on the Instance
/// Pool, and the reverse.**
#[test]
fn who_lists_a_character_on_the_instance_pool_from_the_world_shard_and_back() {
    let (_realm, world, instances, _calls) = party_topology();
    let request = wide_open_who();

    let from_world = who::respond(world.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let names: Vec<_> = from_world.players.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"Ginger"),
        "the requester's own shard: {names:?}"
    );
    assert!(
        names.contains(&"Vim"),
        "the Instance Pool, across the boundary: {names:?}"
    );

    let from_instances = who::respond(instances.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let names: Vec<_> = from_instances
        .players
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(
        names.contains(&"Vim"),
        "the requester's own shard: {names:?}"
    );
    assert!(
        names.contains(&"Ginger"),
        "the World Shard, across the boundary: {names:?}"
    );
}

/// **AC 4: fifty matches list 49 and report 50.**
#[test]
fn fifty_matches_list_forty_nine_and_report_fifty() {
    let characters: Vec<_> = (0..50u64)
        .map(|i| character(100 + i, &format!("Who{i}")))
        .collect();
    let live_guids = characters.iter().map(|c| c.guid).collect();
    let store = std::sync::Arc::new(InMemoryStore {
        characters,
        live_guids,
        ..Default::default()
    });

    let resp = who::respond(store.as_ref(), 1, &wide_open_who())
        .unwrap()
        .expect("a well-formed request always answers");
    assert_eq!(resp.players.len(), 49, "the vanilla client's display cap");
    assert_eq!(resp.online_players, 50, "the match count is uncapped");
}

/// **AC 8: an unsharded Gateway answers `/who` from its one database with the same filters** — no
/// `world_stores()` fan-out, but the level-range rule still applies exactly as it would sharded.
#[test]
fn an_unsharded_gateway_answers_who_with_the_same_filters() {
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![
            codec::CharacterView {
                guid: 1,
                name: "Low".into(),
                race: 1,
                class: 1,
                level: 5,
                ..Default::default()
            },
            codec::CharacterView {
                guid: 2,
                name: "High".into(),
                race: 1,
                class: 1,
                level: 40,
                ..Default::default()
            },
        ],
        live_guids: vec![1, 2],
        ..Default::default()
    });
    assert!(
        store.world_stores().is_empty(),
        "an unsharded gateway has no world_stores() fan-out"
    );
    let mut request = wide_open_who();
    request.minimum_level = Level::new(20);

    let resp = who::respond(store.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    assert_eq!(resp.players.len(), 1);
    assert_eq!(resp.players[0].name, "High");
}
