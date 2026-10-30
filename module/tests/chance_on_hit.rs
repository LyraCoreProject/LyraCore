mod support;

use lyracore_shared::spell::taxonomy::{
    E_DAMAGE, E_WEAPON_STRIKE, P_NONE, P_RAW, SPELL_ATTR_WEAPON_PROCS, T_TARGET_ENEMY,
};

use std::collections::BTreeMap;

use support::{event_rows, poll_until, Standalone, TableSubscription, POLL_TIMEOUT};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const TEST_WOLF_ENTRY: u32 = 51000;
const WOLF_HEALTH: u32 = 100_000;
const FIXTURE_BLADE: u32 = 5_090_050;
const SINISTER_STRIKE: u32 = 1752;
const THUNDERFURY_PROC: u32 = 21992;
const CHANCE_ON_HIT: u8 = 2;
// 30 procs per minute on a 2 second weapon is a certain proc.
const CERTAIN_PPM: f32 = 30.0;
const WEAPON_DELAY_MS: u32 = 2000;
// At this level the capped miss, dodge and parry chances sum past certainty for a level 1 player.
const OUT_OF_REACH_LEVEL: u32 = 60;
const AVOIDED_SWINGS: usize = 3;
const CAST_EVENTS: &str = "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1";
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1";

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_avoided_weapon_ability_still_casts_the_main_hand_chance_on_hit_spell() {
    let mut standalone = Standalone::start("chance-on-hit-strike");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        SINISTER_STRIKE,
        "Sinister Strike",
        SPELL_ATTR_WEAPON_PROCS,
        E_WEAPON_STRIKE,
        (100, P_RAW),
    );
    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    let events = standalone.subscribe(&[CAST_EVENTS]);

    standalone.assert_call(
        "debug_cast_at",
        &["1", &SINISTER_STRIKE.to_string(), &wolf.to_string()],
    );

    assert!(
        poll_until(POLL_TIMEOUT, || go_of(&events, THUNDERFURY_PROC).is_some()),
        "a certain Chance on Hit must cast its spell even when the strike is avoided"
    );
    assert_ne!(
        go_of(&events, SINISTER_STRIKE).expect("the strike reports its GO")["miss_info"],
        "0",
        "a target sixty levels up must avoid the strike"
    );
    assert_eq!(
        go_of(&events, THUNDERFURY_PROC).unwrap()["target_guid"],
        wolf.to_string(),
        "the Chance on Hit spell strikes the unit the weapon hit"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn only_a_white_swing_that_connects_casts_the_chance_on_hit_spell() {
    let mut standalone = Standalone::start("chance-on-hit-swing");
    let wolf = stage(&mut standalone);
    set_level(&standalone, wolf, OUT_OF_REACH_LEVEL);
    let events = standalone.subscribe(&[CAST_EVENTS, SWING_EVENTS]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    assert!(
        poll_until(POLL_TIMEOUT, || swings(&events) >= AVOIDED_SWINGS),
        "the engagement never swung {AVOIDED_SWINGS} times"
    );
    assert_eq!(
        go_of(&events, THUNDERFURY_PROC),
        None,
        "an avoided white swing rolls no Chance on Hit"
    );

    set_level(&standalone, wolf, 1);
    assert!(
        poll_until(POLL_TIMEOUT, || go_of(&events, THUNDERFURY_PROC).is_some()),
        "a certain Chance on Hit must cast its spell when a white swing connects"
    );
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    stage_spell(
        standalone,
        THUNDERFURY_PROC,
        "Thunderfury",
        0,
        E_DAMAGE,
        (0, P_NONE),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = {WEAPON_DELAY_MS}, \
         spellid_1 = {THUNDERFURY_PROC}, spelltrigger_1 = {CHANCE_ON_HIT}, \
         spellppmrate_1 = {CERTAIN_PPM:.1} WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn set_level(standalone: &Standalone, guid: u64, level: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = {level} WHERE guid = {guid}"
    ));
}

fn stage_spell(
    standalone: &Standalone,
    spell_id: u32,
    name: &str,
    cast_flags: u32,
    kind: u8,
    (p0, p0_kind): (i32, u8),
) {
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({spell_id}, '{name}', 0, 0, 0, 0, 0, 5, 0, 1, 0, 0, 0, 0, 0, \
         0, 0, true, {cast_flags}, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({}, {spell_id}, 0, {kind}, 5, 0, 0.0, 0, {T_TARGET_ENEMY}, \
         0.0, 0, 0, 0, {p0}, {p0_kind}, 0, 0, false, 0.0)",
        (spell_id as u64) << 2
    ));
}

fn go_of(events: &TableSubscription, spell_id: u32) -> Option<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .find(|row| row["spell_id"] == spell_id.to_string() && row["kind"] == "2")
}

fn swings(events: &TableSubscription) -> usize {
    event_rows(events, "game_combat_event_live").len()
}
