mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_IMMUNITY, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, M_FEAR, P_MECHANIC,
    P_SCHOOL_MASK, T_SELF, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, number, subscribe_with_fence, Standalone};

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
const PHYSICAL_IMMUNITY: u32 = 5_113_302;
const PHYSICAL: i32 = 1;
// SPELL_AURA_SCHOOL_IMMUNITY in the low byte of an immunity Aura's p1.
const SOURCE_SCHOOL_IMMUNITY: i32 = 39;
const EVADE_SPELL: u32 = 5_113_303;
const EVADING_CAST: u32 = 5_113_304;
const EVADING_CAST_TIME_MS: u32 = 10_000;
// AttackOutcome::wire in game_combat_event_live.hit_info, and vmangos SpellMissInfo in
// game_spell_cast_event_live.miss_info.
const HIT_INFO_EVADE: &str = "9";
const MISS_EVADE: &str = "6";
// vmangos SpellSchoolMask FROST, and the spell's range in yards.
const FROST: u32 = 0x10;
const EVADE_SPELL_RANGE_YD: u32 = 30;

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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_that_evades_stops_the_cast_it_began() {
    let fight = Fight::start("threat-evade-cast", 3.0);
    let wolf = &fight.wolf;
    fight.shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &EVADING_CAST),
            ("name", &"Evading cast fixture"),
            ("school_mask", &FROST),
            ("range_yd", &EVADE_SPELL_RANGE_YD),
            ("is_negative", &true),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
            ("cast_time_ms", &EVADING_CAST_TIME_MS),
        ],
    );
    fight.shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(EVADING_CAST, 0)),
            ("spell_id", &EVADING_CAST),
            ("kind", &E_DAMAGE),
            ("base_points", &50),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    fight.shard.assert_call(
        "debug_begin_cast",
        &[wolf, &EVADING_CAST.to_string(), VICTIM],
    );
    let casting = format!("SELECT spell_id FROM game_pending_cast WHERE caster_guid = {wolf}");
    assert!(
        !fight.shard.query_rows(&casting).is_empty(),
        "the wolf is mid-cast"
    );

    fight.befriend_wolf_to_characters();
    fight.settle();

    assert!(
        fight
            .shard
            .query_rows(&format!(
                "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {wolf}"
            ))
            .is_empty(),
        "the wolf evaded"
    );
    assert!(
        fight.shard.query_rows(&casting).is_empty(),
        "the evade interrupted the cast"
    );
}

impl Fight {
    fn row(&self, query: &str) -> Vec<support::Row> {
        self.shard.query_rows(query)
    }

    fn make_wolf_hostile_again(&self) {
        for (id, group, friend_group, enemy_group) in [
            (CHARACTER_FACTION, 1u32, 1u32, 2u32),
            (WOLF_FACTION, 2, 2, 1),
        ] {
            self.shard.assert_sql(&format!(
                "UPDATE game_faction_template SET faction_group = {group}, \
                 friend_group = {friend_group}, enemy_group = {enemy_group} WHERE id = {id}"
            ));
        }
    }

    fn returning(&self) -> bool {
        !self
            .row(&format!(
                "SELECT creature_guid FROM game_creature_ai_returning_home \
                 WHERE creature_guid = {}",
                self.wolf
            ))
            .is_empty()
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_returning_home_evades_damage_swings_and_spells() {
    let fight = Fight::start("threat-evade-returning", 3.0);
    let wolf = &fight.wolf;
    let spawn = fight.row(&format!(
        "SELECT x FROM game_creature_spawn WHERE guid = {wolf}"
    ));
    let home_x = number::<f32>(&spawn[0], "x") - 2000.0;
    fight.shard.assert_sql(&format!(
        "UPDATE game_creature_spawn SET x = {home_x} WHERE guid = {wolf}"
    ));
    fight.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 50000 WHERE guid = {wolf}"
    ));
    fight.befriend_wolf_to_characters();
    fight.settle();
    assert!(
        fight.returning(),
        "the wolf evaded and walks to its far home"
    );
    let max_health = fight.entity(wolf)["max_health"].clone();
    assert_eq!(
        fight.entity(wolf)["health"],
        max_health,
        "the evade healed it"
    );

    fight.make_wolf_hostile_again();
    fight.shard.assert_sql(&format!(
        "DELETE FROM game_creature_quest_tap WHERE creature_guid = {wolf}"
    ));
    let tapped =
        format!("SELECT creature_guid FROM game_creature_quest_tap WHERE creature_guid = {wolf}");
    assert!(fight.row(&tapped).is_empty(), "no Loot Tag before the hit");

    fight.shard.assert_call(
        "debug_spawn_at_feet",
        &[VICTIM, &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    let control = fight
        .row(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .unwrap()
        .to_string();
    assert_ne!(&control, wolf, "the control wolf is a second creature");
    fight.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {WOLF_FACTION}, health = 100000, \
         max_health = 100000 WHERE guid = {control}"
    ));
    fight
        .shard
        .assert_call("debug_apply_damage", &[&control, "500", VICTIM]);
    assert!(
        number::<u32>(&fight.entity(&control), "health") < 100_000,
        "a hostile wolf that is not returning takes the damage"
    );

    fight
        .shard
        .assert_call("debug_apply_damage", &[wolf, "500", VICTIM]);
    let after = fight.entity(wolf);
    assert_eq!(
        after["health"], max_health,
        "the returning wolf took no damage"
    );
    assert_eq!(number::<u32>(&after, "unit_flags") & IN_COMBAT, 0);
    assert!(fight
        .row(&format!(
            "SELECT id FROM game_threat WHERE creature_guid = {wolf}"
        ))
        .is_empty());
    assert!(
        fight.row(&tapped).is_empty(),
        "no Loot Tag from an evaded hit"
    );
    assert!(fight.returning());

    let at = &fight.row(&format!(
        "SELECT x, y, z, map_id FROM game_world_entity WHERE guid = {wolf}"
    ))[0];
    let [x, y, z] = ["x", "y", "z"].map(|axis| number::<f32>(at, axis));
    fight.shard.assert_call(
        "debug_teleport",
        &[
            VICTIM,
            &at["map_id"],
            &(x + 1.0).to_string(),
            &y.to_string(),
            &z.to_string(),
            &std::f32::consts::PI.to_string(),
        ],
    );
    let swings = subscribe_with_fence(&fight.shard, &["SELECT * FROM game_combat_event_live"]);
    fight.shard.assert_call("debug_engage", &[VICTIM, wolf]);
    fight.shard.assert_call("debug_advance_roll_batch", &["10"]);
    fence_subscription(&fight.shard, &swings);
    let swings: Vec<_> = event_rows(&swings, "game_combat_event_live")
        .into_iter()
        .filter(|row| row["attacker_guid"] == VICTIM && row["target_guid"] == *wolf)
        .collect();
    assert!(
        !swings.is_empty(),
        "the swing at the returning wolf is reported"
    );
    assert!(
        swings
            .iter()
            .all(|row| row["hit_info"] == HIT_INFO_EVADE && row["damage"] == "0"),
        "every swing evades: {swings:?}"
    );

    fight.shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &EVADE_SPELL),
            ("name", &"Evade fixture"),
            ("school_mask", &FROST),
            ("range_yd", &EVADE_SPELL_RANGE_YD),
            ("is_negative", &true),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
    );
    fight.shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(EVADE_SPELL, 0)),
            ("spell_id", &EVADE_SPELL),
            ("kind", &E_DAMAGE),
            ("base_points", &50),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    let casts = subscribe_with_fence(
        &fight.shard,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {EVADE_SPELL} AND kind = 2"
        )],
    );
    fight
        .shard
        .assert_call("debug_cast_at", &[VICTIM, &EVADE_SPELL.to_string(), wolf]);
    fight
        .shard
        .assert_call("debug_advance_virtual_clock", &["50000"]);
    fence_subscription(&fight.shard, &casts);
    let go = event_rows(&casts, "game_spell_cast_event_live")
        .pop()
        .expect("the cast reports its GO");
    assert_eq!(go["miss_info"], MISS_EVADE);
    assert_eq!(fight.entity(wolf)["health"], max_health);
    assert!(fight.returning());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_turns_from_a_victim_immune_to_its_melee_to_a_free_source() {
    let fight = Fight::start("threat-priority-physical-immunity", 3.0);
    fight.set_threat(CHALLENGER, 100);
    fight.settle();
    assert_eq!(fight.victim(), VICTIM, "10% of the victim's threat");

    fight.shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &PHYSICAL_IMMUNITY),
            ("name", &"Physical immunity fixture"),
            ("school_mask", &2u8),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    fight.shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PHYSICAL_IMMUNITY, 0)),
            ("spell_id", &PHYSICAL_IMMUNITY),
            ("kind", &A_IMMUNITY),
            ("target", &T_SELF),
            ("p_0", &PHYSICAL),
            ("p_0_kind", &P_SCHOOL_MASK),
            ("p_1", &SOURCE_SCHOOL_IMMUNITY),
        ],
    );
    fight.shard.assert_call(
        "debug_force_cast_at",
        &[VICTIM, &PHYSICAL_IMMUNITY.to_string(), VICTIM],
    );
    fight.settle();
    assert_eq!(
        fight.victim(),
        CHALLENGER,
        "a victim immune to physical damage is low priority"
    );

    fight.shard.assert_call(
        "debug_shed_spell_auras",
        &[VICTIM, &PHYSICAL_IMMUNITY.to_string()],
    );
    fight.settle();
    assert_eq!(
        fight.victim(),
        VICTIM,
        "the immunity ends and the margin returns"
    );
}
