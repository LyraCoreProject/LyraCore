//! Durable authenticated companion-order behavior on private Module databases.

mod support;

use std::collections::BTreeMap;
use support::{poll_until, Standalone, POLL_TIMEOUT};

const WARRIOR: &str = "1";
const PRIEST: &str = "5";
const MAGE: &str = "8";
const TANK: &str = "0";
const HEALER: &str = "1";
const DAMAGE: &str = "2";

struct OrdersFixture {
    node: Standalone,
    warrior: String,
    priest: String,
    mage: String,
    leader: String,
    enemies: Vec<String>,
    actor: String,
}

fn runner(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ))
    .into_iter()
    .next()
    .expect("runner explanation missing")
}

fn entity(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT guid, x, y, z, health, max_health, dead, target_guid FROM game_world_entity WHERE guid = {guid}"
    ))[0]
        .clone()
}

fn order(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {guid}"
    ))
    .into_iter()
    .next()
    .expect("companion order missing")
}

fn evidence(fixture: &OrdersFixture, case: &str) {
    let node = &fixture.node;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = root.join("packages/playerbots");
    let mut package_digest = blake3::Hasher::new();
    digest_files(&package, &mut package_digest);
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    let record = serde_json::json!({
        "case": case,
        "spacetimedb": "2.7.1",
        "rust": "1.93.0",
        "tested_core": git(root, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_dirty": !git(root, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "module_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "package_content_identity": package_digest.finalize().to_hex().to_string(),
        "orders": node.query_rows("SELECT * FROM pkg_playerbots_companion_order"),
        "intents": node.query_rows("SELECT * FROM game_party_command_intent"),
        "issuers": node.query_rows("SELECT * FROM game_party_command_issuer"),
        "dispatch_lanes": node.query_rows("SELECT * FROM game_party_command_dispatch_lane"),
        "receipts": node.query_rows("SELECT * FROM game_party_command_receipt"),
        "runners": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "quest_purposes": node.query_rows("SELECT * FROM pkg_playerbots_quest_objective"),
        "character_quests": node.query_rows("SELECT * FROM game_character_quest"),
        "group_members": node.query_rows("SELECT * FROM game_group_member"),
        "entities": node.query_rows("SELECT guid, entry, map_id, instance_id, x, y, z, health, max_health, dead, target_guid FROM game_world_entity"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "pending_casts": node.query_rows("SELECT * FROM game_pending_cast"),
        "melee": node.query_rows("SELECT * FROM game_melee_attack"),
        "splines": node.query_rows("SELECT * FROM game_creature_spline"),
        "auras": node.query_rows("SELECT id, caster_guid, target_guid, spell_id, eff_kind FROM game_aura"),
        "provisioning": node.query_rows("SELECT * FROM pkg_playerbots_provisioning"),
        "content": {
            "revision": "playerbots-starter-roles-v1",
            "provenance": "curated Core seeds and private role/order fixture rows",
            "imported_content": null,
        },
        "geometry": {
            "revision": "playerbots-synthetic-nav-v1",
            "client_geometry": null,
        },
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn digest_files(path: &std::path::Path, digest: &mut blake3::Hasher) {
    let mut children: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    digest.update(&(children.len() as u64).to_le_bytes());
    for child in children {
        let name = child.file_name().unwrap().as_encoded_bytes();
        digest.update(&(name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update(&[u8::from(child.is_dir())]);
        if child.is_dir() {
            digest_files(&child, digest);
        } else {
            let bytes = std::fs::read(&child).unwrap();
            digest.update(&(bytes.len() as u64).to_le_bytes());
            digest.update(&bytes);
        }
    }
}

struct PrecedingOrders {
    wasm: Vec<u8>,
    manifest: serde_json::Value,
}

fn preceding_orders() -> PrecedingOrders {
    let wasm_path = std::env::var_os("PLAYERBOTS_ORDERS_PRECEDING_WASM")
        .expect("PLAYERBOTS_ORDERS_PRECEDING_WASM must name the merged PB-008 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_ORDERS_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_ORDERS_PRECEDING_MANIFEST must describe that Wasm build");
    let core_path = std::env::var_os("PLAYERBOTS_ORDERS_PRECEDING_CORE")
        .expect("PLAYERBOTS_ORDERS_PRECEDING_CORE must name the clean merged Core checkout");
    let collection_path = std::env::var_os("PLAYERBOTS_ORDERS_PRECEDING_COLLECTION").expect(
        "PLAYERBOTS_ORDERS_PRECEDING_COLLECTION must name the clean merged Package checkout",
    );
    let core_path = std::path::Path::new(&core_path);
    let collection_path = std::path::Path::new(&collection_path);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let wasm = std::fs::read(&wasm_path).unwrap();
    let mut package_digest = blake3::Hasher::new();
    digest_files(&collection_path.join("playerbots"), &mut package_digest);
    for (field, expected) in [
        ("core", git(core_path, &["rev-parse", "HEAD"])),
        ("collection", git(collection_path, &["rev-parse", "HEAD"])),
        ("core_tree", git(core_path, &["rev-parse", "HEAD^{tree}"])),
        (
            "collection_tree",
            git(collection_path, &["rev-parse", "HEAD^{tree}"]),
        ),
        (
            "playerbots_tree",
            git(collection_path, &["rev-parse", "HEAD:playerbots"]),
        ),
        (
            "package_content_identity",
            package_digest.finalize().to_hex().to_string(),
        ),
    ] {
        assert_eq!(manifest[field], expected, "preceding manifest {field}");
    }
    assert_eq!(manifest["core_dirty"], false);
    assert_eq!(manifest["collection_dirty"], false);
    assert_eq!(manifest["rust"], "1.93.0");
    assert_eq!(manifest["spacetimedb"], "2.7.1");
    assert_eq!(manifest["target"], "wasm32-unknown-unknown");
    assert_eq!(manifest["profile"], "release");
    assert_eq!(manifest["features"], serde_json::json!(["debug_reducers"]));
    assert_eq!(manifest["wasm_bytes"].as_u64(), Some(wasm.len() as u64));
    assert!(git(core_path, &["status", "--porcelain"]).is_empty());
    assert!(git(collection_path, &["status", "--porcelain"]).is_empty());
    let sha256 = std::process::Command::new("sha256sum")
        .arg(&wasm_path)
        .output()
        .unwrap();
    assert!(sha256.status.success());
    let sha256 = String::from_utf8(sha256.stdout).unwrap();
    assert_eq!(
        sha256.split_whitespace().next().unwrap(),
        manifest["wasm_sha256"].as_str().unwrap()
    );
    PrecedingOrders { wasm, manifest }
}

fn stage_quest_geometry(node: &Standalone) {
    let x0 = lyracore_shared::terrain::cell_index(1_150.0).unwrap();
    let x1 = lyracore_shared::terrain::cell_index(1_400.0).unwrap();
    let y0 = lyracore_shared::terrain::cell_index(1_150.0).unwrap();
    let y1 = lyracore_shared::terrain::cell_index(1_250.0).unwrap();
    let mut rows = Vec::new();
    for cell_x in x0.min(x1)..=x0.max(x1) {
        for cell_y in y0.min(y1)..=y0.max(y1) {
            rows.push(format!("0,{cell_x},{cell_y},50,,"));
        }
    }
    node.assert_call("import_nav_chunks", &[&rows.join(";")]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
}

fn fixture(name: &str) -> OrdersFixture {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn", &["4", "1200", "1200", "50"]);
    let mut bots = node.query_rows("SELECT character_guid, class, role FROM pkg_playerbots_bot");
    bots.sort_by_key(|row| row["character_guid"].parse::<u64>().unwrap());
    let priest = bots
        .iter()
        .find(|row| row["class"] == PRIEST && row["role"] == HEALER)
        .unwrap()["character_guid"]
        .clone();
    let mage = bots
        .iter()
        .find(|row| row["class"] == MAGE && row["role"] == DAMAGE)
        .unwrap()["character_guid"]
        .clone();
    let warriors: Vec<_> = bots
        .iter()
        .filter(|row| row["class"] == WARRIOR && row["role"] == TANK)
        .map(|row| row["character_guid"].clone())
        .collect();
    let warrior = warriors[0].clone();
    let leader = warriors[1].clone();
    node.assert_call(
        "playerbots_fixture_roles_stage",
        &[&warrior, &priest, &mage, &leader],
    );
    for guid in [&warrior, &priest, &mage] {
        node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    }
    node.assert_call("provision_account", &[r#""PB009HUMAN""#, "[]", "[]"]);
    let account = node.query_rows("SELECT id FROM game_account WHERE username = 'PB009HUMAN'")[0]
        ["id"]
        .clone();
    node.assert_call("playerbots_fixture_orders_account", &[&leader, &account]);
    node.assert_call("claim_account", &[&account, &leader, "9009"]);
    let generation = node.query_rows(&format!(
        "SELECT generation FROM game_account_claim WHERE account_id = {account}"
    ))[0]["generation"]
        .clone();
    let ownership =
        format!(r#"{{"account_id":{account},"generation":{generation},"request_nonce":9009}}"#);
    let actor = format!(r#"{{"guid":{leader},"ownership":{{"some":{ownership}}}}}"#);
    let mut enemies: Vec<_> = node
        .query_rows(
            "SELECT guid FROM game_world_entity WHERE entry >= 5098001 AND entry <= 5098003",
        )
        .into_iter()
        .map(|row| row["guid"].clone())
        .collect();
    enemies.sort_by_key(|guid| guid.parse::<u64>().unwrap());
    let fixture = OrdersFixture {
        node,
        warrior,
        priest,
        mage,
        leader,
        enemies,
        actor,
    };
    evidence(&fixture, "staged");
    fixture
}

fn queue(fixture: &OrdersFixture, payload: &str) -> String {
    queue_as(&fixture.node, &fixture.actor, payload)
}

fn queue_as(node: &Standalone, actor: &str, payload: &str) -> String {
    let before: u64 = node
        .query_rows("SELECT id FROM game_party_command_intent")
        .iter()
        .map(|row| row["id"].parse().unwrap())
        .max()
        .unwrap_or(0);
    node.assert_call(
        "gw_client_command",
        &[actor, r#""playerbots.order""#, &format!(r#""{payload}""#)],
    );
    node.query_rows("SELECT id FROM game_party_command_intent")
        .into_iter()
        .map(|row| row["id"].parse::<u64>().unwrap())
        .filter(|id| *id > before)
        .max()
        .expect("authenticated command did not queue")
        .to_string()
}

fn set_party_as(fixture: &OrdersFixture, mode: u8, actor: &str) {
    fixture.node.assert_call(
        "playerbots_fixture_orders_party_as",
        &[
            &fixture.warrior,
            &fixture.priest,
            &fixture.mage,
            &fixture.leader,
            &mode.to_string(),
            actor,
        ],
    );
}

fn finish_refused_command(fixture: &OrdersFixture, payload: &str, token_base: u64) -> String {
    let intent = queue(fixture, payload);
    let token = (token_base + intent.parse::<u64>().unwrap()).to_string();
    fixture
        .node
        .assert_call("playerbots_fixture_command_apply", &[&intent, &token]);
    fixture
        .node
        .assert_call("playerbots_fixture_command_finish", &[&intent, &token]);
    intent
}

fn issue(fixture: &OrdersFixture, payload: &str, bot: &str, run_once: bool) -> String {
    let intent = queue(fixture, payload);
    fixture.node.assert_call(
        "playerbots_fixture_orders_drive",
        &[
            &intent,
            &(10_000 + intent.parse::<u64>().unwrap()).to_string(),
            bot,
            &run_once.to_string(),
        ],
    );
    intent
}

fn pass(node: &Standalone, guid: &str) {
    node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
}

fn select_and_engage(node: &Standalone, member_guid: &str, target_guid: &str) {
    node.assert_call(
        "playerbots_fixture_roles_select",
        &[member_guid, target_guid],
    );
    node.assert_call(
        "playerbots_fixture_roles_engage",
        &[member_guid, target_guid],
    );
}

fn movement_leg(node: &Standalone, guid: &str) -> Option<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT spline_id, dx, dy FROM game_creature_spline WHERE guid = {guid}"
    ))
    .into_iter()
    .next()
}

fn finish_movement(node: &Standalone, guid: &str, leg: &BTreeMap<String, String>) {
    let spline_id = leg["spline_id"].clone();
    let destination_x: f32 = leg["dx"].parse().unwrap();
    let destination_y: f32 = leg["dy"].parse().unwrap();
    assert!(poll_until(POLL_TIMEOUT, || {
        let position = entity(node, guid);
        let arrived = (position["x"].parse::<f32>().unwrap() - destination_x).abs() < 0.01
            && (position["y"].parse::<f32>().unwrap() - destination_y).abs() < 0.01;
        let leg_finished = node
            .query_rows(&format!(
                "SELECT spline_id FROM game_creature_spline WHERE guid = {guid}"
            ))
            .into_iter()
            .next()
            .is_none_or(|current| current["spline_id"] != spline_id);
        arrived && leg_finished
    }));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_orders_authenticate_follow_and_do_not_restart_a_retained_cast() {
    let fixture = fixture("playerbots-orders-follow");
    let node = &fixture.node;
    let first = issue(
        &fixture,
        &format!("follow|{}", fixture.priest),
        &fixture.priest,
        false,
    );
    let accepted = order(node, &fixture.priest);
    evidence(&fixture, "follow-accepted");
    assert!(accepted["order"].to_ascii_lowercase().contains("follow"));
    assert_eq!(accepted["issuer_guid"], fixture.leader);
    assert!(accepted["history"].contains(&first));
    assert!(accepted["history"].contains(&format!("issuer_guid = {}", fixture.leader)));

    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.mage, "25"],
    );
    node.assert_call("playerbots_fixture_roles_priest_mana", &[&fixture.priest]);
    let mut pending = Vec::new();
    let cast_started = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        pending = node.query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {} AND spell_id = 2050 AND target_guid = {}",
            fixture.priest, fixture.mage
        ));
        pending.len() == 1
    });
    evidence(&fixture, "follow-cast-started");
    assert!(
        cast_started,
        "Lesser Heal did not start: {:?}",
        runner(node, &fixture.priest)
    );
    assert_eq!(pending.len(), 1);
    let cast_id = pending[0]["scheduled_id"].clone();
    let before = runner(node, &fixture.priest);
    let second = issue(
        &fixture,
        &format!("follow|{}", fixture.priest),
        &fixture.priest,
        false,
    );
    let unchanged = order(node, &fixture.priest);
    let after = runner(node, &fixture.priest);
    let still_pending = node.query_rows(&format!(
        "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {}",
        fixture.priest
    ));
    evidence(&fixture, "follow-repeated");
    assert!(unchanged["last_outcome"]
        .to_ascii_lowercase()
        .contains("unchanged"));
    assert!(unchanged["history"].contains(&second));
    assert_eq!(unchanged["revision"], accepted["revision"]);
    assert_eq!(after["generation"], before["generation"]);
    assert_eq!(after["foreground"], before["foreground"]);
    assert_eq!(still_pending[0]["scheduled_id"], cast_id);
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {}",
            fixture.priest
        ))
        .is_empty()));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_each_issuer_fence_survives_intervening_leadership() {
    let fixture = fixture("playerbots-orders-causal-fence-leader-cycle");
    let node = &fixture.node;
    let older = queue(&fixture, &format!("stay|{}", fixture.warrior));
    let newer = queue(&fixture, &format!("follow|{}", fixture.warrior));
    let older_token = (15_000 + older.parse::<u64>().unwrap()).to_string();
    let newer_token = (16_000 + newer.parse::<u64>().unwrap()).to_string();
    node.assert_call("claim_party_command_intent", &[&older, &older_token]);
    node.assert_call("defer_party_command_intent", &[&older, &older_token]);
    node.assert_call("playerbots_fixture_command_drive", &[&newer, &newer_token]);
    let applied_newer = order(node, &fixture.warrior);
    let sequences = node.query_rows(&format!(
        "SELECT id, issuer_sequence FROM game_party_command_intent WHERE id = {older} OR id = {newer}"
    ));
    set_party_as(&fixture, 2, &fixture.actor);
    node.assert_call("provision_account", &[r#""PB009SECONDLEADER""#, "[]", "[]"]);
    let second_account = node
        .query_rows("SELECT id FROM game_account WHERE username = 'PB009SECONDLEADER'")[0]["id"]
        .clone();
    node.assert_call(
        "playerbots_fixture_orders_account",
        &[&fixture.mage, &second_account],
    );
    node.assert_call("claim_account", &[&second_account, &fixture.mage, "9010"]);
    let generation = node.query_rows(&format!(
        "SELECT generation FROM game_account_claim WHERE account_id = {second_account}"
    ))[0]["generation"]
        .clone();
    let second_actor = format!(
        r#"{{"guid":{},"ownership":{{"some":{{"account_id":{},"generation":{},"request_nonce":9010}}}}}}"#,
        fixture.mage, second_account, generation
    );
    let intervening = queue_as(node, &second_actor, &format!("stay|{}", fixture.warrior));
    let intervening_token = (17_000 + intervening.parse::<u64>().unwrap()).to_string();
    node.assert_call("claim_party_command_intent", &[&older, &older_token]);
    node.assert_call("defer_party_command_intent", &[&older, &older_token]);
    node.assert_call(
        "playerbots_fixture_command_drive",
        &[&intervening, &intervening_token],
    );
    let applied_intervening = order(node, &fixture.warrior);
    assert_eq!(applied_intervening["issuer_guid"], fixture.mage);
    set_party_as(&fixture, 0, &second_actor);
    node.assert_call("playerbots_fixture_command_apply", &[&older, &older_token]);
    node.assert_call("playerbots_fixture_command_finish", &[&older, &older_token]);
    let after_delayed = order(node, &fixture.warrior);
    let receipt = node.query_rows(&format!(
        "SELECT outcome FROM game_party_command_receipt WHERE intent_id = {older}"
    ));
    evidence(&fixture, "older-order-superseded-after-intervening-leader");
    assert_eq!(sequences.len(), 2);
    let older_sequence: u64 = sequences.iter().find(|row| row["id"] == older).unwrap()
        ["issuer_sequence"]
        .parse()
        .unwrap();
    let newer_sequence: u64 = sequences.iter().find(|row| row["id"] == newer).unwrap()
        ["issuer_sequence"]
        .parse()
        .unwrap();
    assert!(older_sequence < newer_sequence);
    assert!(applied_newer["order"]
        .to_ascii_lowercase()
        .contains("follow"));
    assert_eq!(after_delayed, applied_intervening);
    assert!(after_delayed["order"].to_ascii_lowercase().contains("stay"));
    assert!(after_delayed["issuer_fences"].contains(&format!(
        "issuer_guid = {}, sequence = {}",
        fixture.leader, newer_sequence
    )));
    assert!(receipt[0]["outcome"]
        .to_ascii_lowercase()
        .contains("superseded"));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_stay_blocks_heal_positioning_but_allows_in_range_healing_and_survival() {
    let fixture = fixture("playerbots-orders-stay");
    let node = &fixture.node;
    issue(
        &fixture,
        &format!("stay|{}", fixture.priest),
        &fixture.priest,
        false,
    );
    let held = entity(node, &fixture.priest);
    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.leader, "1300", "1200"],
    );
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.leader, "25"],
    );
    pass(node, &fixture.priest);
    let distant = runner(node, &fixture.priest);
    evidence(&fixture, "stay-distant-wounded-leader");
    assert!(distant["chosen"].to_ascii_lowercase().contains("stay"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_creature_spline WHERE guid = {}",
            fixture.priest
        ))
        .is_empty());
    assert_eq!(entity(node, &fixture.priest)["x"], held["x"]);

    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.leader, "1200", "1200"],
    );
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.leader, "25"],
    );
    let before = entity(node, &fixture.leader)["health"]
        .parse::<u32>()
        .unwrap();
    pass(node, &fixture.priest);
    assert!(poll_until(POLL_TIMEOUT, || entity(node, &fixture.leader)
        ["health"]
        .parse::<u32>()
        .unwrap()
        > before));
    evidence(&fixture, "stay-in-range-heal");
    assert_eq!(entity(node, &fixture.priest)["x"], held["x"]);

    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.priest, "1150", "1200"],
    );
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.priest, "1"],
    );
    node.assert_call("playerbots_fixture_runner_survival", &[&fixture.priest]);
    pass(node, &fixture.priest);
    let survival = runner(node, &fixture.priest);
    evidence(&fixture, "stay-survival-interrupt");
    assert!(survival["chosen"].to_ascii_lowercase().contains("survival"));
    assert!(survival["chosen"].to_ascii_lowercase().contains("move"));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_stay_blocks_a_recovery_position_selected_after_normal_recovery() {
    let fixture = fixture("playerbots-orders-stay-recovery");
    let node = &fixture.node;
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
    node.assert_call(
        "playerbots_quest_loop_fixture_stage_named",
        &[&fixture.priest],
    );
    for guid in [&fixture.warrior, &fixture.mage] {
        node.assert_call("playerbots_fixture_companion_health", &[guid, "100"]);
    }
    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.leader, "1240", "1200"],
    );
    issue(
        &fixture,
        &format!("follow|{}", fixture.priest),
        &fixture.priest,
        false,
    );
    node.assert_call(
        "playerbots_recovery_fixture_block_companion",
        &[&fixture.priest],
    );
    let recovery_selected = poll_until(std::time::Duration::from_secs(14), || {
        pass(node, &fixture.priest);
        let selected = runner(node, &fixture.priest)["chosen"]
            .to_ascii_lowercase()
            .contains("recoveryposition");
        if !selected {
            std::thread::sleep(std::time::Duration::from_millis(1_000));
        }
        selected
    });
    let before_stay = runner(node, &fixture.priest);
    evidence(&fixture, "stay-recovery-position-selected");
    assert!(recovery_selected, "{before_stay:?}");
    assert!(before_stay["recovery"]
        .to_ascii_lowercase()
        .contains("position = (some"));

    let held = entity(node, &fixture.priest);
    issue(
        &fixture,
        &format!("stay|{}", fixture.priest),
        &fixture.priest,
        false,
    );
    pass(node, &fixture.priest);
    let after_stay = runner(node, &fixture.priest);
    let applied = order(node, &fixture.priest);
    std::thread::sleep(std::time::Duration::from_millis(1_000));
    let after_wait = entity(node, &fixture.priest);
    evidence(&fixture, "stay-blocks-recovery-position");
    assert!(after_stay["chosen"].to_ascii_lowercase().contains("hold"));
    assert_eq!(applied["active"], "true", "{applied:?}");
    assert!(applied["order"].contains("stay"), "{applied:?}");
    assert_eq!(after_stay["foreground"], "(none = ())", "{after_stay:?}");
    assert!(
        after_stay["recovery"].contains("active = (none"),
        "{after_stay:?}"
    );
    assert!(after_stay["recovery"]
        .to_ascii_lowercase()
        .contains("position = (some"));
    assert_eq!(after_wait["x"], held["x"]);
    assert_eq!(after_wait["y"], held["y"]);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_assist_uses_only_the_named_members_actual_fight() {
    let fixture = fixture("playerbots-orders-assist");
    let node = &fixture.node;
    let chosen = &fixture.enemies[1];
    let unrelated = &fixture.enemies[0];
    select_and_engage(node, &fixture.leader, chosen);
    issue(
        &fixture,
        &format!("assist|{}|{}", fixture.warrior, fixture.leader),
        &fixture.warrior,
        true,
    );
    let initial_leg = movement_leg(node, &fixture.warrior);
    let approach = runner(node, &fixture.warrior);
    evidence(&fixture, "assist-named-fight-approach");
    assert!(approach["chosen"].contains("move"));
    assert!(approach["chosen"].contains(chosen.as_str()));
    let initial_leg = initial_leg.expect("expected a real movement leg");
    finish_movement(node, &fixture.warrior, &initial_leg);
    pass(node, &fixture.warrior);
    let melee = node.query_rows(&format!(
        "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "assist-named-fight");
    assert_eq!(melee.len(), 1);
    assert_eq!(melee[0]["target_guid"], *chosen);
    assert_ne!(melee[0]["target_guid"], *unrelated);

    issue(
        &fixture,
        &format!("assist|{}|{}", fixture.mage, fixture.priest),
        &fixture.mage,
        true,
    );
    evidence(&fixture, "assist-no-named-fight");
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.mage
        ))
        .is_empty());
    assert!(!runner(node, &fixture.mage)["chosen"]
        .to_ascii_lowercase()
        .contains("damagefight"));

    let retained_revision = order(node, &fixture.warrior)["revision"].clone();
    node.assert_call("playerbots_fixture_roles_select", &[&fixture.leader, "0"]);
    pass(node, &fixture.warrior);
    evidence(&fixture, "assist-member-has-no-fight");
    assert!(order(node, &fixture.warrior)["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetunavailable"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        ))
        .is_empty());

    select_and_engage(node, &fixture.leader, chosen);
    pass(node, &fixture.warrior);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        ))[0]["target_guid"],
        *chosen
    );

    node.assert_call(
        "playerbots_fixture_roles_control",
        &[&fixture.leader, chosen, "50020"],
    );
    pass(node, &fixture.warrior);
    evidence(&fixture, "assist-target-controlled");
    assert!(order(node, &fixture.warrior)["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetcontrolled"));
    node.assert_call(
        "playerbots_fixture_roles_clear_control",
        &[&fixture.leader, chosen],
    );
    select_and_engage(node, &fixture.leader, chosen);
    pass(node, &fixture.warrior);

    node.assert_call("playerbots_fixture_orders_target_state", &[chosen, "1"]);
    pass(node, &fixture.warrior);
    evidence(&fixture, "assist-target-remote");
    assert!(order(node, &fixture.warrior)["last_outcome"]
        .to_ascii_lowercase()
        .contains("wrongpartition"));
    node.assert_call("playerbots_fixture_orders_target_state", &[chosen, "2"]);
    select_and_engage(node, &fixture.leader, chosen);
    pass(node, &fixture.warrior);

    node.assert_call("playerbots_fixture_orders_target_state", &[chosen, "0"]);
    pass(node, &fixture.warrior);
    evidence(&fixture, "assist-target-dead");
    assert!(order(node, &fixture.warrior)["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetdead"));
    node.assert_call("playerbots_fixture_orders_target_state", &[chosen, "2"]);
    select_and_engage(node, &fixture.leader, chosen);
    pass(node, &fixture.warrior);

    node.assert_call("playerbots_fixture_roles_despawn", &[chosen]);
    pass(node, &fixture.warrior);
    evidence(&fixture, "assist-target-missing");
    assert!(order(node, &fixture.warrior)["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetunavailable"));
    node.assert_call("playerbots_fixture_orders_restore_target", &[chosen]);
    select_and_engage(node, &fixture.leader, chosen);
    pass(node, &fixture.warrior);
    let recovered_leg = movement_leg(node, &fixture.warrior);
    evidence(&fixture, "assist-target-recovered");
    let recovered = order(node, &fixture.warrior);
    assert!(recovered["last_outcome"]
        .to_ascii_lowercase()
        .contains("applied"));
    assert_eq!(recovered["revision"], retained_revision);
    let recovered_approach = runner(node, &fixture.warrior);
    assert!(recovered_approach["chosen"].contains("move"));
    assert!(recovered_approach["chosen"].contains(chosen.as_str()));
    let recovered_leg = recovered_leg.expect("expected a real movement leg");
    finish_movement(node, &fixture.warrior, &recovered_leg);
    pass(node, &fixture.warrior);
    evidence(&fixture, "assist-target-recovered-melee");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        ))[0]["target_guid"],
        *chosen
    );

    node.assert_call(
        "playerbots_fixture_orders_remove_member_body",
        &[&fixture.leader],
    );
    pass(node, &fixture.warrior);
    let missing_member = order(node, &fixture.warrior);
    evidence(&fixture, "assist-member-body-unavailable");
    assert!(missing_member["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetunavailable"));
    assert_eq!(missing_member["revision"], retained_revision);
    assert_eq!(missing_member["active"], "true");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_assist_honors_named_companions_current_target_order() {
    let fixture = fixture("playerbots-orders-assist-companion-target");
    let node = &fixture.node;
    let first = &fixture.enemies[0];
    let second = &fixture.enemies[1];
    assert_eq!(entity(node, &fixture.priest)["target_guid"], "0");
    issue(
        &fixture,
        &format!("target|{}|{first}", fixture.priest),
        &fixture.priest,
        false,
    );
    issue(
        &fixture,
        &format!("assist|{}|{}", fixture.mage, fixture.priest),
        &fixture.mage,
        true,
    );
    let assert_target = |target: &str| {
        assert_eq!(
            runner(node, &fixture.mage)["companion_fight_target_guid"],
            format!("(some = {target})")
        );
        assert!(order(node, &fixture.mage)["last_outcome"]
            .to_ascii_lowercase()
            .contains("applied"));
    };
    assert_target(first);
    evidence(&fixture, "assist-companion-target-before-attack");
    issue(
        &fixture,
        &format!("target|{}|{second}", fixture.priest),
        &fixture.priest,
        false,
    );
    pass(node, &fixture.mage);
    assert_target(second);
    evidence(&fixture, "assist-companion-target-replaced");

    select_and_engage(node, &fixture.leader, first);
    let assert_refusal = |outcome: &str| {
        assert!(order(node, &fixture.mage)["last_outcome"]
            .to_ascii_lowercase()
            .contains(outcome));
        assert_eq!(
            runner(node, &fixture.mage)["companion_fight_target_guid"],
            "(none = ())"
        );
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM game_pending_cast WHERE caster_guid = {}",
                fixture.mage
            ))
            .is_empty());
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
                fixture.mage
            ))
            .is_empty());
    };
    node.assert_call(
        "playerbots_fixture_roles_control",
        &[&fixture.leader, second, "50020"],
    );
    pass(node, &fixture.mage);
    assert_refusal("targetcontrolled");
    node.assert_call(
        "playerbots_fixture_roles_clear_control",
        &[&fixture.leader, second],
    );
    for (mode, outcome) in [("1", "wrongpartition"), ("0", "targetdead")] {
        node.assert_call("playerbots_fixture_orders_target_state", &[second, mode]);
        pass(node, &fixture.mage);
        assert_refusal(outcome);
        node.assert_call("playerbots_fixture_orders_target_state", &[second, "2"]);
    }
    pass(node, &fixture.mage);
    assert_target(second);
    node.assert_call(
        "playerbots_select_controller",
        &[&fixture.priest, "{\"recordOnly\":[]}"],
    );
    pass(node, &fixture.mage);
    assert_refusal("targetunavailable");
    evidence(&fixture, "assist-companion-target-authority-ended");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_target_pulls_only_the_exact_eligible_creature_and_releases_controlled_work() {
    let fixture = fixture("playerbots-orders-target");
    let node = &fixture.node;
    let exact = &fixture.enemies[2];
    let nearer = &fixture.enemies[0];
    issue(
        &fixture,
        &format!("target|{}|{exact}", fixture.warrior),
        &fixture.warrior,
        true,
    );
    let approach = runner(node, &fixture.warrior);
    let leg = movement_leg(node, &fixture.warrior);
    evidence(&fixture, "target-exact-approach");
    assert!(approach["chosen"].contains(exact));
    assert!(approach["chosen"].contains("move"));
    let leg = leg.expect("exact Target did not start an approach");
    finish_movement(node, &fixture.warrior, &leg);
    let mut melee = Vec::new();
    let pulled = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.warrior);
        melee = node.query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        ));
        melee.len() == 1 && melee[0]["target_guid"] == *exact
    });
    evidence(&fixture, "target-exact-pull");
    assert!(
        pulled,
        "exact Target did not enter melee: {:?}",
        runner(node, &fixture.warrior)
    );
    assert_eq!(melee.len(), 1);
    assert_eq!(melee[0]["target_guid"], *exact);
    assert_ne!(melee[0]["target_guid"], *nearer);
    assert!(runner(node, &fixture.warrior)["chosen"].contains(exact));

    node.assert_call(
        "playerbots_fixture_roles_control",
        &[&fixture.leader, exact, "50020"],
    );
    pass(node, &fixture.warrior);
    let controlled = order(node, &fixture.warrior);
    evidence(&fixture, "target-controlled");
    assert!(controlled["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetcontrolled"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        ))
        .is_empty());
    assert!(!runner(node, &fixture.warrior)["chosen"].contains(nearer));

    let dead = &fixture.enemies[1];
    node.assert_call("playerbots_fixture_orders_target_state", &[dead, "0"]);
    let dead_intent = finish_refused_command(
        &fixture,
        &format!("target|{}|{dead}", fixture.warrior),
        25_000,
    );
    let remote = &fixture.enemies[0];
    node.assert_call("playerbots_fixture_orders_target_state", &[remote, "1"]);
    let remote_intent = finish_refused_command(
        &fixture,
        &format!("target|{}|{remote}", fixture.warrior),
        26_000,
    );
    let refusals = node.query_rows(&format!(
        "SELECT intent_id, outcome FROM game_party_command_receipt WHERE intent_id = {dead_intent} OR intent_id = {remote_intent}"
    ));
    evidence(&fixture, "target-dead-and-remote-refused");
    assert_eq!(refusals.len(), 2, "{refusals:?}");
    assert!(refusals.iter().any(|row| {
        row["intent_id"] == dead_intent
            && row["outcome"].to_ascii_lowercase().contains("targetdead")
    }));
    assert!(refusals.iter().any(|row| {
        row["intent_id"] == remote_intent
            && row["outcome"]
                .to_ascii_lowercase()
                .contains("wrongpartition")
    }));
    assert_eq!(
        order(node, &fixture.warrior)["revision"],
        controlled["revision"]
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_orders_reject_forged_issuer_ownership_before_queueing() {
    let fixture = fixture("playerbots-orders-forged-issuer");
    let node = &fixture.node;
    let before = node
        .query_rows("SELECT id FROM game_party_command_intent")
        .len();
    let forged = format!(
        r#"{{"guid":{},"ownership":{{"some":{{"account_id":1,"generation":1,"request_nonce":1}}}}}}"#,
        fixture.leader
    );
    let refused = node.call(
        "gw_client_command",
        &[
            &forged,
            r#""playerbots.order""#,
            &format!(r#""follow|{}""#, fixture.warrior),
        ],
    );
    evidence(&fixture, "forged-ownership-refused");
    assert!(!refused.status.success());
    assert_eq!(
        node.query_rows("SELECT id FROM game_party_command_intent")
            .len(),
        before
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_assist_invalidates_when_the_named_member_leaves() {
    let fixture = fixture("playerbots-orders-assist-member-left");
    let node = &fixture.node;
    issue(
        &fixture,
        &format!("assist|{}|{}", fixture.warrior, fixture.priest),
        &fixture.warrior,
        false,
    );
    set_party_as(&fixture, 3, &fixture.actor);
    pass(node, &fixture.warrior);
    evidence(&fixture, "assisted-member-left");
    assert_eq!(order(node, &fixture.warrior)["active"], "false");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_orders_stop_when_the_bot_is_reclaimed_by_an_account() {
    let fixture = fixture("playerbots-orders-bot-reclaimed");
    let node = &fixture.node;
    let bot_account = node.query_rows(&format!(
        "SELECT account_id FROM game_character WHERE guid = {}",
        fixture.warrior
    ))[0]["account_id"]
        .clone();
    node.assert_call("claim_account", &[&bot_account, &fixture.warrior, "9010"]);
    let reclaimed =
        finish_refused_command(&fixture, &format!("follow|{}", fixture.warrior), 30_000);
    let reclaimed_receipt = node.query_rows(&format!(
        "SELECT outcome FROM game_party_command_receipt WHERE intent_id = {reclaimed}"
    ));
    let reclaimed_intent = node.query_rows(&format!(
        "SELECT state FROM game_party_command_intent WHERE id = {reclaimed}"
    ));
    evidence(&fixture, "bot-reclaimed-by-account");
    assert_eq!(reclaimed_receipt.len(), 1);
    assert!(reclaimed_receipt[0]["outcome"]
        .to_ascii_lowercase()
        .contains("wrongaccount"));
    assert!(reclaimed_intent[0]["state"]
        .to_ascii_lowercase()
        .contains("wrongaccount"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.warrior
        ))
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_order_target_gate_rechecks_body_and_consent_after_authority() {
    let fixture = fixture("playerbots-orders-final-target-gate");
    let node = &fixture.node;

    let suppressed = queue(&fixture, &format!("follow|{}", fixture.warrior));
    let suppressed_token = 40_000 + suppressed.parse::<u64>().unwrap();
    node.assert_call(
        "playerbots_fixture_command_apply_after_gate_change",
        &[&suppressed, &suppressed_token.to_string(), "2"],
    );
    node.assert_call(
        "playerbots_fixture_command_finish",
        &[&suppressed, &suppressed_token.to_string()],
    );
    let suppressed_receipt = node.query_rows(&format!(
        "SELECT outcome FROM game_party_command_receipt WHERE intent_id = {suppressed}"
    ));
    evidence(&fixture, "consent-suppressed-after-authority");
    assert!(suppressed_receipt[0]["outcome"]
        .to_ascii_lowercase()
        .contains("suppressed"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.warrior
        ))
        .is_empty());

    let missing = queue(&fixture, &format!("follow|{}", fixture.mage));
    let missing_token = 41_000 + missing.parse::<u64>().unwrap();
    node.assert_call(
        "playerbots_fixture_command_apply_after_gate_change",
        &[&missing, &missing_token.to_string(), "1"],
    );
    node.assert_call(
        "playerbots_fixture_command_finish",
        &[&missing, &missing_token.to_string()],
    );
    let missing_receipt = node.query_rows(&format!(
        "SELECT outcome FROM game_party_command_receipt WHERE intent_id = {missing}"
    ));
    evidence(&fixture, "body-removed-after-authority");
    assert!(missing_receipt[0]["outcome"]
        .to_ascii_lowercase()
        .contains("missingbot"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.mage
        ))
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_record_only_refuses_an_order_without_gameplay_writes() {
    let fixture = fixture("playerbots-orders-record-only");
    let node = &fixture.node;
    node.assert_call(
        "playerbots_select_controller",
        &[&fixture.warrior, "{\"recordOnly\":[]}"],
    );
    let entity_before = entity(node, &fixture.warrior);
    let actions_before = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_action WHERE character_guid = {}",
        fixture.warrior
    ));
    let pending_before = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {}",
        fixture.warrior
    ));
    let melee_before = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
        fixture.warrior
    ));
    let movement_before = node.query_rows(&format!(
        "SELECT * FROM game_creature_spline WHERE guid = {}",
        fixture.warrior
    ));
    let suppressed =
        finish_refused_command(&fixture, &format!("follow|{}", fixture.warrior), 31_000);
    let suppressed_receipt = node.query_rows(&format!(
        "SELECT outcome FROM game_party_command_receipt WHERE intent_id = {suppressed}"
    ));
    evidence(&fixture, "record-only-order-suppressed");
    assert!(suppressed_receipt[0]["outcome"]
        .to_ascii_lowercase()
        .contains("suppressed"));
    assert_eq!(entity(node, &fixture.warrior), entity_before);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM pkg_playerbots_action WHERE character_guid = {}",
            fixture.warrior
        )),
        actions_before
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {}",
            fixture.warrior
        )),
        pending_before
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        )),
        melee_before
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_creature_spline WHERE guid = {}",
            fixture.warrior
        )),
        movement_before
    );
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.warrior
        ))
        .is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_record_only_runtime_refusal_preserves_gameplay() {
    let fixture = fixture("playerbots-orders-record-only-runtime-refusal");
    let node = &fixture.node;
    let target = &fixture.enemies[1];
    node.assert_call(
        "playerbots_fixture_roles_engage",
        &[&fixture.leader, target],
    );
    issue(
        &fixture,
        &format!("assist|{}|{}", fixture.warrior, fixture.leader),
        &fixture.warrior,
        true,
    );
    node.assert_call(
        "playerbots_select_controller",
        &[&fixture.warrior, "{\"recordOnly\":[]}"],
    );
    node.assert_call(
        "playerbots_fixture_roles_record_only_attack",
        &[&fixture.warrior, target],
    );
    node.assert_call(
        "playerbots_fixture_orders_remove_member_body",
        &[&fixture.leader],
    );
    let entity_before = entity(node, &fixture.warrior);
    let actions_before = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_action WHERE character_guid = {}",
        fixture.warrior
    ));
    let pending_before = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {}",
        fixture.warrior
    ));
    let melee_before = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
        fixture.warrior
    ));
    let movement_before = node.query_rows(&format!(
        "SELECT * FROM game_creature_spline WHERE guid = {}",
        fixture.warrior
    ));
    pass(node, &fixture.warrior);
    let recorded = order(node, &fixture.warrior);
    evidence(&fixture, "record-only-runtime-refusal");
    assert!(recorded["last_outcome"]
        .to_ascii_lowercase()
        .contains("targetunavailable"));
    assert_eq!(entity(node, &fixture.warrior), entity_before);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM pkg_playerbots_action WHERE character_guid = {}",
            fixture.warrior
        )),
        actions_before
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {}",
            fixture.warrior
        )),
        pending_before
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.warrior
        )),
        melee_before
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_creature_spline WHERE guid = {}",
            fixture.warrior
        )),
        movement_before
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_order_clears_when_leadership_changes_and_preserves_the_role() {
    let fixture = fixture("playerbots-orders-leadership-change");
    let node = &fixture.node;
    issue(
        &fixture,
        &format!("follow|{}", fixture.warrior),
        &fixture.warrior,
        false,
    );
    set_party_as(&fixture, 2, &fixture.actor);
    pass(node, &fixture.warrior);
    evidence(&fixture, "leadership-lost");
    assert_eq!(order(node, &fixture.warrior)["active"], "false");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {}",
            fixture.warrior
        ))[0]["role"],
        TANK
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_human_party_suspends_then_rechecks_the_retained_solo_quest() {
    let fixture = fixture("playerbots-orders-retained-solo-quest");
    let node = &fixture.node;
    set_party_as(&fixture, 1, &fixture.actor);
    let solo_membership = node.query_rows(&format!(
        "SELECT * FROM game_group_member WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "solo-party-left");
    assert!(solo_membership.is_empty());
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
    node.assert_call("playerbots_quest_fixture_stage", &[&fixture.warrior]);
    node.assert_call(
        "playerbots_quest_fixture_admit_accept",
        &[&fixture.warrior, "7"],
    );
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.warrior, "100"],
    );
    let mut solo_runner = runner(node, &fixture.warrior);
    let quest_selected = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.warrior);
        solo_runner = runner(node, &fixture.warrior);
        solo_runner["objective"]
            .to_ascii_lowercase()
            .contains("quest")
    });
    let retained = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "solo-quest-before-party");
    assert!(
        quest_selected,
        "solo Quest did not become active: {solo_runner:?}"
    );
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0]["quest_entry"], "7");
    let retained_identity = retained[0]["runner_objective_identity"].clone();

    set_party_as(&fixture, 0, &fixture.actor);
    issue(
        &fixture,
        &format!("follow|{}", fixture.warrior),
        &fixture.warrior,
        true,
    );
    let companion_runner = runner(node, &fixture.warrior);
    let suspended = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "solo-quest-suspended-by-party");
    assert!(companion_runner["objective"]
        .to_ascii_lowercase()
        .contains("companion"));
    assert_eq!(suspended, retained);
    assert!(!order(node, &fixture.warrior)["order"].is_empty());

    set_party_as(&fixture, 1, &fixture.actor);
    let departed_membership = node.query_rows(&format!(
        "SELECT * FROM game_group_member WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "party-left-after-follow");
    assert!(departed_membership.is_empty());
    let mut resumed_runner = runner(node, &fixture.warrior);
    let resumed = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.warrior);
        resumed_runner = runner(node, &fixture.warrior);
        order(node, &fixture.warrior)["active"] == "false"
            && resumed_runner["objective"].contains("kind = (quest = ())")
    });
    let rechecked = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "solo-quest-rechecked-after-party");
    assert!(
        resumed,
        "retained Quest did not resume after departure: {resumed_runner:?}"
    );
    assert_eq!(order(node, &fixture.warrior)["active"], "false");
    assert_eq!(rechecked.len(), 1);
    assert_eq!(rechecked[0]["quest_entry"], "7");
    let resumed_identity = rechecked[0]["runner_objective_identity"].clone();
    assert_ne!(resumed_identity, retained_identity);
    assert!(resumed_runner["objective"].contains(&format!(
        "identity = {resumed_identity}, kind = (quest = ())"
    )));
    assert!(!resumed_runner["foreground"]
        .to_ascii_lowercase()
        .contains("follow"));
    assert!(!resumed_runner["chosen"]
        .to_ascii_lowercase()
        .contains("follow"));
    assert!(resumed_runner["companion_leader_guid"]
        .to_ascii_lowercase()
        .contains("none"));
    let mut retained_purpose = retained[0].clone();
    retained_purpose.remove("runner_objective_identity");
    let mut rechecked_purpose = rechecked[0].clone();
    rechecked_purpose.remove("runner_objective_identity");
    assert_eq!(rechecked_purpose, retained_purpose);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {}",
            fixture.warrior
        ))[0]["role"],
        TANK
    );
}

#[test]
#[ignore = "requires the merged PB-008 Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_populated_pb008_state_adds_empty_order_state_without_changing_current_work() {
    let preceding = preceding_orders();
    assert_ne!(
        blake3::hash(&preceding.wasm),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start("playerbots-orders-pb008-migration");
    node.publish_module_bytes(&preceding.wasm);
    let imports =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(imports.len(), 1, "unexpected preceding Import Catalogue");
    assert_eq!(imports[0]["family"], "weather_seed");
    assert_eq!(imports[0]["source_sha"], "");
    assert_eq!(imports[0]["file_hash"], "");
    assert_eq!(imports[0]["row_count"], "2");
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_quest_geometry(&node);
    node.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", "0"]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_select_controller", &[&guid, "{\"cohort\":[]}"]);
    node.assert_call("debug_learn_spell", &[&guid, "355"]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    node.assert_sql(&format!(
        "UPDATE pkg_playerbots_provisioning SET next_repair_micros = 9223372036854775807 WHERE character_guid = {guid}"
    ));
    node.assert_call("playerbots_quest_fixture_stage", &[&guid]);
    node.assert_call(
        "playerbots_quest_fixture_move_creature_spawn",
        &["6", "1230"],
    );
    node.assert_call("playerbots_quest_fixture_refresh", &[]);
    node.assert_call("playerbots_quest_fixture_admit_accept", &[&guid, "7"]);
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "true"]);
    node.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 5090100");
    let foreground_started = poll_until(POLL_TIMEOUT, || {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let runner = runner(&node, &guid);
        runner["objective"].to_ascii_lowercase().contains("quest")
            && runner["foreground"].to_ascii_lowercase().contains("cast")
            && runner["foreground"].contains("spell = 5090100")
    });
    node.assert_call("playerbots_fixture_freeze", &[&guid]);
    let preceding_runner = runner(&node, &guid);
    let preceding_quest = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
    ));
    let preceding_cast = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"
    ));
    let preceding_bot = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_bot WHERE character_guid = {guid}"
    ));
    let preceding_provisioning = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
    ));
    std::fs::write(
        support::log_dir().join(format!(
            "{}-orders-populated-predecessor.json",
            node.shard_name()
        )),
        serde_json::to_vec_pretty(&serde_json::json!({
            "preceding_build": preceding.manifest.clone(),
            "runner": preceding_runner,
            "quest_purpose": preceding_quest,
            "pending_cast": preceding_cast,
            "bot": preceding_bot,
            "provisioning": preceding_provisioning,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        foreground_started,
        "preceding foreground cast did not start"
    );
    assert_eq!(preceding_quest.len(), 1);
    assert_eq!(preceding_quest[0]["quest_entry"], "7");
    assert_eq!(preceding_cast.len(), 1);
    assert!(preceding_runner["objective"]
        .to_ascii_lowercase()
        .contains("quest"));
    assert!(preceding_runner["foreground"]
        .to_ascii_lowercase()
        .contains("cast"));
    assert!(preceding_runner["recovery"]
        .to_ascii_lowercase()
        .contains("some"));

    node.publish_module();
    let upgraded_runner = runner(&node, &guid);
    let upgraded_quest = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {guid}"
    ));
    let upgraded_cast = node.query_rows(&format!(
        "SELECT * FROM game_pending_cast WHERE caster_guid = {guid}"
    ));
    let upgraded_bot = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_bot WHERE character_guid = {guid}"
    ));
    let upgraded_provisioning = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
    ));
    let orders = node.query_rows("SELECT * FROM pkg_playerbots_companion_order");
    std::fs::write(
        support::log_dir().join(format!(
            "{}-orders-populated-migration.json",
            node.shard_name()
        )),
        serde_json::to_vec_pretty(&serde_json::json!({
            "preceding_build": preceding.manifest,
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "preceding_runner": preceding_runner,
            "upgraded_runner": upgraded_runner,
            "preceding_quest_purpose": preceding_quest,
            "upgraded_quest_purpose": upgraded_quest,
            "preceding_pending_cast": preceding_cast,
            "upgraded_pending_cast": upgraded_cast,
            "preceding_bot": preceding_bot,
            "upgraded_bot": upgraded_bot,
            "preceding_provisioning": preceding_provisioning,
            "upgraded_provisioning": upgraded_provisioning,
            "orders": orders,
        }))
        .unwrap(),
    )
    .unwrap();
    for (field, value) in &preceding_runner {
        assert_eq!(&upgraded_runner[field], value, "runner field {field}");
    }
    assert_eq!(upgraded_runner["companion_order_revision"], "0");
    assert_eq!(upgraded_quest, preceding_quest);
    assert_eq!(upgraded_cast, preceding_cast);
    assert_eq!(upgraded_bot, preceding_bot);
    assert_eq!(upgraded_provisioning, preceding_provisioning);
    assert!(orders.is_empty());
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_role_and_order_survive_death_and_resume_after_resurrection() {
    let fixture = fixture("playerbots-orders-death");
    let node = &fixture.node;
    issue(
        &fixture,
        &format!("follow|{}", fixture.warrior),
        &fixture.warrior,
        false,
    );
    node.assert_call(
        "playerbots_fixture_runner_damage",
        &[&fixture.warrior, &fixture.enemies[0], "100000"],
    );
    evidence(&fixture, "order-dead");
    assert_eq!(entity(node, &fixture.warrior)["dead"], "true");
    pass(node, &fixture.warrior);
    let resurrected = runner(node, &fixture.warrior);
    evidence(&fixture, "order-death-resurrection");
    assert!(resurrected["chosen"]
        .to_ascii_lowercase()
        .contains("resurrection"));
    assert!(!node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.warrior
        ))
        .is_empty());
    assert_eq!(
        node.query_rows(&format!(
            "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {}",
            fixture.warrior
        ))[0]["role"],
        TANK
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_command_receipts_survive_history_rollover_and_wait_for_capacity() {
    let fixture = fixture("playerbots-orders-receipts");
    let node = &fixture.node;
    let payload = format!("follow|{}", fixture.warrior);
    for _ in 0..32 {
        issue(&fixture, &payload, &fixture.warrior, false);
    }
    let before = order(node, &fixture.warrior);
    evidence(&fixture, "receipt-window-full");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT id FROM game_party_command_receipt WHERE bot_guid = {}",
            fixture.warrior
        ))
        .len(),
        32
    );
    assert_eq!(before["history"].matches("intent_id").count(), 8);

    let waiting = queue(&fixture, &payload);
    let token = (20_000 + waiting.parse::<u64>().unwrap()).to_string();
    node.assert_call("playerbots_fixture_command_apply", &[&waiting, &token]);
    let pending = node.query_rows(&format!(
        "SELECT pending, state FROM game_party_command_intent WHERE id = {waiting}"
    ));
    let after_wait = order(node, &fixture.warrior);
    evidence(&fixture, "receipt-capacity-wait");
    assert_eq!(pending[0]["pending"], "true");
    assert!(pending[0]["state"].to_ascii_lowercase().contains("pending"));
    assert_eq!(after_wait["revision"], before["revision"]);
    assert_eq!(after_wait["history"], before["history"]);
    assert!(node
        .query_rows(&format!(
            "SELECT id FROM game_party_command_receipt WHERE intent_id = {waiting}"
        ))
        .is_empty());

    let mut blocked = vec![waiting.clone()];
    for _ in 0..16 {
        blocked.push(queue(&fixture, &payload));
    }
    let lane_count = u64::from(lyracore_shared::group::COMMAND_DISPATCH_LANES);
    let warrior_lane = fixture.warrior.parse::<u64>().unwrap() % lane_count;
    let later_bot = [&fixture.priest, &fixture.mage]
        .into_iter()
        .find(|guid| guid.parse::<u64>().unwrap() % lane_count != warrior_lane)
        .expect("role fixture needs a bot in another command dispatch lane");
    let later = issue(&fixture, &format!("follow|{later_bot}"), later_bot, false);
    let later_state = node.query_rows(&format!(
        "SELECT pending, state FROM game_party_command_intent WHERE id = {later}"
    ));
    evidence(&fixture, "receipt-capacity-fair-dispatch");
    assert_eq!(
        node.query_rows("SELECT id FROM game_party_command_intent WHERE pending = true")
            .len(),
        blocked.len()
    );
    assert_eq!(later_state[0]["pending"], "false");
    assert!(later_state[0]["state"]
        .to_ascii_lowercase()
        .contains("applied"));

    node.assert_call(
        "playerbots_fixture_command_release_receipt",
        &[&fixture.warrior],
    );
    node.assert_call("playerbots_fixture_command_apply", &[&waiting, &token]);
    node.assert_call("playerbots_fixture_command_finish", &[&waiting, &token]);
    let finished = node.query_rows(&format!(
        "SELECT pending, state FROM game_party_command_intent WHERE id = {waiting}"
    ));
    evidence(&fixture, "receipt-capacity-recovered");
    assert_eq!(finished[0]["pending"], "false");
    assert!(finished[0]["state"]
        .to_ascii_lowercase()
        .contains("unchanged"));
    assert!(!node
        .query_rows(&format!(
            "SELECT id FROM game_party_command_receipt WHERE intent_id = {waiting}"
        ))
        .is_empty());

    let expired_head = &blocked[1];
    node.assert_call("playerbots_fixture_command_expire", &[expired_head]);
    let before_rotation =
        node.query_rows("SELECT head_intent_id FROM game_party_command_dispatch_lane");
    node.assert_call("defer_party_command_intent", &[expired_head, "999999"]);
    let after_rotation =
        node.query_rows("SELECT head_intent_id FROM game_party_command_dispatch_lane");
    evidence(&fixture, "expired-unavailable-head-rotated");
    assert!(before_rotation
        .iter()
        .any(|row| row["head_intent_id"] == *expired_head));
    assert!(!after_rotation
        .iter()
        .any(|row| row["head_intent_id"] == *expired_head));
}
