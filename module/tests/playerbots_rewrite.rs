//! Real Package action requests against a private, seeded Module.

mod support;
use support::{poll_until, Standalone, POLL_TIMEOUT};

fn record_inputs(node: &Standalone) {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let git = |path: &std::path::Path, args: &[&str]| {
        let result = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success());
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    };
    fn digest_files(path: &std::path::Path, digest: &mut blake3::Hasher) {
        let mut children: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
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
                let contents = std::fs::read(&child).unwrap();
                digest.update(&(contents.len() as u64).to_le_bytes());
                digest.update(&contents);
            }
        }
    }
    let mut digest = blake3::Hasher::new();
    digest_files(&package, &mut digest);
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package.join("fixtures/actions.json")).unwrap())
            .unwrap();
    record["tested_core"] = git(core, &["rev-parse", "HEAD"]).into();
    record["tested_collection"] = git(&package, &["rev-parse", "HEAD"]).into();
    record["core_dirty"] = (!git(core, &["status", "--porcelain"]).is_empty()).into();
    record["collection_dirty"] = (!git(&package, &["status", "--porcelain"]).is_empty()).into();
    record["package_content_identity"] = digest.finalize().to_hex().to_string().into();
    record["module_wasm_identity"] = blake3::hash(support::module_bytes())
        .to_hex()
        .to_string()
        .into();
    let path = support::log_dir().join(format!("{}-inputs.json", node.shard_name()));
    std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    eprintln!("fixture inputs: {}", path.display());
}

fn record_observations(node: &Standalone) {
    let mut record = serde_json::json!({
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "goals": node.query_rows("SELECT * FROM pkg_playerbots_goal"),
        "quests": node.query_rows("SELECT * FROM game_character_quest"),
        "pending_casts": node.query_rows("SELECT * FROM game_pending_cast"),
    });
    let bots = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot");
    let positions: Vec<_> = bots.iter().flat_map(|bot| node.query_rows(&format!(
        "SELECT guid, map_id, instance_id, x, y, z, health FROM game_world_entity WHERE guid = {}", bot["character_guid"]))).collect();
    record["characters"] = serde_json::to_value(positions).unwrap();
    let path = support::log_dir().join(format!("{}-outcomes.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

fn fixture(name: &str, count: &str) -> (Standalone, Vec<String>) {
    fixture_role(name, count, "1")
}

fn fixture_role(name: &str, count: &str, role: &str) -> (Standalone, Vec<String>) {
    let mut node = Standalone::start(name);
    node.publish_module();
    record_inputs(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call(
        "playerbots_spawn_role",
        &[count, "1200", "1200", "50", role],
    );
    node.assert_call("playerbots_fixture_prepare", &[]);
    let bots = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .iter()
        .map(|r| r["character_guid"].clone())
        .collect();
    (node, bots)
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_a_long_bot_cast_waits_for_core_completion() {
    let (node, bots) = fixture("playerbots-cast", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_cast", &[bot, bot]);
    let pending = node.query_rows("SELECT scheduled_id, caster_guid FROM game_pending_cast");
    assert_eq!(pending.len(), 1, "a five-second heal must leave a cast bar");
    assert_eq!(
        node.query_rows(&format!(
            "SELECT kind FROM game_spell_cast_event WHERE caster_guid = {bot}"
        ))
        .iter()
        .filter(|r| r["kind"] == "1")
        .count(),
        1
    );
    assert!(
        node.query_rows(&format!(
            "SELECT kind FROM game_spell_cast_event WHERE caster_guid = {bot}"
        ))
        .iter()
        .all(|r| r["kind"] != "2"),
        "the cast has not resolved yet"
    );
    node.assert_call("playerbots_fixture_resume", &[bot]);
    for target in [bot.as_str(), "0"] {
        std::thread::sleep(std::time::Duration::from_secs(1));
        node.assert_call("playerbots_fixture_cast", &[bot, target]);
        assert_eq!(
            node.query_rows("SELECT scheduled_id, caster_guid FROM game_pending_cast"),
            pending,
            "intervening requests and brain ticks retain the same cast"
        );
    }
    poll_until(POLL_TIMEOUT, || {
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")
            .is_empty()
    });
    let events = node.query_rows(&format!(
        "SELECT kind FROM game_spell_cast_event WHERE caster_guid = {bot}"
    ));
    assert_eq!(events.iter().filter(|r| r["kind"] == "2").count(), 1);
    assert!(
        node.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {bot}"
        ))[0]["health"]
            .parse::<u32>()
            .unwrap()
            >= 50
    );
    let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(
        result.iter().any(|r| r["outcome"] == "(castResolved = ())"),
        "{result:?}"
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_an_expired_cast_cannot_heal_later() {
    let (node, bots) = fixture("playerbots-cast-expiry", "2");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_cast", &[bot, bot]);
    node.assert_call("playerbots_fixture_cancel", &[bot, "true"]);
    assert!(node
        .query_rows("SELECT scheduled_id FROM game_pending_cast")
        .is_empty());
    assert_eq!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"],
        "(expired = ())"
    );
    std::thread::sleep(std::time::Duration::from_secs(6));
    let health = |guid: &str| {
        node.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {guid}"
        ))[0]["health"]
            .clone()
    };
    assert_eq!(
        health(bot),
        health(&bots[1]),
        "the expired cast adds no healing beyond natural recovery"
    );
    assert_eq!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"],
        "(expired = ())"
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_an_out_of_range_heal_reports_the_owning_cast_gate() {
    let (node, bots) = fixture("playerbots-cast-range", "2");
    node.assert_call("playerbots_fixture_position", &[&bots[1], "1400"]);
    node.assert_call("playerbots_fixture_cast", &[&bots[0], &bots[1]]);
    assert!(node
        .query_rows("SELECT scheduled_id FROM game_pending_cast")
        .is_empty());
    let rows = node.query_rows("SELECT target_guid, spell_id, outcome FROM pkg_playerbots_action");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["target_guid"], bots[1]);
    assert_eq!(rows[0]["spell_id"], "5090100");
    assert!(rows[0]["outcome"].contains("outOfRange"), "{rows:?}");
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_explicit_cancellation_removes_the_cast_and_retains_its_outcome() {
    let (node, bots) = fixture("playerbots-cast-cancel", "1");
    node.assert_call("playerbots_fixture_cast", &[&bots[0], &bots[0]]);
    node.assert_call("playerbots_fixture_cancel", &[&bots[0], "false"]);
    assert!(node
        .query_rows("SELECT scheduled_id FROM game_pending_cast")
        .is_empty());
    assert_eq!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"],
        "(cancelled = ())"
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_an_accepted_attack_without_a_hit_does_not_clear_quest_stall() {
    let (node, bots) = fixture_role("playerbots-blocked-quest", "1", "0");
    let bot = &bots[0];
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    poll_until(POLL_TIMEOUT, || {
        !node
            .query_rows("SELECT attacker_guid FROM game_melee_attack")
            .is_empty()
    });
    std::thread::sleep(std::time::Duration::from_secs(2));
    let actions = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(
        actions
            .iter()
            .any(|r| r["outcome"].contains("attackAccepted")),
        "{actions:?}"
    );
    let quest = node.query_rows(&format!(
        "SELECT counts FROM game_character_quest WHERE character_guid = {bot}"
    ));
    assert_eq!(quest[0]["counts"], "0");
    let read_stall = || {
        node.query_rows(&format!(
            "SELECT stalled_since_micros FROM pkg_playerbots_goal WHERE character_guid = {bot}"
        ))[0]["stalled_since_micros"]
            .clone()
    };
    let before = read_stall();
    assert_ne!(
        before, "0",
        "accepted combat with no hit must start the no-progress clock"
    );
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert_eq!(
        read_stall(),
        before,
        "another accepted attack cannot reset the clock"
    );
    let target = ((0xF130u64 << 48) | (5_090_101u64 << 24) | 1).to_string();
    let health = node.query_rows(&format!(
        "SELECT health, max_health FROM game_world_entity WHERE guid = {target}"
    ));
    assert_eq!(
        health[0]["health"], health[0]["max_health"],
        "accepted combat dealt no damage"
    );
    node.assert_call("playerbots_fixture_move", &[bot, "1210"]);
    let route = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(
        route.iter().any(|r| r["outcome"].contains("blocked")),
        "{route:?}"
    );
    assert!(node
        .query_rows(&format!(
            "SELECT guid FROM game_creature_spline WHERE guid = {bot}"
        ))
        .is_empty());
    node.assert_call("playerbots_fixture_credit_kill", &[bot]);
    poll_until(POLL_TIMEOUT, || read_stall() != before);
    assert_ne!(
        read_stall(),
        before,
        "actual objective credit ends the previous stall"
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_route_evidence_distinguishes_arrival_from_a_planned_leg() {
    let (node, bots) = fixture("playerbots-route", "1");
    let bot = &bots[0];
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_move", &[bot, "1210"]);
    let read = || node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    let first = read();
    assert!(first[0]["outcome"].contains("complete"), "{first:?}");
    assert!(
        first[0]["outcome"].contains("unknown"),
        "sparse seeded input cannot prove coverage: {first:?}"
    );
    poll_until(POLL_TIMEOUT, || {
        node.query_rows(&format!(
            "SELECT x FROM game_world_entity WHERE guid = {bot}"
        ))[0]["x"]
            .parse::<f32>()
            .unwrap()
            > 1200.5
    });
    for _ in 0..4 {
        node.assert_call("playerbots_fixture_move", &[bot, "1210"]);
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    let x = node.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {bot}"
    ))[0]["x"]
        .parse::<f32>()
        .unwrap();
    assert!((x - 1210.0).abs() < 0.1, "actual position {x}");
    node.assert_call("playerbots_fixture_move", &[bot, "1210"]);
    assert!(read()[0]["outcome"].contains("arrived = true"));
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_instant_casts_resolve_synchronously_and_channels_are_explicitly_refused() {
    let (node, bots) = fixture("playerbots-cast-instant", "1");
    node.assert_call(
        "playerbots_fixture_cast_mode",
        &[&bots[0], &bots[0], "false"],
    );
    assert!(node
        .query_rows("SELECT scheduled_id FROM game_pending_cast")
        .is_empty());
    assert_eq!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"],
        "(castResolved = ())"
    );
    let events = node.query_rows("SELECT kind FROM game_spell_cast_event");
    assert_eq!(events.iter().filter(|r| r["kind"] == "2").count(), 1);
    node.assert_call(
        "playerbots_fixture_cast_mode",
        &[&bots[0], &bots[0], "true"],
    );
    assert!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"]
            .contains("unsupportedChannel")
    );
    assert!(node
        .query_rows("SELECT scheduled_id FROM game_pending_cast")
        .is_empty());
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_cast_completion_rechecks_range_and_stale_handles_cannot_cancel() {
    let (node, bots) = fixture("playerbots-cast-terminal-range", "2");
    node.assert_call("playerbots_fixture_cast", &[&bots[0], &bots[1]]);
    let pending = node.query_rows("SELECT scheduled_id FROM game_pending_cast");
    let id = pending[0]["scheduled_id"].parse::<u64>().unwrap();
    node.assert_call(
        "playerbots_fixture_cancel_handle",
        &[&bots[0], &(id + 1).to_string(), "0"],
    );
    node.assert_call(
        "playerbots_fixture_cancel_handle",
        &[&bots[0], &id.to_string(), &i64::MAX.to_string()],
    );
    assert_eq!(
        node.query_rows("SELECT scheduled_id FROM game_pending_cast"),
        pending
    );
    node.assert_call("playerbots_fixture_position", &[&bots[1], "1400"]);
    poll_until(POLL_TIMEOUT, || {
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")
            .is_empty()
    });
    let result = node.query_rows("SELECT cast_id, outcome FROM pkg_playerbots_action");
    assert_eq!(result[0]["cast_id"], id.to_string());
    assert!(result[0]["outcome"].contains("outOfRange"), "{result:?}");
    assert!(node
        .query_rows("SELECT kind FROM game_spell_cast_event")
        .iter()
        .all(|r| r["kind"] != "2"));
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_a_missing_attack_target_has_a_typed_refusal() {
    let (node, bots) = fixture("playerbots-attack-refusal", "1");
    node.assert_call("playerbots_fixture_attack", &[&bots[0], "999999999"]);
    let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(result[0]["outcome"].contains("missingTarget"), "{result:?}");
    assert!(node
        .query_rows("SELECT attacker_guid FROM game_melee_attack")
        .is_empty());
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_a_partial_route_retains_its_waypoint_and_search_budget() {
    let (node, bots) = fixture("playerbots-route-partial", "1");
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_partial_route", &[&bots[0]]);
    node.assert_call("playerbots_fixture_move", &[&bots[0], "1220"]);
    let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(result[0]["outcome"].contains("partial"), "{result:?}");
    assert!(
        result[0]["outcome"].contains("expansions = 4096"),
        "{result:?}"
    );
    poll_until(POLL_TIMEOUT, || {
        node.query_rows(&format!(
            "SELECT x FROM game_world_entity WHERE guid = {}",
            bots[0]
        ))[0]["x"]
            .parse::<f32>()
            .unwrap()
            > 1200.5
    });
    record_observations(&node);
}

fn inventory(node: &Standalone, guid: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    let mut rows = node.query_rows(&format!(
        "SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    rows.sort_by(|a, b| a["guid"].cmp(&b["guid"]));
    rows
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_caught_turn_in_refusals_leave_inventory_and_rewards_unchanged() {
    let (node, bots) = fixture("playerbots-turnin-atomic", "1");
    let bot = &bots[0];
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_interaction_stage", &[bot, "false"]);
    let before = inventory(&node, bot);
    assert_eq!(before.len(), 16);
    let read_player = || {
        node.query_rows(&format!(
            "SELECT money, xp, level FROM game_world_entity WHERE guid = {bot}"
        ))
    };
    let read_quest = || {
        node.query_rows(&format!(
            "SELECT counts, rewarded FROM game_character_quest WHERE character_guid = {bot}"
        ))
    };
    let player = read_player();
    let quest = read_quest();
    for choice in ["99", "0"] {
        node.assert_call("playerbots_fixture_interact", &[bot, "false", choice]);
        let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
        assert!(result[0]["outcome"].contains("refused"), "{result:?}");
        if choice == "0" {
            assert!(result[0]["outcome"].contains("inventoryFull"), "{result:?}");
        }
        assert_eq!(
            inventory(&node, bot),
            before,
            "caught refusal must not consume or grant an item"
        );
        assert_eq!(read_player(), player);
        assert_eq!(read_quest(), quest);
    }
    node.assert_call("playerbots_fixture_free_slot", &[bot]);
    node.assert_call("playerbots_fixture_interact", &[bot, "false", "0"]);
    assert_eq!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"],
        "(completed = ())"
    );
    assert_eq!(read_quest()[0]["rewarded"], "true");
    let items = inventory(&node, bot);
    assert!(items.iter().all(|r| r["entry"] != "5090120"));
    assert_eq!(
        items
            .iter()
            .filter(|r| r["entry"] == "5090121" || r["entry"] == "5090122")
            .count(),
        2
    );
    assert_eq!(
        read_player()[0]["money"].parse::<u32>().unwrap(),
        player[0]["money"].parse::<u32>().unwrap() + 150
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_acceptance_reports_range_and_capacity_before_granting_a_quest() {
    let (node, bots) = fixture("playerbots-accept-atomic", "1");
    let bot = &bots[0];
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_interaction_stage", &[bot, "true"]);
    let before = inventory(&node, bot);
    node.assert_call("playerbots_fixture_position", &[bot, "1400"]);
    node.assert_call("playerbots_fixture_interact", &[bot, "true", "0"]);
    let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(result[0]["outcome"].contains("outOfRange"), "{result:?}");
    node.assert_call("playerbots_fixture_position", &[bot, "1200"]);
    node.assert_call("playerbots_fixture_interact", &[bot, "true", "0"]);
    let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(result[0]["outcome"].contains("inventoryFull"), "{result:?}");
    assert!(node
        .query_rows(&format!(
            "SELECT id FROM game_character_quest WHERE character_guid = {bot}"
        ))
        .is_empty());
    assert_eq!(inventory(&node, bot), before);
    node.assert_call("playerbots_fixture_free_slot", &[bot]);
    node.assert_call("playerbots_fixture_interact", &[bot, "true", "0"]);
    assert_eq!(
        node.query_rows("SELECT outcome FROM pkg_playerbots_action")[0]["outcome"],
        "(completed = ())"
    );
    assert_eq!(
        node.query_rows(&format!(
            "SELECT id FROM game_character_quest WHERE character_guid = {bot}"
        ))
        .len(),
        1
    );
    assert_eq!(
        inventory(&node, bot)
            .iter()
            .filter(|r| r["entry"] == "5090120")
            .count(),
        1
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_initial_credit_observation_preserves_an_existing_stall() {
    let (node, bots) = fixture_role("playerbots-credit-baseline", "1", "0");
    let bot = &bots[0];
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    let stall = || {
        node.query_rows(&format!(
            "SELECT stalled_since_micros FROM pkg_playerbots_goal WHERE character_guid = {bot}"
        ))[0]["stalled_since_micros"]
            .clone()
    };
    poll_until(POLL_TIMEOUT, || {
        !node
            .query_rows("SELECT id FROM pkg_playerbots_goal")
            .is_empty()
    });
    poll_until(POLL_TIMEOUT, || stall() != "0");
    node.assert_call("playerbots_fixture_freeze", &[bot]);
    let before = stall();
    node.assert_call("playerbots_fixture_credit_kill", &[bot]);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT counts FROM game_character_quest WHERE character_guid = {bot}"
        ))[0]["counts"],
        "1"
    );
    node.assert_call("playerbots_fixture_forget_credit", &[bot]);
    let credit = || {
        node.query_rows(&format!(
            "SELECT quest_credit FROM pkg_playerbots_goal WHERE character_guid = {bot}"
        ))[0]["quest_credit"]
            .clone()
    };
    let unknown = credit();
    node.assert_call("playerbots_fixture_resume", &[bot]);
    poll_until(POLL_TIMEOUT, || credit() != unknown);
    assert_eq!(
        stall(),
        before,
        "initializing the migration default is not new objective credit"
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_core_cast_replacement_finishes_the_original_handle() {
    let (node, bots) = fixture("playerbots-cast-replacement", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_cast", &[bot, bot]);
    let original =
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"].clone();
    node.assert_call("debug_begin_cast", &[bot, "5090100", bot]);
    assert_ne!(
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"],
        original
    );
    let outcome = node.query_rows("SELECT cast_id, outcome FROM pkg_playerbots_action");
    assert_eq!(outcome[0]["cast_id"], original);
    assert_eq!(outcome[0]["outcome"], "(cancelled = ())");
    assert_eq!(
        node.query_rows("SELECT is_interrupted FROM game_spell_cast_event")
            .iter()
            .filter(|r| r["is_interrupted"] == "true")
            .count(),
        1,
        "replacement preserves the existing core interruption signal"
    );
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_action_rows_are_bounded_and_deleted_with_the_character() {
    let (node, bots) = fixture("playerbots-action-lifecycle", "1");
    let bot = &bots[0];
    for _ in 0..3 {
        node.assert_call("playerbots_fixture_cast", &[bot, bot]);
    }
    assert_eq!(
        node.query_rows("SELECT id FROM pkg_playerbots_action")
            .len(),
        1
    );
    node.assert_call("playerbots_fixture_cancel", &[bot, "false"]);
    node.assert_call("debug_delete_character", &[bot]);
    assert!(node
        .query_rows("SELECT id FROM pkg_playerbots_action")
        .is_empty());
    assert!(node
        .query_rows("SELECT scheduled_id FROM game_pending_cast")
        .is_empty());
    record_observations(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_a_dead_attacker_is_refused_before_engagement() {
    let (node, bots) = fixture_role("playerbots-attack-dead", "1", "0");
    let bot = &bots[0];
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    node.assert_call("playerbots_fixture_freeze", &[bot]);
    node.assert_call("debug_set_health", &[bot, "0"]);
    let target = ((0xF130u64 << 48) | (5_090_101u64 << 24) | 1).to_string();
    node.assert_call("playerbots_fixture_attack", &[bot, &target]);
    let result = node.query_rows("SELECT outcome FROM pkg_playerbots_action");
    assert!(
        result.iter().any(|r| r["outcome"].contains("deadActor")),
        "{result:?}"
    );
    assert!(node
        .query_rows(&format!(
            "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {bot}"
        ))
        .is_empty());
    let ranged = node.call("debug_ranged_attack_nearest", &[bot, "75"]);
    assert!(!ranged.status.success());
    assert!(String::from_utf8_lossy(&ranged.stderr).contains("dead attackers cannot attack"));
    record_observations(&node);
}
