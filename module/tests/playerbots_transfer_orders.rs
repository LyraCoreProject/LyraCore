//! Order decisions on a private Module with declared later Realm partition facts.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use support::Standalone;

const TARGET: u64 = (0xF130u64 << 48) | (5_098_001u64 << 24) | 1;

type Row = BTreeMap<String, String>;

fn one(node: &Standalone, query: &str) -> Row {
    let mut rows = node.query_rows(query);
    assert_eq!(rows.len(), 1, "{query}: {rows:?}");
    rows.remove(0)
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn digest(path: &Path, hash: &mut blake3::Hasher) {
    let mut children: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    hash.update(&(children.len() as u64).to_le_bytes());
    for child in children {
        let name = child.file_name().unwrap().as_encoded_bytes();
        hash.update(&(name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update(&[u8::from(child.is_dir())]);
        if child.is_dir() {
            digest(&child, hash);
        } else {
            let bytes = std::fs::read(child).unwrap();
            hash.update(&(bytes.len() as u64).to_le_bytes());
            hash.update(&bytes);
        }
    }
}

struct Party {
    node: Standalone,
    bots: [String; 4],
    leader: String,
    actor: String,
}

fn setup(mode: u8) -> Party {
    let mut node = Standalone::start(&format!("playerbots-transfer-orders-{mode}"));
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    for (count, class, role) in [("2", "1", "0"), ("1", "5", "1"), ("2", "8", "2")] {
        node.assert_call(
            "playerbots_spawn_class_role",
            &[count, "1200", "1200", "50", class, role],
        );
    }
    let mut bots = node.query_rows("SELECT character_guid, role FROM pkg_playerbots_bot");
    bots.sort_by_key(|bot| bot["character_guid"].parse::<u64>().unwrap());
    let by_role = |role: &str| -> Vec<String> {
        bots.iter()
            .filter(|bot| bot["role"] == role)
            .map(|bot| bot["character_guid"].clone())
            .collect()
    };
    let warriors = by_role("0");
    let priests = by_role("1");
    let mages = by_role("2");
    assert_eq!((warriors.len(), priests.len(), mages.len()), (2, 1, 2));
    let leader = warriors[1].clone();
    let bots = [
        warriors[0].clone(),
        priests[0].clone(),
        mages[0].clone(),
        mages[1].clone(),
    ];
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call(
        "playerbots_transfer_orders_stage",
        &[&bots[0], &bots[1], &bots[2], &bots[3], &leader],
    );
    for guid in &bots {
        node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    }
    let x = lyracore_shared::terrain::cell_index(-11208.5).unwrap();
    let y = lyracore_shared::terrain::cell_index(1685.34).unwrap();
    let rows: Vec<_> = (x - 5..=x + 5)
        .flat_map(|cx| (y - 5..=y + 5).map(move |cy| format!("0,{cx},{cy},25.7612,,")))
        .collect();
    node.assert_call("import_nav_chunks", &[&rows.join(";")]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("provision_account", &[r#""PB010ORDERS""#, "[]", "[]"]);
    let account = one(
        &node,
        "SELECT id FROM game_account WHERE username = 'PB010ORDERS'",
    )["id"]
        .clone();
    node.assert_call("playerbots_fixture_orders_account", &[&leader, &account]);
    node.assert_call("claim_account", &[&account, &leader, "9012"]);
    let generation = one(
        &node,
        &format!("SELECT generation FROM game_account_claim WHERE account_id = {account}"),
    )["generation"]
        .clone();
    let actor = serde_json::json!({"guid": leader.parse::<u64>().unwrap(), "ownership": {"some": {
        "account_id": account.parse::<u64>().unwrap(), "generation": generation.parse::<u64>().unwrap(), "request_nonce": 9012,
    }}}).to_string();
    let party = Party {
        node,
        bots,
        leader,
        actor,
    };
    for (index, payload) in [
        format!("follow|{}", party.bots[0]),
        format!("stay|{}", party.bots[1]),
        format!("assist|{}|{}", party.bots[2], party.bots[1]),
        format!("target|{}|{TARGET}", party.bots[3]),
    ]
    .into_iter()
    .enumerate()
    {
        party.node.assert_call(
            "gw_client_command",
            &[
                &party.actor,
                r#""playerbots.order""#,
                &serde_json::to_string(&payload).unwrap(),
            ],
        );
        let id = party
            .node
            .query_rows("SELECT id FROM game_party_command_intent")
            .iter()
            .map(|row| row["id"].parse::<u64>().unwrap())
            .max()
            .unwrap();
        party.node.assert_call(
            "playerbots_fixture_orders_drive",
            &[
                &id.to_string(),
                &(10000 + id).to_string(),
                &party.bots[index],
                "true",
            ],
        );
    }
    party
}

fn snapshot(party: &Party, phase: &str, mode: u8) -> serde_json::Value {
    let mut bots = serde_json::Map::new();
    for guid in &party.bots {
        bots.insert(guid.clone(), serde_json::json!({
            "order": one(&party.node, &format!("SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {guid}")),
            "runner": one(&party.node, &format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}")),
            "character": party.node.query_rows(&format!("SELECT guid, map_id, pending_instance_id, x, y, z FROM game_character WHERE guid = {guid}")),
            "movement": party.node.query_rows(&format!("SELECT * FROM game_creature_spline WHERE guid = {guid}")),
            "entity": party.node.query_rows(&format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {guid}")),
            "intent": party.node.query_rows(&format!("SELECT * FROM game_bot_transfer_intent WHERE bot_guid = {guid}")),
            "cast": party.node.query_rows(&format!("SELECT * FROM game_pending_cast WHERE caster_guid = {guid}")),
            "attack": party.node.query_rows(&format!("SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}")),
        }));
    }
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let package = core.join("packages/playerbots");
    let mut content = blake3::Hasher::new();
    digest(&package, &mut content);
    let record = serde_json::json!({
        "phase": phase, "mode": mode, "bots": bots,
        "leader_entity": party.node.query_rows(&format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {}", party.leader)),
        "leader_character": party.node.query_rows(&format!("SELECT guid, map_id, pending_instance_id, x, y, z FROM game_character WHERE guid = {}", party.leader)),
        "target_entity": party.node.query_rows(&format!("SELECT guid, map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {TARGET}")),
        "movement_tick": party.node.query_rows("SELECT * FROM game_creature_move_schedule"),
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "package_content_identity": content.finalize().to_hex().to_string(),
        "module_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "partition_input_scope": "declared later Realm inputs applied through the Module mirror; real Gateway crossing and return route remain separate",
        "geometry": "declared flat source navigation at the audited trigger 78 coordinates; no imported floor or client claim",
        "movement_schedule": "next ordinary Core movement tick declared sixty seconds before orders; Target cancellation must remove the retained leg before that tick",
        "source_audit": "pb010-imported-portal-source-14cda70a",
        "partitions": party.node.query_rows("SELECT * FROM game_group_member_partition WHERE group_id = 5098000"),
        "portal": party.node.query_rows("SELECT * FROM game_area_trigger WHERE id = 78"),
        "route": party.node.query_rows("SELECT * FROM game_areatrigger_teleport WHERE trigger_id = 78"),
    });
    std::fs::write(
        support::log_dir().join(format!("{}-{phase}.json", party.node.shard_name())),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    record
}

fn partition<'a>(snapshot: &'a serde_json::Value, guid: &str) -> &'a serde_json::Value {
    let rows: Vec<_> = snapshot["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["character_guid"] == guid)
        .collect();
    assert_eq!(rows.len(), 1, "partition {guid}: {snapshot}");
    rows[0]
}

fn assert_body(row: &serde_json::Value, map: &str, instance: &str) {
    assert_eq!(row["map_id"], map, "{row}");
    assert_eq!(row["instance_id"], instance, "{row}");
}

fn assert_initial_orders(party: &Party, before: &serde_json::Value) {
    let bodies = before["leader_entity"].as_array().unwrap();
    assert_eq!(bodies.len(), 1, "{before}");
    assert_body(&bodies[0], "0", "0");
    assert_eq!(before["target_entity"].as_array().unwrap().len(), 1);
    assert_body(&before["target_entity"][0], "0", "0");
    for guid in party.bots.iter().chain(std::iter::once(&party.leader)) {
        let p = partition(before, guid);
        assert_eq!(p["group_id"], "5098000");
        assert_eq!(p["member_active"], "true");
        assert_eq!(p["state"], "(known = ())");
        assert_eq!(p["locator_revision"], "1");
        assert_body(p, "0", "0");
    }
    for (index, guid) in party.bots.iter().enumerate() {
        let bot = &before["bots"][guid];
        let order = &bot["order"];
        assert_eq!(order["character_guid"], *guid);
        assert_eq!(order["issuer_guid"], party.leader);
        assert_eq!(order["group_id"], "5098000");
        assert_eq!(order["active"], "true");
        assert_eq!(order["revision"], "1");
        assert!(
            order["issuer_sequence"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 0
        );
        let expected = match index {
            0 => format!("(follow = (leader_guid = {}))", party.leader),
            1 => {
                let entity = &bot["entity"][0];
                format!(
                    "(stay = (map_id = 0, instance_id = 0, x = {}, y = {}, z = {}))",
                    entity["x"].as_str().unwrap(),
                    entity["y"].as_str().unwrap(),
                    entity["z"].as_str().unwrap()
                )
            }
            2 => format!("(assist = (member_guid = {}))", party.bots[1]),
            3 => format!("(target = (target_guid = {TARGET}))"),
            _ => unreachable!(),
        };
        assert_eq!(order["order"], expected, "{before}");
    }
    let target = &before["bots"][&party.bots[3]];
    assert!(
        target["runner"]["foreground"]
            .as_str()
            .unwrap()
            .contains("move"),
        "{before}"
    );
    assert_eq!(target["movement"].as_array().unwrap().len(), 1, "{before}");
}

fn assert_remote_inputs(
    party: &Party,
    before: &serde_json::Value,
    remote: &serde_json::Value,
    mode: u8,
) {
    assert!(
        remote["leader_entity"].as_array().unwrap().is_empty(),
        "{remote}"
    );
    assert_eq!(remote["leader_character"].as_array().unwrap().len(), 1);
    assert_eq!(remote["leader_character"][0]["map_id"], "36");
    assert_eq!(
        remote["leader_character"][0]["pending_instance_id"],
        "5098078"
    );
    for guid in party.bots.iter().chain(std::iter::once(&party.leader)) {
        let old = partition(before, guid);
        let current = partition(remote, guid);
        let remote_member = *guid == party.leader || (mode == 1 && *guid == party.bots[1]);
        let mut expected = old.clone();
        if remote_member {
            expected["map_id"] = "36".into();
            expected["instance_id"] = "5098078".into();
            expected["locator_revision"] = (old["locator_revision"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                + 1)
            .to_string()
            .into();
        }
        assert_eq!(*current, expected, "{guid}: {remote}");
    }
    for (index, guid) in party.bots.iter().enumerate() {
        let old = &before["bots"][guid];
        let current = &remote["bots"][guid];
        assert_eq!(current["order"], old["order"], "{remote}");
        assert_eq!(current["runner"], old["runner"], "{remote}");
        if mode == 1 && index == 1 {
            assert!(current["entity"].as_array().unwrap().is_empty(), "{remote}");
            assert_eq!(current["character"][0]["map_id"], "36");
            assert_eq!(current["character"][0]["pending_instance_id"], "5098078");
        } else {
            assert_eq!(current["entity"], old["entity"], "{remote}");
            assert_eq!(current["character"], old["character"], "{remote}");
        }
    }
    if mode == 2 {
        assert_body(&remote["target_entity"][0], "36", "5098078");
    } else {
        assert_eq!(remote["target_entity"], before["target_entity"]);
    }
    assert_eq!(
        remote["bots"][&party.bots[3]]["movement"], before["bots"][&party.bots[3]]["movement"],
        "the real Target leg must still be pending before the ordinary decision: {remote}"
    );
}

fn order_decisions(mode: u8) {
    let party = setup(mode);
    let target_started = support::poll_until(Duration::from_secs(10), || {
        party
            .node
            .assert_call("playerbots_fixture_runner_pass_once", &[&party.bots[3]]);
        !party
            .node
            .query_rows(&format!(
                "SELECT guid FROM game_creature_spline WHERE guid = {}",
                party.bots[3]
            ))
            .is_empty()
    });
    let before = snapshot(&party, "before-remote-member", mode);
    assert!(target_started, "Target approach did not start: {before}");
    assert_initial_orders(&party, &before);
    assert_eq!(before["partitions"].as_array().unwrap().len(), 5);
    assert_eq!(before["portal"].as_array().unwrap().len(), 1);
    assert_eq!(before["route"].as_array().unwrap().len(), 1);
    assert_eq!(before["portal"][0]["id"], "78");
    assert_eq!(before["portal"][0]["map_id"], "0");
    for (field, expected) in [
        ("x", -11208.5),
        ("y", 1685.34),
        ("z", 25.7612),
        ("radius", 7.0),
    ] {
        let value: f64 = before["portal"][0][field]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((value - expected).abs() < 0.002);
    }
    assert_eq!(before["route"][0]["target_map"], "36");
    for guid in &party.bots {
        assert_eq!(before["bots"][guid]["order"]["revision"], "1");
        assert_eq!(
            before["bots"][guid]["runner"]["companion_order_revision"],
            "1"
        );
    }
    party.node.assert_call(
        "playerbots_transfer_orders_remote",
        &[
            &party.actor,
            &party.bots[1],
            &TARGET.to_string(),
            &mode.to_string(),
        ],
    );
    let remote = snapshot(&party, "remote-inputs-before-decisions", mode);
    assert_remote_inputs(&party, &before, &remote, mode);
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(1100));
        for (index, guid) in party.bots.iter().enumerate() {
            if mode == 1 && index == 1 {
                continue;
            }
            if !party
                .node
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE guid = {guid}"
                ))
                .is_empty()
            {
                party
                    .node
                    .assert_call("playerbots_fixture_runner_pass_once", &[guid]);
            }
        }
    }
    let after = snapshot(&party, "after-remote-decisions", mode);
    for (index, guid) in party.bots.iter().enumerate() {
        let old = &before["bots"][guid];
        let current = &after["bots"][guid];
        for field in [
            "order",
            "revision",
            "issuer_guid",
            "issuer_sequence",
            "group_id",
            "active",
        ] {
            assert_eq!(
                old["order"][field], current["order"][field],
                "{guid} {field}: {after}"
            );
        }
        if mode == 1 && index == 1 {
            assert_eq!(
                current, &remote["bots"][guid],
                "remote Priest changed without a decision: {after}"
            );
            continue;
        }
        let follows_crossing = index == 0 || (mode == 1 && index == 2);
        if follows_crossing {
            assert_eq!(current["intent"].as_array().unwrap().len(), 1, "{after}");
            assert_eq!(current["intent"][0]["destination_map"], "36");
            assert_eq!(current["intent"][0]["destination_instance"], "5098078");
            assert!(current["entity"].as_array().unwrap().is_empty());
            let generation = old["runner"]["generation"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                + 1;
            assert_eq!(current["runner"]["generation"], generation.to_string());
            assert_eq!(
                current["intent"][0]["controller_generation"],
                generation.to_string()
            );
            let checkpoint = current["runner"]["transfer_checkpoint"].as_str().unwrap();
            let member = if index == 2 {
                &party.bots[1]
            } else {
                &party.leader
            };
            assert!(
                checkpoint.contains(&format!("member_guid = {member}")),
                "{after}"
            );
            assert!(current["cast"].as_array().unwrap().is_empty());
            assert!(current["attack"].as_array().unwrap().is_empty());
        } else {
            assert!(current["intent"].as_array().unwrap().is_empty(), "{after}");
            assert_eq!(current["runner"]["transfer_checkpoint"], "(none = ())");
            assert_eq!(current["runner"]["generation"], old["runner"]["generation"]);
            assert_eq!(current["entity"].as_array().unwrap().len(), 1);
            assert_eq!(current["entity"][0]["map_id"], "0");
            assert_eq!(current["entity"][0]["instance_id"], "0");
            if index == 1 {
                for axis in ["x", "y", "z"] {
                    let previous: f64 = old["entity"][0][axis].as_str().unwrap().parse().unwrap();
                    let position: f64 = current["entity"][0][axis]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap();
                    assert!((position - previous).abs() <= 0.05, "Stay moved: {after}");
                }
            }
        }
    }
    if mode == 2 {
        let target_bot = &after["bots"][&party.bots[3]];
        assert!(
            target_bot["order"]["last_outcome"]
                .as_str()
                .unwrap()
                .contains("wrongPartition"),
            "{after}"
        );
        assert!(target_bot["cast"].as_array().unwrap().is_empty());
        assert!(target_bot["attack"].as_array().unwrap().is_empty());
        assert!(
            target_bot["movement"].as_array().unwrap().is_empty(),
            "stale Target leg survived: {after}"
        );
        assert_eq!(target_bot["runner"]["foreground"], "(none = ())", "{after}");
    }
}

#[test]
#[ignore = "requires composed PB-009/PB-010 Module, SpacetimeDB and playerbots"]
fn playerbots_only_follow_crosses_when_the_leader_leaves() {
    order_decisions(0);
}

#[test]
#[ignore = "requires composed PB-009/PB-010 Module, SpacetimeDB and playerbots"]
fn playerbots_assist_crosses_when_its_named_member_leaves() {
    order_decisions(1);
}

#[test]
#[ignore = "requires composed PB-009/PB-010 Module, SpacetimeDB and playerbots"]
fn playerbots_target_remains_local_when_the_target_is_remote() {
    order_decisions(2);
}
