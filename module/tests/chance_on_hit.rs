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
const ATTEMPTS: u32 = 15;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_landed_weapon_ability_casts_the_main_hand_chance_on_hit_spell() {
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
    stage_spell(
        &standalone,
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

    let events = strike_until_hit(&standalone, wolf);

    assert!(
        poll_until(POLL_TIMEOUT, || go_of(&events, THUNDERFURY_PROC).is_some()),
        "a certain Chance on Hit must cast its spell when the strike lands"
    );
    assert_eq!(
        go_of(&events, THUNDERFURY_PROC).unwrap()["target_guid"],
        wolf.to_string(),
        "the Chance on Hit spell strikes the unit the weapon hit"
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
    wolf
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

// Casts until one strike lands; returns the events of the landing cast.
fn strike_until_hit(standalone: &Standalone, wolf: u64) -> TableSubscription {
    for _ in 0..ATTEMPTS {
        let events = standalone
            .subscribe(&["SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1"]);
        standalone.assert_call(
            "debug_cast_at",
            &["1", &SINISTER_STRIKE.to_string(), &wolf.to_string()],
        );
        assert!(
            poll_until(POLL_TIMEOUT, || go_of(&events, SINISTER_STRIKE).is_some()),
            "the strike never reported its GO"
        );
        if go_of(&events, SINISTER_STRIKE).unwrap()["miss_info"] == "0" {
            return events;
        }
    }
    panic!("no strike landed in {ATTEMPTS} attempts");
}

fn go_of(events: &TableSubscription, spell_id: u32) -> Option<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .find(|row| row["spell_id"] == spell_id.to_string() && row["kind"] == "2")
}
