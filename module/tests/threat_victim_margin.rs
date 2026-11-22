mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use lyracore_shared::spell::taxonomy::{
    A_CONTROL, CAST_FLAG_IGNORE_RESISTANCES, M_FEAR, P_MECHANIC, T_TARGET_ENEMY,
};
use support::{number, Standalone};

const VICTIM: &str = "1";
const CHALLENGER: &str = "5096010";
const SETTLE_TICKS: &str = "100";
const FAR_FUTURE_MS: u64 = 1_900_000_000_000;
const DEFENSIVE: u8 = 1;
const HUMAN: &str = "1";
const WARRIOR: &str = "1";
const FEAR: u32 = 5_113_301;
const CHARACTER_FACTION: u32 = 9_101;
const WOLF_FACTION: u32 = 9_102;

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

    fn give_wolf_combat_reach(&self, combat_reach: f32) {
        let wolf = &self.shard.query_rows(&format!(
            "SELECT display_id, scale_x FROM game_world_entity WHERE guid = {}",
            self.wolf
        ))[0];
        let display_id: u32 = number(wolf, "display_id");
        let scale: f32 = number(wolf, "scale_x");
        self.shard.insert_row(
            "game_display_combat_reach",
            &[
                ("display_id", &display_id),
                ("combat_reach", &combat_reach),
                ("display_scale", &scale),
            ],
        );
    }

    fn befriend_wolf_to_characters(&self) {
        for (id, group, friend_group) in [(CHARACTER_FACTION, 1u32, 0u32), (WOLF_FACTION, 0, 1)] {
            self.shard.insert_row(
                "game_faction_template",
                &[
                    ("id", &id),
                    ("faction", &id),
                    ("faction_group", &group),
                    ("friend_group", &friend_group),
                ],
            );
        }
        self.shard.assert_sql(&format!(
            "UPDATE game_world_entity SET faction_template = {CHARACTER_FACTION} \
             WHERE guid = {VICTIM}"
        ));
        self.shard.assert_sql(&format!(
            "UPDATE game_world_entity SET faction_template = {WOLF_FACTION} WHERE guid = {}",
            self.wolf
        ));
    }

    fn entity(&self, guid: &str) -> support::Row {
        self.shard
            .query_rows(&format!(
                "SELECT unit_flags, health, max_health FROM game_world_entity WHERE guid = {guid}"
            ))
            .remove(0)
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
fn a_large_creature_counts_a_challenger_in_its_own_melee_reach() {
    let fight = Fight::start("threat-margin-large-creature", 6.0);
    assert_eq!(
        fight.challenge(1110),
        VICTIM,
        "6 yd is outside a normal wolf's 5 yd reach"
    );
    fight.give_wolf_combat_reach(4.0);
    fight.settle();
    assert_eq!(
        fight.victim(),
        CHALLENGER,
        "4 + 1.5 + 4/3 yd reaches the challenger 6 yd away"
    );
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_turns_from_its_feared_victim_to_a_free_source() {
    let fight = Fight::start("threat-priority-fear", 3.0);
    fight.set_threat(CHALLENGER, 100);
    fight.settle();
    assert_eq!(fight.victim(), VICTIM, "10% of the victim's threat");

    fight.shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &FEAR),
            ("name", &"Fear fixture"),
            ("school_mask", &32u8),
            ("range_yd", &30),
            ("is_negative", &true),
            ("duration_ms", &600_000),
            ("spell_level", &1),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
    );
    fight.shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(FEAR, 0)),
            ("spell_id", &FEAR),
            ("kind", &A_CONTROL),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &M_FEAR),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
    fight
        .shard
        .assert_call("debug_cast_at", &[&fight.wolf, &FEAR.to_string(), VICTIM]);
    fight.settle();
    assert_eq!(
        fight.victim(),
        CHALLENGER,
        "a feared victim is low priority"
    );

    fight
        .shard
        .assert_call("debug_shed_spell_auras", &[VICTIM, &FEAR.to_string()]);
    fight.settle();
    assert_eq!(
        fight.victim(),
        VICTIM,
        "the fear ends and the margin returns"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_whose_only_source_turns_friendly_evades_with_full_health() {
    let fight = Fight::start("threat-evade-friendly", 3.0);
    fight.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 50000 WHERE guid = {}",
        fight.wolf
    ));
    fight.befriend_wolf_to_characters();
    fight.settle();

    let wolf = fight.entity(&fight.wolf);
    assert!(
        fight
            .shard
            .query_rows(&format!(
                "SELECT id FROM game_threat WHERE creature_guid = {}",
                fight.wolf
            ))
            .is_empty(),
        "the wolf cleared its threat"
    );
    assert!(
        fight
            .shard
            .query_rows(&format!(
                "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {}",
                fight.wolf
            ))
            .is_empty(),
        "the wolf stopped attacking"
    );
    assert_eq!(number::<u32>(&wolf, "unit_flags") & IN_COMBAT, 0);
    assert_eq!(wolf["health"], wolf["max_health"]);
}
