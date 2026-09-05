mod support;

use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use support::Standalone;

const CHARACTER: u64 = 1;
const WOLF: u64 = (0xF130_u64 << 48) | (51_000_u64 << 24) | 1;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_periodic_kill_commits_the_death_and_an_unrelated_heal() {
    let standalone = Standalone::start("aura-tick-death");
    standalone.publish_module();
    standalone.assert_sql("DELETE FROM game_creature_move_schedule");
    standalone.assert_sql("DELETE FROM game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1, max_health = 1 WHERE guid = {WOLF}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 50, max_health = 100 WHERE guid = {CHARACTER}"
    ));
    assert_eq!(health(&standalone, WOLF), 1);
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[&WOLF.to_string(), "1", "true", "1"],
    );
    standalone.assert_call("debug_fill_aura_slots", &["1", "1", "false", "1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET eff_kind = 144, amount = 7 WHERE target_guid = {WOLF}"
    ));
    standalone.assert_sql("UPDATE game_aura SET eff_kind = 145, amount = 7 WHERE target_guid = 1");
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        - 1_000_000;
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET period_ms = 600000, next_tick_micros = {due}"
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let wolf = standalone.query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {WOLF}"
        ));
        if wolf[0]["dead"] == "true" {
            assert_eq!(wolf[0]["health"], "0");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the periodic kill did not commit; wolf health={}, unrelated heal health={}",
            wolf[0]["health"],
            health(&standalone, CHARACTER),
        );
        thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(health(&standalone, CHARACTER), 57);
    assert!(standalone
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {WOLF}"
        ))
        .is_empty());
    let surviving =
        standalone.query_rows("SELECT next_tick_micros FROM game_aura WHERE target_guid = 1");
    assert_eq!(surviving.len(), 1);
    assert_eq!(
        surviving[0]["next_tick_micros"].parse::<i64>().unwrap(),
        due + 600_000_000,
    );
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
