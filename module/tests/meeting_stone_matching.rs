//! Meeting Stone matching on a private Standalone: the bucket pass forms and fills Parties by Open
//! Role, the group cores keep a queued Party's queue state in step with its roster, and the
//! reminder tick nudges a waiting Party. One database plays both the Home Shard and Realm-core, as
//! on a single-database realm. The hook tests run each case through `realm_group_op`, the
//! Realm-core plane, and through the `gw_group_*` reducers, the single-database plane.

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::json;
use support::Standalone;

// Fixture ids sit in 509_6000-509_6099.
/// The Deadmines (AreaTable 1581).
const DEADMINES: u32 = 1581;
/// The Stockade (AreaTable 717).
const STOCKADE: u32 = 717;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

const HUMAN: u8 = 1;
const ORC: u8 = 2;
const ALLIANCE: u32 = 469;
const HORDE: u32 = 67;

const WARRIOR: u8 = 1;
const PRIEST: u8 = 5;
const MAGE: u8 = 8;

// realm_meeting_stone_op bytes.
const JOIN: &str = "0";

// realm_group_op bytes.
const INVITE: &str = "0";
const ACCEPT: &str = "1";
const LEAVE: &str = "3";
const UNINVITE: &str = "4";
const RAID_CONVERT: &str = "6";

// SMSG_MEETINGSTONE_SETQUEUE status bytes, cm:LFG/LFGDefines.h:50-58.
const LEAVE_QUEUE: u8 = 0;
const JOINED_QUEUE: u8 = 1;
const PARTY_MEMBER_LEFT_LFG: u8 = 2;
const PARTY_MEMBER_REMOVED_PARTY_REMOVED: u8 = 3;
const LOOKING_FOR_NEW_PARTY_IN_QUEUE: u8 = 4;
const NONE: u8 = 5;

/// `game_group_event` kinds: the roster list and the four Meeting Stone kinds.
const LIST: u8 = 1;
const QUEUE: u8 = 27;
const MEMBER_ADDED: u8 = 28;
const IN_PROGRESS: u8 = 29;
const COMPLETE: u8 = 30;

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    // Group events are reaped by the shared GC. Disarming it keeps every row for the whole test,
    // so each assertion reads the rows an action added by id.
    realm.assert_sql("DELETE FROM game_event_reaper_schedule");
    realm
}

/// A Character at the Deadmines stone with a live claim of its own Account.
fn stage(realm: &Standalone, guid: u64, race: u8, class: u8) {
    let (x, y, z) = STONE_AT;
    realm.assert_call(
        "debug_stage_meeting_stone_character",
        &[
            &guid.to_string(),
            &guid.to_string(),
            &race.to_string(),
            &class.to_string(),
            "18",
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
}

fn stage_group(realm: &Standalone, leader: u64, members: &[u64]) {
    realm.assert_call(
        "debug_stage_meeting_stone_group",
        &[&leader.to_string(), &json!(members).to_string(), "false"],
    );
}

/// The World Session actor the fixture claim admits.
fn session(guid: u64) -> String {
    json!({
        "guid": guid,
        "ownership": { "some": { "account_id": guid, "generation": 1, "request_nonce": guid } },
    })
    .to_string()
}

fn stone_join(realm: &Standalone, actor: u64, area: u32, facts: &[(u64, u8, u8)]) {
    let facts: Vec<_> = facts
        .iter()
        .map(|&(guid, race, class)| json!({ "character_guid": guid, "race": race, "class": class }))
        .collect();
    realm.assert_call(
        "realm_meeting_stone_op",
        &[
            JOIN,
            &session(actor),
            &area.to_string(),
            &json!(facts).to_string(),
        ],
    );
}

/// A human Character queues alone.
fn join_alone(realm: &Standalone, guid: u64, class: u8, area: u32) {
    stone_join(realm, guid, area, &[(guid, HUMAN, class)]);
}

/// A human leader queues its whole Party, `(guid, class)` per member.
fn join_party(realm: &Standalone, leader: u64, area: u32, members: &[(u64, u8)]) {
    let facts: Vec<_> = members
        .iter()
        .map(|&(guid, class)| (guid, HUMAN, class))
        .collect();
    stone_join(realm, leader, area, &facts);
}

fn group_op(realm: &Standalone, op: &str, actor: u64, target: u64, arg_a: u8, arg_b: u8) {
    realm.assert_call(
        "realm_group_op",
        &[
            op,
            &session(actor),
            &target.to_string(),
            &arg_a.to_string(),
            &arg_b.to_string(),
            "0",
        ],
    );
}

/// The two ways a Gateway drives the party authority.
#[derive(Clone, Copy, Debug)]
enum Plane {
    /// `realm_group_op` on Realm-core.
    RealmCore,
    /// The `gw_group_*` reducers on a single database.
    SingleDatabase,
}

const PLANES: [Plane; 2] = [Plane::RealmCore, Plane::SingleDatabase];

impl Plane {
    fn start(self, name: &str) -> Standalone {
        start(&format!("{name}-{self:?}").to_lowercase())
    }

    fn invite(self, realm: &Standalone, actor: u64, target: u64) {
        match self {
            Plane::RealmCore => group_op(realm, INVITE, actor, target, 0, 0),
            Plane::SingleDatabase => {
                realm.assert_call("gw_group_invite", &[&session(actor), &target.to_string()])
            }
        }
    }

    /// The Gateway conveys class and race on Realm-core; the single database reads its own row.
    fn accept(self, realm: &Standalone, guid: u64, class: u8) {
        match self {
            Plane::RealmCore => group_op(realm, ACCEPT, guid, 0, class, HUMAN),
            Plane::SingleDatabase => realm.assert_call("gw_accept_group_invite", &[&session(guid)]),
        }
    }

    fn leave(self, realm: &Standalone, guid: u64) {
        match self {
            Plane::RealmCore => group_op(realm, LEAVE, guid, 0, 0, 0),
            Plane::SingleDatabase => realm.assert_call("gw_group_leave", &[&session(guid)]),
        }
    }

    /// Character deletion: a CHARACTER_DELETED leave on Realm-core, the delete sweep on a single
    /// database.
    fn delete(self, realm: &Standalone, guid: u64) {
        match self {
            Plane::RealmCore => group_op(realm, LEAVE, guid, 0, 1, 0),
            Plane::SingleDatabase => {
                realm.assert_call("debug_delete_character", &[&guid.to_string()])
            }
        }
    }

    fn kick(self, realm: &Standalone, actor: u64, target: u64) {
        match self {
            Plane::RealmCore => group_op(realm, UNINVITE, actor, target, 0, 0),
            Plane::SingleDatabase => {
                realm.assert_call("gw_group_uninvite", &[&session(actor), &target.to_string()])
            }
        }
    }
}

/// One `game_group_event` row the tests read.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    List,
    Queue(u32, u8),
    MemberAdded(u64),
    InProgress,
    Complete,
}

/// Every LIST and Meeting Stone event so far, as `(id, recipient, event)` in insert order.
fn events(realm: &Standalone) -> Vec<(u64, u64, Event)> {
    let mut rows: Vec<_> = realm
        .query_rows(&format!(
            "SELECT id, recipient_guid, kind, other_guid, payload FROM game_group_event \
             WHERE kind = {LIST} OR kind >= {QUEUE}"
        ))
        .into_iter()
        .map(|row| {
            let kind: u8 = row["kind"].parse().unwrap();
            let event = match kind {
                LIST => Event::List,
                QUEUE => {
                    let payload = row["payload"].trim_matches('"');
                    let (area, status) = payload.split_once(',').unwrap();
                    Event::Queue(area.parse().unwrap(), status.parse().unwrap())
                }
                MEMBER_ADDED => Event::MemberAdded(row["other_guid"].parse().unwrap()),
                IN_PROGRESS => Event::InProgress,
                COMPLETE => Event::Complete,
                other => panic!("unexpected group event kind {other}"),
            };
            (
                row["id"].parse().unwrap(),
                row["recipient_guid"].parse().unwrap(),
                event,
            )
        })
        .collect();
    rows.sort_unstable_by_key(|(id, _, _)| *id);
    rows
}

/// The events `action` adds, per recipient in insert order.
struct Heard(BTreeMap<u64, Vec<Event>>);

impl Heard {
    /// The Meeting Stone events `guid` heard.
    fn stone(&self, guid: u64) -> Vec<Event> {
        self.with_lists(guid)
            .into_iter()
            .filter(|event| *event != Event::List)
            .collect()
    }

    /// Every event `guid` heard, roster lists included.
    fn with_lists(&self, guid: u64) -> Vec<Event> {
        self.0.get(&guid).cloned().unwrap_or_default()
    }
}

fn heard(realm: &Standalone, action: impl FnOnce()) -> Heard {
    let seen = events(realm).last().map_or(0, |(id, _, _)| *id);
    action();
    let mut by_recipient: BTreeMap<u64, Vec<Event>> = BTreeMap::new();
    for (_, recipient, event) in events(realm).into_iter().filter(|(id, _, _)| *id > seen) {
        by_recipient.entry(recipient).or_default().push(event);
    }
    Heard(by_recipient)
}

/// `(area_id, team, class, group_id)` of one Seeker row.
type SeekerRow = (u32, u32, u8, u64);

fn seekers(realm: &Standalone) -> BTreeMap<u64, SeekerRow> {
    realm
        .query_rows("SELECT * FROM game_meeting_stone_seeker")
        .into_iter()
        .map(|row| {
            (
                row["character_guid"].parse().unwrap(),
                (
                    row["area_id"].parse().unwrap(),
                    row["team"].parse().unwrap(),
                    row["class"].parse().unwrap(),
                    row["group_id"].parse().unwrap(),
                ),
            )
        })
        .collect()
}

fn queued_parties(realm: &Standalone) -> Vec<u64> {
    let mut parties: Vec<u64> = realm
        .query_rows("SELECT group_id FROM game_meeting_stone_party")
        .into_iter()
        .map(|row| row["group_id"].parse().unwrap())
        .collect();
    parties.sort_unstable();
    parties
}

fn group_of(realm: &Standalone, guid: u64) -> Option<u64> {
    realm
        .query_rows(&format!(
            "SELECT group_id FROM game_group_member WHERE character_guid = {guid}"
        ))
        .first()
        .map(|row| row["group_id"].parse().unwrap())
}

/// `group_id`'s members in join order.
fn members(realm: &Standalone, group_id: u64) -> Vec<u64> {
    let mut rows: Vec<(u64, u64)> = realm
        .query_rows(&format!(
            "SELECT id, character_guid FROM game_group_member WHERE group_id = {group_id}"
        ))
        .into_iter()
        .map(|row| {
            (
                row["id"].parse().unwrap(),
                row["character_guid"].parse().unwrap(),
            )
        })
        .collect();
    rows.sort_unstable();
    rows.into_iter().map(|(_, guid)| guid).collect()
}

fn leader_of(realm: &Standalone, group_id: u64) -> u64 {
    realm.query_rows(&format!(
        "SELECT leader_guid FROM game_group WHERE group_id = {group_id}"
    ))[0]["leader_guid"]
        .parse()
        .unwrap()
}

fn revision(realm: &Standalone, group_id: u64) -> Option<u64> {
    realm
        .query_rows(&format!(
            "SELECT revision FROM game_group_roster_revision WHERE group_id = {group_id}"
        ))
        .first()
        .map(|row| row["revision"].parse().unwrap())
}

fn in_party(group_id: u64, class: u8) -> SeekerRow {
    (DEADMINES, ALLIANCE, class, group_id)
}

fn alone(area: u32, team: u32, class: u8) -> SeekerRow {
    (area, team, class, 0)
}

/// Criteria 1 and 4: five Seekers covering every role form one Party that completes at once.
/// Each Stone Add is announced to the members before it, never to the added Character.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn five_seekers_with_every_role_complete_a_party_at_once() {
    let realm = start("stone-match-complete");
    const TANK: u64 = 509_6000;
    const HEALER: u64 = 509_6001;
    const DPS: [u64; 3] = [509_6002, 509_6003, 509_6004];
    let classes = [(TANK, WARRIOR), (HEALER, PRIEST)]
        .into_iter()
        .chain(DPS.map(|guid| (guid, MAGE)));
    for (guid, class) in classes.clone() {
        stage(&realm, guid, HUMAN, class);
    }
    for (guid, class) in classes.clone().take(4) {
        join_alone(&realm, guid, class, DEADMINES);
    }
    assert!(queued_parties(&realm).is_empty(), "four Seekers wait");

    let fifth = heard(&realm, || join_alone(&realm, DPS[2], MAGE, DEADMINES));

    let group = group_of(&realm, TANK).expect("the longest wait leads a new party");
    assert_eq!(leader_of(&realm, group), TANK);
    assert_eq!(
        members(&realm, group),
        [TANK, HEALER, DPS[0], DPS[1], DPS[2]]
    );
    let done = [Event::Complete, Event::Queue(0, NONE)];
    let added = |guids: &[u64]| -> Vec<Event> {
        guids.iter().map(|&guid| Event::MemberAdded(guid)).collect()
    };
    let then = |mut first: Vec<Event>| {
        first.extend(done.clone());
        first
    };
    assert_eq!(
        fifth.stone(TANK),
        then(
            [
                Event::MemberAdded(HEALER),
                Event::Queue(DEADMINES, JOINED_QUEUE)
            ]
            .into_iter()
            .chain(added(&DPS))
            .collect()
        )
    );
    assert_eq!(
        fifth.stone(HEALER),
        then(
            [Event::Queue(DEADMINES, JOINED_QUEUE)]
                .into_iter()
                .chain(added(&DPS))
                .collect()
        )
    );
    assert_eq!(fifth.stone(DPS[0]), then(added(&DPS[1..])));
    assert_eq!(fifth.stone(DPS[1]), then(added(&DPS[2..])));
    assert_eq!(
        fifth.stone(DPS[2]),
        then(vec![Event::Queue(DEADMINES, JOINED_QUEUE)])
    );
    assert!(seekers(&realm).is_empty());
    assert!(queued_parties(&realm).is_empty());
}

/// Criterion 2: five mages form a party of three damage dealers that waits for a tank and a
/// healer. The other two mages keep waiting alone.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn five_mages_form_a_party_that_waits_for_a_tank_and_a_healer() {
    let realm = start("stone-match-mages");
    let mages: Vec<u64> = (509_6010..509_6015).collect();
    for &guid in &mages {
        stage(&realm, guid, HUMAN, MAGE);
        join_alone(&realm, guid, MAGE, DEADMINES);
    }

    let group = group_of(&realm, mages[0]).expect("five mages form a party");
    assert_eq!(members(&realm, group), mages[..3]);
    assert_eq!(queued_parties(&realm), [group]);
    assert_eq!(
        seekers(&realm),
        BTreeMap::from([
            (mages[0], in_party(group, MAGE)),
            (mages[1], in_party(group, MAGE)),
            (mages[2], in_party(group, MAGE)),
            (mages[3], alone(DEADMINES, ALLIANCE, MAGE)),
            (mages[4], alone(DEADMINES, ALLIANCE, MAGE)),
        ])
    );
}

/// Criterion 3: a queued party with only its healer open takes the longest-waiting priest. The
/// second priest keeps waiting. The leader hears MEMBER_ADDED before the roster list it precedes.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_queued_party_takes_the_longest_waiting_healer() {
    let realm = start("stone-match-healer");
    const LEADER: u64 = 509_6020;
    const DPS: [u64; 3] = [509_6021, 509_6022, 509_6023];
    const FIRST_PRIEST: u64 = 509_6025;
    const SECOND_PRIEST: u64 = 509_6024;
    stage(&realm, LEADER, HUMAN, WARRIOR);
    for guid in DPS {
        stage(&realm, guid, HUMAN, MAGE);
    }
    stage_group(&realm, LEADER, &DPS);
    let group = group_of(&realm, LEADER).unwrap();
    // The higher guid waits longer, so the wait decides and not the guid.
    for priest in [FIRST_PRIEST, SECOND_PRIEST] {
        stage(&realm, priest, HUMAN, PRIEST);
        join_alone(&realm, priest, PRIEST, DEADMINES);
    }

    let queued = heard(&realm, || {
        join_party(
            &realm,
            LEADER,
            DEADMINES,
            &[
                (LEADER, WARRIOR),
                (DPS[0], MAGE),
                (DPS[1], MAGE),
                (DPS[2], MAGE),
            ],
        )
    });

    assert_eq!(group_of(&realm, FIRST_PRIEST), Some(group));
    assert_eq!(group_of(&realm, SECOND_PRIEST), None);
    assert_eq!(
        queued.with_lists(LEADER),
        [
            Event::Queue(DEADMINES, JOINED_QUEUE),
            Event::MemberAdded(FIRST_PRIEST),
            Event::List,
            Event::Complete,
            Event::Queue(0, NONE),
        ]
    );
    assert_eq!(
        queued.stone(FIRST_PRIEST),
        [Event::Complete, Event::Queue(0, NONE)]
    );
    assert!(queued.stone(SECOND_PRIEST).is_empty());
    assert_eq!(
        seekers(&realm),
        BTreeMap::from([(SECOND_PRIEST, alone(DEADMINES, ALLIANCE, PRIEST))])
    );
}

/// Criterion 5: a team or an area apart, Seekers never meet, neither to form a party nor to fill one.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn seekers_of_another_team_or_area_never_meet() {
    let realm = start("stone-match-buckets");
    let mages: Vec<u64> = (509_6030..509_6034).collect();
    const HORDE_MAGE: u64 = 509_6034;
    const STOCKADE_MAGE: u64 = 509_6035;
    const FIFTH_MAGE: u64 = 509_6036;
    const HORDE_PRIEST: u64 = 509_6037;
    const STOCKADE_PRIEST: u64 = 509_6038;
    for &guid in &mages {
        stage(&realm, guid, HUMAN, MAGE);
        join_alone(&realm, guid, MAGE, DEADMINES);
    }
    stage(&realm, HORDE_MAGE, ORC, MAGE);
    stone_join(&realm, HORDE_MAGE, DEADMINES, &[(HORDE_MAGE, ORC, MAGE)]);
    stage(&realm, STOCKADE_MAGE, HUMAN, MAGE);
    join_alone(&realm, STOCKADE_MAGE, MAGE, STOCKADE);
    assert!(
        queued_parties(&realm).is_empty(),
        "four Seekers per bucket form nothing"
    );

    stage(&realm, FIFTH_MAGE, HUMAN, MAGE);
    join_alone(&realm, FIFTH_MAGE, MAGE, DEADMINES);
    let group = group_of(&realm, mages[0]).expect("five Alliance Seekers form a party");
    assert_eq!(members(&realm, group), mages[..3]);

    // The party has its tank and healer open; a priest a team or an area away does not take one.
    stage(&realm, HORDE_PRIEST, ORC, PRIEST);
    stone_join(
        &realm,
        HORDE_PRIEST,
        DEADMINES,
        &[(HORDE_PRIEST, ORC, PRIEST)],
    );
    stage(&realm, STOCKADE_PRIEST, HUMAN, PRIEST);
    join_alone(&realm, STOCKADE_PRIEST, PRIEST, STOCKADE);

    assert_eq!(members(&realm, group), mages[..3]);
    let waiting: BTreeMap<u64, SeekerRow> = seekers(&realm)
        .into_iter()
        .filter(|(_, row)| row.3 == 0)
        .collect();
    assert_eq!(
        waiting,
        BTreeMap::from([
            (mages[3], alone(DEADMINES, ALLIANCE, MAGE)),
            (HORDE_MAGE, alone(DEADMINES, HORDE, MAGE)),
            (STOCKADE_MAGE, alone(STOCKADE, ALLIANCE, MAGE)),
            (FIFTH_MAGE, alone(DEADMINES, ALLIANCE, MAGE)),
            (HORDE_PRIEST, alone(DEADMINES, HORDE, PRIEST)),
            (STOCKADE_PRIEST, alone(STOCKADE, ALLIANCE, PRIEST)),
        ])
    );
}

/// Criterion 6: a Seeker whose claim expired before the lease reaper closed it is never added, and
/// the pass that meets it drops its row without a packet.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_seeker_without_a_live_claim_is_dropped_by_the_pass() {
    let realm = start("stone-match-claims");
    // Without the lease reaper only the pass can notice the expired claim.
    realm.assert_sql("DELETE FROM game_gateway_lease_reaper_schedule");
    let mages: Vec<u64> = (509_6040..509_6046).collect();
    let expired = mages[1];
    for &guid in &mages {
        stage(&realm, guid, HUMAN, MAGE);
    }
    for &guid in &mages[..4] {
        join_alone(&realm, guid, MAGE, DEADMINES);
    }
    realm.assert_sql(&format!(
        "UPDATE game_account_claim SET expires_micros = 0 WHERE account_id = {expired}"
    ));

    let fifth = heard(&realm, || join_alone(&realm, mages[4], MAGE, DEADMINES));
    assert!(
        queued_parties(&realm).is_empty(),
        "four live Seekers form nothing"
    );
    assert!(!seekers(&realm).contains_key(&expired));
    assert!(fifth.stone(expired).is_empty());

    join_alone(&realm, mages[5], MAGE, DEADMINES);
    let group = group_of(&realm, mages[0]).expect("five live Seekers form a party");
    assert_eq!(members(&realm, group), [mages[0], mages[2], mages[3]]);
    assert_eq!(group_of(&realm, expired), None);
}

/// Criterion 7, leave and deletion with the leader staying: the leaver hears NONE, the rest hear
/// PARTY_MEMBER_LEFT_LFG, and the party stays queued and fills the role the leaver freed.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_member_who_leaves_keeps_the_party_queued_and_frees_a_role() {
    for plane in PLANES {
        let realm = plane.start("stone-hook-leave");
        const LEADER: u64 = 509_6050;
        const LEAVER: u64 = 509_6051;
        const DPS: [u64; 2] = [509_6052, 509_6053];
        const WAITING: u64 = 509_6054;
        stage(&realm, LEADER, HUMAN, WARRIOR);
        for guid in [LEAVER, DPS[0], DPS[1], WAITING] {
            stage(&realm, guid, HUMAN, MAGE);
        }
        stage_group(&realm, LEADER, &[LEAVER, DPS[0], DPS[1]]);
        let group = group_of(&realm, LEADER).unwrap();
        // A warrior and three mages leave only the healer open.
        join_party(
            &realm,
            LEADER,
            DEADMINES,
            &[
                (LEADER, WARRIOR),
                (LEAVER, MAGE),
                (DPS[0], MAGE),
                (DPS[1], MAGE),
            ],
        );
        join_alone(&realm, WAITING, MAGE, DEADMINES);
        assert_eq!(
            group_of(&realm, WAITING),
            None,
            "{plane:?}: no damage role is open"
        );

        let left = heard(&realm, || plane.leave(&realm, LEAVER));
        assert_eq!(left.stone(LEAVER), [Event::Queue(0, NONE)], "{plane:?}");
        const LEFT_LFG: Event = Event::Queue(DEADMINES, PARTY_MEMBER_LEFT_LFG);
        assert_eq!(
            left.stone(LEADER),
            [LEFT_LFG, Event::MemberAdded(WAITING)],
            "{plane:?}"
        );
        assert_eq!(left.stone(WAITING), [], "{plane:?}");
        assert_eq!(
            members(&realm, group),
            [LEADER, DPS[0], DPS[1], WAITING],
            "{plane:?}"
        );

        let deleted = heard(&realm, || plane.delete(&realm, DPS[0]));
        assert_eq!(deleted.stone(DPS[0]), [Event::Queue(0, NONE)], "{plane:?}");
        for rest in [LEADER, DPS[1], WAITING] {
            assert_eq!(deleted.stone(rest), [LEFT_LFG], "{plane:?}");
        }
        assert_eq!(queued_parties(&realm), [group], "{plane:?}");
        assert_eq!(
            seekers(&realm),
            BTreeMap::from([
                (LEADER, in_party(group, WARRIOR)),
                (DPS[1], in_party(group, MAGE)),
                (WAITING, in_party(group, MAGE)),
            ]),
            "{plane:?}"
        );
    }
}

/// Criterion 7, leave with the leader changing: the party leaves the queue with LEAVE_QUEUE.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_leader_who_leaves_takes_the_party_out_of_the_queue() {
    for plane in PLANES {
        let realm = plane.start("stone-hook-leader-leaves");
        const LEADER: u64 = 509_6060;
        const REST: [u64; 2] = [509_6061, 509_6062];
        for guid in [LEADER].into_iter().chain(REST) {
            stage(&realm, guid, HUMAN, MAGE);
        }
        stage_group(&realm, LEADER, &REST);
        join_party(
            &realm,
            LEADER,
            DEADMINES,
            &[(LEADER, MAGE), (REST[0], MAGE), (REST[1], MAGE)],
        );

        let left = heard(&realm, || plane.leave(&realm, LEADER));
        assert_eq!(left.stone(LEADER), [Event::Queue(0, NONE)], "{plane:?}");
        for rest in REST {
            assert_eq!(
                left.stone(rest),
                [Event::Queue(0, LEAVE_QUEUE)],
                "{plane:?}"
            );
        }
        assert!(queued_parties(&realm).is_empty(), "{plane:?}");
        assert!(seekers(&realm).is_empty(), "{plane:?}");
    }
}

/// Criterion 7, kick from a party that survives: the party leaves the queue, and the kicked
/// Character waits alone with its class and team.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_kicked_member_waits_alone_and_the_party_leaves_the_queue() {
    for plane in PLANES {
        let realm = plane.start("stone-hook-kick");
        const LEADER: u64 = 509_6065;
        const KICKED: u64 = 509_6066;
        const STAYS: u64 = 509_6067;
        stage(&realm, LEADER, HUMAN, WARRIOR);
        stage(&realm, KICKED, HUMAN, PRIEST);
        stage(&realm, STAYS, HUMAN, MAGE);
        stage_group(&realm, LEADER, &[KICKED, STAYS]);
        join_party(
            &realm,
            LEADER,
            DEADMINES,
            &[(LEADER, WARRIOR), (KICKED, PRIEST), (STAYS, MAGE)],
        );

        let kicked = heard(&realm, || plane.kick(&realm, LEADER, KICKED));
        for rest in [LEADER, STAYS] {
            assert_eq!(
                kicked.stone(rest),
                [
                    Event::Queue(0, PARTY_MEMBER_REMOVED_PARTY_REMOVED),
                    Event::Queue(0, LEAVE_QUEUE),
                ],
                "{plane:?}"
            );
        }
        assert_eq!(
            kicked.stone(KICKED),
            [
                Event::Queue(DEADMINES, LOOKING_FOR_NEW_PARTY_IN_QUEUE),
                Event::Queue(DEADMINES, JOINED_QUEUE),
            ],
            "{plane:?}"
        );
        assert!(queued_parties(&realm).is_empty(), "{plane:?}");
        assert_eq!(
            seekers(&realm),
            BTreeMap::from([(KICKED, alone(DEADMINES, ALLIANCE, PRIEST))]),
            "{plane:?}"
        );
    }
}

/// Criterion 7, disband by a leave or a kick from two members: both hear NONE once, and nobody
/// waits on alone.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_disband_answers_every_former_member_with_none() {
    for plane in PLANES {
        let realm = plane.start("stone-hook-disband");
        const LEFT_PAIR: [u64; 2] = [509_6070, 509_6071];
        const KICKED_PAIR: [u64; 2] = [509_6072, 509_6073];
        for [leader, member] in [LEFT_PAIR, KICKED_PAIR] {
            stage(&realm, leader, HUMAN, WARRIOR);
            stage(&realm, member, HUMAN, PRIEST);
            stage_group(&realm, leader, &[member]);
            join_party(
                &realm,
                leader,
                DEADMINES,
                &[(leader, WARRIOR), (member, PRIEST)],
            );
        }

        let left = heard(&realm, || plane.leave(&realm, LEFT_PAIR[1]));
        let kicked = heard(&realm, || {
            plane.kick(&realm, KICKED_PAIR[0], KICKED_PAIR[1])
        });
        for (heard, pair) in [(left, LEFT_PAIR), (kicked, KICKED_PAIR)] {
            for guid in pair {
                assert_eq!(heard.stone(guid), [Event::Queue(0, NONE)], "{plane:?}");
            }
        }
        assert!(queued_parties(&realm).is_empty(), "{plane:?}");
        assert!(seekers(&realm).is_empty(), "{plane:?}");
    }
}

/// Criterion 7, accept: a solo Seeker who joins any party leaves the solo queue, told LEAVE_QUEUE
/// unless the party is queued for its area. A queued party gains it as a Seeker with its class and
/// completes at five members.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn accepting_an_invite_moves_a_solo_seeker_into_the_party() {
    for plane in PLANES {
        let realm = plane.start("stone-hook-accept");
        const LEADER: u64 = 509_6075;
        const HEALER: u64 = 509_6076;
        const DPS: u64 = 509_6077;
        /// Waits at the Stockade.
        const ELSEWHERE: u64 = 509_6078;
        /// Waits at the Deadmines with a class the Gateway could not read, so no role fits it.
        const UNREAD: u64 = 509_6079;
        /// A solo Seeker whose own invite forms a new party.
        const INVITER: u64 = 509_6080;
        const INVITED: u64 = 509_6081;
        stage(&realm, LEADER, HUMAN, WARRIOR);
        stage(&realm, HEALER, HUMAN, PRIEST);
        for guid in [DPS, ELSEWHERE, UNREAD, INVITER, INVITED] {
            stage(&realm, guid, HUMAN, MAGE);
        }
        join_alone(&realm, ELSEWHERE, MAGE, STOCKADE);
        join_alone(&realm, UNREAD, 0, DEADMINES);
        join_alone(&realm, INVITER, MAGE, STOCKADE);
        stage_group(&realm, LEADER, &[HEALER, DPS]);
        let group = group_of(&realm, LEADER).unwrap();
        join_party(
            &realm,
            LEADER,
            DEADMINES,
            &[(LEADER, WARRIOR), (HEALER, PRIEST), (DPS, MAGE)],
        );

        plane.invite(&realm, LEADER, ELSEWHERE);
        let other_area = heard(&realm, || plane.accept(&realm, ELSEWHERE, MAGE));
        assert_eq!(
            other_area.stone(ELSEWHERE),
            [Event::Queue(0, LEAVE_QUEUE)],
            "{plane:?}"
        );
        assert_eq!(
            seekers(&realm).get(&ELSEWHERE),
            Some(&in_party(group, MAGE)),
            "{plane:?}"
        );

        plane.invite(&realm, LEADER, UNREAD);
        let same_area = heard(&realm, || plane.accept(&realm, UNREAD, MAGE));
        for guid in [LEADER, HEALER, DPS, ELSEWHERE, UNREAD] {
            assert_eq!(
                same_area.stone(guid),
                [Event::Complete, Event::Queue(0, NONE)],
                "{plane:?}: the fifth member completes the party"
            );
        }
        assert!(queued_parties(&realm).is_empty(), "{plane:?}");

        plane.invite(&realm, INVITER, INVITED);
        let formed = heard(&realm, || plane.accept(&realm, INVITED, MAGE));
        assert_eq!(
            formed.stone(INVITER),
            [Event::Queue(0, LEAVE_QUEUE)],
            "{plane:?}: the inviter joins the party its invite formed"
        );
        assert_eq!(formed.stone(INVITED), [], "{plane:?}");
        assert!(seekers(&realm).is_empty(), "{plane:?}");
    }
}

/// Criterion 7, raid convert: every member hears LEAVE_QUEUE before the raid list. The convert has
/// no single-database reducer; the Gateway sends it to the party authority on both realm shapes.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_queued_party_that_converts_to_a_raid_leaves_the_queue() {
    let realm = start("stone-hook-raid-convert");
    const LEADER: u64 = 509_6085;
    const MEMBER: u64 = 509_6086;
    stage(&realm, LEADER, HUMAN, WARRIOR);
    stage(&realm, MEMBER, HUMAN, PRIEST);
    stage_group(&realm, LEADER, &[MEMBER]);
    join_party(
        &realm,
        LEADER,
        DEADMINES,
        &[(LEADER, WARRIOR), (MEMBER, PRIEST)],
    );

    let converted = heard(&realm, || group_op(&realm, RAID_CONVERT, LEADER, 0, 0, 0));
    for guid in [LEADER, MEMBER] {
        assert_eq!(
            converted.with_lists(guid),
            [Event::Queue(0, LEAVE_QUEUE), Event::List]
        );
    }
    assert!(queued_parties(&realm).is_empty());
    assert!(seekers(&realm).is_empty());
}

/// Criterion 8: a kicked Character re-queued alone fills a second queued party's open healer in the
/// same transaction.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_kicked_member_joins_a_second_queued_party_at_once() {
    let realm = start("stone-match-kick-requeue");
    const FIRST_LEADER: u64 = 509_6090;
    const KICKED: u64 = 509_6091;
    const FIRST_DPS: u64 = 509_6092;
    const SECOND_LEADER: u64 = 509_6093;
    const SECOND_DPS: u64 = 509_6094;
    stage(&realm, FIRST_LEADER, HUMAN, WARRIOR);
    stage(&realm, KICKED, HUMAN, PRIEST);
    stage(&realm, SECOND_LEADER, HUMAN, WARRIOR);
    for guid in [FIRST_DPS, SECOND_DPS] {
        stage(&realm, guid, HUMAN, MAGE);
    }
    stage_group(&realm, FIRST_LEADER, &[KICKED, FIRST_DPS]);
    stage_group(&realm, SECOND_LEADER, &[SECOND_DPS]);
    let second = group_of(&realm, SECOND_LEADER).unwrap();
    join_party(
        &realm,
        FIRST_LEADER,
        DEADMINES,
        &[(FIRST_LEADER, WARRIOR), (KICKED, PRIEST), (FIRST_DPS, MAGE)],
    );
    join_party(
        &realm,
        SECOND_LEADER,
        DEADMINES,
        &[(SECOND_LEADER, WARRIOR), (SECOND_DPS, MAGE)],
    );

    let kicked = heard(&realm, || {
        group_op(&realm, UNINVITE, FIRST_LEADER, KICKED, 0, 0)
    });

    assert_eq!(group_of(&realm, KICKED), Some(second));
    assert_eq!(
        kicked.stone(KICKED),
        [
            Event::Queue(DEADMINES, LOOKING_FOR_NEW_PARTY_IN_QUEUE),
            Event::Queue(DEADMINES, JOINED_QUEUE),
        ]
    );
    for guid in [SECOND_LEADER, SECOND_DPS] {
        assert_eq!(kicked.stone(guid), [Event::MemberAdded(KICKED)]);
    }
    assert_eq!(queued_parties(&realm), [second]);
    assert_eq!(seekers(&realm)[&KICKED], in_party(second, PRIEST));
    assert!(
        revision(&realm, second) > Some(1),
        "a staged party starts at the implicit revision 1"
    );
}

/// Criterion 9: the pass advances the Roster Revision of each party it forms and of each party a
/// Stone Add changes.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn every_party_the_pass_forms_or_fills_has_a_higher_roster_revision() {
    let realm = start("stone-match-revisions");
    let mages: Vec<u64> = (509_6000..509_6005).collect();
    const PRIEST_SEEKER: u64 = 509_6005;
    for &guid in &mages {
        stage(&realm, guid, HUMAN, MAGE);
        join_alone(&realm, guid, MAGE, DEADMINES);
    }
    let group = group_of(&realm, mages[0]).unwrap();
    let formed = revision(&realm, group).expect("a formed party has a Roster Revision");
    assert_eq!(formed, 2, "formed at 1, then one Stone Add");

    stage(&realm, PRIEST_SEEKER, HUMAN, PRIEST);
    join_alone(&realm, PRIEST_SEEKER, PRIEST, DEADMINES);
    assert_eq!(group_of(&realm, PRIEST_SEEKER), Some(group));
    assert_eq!(revision(&realm, group), Some(formed + 1));
}

/// Criterion 10: a due reminder sends IN_PROGRESS to every member once and is due again five
/// minutes after it fired.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_due_reminder_tells_every_member_once() {
    let realm = start("stone-match-reminder");
    const LEADER: u64 = 509_6000;
    const MEMBER: u64 = 509_6001;
    stage(&realm, LEADER, HUMAN, WARRIOR);
    stage(&realm, MEMBER, HUMAN, PRIEST);
    stage_group(&realm, LEADER, &[MEMBER]);
    let group = group_of(&realm, LEADER).unwrap();
    join_party(
        &realm,
        LEADER,
        DEADMINES,
        &[(LEADER, WARRIOR), (MEMBER, PRIEST)],
    );
    let reminders = |realm: &Standalone| {
        events(realm)
            .into_iter()
            .filter(|(_, _, event)| *event == Event::InProgress)
            .map(|(_, recipient, _)| recipient)
            .collect::<Vec<u64>>()
    };
    let next_due = |realm: &Standalone| -> i64 {
        let rows = realm.query_rows("SELECT next_reminder_at FROM game_meeting_stone_party");
        let value = &rows[0]["next_reminder_at"];
        spacetimedb::Timestamp::parse_from_rfc3339(value)
            .unwrap_or_else(|error| panic!("invalid durable timestamp {value:?}: {error}"))
            .to_micros_since_unix_epoch()
    };
    let queued_due = next_due(&realm);

    realm.assert_call(
        "debug_backdate_meeting_stone_party",
        &[&group.to_string(), "300"],
    );
    let backdated_due = next_due(&realm);
    assert_eq!(backdated_due, queued_due - 300_000_000);
    assert!(
        support::poll_until(Duration::from_secs(20), || reminders(&realm).len() == 2),
        "the reminder tick never fired"
    );
    let first_due = next_due(&realm);
    // The tick fires within its 5 s cadence of the reminder falling due.
    let fired_after = first_due - 300_000_000 - backdated_due;
    assert!(
        (0..15_000_000).contains(&fired_after),
        "the next reminder is five minutes after the one that fired, {fired_after} us late"
    );

    std::thread::sleep(Duration::from_secs(6));
    let mut recipients = reminders(&realm);
    recipients.sort_unstable();
    assert_eq!(recipients, [LEADER, MEMBER], "each member hears it once");
}

/// Criterion 11: a stone-formed party starts with the same `game_group` defaults as an invited one.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_stone_formed_party_has_the_defaults_of_an_invited_one() {
    let realm = start("stone-match-defaults");
    let mages: Vec<u64> = (509_6000..509_6005).collect();
    const INVITER: u64 = 509_6010;
    const INVITED: u64 = 509_6011;
    for &guid in &mages {
        stage(&realm, guid, HUMAN, MAGE);
        join_alone(&realm, guid, MAGE, DEADMINES);
    }
    stage(&realm, INVITER, HUMAN, MAGE);
    stage(&realm, INVITED, HUMAN, MAGE);
    group_op(&realm, INVITE, INVITER, INVITED, 0, 0);
    group_op(&realm, ACCEPT, INVITED, 0, MAGE, HUMAN);

    let defaults = |guid: u64| {
        let group = group_of(&realm, guid).unwrap();
        let mut row = realm.query_rows(&format!(
            "SELECT * FROM game_group WHERE group_id = {group}"
        ))[0]
            .clone();
        row.remove("group_id");
        row.remove("leader_guid");
        let slots: Vec<String> = realm
            .query_rows(&format!(
                "SELECT raid_slot FROM game_group_member WHERE group_id = {group}"
            ))
            .into_iter()
            .map(|member| member["raid_slot"].clone())
            .collect();
        (row, slots.into_iter().all(|slot| slot == "0"))
    };
    let (stone, stone_slots) = defaults(mages[0]);
    assert_eq!(stone, defaults(INVITER).0);
    assert!(stone_slots);
    assert_eq!(stone["loot_method"], "3", "group loot");
    assert_eq!(stone["loot_threshold"], "2", "at Uncommon");
    assert_eq!(stone["group_type"], "0", "a Party");
}

/// Criterion 12: a Character counting down in an instance its new stone party owns stays.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_stone_add_into_the_party_that_owns_the_instance_cancels_the_countdown() {
    let realm = start("stone-match-instance");
    // The Characters `debug_stage_instance_removal_fixture` stages; all three stand in its instance.
    const ALPHA: u64 = 509_295_001;
    const BRAVO: u64 = 509_295_002;
    const CHARLIE: u64 = 509_295_003;
    // Party ops without a token run before the claims exist, as in the Instance Removal tests.
    let tokenless = |op: &str, actor: u64, target: u64| {
        realm.assert_call(
            "realm_group_op",
            &[
                op,
                &support::actor(&actor.to_string()),
                &target.to_string(),
                "0",
                "0",
                "0",
            ],
        );
    };
    for guid in [BRAVO, CHARLIE] {
        tokenless(INVITE, ALPHA, guid);
        tokenless(ACCEPT, guid, 0);
    }
    let group = group_of(&realm, ALPHA).unwrap();
    realm.assert_call(
        "debug_stage_instance_removal_fixture",
        &[&group.to_string()],
    );
    tokenless(LEAVE, CHARLIE, 0);
    let countdowns = |realm: &Standalone| {
        realm.assert_call("debug_hold_instance_removals", &[]);
        realm
            .query_rows("SELECT character_guid FROM game_instance_removal")
            .into_iter()
            .map(|row| row["character_guid"].parse().unwrap())
            .collect::<Vec<u64>>()
    };
    assert_eq!(countdowns(&realm), [CHARLIE], "the leaver counts down");

    for guid in [ALPHA, CHARLIE] {
        realm.assert_sql(&format!(
            "INSERT INTO game_account_claim (account_id, generation, request_nonce, \
             character_guid, expires_micros, closed) VALUES ({guid}, 1, {guid}, {guid}, \
             4102444800000000, false)"
        ));
    }
    join_party(
        &realm,
        ALPHA,
        DEADMINES,
        &[(ALPHA, WARRIOR), (BRAVO, PRIEST)],
    );
    join_alone(&realm, CHARLIE, MAGE, DEADMINES);

    assert_eq!(group_of(&realm, CHARLIE), Some(group));
    assert!(
        countdowns(&realm).is_empty(),
        "the Stone Add cancels the countdown"
    );
}
