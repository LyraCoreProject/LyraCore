//! Locked GOOBER use through the Module boundary.

mod support;

use support::Standalone;

const CHARACTER_GUID: u64 = 1;
const QUEST_ENTRY: u32 = 50_900;
const QUEST_OBJECTIVE_ID: u64 = 5_090_000;
const LOCKED_GOOBER_ENTRY: u32 = 5_093_670;
const LOCKLESS_GOOBER_ENTRY: u32 = 5_093_672;
const LOCK_ID: u32 = 5_093_671;
const KEY_ITEM_ENTRY: u32 = 5_090_052;
const SCRIPT_HASH: &str = "3673673673673673673673673673673673673673673673673673673673673673";

fn json_arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

fn install_go_used_hook(standalone: &Standalone) {
    let artifact = serde_json::json!({
        "kind": "script",
        "version": 1,
        "package": "fixture.goober-lock",
        "source_hash": SCRIPT_HASH,
        "scripts": [{
            "script_id": 367_367,
            "name": "goober-lock.used",
            "event": "on_go_used",
            "priority": 0,
            "enabled": true,
            "source": "grant_xp(event.actor, 7)",
        }],
    })
    .to_string();
    standalone.assert_call(
        "apply_package_deltas",
        &[&json_arg("script"), &json_arg(&artifact)],
    );
}

fn spawn_goober(standalone: &Standalone, entry: u32) {
    let args = [
        entry.to_string(),
        "10".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "100".to_string(),
        "100".to_string(),
        "100".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
    ];
    standalone.assert_call(
        "debug_spawn_gameobject",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn quest_count(standalone: &Standalone) -> u32 {
    let rows = standalone.query_rows(&format!(
        "SELECT counts FROM game_character_quest WHERE character_guid = {CHARACTER_GUID} AND quest_entry = {QUEST_ENTRY}"
    ));
    let counts = rows.first().expect("the quest is in the log")["counts"]
        .trim_matches(['[', ']'])
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<u32>().expect("quest count is a number"))
        .collect::<Vec<_>>();
    counts[0]
}

fn xp(standalone: &Standalone) -> u32 {
    standalone
        .query_rows(&format!(
            "SELECT xp FROM game_world_entity WHERE guid = {CHARACTER_GUID}"
        ))
        .first()
        .expect("the Character is live")["xp"]
        .parse()
        .expect("xp is a number")
}

fn goober_state(standalone: &Standalone, entry: u32) -> u8 {
    standalone
        .query_rows(&format!(
            "SELECT state FROM game_gameobject WHERE template_entry = {entry}"
        ))
        .first()
        .expect("the GOOBER is spawned")["state"]
        .parse()
        .expect("state is a number")
}

fn unlocked_count(standalone: &Standalone) -> usize {
    standalone
        .query_rows("SELECT go_guid FROM game_gameobject_unlocked")
        .len()
}

fn key_count(standalone: &Standalone) -> u32 {
    standalone
        .query_rows(&format!(
            "SELECT stack_count FROM game_item_instance WHERE owner_guid = {CHARACTER_GUID} AND entry = {KEY_ITEM_ENTRY}"
        ))
        .iter()
        .map(|row| {
            row["stack_count"]
                .parse::<u32>()
                .expect("stack count is a number")
        })
        .sum()
}

/// Runs only when requested because it builds and publishes the Module to a private Standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn locked_goober_use_is_atomic_until_an_opener_unlocks_it() {
    let standalone = Standalone::start("goober-lock");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER_GUID.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = {CHARACTER_GUID}"
    ));

    spawn_goober(&standalone, LOCKED_GOOBER_ENTRY);
    standalone.assert_sql(&format!(
        "UPDATE game_gameobject_template SET lock_id = {LOCK_ID} WHERE entry = {LOCKED_GOOBER_ENTRY}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_lock (id, lock_id, index, kind, property, required_skill) VALUES (5093671, {LOCK_ID}, 0, 1, {KEY_ITEM_ENTRY}, 0)"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_quest_objective SET kind = 2, target_entry = {LOCKED_GOOBER_ENTRY}, required_count = 2 WHERE id = {QUEST_OBJECTIVE_ID}"
    ));
    standalone.assert_call(
        "debug_grant_quest",
        &[&CHARACTER_GUID.to_string(), &QUEST_ENTRY.to_string()],
    );
    install_go_used_hook(&standalone);

    let before_quest = quest_count(&standalone);
    let before_xp = xp(&standalone);
    let before_state = goober_state(&standalone, LOCKED_GOOBER_ENTRY);
    let refused = standalone.call(
        "debug_use_gameobject_entry",
        &[
            &CHARACTER_GUID.to_string(),
            &LOCKED_GOOBER_ENTRY.to_string(),
        ],
    );
    let refused_output = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success() && refused_output.contains("it is locked"),
        "locked use should be refused before any effect:\n{refused_output}"
    );
    assert_eq!(quest_count(&standalone), before_quest);
    assert_eq!(xp(&standalone), before_xp);
    assert_eq!(goober_state(&standalone, LOCKED_GOOBER_ENTRY), before_state);
    assert_eq!(unlocked_count(&standalone), 0);

    let pick_without_key = standalone.call(
        "debug_pick_lock_entry",
        &[
            &CHARACTER_GUID.to_string(),
            &LOCKED_GOOBER_ENTRY.to_string(),
        ],
    );
    assert!(!pick_without_key.status.success());
    assert_eq!(unlocked_count(&standalone), 0);

    standalone.assert_call(
        "debug_grant_item",
        &[
            &CHARACTER_GUID.to_string(),
            &KEY_ITEM_ENTRY.to_string(),
            "1",
        ],
    );
    assert_eq!(key_count(&standalone), 1);
    standalone.assert_call(
        "debug_pick_lock_entry",
        &[
            &CHARACTER_GUID.to_string(),
            &LOCKED_GOOBER_ENTRY.to_string(),
        ],
    );
    assert_eq!(unlocked_count(&standalone), 1);
    assert_eq!(key_count(&standalone), 1, "OPEN_LOCK does not consume keys");

    standalone.assert_call(
        "debug_use_gameobject_entry",
        &[
            &CHARACTER_GUID.to_string(),
            &LOCKED_GOOBER_ENTRY.to_string(),
        ],
    );
    assert_eq!(quest_count(&standalone), before_quest + 1);
    assert_eq!(xp(&standalone), before_xp + 7);
    assert_eq!(goober_state(&standalone, LOCKED_GOOBER_ENTRY), before_state);

    spawn_goober(&standalone, LOCKLESS_GOOBER_ENTRY);
    standalone.assert_sql(&format!(
        "UPDATE game_quest_objective SET target_entry = {LOCKLESS_GOOBER_ENTRY} WHERE id = {QUEST_OBJECTIVE_ID}"
    ));
    standalone.assert_call(
        "debug_use_gameobject_entry",
        &[
            &CHARACTER_GUID.to_string(),
            &LOCKLESS_GOOBER_ENTRY.to_string(),
        ],
    );
    assert_eq!(quest_count(&standalone), before_quest + 2);
    assert_eq!(xp(&standalone), before_xp + 14);
    assert_eq!(goober_state(&standalone, LOCKLESS_GOOBER_ENTRY), 0);
}
