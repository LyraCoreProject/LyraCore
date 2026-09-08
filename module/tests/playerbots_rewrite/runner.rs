use super::*;
use std::collections::BTreeMap;
use std::time::Duration;

fn select(node: &Standalone, bot: &str, mode: &str) {
    node.assert_call(
        "playerbots_select_controller",
        &[bot, &format!("{{\"{mode}\":[]}}")],
    );
}

fn runner(node: &Standalone, bot: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {bot}"
    ))
    .into_iter()
    .next()
    .expect("runner explanation missing")
}

fn position(node: &Standalone, bot: &str) -> f32 {
    node.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {bot}"
    ))[0]["x"]
        .parse()
        .unwrap()
}

fn outcomes(node: &Standalone) {
    record_observations(node);
    let path = support::log_dir().join(format!("{}-runner.json", node.shard_name()));
    let record = serde_json::json!({
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
        "runner": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "scheduler": node.query_rows("SELECT * FROM pkg_playerbots_scheduler"),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_returns_home_with_observed_arrival_and_one_objective() {
    let (node, bots) = fixture("playerbots-runner-home", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)
        ["foreground"]
        .contains("movement")));
    let beginning = runner(&node, bot);
    assert!(beginning["objective"].contains("travelling"));
    assert!(!beginning["last_outcome"].contains("arrived"));
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("completed")));
    let completed = runner(&node, bot);
    assert_eq!(
        completed["objective_sequence"],
        beginning["objective_sequence"]
    );
    assert!((position(&node, bot) - 1238.0).abs() < 0.1);
    assert!(completed["movement_progress"].contains("true"));
    assert!(!completed["progress_age_micros"].contains("none"));
    assert_eq!(completed["retry_count"], "0");
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_record_only_observes_without_gameplay_and_freeze_holds_position() {
    let (node, bots) = fixture("playerbots-runner-record", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "true"]);
    select(&node, bot, "recordOnly");
    node.assert_sql("DELETE FROM pkg_playerbots_action");
    let generation = runner(&node, bot)["generation"].clone();
    select(&node, bot, "recordOnly");
    assert_eq!(runner(&node, bot)["generation"], generation);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)
        ["last_outcome"]
        .contains("recorded")));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(position(&node, bot), 1200.0);
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_action")
        .is_empty());
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {bot}"
        ))
        .is_empty());
    let attacker = ((0xF130u64 << 48) | (5_090_101u64 << 24) | 1).to_string();
    node.assert_call("playerbots_fixture_runner_damage", &[bot, &attacker, "1"]);
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {bot}"
        ))
        .is_empty());
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty()));
    select(&node, bot, "frozen");
    let frozen = runner(&node, bot);
    assert!(frozen["foreground"].contains("none"));
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    select(&node, bot, "frozen");
    assert_eq!(runner(&node, bot)["generation"], frozen["generation"]);
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(position(&node, bot), 1200.0);
    assert!(runner(&node, bot)["last_outcome"].contains("frozen"));
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_retains_a_cast_across_real_pushback_and_resumes_home() {
    let (node, bots) = fixture("playerbots-runner-pushback", "1");
    let bot = &bots[0];
    node.assert_sql("DELETE FROM game_event_reaper_schedule");
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "true"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty()));
    let start =
        node.query_rows("SELECT scheduled_id, scheduled_at FROM game_pending_cast")[0].clone();
    let due = |row: &BTreeMap<String, String>| -> i64 {
        row["foreground"]
            .split("due_micros = ")
            .nth(1)
            .unwrap()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap()
    };
    let original_due = due(&runner(&node, bot));
    let objective_id = runner(&node, bot)["objective_sequence"].clone();
    node.assert_call("playerbots_fixture_runner_damage", &[bot, "0", "1"]);
    let delayed =
        node.query_rows("SELECT scheduled_id, scheduled_at FROM game_pending_cast")[0].clone();
    assert_eq!(delayed["scheduled_id"], start["scheduled_id"]);
    assert_ne!(delayed["scheduled_at"], start["scheduled_at"]);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"],
        start["scheduled_id"]
    );
    assert_eq!(runner(&node, bot)["objective_sequence"], objective_id);
    assert_eq!(due(&runner(&node, bot)), original_due + 500_000);
    assert_eq!(position(&node, bot), 1200.0);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)
        ["cast_progress"]
        .contains(&start["scheduled_id"])));
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("completed")));
    let events = node.query_rows(&format!(
        "SELECT kind FROM game_spell_cast_event WHERE caster_guid = {bot}"
    ));
    assert_eq!(events.iter().filter(|e| e["kind"] == "1").count(), 1);
    assert_eq!(events.iter().filter(|e| e["kind"] == "2").count(), 1);
    assert_eq!(runner(&node, bot)["objective_sequence"], objective_id);
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_batches_due_bots_in_stable_fair_order() {
    let (node, bots) = fixture("playerbots-runner-fair", "25");
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    for bot in &bots {
        select(&node, bot, "recordOnly");
    }
    node.assert_call("playerbots_fixture_runner_due", &[]);
    let mut ordered = node.query_rows("SELECT id, character_guid FROM pkg_playerbots_bot");
    ordered.sort_by_key(|r| r["id"].parse::<u64>().unwrap());
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let first = node.query_rows("SELECT * FROM pkg_playerbots_scheduler")[0].clone();
    assert_eq!(first["processed"], "16");
    assert_eq!(first["excess_due"], "true");
    assert!(first["oldest_deferred_lag_micros"].parse::<i64>().unwrap() >= 2_000_000);
    for bot in &ordered[..16] {
        assert!(first["processed_guids"].contains(&bot["character_guid"]));
    }
    for bot in &ordered[16..] {
        assert!(!first["processed_guids"].contains(&bot["character_guid"]));
    }
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let second = node.query_rows("SELECT * FROM pkg_playerbots_scheduler")[0].clone();
    assert_eq!(second["processed"], "9");
    for bot in &ordered[16..] {
        assert!(second["processed_guids"].contains(&bot["character_guid"]));
    }
    assert_eq!(second["excess_due"], "false");
    outcomes(&node);
}

#[test]
#[ignore = "requires pinned preceding Wasm, SpacetimeDB, and the playerbots Package"]
fn playerbots_runner_migrates_populated_preceding_wasm_and_backfills_boundedly() {
    let old_path = std::env::var_os("PLAYERBOTS_PRECEDING_WASM")
        .expect("PLAYERBOTS_PRECEDING_WASM must name the built PB-001 Wasm");
    let old_wasm = std::fs::read(old_path).unwrap();
    assert_ne!(
        blake3::hash(&old_wasm),
        blake3::hash(support::module_bytes())
    );
    let mut node = Standalone::start("playerbots-runner-migration");
    node.publish_module_bytes(&old_wasm);
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn_role", &["25", "1200", "1200", "50", "1"]);
    node.assert_call("playerbots_fixture_prepare", &[]);
    let bot = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]["character_guid"]
        .clone();
    node.assert_call("playerbots_fixture_blocked_quest", &[&bot]);
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows("SELECT * FROM pkg_playerbots_goal")
        .is_empty()));
    node.assert_call("playerbots_fixture_freeze", &[&bot]);
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    node.assert_call("playerbots_fixture_cast", &[&bot, &bot]);
    node.assert_call("playerbots_fixture_cancel", &[&bot, "false"]);
    let roster = node.query_rows("SELECT * FROM pkg_playerbots_bot");
    let goals = node.query_rows("SELECT * FROM pkg_playerbots_goal");
    let actions = node.query_rows("SELECT * FROM pkg_playerbots_action");
    let quests = node.query_rows("SELECT * FROM game_character_quest");
    assert_eq!(roster.len(), 25);
    assert!(!roster[0].contains_key("controller"));
    assert!(!goals.is_empty());
    assert!(!actions.is_empty());
    assert!(!quests.is_empty());
    node.publish_module();
    record_inputs(&node);
    let upgraded = node.query_rows("SELECT * FROM pkg_playerbots_bot");
    assert_eq!(upgraded.len(), roster.len());
    for old in &roster {
        let new = upgraded.iter().find(|r| r["id"] == old["id"]).unwrap();
        for (key, value) in old {
            assert_eq!(new[key], *value, "migrated roster column {key}");
        }
        assert!(new["controller"].contains("legacy"));
        assert_eq!(new["scheduler_lag_micros"], "0");
    }
    assert_eq!(node.query_rows("SELECT * FROM pkg_playerbots_goal"), goals);
    assert_eq!(
        node.query_rows("SELECT * FROM pkg_playerbots_action"),
        actions
    );
    assert_eq!(
        node.query_rows("SELECT * FROM game_character_quest"),
        quests
    );
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_runner")
        .is_empty());
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    assert_eq!(
        node.query_rows("SELECT * FROM pkg_playerbots_runner").len(),
        16
    );
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    assert_eq!(
        node.query_rows("SELECT * FROM pkg_playerbots_runner").len(),
        25
    );
    select(&node, &bot, "recordOnly");
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    let path = support::log_dir().join(format!("{}-migration.json", node.shard_name()));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "spacetimedb": "2.7.1",
            "preceding_core": "be3fa67d0f0c24749230560544a3e8e8b577f61d",
            "preceding_collection": "5724de1660a2628e88320914bdd5abb0c69da517",
            "preceding_wasm_identity": blake3::hash(&old_wasm).to_hex().to_string(),
            "new_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
            "preceding_roster": roster, "preceding_goals": goals, "preceding_actions": actions,
            "preceding_quests": quests, "upgraded_roster": upgraded,
        }))
        .unwrap(),
    )
    .unwrap();
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_survival_cancels_cast_before_movement_and_keeps_the_objective() {
    let (node, bots) = fixture("playerbots-runner-preempt", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "true"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty()));
    let cast_id =
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"].clone();
    let objective_id = runner(&node, bot)["objective_sequence"].clone();
    node.assert_call("playerbots_fixture_runner_survival", &[bot]);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["chosen"]
        .contains("survival")));
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    let after = runner(&node, bot);
    assert_eq!(after["objective_sequence"], objective_id);
    assert!(after["foreground"].contains("movement"));
    assert!(after["history"].contains("cancelled"));
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_action")
        .iter()
        .any(|r| r["cast_id"] == cast_id && r["outcome"].contains("cancelled")));
    select(&node, bot, "frozen");
    let stopped_at = position(&node, bot);
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(position(&node, bot), stopped_at);
    assert!(runner(&node, bot)["cast_progress"].contains("none"));
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_defense_preserves_home_and_accepted_attack_is_not_progress() {
    let (node, bots) = fixture("playerbots-runner-defense", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    node.assert_sql("DELETE FROM game_melee_schedule");
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["chosen"]
        .contains("returnHome")));
    let objective_id = runner(&node, bot)["objective_sequence"].clone();
    let target = ((0xF130u64 << 48) | (5_090_101u64 << 24) | 1).to_string();
    node.assert_call("playerbots_fixture_runner_damage", &[bot, &target, "1"]);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["chosen"]
        .contains("attack")));
    let defended = runner(&node, bot);
    assert_eq!(defended["objective_sequence"], objective_id);
    assert!(defended["last_outcome"].contains("accepted"));
    assert!(defended["combat_progress"].contains("none"));
    assert!(defended["movement_progress"].contains("none"));
    assert!(defended["objective"].contains("last_verified_progress_micros = (none"));
    assert!(defended["quest_progress"].contains("credit = 0"));
    node.assert_call("playerbots_fixture_credit_kill", &[bot]);
    node.assert_call("playerbots_fixture_runner_clear_navigation", &[bot]);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("completed")));
    let resumed = runner(&node, bot);
    assert_eq!(resumed["objective_sequence"], objective_id);
    assert!(resumed["quest_progress"].contains("credit = 1"));
    assert!(!resumed["combat_progress"].contains("none"));
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_defers_a_blocked_destination_with_bounded_failure_memory() {
    let (node, bots) = fixture("playerbots-runner-deferred", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select(&node, bot, "cohort");
    assert!(poll_until(Duration::from_secs(38), || runner(&node, bot)
        ["objective"]
        .contains("deferred")));
    let deferred = runner(&node, bot);
    assert_eq!(deferred["retry_count"], "3");
    assert!(deferred["failures"].contains("noMovement"));
    assert!(deferred["deferred_destinations"].contains("geometry_revision = (none"));
    assert!(deferred["foreground"].contains("none"));
    assert!(deferred["route_expansions"].parse::<u32>().unwrap() <= 4096);
    assert_eq!(position(&node, bot), 1200.0);
    let objective_id = deferred["objective_sequence"].clone();
    select(&node, bot, "frozen");
    select(&node, bot, "cohort");
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(runner(&node, bot)["objective_sequence"], objective_id);
    assert!(runner(&node, bot)["objective"].contains("deferred"));
    assert!(runner(&node, bot)["foreground"].contains("none"));
    node.assert_call("playerbots_fixture_runner_survival", &[bot]);
    node.assert_call("playerbots_fixture_runner_damage", &[bot, "0", "1"]);
    let prior_move = node.query_rows("SELECT observed_micros FROM pkg_playerbots_action");
    std::thread::sleep(Duration::from_secs(3));
    let waiting = runner(&node, bot);
    assert!(waiting["chosen"].contains("survival"));
    assert!(waiting["chosen"].contains("hold"));
    assert!(waiting["foreground"].contains("none"));
    assert_eq!(waiting["route_expansions"], "0");
    assert_eq!(
        node.query_rows("SELECT observed_micros FROM pkg_playerbots_action"),
        prior_move
    );
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_never_selects_an_unlearned_rotation_spell() {
    let (node, bots) = fixture("playerbots-runner-spellbook", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "true"]);
    node.assert_sql(&format!(
        "DELETE FROM game_player_spell WHERE character_guid = {bot} AND spell_id = 5090100"
    ));
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("completed")));
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_action")
        .iter()
        .all(|r| r["spell_id"] != "5090100"));
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_history_is_bounded_and_deleted_with_the_character() {
    let (node, bots) = fixture("playerbots-runner-rows", "1");
    let bot = &bots[0];
    for _ in 0..12 {
        select(&node, bot, "recordOnly");
        select(&node, bot, "frozen");
    }
    let row = runner(&node, bot);
    assert_eq!(row["generation"], "24");
    assert_eq!(row["history"].matches("at_micros").count(), 8);
    assert_eq!(
        node.query_rows("SELECT * FROM pkg_playerbots_runner").len(),
        1
    );
    node.assert_call("playerbots_despawn_all", &[]);
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_runner")
        .is_empty());
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_bot")
        .is_empty());
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_selection_owns_normal_invites_and_group_consent() {
    let (node, bots) = fixture("playerbots-runner-invite", "1");
    let bot = &bots[0];
    node.assert_call("debug_spawn_player_entity", &["1"]);
    let human = support::actor("1");
    let bot_actor = support::actor(bot);
    select(&node, bot, "cohort");
    node.assert_call("admit_sessionless_group_action", &[bot]);
    node.assert_call("gw_group_invite", &[&human, bot]);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT * FROM game_group_member WHERE character_guid = {bot}"
        ))
        .len(),
        1
    );
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_group_invite WHERE target_guid = {bot}"
        ))
        .is_empty());
    node.assert_call("gw_group_leave", &[&bot_actor]);
    for mode in ["recordOnly", "frozen"] {
        select(&node, bot, mode);
        let refusal = node.call("admit_sessionless_group_action", &[bot]);
        assert!(!refusal.status.success());
        assert!(String::from_utf8_lossy(&refusal.stderr).contains("group:action_suppressed"));
        node.assert_call("gw_group_invite", &[&human, bot]);
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM game_group_member WHERE character_guid = {bot}"
            ))
            .is_empty());
        assert_eq!(
            node.query_rows(&format!(
                "SELECT * FROM game_group_invite WHERE target_guid = {bot}"
            ))
            .len(),
            1
        );
        node.assert_sql(&format!(
            "DELETE FROM game_group_invite WHERE target_guid = {bot}"
        ));
        node.assert_call("debug_emit_sessionless_group_intent", &[bot, "1", "false"]);
        select(&node, bot, mode);
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM game_bot_invite_intent WHERE inviter_guid = {bot}"
            ))
            .is_empty());
    }
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_objective_deadline_expires_the_retained_cast_identity() {
    let (node, bots) = fixture("playerbots-runner-deadline", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "true"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || !node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty()));
    let cast_id =
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"].clone();
    node.assert_call("playerbots_fixture_runner_expire_objective", &[bot]);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("deferred")));
    assert!(node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty());
    assert!(node
        .query_rows("SELECT * FROM pkg_playerbots_action")
        .iter()
        .any(|r| r["cast_id"] == cast_id && r["outcome"].contains("expired")));
    select(&node, bot, "frozen");
    std::thread::sleep(Duration::from_secs(6));
    assert!(runner(&node, bot)["objective"].contains("deferred"));
    assert!(runner(&node, bot)["cast_progress"].contains("none"));
    assert_eq!(position(&node, bot), 1200.0);
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_replaces_a_changed_destination_with_a_new_candidate_identity() {
    let (node, bots) = fixture("playerbots-runner-destination", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)
        ["foreground"]
        .contains("movement")));
    assert_eq!(runner(&node, bot)["objective_sequence"], "1");
    node.assert_sql(&format!(
        "UPDATE pkg_playerbots_bot SET home_x = 1260 WHERE character_guid = {bot}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)
        ["objective_sequence"]
        == "2"));
    let replaced = runner(&node, bot);
    assert!(replaced["chosen"].contains("objective = 2"));
    assert!(replaced["history"].contains("cancelled"));
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("completed")));
    assert!((position(&node, bot) - 1258.0).abs() < 0.1);
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_relinquishes_current_account_ownership_without_cancelling_human_cast() {
    let (node, bots) = fixture("playerbots-runner-ownership", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "true"]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)
        ["foreground"]
        .contains("movement")));
    let account = node.query_rows(&format!(
        "SELECT account_id FROM pkg_playerbots_bot WHERE character_guid = {bot}"
    ))[0]["account_id"]
        .clone();
    node.assert_call("claim_account", &[&account, bot, "501"]);
    assert_eq!(
        node.query_rows(&format!(
            "SELECT online FROM game_character WHERE guid = {bot}"
        ))[0]["online"],
        "false"
    );
    let generation = node.query_rows(&format!(
        "SELECT generation FROM game_account_claim WHERE account_id = {account}"
    ))[0]["generation"]
        .clone();
    let token =
        format!(r#"{{"account_id":{account},"generation":{generation},"request_nonce":501}}"#);
    let actor = format!(r#"{{"guid":{bot},"ownership":{{"some":{token}}}}}"#);
    node.assert_call("playerbots_fixture_runner_due", &[]);
    node.assert_call("playerbots_fixture_runner_pass", &[]);
    assert!(runner(&node, bot)["chosen"].contains("restricted"));
    let held = position(&node, bot);
    node.assert_call("gw_cast_spell", &[&actor, "5090100", bot]);
    let human_cast =
        node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"].clone();
    for mode in ["cohort", "legacy", "frozen"] {
        select(&node, bot, mode);
        node.assert_call("playerbots_fixture_runner_due", &[]);
        node.assert_call("playerbots_fixture_runner_pass", &[]);
        assert_eq!(
            node.query_rows("SELECT scheduled_id FROM game_pending_cast")[0]["scheduled_id"],
            human_cast
        );
        assert!((position(&node, bot) - held).abs() < 0.01);
    }
    assert!(poll_until(POLL_TIMEOUT, || node
        .query_rows("SELECT * FROM game_pending_cast")
        .is_empty()));
    node.assert_call("release_account_claim", &[&token]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    select(&node, bot, "cohort");
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["objective"]
        .contains("completed")));
    assert!((position(&node, bot) - 1238.0).abs() < 0.1);
    outcomes(&node);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn playerbots_runner_observes_tactical_movement_without_advancing_the_home_clock() {
    let (node, bots) = fixture("playerbots-runner-tactical", "1");
    let bot = &bots[0];
    node.assert_call("playerbots_fixture_blocked_quest", &[bot]);
    node.assert_call("playerbots_fixture_runner_stage", &[bot, "false"]);
    node.assert_call("playerbots_fixture_runner_clear_navigation", &[bot]);
    let target = ((0xF130u64 << 48) | (5_090_101u64 << 24) | 1).to_string();
    node.assert_call("playerbots_fixture_position", &[&target, "1400"]);
    node.assert_sql(&format!(
        "UPDATE game_creature_spawn SET x = 1400 WHERE guid = {target}"
    ));
    node.assert_sql("DELETE FROM game_melee_schedule");
    select(&node, bot, "cohort");
    node.assert_call("playerbots_fixture_runner_damage", &[bot, &target, "1"]);
    assert!(poll_until(POLL_TIMEOUT, || runner(&node, bot)["chosen"]
        .contains("defense")));
    let start = position(&node, bot);
    std::thread::sleep(Duration::from_secs(12));
    let observed = runner(&node, bot);
    assert!(position(&node, bot) > start + 30.0);
    assert!(observed["chosen"].contains("defense"));
    assert!(!observed["failures"].contains("noMovement"));
    assert_eq!(observed["retry_count"], "0");
    assert!(!observed["movement_progress"].contains("none"));
    assert!(observed["objective"].contains("last_verified_progress_micros = (none"));
    outcomes(&node);
}
