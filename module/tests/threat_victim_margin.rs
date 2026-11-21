mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::{number, Standalone};

const VICTIM: &str = "1";
const CHALLENGER: &str = "5096010";
const SETTLE_TICKS: &str = "100";
const FAR_FUTURE_MS: u64 = 1_900_000_000_000;
const DEFENSIVE: u8 = 1;
const HUMAN: &str = "1";
const WARRIOR: &str = "1";

struct Fight {
    shard: Standalone,
    wolf: String,
}

impl Fight {
    fn start(name: &str, challenger_distance: f32) -> Self {
        let mut shard = Standalone::start(name);
        shard.publish_module();
        shard.assert_call("claim_operator", &[]);
        shard.assert_call("install_guid_range", &["0"]);
        shard.assert_call("debug_spawn_player_entity", &[VICTIM]);
        shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
        shard.assert_sql("UPDATE game_world_entity SET godmode = true WHERE guid = 1");
        shard.assert_call(
            "debug_spawn_at_feet",
            &[VICTIM, &TEST_WOLF_ENTRY.to_string(), "2"],
        );
        let wolf = shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
            ))
            .into_iter()
            .map(|row| number::<u64>(&row, "guid"))
            .max()
            .unwrap()
            .to_string();
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET simulation_indexed = true, health = 100000, \
             max_health = 100000 WHERE guid = {wolf}"
        ));
        shard.assert_sql(&format!(
            "DELETE FROM game_creature_ai_state WHERE creature_guid = {wolf}"
        ));
        shard.insert_row(
            "game_creature_ai_state",
            &[
                ("creature_guid", &wolf),
                ("engagement_id", &1),
                ("react_state", &DEFENSIVE),
            ],
        );
        let at = &shard.query_rows(&format!(
            "SELECT x, y, z, map_id FROM game_world_entity WHERE guid = {wolf}"
        ))[0];
        let [x, y, z] = ["x", "y", "z"].map(|axis| number::<f32>(at, axis));
        shard.assert_call(
            "debug_stage_meeting_stone_character",
            &[
                CHALLENGER,
                CHALLENGER,
                HUMAN,
                WARRIOR,
                "60",
                &at["map_id"],
                &(x + challenger_distance).to_string(),
                &y.to_string(),
                &z.to_string(),
            ],
        );
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET godmode = true WHERE guid = {CHALLENGER}"
        ));
        let fight = Self { shard, wolf };
        fight.set_threat(VICTIM, 1000);
        fight.settle();
        assert_eq!(
            fight.victim(),
            VICTIM,
            "the first threat source is the victim"
        );
        fight
    }

    fn set_threat(&self, source: &str, threat: i64) {
        let wolf = &self.wolf;
        let updated = self.shard.query_rows(&format!(
            "SELECT id FROM game_threat WHERE creature_guid = {wolf} AND source_guid = {source}"
        ));
        if updated.is_empty() {
            self.shard
                .assert_call("debug_add_threat", &[wolf, source, &threat.to_string()]);
        } else {
            self.shard.assert_sql(&format!(
                "UPDATE game_threat SET threat = {threat} \
                 WHERE creature_guid = {wolf} AND source_guid = {source}"
            ));
        }
    }

    fn settle(&self) {
        self.shard
            .assert_call("debug_advance_roll_batch", &[SETTLE_TICKS]);
    }

    fn victim(&self) -> String {
        let wolf = &self.wolf;
        self.shard.query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {wolf}"
        ))[0]["target_guid"]
            .clone()
    }

    fn challenge(&self, threat: i64) -> String {
        self.set_threat(CHALLENGER, threat);
        self.settle();
        self.victim()
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_challenger_in_melee_reach_needs_more_than_110_percent_of_the_victims_threat() {
    let fight = Fight::start("threat-margin-melee", 3.0);
    assert_eq!(fight.challenge(1090), VICTIM, "109% does not switch");
    assert_eq!(
        fight.challenge(1100),
        VICTIM,
        "exactly 110% does not switch"
    );
    assert_eq!(fight.challenge(1110), CHALLENGER, "111% switches");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_challenger_outside_melee_reach_needs_more_than_130_percent_of_the_victims_threat() {
    let fight = Fight::start("threat-margin-range", 15.0);
    assert_eq!(
        fight.challenge(1290),
        VICTIM,
        "129% at range does not switch"
    );
    assert_eq!(
        fight.challenge(1300),
        VICTIM,
        "exactly 130% does not switch"
    );
    assert_eq!(fight.challenge(1310), CHALLENGER, "131% at range switches");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_margin_follows_the_victims_own_threat() {
    let fight = Fight::start("threat-margin-victim", 3.0);
    fight.set_threat(VICTIM, 1100);
    assert_eq!(fight.challenge(1200), VICTIM, "1200 is 109% of 1100");
    fight.set_threat(VICTIM, 1000);
    fight.settle();
    assert_eq!(fight.victim(), CHALLENGER, "1200 is 120% of 1000");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_taunt_lock_holds_the_victim_against_any_threat_and_ends_with_the_lock() {
    let fight = Fight::start("threat-margin-taunt", 3.0);
    fight.shard.insert_row(
        "game_taunt_lock",
        &[
            ("creature_guid", &fight.wolf.parse::<u64>().unwrap()),
            ("forced_target_guid", &1u64),
            ("forced_until_ms", &FAR_FUTURE_MS),
        ],
    );
    assert_eq!(fight.challenge(5000), VICTIM, "the lock pins the victim");
    fight.shard.assert_sql(&format!(
        "DELETE FROM game_taunt_lock WHERE creature_guid = {}",
        fight.wolf
    ));
    fight.settle();
    assert_eq!(fight.victim(), CHALLENGER, "without the lock 500% switches");
}
