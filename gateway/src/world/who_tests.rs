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

/// Decode a RAW `SMSG_WHO` body ([`codec::build_who_response_raw`]'s layout) into
/// `(listed_players, online_players, names)`, in wire order — enough for these tests without
/// duplicating a full reader.
fn decode_who(body: &[u8]) -> (u32, u32, Vec<String>) {
    let listed_players = u32::from_le_bytes(body[0..4].try_into().unwrap());
    let online_players = u32::from_le_bytes(body[4..8].try_into().unwrap());
    let mut rest = &body[8..];
    let mut names = Vec::new();
    for _ in 0..listed_players {
        let name_end = rest.iter().position(|&b| b == 0).unwrap();
        names.push(String::from_utf8(rest[..name_end].to_vec()).unwrap());
        rest = &rest[name_end + 1..];
        let guild_end = rest.iter().position(|&b| b == 0).unwrap();
        rest = &rest[guild_end + 1..];
        rest = &rest[16..]; // level, class, race, zone: u32 each
    }
    (listed_players, online_players, names)
}

/// **AC 1: a requester on a World Shard sees a Character standing in an instance on the Instance
/// Pool, and the reverse.**
#[test]
fn who_lists_a_character_on_the_instance_pool_from_the_world_shard_and_back() {
    let (_realm, world, instances, _calls) = party_topology();
    let request = wide_open_who();

    let (_, body) = who::respond(world.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let (.., names) = decode_who(&body);
    assert!(
        names.iter().any(|n| n == "Ginger"),
        "the requester's own shard: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Vim"),
        "the Instance Pool, across the boundary: {names:?}"
    );

    let (_, body) = who::respond(instances.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let (.., names) = decode_who(&body);
    assert!(
        names.iter().any(|n| n == "Vim"),
        "the requester's own shard: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Ginger"),
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

    let (_, body) = who::respond(store.as_ref(), 1, &wide_open_who())
        .unwrap()
        .expect("a well-formed request always answers");
    let (listed_players, online_players, names) = decode_who(&body);
    assert_eq!(listed_players, 49, "the vanilla client's display cap");
    assert_eq!(names.len(), 49);
    assert_eq!(online_players, 50, "the match count is uncapped");
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

    let (_, body) = who::respond(store.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let (listed_players, _, names) = decode_who(&body);
    assert_eq!(listed_players, 1);
    assert_eq!(names, ["High"]);
}

/// A search string matches a zone name read from the Store — proves the wiring from
/// `store.zone_name` through to the filter, not just the pure rule `who.rs`'s own tests pin. A
/// zone the Fake was never told about stays blank and fails the match.
#[test]
fn a_search_string_matches_a_zone_name_the_store_resolves() {
    let store = std::sync::Arc::new(InMemoryStore {
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Ginger".into(),
            race: 1,
            class: 1,
            level: 10,
            zone_id: 12,
            ..Default::default()
        }],
        live_guids: vec![1],
        zone_names: [(12, "Elwynn Forest".to_string())].into(),
        ..Default::default()
    });
    let mut request = wide_open_who();
    request.search_strings = vec!["elwynn".to_string()];

    let (_, body) = who::respond(store.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let (listed_players, _, names) = decode_who(&body);
    assert_eq!(
        listed_players, 1,
        "the seeded zone name must resolve and match: {names:?}"
    );

    // The same request against a Store that never learned the zone's name: the blank lookup must
    // fail the match rather than matching everything by accident.
    let blank = std::sync::Arc::new(InMemoryStore {
        characters: vec![codec::CharacterView {
            guid: 1,
            name: "Ginger".into(),
            race: 1,
            class: 1,
            level: 10,
            zone_id: 12,
            ..Default::default()
        }],
        live_guids: vec![1],
        ..Default::default()
    });
    let (_, body) = who::respond(blank.as_ref(), 1, &request)
        .unwrap()
        .expect("a well-formed request always answers");
    let (listed_players, ..) = decode_who(&body);
    assert_eq!(
        listed_players, 0,
        "a blank zone name must not match \"elwynn\""
    );
}
