//! A Package's personality as a Runtime Script: edit, reconcile, and the bots decide differently.
//!
//! This is the rung nothing below it reaches. The answer rule is pure and tested where it lives
//! (`packages/playerbots/src/goals.rs::threshold_from`), and the shipped Lua is run against the
//! real Host by that Package's own tests. What only a live Shard can show is the whole loop: a
//! Script Artifact applied by a reducer, read back by the Event Binding dispatch, answered inside
//! the Runtime Script Host, and reaching a bot's decision on the next brain tick — with no
//! republish anywhere in it.
//!
//! Every case here is the same bot at full health with nothing to fight. On its personality row it
//! has no reason to break off; a script that answers "flee at 100%" gives it one. So the goal row
//! reading FLEE is the script deciding, and the goal row leaving FLEE is the fallback deciding.

mod support;

use std::path::PathBuf;

use support::{poll_until, Standalone, POLL_TIMEOUT};

/// The Package's own identity, which is also the prefix of every event it fires.
const PACKAGE: &str = "playerbots";

/// A hand-written artifact carries a placeholder digest; nothing recomputes it.
const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Where the bot is spawned: empty ground on the open-world map, so its fallback decision is
/// WANDER and nothing else, and nothing can hurt it into fleeing for a reason this test did not
/// give it.
///
/// Deliberately NOT the Package Config home point in Elwynn. That one is negative, and `spacetime
/// call` reads a leading `-` as a flag before the reducer's arguments are parsed at all. Nothing
/// about this test needs the seeded neighbourhood; it needs a bot with nothing happening to it.
const SPAWN_AT: (&str, &str, &str) = ("1200.0", "1200.0", "50.0");

/// `pkg_playerbots_goal.kind` for an ungrouped bot milling about near home — what this bot decides
/// whenever no script tells it to run.
const GOAL_WANDER: u32 = 3;

/// `playerbots_spawn_role`'s damage role.
const ROLE_DPS: &str = "2";

/// Where this Shard mints Character guids from. A bare standalone has no Gateway to hand it a
/// range, and minting a bot Character is the first thing here that needs one — `create_character`
/// refuses with `NO_GUID_RANGE` until it exists.
const GUID_BASE: &str = "1000000";

/// `pkg_playerbots_goal.kind` for a bot that has broken off and is running home.
const GOAL_FLEE: u32 = 2;

/// The flee share a bot of the damage role is spawned with. Never 100, which is what makes the
/// script's answer visible.
const ROW_FLEE_AT_PCT: u32 = 15;

/// One reducer argument. `spacetime call` parses every argument as JSON, so a `String` parameter
/// has to arrive as a JSON string literal.
fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

/// A one-script artifact for `playerbots`, built through `serde_json` so the Lua's own quotes and
/// newlines are escaped by something that knows the rules.
fn flee_artifact(enabled: bool, lua: &str) -> String {
    serde_json::json!({
        "kind": "script",
        "version": 1,
        "package": PACKAGE,
        "source_hash": HASH,
        "scripts": [{
            "script_id": 100_100,
            "name": "playerbots.test-flee",
            "event": "playerbots.flee_at",
            "priority": 0,
            "enabled": enabled,
            "source": lua,
        }],
    })
    .to_string()
}

/// The artifact the Package actually ships, read off disk rather than restated here — a copy would
/// pass while the shipped file was broken, which is the one thing this step exists to catch.
///
/// Minified on the way through, because artifacts travel to `apply_package_deltas` one per LINE
/// and a Package ships a file a human edits. That is the same `jq -c` the Package README's
/// reconcile command runs, so this reads the file the way an Operator applies it.
fn shipped_artifact() -> String {
    let path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the module crate sits in the workspace")
        .join("packages/playerbots/data/.generated/personality.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the playerbots Package must be installed at {path:?}: {e}"));
    let json: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("the shipped artifact must be JSON: {e}"));
    json.to_string()
}

fn reconcile(standalone: &Standalone, artifacts: &[String]) {
    standalone.assert_call(
        "apply_package_deltas",
        &[&arg("script"), &arg(&artifacts.join("\n"))],
    );
}

fn bot_guid(standalone: &Standalone) -> u64 {
    standalone
        .query_rows("SELECT * FROM pkg_playerbots_bot")
        .first()
        .expect("one bot was spawned")["character_guid"]
        .parse()
        .expect("a character guid")
}

/// This bot's current goal kind, or `None` before it has decided anything.
fn goal_kind(standalone: &Standalone, bot: u64) -> Option<u32> {
    standalone
        .query_rows(&format!(
            "SELECT * FROM pkg_playerbots_goal WHERE character_guid = {bot}"
        ))
        .first()
        .map(|row| row["kind"].parse().expect("a goal kind"))
}

/// Poll the goal row until `wanted` says yes. A bot decides once a second, and reconciliation lands
/// between two of its decisions, so every assertion here is a settling one. Returns the last goal
/// seen, so the caller reports what the bot actually decided.
fn wait_for_goal(
    standalone: &Standalone,
    bot: u64,
    wanted: impl Fn(Option<u32>) -> bool,
) -> Option<u32> {
    let mut seen = None;
    poll_until(POLL_TIMEOUT, || {
        seen = goal_kind(standalone, bot);
        wanted(seen)
    });
    seen
}

fn assert_flees(standalone: &Standalone, bot: u64, why: &str) {
    let goal = wait_for_goal(standalone, bot, |kind| kind == Some(GOAL_FLEE));
    assert_eq!(
        goal,
        Some(GOAL_FLEE),
        "{why}: the bot is at full health with nothing to fight, so only a Script Answer of 100 \
         can put it on FLEE. Goal was {goal:?}"
    );
}

fn assert_does_not_flee(standalone: &Standalone, bot: u64, why: &str) {
    let goal = wait_for_goal(standalone, bot, |kind| kind == Some(GOAL_WANDER));
    assert_eq!(
        goal,
        Some(GOAL_WANDER),
        "{why}: the bot must fall back to its personality row ({ROW_FLEE_AT_PCT}%) and carry on \
         deciding — a bot on empty ground with nothing to fight wanders. Anything but WANDER here \
         is a bot stuck on the last thing a script told it. Goal was {goal:?}"
    );
}

/// Runs only when requested: it builds and publishes the Wasm module, and it needs the Package
/// whose events the scripts bind.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI, the Wasm toolchain, and the playerbots Package installed (lyracore packages add playerbots)"]
fn a_personality_script_decides_for_a_bot_and_a_broken_one_leaves_it_on_its_row() {
    let mut standalone = Standalone::start("playerbots-personality");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &[GUID_BASE]);
    standalone.assert_call(
        "playerbots_spawn_role",
        &["1", SPAWN_AT.0, SPAWN_AT.1, SPAWN_AT.2, ROLE_DPS],
    );
    let bot = bot_guid(&standalone);

    // --- The row alone: a bot at full health has no reason to break off.
    assert_does_not_flee(&standalone, bot, "before any script exists");

    // --- A script answers, and the bot decides on the answer. No republish happened.
    reconcile(&standalone, &[flee_artifact(true, "return 100")]);
    assert_flees(&standalone, bot, "a script answering 100");

    // --- Disabled through reconciliation alone. The row is still on the Shard.
    reconcile(&standalone, &[flee_artifact(false, "return 100")]);
    assert_does_not_flee(&standalone, bot, "the same script switched off");
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_script").len(),
        1,
        "a disabled script is applied and skipped, not withheld"
    );

    // --- A script that raises. The Host is the failure boundary, so the bot keeps its row.
    reconcile(&standalone, &[flee_artifact(true, "error(\"no\")")]);
    assert_does_not_flee(&standalone, bot, "a script that raises");

    // --- A script that will not stop. The Fuel Budget cuts it off; the bot is not cut off with it.
    reconcile(&standalone, &[flee_artifact(true, "while true do end")]);
    assert_does_not_flee(&standalone, bot, "a script that runs out of Fuel");

    // --- A script that answers something that is not a share. Refused, not clamped.
    reconcile(&standalone, &[flee_artifact(true, "return 5000")]);
    assert_does_not_flee(&standalone, bot, "an answer outside 0..=100");

    // --- And back again, from the same reconcile path. The override is not one-way.
    reconcile(&standalone, &[flee_artifact(true, "return 100")]);
    assert_flees(&standalone, bot, "the script switched back on");
}

/// The artifact the Package ships has to reconcile as written — it is hand-authored, so nothing
/// upstream of a Shard would have caught a bad identifier, event or Package name.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI, the Wasm toolchain, and the playerbots Package installed (lyracore packages add playerbots)"]
fn the_packages_own_personality_artifact_reconciles_onto_a_shard() {
    let mut standalone = Standalone::start("playerbots-personality-shipped");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

    reconcile(&standalone, &[shipped_artifact()]);

    // Named columns, never `SELECT *`: `game_script.source` holds the script's whole Lua, newlines
    // and all, and the shared row parser reads one row per line.
    let mut rows: Vec<(u32, String, String)> = standalone
        .query_rows("SELECT script_id, event, package FROM game_script")
        .into_iter()
        .map(|row| {
            (
                row["script_id"].parse().expect("a script id"),
                row["event"].clone(),
                row["package"].clone(),
            )
        })
        .collect();
    rows.sort_by_key(|row| row.0);
    assert_eq!(
        rows,
        vec![
            (
                100_100,
                "playerbots.flee_at".to_string(),
                PACKAGE.to_string()
            ),
            (
                100_101,
                "playerbots.heal_at".to_string(),
                PACKAGE.to_string()
            ),
        ],
        "both personality scripts land, bound to the Package Events the Package asks"
    );
}
