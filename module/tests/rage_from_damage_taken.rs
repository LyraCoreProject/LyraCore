mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use lyracore_shared::packing::power_type::RAGE;
use lyracore_shared::spell::taxonomy::{A_PERIODIC_DAMAGE, E_DAMAGE, T_TARGET_ENEMY};

use support::{event_rows, number, poll_until, wait_for_event_rows, Row, Standalone, POLL_TIMEOUT};

const WARRIOR: u64 = 1;
const WOLF_ENTRY: u32 = 51_000;
const FIXTURE_BOLT: u32 = 509_0201;
/// `debug_fill_aura_slots` gives the first debuff slot spell id `9_000_000 + 32`.
const DOT: u32 = 9_000_032;
const HEALTH: u32 = 10_000;
/// `SpellCastEventKind::PeriodicLog`.
const PERIODIC_LOG: u8 = 6;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_warrior_gains_rage_from_spell_and_periodic_damage_taken() {
    let mut standalone = Standalone::start("rage-from-damage-taken");
    standalone.publish_module();
    standalone.assert_sql("DELETE FROM game_creature_move_schedule");
    standalone.assert_sql("DELETE FROM game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&WARRIOR.to_string()]);
    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&WARRIOR.to_string(), &WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {WOLF_ENTRY}"
        ))
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60 WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = {HEALTH}, \
         max_health = {HEALTH}, power = 0, max_power = 1000 WHERE guid = {WARRIOR}"
    ));
    assert_eq!(
        (number::<u32>(&warrior(&standalone), "unit_bytes_0") >> 24) & 0xFF,
        u32::from(RAGE),
        "the fixture Character must be a rage user"
    );

    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({FIXTURE_BOLT}, 'Fixture Bolt', 0, 0, 0, 0, 0, 30, 0, 16, 0, 0, 0, 0, 0, 0, \
         0, true, 0, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({}, {FIXTURE_BOLT}, 0, {E_DAMAGE}, 9, 0, 0.0, 0, {T_TARGET_ENEMY}, 0.0, 0, 0, 0, \
         0, 0, 0, 0, false, 0.0)",
        (FIXTURE_BOLT as u64) << 2
    ));

    let mut hit = None;
    for _ in 0..10 {
        let events = standalone.subscribe(&[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {FIXTURE_BOLT} AND kind = 2"
        )]);
        standalone.assert_call(
            "debug_cast_at",
            &[
                &wolf.to_string(),
                &FIXTURE_BOLT.to_string(),
                &WARRIOR.to_string(),
            ],
        );
        let go = wait_for_event_rows(&events, "game_spell_cast_event_live")
            .pop()
            .expect("the cast must report its GO");
        if number::<u32>(&go, "miss_info") != 0 {
            continue;
        }
        assert!(
            poll_until(POLL_TIMEOUT, || number::<u32>(
                &warrior(&standalone),
                "health"
            ) < HEALTH),
            "a bolt that did not miss must land"
        );
        hit = Some(warrior(&standalone));
        break;
    }
    let after = hit.expect("ten casts from a level 60 wolf must land at least once");
    let taken = HEALTH - number::<u32>(&after, "health");
    assert_eq!(
        number::<u32>(&after, "power"),
        taken * 3,
        "a spell hit grants the rage for {taken} damage taken"
    );

    standalone.assert_call("debug_set_power", &[&WARRIOR.to_string(), "0"]);
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[&WARRIOR.to_string(), &wolf.to_string(), "true", "1"],
    );
    let ticks = standalone.subscribe(&[&format!(
        "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {DOT} AND kind = {PERIODIC_LOG}"
    )]);
    make_due(
        &standalone,
        &format!(", eff_kind = {A_PERIODIC_DAMAGE}, amount = 7"),
    );
    assert!(
        poll_until(POLL_TIMEOUT, || event_rows(
            &ticks,
            "game_spell_cast_event_live"
        )
        .len()
            == 1),
        "the periodic damage must tick"
    );
    assert_eq!(
        number::<u32>(&warrior(&standalone), "power"),
        7 * 3,
        "a 7 damage physical tick grants 21 tenths of rage"
    );

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1, power = 0 WHERE guid = {WARRIOR}"
    ));
    make_due(&standalone, "");
    assert!(
        poll_until(POLL_TIMEOUT, || event_rows(
            &ticks,
            "game_spell_cast_event_live"
        )
        .len()
            == 2),
        "the periodic damage must tick again"
    );
    let floored = warrior(&standalone);
    assert_eq!(number::<u32>(&floored, "health"), 1);
    assert_eq!(
        number::<u32>(&floored, "power"),
        0,
        "a tick that takes no health grants no rage"
    );
}

fn make_due(standalone: &Standalone, also_set: &str) {
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        - 1_000_000;
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET period_ms = 600000, next_tick_micros = {due}{also_set} \
         WHERE spell_id = {DOT}"
    ));
}

fn warrior(standalone: &Standalone) -> Row {
    standalone
        .query_rows(&format!(
            "SELECT health, power, unit_bytes_0 FROM game_world_entity WHERE guid = {WARRIOR}"
        ))
        .pop()
        .expect("the Warrior must be live")
}
