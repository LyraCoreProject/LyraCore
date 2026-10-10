use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::{module_bytes, poll_until, Standalone, POLL_TIMEOUT};
use crate::stdb::subscriptions::DURABLE_TOPOLOGY_ENV_LOCK;
use crate::stdb::Coordinator;
use crate::world::TransferStore;

fn login_on_kalimdor(gateway: Coordinator, runtime: &tokio::runtime::Runtime, guid: u64) {
    use crate::world::test_support::{
        client_handshake, drain_world_entry, world_session_socket_pair,
    };
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::{
        ClientMessage, Map, CMSG_PING, CMSG_PLAYER_LOGIN, SMSG_PONG,
    };
    use wow_world_messages::{Guid, Message};

    for shard in [&gateway, &gateway.realm_core().unwrap()] {
        let account = shard.account_by_username("TEST").unwrap().unwrap().id;
        shard
            .establish_session(account, &[7; 40], shard.bound_identity(account).unwrap())
            .unwrap();
    }
    let (mut client, socket) = world_session_socket_pair();
    let handle = runtime.handle().clone();
    let session = std::thread::spawn(move || {
        let _entered = handle.enter();
        crate::world::run_world_session(socket, std::sync::Arc::new(gateway))
    });
    let (mut encrypt, mut decrypt) = client_handshake(&mut client, "TEST", [7; 40]);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(guid),
    }
    .write_encrypted_client(&mut client, &mut encrypt)
    .unwrap();
    let entry = drain_world_entry(&mut client, &mut decrypt);
    assert!(entry.iter().any(|packet| matches!(packet, ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(world) if world.map == Map::Kalimdor)));
    CMSG_PING {
        sequence_id: 7,
        round_time_in_ms: 0,
    }
    .write_encrypted_client(&mut client, &mut encrypt)
    .unwrap();
    let answered = (0..64).any(|_| {
        let (opcode, body) = super::read_raw_frame(&mut client, &mut decrypt);
        opcode == SMSG_PONG::OPCODE as u16 && body == 7u32.to_le_bytes()
    });
    assert!(
        answered,
        "the Character must remain connected after Kalimdor entry"
    );
    drop(client);
    session.join().unwrap().unwrap();
}

struct TopologyEnv {
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl TopologyEnv {
    fn install(kalimdor: &str, instances: &str, realm: &str) -> Self {
        let guard = DURABLE_TOPOLOGY_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let map = format!("1:*={kalimdor},36:*={instances}");
        let values = [
            ("LYRACORE_SHARD_MAP", Some(map.as_str())),
            ("LYRACORE_SHARD_MAP_FILE", None),
            ("LYRACORE_REALM_CORE", Some(realm)),
            ("LYRACORE_CALL_PIPES", Some("1")),
        ];
        let previous = values
            .into_iter()
            .map(|(key, value)| {
                let previous = std::env::var_os(key);
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
                (key, previous)
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
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_first_kalimdor_transfer_arrives_and_recovers_after_import_or_source_finish() {
    const KALIMDOR: &str = "first-transfer-kalimdor";
    const INSTANCES: &str = "first-transfer-instances";
    const REALM: &str = "first-transfer-realm";
    let mut node = Standalone::start_persistent("first-transfer-world");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["0"]);
    node.assert_sql("INSERT INTO game_start_position (race_class, race, class, map_id, zone_id, x, y, z, orientation, display_id) VALUES (1025, 4, 1, 1, 141, 0, 0, 0, 0, 55)");
    for (shard, base) in [
        (KALIMDOR, "1000000000"),
        (INSTANCES, "2000000000"),
        (REALM, "3000000000"),
    ] {
        node.publish_named_module_bytes(shard, module_bytes());
        node.assert_call_database(shard, "claim_operator", &[]);
        node.assert_call_database(shard, "install_guid_range", &[base]);
    }

    let _topology = TopologyEnv::install(KALIMDOR, INSTANCES, REALM);
    let config = GatewayConfig {
        logon_bind: "127.0.0.1:0".into(),
        world_bind: "127.0.0.1:0".into(),
        stdb_uri: node.server().into(),
        module_name: node.shard_name().into(),
        coordinator_token: Some(node.owner_token()),
        gateway_id: "first-transfer-test".into(),
        blocking_task_capacity: BlockingTaskCapacity::new(2),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let gateway = runtime.block_on(Coordinator::connect(&config)).unwrap();
    for shard in [node.shard_name(), KALIMDOR, INSTANCES, REALM] {
        node.assert_call_database(shard, "gw_heartbeat", &[]);
    }
    {
        let _entered = runtime.enter();
        gateway.spawn_gateway_heartbeat();
    }
    let destination = gateway.shard_handle(KALIMDOR).unwrap();

    for (name, interruption) in [
        ("Firstarrival", None),
        ("Afterimport", Some("import_character_blob")),
        ("Afterfinish", Some("finish_transfer")),
    ] {
        node.assert_call(
            "create_character",
            &["1", name, "4", "1", "0", "0", "0", "0", "0", "0"],
        );
        let rows = node.query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ));
        let guid = rows[0]["guid"].parse::<u64>().unwrap();
        assert!(poll_until(POLL_TIMEOUT, || gateway
            .character_location(guid)
            == Some((1, 0))));
        assert!(gateway
            .realm_core()
            .unwrap()
            .realm_character_partition(guid)
            .unwrap()
            .is_none());

        if let Some(step) = interruption {
            let plan = gateway.character_destination(guid).unwrap();
            let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::world::transfer::run_transfer_injected(
                    &gateway,
                    &destination,
                    &plan,
                    Some(step),
                )
            }));
            assert!(
                interrupted.is_err(),
                "Transfer must reach the committed {step} boundary"
            );
        }

        // A fresh Coordinator reads the committed state as a replacement Gateway would.
        let replacement = runtime.block_on(Coordinator::connect(&config)).unwrap();
        login_on_kalimdor(replacement, &runtime, guid);
        let character_query = format!("SELECT guid FROM game_character WHERE guid = {guid}");
        assert!(node.query_rows(&character_query).is_empty());
        assert_eq!(
            node.query_database_rows(KALIMDOR, &character_query).len(),
            1
        );
        assert!(node
            .query_rows(&format!(
                "SELECT transfer_id FROM game_transfer_out WHERE character_guid = {guid}"
            ))
            .is_empty());
        assert!(node
            .query_database_rows(
                KALIMDOR,
                &format!("SELECT transfer_id FROM game_transfer_in WHERE character_guid = {guid}")
            )
            .is_empty());
        let locator = node.query_database_rows(REALM, &format!("SELECT revision, map_id, transfer_pending FROM game_character_shard WHERE character_guid = {guid}"));
        assert_eq!(locator[0]["map_id"], "1");
        assert_eq!(locator[0]["revision"], "2");
        assert_eq!(locator[0]["transfer_pending"], "false");
    }
}
