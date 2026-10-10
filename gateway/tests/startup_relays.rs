use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use lyracore_shared::group::realm_op;
use lyracore_test_support::{actor, log_dir, module_bytes, poll_until, Standalone, POLL_TIMEOUT};

struct Gateway {
    child: Child,
    log_path: PathBuf,
}

impl Gateway {
    fn start(node: &Standalone, other: &str, realm: &str) -> Self {
        let log_path = log_dir().join(format!("{}-gateway.log", node.shard_name()));
        let log = File::create(&log_path).expect("create Gateway log");
        let child = Command::new(env!("CARGO_BIN_EXE_lyracore-gateway"))
            .env_clear()
            .env("LYRACORE_LOGON_BIND", "127.0.0.1:0")
            .env("LYRACORE_WORLD_BIND", "127.0.0.1:0")
            .env("LYRACORE_SPACETIMEDB_URL", node.server())
            .env("LYRACORE_DATABASE", node.shard_name())
            .env("LYRACORE_COORDINATOR_TOKEN", node.owner_token())
            .env("LYRACORE_SHARD_MAP", format!("1:*={other}"))
            .env("LYRACORE_REALM_CORE", realm)
            .env("LYRACORE_GATEWAY_ID", node.shard_name())
            .env("LYRACORE_MAX_BLOCKING_THREADS", "4")
            .env("LYRACORE_CALL_PIPES", "1")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().expect("clone Gateway log")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("start Gateway");
        Self { child, log_path }
    }

    fn wait_for(&mut self, outcome: &str, mut probe: impl FnMut() -> bool) {
        let reached = poll_until(POLL_TIMEOUT, || {
            assert!(
                self.child
                    .try_wait()
                    .expect("read Gateway status")
                    .is_none(),
                "Gateway exited before {outcome}:\n{}",
                fs::read_to_string(&self.log_path).unwrap_or_default()
            );
            probe()
        });
        assert!(
            reached,
            "Gateway did not reach {outcome}:\n{}",
            fs::read_to_string(&self.log_path).unwrap_or_default()
        );
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn members(node: &Standalone, shard: &str, group: &str) -> BTreeSet<String> {
    node.query_database_rows(
        shard,
        &format!("SELECT character_guid FROM game_group_member WHERE group_id = {group}"),
    )
    .into_iter()
    .map(|row| row["character_guid"].clone())
    .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn gateway_startup_relays_rosters_and_completes_a_transfer_intent() {
    let mut node = Standalone::start_persistent("startup-relays-world");
    node.publish_module();
    let source = node.shard_name().to_owned();
    let other = format!("{source}-other");
    let realm = format!("{source}-realm");
    for shard in [&other, &realm] {
        node.publish_named_module_bytes(shard, module_bytes());
    }
    for shard in [&source, &other, &realm] {
        node.assert_call_database(shard, "claim_operator", &[]);
    }
    node.assert_call("install_guid_range", &["0"]);
    node.assert_call_database(&other, "install_guid_range", &["1000000000"]);
    node.assert_call_database(&other, "debug_delete_character", &["1"]);
    node.assert_call("debug_spawn_player_entity", &["1"]);
    node.assert_call(
        "create_character",
        &["1", "\"Companion\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let created = node.query_rows("SELECT guid FROM game_character WHERE name = 'Companion'");
    assert_eq!(created.len(), 1);
    let companion = created[0]["guid"].clone();
    node.assert_call("debug_spawn_player_entity", &[&companion]);
    for guid in ["1", companion.as_str()] {
        node.assert_call("debug_set_level", &[guid, "20"]);
        node.assert_call_database(
            &realm,
            "set_character_shard",
            &[guid, "0", "0", &actor(guid)],
        );
    }

    let mut gateway = Gateway::start(&node, &other, &realm);
    gateway.wait_for("a lease on every configured Shard", || {
        [&source, &other, &realm].into_iter().all(|shard| {
            node.query_database_rows(shard, "SELECT id FROM game_gateway_lease")
                .len()
                == 1
        })
    });

    node.assert_call(
        "debug_emit_sessionless_group_intent",
        &["1", &companion, "false"],
    );
    gateway.wait_for("a session-less invite executed on Realm-core", || {
        node.query_database_rows(&realm, "SELECT character_guid FROM game_group_member")
            .len()
            == 2
            && node
                .query_rows("SELECT id FROM game_bot_invite_intent")
                .is_empty()
    });
    let groups = node.query_database_rows(
        &realm,
        "SELECT group_id FROM game_group WHERE leader_guid = 1",
    );
    assert_eq!(groups.len(), 1);
    let group = &groups[0]["group_id"];
    let revision_query =
        format!("SELECT revision, active FROM game_group_roster_revision WHERE group_id = {group}");
    let revision = node.query_database_rows(&realm, &revision_query);
    let expected_members = BTreeSet::from(["1".to_owned(), companion.clone()]);
    assert_eq!(members(&node, &realm, group), expected_members);
    gateway.wait_for("the Realm roster on both World Shards", || {
        [&source, &other].into_iter().all(|shard| {
            members(&node, shard, group) == expected_members
                && node.query_database_rows(shard, &revision_query) == revision
        })
    });

    node.assert_call_database(
        &realm,
        "realm_group_op",
        &[
            &realm_op::LOOT_METHOD.to_string(),
            &actor("1"),
            "0",
            "0",
            "2",
            "0",
        ],
    );
    let changed_revision = node.query_database_rows(&realm, &revision_query);
    assert_ne!(changed_revision, revision);
    let loot_query =
        format!("SELECT loot_method, loot_threshold FROM game_group WHERE group_id = {group}");
    gateway.wait_for("changed loot rules on both World Shards", || {
        [&source, &other].into_iter().all(|shard| {
            let rules = node.query_database_rows(shard, &loot_query);
            rules.len() == 1
                && rules[0]["loot_method"] == "0"
                && rules[0]["loot_threshold"] == "2"
                && node.query_database_rows(shard, &revision_query) == changed_revision
        })
    });

    node.assert_call("debug_delete_character", &["1"]);
    gateway.wait_for(
        "the deleted Character's party to disband on Realm-core",
        || {
            let revision = node.query_database_rows(&realm, &revision_query);
            members(&node, &realm, group).is_empty()
                && revision.len() == 1
                && revision[0]["active"] == "false"
        },
    );
    let tombstone = node.query_database_rows(&realm, &revision_query);
    assert_eq!(tombstone.len(), 1);
    assert_eq!(tombstone[0]["active"], "false");
    gateway.wait_for("the disband on both World Shards", || {
        [&source, &other].into_iter().all(|shard| {
            members(&node, shard, group).is_empty()
                && node
                    .query_database_rows(
                        shard,
                        &format!("SELECT group_id FROM game_group WHERE group_id = {group}"),
                    )
                    .is_empty()
                && node.query_database_rows(shard, &revision_query) == tombstone
        })
    });

    node.assert_call(
        "debug_bot_transfer",
        &[
            &companion,
            "1",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "\"startup relay\"",
        ],
    );
    gateway.wait_for("a completed Transfer Intent", || {
        let arrived = node.query_database_rows(
            &other,
            &format!("SELECT map_id, pending_instance_id FROM game_character WHERE guid = {companion}"),
        );
        let locator = node.query_database_rows(
            &realm,
            &format!(
                "SELECT map_id, instance_id, transfer_pending FROM game_character_shard \
                 WHERE character_guid = {companion}"
            ),
        );
        arrived.len() == 1
            && arrived[0]["map_id"] == "1"
            && arrived[0]["pending_instance_id"] == "0"
            && locator.len() == 1
            && locator[0]["map_id"] == "1"
            && locator[0]["instance_id"] == "0"
            && locator[0]["transfer_pending"] == "false"
            && node
                .query_rows(&format!("SELECT guid FROM game_character WHERE guid = {companion}"))
                .is_empty()
            && node
                .query_rows(&format!(
                    "SELECT id FROM game_bot_transfer_intent WHERE bot_guid = {companion}"
                ))
                .is_empty()
            && node
                .query_database_rows(
                    &other,
                    &format!(
                        "SELECT transfer_id FROM game_transfer_in WHERE character_guid = {companion}"
                    ),
                )
                .is_empty()
    });
}
