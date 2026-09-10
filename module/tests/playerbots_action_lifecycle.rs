//! Required PB-011 lifecycle gaps at the real Runner ownership boundary.

mod support;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::Standalone;

const CREATURE_PREFIX: u64 = 0xF130u64 << 48;
const GAMEOBJECT_PREFIX: u64 = 0xF110u64 << 48;

#[derive(Clone, Copy)]
struct QuestRoot {
    fixture_kind: u8,
    label: &'static str,
    class: &'static str,
    role: &'static str,
    target: u64,
    root_shape: &'static str,
    prerequisite: &'static str,
}

const QUEST_ROOTS: [QuestRoot; 7] = [
    QuestRoot {
        fixture_kind: 0,
        label: "accept-quest",
        class: "1",
        role: "0",
        target: CREATURE_PREFIX | (823u64 << 24) | 1,
        root_shape: "acceptQuest = (",
        prerequisite: "entity",
    },
    QuestRoot {
        fixture_kind: 1,
        label: "turn-in-quest",
        class: "1",
        role: "0",
        target: CREATURE_PREFIX | (197u64 << 24) | 1,
        root_shape: "turnInQuest = (",
        prerequisite: "entity",
    },
    QuestRoot {
        fixture_kind: 2,
        label: "loot-creature",
        class: "1",
        role: "0",
        target: CREATURE_PREFIX | (299u64 << 24) | 1,
        root_shape: "lootCreature = (",
        prerequisite: "entity",
    },
    QuestRoot {
        fixture_kind: 3,
        label: "use-gameobject",
        class: "8",
        role: "2",
        target: GAMEOBJECT_PREFIX | 161_557,
        root_shape: "useGameObject = (",
        prerequisite: "gameObject",
    },
    QuestRoot {
        fixture_kind: 4,
        label: "loot-gameobject",
        class: "8",
        role: "2",
        target: GAMEOBJECT_PREFIX | 161_557,
        root_shape: "lootGameObject = (",
        prerequisite: "gameObject",
    },
    QuestRoot {
        fixture_kind: 5,
        label: "attack",
        class: "1",
        role: "0",
        target: CREATURE_PREFIX | (6u64 << 24) | 1,
        root_shape: "attack = ",
        prerequisite: "entity",
    },
    QuestRoot {
        fixture_kind: 6,
        label: "quest-cast",
        class: "5",
        role: "1",
        target: CREATURE_PREFIX | (6u64 << 24) | 1,
        root_shape: "cast = (",
        prerequisite: "castingPosition",
    },
];

fn remove_builtin_weather_import_stamp(node: &Standalone) {
    let rows =
        node.query_rows("SELECT family, source_sha, file_hash, row_count FROM game_import_meta");
    assert_eq!(rows.len(), 1, "unexpected initial Import Catalogue");
    assert_eq!(rows[0]["family"], "weather_seed");
    assert_eq!(rows[0]["source_sha"], "");
    assert_eq!(rows[0]["file_hash"], "");
    assert_eq!(rows[0]["row_count"], "2");
    node.assert_sql(
        "DELETE FROM game_import_meta WHERE family = 'weather_seed' AND source_sha = '' AND file_hash = '' AND row_count = 2",
    );
}

fn stage_geometry(node: &Standalone) {
    let x0 = lyracore_shared::terrain::cell_index(1_150.0).unwrap();
    let x1 = lyracore_shared::terrain::cell_index(1_300.0).unwrap();
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

fn fixture(name: &str, count: usize) -> (Standalone, Vec<String>) {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call(
        "playerbots_spawn_role",
        &[&count.to_string(), "1200", "1200", "50", "1"],
    );
    node.assert_call("playerbots_fixture_prepare", &[]);
    let mut bots: Vec<_> = node
        .query_rows("SELECT character_guid FROM pkg_playerbots_bot")
        .into_iter()
        .map(|row| row["character_guid"].clone())
        .collect();
    bots.sort();
    (node, bots)
}

fn quest_fixture(root: QuestRoot, lifecycle: &str, blocked: bool) -> (Standalone, String) {
    let mut node = Standalone::start(&format!("playerbots-action-{}-{lifecycle}", root.label));
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    stage_geometry(&node);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", root.class, root.role],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call(
        if root.fixture_kind == 6 {
            "playerbots_quest_loop_fixture_stage_named"
        } else {
            "playerbots_quest_fixture_stage"
        },
        &[&guid],
    );
    node.assert_call(
        "playerbots_action_lifecycle_stage_quest_plan",
        &[
            &guid,
            &root.fixture_kind.to_string(),
            &root.target.to_string(),
        ],
    );
    node.assert_call(
        if blocked {
            "playerbots_recovery_fixture_block_companion"
        } else {
            "playerbots_fixture_runner_select_cohort"
        },
        &[&guid],
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    (node, guid)
}

fn save(node: &Standalone, label: &str, value: &Value) {
    let path = support::log_dir().join(format!("{}-{label}.json", node.shard_name()));
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn sorted(mut rows: Vec<BTreeMap<String, String>>, key: &str) -> Vec<BTreeMap<String, String>> {
    rows.sort_by(|left, right| left[key].cmp(&right[key]));
    rows
}

fn snapshot(node: &Standalone) -> Value {
    json!({
        "bot": sorted(node.query_rows("SELECT character_guid, controller FROM pkg_playerbots_bot"), "character_guid"),
        "runner": sorted(node.query_rows("SELECT character_guid, objective_sequence, objective, foreground, chosen, candidate_order, transfer_checkpoint, last_outcome, failures, history FROM pkg_playerbots_runner"), "character_guid"),
        "actions": sorted(node.query_rows("SELECT character_guid, kind, target_guid, spell_id, quest_entry, outcome, started_micros, observed_micros FROM pkg_playerbots_action"), "character_guid"),
        "movement": sorted(node.query_rows("SELECT guid, sx, sy, sz, dx, dy, dz, start_micros, dur_ms, run FROM game_creature_spline"), "guid"),
        "characters": sorted(node.query_rows("SELECT guid, map_id, instance_id, x, y, z, dead, player_flags FROM game_world_entity WHERE guid >= 1000000 AND guid < 2000000"), "guid"),
        "casts": sorted(node.query_rows("SELECT caster_guid, scheduled_id FROM game_pending_cast"), "caster_guid"),
        "attacks": sorted(node.query_rows("SELECT attacker_guid, target_guid FROM game_melee_attack"), "attacker_guid"),
        "quests": sorted(node.query_rows("SELECT character_guid, quest_entry, counts, rewarded, failed FROM game_character_quest"), "character_guid"),
        "loot": sorted(node.query_rows("SELECT corpse_guid, slot, item_entry, count FROM game_corpse_loot"), "corpse_guid"),
        "transfers": sorted(node.query_rows("SELECT bot_guid, id FROM game_bot_transfer_intent"), "bot_guid"),
    })
}

fn assert_objective_identity(before: &Value, after: &Value) {
    assert_eq!(
        before["objective_sequence"], after["objective_sequence"],
        "before={before}; after={after}"
    );
    let identity = before["objective_sequence"].as_str().unwrap();
    let marker = format!("identity = {identity}");
    assert!(
        before["objective"].as_str().unwrap().contains(&marker),
        "{before}"
    );
    assert!(
        after["objective"].as_str().unwrap().contains(&marker),
        "{after}"
    );
}

fn assert_real_pending_root(evidence: &Value, root: QuestRoot, guid: &str) {
    assert_eq!(
        evidence["bot"][0]["controller"], "(cohort = ())",
        "{evidence}"
    );
    let runner = &evidence["runner"][0];
    let order = runner["candidate_order"].as_str().unwrap();
    assert_eq!(order.matches(root.root_shape).count(), 1, "{evidence}");
    assert!(order.contains(&root.target.to_string()), "{evidence}");
    assert!(order.contains(root.prerequisite), "{evidence}");
    let chosen = runner["chosen"].as_str().unwrap();
    assert!(chosen.contains(root.prerequisite), "{evidence}");
    assert!(chosen.contains(&root.target.to_string()), "{evidence}");
    let foreground = runner["foreground"].as_str().unwrap();
    assert!(foreground.contains(root.prerequisite), "{evidence}");
    assert!(foreground.contains(&root.target.to_string()), "{evidence}");
    assert!(
        runner["objective"].as_str().unwrap().contains("travelling"),
        "{evidence}"
    );
    assert_eq!(
        evidence["movement"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["guid"] == guid)
            .count(),
        1,
        "{evidence}"
    );
    assert!(
        evidence["transfers"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
}

fn assert_stopped_owned_movement(evidence: &Value, guid: &str) {
    let movement = evidence["movement"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["guid"] == guid)
        .unwrap_or_else(|| panic!("stopped movement missing: {evidence}"));
    assert_eq!(movement["run"], "false", "{evidence}");
    assert_eq!(movement["dur_ms"], "0", "{evidence}");
    assert_eq!(movement["sx"], movement["dx"], "{evidence}");
    assert_eq!(movement["sy"], movement["dy"], "{evidence}");
    assert_eq!(movement["sz"], movement["dz"], "{evidence}");
}

fn structured_number(value: &str, field: &str) -> String {
    let marker = format!("{field} = ");
    value
        .split(&marker)
        .nth(1)
        .and_then(|tail| tail.split([',', ')']).next())
        .unwrap_or_else(|| panic!("{field} missing from {value}"))
        .trim()
        .to_string()
}

fn home_pending(lifecycle: &str) -> (Standalone, String, Value) {
    let (node, bots) = fixture(&format!("playerbots-action-home-{lifecycle}"), 1);
    let guid = bots[0].clone();
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "false"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let pending = snapshot(&node);
    save(&node, "home-pending", &pending);
    let runner = &pending["runner"][0];
    assert_eq!(
        runner["candidate_order"]
            .as_str()
            .unwrap()
            .matches("move = (home = ())")
            .count(),
        1,
        "{pending}"
    );
    assert!(
        runner["chosen"].as_str().unwrap().contains("home"),
        "{pending}"
    );
    assert!(
        runner["foreground"].as_str().unwrap().contains("home"),
        "{pending}"
    );
    assert!(
        runner["objective"].as_str().unwrap().contains("returnHome"),
        "{pending}"
    );
    assert!(
        runner["objective"].as_str().unwrap().contains("travelling"),
        "{pending}"
    );
    (node, guid, pending)
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_return_home_cancels_its_real_owned_movement() {
    let (node, guid, pending) = home_pending("cancellation");
    node.assert_call("playerbots_select_controller", &[&guid, "{\"frozen\":[]}"]);
    let cancelled = snapshot(&node);
    save(&node, "home-cancelled", &cancelled);
    assert_objective_identity(&pending["runner"][0], &cancelled["runner"][0]);
    assert!(
        cancelled["runner"][0]["history"]
            .as_str()
            .unwrap()
            .contains("home"),
        "{cancelled}"
    );
    assert_stopped_owned_movement(&cancelled, &guid);
    assert_eq!(cancelled["actions"], pending["actions"], "{cancelled}");
    assert_eq!(
        cancelled["characters"], pending["characters"],
        "{cancelled}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_return_home_expires_its_real_owned_movement() {
    let (node, guid, pending) = home_pending("expiry");
    node.assert_call("playerbots_fixture_runner_expire_objective", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let expired = snapshot(&node);
    save(&node, "home-expired", &expired);
    let runner = &expired["runner"][0];
    assert_objective_identity(&pending["runner"][0], runner);
    assert!(
        runner["failures"].as_str().unwrap().contains("deadline"),
        "{expired}"
    );
    assert!(
        runner["objective"].as_str().unwrap().contains("deferred"),
        "{expired}"
    );
    assert!(
        runner["history"].as_str().unwrap().contains("home"),
        "{expired}"
    );
    assert_stopped_owned_movement(&expired, &guid);
    assert_eq!(expired["actions"], pending["actions"], "{expired}");
    assert_eq!(expired["characters"], pending["characters"], "{expired}");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_home_move_records_a_verified_blocked_route() {
    let (node, bots) = fixture("playerbots-action-home-blocked-route", 1);
    let guid = &bots[0];
    node.assert_call("playerbots_fixture_blocked_quest", &[guid]);
    node.assert_call("playerbots_fixture_runner_stage", &[guid, "false"]);
    node.assert_call("gw_abandon_quest", &[&support::actor(guid), "50909"]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
    let first = snapshot(&node);
    save(&node, "home-blocked-first", &first);
    let start_position = first["characters"][0].clone();
    assert!(
        first["runner"][0]["candidate_order"]
            .as_str()
            .unwrap()
            .contains("move = (home = ())"),
        "{first}"
    );
    assert!(
        first["runner"][0]["chosen"]
            .as_str()
            .unwrap()
            .contains("move = (home = ())"),
        "{first}"
    );
    assert!(
        first["actions"].as_array().unwrap().iter().any(|action| {
            action["kind"].as_str().unwrap().contains("move")
                && action["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("status = (blocked = ())")
        }),
        "{first}"
    );

    node.assert_call("playerbots_fixture_companion_due", &[guid]);
    let deadline = Instant::now() + Duration::from_secs(38);
    let mut samples = vec![first];
    loop {
        let current = snapshot(&node);
        let stopped = current["runner"][0]["failures"]
            .as_str()
            .unwrap()
            .contains("noMovement");
        samples.push(current);
        if stopped || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(750));
    }
    let evidence = json!({"guid": guid, "samples": samples});
    save(&node, "home-blocked-route", &evidence);
    let samples = evidence["samples"].as_array().unwrap();
    assert!(
        samples.iter().any(|sample| sample["runner"][0]["failures"]
            .as_str()
            .unwrap()
            .contains("noMovement")),
        "{evidence}"
    );
    assert!(
        samples.iter().all(|sample| {
            sample["characters"][0]["x"] == start_position["x"]
                && sample["characters"][0]["y"] == start_position["y"]
        }),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_roots_cancel_their_real_movement_prerequisites() {
    for root in QUEST_ROOTS {
        let (node, guid) = quest_fixture(root, "cancellation", false);
        let pending = snapshot(&node);
        save(&node, &format!("{}-pending", root.label), &pending);
        assert_real_pending_root(&pending, root, &guid);

        node.assert_call("playerbots_select_controller", &[&guid, "{\"frozen\":[]}"]);
        let cancelled = snapshot(&node);
        save(&node, &format!("{}-cancelled", root.label), &cancelled);
        let before = &pending["runner"][0];
        let after = &cancelled["runner"][0];
        assert_objective_identity(before, after);
        assert!(
            after["foreground"].as_str().unwrap().contains("none"),
            "{cancelled}"
        );
        assert!(
            after["last_outcome"].as_str().unwrap().contains("frozen"),
            "{cancelled}"
        );
        assert!(
            after["history"]
                .as_str()
                .unwrap()
                .contains(root.prerequisite),
            "{cancelled}"
        );
        assert!(
            after["history"]
                .as_str()
                .unwrap()
                .contains(&root.target.to_string()),
            "{cancelled}"
        );
        assert_stopped_owned_movement(&cancelled, &guid);
        assert_eq!(cancelled["actions"], pending["actions"], "{cancelled}");
        assert_eq!(cancelled["quests"], pending["quests"], "{cancelled}");
        assert_eq!(cancelled["loot"], pending["loot"], "{cancelled}");
        assert!(
            cancelled["casts"].as_array().unwrap().is_empty(),
            "{cancelled}"
        );
        assert!(
            cancelled["attacks"].as_array().unwrap().is_empty(),
            "{cancelled}"
        );
        assert!(
            cancelled["transfers"].as_array().unwrap().is_empty(),
            "{cancelled}"
        );
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_roots_expire_their_real_movement_prerequisites() {
    for root in QUEST_ROOTS {
        let (node, guid) = quest_fixture(root, "expiry", false);
        let pending = snapshot(&node);
        save(&node, &format!("{}-pending", root.label), &pending);
        assert_real_pending_root(&pending, root, &guid);

        node.assert_call("playerbots_fixture_runner_expire_objective", &[&guid]);
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let expired = snapshot(&node);
        save(&node, &format!("{}-expired", root.label), &expired);
        let before = &pending["runner"][0];
        let after = &expired["runner"][0];
        assert_objective_identity(before, after);
        assert!(
            after["foreground"].as_str().unwrap().contains("none"),
            "{expired}"
        );
        assert!(
            after["failures"].as_str().unwrap().contains("deadline"),
            "{expired}"
        );
        assert!(
            after["objective"].as_str().unwrap().contains("deferred"),
            "{expired}"
        );
        assert!(
            after["history"]
                .as_str()
                .unwrap()
                .contains(root.prerequisite),
            "{expired}"
        );
        assert!(
            after["history"]
                .as_str()
                .unwrap()
                .contains(&root.target.to_string()),
            "{expired}"
        );
        assert_stopped_owned_movement(&expired, &guid);
        assert_eq!(expired["actions"], pending["actions"], "{expired}");
        assert_eq!(expired["quests"], pending["quests"], "{expired}");
        assert_eq!(expired["loot"], pending["loot"], "{expired}");
        assert!(expired["casts"].as_array().unwrap().is_empty(), "{expired}");
        assert!(
            expired["attacks"].as_array().unwrap().is_empty(),
            "{expired}"
        );
        assert!(
            expired["transfers"].as_array().unwrap().is_empty(),
            "{expired}"
        );

        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let after_retry = snapshot(&node);
        save(&node, &format!("{}-after-retry", root.label), &after_retry);
        assert_eq!(after_retry["actions"], expired["actions"], "{after_retry}");
        assert_eq!(after_retry["quests"], expired["quests"], "{after_retry}");
        assert_eq!(after_retry["loot"], expired["loot"], "{after_retry}");
        assert!(
            after_retry["casts"].as_array().unwrap().is_empty(),
            "{after_retry}"
        );
        assert!(
            after_retry["attacks"].as_array().unwrap().is_empty(),
            "{after_retry}"
        );
        assert!(
            after_retry["transfers"].as_array().unwrap().is_empty(),
            "{after_retry}"
        );
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_move_targets_record_verified_blocked_routes() {
    for root in [QUEST_ROOTS[3], QUEST_ROOTS[5], QUEST_ROOTS[6]] {
        let (node, guid) = quest_fixture(root, "blocked-route", true);
        let pending = snapshot(&node);
        save(&node, &format!("{}-blocked-first", root.label), &pending);
        let runner = &pending["runner"][0];
        let order = runner["candidate_order"].as_str().unwrap();
        assert_eq!(order.matches(root.root_shape).count(), 1, "{pending}");
        assert!(order.contains(root.prerequisite), "{pending}");
        assert!(order.contains(&root.target.to_string()), "{pending}");
        let chosen = runner["chosen"].as_str().unwrap();
        assert!(chosen.contains(root.prerequisite), "{pending}");
        assert!(chosen.contains(&root.target.to_string()), "{pending}");
        assert!(
            pending["actions"].as_array().unwrap().iter().any(|action| {
                action["kind"].as_str().unwrap().contains("move")
                    && action["outcome"]
                        .as_str()
                        .unwrap()
                        .contains("status = (blocked = ())")
            }),
            "{pending}"
        );
        let start_position = pending["characters"][0].clone();
        node.assert_call("playerbots_fixture_companion_due", &[&guid]);
        let deadline = Instant::now() + Duration::from_secs(36);
        let mut samples = vec![pending.clone()];
        loop {
            let current = snapshot(&node);
            let refused = current["runner"][0]["failures"]
                .as_str()
                .unwrap()
                .contains("noMovement");
            samples.push(current);
            if refused || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(750));
        }
        let evidence = json!({
            "root": root.label,
            "target": root.target,
            "samples": samples,
        });
        save(&node, &format!("{}-blocked-route", root.label), &evidence);
        let samples = evidence["samples"].as_array().unwrap();
        assert!(
            samples.iter().any(|sample| sample["runner"][0]["failures"]
                .as_str()
                .unwrap()
                .contains("noMovement")),
            "{evidence}"
        );
        assert!(
            samples.iter().any(|sample| {
                sample["runner"][0]["foreground"]
                    .as_str()
                    .unwrap()
                    .contains("recoveryPosition")
                    && sample["actions"].as_array().unwrap().iter().any(|action| {
                        action["kind"].as_str().unwrap().contains("move")
                            && action["outcome"]
                                .as_str()
                                .unwrap()
                                .contains("status = (blocked = ())")
                    })
            }),
            "{evidence}"
        );
        assert!(
            samples.iter().all(|sample| {
                sample["characters"][0]["x"] == start_position["x"]
                    && sample["characters"][0]["y"] == start_position["y"]
                    && sample["quests"] == pending["quests"]
                    && sample["loot"] == pending["loot"]
            }),
            "{evidence}"
        );
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_recovery_position_expires_with_its_retained_quest() {
    let mut node = Standalone::start("playerbots-action-recovery-position-expiry");
    node.publish_module();
    remove_builtin_weather_import_stamp(&node);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call(
        "playerbots_spawn_class_role",
        &["1", "1200", "1200", "50", "1", "0"],
    );
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    node.assert_call("playerbots_quest_loop_fixture_stage_named", &[&guid]);
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "32"]);
    let accept_deadline = Instant::now() + Duration::from_secs(20);
    while node
        .query_rows(&format!(
            "SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 7"
        ))
        .is_empty()
    {
        assert!(Instant::now() < accept_deadline, "Quest 7 was not accepted");
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        std::thread::sleep(Duration::from_millis(250));
    }
    node.assert_call("playerbots_recovery_fixture_block_quest_target", &[&guid]);
    let recovery_deadline = Instant::now() + Duration::from_secs(20);
    let pending = loop {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let evidence = snapshot(&node);
        if evidence["runner"][0]["foreground"]
            .as_str()
            .unwrap()
            .contains("recoveryPosition")
        {
            break evidence;
        }
        assert!(
            Instant::now() < recovery_deadline,
            "RecoveryPosition was not selected: {evidence}"
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    save(&node, "recovery-position-pending", &pending);
    let foreground = pending["runner"][0]["foreground"].as_str().unwrap();
    let position = structured_number(foreground, "recoveryPosition");
    assert_eq!(
        pending["runner"][0]["candidate_order"]
            .as_str()
            .unwrap()
            .matches(&format!("recoveryPosition = {position}"))
            .count(),
        1,
        "{pending}"
    );

    node.assert_call("playerbots_fixture_runner_expire_objective", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let expired = snapshot(&node);
    save(&node, "recovery-position-expired", &expired);
    let runner = &expired["runner"][0];
    assert_objective_identity(&pending["runner"][0], runner);
    assert!(
        runner["failures"].as_str().unwrap().contains("deadline"),
        "{expired}"
    );
    assert!(
        runner["objective"].as_str().unwrap().contains("deferred"),
        "{expired}"
    );
    assert!(
        runner["history"]
            .as_str()
            .unwrap()
            .contains(&format!("recoveryPosition = {position}")),
        "{expired}"
    );
    assert_stopped_owned_movement(&expired, &guid);
    assert_eq!(expired["actions"], pending["actions"], "{expired}");
    assert_eq!(expired["quests"], pending["quests"], "{expired}");
}

struct TransferFixture {
    node: Standalone,
    companion: String,
    priest: String,
    mage: String,
    leader: String,
}

fn transfer_party(name: &str, blocked: bool) -> TransferFixture {
    let mut node = Standalone::start(name);
    node.publish_module();
    if blocked {
        remove_builtin_weather_import_stamp(&node);
    }
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn", &["4", "1200", "1200", "50"]);
    let mut bots = node.query_rows("SELECT character_guid, class, role FROM pkg_playerbots_bot");
    bots.sort_by_key(|bot| bot["character_guid"].parse::<u64>().unwrap());
    let companion = bots
        .iter()
        .find(|bot| bot["class"] == "1" && bot["role"] == "0")
        .unwrap()["character_guid"]
        .clone();
    let leader = bots
        .iter()
        .rev()
        .find(|bot| bot["class"] == "1" && bot["role"] == "0")
        .unwrap()["character_guid"]
        .clone();
    let priest = bots
        .iter()
        .find(|bot| bot["class"] == "5" && bot["role"] == "1")
        .unwrap()["character_guid"]
        .clone();
    let mage = bots
        .iter()
        .find(|bot| bot["class"] == "8" && bot["role"] == "2")
        .unwrap()["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_prepare", &[]);
    node.assert_call(
        "playerbots_fixture_roles_stage",
        &[&companion, &priest, &mage, &leader],
    );
    for guid in [&companion, &priest, &mage] {
        node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    }
    TransferFixture {
        node,
        companion,
        priest,
        mage,
        leader,
    }
}

fn transfer_fixture(name: &str, mode: u8, blocked: bool) -> TransferFixture {
    let fixture = transfer_party(name, blocked);
    fixture.node.assert_call(
        "playerbots_transfer_fixture_stage",
        &[&fixture.companion, &fixture.leader, &mode.to_string()],
    );
    if blocked {
        fixture
            .node
            .assert_call("playerbots_quest_fixture_stage", &[&fixture.companion]);
        fixture.node.assert_call(
            "playerbots_recovery_fixture_block_companion",
            &[&fixture.companion],
        );
    }
    fixture
}

fn set_transfer_party_membership(fixture: &TransferFixture, mode: u8) {
    fixture.node.assert_call(
        "playerbots_fixture_orders_party",
        &[
            &fixture.companion,
            &fixture.priest,
            &fixture.mage,
            &fixture.leader,
            &mode.to_string(),
        ],
    );
}

fn quest_transfer_fixture(name: &str, mode: u8) -> (TransferFixture, Value) {
    let fixture = transfer_party(name, false);
    remove_builtin_weather_import_stamp(&fixture.node);
    stage_geometry(&fixture.node);
    set_transfer_party_membership(&fixture, 1);
    fixture.node.assert_call(
        "playerbots_quest_loop_fixture_stage_named",
        &[&fixture.companion],
    );
    fixture.node.assert_call(
        "playerbots_action_lifecycle_stage_quest_plan",
        &[
            &fixture.companion,
            &QUEST_ROOTS[5].fixture_kind.to_string(),
            &QUEST_ROOTS[5].target.to_string(),
        ],
    );
    fixture.node.assert_call(
        "playerbots_fixture_runner_select_cohort",
        &[&fixture.companion],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let solo_quest = snapshot(&fixture.node);
    save(&fixture.node, "solo-quest-before-party", &solo_quest);
    let solo_runner = solo_quest["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert!(
        solo_runner["objective"].as_str().unwrap().contains("quest"),
        "{solo_quest}"
    );
    assert!(
        solo_runner["chosen"].as_str().unwrap().contains("entity"),
        "{solo_quest}"
    );

    set_transfer_party_membership(&fixture, 0);
    fixture.node.assert_call(
        "playerbots_transfer_fixture_stage",
        &[&fixture.companion, &fixture.leader, &mode.to_string()],
    );
    (fixture, solo_quest)
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_transfer_root_cancels_its_real_areatrigger_approach() {
    let (fixture, solo_quest) =
        quest_transfer_fixture("playerbots-action-transfer-cancellation", 1);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let pending = snapshot(&fixture.node);
    save(&fixture.node, "transfer-approach-pending", &pending);
    let runner = pending["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_eq!(
        runner["candidate_order"]
            .as_str()
            .unwrap()
            .matches("transfer = (")
            .count(),
        1,
        "{pending}"
    );
    assert!(
        runner["chosen"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{pending}"
    );
    assert!(
        runner["foreground"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{pending}"
    );
    assert!(
        pending["transfers"].as_array().unwrap().is_empty(),
        "{pending}"
    );
    assert_objective_identity(&solo_quest["runner"][0], runner);

    fixture.node.assert_call(
        "playerbots_select_controller",
        &[&fixture.companion, "{\"frozen\":[]}"],
    );
    let cancelled = snapshot(&fixture.node);
    save(&fixture.node, "transfer-approach-cancelled", &cancelled);
    let after = cancelled["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_objective_identity(runner, after);
    assert!(
        after["history"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{cancelled}"
    );
    assert!(
        after["last_outcome"].as_str().unwrap().contains("frozen"),
        "{cancelled}"
    );
    assert_stopped_owned_movement(&cancelled, &fixture.companion);
    assert_eq!(cancelled["actions"], pending["actions"], "{cancelled}");
    assert!(
        cancelled["transfers"].as_array().unwrap().is_empty(),
        "{cancelled}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_transfer_root_expires_before_areatrigger_admission() {
    let (fixture, solo_quest) = quest_transfer_fixture("playerbots-action-transfer-expiry", 1);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let pending = snapshot(&fixture.node);
    save(&fixture.node, "quest-transfer-approach-pending", &pending);
    let before = pending["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_objective_identity(&solo_quest["runner"][0], before);
    assert!(
        before["objective"].as_str().unwrap().contains("quest"),
        "{pending}"
    );
    assert!(
        before["chosen"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{pending}"
    );
    assert!(
        before["foreground"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{pending}"
    );
    assert!(
        pending["transfers"].as_array().unwrap().is_empty(),
        "{pending}"
    );

    fixture.node.assert_call(
        "playerbots_fixture_runner_expire_objective",
        &[&fixture.companion],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let expired = snapshot(&fixture.node);
    save(&fixture.node, "quest-transfer-approach-expired", &expired);
    let after = expired["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_objective_identity(before, after);
    assert!(
        after["failures"].as_str().unwrap().contains("deadline"),
        "{expired}"
    );
    assert!(
        after["objective"].as_str().unwrap().contains("deferred"),
        "{expired}"
    );
    assert!(
        after["history"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{expired}"
    );
    assert_stopped_owned_movement(&expired, &fixture.companion);
    assert_eq!(expired["actions"], pending["actions"], "{expired}");
    assert!(
        expired["transfers"].as_array().unwrap().is_empty(),
        "{expired}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_transfer_records_core_refusal_before_intent() {
    let (fixture, solo_quest) = quest_transfer_fixture("playerbots-action-transfer-refusal", 2);
    fixture.node.assert_call(
        "debug_set_sessionless_action_consent",
        &[&fixture.companion, "false"],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let refused = snapshot(&fixture.node);
    save(&fixture.node, "quest-transfer-refused", &refused);
    let runner = refused["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_objective_identity(&solo_quest["runner"][0], runner);
    assert!(
        runner["chosen"].as_str().unwrap().contains("transfer = ("),
        "{refused}"
    );
    assert!(
        runner["failures"]
            .as_str()
            .unwrap()
            .contains("actionRefused = (cannotAct = ())"),
        "{refused}"
    );
    let actions: Vec<_> = refused["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["character_guid"] == fixture.companion)
        .filter(|action| action["kind"].as_str().unwrap().contains("transfer"))
        .collect();
    assert_eq!(actions.len(), 1, "{refused}");
    assert_eq!(actions[0]["spell_id"], "78", "{refused}");
    assert_eq!(actions[0]["quest_entry"], "36", "{refused}");
    assert!(
        actions[0]["outcome"]
            .as_str()
            .unwrap()
            .contains("refused = (kind = (cannotAct = ())"),
        "{refused}"
    );
    assert!(
        refused["transfers"].as_array().unwrap().is_empty(),
        "{refused}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_areatrigger_move_records_a_verified_blocked_route() {
    let fixture = transfer_fixture("playerbots-action-transfer-blocked-route", 1, true);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let pending = snapshot(&fixture.node);
    save(&fixture.node, "areatrigger-blocked-first", &pending);
    let runner = pending["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert!(
        runner["candidate_order"]
            .as_str()
            .unwrap()
            .contains("transfer = ("),
        "{pending}"
    );
    assert!(
        runner["chosen"]
            .as_str()
            .unwrap()
            .contains("areaTrigger = 78"),
        "{pending}"
    );
    assert!(
        pending["actions"].as_array().unwrap().iter().any(|action| {
            action["kind"].as_str().unwrap().contains("move")
                && action["outcome"]
                    .as_str()
                    .unwrap()
                    .contains("status = (blocked = ())")
        }),
        "{pending}"
    );
    let start_position = pending["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["guid"] == fixture.companion)
        .unwrap()
        .clone();
    fixture
        .node
        .assert_call("playerbots_fixture_companion_due", &[&fixture.companion]);
    let deadline = Instant::now() + Duration::from_secs(36);
    let mut samples = vec![pending.clone()];
    loop {
        let current = snapshot(&fixture.node);
        let stopped = current["runner"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["character_guid"] == fixture.companion)
            .is_some_and(|runner| runner["failures"].as_str().unwrap().contains("noMovement"));
        samples.push(current);
        if stopped || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(750));
    }
    let evidence = json!({"companion": fixture.companion, "samples": samples});
    save(&fixture.node, "areatrigger-blocked-route", &evidence);
    let samples = evidence["samples"].as_array().unwrap();
    assert!(
        samples.iter().any(|sample| sample["runner"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["character_guid"] == fixture.companion)
            .is_some_and(|runner| runner["failures"].as_str().unwrap().contains("noMovement"))),
        "{evidence}"
    );
    assert!(
        samples.iter().all(|sample| sample["characters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["guid"] == fixture.companion)
            .is_some_and(|row| row["x"] == start_position["x"] && row["y"] == start_position["y"])),
        "{evidence}"
    );
    assert!(
        samples
            .iter()
            .all(|sample| sample["transfers"].as_array().unwrap().is_empty()),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_action_resurrection_cancels_between_release_and_spirit_resurrection() {
    let (node, bots) = fixture("playerbots-action-resurrection-cancellation", 1);
    let bot = &bots[0];
    node.assert_call("playerbots_select_controller", &[bot, "{\"cohort\":[]}"]);
    node.assert_call("playerbots_fixture_runner_damage", &[bot, "0", "1000000"]);
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let released = snapshot(&node);
    save(&node, "released", &released);
    assert_eq!(released["characters"][0]["dead"], "true", "{released}");
    assert_ne!(released["characters"][0]["player_flags"], "0", "{released}");
    assert!(
        released["runner"][0]["chosen"]
            .as_str()
            .unwrap()
            .contains("resurrection"),
        "{released}"
    );

    node.assert_call(
        "playerbots_select_controller",
        &[bot, "{\"recordOnly\":[]}"],
    );
    let invalidated = snapshot(&node);
    save(&node, "invalidated", &invalidated);
    assert_eq!(
        invalidated["characters"][0]["dead"], "true",
        "{invalidated}"
    );
    assert_ne!(
        invalidated["characters"][0]["player_flags"], "0",
        "{invalidated}"
    );
    assert!(
        invalidated["runner"][0]["last_outcome"]
            .as_str()
            .unwrap()
            .contains("cancelled"),
        "{invalidated}"
    );
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let cancelled = snapshot(&node);
    save(&node, "cancelled", &cancelled);
    assert_eq!(cancelled["characters"][0]["dead"], "true", "{cancelled}");
    assert_ne!(
        cancelled["characters"][0]["player_flags"], "0",
        "{cancelled}"
    );
    assert!(
        cancelled["runner"][0]["last_outcome"]
            .as_str()
            .unwrap()
            .contains("recorded"),
        "{cancelled}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_expired_quest_does_not_preempt_resurrection_or_defense() {
    let (resurrection_node, resurrection) =
        quest_fixture(QUEST_ROOTS[5], "expired-quest-resurrection-priority", false);
    resurrection_node.assert_call(
        "playerbots_fixture_runner_expire_objective",
        &[&resurrection],
    );
    let resurrection_expired = snapshot(&resurrection_node);
    save(
        &resurrection_node,
        "resurrection-expired-quest",
        &resurrection_expired,
    );
    resurrection_node.assert_call(
        "playerbots_fixture_runner_damage",
        &[&resurrection, &QUEST_ROOTS[5].target.to_string(), "1000000"],
    );
    resurrection_node.assert_call("playerbots_fixture_runner_pass_once", &[&resurrection]);

    let (defense_node, defense) =
        quest_fixture(QUEST_ROOTS[5], "expired-quest-defense-priority", false);
    defense_node.assert_call("playerbots_fixture_runner_expire_objective", &[&defense]);
    let defense_expired = snapshot(&defense_node);
    save(&defense_node, "defense-expired-quest", &defense_expired);
    defense_node.assert_call(
        "playerbots_fixture_runner_damage",
        &[&defense, &QUEST_ROOTS[5].target.to_string(), "1"],
    );
    defense_node.assert_call("playerbots_fixture_runner_pass_once", &[&defense]);

    let evidence = json!({
        "resurrection": {
            "expired_quest": resurrection_expired,
            "after": snapshot(&resurrection_node),
        },
        "defense": {
            "expired_quest": defense_expired,
            "after": snapshot(&defense_node),
        },
    });
    save(&resurrection_node, "resurrection-priority", &evidence);
    save(&defense_node, "defense-priority", &evidence);
    let resurrection_runner = &evidence["resurrection"]["after"]["runner"][0];
    let resurrection_before = &evidence["resurrection"]["expired_quest"]["runner"][0];
    assert_objective_identity(resurrection_before, resurrection_runner);
    assert!(
        resurrection_runner["chosen"]
            .as_str()
            .unwrap()
            .contains("resurrection"),
        "{evidence}"
    );
    assert!(
        !resurrection_runner["failures"]
            .as_str()
            .unwrap()
            .contains("deadline"),
        "{evidence}"
    );
    let defense_runner = &evidence["defense"]["after"]["runner"][0];
    let defense_before = &evidence["defense"]["expired_quest"]["runner"][0];
    assert_objective_identity(defense_before, defense_runner);
    assert!(
        defense_runner["chosen"]
            .as_str()
            .unwrap()
            .contains("defense"),
        "{evidence}"
    );
    assert!(
        !defense_runner["failures"]
            .as_str()
            .unwrap()
            .contains("deadline"),
        "{evidence}"
    );
    assert!(
        evidence["defense"]["after"]["attacks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["attacker_guid"] == defense),
        "{evidence}"
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_admitted_transfer_ignores_an_unrelated_expired_objective() {
    let fixture = transfer_fixture("playerbots-action-admitted-transfer-expiry", 2, false);
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let admitted = snapshot(&fixture.node);
    save(&fixture.node, "admitted", &admitted);
    assert_eq!(
        admitted["transfers"].as_array().unwrap().len(),
        1,
        "{admitted}"
    );
    let runner = admitted["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert!(
        !runner["objective"].as_str().unwrap().contains("none"),
        "{admitted}"
    );
    assert!(
        !runner["transfer_checkpoint"]
            .as_str()
            .unwrap()
            .contains("none"),
        "{admitted}"
    );
    assert!(
        runner["foreground"].as_str().unwrap().contains("none"),
        "{admitted}"
    );

    fixture.node.assert_call(
        "playerbots_fixture_runner_expire_objective",
        &[&fixture.companion],
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let waiting = snapshot(&fixture.node);
    save(&fixture.node, "waiting", &waiting);
    assert_eq!(waiting["transfers"], admitted["transfers"], "{waiting}");
    let runner = waiting["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    let admitted_runner = admitted["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_objective_identity(admitted_runner, runner);
    assert_eq!(
        runner["transfer_checkpoint"], admitted_runner["transfer_checkpoint"],
        "{waiting}"
    );
    assert!(
        !runner["failures"].as_str().unwrap().contains("deadline"),
        "{waiting}"
    );
}
