mod support;

use lyracore_shared::spell::taxonomy::{
    E_FINISHER_DAMAGE, SPELL_ATTR_COMBO_FINISHER, T_TARGET_ENEMY,
};

use support::{number, wait_for_event_rows, Standalone};

const CHARACTER: u64 = 1;
const FINISHER: u32 = 509_0110;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_finisher_adds_its_per_point_damage_and_spends_the_points() {
    let mut standalone = Standalone::start("combo-finisher");
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
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, health = 100000, \
         max_health = 100000 WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({FINISHER}, 'Fixture Finisher', 0, 0, 0, 0, 0, 5, 0, 1, 0, 0, 0, 0, 0, 0, \
         0, true, {SPELL_ATTR_COMBO_FINISHER}, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, \
         die_sides, per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, \
         effect_mechanic, p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({}, {FINISHER}, 0, {E_FINISHER_DAMAGE}, 1, 0, 0.0, 0, {T_TARGET_ENEMY}, 0.0, \
         0, 0, 0, 0, 0, 0, 0, false, 5.0)",
        (FINISHER as u64) << 2
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_combo_point (id, owner_guid, target_guid, points) \
         VALUES (0, {CHARACTER}, {wolf}, 3)"
    ));

    let held = format!("SELECT points FROM game_combo_point WHERE owner_guid = {CHARACTER}");
    for _ in 0..10 {
        let events = standalone.subscribe(&[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {FINISHER} AND kind = 2"
        )]);
        standalone.assert_call(
            "debug_cast_at",
            &[
                &CHARACTER.to_string(),
                &FINISHER.to_string(),
                &wolf.to_string(),
            ],
        );
        let go = wait_for_event_rows(&events, "game_spell_cast_event_live")
            .pop()
            .expect("the finisher must report its GO");
        if number::<u32>(&go, "miss_info") != 0 {
            let points = standalone.query_rows(&held);
            assert_eq!(
                points.len(),
                1,
                "an avoided finisher keeps its combo points"
            );
            assert_eq!(number::<u32>(&points[0], "points"), 3);
            continue;
        }
        // One base point plus five per point at three points is 16, or 32 on a melee crit.
        let damage = number::<u32>(&go, "damage");
        assert!(matches!(damage, 16 | 32), "damage {damage}");
        assert!(
            standalone.query_rows(&held).is_empty(),
            "the finisher must spend its combo points"
        );
        return;
    }
    panic!("ten finishers must land at least once");
}
