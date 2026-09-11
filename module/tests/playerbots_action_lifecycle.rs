//! Required PB-011 lifecycle gaps at the real Runner ownership boundary.

mod support;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::Standalone;

const CREATURE_PREFIX: u64 = 0xF130u64 << 48;
const GAMEOBJECT_PREFIX: u64 = 0xF110u64 << 48;
const EXIT_TRIGGER: u32 = 119;
const EXIT_SOURCE: (f32, f32, f32) = (-14.3628, -393.38, 64.5605);
const BLOCKED_QUEST_TARGET: u64 = CREATURE_PREFIX | (5_090_101u64 << 24) | 1;

#[derive(Clone, Copy)]
struct QuestRoot {
    fixture_kind: u8,
    label: &'static str,
    class: &'static str,
    role: &'static str,
    quest: u32,
    target: u64,
    recovery_shape: &'static str,
    prerequisite: &'static str,
    cast_spell: Option<u32>,
}

const QUEST_ROOTS: [QuestRoot; 7] = [
    QuestRoot {
        fixture_kind: 0,
        label: "accept-quest",
        class: "1",
        role: "0",
        quest: 783,
        target: CREATURE_PREFIX | (823u64 << 24) | 1,
        recovery_shape: "operation = (accept = ())",
        prerequisite: "entity",
        cast_spell: None,
    },
    QuestRoot {
        fixture_kind: 1,
        label: "turn-in-quest",
        class: "1",
        role: "0",
        quest: 783,
        target: CREATURE_PREFIX | (197u64 << 24) | 1,
        recovery_shape: "operation = (turnIn = ())",
        prerequisite: "entity",
        cast_spell: None,
    },
    QuestRoot {
        fixture_kind: 2,
        label: "loot-creature",
        class: "1",
        role: "0",
        quest: 33,
        target: CREATURE_PREFIX | (69u64 << 24) | 1,
        recovery_shape: "operation = (lootCreature = ())",
        prerequisite: "entity",
        cast_spell: None,
    },
    QuestRoot {
        fixture_kind: 3,
        label: "use-gameobject",
        class: "8",
        role: "2",
        quest: 3904,
        target: GAMEOBJECT_PREFIX | 161_557,
        recovery_shape: "operation = (useGameObject = ())",
        prerequisite: "gameObject",
        cast_spell: None,
    },
    QuestRoot {
        fixture_kind: 4,
        label: "loot-gameobject",
        class: "8",
        role: "2",
        quest: 3904,
        target: GAMEOBJECT_PREFIX | 161_557,
        recovery_shape: "operation = (lootGameObject = ())",
        prerequisite: "gameObject",
        cast_spell: None,
    },
    QuestRoot {
        fixture_kind: 5,
        label: "attack",
        class: "1",
        role: "0",
        quest: 7,
        target: CREATURE_PREFIX | (6u64 << 24) | 1,
        recovery_shape: "fight = ",
        prerequisite: "entity",
        cast_spell: None,
    },
    QuestRoot {
        fixture_kind: 6,
        label: "quest-cast",
        class: "5",
        role: "1",
        quest: 7,
        target: CREATURE_PREFIX | (6u64 << 24) | 1,
        recovery_shape: "fight = ",
        prerequisite: "castingPosition",
        cast_spell: Some(585),
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

fn stage_exit_geometry(node: &Standalone) {
    let cell_x = lyracore_shared::terrain::cell_index(EXIT_SOURCE.0).unwrap();
    let cell_y = lyracore_shared::terrain::cell_index(EXIT_SOURCE.1).unwrap();
    let mut rows = Vec::new();
    for x in cell_x.saturating_sub(2)..=cell_x.saturating_add(2).min(1023) {
        for y in cell_y.saturating_sub(2)..=cell_y.saturating_add(2).min(1023) {
            rows.push(format!("36,{x},{y},{},,", EXIT_SOURCE.2));
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
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "32"]);
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
    if blocked {
        node.assert_call("playerbots_recovery_fixture_block_companion", &[&guid]);
    }
    std::thread::sleep(Duration::from_millis(1_100));
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
        "runner": sorted(node.query_rows("SELECT character_guid, objective_sequence, objective, foreground, chosen, candidate_order, recovery, deferred_destinations, transfer_checkpoint, last_outcome, failures, history, observed_micros FROM pkg_playerbots_runner"), "character_guid"),
        "retained_quests": sorted(node.query_rows("SELECT character_guid, runner_objective_identity, quest_entry, target, destination FROM pkg_playerbots_quest_objective"), "character_guid"),
        "quest_cast_rotation": sorted(node.query_rows("SELECT class, role, priority, spell_id, condition FROM pkg_playerbots_rotation WHERE class = 5 AND role = 1 AND condition = 0"), "priority"),
        "quest_cast_spellbook": sorted(node.query_rows("SELECT character_guid, spell_id FROM game_player_spell WHERE spell_id = 585"), "character_guid"),
        "actions": sorted(node.query_rows("SELECT character_guid, kind, target_guid, spell_id, quest_entry, outcome, started_micros, observed_micros FROM pkg_playerbots_action"), "character_guid"),
        "movement": sorted(node.query_rows("SELECT guid, sx, sy, sz, dx, dy, dz, start_micros, dur_ms, run FROM game_creature_spline"), "guid"),
        "characters": sorted(node.query_rows("SELECT guid, map_id, instance_id, x, y, z, health, dead, player_flags FROM game_world_entity WHERE guid >= 1000000 AND guid < 2000000"), "guid"),
        "quest_loot_source": node.query_rows(&format!("SELECT guid, entry, map_id, instance_id, x, y, z, health, dead FROM game_world_entity WHERE guid = {}", QUEST_ROOTS[2].target)),
        "casts": sorted(node.query_rows("SELECT caster_guid, scheduled_id, spell_id, target_guid FROM game_pending_cast"), "caster_guid"),
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
    assert_real_quest_root(evidence, root, guid);
    let runner = &evidence["runner"][0];
    let order = runner["candidate_order"].as_str().unwrap();
    assert_eq!(order.matches(root.prerequisite).count(), 1, "{evidence}");
    assert!(order.contains(&root.target.to_string()), "{evidence}");
    let chosen = runner["chosen"].as_str().unwrap();
    assert!(chosen.contains(root.prerequisite), "{evidence}");
    assert!(chosen.contains(&root.target.to_string()), "{evidence}");
    let foreground = runner["foreground"].as_str().unwrap();
    assert!(foreground.contains(root.prerequisite), "{evidence}");
    assert!(foreground.contains(&root.target.to_string()), "{evidence}");
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
}

fn assert_real_quest_root(evidence: &Value, root: QuestRoot, guid: &str) {
    assert_eq!(
        evidence["bot"][0]["controller"], "(cohort = ())",
        "{evidence}"
    );
    let runner = &evidence["runner"][0];
    assert!(
        runner["objective"]
            .as_str()
            .unwrap()
            .contains("kind = (quest = ())")
            && runner["objective"].as_str().unwrap().contains("travelling"),
        "{evidence}"
    );
    let recovery = runner["recovery"].as_str().unwrap();
    assert!(recovery.contains(root.recovery_shape), "{evidence}");
    assert!(recovery.contains(&root.target.to_string()), "{evidence}");
    assert!(
        recovery.contains(&format!(
            "objective = {}",
            runner["objective_sequence"].as_str().unwrap()
        )),
        "{evidence}"
    );
    let retained = evidence["retained_quests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == guid)
        .unwrap_or_else(|| panic!("retained Quest missing: {evidence}"));
    assert_eq!(
        retained["quest_entry"],
        root.quest.to_string(),
        "{evidence}"
    );
    assert_eq!(
        retained["runner_objective_identity"], runner["objective_sequence"],
        "{evidence}"
    );
    assert!(
        retained["destination"]
            .as_str()
            .unwrap()
            .contains(&format!("guid = {}", root.target)),
        "{evidence}"
    );
    if root.fixture_kind == 2 {
        let retained_target = retained["target"].as_str().unwrap();
        assert!(
            retained_target.contains("executor = (creatureLoot = ())")
                && retained_target.contains("entry = 69,")
                && retained_target.contains(&format!("guid = {}", root.target)),
            "{evidence}"
        );
        let source = evidence["quest_loot_source"].as_array().unwrap();
        assert_eq!(source.len(), 1, "{evidence}");
        assert_eq!(source[0]["entry"], "69", "{evidence}");
        assert_eq!(source[0]["health"], "0", "{evidence}");
        assert_eq!(source[0]["dead"], "true", "{evidence}");
        let corpse_guid = root.target.to_string();
        let loot: Vec<_> = evidence["loot"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["corpse_guid"].as_str() == Some(corpse_guid.as_str()))
            .collect();
        assert_eq!(loot.len(), 1, "{evidence}");
        assert_eq!(loot[0]["item_entry"], "750", "{evidence}");
        assert_eq!(loot[0]["count"], "8", "{evidence}");
        let character = evidence["characters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["guid"].as_str() == Some(guid))
            .unwrap();
        assert_eq!(character["dead"], "false", "{evidence}");
        assert_ne!(character["health"], "0", "{evidence}");
    }
    if let Some(spell) = root.cast_spell {
        let rotations = evidence["quest_cast_rotation"].as_array().unwrap();
        assert_eq!(rotations.len(), 1, "{evidence}");
        assert_eq!(rotations[0]["priority"], "10", "{evidence}");
        assert_eq!(rotations[0]["spell_id"], spell.to_string(), "{evidence}");
        let spellbook = evidence["quest_cast_spellbook"].as_array().unwrap();
        assert_eq!(spellbook.len(), 1, "{evidence}");
        assert_eq!(spellbook[0]["character_guid"], guid, "{evidence}");
        assert_eq!(spellbook[0]["spell_id"], spell.to_string(), "{evidence}");
    }
    assert!(
        evidence["transfers"].as_array().unwrap().is_empty(),
        "{evidence}"
    );
}

fn assert_transfer_root(evidence: &Value, guid: &str, action: &str) {
    let runner = evidence["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == guid)
        .unwrap_or_else(|| panic!("Transfer runner missing: {evidence}"));
    let objective = runner["objective"].as_str().unwrap();
    assert!(objective.contains("kind = (companion = ())"), "{evidence}");
    let identity = runner["objective_sequence"].as_str().unwrap();
    assert!(
        objective.contains(&format!("identity = {identity}")),
        "{evidence}"
    );
    let recovery = runner["recovery"].as_str().unwrap();
    assert!(recovery.contains("work = (areaTrigger = 78)"), "{evidence}");
    assert!(recovery.contains("reason = (transfer = ())"), "{evidence}");
    assert!(
        recovery.contains(&format!("objective = {identity}")),
        "{evidence}"
    );
    assert!(
        runner["chosen"].as_str().unwrap().contains(action),
        "{evidence}"
    );
}

fn assert_no_active_owned_movement(evidence: &Value, guid: &str) {
    let owned: Vec<_> = evidence["movement"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["guid"].as_str() == Some(guid))
        .collect();
    assert!(owned.len() <= 1, "{evidence}");
    if let Some(stopped) = owned.first() {
        assert_eq!(stopped["dur_ms"], "0", "{evidence}");
        assert_eq!(stopped["run"], "false", "{evidence}");
        assert_eq!(stopped["sx"], stopped["dx"], "{evidence}");
        assert_eq!(stopped["sy"], stopped["dy"], "{evidence}");
        assert_eq!(stopped["sz"], stopped["dz"], "{evidence}");
    }
}

fn assert_quest_root_effects_unchanged(before: &Value, after: &Value, root: QuestRoot, guid: &str) {
    let root_quest = root.quest.to_string();
    let root_target = root.target.to_string();
    let action_kind = match root.fixture_kind {
        0 => "acceptQuest",
        1 => "turnInQuest",
        2 | 4 => "openLoot",
        3 => "useGameObject",
        5 => "attack",
        6 => "cast",
        _ => unreachable!(),
    };
    let root_rows = |evidence: &Value, table: &str, guid_field: &str, kind: Option<&str>| {
        evidence[table]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row[guid_field].as_str() == Some(guid))
            .filter(|row| row["target_guid"].as_str() == Some(root_target.as_str()))
            .filter(|row| kind.is_none_or(|kind| row["kind"].as_str().unwrap().contains(kind)))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        root_rows(after, "actions", "character_guid", Some(action_kind)),
        root_rows(before, "actions", "character_guid", Some(action_kind)),
        "{after}"
    );
    assert_eq!(
        root_rows(after, "attacks", "attacker_guid", None),
        root_rows(before, "attacks", "attacker_guid", None),
        "{after}"
    );
    assert_eq!(
        root_rows(after, "casts", "caster_guid", None),
        root_rows(before, "casts", "caster_guid", None),
        "{after}"
    );
    let quest_rows = |evidence: &Value| {
        evidence["quests"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["character_guid"].as_str() == Some(guid))
            .filter(|row| row["quest_entry"].as_str() == Some(root_quest.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(quest_rows(after), quest_rows(before), "{after}");
    let loot_rows = |evidence: &Value| {
        evidence["loot"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["corpse_guid"].as_str() == Some(root_target.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(loot_rows(after), loot_rows(before), "{after}");
}

fn run_root_cases(roots: &[QuestRoot], mut run: impl FnMut(QuestRoot)) {
    let failures: Vec<_> = roots
        .iter()
        .filter_map(|root| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(*root)))
                .err()
                .map(|panic| {
                    let detail = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("panic without a message");
                    format!("{}: {detail}", root.label)
                })
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
    node.assert_call("playerbots_fixture_runner_select_cohort", &[&guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[&guid, "32"]);
    node.assert_call("playerbots_fixture_runner_stage", &[&guid, "false"]);
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut attempts = Vec::new();
    let pending = loop {
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let current = snapshot(&node);
        let runner = &current["runner"][0];
        let ready = runner["chosen"].as_str().unwrap().contains("home")
            && runner["foreground"].as_str().unwrap().contains("home")
            && current["movement"].as_array().unwrap().iter().any(|row| {
                row["guid"].as_str() == Some(guid.as_str()) && row["dur_ms"].as_str() != Some("0")
            });
        attempts.push(current.clone());
        if ready || Instant::now() >= deadline {
            break current;
        }
        std::thread::sleep(Duration::from_millis(1_100));
    };
    save(&node, "home-pending-attempts", &json!(attempts));
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
    assert_no_active_owned_movement(&cancelled, &guid);
    assert_eq!(cancelled["actions"], pending["actions"], "{cancelled}");
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after_retry = snapshot(&node);
    save(&node, "home-after-cancellation", &after_retry);
    assert_eq!(
        after_retry["actions"], cancelled["actions"],
        "{after_retry}"
    );
    assert_eq!(
        after_retry["characters"], cancelled["characters"],
        "{after_retry}"
    );
    assert_no_active_owned_movement(&after_retry, &guid);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_return_home_expires_its_real_owned_movement() {
    let (node, guid, pending) = home_pending("expiry");
    let identity = pending["runner"][0]["objective_sequence"].as_str().unwrap();
    std::thread::sleep(Duration::from_millis(1_100));
    node.assert_call(
        "playerbots_action_lifecycle_expire_movement",
        &[&guid, identity, "0", "0"],
    );
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
    assert_no_active_owned_movement(&expired, &guid);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let after_retry = snapshot(&node);
    save(&node, "home-after-expiry", &after_retry);
    assert_eq!(after_retry["actions"], expired["actions"], "{after_retry}");
    assert_eq!(
        after_retry["characters"], expired["characters"],
        "{after_retry}"
    );
    assert_no_active_owned_movement(&after_retry, &guid);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_home_move_records_a_verified_blocked_route() {
    let (node, bots) = fixture("playerbots-action-home-blocked-route", 1);
    let guid = &bots[0];
    node.assert_call("playerbots_fixture_runner_select_cohort", &[guid]);
    node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    node.assert_call("playerbots_fixture_blocked_quest", &[guid]);
    node.assert_call("playerbots_fixture_runner_stage", &[guid, "false"]);
    node.assert_call("gw_abandon_quest", &[&support::actor(guid), "50909"]);
    node.assert_call(
        "playerbots_fixture_roles_despawn",
        &[&BLOCKED_QUEST_TARGET.to_string()],
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut attempts = Vec::new();
    let first = loop {
        node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
        let current = snapshot(&node);
        let runner = &current["runner"][0];
        let blocked_home = runner["chosen"]
            .as_str()
            .unwrap()
            .contains("move = (home = ())")
            && current["actions"].as_array().unwrap().iter().any(|action| {
                action["kind"].as_str().unwrap().contains("move")
                    && action["outcome"]
                        .as_str()
                        .unwrap()
                        .contains("status = (blocked = ())")
            });
        attempts.push(current.clone());
        if blocked_home || Instant::now() >= deadline {
            break current;
        }
        std::thread::sleep(Duration::from_millis(1_100));
    };
    save(&node, "home-blocked-first-attempts", &json!(attempts));
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
    run_root_cases(&QUEST_ROOTS, |root| {
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
        assert_eq!(after["foreground"], "(none = ())", "{cancelled}");
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
        assert_no_active_owned_movement(&cancelled, &guid);
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
        node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        let after_retry = snapshot(&node);
        save(
            &node,
            &format!("{}-after-cancellation", root.label),
            &after_retry,
        );
        assert_eq!(
            after_retry["actions"], cancelled["actions"],
            "{after_retry}"
        );
        assert_eq!(after_retry["quests"], cancelled["quests"], "{after_retry}");
        assert_eq!(after_retry["loot"], cancelled["loot"], "{after_retry}");
        assert_no_active_owned_movement(&after_retry, &guid);
    });
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_roots_expire_their_real_movement_prerequisites() {
    run_root_cases(&QUEST_ROOTS, |root| {
        let (node, guid) = quest_fixture(root, "expiry", false);
        let pending = snapshot(&node);
        save(&node, &format!("{}-pending", root.label), &pending);
        assert_real_pending_root(&pending, root, &guid);

        let identity = pending["runner"][0]["objective_sequence"].as_str().unwrap();
        let expected_move = match root.fixture_kind {
            3 | 4 => "2",
            6 => "3",
            _ => "1",
        };
        std::thread::sleep(Duration::from_millis(1_100));
        node.assert_call(
            "playerbots_action_lifecycle_expire_movement",
            &[&guid, identity, expected_move, &root.target.to_string()],
        );
        let expired = snapshot(&node);
        save(&node, &format!("{}-expired", root.label), &expired);
        let before = &pending["runner"][0];
        let after = &expired["runner"][0];
        assert_objective_identity(before, after);
        assert_eq!(after["foreground"], "(none = ())", "{expired}");
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
        assert_no_active_owned_movement(&expired, &guid);
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
        assert_quest_root_effects_unchanged(&expired, &after_retry, root, &guid);
        let old_identity = before["objective_sequence"].as_str().unwrap();
        assert!(
            !after_retry["runner"][0]["foreground"]
                .as_str()
                .unwrap()
                .contains(&format!("objective = {old_identity})")),
            "{after_retry}"
        );
    });
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_quest_move_targets_record_verified_blocked_routes() {
    run_root_cases(&[QUEST_ROOTS[3], QUEST_ROOTS[5], QUEST_ROOTS[6]], |root| {
        let (node, guid) = quest_fixture(root, "blocked-route", true);
        let pending = snapshot(&node);
        save(&node, &format!("{}-blocked-first", root.label), &pending);
        assert_real_quest_root(&pending, root, &guid);
        let runner = &pending["runner"][0];
        let order = runner["candidate_order"].as_str().unwrap();
        assert_eq!(order.matches(root.prerequisite).count(), 1, "{pending}");
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
        let root_quest = root.quest.to_string();
        assert!(
            samples.iter().all(|sample| {
                sample["characters"][0]["x"] == start_position["x"]
                    && sample["characters"][0]["y"] == start_position["y"]
                    && sample["loot"] == pending["loot"]
                    && sample["quests"].as_array().unwrap().iter().any(|quest| {
                        quest["character_guid"] == guid
                            && quest["quest_entry"].as_str() == Some(root_quest.as_str())
                            && pending["quests"].as_array().unwrap().contains(quest)
                    })
            }),
            "{evidence}"
        );
    });
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
    assert_real_quest_root(&pending, QUEST_ROOTS[5], &guid);
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
    assert!(
        pending["movement"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["guid"] != guid),
        "a blocked route must not claim a live spline: {pending}"
    );

    node.assert_call("playerbots_fixture_runner_expire_objective", &[&guid]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    let expired = snapshot(&node);
    save(&node, "recovery-position-expired", &expired);
    let runner = &expired["runner"][0];
    let retained_identity = pending["runner"][0]["objective_sequence"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let current_identity = runner["objective_sequence"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(current_identity >= retained_identity, "{expired}");
    assert!(
        runner["failures"].as_str().unwrap().contains("deadline"),
        "{expired}"
    );
    let expired_position = format!(
        "action = (move = (recoveryPosition = {position})), reason = (quest = ()), objective = {retained_identity}), priority = 110)), outcome = "
    );
    let history = runner["history"].as_str().unwrap();
    assert!(
        history.contains(&format!("{expired_position}(cancelled = ())"))
            && history.contains(&format!("{expired_position}(refused = (deadline = ()))")),
        "{expired}"
    );
    let recovery = runner["recovery"].as_str().unwrap();
    assert!(
        recovery.contains(&format!("fight = {}", QUEST_ROOTS[5].target))
            && recovery.contains(&format!("objective = {retained_identity}")),
        "{expired}"
    );
    let deferral = runner["deferred_destinations"].as_str().unwrap();
    assert_eq!(
        pending["runner"][0]["deferred_destinations"], "",
        "{pending}"
    );
    let deferred_until = structured_number(deferral, "until_micros")
        .parse::<i64>()
        .unwrap();
    let observed_micros = runner["observed_micros"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(
        deferral.contains("x = 1360, y = 1200, z = 50")
            && deferred_until > observed_micros
            && deferred_until <= observed_micros.saturating_add(30_000_000),
        "{expired}"
    );
    let objective = runner["objective"].as_str().unwrap();
    assert!(
        objective.contains(&format!("identity = {current_identity}"))
            && objective.contains("kind = (quest = ())"),
        "{expired}"
    );
    let retained = expired["retained_quests"].as_array().unwrap();
    assert_eq!(retained.len(), 1, "{expired}");
    let current_identity_text = current_identity.to_string();
    assert_eq!(
        retained[0]["runner_objective_identity"].as_str(),
        Some(current_identity_text.as_str()),
        "{expired}"
    );
    let foreground = runner["foreground"].as_str().unwrap();
    assert!(!foreground.contains("recoveryPosition"), "{expired}");
    if current_identity == retained_identity {
        assert!(objective.contains("stage = (deferred = ())"), "{expired}");
        assert_eq!(retained[0]["quest_entry"], "7", "{expired}");
    } else {
        assert_ne!(retained[0]["quest_entry"], "7", "{expired}");
        let chosen = runner["chosen"].as_str().unwrap();
        assert!(
            chosen.contains(&format!("objective = {current_identity}"))
                && chosen.contains("reason = (quest = ())")
                && !chosen.contains("recoveryPosition")
                && (foreground == "(none = ())"
                    || foreground.contains(&format!("objective = {current_identity}"))),
            "{expired}"
        );
    }
    assert!(
        expired["movement"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["guid"] != guid),
        "expiry of a blocked route must not manufacture a stopped spline: {expired}"
    );
    let retained_quest = |evidence: &Value| {
        evidence["quests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|quest| quest["quest_entry"] == "7")
            .cloned()
            .unwrap()
    };
    assert_eq!(
        retained_quest(&expired),
        retained_quest(&pending),
        "{expired}"
    );
}

struct TransferFixture {
    node: Standalone,
    companion: String,
    priest: String,
    mage: String,
    leader: String,
}

fn spawn_transfer_party(name: &str) -> TransferFixture {
    let mut node = Standalone::start(name);
    node.publish_module();
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
    TransferFixture {
        node,
        companion,
        priest,
        mage,
        leader,
    }
}

fn stage_transfer_roles(fixture: &TransferFixture) {
    let node = &fixture.node;
    node.assert_call(
        "playerbots_fixture_roles_stage",
        &[
            &fixture.companion,
            &fixture.priest,
            &fixture.mage,
            &fixture.leader,
        ],
    );
    for guid in [&fixture.companion, &fixture.priest, &fixture.mage] {
        node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    }
}

fn transfer_party(name: &str) -> TransferFixture {
    let fixture = spawn_transfer_party(name);
    fixture.node.assert_call("playerbots_fixture_prepare", &[]);
    stage_transfer_roles(&fixture);
    fixture
}

fn transfer_fixture(name: &str, mode: u8, blocked: bool) -> TransferFixture {
    let fixture = if blocked {
        let fixture = spawn_transfer_party(name);
        remove_builtin_weather_import_stamp(&fixture.node);
        fixture
            .node
            .assert_call("playerbots_quest_fixture_stage", &[&fixture.companion]);
        stage_transfer_roles(&fixture);
        fixture
    } else {
        transfer_party(name)
    };
    fixture.node.assert_call(
        "playerbots_transfer_fixture_stage",
        &[&fixture.companion, &fixture.leader, &mode.to_string()],
    );
    if blocked {
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
    let fixture = spawn_transfer_party(name);
    remove_builtin_weather_import_stamp(&fixture.node);
    stage_geometry(&fixture.node);
    fixture.node.assert_call(
        "playerbots_quest_loop_fixture_stage_named",
        &[&fixture.companion],
    );
    stage_transfer_roles(&fixture);
    set_transfer_party_membership(&fixture, 1);
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
        .find(|row| row["character_guid"].as_str() == Some(fixture.companion.as_str()))
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
        .find(|row| row["character_guid"].as_str() == Some(fixture.companion.as_str()))
        .unwrap();
    assert_transfer_root(&pending, &fixture.companion, "areaTrigger = 78");
    assert_eq!(
        runner["candidate_order"]
            .as_str()
            .unwrap()
            .matches("areaTrigger = 78")
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
    let solo_runner = solo_quest["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"].as_str() == Some(fixture.companion.as_str()))
        .unwrap();
    assert_ne!(
        solo_runner["objective_sequence"], runner["objective_sequence"],
        "the party must replace the earlier solo Quest purpose: {pending}"
    );

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
    assert_no_active_owned_movement(&cancelled, &fixture.companion);
    assert_eq!(cancelled["actions"], pending["actions"], "{cancelled}");
    assert!(
        cancelled["transfers"].as_array().unwrap().is_empty(),
        "{cancelled}"
    );
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let after_retry = snapshot(&fixture.node);
    save(&fixture.node, "transfer-after-cancellation", &after_retry);
    assert_eq!(
        after_retry["actions"], cancelled["actions"],
        "{after_retry}"
    );
    assert_eq!(
        after_retry["transfers"], cancelled["transfers"],
        "{after_retry}"
    );
    assert_no_active_owned_movement(&after_retry, &fixture.companion);
}

fn return_home_transfer_pending(name: &str, direct_transfer: bool) -> (TransferFixture, Value) {
    let fixture = spawn_transfer_party(name);
    remove_builtin_weather_import_stamp(&fixture.node);
    stage_exit_geometry(&fixture.node);
    stage_transfer_roles(&fixture);
    fixture.node.assert_call(
        "playerbots_transfer_fixture_stage",
        &[&fixture.companion, &fixture.leader, "3"],
    );
    set_transfer_party_membership(&fixture, 1);
    if direct_transfer {
        fixture.node.assert_call(
            "playerbots_select_controller",
            &[&fixture.companion, "{\"recordOnly\":[]}"],
        );
    } else {
        fixture.node.assert_call(
            "playerbots_fixture_position",
            &[&fixture.companion, &(EXIT_SOURCE.0 + 40.0).to_string()],
        );
        fixture.node.assert_call(
            "playerbots_fixture_runner_select_cohort",
            &[&fixture.companion],
        );
    }
    fixture
        .node
        .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    let mut pending = snapshot(&fixture.node);
    pending["return_home_memberships"] = json!(fixture.node.query_rows(&format!(
        "SELECT group_id FROM game_group_member WHERE character_guid = {}",
        fixture.companion
    )));
    save(&fixture.node, "return-home-transfer-pending", &pending);
    let runner = pending["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    let objective = runner["objective"].as_str().unwrap();
    assert!(objective.contains("kind = (returnHome = ())"), "{pending}");
    assert!(objective.contains("stage = (travelling = ())"), "{pending}");
    assert!(
        !objective.contains("deadline_micros = 9223372036854775807"),
        "ReturnHome must retain its finite deadline: {pending}"
    );
    let identity = runner["objective_sequence"].as_str().unwrap();
    assert!(
        objective.contains(&format!("identity = {identity}")),
        "{pending}"
    );
    assert!(
        pending["return_home_memberships"]
            .as_array()
            .unwrap()
            .is_empty(),
        "ReturnHome Transfer must be selected outside party control: {pending}"
    );
    let body = pending["characters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["guid"] == fixture.companion)
        .unwrap();
    assert_eq!(body["map_id"], "36", "{pending}");
    assert_eq!(body["instance_id"], "5098078", "{pending}");
    let distance_sq = (body["x"].as_str().unwrap().parse::<f32>().unwrap() - EXIT_SOURCE.0).powi(2)
        + (body["y"].as_str().unwrap().parse::<f32>().unwrap() - EXIT_SOURCE.1).powi(2)
        + (body["z"].as_str().unwrap().parse::<f32>().unwrap() - EXIT_SOURCE.2).powi(2);
    assert_eq!(distance_sq <= 36.0, direct_transfer, "{pending}");
    let expected_action = if direct_transfer {
        format!("transfer = (trigger = {EXIT_TRIGGER}")
    } else {
        format!("areaTrigger = {EXIT_TRIGGER}")
    };
    assert_eq!(
        runner["candidate_order"]
            .as_str()
            .unwrap()
            .matches(&expected_action)
            .count(),
        1,
        "{pending}"
    );
    assert!(
        runner["chosen"]
            .as_str()
            .unwrap()
            .contains(&expected_action),
        "{pending}"
    );
    if direct_transfer {
        assert_eq!(runner["foreground"], "(none = ())", "{pending}");
        assert!(
            pending["actions"].as_array().unwrap().is_empty(),
            "{pending}"
        );
        assert_no_active_owned_movement(&pending, &fixture.companion);
    } else {
        assert!(
            runner["foreground"]
                .as_str()
                .unwrap()
                .contains(&format!("areaTrigger = {EXIT_TRIGGER}")),
            "{pending}"
        );
        let recovery = runner["recovery"].as_str().unwrap();
        assert!(
            recovery.contains(&format!("work = (areaTrigger = {EXIT_TRIGGER})")),
            "{pending}"
        );
        assert!(recovery.contains("reason = (transfer = ())"), "{pending}");
        assert!(
            recovery.contains(&format!("objective = {identity}")),
            "{pending}"
        );
        assert_eq!(
            pending["movement"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["guid"] == fixture.companion)
                .count(),
            1,
            "{pending}"
        );
    }
    assert!(
        pending["transfers"].as_array().unwrap().is_empty(),
        "{pending}"
    );
    (fixture, pending)
}

fn assert_return_home_transfer_expired(
    fixture: &TransferFixture,
    pending: &Value,
    expected_action: &str,
    direct_transfer: bool,
    label: &str,
) {
    if direct_transfer {
        fixture.node.assert_call(
            "playerbots_select_controller",
            &[&fixture.companion, "{\"frozen\":[]}"],
        );
        fixture.node.assert_call(
            "playerbots_fixture_runner_expire_objective",
            &[&fixture.companion],
        );
        fixture.node.assert_call(
            "playerbots_fixture_runner_select_cohort",
            &[&fixture.companion],
        );
        fixture
            .node
            .assert_call("playerbots_fixture_runner_pass_once", &[&fixture.companion]);
    } else {
        std::thread::sleep(Duration::from_millis(1_100));
        fixture.node.assert_call(
            "playerbots_action_lifecycle_expire_movement",
            &[
                &fixture.companion,
                pending["runner"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["character_guid"].as_str() == Some(fixture.companion.as_str()))
                    .unwrap()["objective_sequence"]
                    .as_str()
                    .unwrap(),
                "4",
                &EXIT_TRIGGER.to_string(),
            ],
        );
    }
    let expired = snapshot(&fixture.node);
    save(&fixture.node, label, &expired);
    let runner = expired["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    let before = pending["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"] == fixture.companion)
        .unwrap();
    assert_objective_identity(before, runner);
    let candidate_order = runner["candidate_order"].as_str().unwrap();
    assert_eq!(
        candidate_order.matches(expected_action).count(),
        1,
        "{expired}"
    );
    if direct_transfer {
        assert!(
            !candidate_order.contains(&format!("areaTrigger = {EXIT_TRIGGER}")),
            "the in-volume expiry pass must select Transfer directly: {expired}"
        );
    } else {
        assert!(
            runner["chosen"]
                .as_str()
                .unwrap()
                .contains(&format!("areaTrigger = {EXIT_TRIGGER}")),
            "{expired}"
        );
    }
    assert!(
        runner["failures"].as_str().unwrap().contains("deadline"),
        "{expired}"
    );
    assert!(
        runner["objective"].as_str().unwrap().contains("deferred"),
        "{expired}"
    );
    assert_no_active_owned_movement(&expired, &fixture.companion);
    if direct_transfer {
        assert_eq!(expired["actions"], pending["actions"], "{expired}");
    }
    assert!(
        expired["transfers"].as_array().unwrap().is_empty(),
        "{expired}"
    );
}

fn run_return_home_transfer_expiry(direct_transfer: bool) {
    let label = if direct_transfer {
        "playerbots-action-return-home-transfer-expiry"
    } else {
        "playerbots-action-return-home-transfer-position-expiry"
    };
    let (fixture, pending) = return_home_transfer_pending(label, direct_transfer);
    let expected_action = if direct_transfer {
        format!("transfer = (trigger = {EXIT_TRIGGER}")
    } else {
        format!("areaTrigger = {EXIT_TRIGGER}")
    };
    let evidence_label = if direct_transfer {
        "return-home-transfer-expired"
    } else {
        "return-home-transfer-position-expired"
    };
    assert_return_home_transfer_expired(
        &fixture,
        &pending,
        &expected_action,
        direct_transfer,
        evidence_label,
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_return_home_transfer_roots_expire_before_admission() {
    let failures: Vec<_> = [false, true]
        .into_iter()
        .filter_map(|direct_transfer| {
            std::panic::catch_unwind(|| run_return_home_transfer_expiry(direct_transfer))
                .err()
                .map(|panic| {
                    let detail = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("panic without a message");
                    format!(
                        "{}: {detail}",
                        if direct_transfer {
                            "Transfer"
                        } else {
                            "Move.AreaTrigger"
                        }
                    )
                })
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
    assert_transfer_root(&refused, &fixture.companion, "transfer = (");
    let solo_runner = solo_quest["runner"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["character_guid"].as_str() == Some(fixture.companion.as_str()))
        .unwrap();
    assert_ne!(
        solo_runner["objective_sequence"], runner["objective_sequence"],
        "the party must replace the earlier solo Quest purpose: {refused}"
    );
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
    assert_transfer_root(&pending, &fixture.companion, "areaTrigger = 78");
    assert_eq!(
        runner["candidate_order"]
            .as_str()
            .unwrap()
            .matches("areaTrigger = 78")
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
    node.assert_call("playerbots_fixture_runner_select_cohort", &[bot]);
    node.assert_call("playerbots_fixture_runner_damage", &[bot, "0", "1000000"]);
    node.assert_call("playerbots_fixture_runner_pass_once", &[bot]);
    let released = snapshot(&node);
    save(&node, "released", &released);
    assert_eq!(released["characters"][0]["dead"], "true", "{released}");
    assert_ne!(released["characters"][0]["player_flags"], "0", "{released}");
    let released_runner = &released["runner"][0];
    let accepted_resurrection = released_runner["chosen"].as_str().unwrap();
    assert!(
        accepted_resurrection.contains("action = (resurrect = ())")
            && accepted_resurrection.contains("reason = (resurrection = ())"),
        "{released}"
    );
    assert_eq!(released_runner["foreground"], "(none = ())", "{released}");
    assert_eq!(
        released_runner["last_outcome"], "(accepted = ())",
        "{released}"
    );
    let released_history = released_runner["history"].as_str().unwrap();
    let accepted_record = format!("chosen = {accepted_resurrection}, outcome = (accepted = ())");
    assert_eq!(
        released_history.matches(&accepted_record).count(),
        1,
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
    let invalidated_runner = &invalidated["runner"][0];
    assert_eq!(
        invalidated_runner["foreground"], "(none = ())",
        "{invalidated}"
    );
    let invalidated_history = invalidated_runner["history"].as_str().unwrap();
    let appended_history = invalidated_history
        .strip_prefix(released_history)
        .expect("controller selection must retain the prior Resurrection history");
    let cancelled_record = format!("chosen = {accepted_resurrection}, outcome = (cancelled = ())");
    let recorded_record = format!("chosen = {accepted_resurrection}, outcome = (recorded = ())");
    assert_eq!(
        appended_history.matches(&cancelled_record).count(),
        1,
        "{invalidated}"
    );
    let recorded_count = appended_history.matches(&recorded_record).count();
    assert!(recorded_count <= 1, "{invalidated}");
    assert_eq!(
        appended_history.matches("(at_micros = ").count(),
        1 + recorded_count,
        "{invalidated}"
    );
    if let Some(recorded_at) = appended_history.find(&recorded_record) {
        assert!(
            appended_history.find(&cancelled_record).unwrap() < recorded_at,
            "{invalidated}"
        );
    }
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let cancelled = snapshot(&node);
    save(&node, "cancelled", &cancelled);
    assert_eq!(cancelled["characters"][0]["dead"], "true", "{cancelled}");
    assert_ne!(
        cancelled["characters"][0]["player_flags"], "0",
        "{cancelled}"
    );
    assert_eq!(
        cancelled["runner"][0]["last_outcome"], "(recorded = ())",
        "{cancelled}"
    );
    let final_history = cancelled["runner"][0]["history"].as_str().unwrap();
    assert_eq!(
        final_history.matches(&cancelled_record).count(),
        1,
        "{cancelled}"
    );
    assert_eq!(
        final_history.matches(&recorded_record).count(),
        1,
        "{cancelled}"
    );
    assert!(
        final_history.find(&cancelled_record).unwrap()
            < final_history.find(&recorded_record).unwrap(),
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
    defense_node.assert_call("playerbots_fixture_companion_health", &[&defense, "100"]);
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
    let defense_target = QUEST_ROOTS[5].target.to_string();
    assert!(
        defense_runner["chosen"]
            .as_str()
            .unwrap()
            .contains(&format!("entity = {defense_target}"))
            && defense_runner["foreground"]
                .as_str()
                .unwrap()
                .contains(&format!("entity = {defense_target}"))
            && defense_runner["recovery"]
                .as_str()
                .unwrap()
                .contains(&format!("fight = {defense_target}")),
        "{evidence}"
    );
    assert_eq!(
        evidence["defense"]["after"]["movement"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["guid"] == defense)
            .count(),
        1,
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
        runner["objective"].as_str().unwrap().starts_with("(some ="),
        "{admitted}"
    );
    assert!(
        runner["transfer_checkpoint"]
            .as_str()
            .unwrap()
            .starts_with("(some ="),
        "{admitted}"
    );
    assert_eq!(runner["foreground"], "(none = ())", "{admitted}");

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
