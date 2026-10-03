use super::*;
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{module_bytes, poll_until, Standalone, POLL_TIMEOUT};
use crate::world::party::PartyOutcome;
use lyracore_shared::group::realm_op;

/// Holds the process-wide topology variables for the whole test, then restores them.
struct TopologyEnv {
    shard_map: Option<std::ffi::OsString>,
    realm_core: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl TopologyEnv {
    fn install(shard_map: &str, realm_core: &str) -> Self {
        let guard = DURABLE_TOPOLOGY_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = Self {
            shard_map: std::env::var_os("LYRACORE_SHARD_MAP"),
            realm_core: std::env::var_os("LYRACORE_REALM_CORE"),
            _guard: guard,
        };
        std::env::set_var("LYRACORE_SHARD_MAP", shard_map);
        std::env::set_var("LYRACORE_REALM_CORE", realm_core);
        previous
    }
}

impl Drop for TopologyEnv {
    fn drop(&mut self) {
        match &self.shard_map {
            Some(value) => std::env::set_var("LYRACORE_SHARD_MAP", value),
            None => std::env::remove_var("LYRACORE_SHARD_MAP"),
        }
        match &self.realm_core {
            Some(value) => std::env::set_var("LYRACORE_REALM_CORE", value),
            None => std::env::remove_var("LYRACORE_REALM_CORE"),
        }
    }
}

/// Every World Shard holds Realm-core's Roster Revision for the party and, while it exists, its
/// members.
fn mirrors_follow_realm_core(
    realm: &Coordinator,
    shards: &[(String, Coordinator)],
    group_id: u64,
    leader_guid: u64,
) -> bool {
    let Some(revision) = realm.held_roster_revision(group_id) else {
        return false;
    };
    let members = realm
        .group_roster(leader_guid)
        .map(|roster| roster.member_guids());
    shards.iter().all(|(_, shard)| {
        shard.held_roster_revision(group_id) == Some(revision)
            && shard
                .group_roster(leader_guid)
                .map(|roster| roster.member_guids())
                == members
    })
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_roster_change_made_only_on_realm_core_reaches_every_world_shard_mirror() {
    const INSTANCES: &str = "roster-relay-instances";
    const REALM: &str = "roster-relay-realm";
    const LEADER: u64 = 1;
    const MEMBER: u64 = 2;

    let mut standalone = Standalone::start_persistent("roster-relay-world");
    standalone.publish_module();
    for database in [INSTANCES, REALM] {
        standalone.publish_named_module_bytes(database, module_bytes());
    }
    standalone.assert_call("claim_operator", &[]);
    for database in [INSTANCES, REALM] {
        standalone.assert_call_database(database, "claim_operator", &[]);
    }

    let _topology = TopologyEnv::install(&format!("36:*={INSTANCES}"), REALM);
    let cfg = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: standalone.server().into(),
        module_name: standalone.shard_name().into(),
        coordinator_token: Some(standalone.owner_token()),
        gateway_id: "roster-relay-test".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(1),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let gateway = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    gateway.spawn_roster_revision_relay();
    let realm = gateway.realm_core().unwrap();
    let shards = gateway.world_shards();
    assert_eq!(shards.len(), 2, "the default World Shard and {INSTANCES}");

    // Straight to Realm-core: no `party::run`, so only the relay can update the mirrors.
    assert_eq!(
        realm
            .realm_group_op(realm_op::INVITE, LEADER, MEMBER, 0, 0, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert_eq!(
        realm
            .realm_group_op(realm_op::ACCEPT, MEMBER, 0, 1, 1, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert!(poll_until(POLL_TIMEOUT, || {
        realm
            .group_roster(LEADER)
            .is_some_and(|roster| roster.member_guids() == [LEADER, MEMBER])
    }));
    let group_id = realm.group_roster(LEADER).unwrap().group_id;
    assert!(
        poll_until(POLL_TIMEOUT, || mirrors_follow_realm_core(
            &realm, &shards, group_id, LEADER
        )),
        "the formed party never reached every mirror: {:?}",
        shards
            .iter()
            .map(|(name, shard)| (name, shard.held_roster_revision(group_id)))
            .collect::<Vec<_>>()
    );

    // A party of one disbands, so the mirrors get the tombstone and forget the party.
    assert_eq!(
        realm
            .realm_group_op(realm_op::LEAVE, MEMBER, 0, 0, 0, 0)
            .unwrap(),
        PartyOutcome::Ran
    );
    assert!(poll_until(POLL_TIMEOUT, || realm
        .group_roster(LEADER)
        .is_none()));
    assert!(
        poll_until(POLL_TIMEOUT, || mirrors_follow_realm_core(
            &realm, &shards, group_id, LEADER
        )),
        "the disband never reached every mirror: {:?}",
        shards
            .iter()
            .map(|(name, shard)| (name, shard.held_roster_revision(group_id)))
            .collect::<Vec<_>>()
    );
}
