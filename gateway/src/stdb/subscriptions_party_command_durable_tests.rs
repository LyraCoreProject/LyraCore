//! Core's fixture command crosses World Shards with no Package installed. Realm-core certifies the
//! party, an issuer's sequence survives its Transfer, a Command Receipt travels with its Character,
//! and a command that waits too long finishes as Expired or OutcomeUnknown without a second apply.

use super::{party_command_intent, Coordinator, DURABLE_TOPOLOGY_ENV_LOCK};
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{module_bytes, poll_until, Standalone, POLL_TIMEOUT};
use crate::stdb::bindings::GamePartyCommandIntentTableAccess;
use crate::world::party::{self, AdmittedCompanionCommand, CompanionCommandOutcome};
use crate::world::{Actor, PartyStore, SessionStore, TransferStore};
use spacetimedb_sdk::Table;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::MutexGuard;
use std::time::Duration;

/// The Module's debug fixture command, active while no Package registers a handler.
const COMMAND: &str = "fixture.command";
const REPLY: &str = "fixture.command.result";
const GROUP: u64 = 5_098_000;
const USERNAME_ONE: &str = "CMDSOURCEONE";
const USERNAME_TWO: &str = "CMDSOURCETWO";

type Row = BTreeMap<String, String>;

struct TopologyEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
    _guard: MutexGuard<'static, ()>,
}

impl TopologyEnv {
    fn install(shard_map: &str, realm: &str) -> Self {
        let guard = DURABLE_TOPOLOGY_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let values = [
            ("LYRACORE_SHARD_MAP", shard_map),
            ("LYRACORE_REALM_CORE", realm),
            ("LYRACORE_CALL_PIPES", "1"),
            ("LYRACORE_AOI", "0"),
        ];
        let previous = values
            .iter()
            .map(|(name, value)| {
                let prior = std::env::var_os(name);
                std::env::set_var(name, value);
                (*name, prior)
            })
            .collect();
        Self {
            previous,
            _guard: guard,
        }
    }
}

impl Drop for TopologyEnv {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// Three session-less Characters with live bodies, the targets of every command.
struct CommandTopology {
    node: Standalone,
    target: String,
    realm: String,
    source_two: String,
    companions: [u64; 3],
    leader_one: u64,
    leader_two: u64,
    actor_one: String,
    actor_two: String,
    map_id: u32,
}

impl CommandTopology {
    fn new(name: &str) -> Self {
        let mut node = Standalone::start_persistent(name);
        node.publish_module();
        let source = node.shard_name().to_string();
        let target = format!("{source}-target");
        let realm = format!("{source}-realm");
        let source_two = format!("{source}-source-two");
        for database in [&target, &realm, &source_two] {
            node.publish_named_module_bytes(database, module_bytes());
        }
        for (database, guid_base) in [
            (&source, "0"),
            (&target, "1000000000"),
            (&source_two, "2000000000"),
        ] {
            node.assert_call_database(database, "claim_operator", &[]);
            node.assert_call_database(database, "install_guid_range", &[guid_base]);
        }
        node.assert_call_database(&realm, "claim_operator", &[]);
        // The seeded Account owns the companions. Each has no Account Claim, so it acts session-less.
        let companions =
            ["Cmdone", "Cmdtwo", "Cmdthree"].map(|name| stage_character(&node, &target, "1", name));
        let (leader_one, actor_one) = stage_leader(&node, &source, "Cmdleadone", USERNAME_ONE);
        let (leader_two, actor_two) = stage_leader(&node, &source_two, "Cmdleadtwo", USERNAME_TWO);
        let map_id = row(
            &node,
            &target,
            &format!(
                "SELECT map_id FROM game_world_entity WHERE guid = {}",
                companions[0]
            ),
        )["map_id"]
            .parse()
            .unwrap();
        let topology = Self {
            node,
            target,
            realm,
            source_two,
            companions,
            leader_one,
            leader_two,
            actor_one,
            actor_two,
            map_id,
        };
        topology.mirror_party(
            &[&topology.realm, &topology.target],
            leader_one,
            &companions,
        );
        topology
    }

    fn source(&self) -> &str {
        self.node.shard_name()
    }

    fn coordinator(&self, source: &str, name: &str) -> (tokio::runtime::Runtime, Coordinator) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let config = GatewayConfig {
            logon_bind: "127.0.0.1:0".into(),
            world_bind: "127.0.0.1:0".into(),
            stdb_uri: self.node.server().into(),
            module_name: source.into(),
            coordinator_token: Some(self.node.owner_token()),
            gateway_id: name.into(),
            blocking_task_capacity: BlockingTaskCapacity::new(2),
        };
        let coordinator = runtime.block_on(Coordinator::connect(&config)).unwrap();
        (runtime, coordinator)
    }

    /// Write one Party mirror, led by `leader`, onto each of `databases` at the next Roster
    /// Revision. Membership revisions come from Realm-core, so a retained member keeps its own.
    fn mirror_party(&self, databases: &[&str], leader: u64, companions: &[u64]) {
        let retained = rows(
            &self.node,
            &self.realm,
            &format!("SELECT * FROM game_group_member_partition WHERE group_id = {GROUP}"),
        );
        let revision = databases
            .iter()
            .flat_map(|database| {
                rows(
                    &self.node,
                    database,
                    &format!(
                        "SELECT revision FROM game_group_roster_revision WHERE group_id = {GROUP}"
                    ),
                )
            })
            .map(|row| row["revision"].parse::<u64>().unwrap())
            .max()
            .unwrap_or(0)
            + 1;
        let mut next_membership = retained
            .iter()
            .map(|row| row["membership_revision"].parse::<u64>().unwrap())
            .max()
            .unwrap_or(0);
        let mut partitions: Vec<_> = companions
            .iter()
            .chain([&leader])
            .map(|&guid| {
                let membership = retained
                    .iter()
                    .find(|row| {
                        row["character_guid"] == guid.to_string() && row["member_active"] == "true"
                    })
                    .map_or_else(
                        || {
                            next_membership += 1;
                            next_membership
                        },
                        |row| row["membership_revision"].parse().unwrap(),
                    );
                serde_json::json!({
                    "character_guid": guid, "group_id": GROUP,
                    "membership_revision": membership, "member_active": true,
                    "map_id": self.map_id, "instance_id": 0, "locator_revision": 1,
                    "state": {"known": []},
                })
            })
            .collect();
        partitions.sort_by_key(|partition| partition["membership_revision"].as_u64().unwrap());
        let members: Vec<_> = partitions
            .iter()
            .map(|partition| partition["character_guid"].clone())
            .collect();
        let args = [
            GROUP.to_string(),
            leader.to_string(),
            "0".to_string(),
            "2".to_string(),
            "0".to_string(),
            serde_json::to_string(&members).unwrap(),
            serde_json::json!({"guid": leader, "ownership": {"none": []}}).to_string(),
            serde_json::to_string(&partitions).unwrap(),
            revision.to_string(),
            // A Party, every member in Subgroup 0.
            "0".to_string(),
            serde_json::to_string(&vec![0u8; members.len()]).unwrap(),
        ];
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        for database in databases {
            self.node
                .assert_call_database(database, "sync_group_mirror", &args);
        }
    }

    /// Record every party member's current partition in Realm-core's character-to-shard index.
    fn publish_locators(&self, coordinator: &Coordinator) {
        let realm = coordinator.realm_core().unwrap();
        let members = self
            .companions
            .iter()
            .map(|&guid| (self.target.as_str(), guid))
            .chain([
                (self.source(), self.leader_one),
                (self.source_two.as_str(), self.leader_two),
            ]);
        let mut expected = Vec::new();
        for (database, guid) in members {
            let body = row(
                &self.node,
                database,
                &format!("SELECT map_id, instance_id FROM game_world_entity WHERE guid = {guid}"),
            );
            let partition = (
                body["map_id"].parse::<u32>().unwrap(),
                body["instance_id"].parse::<u64>().unwrap(),
            );
            realm
                .set_character_shard(guid, partition.0, partition.1)
                .unwrap();
            expected.push((guid, partition));
        }
        assert!(poll_until(POLL_TIMEOUT, || {
            expected.iter().all(|(guid, partition)| {
                realm
                    .realm_character_partition(*guid)
                    .unwrap()
                    .is_some_and(|locator| {
                        (locator.map_id, locator.instance_id) == *partition
                            && !locator.transfer_pending
                    })
            })
        }));
    }

    /// Queue the fixture command for `bot` through the authenticated client path; returns its id.
    fn queue(&self, source: &str, actor: &str, bot: u64) -> u64 {
        let ids = || {
            rows(
                &self.node,
                source,
                "SELECT id FROM game_party_command_intent",
            )
            .into_iter()
            .map(|row| row["id"].parse::<u64>().unwrap())
        };
        let before = ids().max().unwrap_or(0);
        self.node.assert_call_database(
            source,
            "gw_client_command",
            &[actor, &format!("\"{COMMAND}\""), &format!("\"{bot}\"")],
        );
        ids()
            .filter(|id| *id > before)
            .max()
            .expect("authenticated command did not queue")
    }

    /// The fixture replies `source` holds for one intent.
    fn replies(&self, source: &str, intent_id: u64) -> Vec<String> {
        rows(
            &self.node,
            source,
            &format!("SELECT payload FROM game_addon_message WHERE cmd = '{REPLY}'"),
        )
        .into_iter()
        .map(|row| row["payload"].clone())
        .filter(|payload| payload.starts_with(&format!("{intent_id}|")))
        .collect()
    }

    fn receipts(&self, database: &str, intent_id: u64) -> Vec<Row> {
        rows(
            &self.node,
            database,
            &format!("SELECT * FROM game_party_command_receipt WHERE intent_id = {intent_id}"),
        )
    }

    fn transfer(&self, database: &str, guid: u64, map_id: u32, reason: &str) {
        self.node.assert_call_database(
            database,
            "debug_bot_transfer",
            &[
                &guid.to_string(),
                &map_id.to_string(),
                "0",
                "1200",
                "1200",
                "50",
                "0",
                &format!("\"{reason}\""),
            ],
        );
    }

    fn character_on(&self, database: &str, guid: u64) -> bool {
        poll_until(POLL_TIMEOUT, || {
            rows(
                &self.node,
                database,
                &format!("SELECT guid FROM game_character WHERE guid = {guid}"),
            )
            .len()
                == 1
        })
    }
}

fn rows(node: &Standalone, database: &str, query: &str) -> Vec<Row> {
    node.query_database_rows(database, query)
}

fn row(node: &Standalone, database: &str, query: &str) -> Row {
    let rows = rows(node, database, query);
    assert_eq!(rows.len(), 1, "{query}: {rows:?}");
    rows.into_iter().next().unwrap()
}

/// Create a Human Warrior on `account` with a live body at the race's start; returns its guid.
fn stage_character(node: &Standalone, database: &str, account: &str, name: &str) -> u64 {
    node.assert_call_database(
        database,
        "create_character",
        &[
            account,
            &format!("\"{name}\""),
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let guid = row(
        node,
        database,
        &format!("SELECT guid FROM game_character WHERE name = '{name}'"),
    )["guid"]
        .clone();
    node.assert_call_database(database, "debug_spawn_player_entity", &[&guid]);
    guid.parse().unwrap()
}

/// A human leader with its own Account and a live Account Claim. Returns its guid and the Session
/// Actor its commands carry.
fn stage_leader(node: &Standalone, database: &str, name: &str, username: &str) -> (u64, String) {
    node.assert_call_database(
        database,
        "provision_account",
        &[&format!("\"{username}\""), "[]", "[]"],
    );
    let account = row(
        node,
        database,
        &format!("SELECT id FROM game_account WHERE username = '{username}'"),
    )["id"]
        .clone();
    let leader = stage_character(node, database, &account, name);
    node.assert_call_database(
        database,
        "claim_account",
        &[&account, &leader.to_string(), "9009"],
    );
    let generation = row(
        node,
        database,
        &format!("SELECT generation FROM game_account_claim WHERE account_id = {account}"),
    )["generation"]
        .clone();
    let actor = format!(
        r#"{{"guid":{leader},"ownership":{{"some":{{"account_id":{account},"generation":{generation},"request_nonce":9009}}}}}}"#
    );
    (leader, actor)
}

fn cached_intent(source: &Coordinator, id: u64) -> party::PartyCommandIntent {
    cached_intent_when(source, id, |_| true)
}

fn cached_intent_when(
    source: &Coordinator,
    id: u64,
    predicate: impl Fn(&crate::stdb::bindings::PartyCommandIntent) -> bool,
) -> party::PartyCommandIntent {
    let mut found = None;
    assert!(poll_until(POLL_TIMEOUT, || {
        found = source
            .0
            .coord()
            .conn
            .db
            .game_party_command_intent()
            .iter()
            .find(|row| row.id == id && predicate(row))
            .map(|row| party_command_intent(&row));
        found.is_some()
    }));
    found.unwrap()
}

/// The command as Realm-core certified it for `leader`'s current party.
fn admitted(
    source: &Coordinator,
    intent: &party::PartyCommandIntent,
    leader: u64,
) -> AdmittedCompanionCommand {
    let authority = source.realm_core().unwrap().group_roster(leader).unwrap();
    AdmittedCompanionCommand {
        source_identity: intent.source_identity,
        intent_id: intent.id,
        issuer_guid: intent.issuer_guid,
        issuer_sequence: intent.issuer_sequence,
        group_id: authority.group_id,
        leader_guid: authority.leader_guid,
        members: authority.member_guids(),
        kind: intent.kind,
        bot_guid: intent.bot_guid,
        authority_member_guid: intent.authority_member_guid,
        exact_target_guid: intent.exact_target_guid,
        expires_micros: intent.expires_micros,
        receipt_retain_until_micros: intent
            .expires_micros
            .saturating_add(lyracore_shared::group::COMMAND_RESULT_WINDOW_MICROS),
    }
}

fn applied(receipts: &[Row]) -> bool {
    receipts.len() == 1
        && receipts[0]["outcome"]
            .to_ascii_lowercase()
            .contains("applied")
}

/// Sign the Character's Account in on Realm-core and on `shard`, then enter the world there.
/// Returns the Session Actor its commands carry.
fn enter_transferred_actor(
    topology: &CommandTopology,
    runtime: &tokio::runtime::Runtime,
    shard: &Coordinator,
    character_guid: u64,
) -> String {
    let account_id: u64 = row(
        &topology.node,
        shard.shard_name(),
        &format!("SELECT account_id FROM game_character WHERE guid = {character_guid}"),
    )["account_id"]
        .parse()
        .unwrap();
    let username = row(
        &topology.node,
        shard.shard_name(),
        &format!("SELECT username FROM game_account WHERE id = {account_id}"),
    )["username"]
        .clone();
    let realm = shard.realm_core().unwrap();
    realm.provision_account(&username, &[], &[]).unwrap();
    let mut realm_account = None;
    assert!(poll_until(POLL_TIMEOUT, || {
        realm_account = realm.account_by_username(&username).unwrap();
        realm_account.is_some()
    }));
    let realm_account_id = realm_account.unwrap().id;
    let identity = realm.bound_identity(realm_account_id).unwrap();
    realm
        .establish_session(realm_account_id, &[7; 40], identity)
        .unwrap();
    shard
        .establish_session(account_id, &[7; 40], identity)
        .unwrap();
    assert!(poll_until(POLL_TIMEOUT, || {
        realm.session_key(realm_account_id).unwrap().is_some()
            && shard.session_key(account_id).unwrap().is_some()
    }));
    topology
        .node
        .assert_call_database(shard.shard_name(), "gw_heartbeat", &[]);
    let token = shard.claim_session(account_id, character_guid).unwrap();
    let bound = {
        let _entered = runtime.enter();
        shard.bind_session(token).unwrap()
    };
    bound
        .player_login(
            account_id,
            Actor::new(character_guid).unwrap(),
            crate::codec::WorldEntry::FreshLogin,
        )
        .unwrap();
    assert!(poll_until(POLL_TIMEOUT, || rows(
        &topology.node,
        shard.shard_name(),
        &format!("SELECT guid FROM game_world_entity WHERE guid = {character_guid}"),
    )
    .len()
        == 1));
    format!(
        r#"{{"guid":{character_guid},"ownership":{{"some":{{"account_id":{},"generation":{},"request_nonce":{}}}}}}}"#,
        token.account_id, token.generation, token.request_nonce
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
#[allow(clippy::too_many_lines)] // One durable step per crash, Transfer, and replay boundary.
fn command_receipts_recover_both_gateway_crash_boundaries() {
    let topology = CommandTopology::new("party-command-crash-recovery");
    let bot = topology.companions[0];
    let destination_map = topology.map_id + 1;
    let shard_map = format!(
        "{}:*={},{destination_map}:*={}",
        topology.map_id,
        topology.target,
        topology.source()
    );
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-source-one");
    let target = source.shard_handle(&topology.target).unwrap();
    topology.publish_locators(&source);

    // A worker claims and stops. Once the lease ends, another worker applies and replies once.
    let claimed = topology.queue(topology.source(), &topology.actor_one, bot);
    let claimed_intent = cached_intent(&source, claimed);
    source.claim_party_command_intent(claimed, 101).unwrap();
    assert!(topology.replies(topology.source(), claimed).is_empty());
    std::thread::sleep(Duration::from_millis(2_100));
    assert_eq!(
        party::run_party_command_intent(&source, &claimed_intent, 102).unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert_eq!(
        topology.replies(topology.source(), claimed),
        vec![format!("{claimed}|Applied")]
    );

    // The target applies and the source never hears of it.
    let target_applied = topology.queue(topology.source(), &topology.actor_one, bot);
    let target_intent = cached_intent(&source, target_applied);
    source
        .claim_party_command_intent(target_applied, 201)
        .unwrap();
    let admitted = admitted(&source, &target_intent, topology.leader_one);
    assert_eq!(
        target.apply_admitted_party_command(&admitted).unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert!(applied(
        &topology.receipts(&topology.target, target_applied)
    ));
    assert!(topology
        .replies(topology.source(), target_applied)
        .is_empty());

    // A second source Module mints the same intent id. Its receipt stays distinct.
    topology.mirror_party(
        &[&topology.realm, &topology.target],
        topology.leader_two,
        &topology.companions,
    );
    let (_runtime_two, source_two) =
        topology.coordinator(&topology.source_two, "party-command-source-two");
    let same_numeric_id = topology.queue(&topology.source_two, &topology.actor_two, bot);
    let second_source_intent = cached_intent(&source_two, same_numeric_id);
    assert_eq!(
        same_numeric_id, claimed,
        "fresh source databases must allocate the same numeric intent id for this collision case"
    );
    assert_ne!(
        second_source_intent.source_identity,
        claimed_intent.source_identity
    );
    assert_eq!(
        party::run_party_command_intent(&source_two, &second_source_intent, 301).unwrap(),
        CompanionCommandOutcome::Applied
    );

    // The bot leaves for the source's map. No holder applies a command while it is in transit or
    // after it has gone.
    topology.transfer(
        &topology.target,
        bot,
        destination_map,
        "party-command-receipt",
    );
    assert!(poll_until(POLL_TIMEOUT, || target
        .character_destination(bot)
        .is_some_and(
            |plan| (plan.dest_map_id, plan.dest_instance_id) == (destination_map, 0)
        )));
    let transfer_plan = target.character_destination(bot).unwrap();
    target
        .begin_transfer(Actor::new(bot).unwrap(), &transfer_plan)
        .unwrap();
    let mut delayed = admitted.clone();
    delayed.intent_id = target_applied + 1_000_000;
    let in_transit = target.apply_admitted_party_command(&delayed).unwrap_err();
    assert!(in_transit.to_string().contains("TransferInProgress"));
    crate::world::transfer::run_bot_transfer(
        &target,
        bot,
        destination_map,
        0,
        "party-command-receipt",
    )
    .unwrap();
    assert!(topology.character_on(topology.source(), bot));
    assert!(applied(
        &topology.receipts(topology.source(), target_applied)
    ));
    assert!(topology
        .receipts(&topology.target, target_applied)
        .is_empty());
    let gone = target.apply_admitted_party_command(&delayed).unwrap_err();
    assert!(gone.to_string().contains("NotCharacterHolder"));
    assert!(topology
        .receipts(&topology.target, delayed.intent_id)
        .is_empty());

    // The source finishes from the receipt that travelled, and a replayed apply finds it too.
    assert_eq!(
        party::run_party_command_intent(&source, &target_intent, 201).unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert_eq!(
        topology.replies(topology.source(), target_applied),
        vec![format!("{target_applied}|Applied")]
    );
    assert_eq!(
        source.apply_admitted_party_command(&admitted).unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert!(applied(
        &topology.receipts(topology.source(), target_applied)
    ));
    let collided = topology.receipts(topology.source(), claimed);
    assert_eq!(
        collided.len(),
        2,
        "distinct source identities collided: {collided:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
#[allow(clippy::too_many_lines)] // One durable step per issuer Transfer boundary.
fn command_issuer_sequence_continues_after_the_issuer_transfers() {
    let topology = CommandTopology::new("party-command-issuer-sequence-transfer");
    let bot = topology.companions[0];
    let shard_map = format!("{}:*={}", topology.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (runtime, source) = topology.coordinator(topology.source(), "party-command-old-source");
    let target = source.shard_handle(&topology.target).unwrap();
    topology.publish_locators(&source);

    let older_id = topology.queue(topology.source(), &topology.actor_one, bot);
    let older = cached_intent(&source, older_id);
    assert_eq!(older.issuer_sequence, 1);

    // The leader logs out and crosses to the target with its Account.
    target.provision_account(USERNAME_ONE, &[], &[]).unwrap();
    let actor: serde_json::Value = serde_json::from_str(&topology.actor_one).unwrap();
    let ownership = serde_json::to_string(&actor["ownership"]["some"]).unwrap();
    topology
        .node
        .assert_call_database(topology.source(), "release_account_claim", &[&ownership]);
    assert_eq!(
        row(
            &topology.node,
            &topology.target,
            &format!("SELECT id FROM game_account WHERE username = '{USERNAME_ONE}'"),
        )["id"],
        actor["ownership"]["some"]["account_id"].to_string(),
        "the destination must hold the transferred Account under its id"
    );
    topology.transfer(
        topology.source(),
        topology.leader_one,
        topology.map_id,
        "party-command-issuer",
    );
    crate::world::transfer::run_bot_transfer(
        &source,
        topology.leader_one,
        topology.map_id,
        0,
        "party-command-issuer",
    )
    .unwrap();
    assert!(topology.character_on(&topology.target, topology.leader_one));
    let issuer = |database: &str| {
        rows(
            &topology.node,
            database,
            &format!(
                "SELECT last_sequence FROM game_party_command_issuer WHERE character_guid = {}",
                topology.leader_one
            ),
        )
    };
    assert!(issuer(topology.source()).is_empty());
    assert_eq!(issuer(&topology.target)[0]["last_sequence"], "1");

    // The next command, from the new Shard, continues the issuer's sequence.
    let moved_actor = enter_transferred_actor(&topology, &runtime, &target, topology.leader_one);
    let newer_id = topology.queue(&topology.target, &moved_actor, bot);
    let newer = cached_intent(&target, newer_id);
    assert_eq!(newer.issuer_guid, older.issuer_guid);
    assert_eq!(newer.issuer_sequence, 2);
    assert_ne!(newer.source_identity, older.source_identity);
    assert_eq!(
        party::run_party_command_intent(&target, &newer, 701).unwrap(),
        CompanionCommandOutcome::Applied
    );

    // The older command still finishes and replies on its own source. Whether a newer sequence
    // supersedes it is the handler's decision; the fixture handler applies both.
    assert_eq!(
        party::run_party_command_intent(&source, &older, 702).unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert_eq!(
        target
            .confirm_party_command_receipt(older.source_identity, older_id)
            .unwrap(),
        Some(CompanionCommandOutcome::Applied)
    );
    assert_eq!(
        topology.replies(topology.source(), older_id),
        vec![format!("{older_id}|Applied")]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn command_receipt_stays_with_a_same_database_transfer() {
    let topology = CommandTopology::new("party-command-same-database-transfer");
    let bot = topology.companions[0];
    let destination_map = topology.map_id + 1;
    let shard_map = format!(
        "{}:*={},{destination_map}:*={}",
        topology.map_id, topology.target, topology.target
    );
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) =
        topology.coordinator(topology.source(), "party-command-same-database-source");
    let target = source.shard_handle(&topology.target).unwrap();
    topology.publish_locators(&source);
    let intent_id = topology.queue(topology.source(), &topology.actor_one, bot);
    let intent = cached_intent(&source, intent_id);
    assert_eq!(
        party::run_party_command_intent(&source, &intent, 601).unwrap(),
        CompanionCommandOutcome::Applied
    );
    topology.transfer(
        &topology.target,
        bot,
        destination_map,
        "party-command-same-database",
    );
    assert!(poll_until(POLL_TIMEOUT, || target
        .character_destination(bot)
        .is_some_and(
            |plan| (plan.dest_map_id, plan.dest_instance_id) == (destination_map, 0)
        )));
    crate::world::transfer::run_bot_transfer(
        &target,
        bot,
        destination_map,
        0,
        "party-command-same-database",
    )
    .unwrap();
    assert!(applied(&topology.receipts(&topology.target, intent_id)));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn command_lost_receipt_after_guarantee_reports_unknown_without_reapply() {
    let topology = CommandTopology::new("party-command-outcome-unknown");
    let bot = topology.companions[0];
    let shard_map = format!("{}:*={}", topology.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) =
        topology.coordinator(topology.source(), "party-command-unknown-source");
    let target = source.shard_handle(&topology.target).unwrap();
    let intent_id = topology.queue(topology.source(), &topology.actor_one, bot);
    let intent = cached_intent(&source, intent_id);
    source.claim_party_command_intent(intent_id, 711).unwrap();
    assert_eq!(
        target
            .apply_admitted_party_command(&admitted(&source, &intent, topology.leader_one))
            .unwrap(),
        CompanionCommandOutcome::Applied
    );
    topology.node.assert_call_database(
        &topology.target,
        "party_command_fixture_release_receipt",
        &[&bot.to_string()],
    );
    topology.node.assert_call_database(
        topology.source(),
        "party_command_fixture_expire_after_receipt_window",
        &[&intent_id.to_string()],
    );
    let expired = cached_intent_when(&source, intent_id, |row| {
        row.expires_micros < intent.expires_micros
    });
    assert_eq!(
        party::finish_expired_party_command_intent(&source, &expired, 711).unwrap(),
        CompanionCommandOutcome::OutcomeUnknown
    );
    assert!(topology.receipts(&topology.target, intent_id).is_empty());
    assert_eq!(
        topology.replies(topology.source(), intent_id),
        vec![format!("{intent_id}|OutcomeUnknown")]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn command_realm_admission_rejects_a_changed_unrelated_member() {
    let topology = CommandTopology::new("party-command-roster-certificate");
    let bot = topology.companions[0];
    let shard_map = format!("{}:*={}", topology.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-roster-source");
    let realm = source.realm_core().unwrap();
    let intent_id = topology.queue(topology.source(), &topology.actor_one, bot);
    let intent = cached_intent(&source, intent_id);
    let authority = realm.group_roster(topology.leader_one).unwrap();
    // Another companion leaves on Realm-core after the Gateway read the roster.
    topology.mirror_party(
        &[&topology.realm],
        topology.leader_one,
        &[topology.companions[0], topology.companions[2]],
    );

    let outcome = realm
        .admit_party_command_authority(
            authority.group_id,
            intent.issuer_guid,
            intent.bot_guid,
            intent.authority_member_guid,
            authority.member_guids(),
        )
        .unwrap();

    assert_eq!(outcome, CompanionCommandOutcome::StalePartyMirror);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
#[allow(clippy::too_many_lines)] // One durable step per capacity, lane, and expiry boundary.
fn command_capacity_waits_without_ack_then_recovers_or_expires() {
    let topology = CommandTopology::new("party-command-capacity");
    let bot = topology.companions[0];
    let shard_map = format!("{}:*={}", topology.map_id, topology.target);
    let _environment = TopologyEnv::install(&shard_map, &topology.realm);
    let (_runtime, source) = topology.coordinator(topology.source(), "party-command-capacity");
    let queue = || topology.queue(topology.source(), &topology.actor_one, bot);
    let intent_row = |id: u64| {
        row(
            &topology.node,
            topology.source(),
            &format!("SELECT pending, state FROM game_party_command_intent WHERE id = {id}"),
        )
    };

    // Fill the bot's receipt capacity.
    for token in 1..=32 {
        let intent = cached_intent(&source, queue());
        assert_eq!(
            party::run_party_command_intent(&source, &intent, 1_000 + token).unwrap(),
            CompanionCommandOutcome::Applied
        );
    }
    let waiting_id = queue();
    let waiting = cached_intent(&source, waiting_id);
    assert_eq!(
        party::run_party_command_intent(&source, &waiting, 2_001).unwrap(),
        CompanionCommandOutcome::WaitingForCapacity
    );
    let pending = intent_row(waiting_id);
    assert_eq!(pending["pending"], "true");
    assert!(pending["state"].to_ascii_lowercase().contains("pending"));
    assert!(topology.receipts(&topology.target, waiting_id).is_empty());
    assert!(topology.replies(topology.source(), waiting_id).is_empty());
    source
        .defer_party_command_intent(waiting_id, 2_001)
        .unwrap();

    // A full lane does not hold back a command for a bot in another lane.
    let blocked: Vec<u64> = (0..16).map(|_| queue()).collect();
    let lane_count = u64::from(lyracore_shared::group::COMMAND_DISPATCH_LANES);
    let bot_lane = bot % lane_count;
    let later_bot = topology.companions[1..]
        .iter()
        .copied()
        .find(|guid| *guid % lane_count != bot_lane)
        .expect("the fixture needs a companion in another command dispatch lane");
    let later_id = topology.queue(topology.source(), &topology.actor_one, later_bot);
    source.dispatch_party_command_intents();
    let later = intent_row(later_id);
    assert_eq!(later["pending"], "false");
    assert!(later["state"].to_ascii_lowercase().contains("applied"));
    assert!(blocked
        .iter()
        .all(|id| intent_row(*id)["pending"] == "true"));

    // Deferring every blocked command rotates the waiting command back to the lane head.
    for (offset, intent_id) in blocked.iter().enumerate() {
        let claim_token = 4_000 + offset as u64;
        source
            .claim_party_command_intent(*intent_id, claim_token)
            .unwrap();
        source
            .defer_party_command_intent(*intent_id, claim_token)
            .unwrap();
    }
    let retry_head = row(
        &topology.node,
        topology.source(),
        &format!(
            "SELECT head_intent_id FROM game_party_command_dispatch_lane WHERE lane = {bot_lane}"
        ),
    );
    assert_eq!(retry_head["head_intent_id"], waiting_id.to_string());

    // A released receipt lets the waiting command apply.
    topology.node.assert_call_database(
        &topology.target,
        "party_command_fixture_release_receipt",
        &[&bot.to_string()],
    );
    assert_eq!(
        party::run_party_command_intent(&source, &waiting, 2_001).unwrap(),
        CompanionCommandOutcome::Applied
    );
    assert_eq!(
        topology.replies(topology.source(), waiting_id),
        vec![format!("{waiting_id}|Applied")]
    );

    // A command that waits past its deadline expires once, under its own claim.
    let expiry_id = blocked[0];
    let expiry = cached_intent(&source, expiry_id);
    assert_eq!(
        party::run_party_command_intent(&source, &expiry, 3_001).unwrap(),
        CompanionCommandOutcome::WaitingForCapacity
    );
    topology.node.assert_call_database(
        topology.source(),
        "party_command_fixture_expire",
        &[&expiry_id.to_string()],
    );
    let expired = cached_intent_when(&source, expiry_id, |row| {
        row.expires_micros < expiry.expires_micros
    });
    let competing_finish =
        party::finish_expired_party_command_intent(&source, &expired, 3_002).unwrap_err();
    assert!(competing_finish.to_string().contains("ClaimLost"));
    assert_eq!(
        party::finish_expired_party_command_intent(&source, &expired, 3_001).unwrap(),
        CompanionCommandOutcome::Expired
    );
    let terminal = intent_row(expiry_id);
    assert_eq!(terminal["pending"], "false");
    assert!(terminal["state"].to_ascii_lowercase().contains("expired"));
    assert!(topology.receipts(&topology.target, expiry_id).is_empty());
}
