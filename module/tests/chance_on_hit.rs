mod support;

use lyracore_shared::item_property::COMBAT_SPELL;
use lyracore_shared::spell::taxonomy::{
    E_ADD_EXTRA_ATTACKS, E_DAMAGE, E_HEAL, E_WEAPON_STRIKE, P_NONE, P_RAW, SPELL_ATTR_WEAPON_PROCS,
    T_SELF, T_TARGET_ENEMY,
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
const HOLY_STRENGTH: u32 = 20007;
const CRUSADER: u32 = 1900;
const FURY_OF_FORGEWRIGHT: u32 = 15494;
const IRONFOE_EXTRA_ATTACKS: usize = 2;
const SLOW_SWING_MS: u32 = 60_000;
// 30 procs per minute on a 2 second weapon is a certain proc.
const CERTAIN_PPM: f32 = 30.0;
const WEAPON_DELAY_MS: u32 = 2000;
// At this level the capped miss, dodge and parry chances sum past certainty for a level 1 player.
const OUT_OF_REACH_LEVEL: u32 = 60;
const AVOIDED_SWINGS: usize = 3;
const ATTEMPTS: usize = 15;
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);
const CAST_EVENTS: &str = "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1";
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_avoided_weapon_ability_still_casts_the_main_hand_chance_on_hit_spell() {
    let mut standalone = Standalone::start("chance-on-hit-strike");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        SINISTER_STRIKE,
        "Sinister Strike",
        SPELL_ATTR_WEAPON_PROCS,
        (E_WEAPON_STRIKE, T_TARGET_ENEMY),
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
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_helpful_enchantment_spell_casts_on_the_wielder() {
    let mut standalone = Standalone::start("chance-on-hit-enchant");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        HOLY_STRENGTH,
        "Holy Strength",
        0,
        (E_HEAL, T_SELF),
        (0, P_NONE),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET spelltrigger_1 = 0 WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_item_enchantment (id, enchant_id, effect_index, kind, amount, spell_id, \
         school_mask) VALUES ({CRUSADER}, {CRUSADER}, 0, {COMBAT_SPELL}, 0, {HOLY_STRENGTH}, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_enchant_proc_rate (spell_id, ppm_rate) VALUES ({HOLY_STRENGTH}, {CERTAIN_PPM:.1})"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_item_instance SET enchant_id = {CRUSADER} \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_BLADE}"
    ));
    let events = standalone.subscribe(&[CAST_EVENTS]);

    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    assert!(
        poll_until(POLL_TIMEOUT, || go_of(&events, HOLY_STRENGTH).is_some()),
        "a certain enchantment Chance on Hit must cast its spell when a white swing connects"
    );
    assert_eq!(
        go_of(&events, HOLY_STRENGTH).unwrap()["target_guid"],
        PLAYER.to_string(),
        "a helpful enchantment spell lands on the wielder"
    );
    assert_eq!(
        go_of(&events, THUNDERFURY_PROC),
        None,
        "the cleared item spell slot rolls nothing"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_extra_attack_chance_on_hit_swings_twice_and_grants_no_more() {
    let mut standalone = Standalone::start("chance-on-hit-extra-attacks");
    let wolf = stage(&mut standalone);
    stage_spell(
        &standalone,
        FURY_OF_FORGEWRIGHT,
        "Fury of Forgewright",
        0,
        (E_ADD_EXTRA_ATTACKS, T_SELF),
        (0, P_NONE),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET base_points = {IRONFOE_EXTRA_ATTACKS} \
         WHERE spell_id = {FURY_OF_FORGEWRIGHT}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET spellid_1 = {FURY_OF_FORGEWRIGHT} WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET base_attack_time_ms = {SLOW_SWING_MS} WHERE guid = {PLAYER}"
    ));
    let events = standalone.subscribe(&[CAST_EVENTS, SWING_EVENTS]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    // The 60-second swing timer swings once on engagement; every later normal swing is forced.
    let mut normal_swings = 1;
    assert!(
        poll_until(POLL_TIMEOUT, || swings(&events) >= normal_swings),
        "the engagement never swung"
    );
    while go_of(&events, FURY_OF_FORGEWRIGHT).is_none() {
        assert!(
            normal_swings < ATTEMPTS,
            "no swing connected in {ATTEMPTS} attempts"
        );
        normal_swings += 1;
        standalone
            .assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
        assert!(
            poll_until(POLL_TIMEOUT, || swings(&events) >= normal_swings),
            "the forced swing never fired"
        );
    }

    let after_extra = normal_swings + IRONFOE_EXTRA_ATTACKS;
    assert!(
        poll_until(POLL_TIMEOUT, || swings(&events) >= after_extra),
        "the granted extra attacks never swung"
    );
    std::thread::sleep(SETTLE);
    assert_eq!(
        swings(&events),
        after_extra,
        "the two Extra Attacks swing once each and grant no more"
    );
    assert_eq!(
        gos_of(&events, FURY_OF_FORGEWRIGHT),
        1,
        "an Extra Attack must not roll the extra-attack Chance on Hit again"
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
        (E_DAMAGE, T_TARGET_ENEMY),
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
    (kind, target): (u8, u8),
    (p0, p0_kind): (i32, u8),
) {
    let is_negative = target != T_SELF;
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({spell_id}, '{name}', 0, 0, 0, 0, 0, 5, 0, 1, 0, 0, 0, 0, 0, \
         0, 0, {is_negative}, {cast_flags}, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({}, {spell_id}, 0, {kind}, 5, 0, 0.0, 0, {target}, \
         0.0, 0, 0, 0, {p0}, {p0_kind}, 0, 0, false, 0.0)",
        (spell_id as u64) << 2
    ));
}

fn gos_of(events: &TableSubscription, spell_id: u32) -> usize {
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .filter(|row| row["spell_id"] == spell_id.to_string() && row["kind"] == "2")
        .count()
}

fn go_of(events: &TableSubscription, spell_id: u32) -> Option<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .find(|row| row["spell_id"] == spell_id.to_string() && row["kind"] == "2")
}

fn swings(events: &TableSubscription) -> usize {
    event_rows(events, "game_combat_event_live").len()
}
