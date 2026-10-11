use super::*;
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{poll_until, Standalone, POLL_TIMEOUT};
use std::sync::mpsc::Receiver;
use wow_world_messages::vanilla::{Object, ServerMessage, UpdateMask, VisibleItemIndex};

struct TopologyEnv {
    previous: [Option<std::ffi::OsString>; 3],
    _guard: std::sync::MutexGuard<'static, ()>,
}

const TOPOLOGY_VARS: [&str; 3] = [
    "LYRACORE_SHARD_MAP",
    "LYRACORE_SHARD_MAP_FILE",
    "LYRACORE_REALM_CORE",
];

impl TopologyEnv {
    fn clear() -> Self {
        let guard = super::super::subscriptions::DURABLE_TOPOLOGY_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = TOPOLOGY_VARS.map(std::env::var_os);
        for name in TOPOLOGY_VARS {
            std::env::remove_var(name);
        }
        Self {
            previous,
            _guard: guard,
        }
    }
}

impl Drop for TopologyEnv {
    fn drop(&mut self) {
        for (name, previous) in TOPOLOGY_VARS.into_iter().zip(&self.previous) {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn worn_values(outbound: Outbound, heard: &mut Vec<(u32, u32, u32)>) {
    let message = match outbound {
        Outbound::Job(job) => {
            for message in job() {
                worn_values(message, heard);
            }
            return;
        }
        Outbound::One(message) => message,
        _ => panic!("the item peer Relay emits typed Character VALUES"),
    };
    let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) = message else {
        panic!("private inventory feedback reached a peer");
    };
    let [Object::Values {
        guid1,
        mask1: UpdateMask::Player(mask),
    }] = update.objects.as_slice()
    else {
        panic!("expected Character VALUES, got {:?}", update.objects);
    };
    assert_eq!(guid1.guid(), 1);
    let mut bytes = Vec::new();
    update.write_unencrypted_server(&mut bytes).unwrap();
    let decoded = lyracore_shared::values_mask::parse_values_updates(&bytes[4..]);
    assert_eq!(decoded.len(), 1);
    // Build 5875 puts the main-hand visible item in fields 438 through 449.
    assert!(decoded[0]
        .fields
        .iter()
        .all(|&(field, _)| (438..450).contains(&field)));
    let Some(item) = mask.player_visible_item(VisibleItemIndex::Index15) else {
        return;
    };
    assert_eq!(item.enchants[1], 0);
    heard.push((item.item, item.random_property_id, item.enchants[0]));
}

fn expect_worn(rx: &Receiver<Outbound>, expected: (u32, u32, u32)) {
    let mut heard = Vec::new();
    assert!(
        poll_until(POLL_TIMEOUT, || {
            for outbound in rx.try_iter() {
                worn_values(outbound, &mut heard);
            }
            heard.contains(&expected)
        }),
        "expected worn item {expected:?}, received {heard:?}"
    );
    assert!(heard.iter().all(|item| *item == expected));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn live_worn_item_insert_enchant_and_delete_reach_a_nearby_peer() {
    let _topology = TopologyEnv::clear();
    let mut standalone = Standalone::start("worn-item-relay");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    // Seeded Northshire creatures must not add unrelated packets to this item Relay check.
    standalone.assert_sql("UPDATE game_character SET map_id = 1 WHERE guid = 1");
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1 AND slot = 15");

    let cfg = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: standalone.server().into(),
        module_name: standalone.shard_name().into(),
        coordinator_token: Some(standalone.owner_token()),
        gateway_id: "worn-item-relay-test".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(1),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let coordinator = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
    let view = coordinator.world_view();
    let anchor = view
        .spatial
        .entity_cell_on_shard(EntityLayer::WorldEntity, 1, 0)
        .unwrap();
    let (tx, rx) = SessionTx::with_depth(0);
    let viewer = Arc::new(Viewer {
        active: std::sync::atomic::AtomicBool::new(true),
        session: view.next_session_id(),
        self_guid: 2,
        bound_identity: spacetimedb_sdk::Identity::from_byte_array([2; 32]),
        map_id: anchor.map_id,
        instance_id: anchor.instance_id,
        zone_id: AtomicU32::new(0),
        tx,
        created: Arc::new(Mutex::new(HashSet::from([1, 2]))),
        gates: Arc::new(ViewerGates::default()),
        skill_slots: Arc::new(Mutex::default()),
        explored: Mutex::default(),
        motion_pending: Arc::new(MotionPending::default()),
        member_stats: Default::default(),
        ignored: Mutex::default(),
        friends: Mutex::default(),
        team: lyracore_shared::faction::TEAM_ALLIANCE,
        group_events: Default::default(),
    });
    view.add_viewer_on_shard(viewer.clone(), anchor, 0);

    standalone.assert_call("debug_equip_weapon", &["1", "50"]);
    expect_worn(&rx, (50, 0, 0));
    standalone.assert_sql(
        "UPDATE game_item_instance SET enchant_id = 7745, random_property_id = 117 WHERE owner_guid = 1 AND slot = 15",
    );
    expect_worn(&rx, (50, 117, 823));
    standalone
        .assert_sql("UPDATE game_item_instance SET slot = 23 WHERE owner_guid = 1 AND slot = 15");
    expect_worn(&rx, (0, 0, 0));
    standalone
        .assert_sql("UPDATE game_item_instance SET slot = 15 WHERE owner_guid = 1 AND slot = 23");
    expect_worn(&rx, (50, 117, 823));
    standalone.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1 AND slot = 15");
    expect_worn(&rx, (0, 0, 0));
    view.remove_viewer(viewer.session);
}
