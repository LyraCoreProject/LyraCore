//! The Meeting Stone Queue on a private Standalone: the Home Shard admission, JOIN and LEAVE for a
//! lone Seeker and for a Party, and a solo Seeker ending with its Account Claim. One database plays
//! both the Home Shard and Realm-core, as on a single-database realm.

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;
use support::Standalone;

// Fixture ids sit in 509_6000-509_6099.
const DEADMINES_STONE: (u32, u64) = (509_6000, 509_6001);
const STOCKADE_STONE: (u32, u64) = (509_6002, 509_6003);
/// The Deadmines (AreaTable 1581), levels 15 to 20.
const DEADMINES: u32 = 1581;
/// The Stockade (AreaTable 717), levels 22 to 30.
const STOCKADE: u32 = 717;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

const HUMAN: u8 = 1;
const WARRIOR: u8 = 1;
const PRIEST: u8 = 5;
const MAGE: u8 = 8;

const JOIN: &str = "0";
const LEAVE: &str = "1";

// SMSG_MEETINGSTONE_SETQUEUE status bytes, cm:LFG/LFGDefines.h:50-58.
const LEAVE_QUEUE: u8 = 0;
const JOINED_QUEUE: u8 = 1;
const NONE: u8 = 5;

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    // Group events are reaped by the shared GC. Disarming it keeps every QUEUE row for the whole
    // test, so each assertion reads the rows an action added by id.
    realm.assert_sql("DELETE FROM game_event_reaper_schedule");
    stage_stone(&realm, DEADMINES_STONE, 15, 20, DEADMINES);
    stage_stone(&realm, STOCKADE_STONE, 22, 30, STOCKADE);
    realm
}

fn stage_stone(realm: &Standalone, (entry, go): (u32, u64), min: u32, max: u32, area: u32) {
    let (x, y, z) = STONE_AT;
    realm.assert_call(
        "debug_stage_meeting_stone",
        &[
            &entry.to_string(),
            &go.to_string(),
            &min.to_string(),
            &max.to_string(),
            &area.to_string(),
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
}

/// A Character `offset` yards east of the stones on `map`, with a live claim of its own Account.
fn stage_character(realm: &Standalone, guid: u64, class: u8, level: u8, map: u32, offset: f32) {
    stage_character_on(realm, guid, guid, class, level, map, offset);
}

fn stage_character_on(
    realm: &Standalone,
    guid: u64,
    account_id: u64,
    class: u8,
    level: u8,
    map: u32,
    offset: f32,
) {
    let (x, y, z) = STONE_AT;
    realm.assert_call(
        "debug_stage_meeting_stone_character",
        &[
            &guid.to_string(),
            &account_id.to_string(),
            &HUMAN.to_string(),
            &class.to_string(),
            &level.to_string(),
            &map.to_string(),
            &(x + offset).to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
}

fn stage_group(realm: &Standalone, leader: u64, members: &[u64], raid: bool) {
    realm.assert_call(
        "debug_stage_meeting_stone_group",
        &[
            &leader.to_string(),
            &json!(members).to_string(),
            &raid.to_string(),
        ],
    );
}

fn token(account_id: u64, guid: u64) -> serde_json::Value {
    json!({ "account_id": account_id, "generation": 1, "request_nonce": guid })
}

/// The World Session actor the fixture claim admits.
fn session(guid: u64) -> String {
    session_on(guid, guid)
}

fn session_on(guid: u64, account_id: u64) -> String {
    json!({ "guid": guid, "ownership": { "some": token(account_id, guid) } }).to_string()
}

fn facts(seekers: &[(u64, u8)]) -> String {
    json!(seekers
        .iter()
        .map(|&(guid, class)| json!({ "character_guid": guid, "race": HUMAN, "class": class }))
        .collect::<Vec<_>>())
    .to_string()
}

fn admit(realm: &Standalone, actor: &str, (_, go): (u32, u64)) {
    realm.assert_call("gw_admit_meeting_stone", &[actor, &go.to_string()]);
}

fn stone_op(realm: &Standalone, op: &str, actor: &str, area: u32, seekers: &[(u64, u8)]) {
    realm.assert_call(
        "realm_meeting_stone_op",
        &[op, actor, &area.to_string(), &facts(seekers)],
    );
}

fn refused(realm: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let result = realm.call(reducer, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "{reducer} should refuse with {tag}: {output}"
    );
}

/// Every QUEUE event so far, as `(id, recipient, payload)` in insert order.
fn queue_events(realm: &Standalone) -> Vec<(u64, u64, String)> {
    let mut rows: Vec<_> = realm
        .query_rows("SELECT id, recipient_guid, payload FROM game_group_event WHERE kind = 27")
        .into_iter()
        .map(|row| {
            (
                row["id"].parse().unwrap(),
                row["recipient_guid"].parse().unwrap(),
                row["payload"].clone(),
            )
        })
        .collect();
    rows.sort_unstable();
    rows
}

/// The QUEUE events `action` adds, as `(recipient, payload)` in insert order.
fn queue_events_of(realm: &Standalone, action: impl FnOnce()) -> Vec<(u64, String)> {
    let seen = queue_events(realm).last().map_or(0, |event| event.0);
    action();
    queue_events(realm)
        .into_iter()
        .filter(|event| event.0 > seen)
        .map(|(_, recipient, payload)| (recipient, payload))
        .collect()
}

fn queue(area: u32, status: u8) -> String {
    format!("{area},{status}")
}

/// Every Seeker row by guid: `(area_id, team, class, group_id)`.
fn seekers(realm: &Standalone) -> BTreeMap<u64, (String, String, String, String)> {
    realm
        .query_rows("SELECT * FROM game_meeting_stone_seeker")
        .into_iter()
        .map(|row| {
            (
                row["character_guid"].parse().unwrap(),
                (
                    row["area_id"].clone(),
                    row["team"].clone(),
                    row["class"].clone(),
                    row["group_id"].clone(),
                ),
            )
        })
        .collect()
}

fn parties(realm: &Standalone) -> Vec<BTreeMap<String, String>> {
    realm.query_rows("SELECT * FROM game_meeting_stone_party")
}

fn group_id_of(realm: &Standalone, guid: u64) -> String {
    realm.query_rows(&format!(
        "SELECT group_id FROM game_group_member WHERE character_guid = {guid}"
    ))[0]["group_id"]
        .clone()
}

fn solo(area: u32, class: u8) -> (String, String, String, String) {
    (
        area.to_string(),
        "469".to_string(),
        class.to_string(),
        "0".to_string(),
    )
}

/// Criterion 3: one SETQUEUE(area, JOINED) and one Seeker row; a second stone replaces the row.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_lone_seeker_queues_once_and_a_second_stone_replaces_its_row() {
    let realm = start("meeting-stone-solo");
    const ALDO: u64 = 509_6010;
    stage_character(&realm, ALDO, WARRIOR, 18, 0, 0.0);

    admit(&realm, &session(ALDO), DEADMINES_STONE);
    let joined = queue_events_of(&realm, || {
        stone_op(&realm, JOIN, &session(ALDO), DEADMINES, &[(ALDO, WARRIOR)])
    });
    assert_eq!(joined, [(ALDO, queue(DEADMINES, JOINED_QUEUE))]);
    assert_eq!(
        seekers(&realm),
        BTreeMap::from([(ALDO, solo(DEADMINES, WARRIOR))])
    );

    // Level 22 is inside the Stockade range and above the Deadmines one.
    stage_character(&realm, ALDO, WARRIOR, 22, 0, 0.0);
    admit(&realm, &session(ALDO), STOCKADE_STONE);
    let joined = queue_events_of(&realm, || {
        stone_op(&realm, JOIN, &session(ALDO), STOCKADE, &[(ALDO, WARRIOR)])
    });
    assert_eq!(joined, [(ALDO, queue(STOCKADE, JOINED_QUEUE))]);
    assert_eq!(
        seekers(&realm),
        BTreeMap::from([(ALDO, solo(STOCKADE, WARRIOR))])
    );
    assert!(parties(&realm).is_empty());
}

/// Criterion 4: every admission Refusal leaves no row and no event.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn the_stone_admission_refuses_without_writing_anything() {
    let realm = start("meeting-stone-admission");
    const BRAM: u64 = 509_6020;
    let deadmines = DEADMINES_STONE.1.to_string();
    let before = queue_events(&realm);
    let admission = |actor: &str, go: &str, tag: &str| {
        refused(&realm, "gw_admit_meeting_stone", &[actor, go], tag);
    };

    stage_character(&realm, BRAM, MAGE, 15, 0, 30.0);
    admission(&session(BRAM), &deadmines, "meeting_stone:out_of_range");
    stage_character(&realm, BRAM, MAGE, 15, 1, 0.0);
    admission(&session(BRAM), &deadmines, "meeting_stone:other_partition");
    stage_character(&realm, BRAM, MAGE, 14, 0, 0.0);
    admission(
        &session(BRAM),
        &deadmines,
        "meeting_stone:level_out_of_range",
    );
    stage_character(&realm, BRAM, MAGE, 21, 0, 0.0);
    admission(
        &session(BRAM),
        &deadmines,
        "meeting_stone:level_out_of_range",
    );
    stage_character(&realm, BRAM, MAGE, 20, 0, 0.0);
    admit(&realm, &session(BRAM), DEADMINES_STONE);

    realm.assert_sql(&format!(
        "INSERT INTO game_active_taxi_flight (character_guid, path_id, source_node_id, \
         destination_node_id, mount_display_id, fare, current_node_index, started_micros) \
         VALUES ({BRAM}, 1, 1, 2, 0, 0, 0, 0)"
    ));
    admission(
        &session(BRAM),
        &deadmines,
        "meeting_stone:actor_unavailable",
    );
    realm.assert_sql(&format!(
        "DELETE FROM game_active_taxi_flight WHERE character_guid = {BRAM}"
    ));

    // Without a World Session the actor carries no token. Its claim is gone, so the shared
    // ownership Gate lets it through and the stone refuses it.
    realm.assert_sql(&format!(
        "DELETE FROM game_account_claim WHERE account_id = {BRAM}"
    ));
    admission(
        &support::actor(&BRAM.to_string()),
        &deadmines,
        "meeting_stone:actor_unavailable",
    );
    refused(
        &realm,
        "realm_meeting_stone_op",
        &[
            JOIN,
            &support::actor(&BRAM.to_string()),
            &DEADMINES.to_string(),
            &facts(&[(BRAM, MAGE)]),
        ],
        "meeting_stone:actor_unavailable",
    );
    stage_character(&realm, BRAM, MAGE, 20, 0, 0.0);

    realm.assert_sql(&format!(
        "UPDATE game_gameobject_template SET type_id = 3 WHERE entry = {}",
        DEADMINES_STONE.0
    ));
    admission(
        &session(BRAM),
        &deadmines,
        "meeting_stone:not_a_meeting_stone",
    );
    stage_stone(&realm, DEADMINES_STONE, 15, 20, DEADMINES);
    realm.assert_sql(&format!(
        "DELETE FROM game_meeting_stone WHERE entry = {}",
        DEADMINES_STONE.0
    ));
    admission(
        &session(BRAM),
        &deadmines,
        "meeting_stone:not_a_meeting_stone",
    );
    admission(
        &session(BRAM),
        "509609999",
        "meeting_stone:not_a_meeting_stone",
    );

    assert!(seekers(&realm).is_empty());
    assert_eq!(queue_events(&realm), before);
}

/// Criterion 5: the three party Refusals in cmangos's order, none of which writes anything.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_join_refusal_changes_nothing() {
    let realm = start("meeting-stone-party-refusals");
    const LEADER: u64 = 509_6030;
    const MEMBER: u64 = 509_6031;
    const FULL_LEADER: u64 = 509_6032;
    const RAID_LEADER: u64 = 509_6040;
    const RAIDER: u64 = 509_6041;
    let full_party: Vec<u64> = (FULL_LEADER + 1..FULL_LEADER + 5).collect();
    for guid in [LEADER, MEMBER, FULL_LEADER, RAID_LEADER, RAIDER]
        .into_iter()
        .chain(full_party.iter().copied())
    {
        stage_character(&realm, guid, WARRIOR, 18, 0, 0.0);
    }
    stage_group(&realm, LEADER, &[MEMBER], false);
    stage_group(&realm, FULL_LEADER, &full_party, false);
    stage_group(&realm, RAID_LEADER, &[RAIDER], true);
    let before = queue_events(&realm);
    let join = |actor: u64, tag: &str| {
        refused(
            &realm,
            "realm_meeting_stone_op",
            &[
                JOIN,
                &session(actor),
                &DEADMINES.to_string(),
                &facts(&[(actor, WARRIOR)]),
            ],
            tag,
        );
    };

    join(MEMBER, "meeting_stone:not_leader");
    join(RAIDER, "meeting_stone:not_leader");
    join(RAID_LEADER, "meeting_stone:raid_group");
    join(FULL_LEADER, "meeting_stone:party_full");

    assert!(seekers(&realm).is_empty());
    assert!(parties(&realm).is_empty());
    assert_eq!(queue_events(&realm), before);
}

/// Criterion 6: one party row, one Seeker row per member with the class the Gateway supplied, and
/// SETQUEUE(area, JOINED) to every member.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_party_leaders_join_queues_every_member() {
    let realm = start("meeting-stone-party-join");
    const CARA: u64 = 509_6050;
    const DUNN: u64 = 509_6051;
    const ELLA: u64 = 509_6052;
    stage_character(&realm, CARA, WARRIOR, 18, 0, 0.0);
    // The members stand elsewhere: a party queues from its leader's stone alone.
    stage_character(&realm, DUNN, PRIEST, 30, 1, 0.0);
    stage_character(&realm, ELLA, MAGE, 10, 0, 500.0);
    stage_group(&realm, CARA, &[DUNN, ELLA], false);
    let group_id = group_id_of(&realm, CARA);

    admit(&realm, &session(CARA), DEADMINES_STONE);
    let joined = queue_events_of(&realm, || {
        stone_op(
            &realm,
            JOIN,
            &session(CARA),
            DEADMINES,
            &[(CARA, WARRIOR), (DUNN, PRIEST)],
        )
    });

    let mut recipients: Vec<u64> = joined.iter().map(|(guid, _)| *guid).collect();
    recipients.sort_unstable();
    assert_eq!(recipients, [CARA, DUNN, ELLA]);
    assert!(joined
        .iter()
        .all(|(_, payload)| *payload == queue(DEADMINES, JOINED_QUEUE)));
    let member = |class: u8| {
        (
            DEADMINES.to_string(),
            "469".to_string(),
            class.to_string(),
            group_id.clone(),
        )
    };
    // ELLA's facts were missing, so she holds a seat and no class.
    assert_eq!(
        seekers(&realm),
        BTreeMap::from([
            (CARA, member(WARRIOR)),
            (DUNN, member(PRIEST)),
            (ELLA, member(0)),
        ])
    );
    let party = parties(&realm);
    assert_eq!(party.len(), 1);
    assert_eq!(party[0]["group_id"], group_id);
    assert_eq!(party[0]["area_id"], DEADMINES.to_string());
    assert_eq!(party[0]["team"], "469");
}

/// Criterion 7: LEAVE per cm:LFG/LFGHandler.cpp:86-110.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn leave_answers_the_seeker_the_party_and_the_member() {
    let realm = start("meeting-stone-leave");
    const FINN: u64 = 509_6060;
    const GALE: u64 = 509_6061;
    const HUGO: u64 = 509_6062;
    const IONA: u64 = 509_6063;
    for (guid, class) in [(FINN, MAGE), (GALE, WARRIOR), (HUGO, PRIEST), (IONA, MAGE)] {
        stage_character(&realm, guid, class, 18, 0, 0.0);
    }

    // Nobody queued, no party: nothing happens.
    let nothing = queue_events_of(&realm, || stone_op(&realm, LEAVE, &session(FINN), 0, &[]));
    assert!(nothing.is_empty());

    stone_op(&realm, JOIN, &session(FINN), DEADMINES, &[(FINN, MAGE)]);
    let left = queue_events_of(&realm, || stone_op(&realm, LEAVE, &session(FINN), 0, &[]));
    assert_eq!(left, [(FINN, queue(0, LEAVE_QUEUE))]);
    assert!(seekers(&realm).is_empty());

    stage_group(&realm, GALE, &[HUGO, IONA], false);
    stone_op(
        &realm,
        JOIN,
        &session(GALE),
        DEADMINES,
        &[(GALE, WARRIOR), (HUGO, PRIEST), (IONA, MAGE)],
    );
    let member_left = queue_events_of(&realm, || stone_op(&realm, LEAVE, &session(HUGO), 0, &[]));
    assert_eq!(member_left, [(HUGO, queue(0, NONE))]);
    assert_eq!(seekers(&realm).len(), 3, "a member's LEAVE changes no row");
    assert_eq!(parties(&realm).len(), 1);

    let party_left = queue_events_of(&realm, || stone_op(&realm, LEAVE, &session(GALE), 0, &[]));
    let mut recipients: Vec<u64> = party_left.iter().map(|(guid, _)| *guid).collect();
    recipients.sort_unstable();
    assert_eq!(recipients, [GALE, HUGO, IONA]);
    assert!(party_left
        .iter()
        .all(|(_, payload)| *payload == queue(0, LEAVE_QUEUE)));
    assert!(seekers(&realm).is_empty());
    assert!(parties(&realm).is_empty());

    // The leader of a party that is not queued gets NONE, like any member.
    let unqueued = queue_events_of(&realm, || stone_op(&realm, LEAVE, &session(GALE), 0, &[]));
    assert_eq!(unqueued, [(GALE, queue(0, NONE))]);
}

/// Criterion 9: release, reap and replacement each drop a solo Seeker without a packet. A party
/// Seeker survives all three.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_solo_seeker_ends_with_its_account_claim() {
    let realm = start("meeting-stone-claims");
    const RELEASED: u64 = 509_6070;
    const REAPED: u64 = 509_6071;
    const REPLACED: u64 = 509_6072;
    const LEADER: u64 = 509_6080;
    const MEMBER: u64 = 509_6081;
    const SEATED: u64 = 509_6082;
    // `claim_account` needs a real Account row, and the seeded Account 1 is one. Its claim admits
    // SEATED first and REPLACED after.
    const SEEDED_ACCOUNT: u64 = 1;
    let replace_seeded_claim = |nonce: &str| {
        realm.assert_sql(&format!(
            "UPDATE game_account_claim SET expires_micros = 0 WHERE account_id = {SEEDED_ACCOUNT}"
        ));
        realm.assert_call("claim_account", &["1", "1", nonce]);
    };
    for guid in [RELEASED, REAPED, LEADER, MEMBER] {
        stage_character(&realm, guid, MAGE, 18, 0, 0.0);
    }
    stage_character_on(&realm, SEATED, SEEDED_ACCOUNT, MAGE, 18, 0, 0.0);
    stage_group(&realm, LEADER, &[MEMBER, SEATED], false);
    stone_op(
        &realm,
        JOIN,
        &session(LEADER),
        DEADMINES,
        &[(LEADER, MAGE), (MEMBER, MAGE), (SEATED, MAGE)],
    );
    for guid in [RELEASED, REAPED] {
        stone_op(&realm, JOIN, &session(guid), DEADMINES, &[(guid, MAGE)]);
    }
    let before = queue_events(&realm);

    // Replaced, for a party Seeker: the row stays.
    replace_seeded_claim("9001");
    stage_character_on(&realm, REPLACED, SEEDED_ACCOUNT, MAGE, 18, 0, 0.0);
    stone_op(
        &realm,
        JOIN,
        &session_on(REPLACED, SEEDED_ACCOUNT),
        DEADMINES,
        &[(REPLACED, MAGE)],
    );
    let joined = queue_events(&realm);

    // Released.
    for guid in [RELEASED, LEADER] {
        realm.assert_call("release_account_claim", &[&token(guid, guid).to_string()]);
    }
    assert!(!seekers(&realm).contains_key(&RELEASED));

    // Replaced, for a solo Seeker.
    replace_seeded_claim("9002");
    assert!(!seekers(&realm).contains_key(&REPLACED));

    // Reaped by the Gateway lease schedule.
    realm.assert_sql(&format!(
        "UPDATE game_account_claim SET expires_micros = 0 WHERE account_id = {REAPED} \
         OR account_id = {MEMBER}"
    ));
    realm.assert_call("gw_heartbeat", &[]);
    assert!(
        support::poll_until(Duration::from_secs(30), || !seekers(&realm)
            .contains_key(&REAPED)),
        "the claim reaper never took {REAPED} out of the queue"
    );

    let remaining: Vec<u64> = seekers(&realm).into_keys().collect();
    assert_eq!(remaining, [LEADER, MEMBER, SEATED]);
    assert_eq!(
        joined.len(),
        before.len() + 1,
        "only REPLACED's own JOIN notified anyone"
    );
    assert_eq!(
        queue_events(&realm),
        joined,
        "an ending claim sends no packet"
    );
}
