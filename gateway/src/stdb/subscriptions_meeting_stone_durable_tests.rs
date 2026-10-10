//! Meeting Stone Seekers on two World Shards meet in one queue on Realm-core. Each Seeker uses its
//! stone through the real Coordinator, the stone forms one Party, the Roster Revision Relay carries
//! it to both World Shard mirrors, and the group event relay gives each session its stone events
//! in the order the Module inserted them.

use super::roster_relay_durable_tests::{mirrors_follow_realm_core, TopologyEnv};
use super::*;
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{module_bytes, poll_until, Standalone, POLL_TIMEOUT};
use crate::world::{
    dispatch_meeting_stone_action, MeetingStoneActionOutcome, MeetingStoneActionStore,
    MeetingStonePlayer, Outbound, WorldSessionToken,
};
use lyracore_shared::meeting_stone::queue_status::{JOINED_QUEUE, NONE};
use std::sync::mpsc::Receiver;
use wow_world_messages::vanilla::opcodes::ClientOpcodeMessage;
use wow_world_messages::vanilla::CMSG_MEETINGSTONE_JOIN;
use wow_world_messages::Guid;

const INSTANCES: &str = "meeting-stone-instances";
const REALM: &str = "meeting-stone-realm";

// Fixture ids sit in 509_6000-509_6099.
const STONE_ENTRY: &str = "5096000";
const WORLD_STONE: u64 = 509_6001;
const INSTANCES_STONE: u64 = 509_6002;
/// The Deadmines (AreaTable 1581).
const DEADMINES: u32 = 1581;
const STONE_AT: [&str; 3] = ["-11208", "1672", "24"];
/// The default World Shard serves map 0; the shard map sends map 36 to the Instance Pool.
const WORLD_MAP: &str = "0";
const INSTANCES_MAP: &str = "36";

const HUMAN: &str = "1";
const WARRIOR: &str = "1";
const PRIEST: &str = "5";
const MAGE: &str = "8";
const ROGUE: &str = "4";
const HUNTER: &str = "3";

/// In queue order. The Warrior waits longest, so it leads; the Priest heals; the rest deal damage.
const TANK: u64 = 509_6010;
const HEALER: u64 = 509_6011;
const MAGE_SEEKER: u64 = 509_6012;
const ROGUE_SEEKER: u64 = 509_6013;
const HUNTER_SEEKER: u64 = 509_6014;
const SEEKERS: [u64; 5] = [TANK, HEALER, MAGE_SEEKER, ROGUE_SEEKER, HUNTER_SEEKER];

/// The World Session Token the fixture claims carry: each Seeker owns the Account of its guid.
fn token(guid: u64) -> WorldSessionToken {
    WorldSessionToken {
        account_id: guid,
        generation: 1,
        request_nonce: u128::from(guid),
    }
}

/// A Seeker at the stone on `database`, with its claim on that database and on Realm-core.
fn stage_seeker(standalone: &Standalone, database: &str, map: &str, guid: u64, class: &str) {
    let guid = guid.to_string();
    standalone.assert_call_database(
        database,
        "debug_stage_meeting_stone_character",
        &[
            &guid,
            &guid,
            HUMAN,
            class,
            "18",
            map,
            STONE_AT[0],
            STONE_AT[1],
            STONE_AT[2],
        ],
    );
    standalone.assert_call_database(REALM, "debug_stage_meeting_stone_claim", &[&guid, &guid]);
}

fn stage_stone(standalone: &Standalone, database: &str, map: &str, go_guid: u64) {
    standalone.assert_call_database(
        database,
        "debug_stage_meeting_stone",
        &[
            STONE_ENTRY,
            &go_guid.to_string(),
            "10",
            "30",
            &DEADMINES.to_string(),
            map,
            STONE_AT[0],
            STONE_AT[1],
            STONE_AT[2],
        ],
    );
}

fn use_stone(session: &Coordinator, guid: u64, stone: u64) -> Vec<Outbound> {
    send(
        session,
        guid,
        ClientOpcodeMessage::CMSG_MEETINGSTONE_JOIN(CMSG_MEETINGSTONE_JOIN {
            guid: Guid::new(stone),
        }),
    )
}

fn send(session: &Coordinator, guid: u64, msg: ClientOpcodeMessage) -> Vec<Outbound> {
    let player = MeetingStonePlayer {
        account_id: guid,
        self_guid: Some(guid),
    };
    match dispatch_meeting_stone_action(session, player, msg).unwrap() {
        MeetingStoneActionOutcome::Handled { outbound } => outbound,
        MeetingStoneActionOutcome::PassThrough(_) => panic!("the meeting stone seam passed it on"),
    }
}

/// The meeting stone messages in `outbound`, running each relay job the way the session writer
/// does.
fn stone_messages(outbound: Vec<Outbound>) -> Vec<ServerOpcodeMessage> {
    let mut messages = Vec::new();
    for item in outbound {
        match item {
            Outbound::One(msg) => messages.push(msg),
            Outbound::Batch(batch) => messages.extend(batch),
            Outbound::Job(job) => messages.extend(stone_messages(job())),
            Outbound::Raw { .. } => {}
        }
    }
    messages.retain(|msg| {
        matches!(
            msg,
            ServerOpcodeMessage::SMSG_MEETINGSTONE_SETQUEUE(_)
                | ServerOpcodeMessage::SMSG_MEETINGSTONE_MEMBER_ADDED(_)
                | ServerOpcodeMessage::SMSG_MEETINGSTONE_IN_PROGRESS
                | ServerOpcodeMessage::SMSG_MEETINGSTONE_COMPLETE
        )
    });
    messages
}

fn set_queue(area_id: u32, status: u8) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_MEETINGSTONE_SETQUEUE(
        codec::build_meetingstone_setqueue(area_id, status).unwrap(),
    )
}

fn member_added(guid: u64) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_MEETINGSTONE_MEMBER_ADDED(codec::build_meetingstone_member_added(
        guid,
    ))
}

/// One Seeker's World Session as the relay sees it: its registration and what reached its queue.
struct Session {
    guid: u64,
    _registration: PlayerSubscriptions,
    rx: Receiver<Outbound>,
    heard: Vec<ServerOpcodeMessage>,
}

impl Session {
    fn open(view: Arc<WorldView>, guid: u64) -> Self {
        let (tx, rx, _) = crate::world::session_channel();
        let arrival = codec::EntityView::default();
        Self {
            guid,
            _registration: PlayerSubscriptions::registered_for_test(view, guid, &arrival, tx),
            rx,
            heard: Vec::new(),
        }
    }

    fn drain(&mut self) {
        let queued: Vec<Outbound> = self.rx.try_iter().collect();
        self.heard.extend(stone_messages(queued));
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
// One topology stays alive from staging to the filled Party to prove one realm-wide scenario.
#[allow(clippy::too_many_lines)]
fn seekers_on_two_world_shards_form_one_party_that_reaches_every_mirror() {
    let mut standalone = Standalone::start_persistent("meeting-stone-world");
    standalone.publish_module();
    for database in [INSTANCES, REALM] {
        standalone.publish_named_module_bytes(database, module_bytes());
    }
    standalone.assert_call("claim_operator", &[]);
    for database in [INSTANCES, REALM] {
        standalone.assert_call_database(database, "claim_operator", &[]);
    }
    let world_db = standalone.shard_name().to_string();
    stage_stone(&standalone, &world_db, WORLD_MAP, WORLD_STONE);
    stage_stone(&standalone, INSTANCES, INSTANCES_MAP, INSTANCES_STONE);
    for (database, map, guid, class) in [
        (world_db.as_str(), WORLD_MAP, TANK, WARRIOR),
        (INSTANCES, INSTANCES_MAP, HEALER, PRIEST),
        (world_db.as_str(), WORLD_MAP, MAGE_SEEKER, MAGE),
        (world_db.as_str(), WORLD_MAP, ROGUE_SEEKER, ROGUE),
        (INSTANCES, INSTANCES_MAP, HUNTER_SEEKER, HUNTER),
    ] {
        stage_seeker(&standalone, database, map, guid, class);
    }

    let _topology = TopologyEnv::install(&format!("36:*={INSTANCES}"), REALM);
    let cfg = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: standalone.server().into(),
        module_name: standalone.shard_name().into(),
        coordinator_token: Some(standalone.owner_token()),
        gateway_id: "meeting-stone-test".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(1),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let gateway = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    gateway.spawn_roster_revision_relay();
    let realm = gateway.realm_core().unwrap();
    let instances = gateway.shard_handle(INSTANCES).unwrap();
    let shards = gateway.world_shards();
    assert_eq!(shards.len(), 2, "the default World Shard and {INSTANCES}");
    assert!(poll_until(POLL_TIMEOUT, || {
        gateway.meeting_stone_area(WORLD_STONE).unwrap() == Some(DEADMINES)
            && instances.meeting_stone_area(INSTANCES_STONE).unwrap() == Some(DEADMINES)
            && SEEKERS.iter().all(|&guid| {
                realm
                    .0
                    .coord()
                    .conn
                    .db
                    .game_account_claim()
                    .account_id()
                    .find(&guid)
                    .is_some_and(|claim| claim.character_guid == guid)
            })
    }));
    let bind = |home: &Coordinator, guid: u64| {
        let _runtime = runtime.enter();
        home.bind_session(token(guid)).unwrap()
    };
    let mut sessions: Vec<Session> = SEEKERS
        .iter()
        .map(|&guid| Session::open(gateway.world_view(), guid))
        .collect();

    // The tank queues alone on the default World Shard.
    assert!(use_stone(&bind(&gateway, TANK), TANK, WORLD_STONE).is_empty());
    assert!(poll_until(POLL_TIMEOUT, || realm
        .queued_area(TANK)
        .unwrap()
        == Some(DEADMINES)));

    // It crosses into the Instance Pool, which holds it now. Its Account Claim stays on
    // Realm-core, so it stays queued, and the status query on arrival says so.
    stage_seeker(&standalone, INSTANCES, INSTANCES_MAP, TANK, WARRIOR);
    standalone.assert_call("debug_delete_character", &[&TANK.to_string()]);
    assert!(poll_until(POLL_TIMEOUT, || {
        gateway.character_by_guid(TANK).unwrap().is_none()
            && instances.character_by_guid(TANK).unwrap().is_some()
    }));
    let arrived = stone_messages(send(
        &bind(&instances, TANK),
        TANK,
        ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO,
    ));
    assert_eq!(arrived, [set_queue(DEADMINES, JOINED_QUEUE)]);

    // Three Seekers wait on the Instance Pool now and two on the default World Shard. The fifth
    // JOIN forms the Party and fills it in the same transaction.
    for (home, guid, stone) in [
        (&instances, HEALER, INSTANCES_STONE),
        (&gateway, MAGE_SEEKER, WORLD_STONE),
        (&gateway, ROGUE_SEEKER, WORLD_STONE),
        (&instances, HUNTER_SEEKER, INSTANCES_STONE),
    ] {
        assert!(use_stone(&bind(home, guid), guid, stone).is_empty());
    }

    assert!(
        poll_until(POLL_TIMEOUT, || realm
            .group_roster(TANK)
            .is_some_and(|roster| roster.member_guids() == SEEKERS)),
        "Realm-core holds {:?}",
        realm.group_roster(TANK).map(|roster| roster.member_guids())
    );
    let group_id = realm.group_roster(TANK).unwrap().group_id;
    assert!(poll_until(POLL_TIMEOUT, || realm
        .0
        .coord()
        .conn
        .db
        .game_meeting_stone_seeker()
        .count()
        == 0));
    assert!(standalone
        .query_database_rows(REALM, "SELECT * FROM game_meeting_stone_party")
        .is_empty());

    // No party op ran for these Characters, so only the Roster Revision Relay moved the mirrors.
    assert!(
        poll_until(POLL_TIMEOUT, || mirrors_follow_realm_core(
            &realm, &shards, group_id, TANK
        )),
        "Realm-core revision {:?}, mirrors {:?}",
        realm.held_roster_revision(group_id),
        shards
            .iter()
            .map(|(name, shard)| (
                name,
                shard.held_roster_revision(group_id),
                shard.group_roster(TANK).map(|roster| roster.member_guids())
            ))
            .collect::<Vec<_>>()
    );

    let joined = || set_queue(DEADMINES, JOINED_QUEUE);
    let closing = || {
        [
            ServerOpcodeMessage::SMSG_MEETINGSTONE_COMPLETE,
            set_queue(0, NONE),
        ]
    };
    let expected: Vec<(u64, Vec<ServerOpcodeMessage>)> = vec![
        (
            TANK,
            [
                joined(),
                member_added(HEALER),
                joined(),
                member_added(MAGE_SEEKER),
                member_added(ROGUE_SEEKER),
                member_added(HUNTER_SEEKER),
            ]
            .into_iter()
            .chain(closing())
            .collect(),
        ),
        (
            HEALER,
            [
                joined(),
                joined(),
                member_added(MAGE_SEEKER),
                member_added(ROGUE_SEEKER),
                member_added(HUNTER_SEEKER),
            ]
            .into_iter()
            .chain(closing())
            .collect(),
        ),
        (
            MAGE_SEEKER,
            [
                joined(),
                member_added(ROGUE_SEEKER),
                member_added(HUNTER_SEEKER),
            ]
            .into_iter()
            .chain(closing())
            .collect(),
        ),
        (
            ROGUE_SEEKER,
            [joined(), member_added(HUNTER_SEEKER)]
                .into_iter()
                .chain(closing())
                .collect(),
        ),
        (
            HUNTER_SEEKER,
            [joined()].into_iter().chain(closing()).collect(),
        ),
    ];
    let complete = poll_until(POLL_TIMEOUT, || {
        sessions.iter_mut().all(|session| {
            session.drain();
            let want = &expected
                .iter()
                .find(|(guid, _)| *guid == session.guid)
                .unwrap()
                .1;
            session.heard.len() >= want.len()
        })
    });
    for session in &sessions {
        let want = &expected
            .iter()
            .find(|(guid, _)| *guid == session.guid)
            .unwrap()
            .1;
        assert_eq!(
            &session.heard, want,
            "Seeker {} heard its stone events out of order or incomplete (wait timed out: {})",
            session.guid, !complete
        );
    }
}
