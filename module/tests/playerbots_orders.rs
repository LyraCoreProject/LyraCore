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
        "orders": node.query_rows("SELECT * FROM pkg_playerbots_companion_order"),
        "intents": node.query_rows("SELECT * FROM game_party_command_intent"),
        "receipts": node.query_rows("SELECT * FROM game_party_command_receipt"),
        "runners": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "quest_purposes": node.query_rows("SELECT * FROM pkg_playerbots_quest_objective"),
        "character_quests": node.query_rows("SELECT * FROM game_character_quest"),
        "entities": node.query_rows("SELECT guid, entry, map_id, instance_id, x, y, z, health, max_health, dead, target_guid FROM game_world_entity"),
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "pending_casts": node.query_rows("SELECT * FROM game_pending_cast"),
        "melee": node.query_rows("SELECT * FROM game_melee_attack"),
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
    let before: u64 = fixture
        .node
        .query_rows("SELECT id FROM game_party_command_intent")
        .iter()
        .map(|row| row["id"].parse().unwrap())
        .max()
        .unwrap_or(0);
    fixture.node.assert_call(
        "gw_client_command",
        &[
            &fixture.actor,
            r#""playerbots.order""#,
            &format!(r#""{payload}""#),
        ],
    );
    fixture
        .node
        .query_rows("SELECT id FROM game_party_command_intent")
        .into_iter()
        .map(|row| row["id"].parse::<u64>().unwrap())
        .filter(|id| *id > before)
        .max()
        .expect("authenticated command did not queue")
        .to_string()
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
    pass(node, &fixture.priest);
    let pending = node.query_rows(&format!(
        "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {}",
        fixture.priest
    ));
    evidence(&fixture, "follow-cast-started");
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
    pass(node, &fixture.priest);
    let survival = runner(node, &fixture.priest);
    evidence(&fixture, "stay-survival-interrupt");
    assert!(survival["chosen"].to_ascii_lowercase().contains("survival"));
    assert!(survival["chosen"].to_ascii_lowercase().contains("move"));
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_assist_uses_only_the_named_members_actual_fight() {
    let fixture = fixture("playerbots-orders-assist");
    let node = &fixture.node;
    let chosen = &fixture.enemies[1];
    let unrelated = &fixture.enemies[0];
    node.assert_call(
        "playerbots_fixture_roles_engage",
        &[&fixture.leader, chosen],
    );
    issue(
        &fixture,
        &format!("assist|{}|{}", fixture.warrior, fixture.leader),
        &fixture.warrior,
        true,
    );
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
    let melee = node.query_rows(&format!(
        "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "target-exact-pull");
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
    node.assert_call(
        "playerbots_fixture_orders_party",
        &[
            &fixture.warrior,
            &fixture.priest,
            &fixture.mage,
            &fixture.leader,
            "3",
        ],
    );
    pass(node, &fixture.warrior);
    evidence(&fixture, "assisted-member-left");
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.warrior
        ))
        .is_empty());
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
fn playerbots_order_clears_when_leadership_changes_and_preserves_the_role() {
    let fixture = fixture("playerbots-orders-leadership-change");
    let node = &fixture.node;
    issue(
        &fixture,
        &format!("follow|{}", fixture.warrior),
        &fixture.warrior,
        false,
    );
    node.assert_call(
        "playerbots_fixture_orders_party",
        &[
            &fixture.warrior,
            &fixture.priest,
            &fixture.mage,
            &fixture.leader,
            "2",
        ],
    );
    pass(node, &fixture.warrior);
    evidence(&fixture, "leadership-lost");
    assert!(node
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
fn playerbots_human_party_suspends_then_rechecks_the_retained_solo_quest() {
    let fixture = fixture("playerbots-orders-retained-solo-quest");
    let node = &fixture.node;
    node.assert_call("playerbots_fixture_companion_remove_group", &[]);
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
    pass(node, &fixture.warrior);
    let solo_runner = runner(node, &fixture.warrior);
    let retained = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "solo-quest-before-party");
    assert!(solo_runner["objective"]
        .to_ascii_lowercase()
        .contains("quest"));
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0]["quest_entry"], "7");

    node.assert_call(
        "playerbots_fixture_orders_party",
        &[
            &fixture.warrior,
            &fixture.priest,
            &fixture.mage,
            &fixture.leader,
            "0",
        ],
    );
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

    node.assert_call("playerbots_fixture_companion_remove_group", &[]);
    pass(node, &fixture.warrior);
    let resumed_runner = runner(node, &fixture.warrior);
    let rechecked = node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_quest_objective WHERE character_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "solo-quest-rechecked-after-party");
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = {}",
            fixture.warrior
        ))
        .is_empty());
    assert!(resumed_runner["objective"]
        .to_ascii_lowercase()
        .contains("quest"));
    assert!(!resumed_runner["foreground"]
        .to_ascii_lowercase()
        .contains("follow"));
    assert!(!resumed_runner["chosen"]
        .to_ascii_lowercase()
        .contains("follow"));
    assert!(resumed_runner["companion_leader_guid"]
        .to_ascii_lowercase()
        .contains("none"));
    assert_eq!(rechecked, retained);
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
}
