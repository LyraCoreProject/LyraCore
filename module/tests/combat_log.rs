mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use lyracore_shared::spell::taxonomy::{
    A_PERIODIC_DAMAGE, E_DAMAGE, E_ENERGIZE, T_SELF, T_TARGET_ENEMY,
};

use support::{
    event_rows, fence_subscription, number, poll_until, subscribe_with_fence, wait_for_event_rows,
    Row, Standalone, TableSubscription, POLL_TIMEOUT,
};

const CHARACTER: u64 = 1;
const WOLF: u64 = (0xF130_u64 << 48) | (51_000_u64 << 24) | 1;
/// `SpellCastEventKind::PeriodicLog`.
const PERIODIC_LOG: u8 = 6;
/// `debug_fill_aura_slots` spell ids: `9_000_000 + slot`; debuff slots start at 32.
const HOT: u32 = 9_000_000;
const ENERGIZE: u32 = 9_000_001;
const DOT: u32 = 9_000_032;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn damage_heal_and_energize_ticks_each_log_their_outcome() {
    let mut standalone = Standalone::start("periodic-aura-log");
    standalone.publish_module();
    standalone.assert_sql("DELETE FROM game_creature_move_schedule");
    standalone.assert_sql("DELETE FROM game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 100, max_health = 100 WHERE guid = {WOLF}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 50, max_health = 100, power = 0, max_power = 100 \
         WHERE guid = {CHARACTER}"
    ));
    let character = standalone
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .pop()
        .expect("the Character must be live");
    let power_type = (number::<u32>(&character, "unit_bytes_0") >> 24) & 0xFF;

    standalone.assert_call(
        "debug_fill_aura_slots",
        &[&WOLF.to_string(), &CHARACTER.to_string(), "true", "1"],
    );
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[&CHARACTER.to_string(), &CHARACTER.to_string(), "false", "2"],
    );
    // 0x90 periodic damage, 0x91 periodic heal, 0x92 periodic energize.
    for (spell_id, kind, amount) in [(DOT, 0x90, 7), (HOT, 0x91, 7), (ENERGIZE, 0x92, 5)] {
        standalone.assert_sql(&format!(
            "UPDATE game_aura SET eff_kind = {kind}, amount = {amount} WHERE spell_id = {spell_id}"
        ));
    }
    let events = subscribe_with_fence(
        &standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE kind = {PERIODIC_LOG}"
        )],
    );
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        - 1_000_000;
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET period_ms = 600000, next_tick_micros = {due}"
    ));

    assert!(
        poll_until(POLL_TIMEOUT, || logs(&events).len() >= 3),
        "every tick must leave a log, got {:?}",
        logs(&events)
    );
    fence_subscription(&standalone, &events);
    let logs = logs(&events);
    let mut spells: Vec<u32> = logs.iter().map(|row| number(row, "spell_id")).collect();
    spells.sort_unstable();
    assert_eq!(
        spells,
        [HOT, ENERGIZE, DOT],
        "one log per ticking aura: {logs:?}"
    );
    let log = |spell_id: u32| -> &Row {
        logs.iter()
            .find(|row| number::<u32>(row, "spell_id") == spell_id)
            .unwrap_or_else(|| panic!("no log for spell {spell_id}: {logs:?}"))
    };

    let dot = log(DOT);
    assert_eq!(number::<u64>(dot, "target_guid"), WOLF);
    assert_eq!(number::<u64>(dot, "caster_guid"), CHARACTER);
    assert_eq!(
        number::<u32>(dot, "damage"),
        7,
        "a physical tick is not resisted"
    );
    assert_eq!(number::<u32>(dot, "healed"), 0);

    let hot = log(HOT);
    assert_eq!(number::<u64>(hot, "target_guid"), CHARACTER);
    assert_eq!(number::<u32>(hot, "healed"), 7);
    assert_eq!(number::<u32>(hot, "damage"), 0);

    let energize = log(ENERGIZE);
    assert_eq!(number::<u64>(energize, "target_guid"), CHARACTER);
    assert_eq!(number::<u32>(energize, "energized"), 5);
    assert_eq!(number::<u32>(energize, "power_type"), power_type);
}

fn logs(events: &TableSubscription) -> Vec<Row> {
    event_rows(events, "game_spell_cast_event_live")
}

const SELF_ENERGIZE: u32 = 509_0101;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_direct_energize_records_the_power_given_on_the_go() {
    let mut standalone = Standalone::start("energize-log");
    standalone.publish_module();
    standalone.assert_sql("DELETE FROM game_creature_move_schedule");
    standalone.assert_sql("DELETE FROM game_event_reaper_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_sql(&format!(
        "DELETE FROM game_spell WHERE spell_id = {SELF_ENERGIZE}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({SELF_ENERGIZE}, 'Fixture Energize', 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, \
         0, false, 0, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({}, {SELF_ENERGIZE}, 0, {E_ENERGIZE}, 9, 0, 0.0, 0, {T_SELF}, 0.0, 0, 0, 0, \
         0, 0, 0, 0, false, 0.0)",
        (SELF_ENERGIZE as u64) << 2
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, power = 0, max_power = 100 WHERE guid = {CHARACTER}"
    ));
    let character = standalone
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .pop()
        .expect("the Character must be live");
    let power_type = (number::<u32>(&character, "unit_bytes_0") >> 24) & 0xFF;

    let events = standalone.subscribe(&[&format!(
        "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {SELF_ENERGIZE} AND kind = 2"
    )]);
    standalone.assert_call(
        "debug_cast_at",
        &[
            &CHARACTER.to_string(),
            &SELF_ENERGIZE.to_string(),
            &CHARACTER.to_string(),
        ],
    );

    let go = wait_for_event_rows(&events, "game_spell_cast_event_live")
        .pop()
        .expect("the energize must report its GO");
    let power: u32 = number(
        &standalone
            .query_rows(&format!(
                "SELECT power FROM game_world_entity WHERE guid = {CHARACTER}"
            ))
            .pop()
            .expect("the Character must be live"),
        "power",
    );
    assert_eq!(
        power, 9,
        "base_points 9 with no die fills an empty pool by 9"
    );
    assert_eq!(
        number::<u32>(&go, "energized"),
        9,
        "the GO names the power given to an empty pool"
    );
    assert_eq!(number::<u32>(&go, "power_type"), power_type);
}

const FIRE_WITH_DOT: u32 = 509_0102;
/// `CAST_MISS_RESIST`.
const CAST_MISS_RESIST: u32 = 2;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_resisted_spell_lands_none_of_its_effects_and_reports_the_resist() {
    let mut standalone = Standalone::start("cast-miss");
    standalone.publish_module();
    standalone.assert_sql("DELETE FROM game_creature_move_schedule");
    standalone.assert_sql("DELETE FROM game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&CHARACTER.to_string(), "51000", "5.0"],
    );
    let wolf = standalone
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .expect("the spawned wolf must exist");
    // Level 1 against level 60 misses 99% of the time.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 1 WHERE guid = {CHARACTER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, health = 100000, max_health = 100000 \
         WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({FIRE_WITH_DOT}, 'Fixture Fire', 0, 0, 0, 0, 0, 30, 12000, 4, 0, 0, 0, 0, 0, 0, \
         0, true, 0, 0, 0, 0, 0, 0, 0, 0)"
    ));
    for (index, kind, period_ms) in [(0u64, E_DAMAGE, 0), (1, A_PERIODIC_DAMAGE, 3000)] {
        standalone.assert_sql(&format!(
            "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, \
             die_sides, per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, \
             effect_mechanic, p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
             VALUES ({}, {FIRE_WITH_DOT}, {index}, {kind}, 9, 0, 0.0, {period_ms}, \
             {T_TARGET_ENEMY}, 0.0, 0, 0, 0, 0, 0, 0, 0, false, 0.0)",
            ((FIRE_WITH_DOT as u64) << 2) | index
        ));
    }

    let mut missed = None;
    for _ in 0..10 {
        standalone.assert_sql(&format!(
            "DELETE FROM game_aura WHERE spell_id = {FIRE_WITH_DOT}"
        ));
        let events = standalone.subscribe(&[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {FIRE_WITH_DOT} AND kind = 2"
        )]);
        standalone.assert_call(
            "debug_cast_at",
            &[
                &CHARACTER.to_string(),
                &FIRE_WITH_DOT.to_string(),
                &wolf.to_string(),
            ],
        );
        let go = wait_for_event_rows(&events, "game_spell_cast_event_live")
            .pop()
            .expect("the cast must report its GO");
        if number::<u32>(&go, "miss_info") != 0 {
            missed = Some(go);
            break;
        }
    }
    let go = missed.expect("ten casts at a 99% miss chance must miss at least once");
    assert_eq!(
        number::<u32>(&go, "miss_info"),
        CAST_MISS_RESIST,
        "a fire spell is resisted"
    );
    assert_eq!(number::<u32>(&go, "damage"), 0);
    assert!(
        standalone
            .query_rows(&format!(
                "SELECT id FROM game_aura WHERE spell_id = {FIRE_WITH_DOT}"
            ))
            .is_empty(),
        "a resisted spell must not apply its damage-over-time aura"
    );
}
