mod support;

use lyracore_shared::spell::taxonomy::{
    A_IMMUNITY, CAST_FLAG_COMBO_FINISHER, E_FINISHER_DAMAGE, P_MECHANIC, T_SELF, T_TARGET_ENEMY,
};

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use support::{number, wait_for_event_rows, Standalone};

const CHARACTER: u64 = 1;
const FINISHER: u32 = 509_0110;
const STUN_IMMUNITY: u32 = 509_0111;
// vmangos SharedDefines.h MECHANIC_STUN, and SpellMissInfo Immune.
const MECHANIC_STUN: u8 = 12;
const MISS_IMMUNE: u32 = 7;
// vmangos SpellDamageClass.
const DAMAGE_CLASS_MELEE: u8 = 2;

fn stage_finisher(name: &str) -> (Standalone, u64) {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&CHARACTER.to_string(), &TEST_WOLF_ENTRY.to_string(), "5.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, health = 100000, \
         max_health = 100000 WHERE guid = {wolf}"
    ));
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &FINISHER),
            ("name", &"Fixture Finisher"),
            ("range_yd", &5),
            ("school_mask", &1),
            ("damage_class", &DAMAGE_CLASS_MELEE),
            ("is_negative", &true),
            ("cast_flags", &CAST_FLAG_COMBO_FINISHER),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(FINISHER, 0)),
            ("spell_id", &FINISHER),
            ("kind", &E_FINISHER_DAMAGE),
            ("base_points", &1),
            ("target", &T_TARGET_ENEMY),
            ("points_per_combo", &5.0),
        ],
    );
    standalone.insert_row(
        "game_combo_point",
        &[
            ("id", &0),
            ("owner_guid", &CHARACTER),
            ("target_guid", &wolf),
            ("points", &3),
        ],
    );

    (standalone, wolf)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_finisher_adds_its_per_point_damage_and_spends_the_points() {
    let (standalone, wolf) = stage_finisher("combo-finisher");
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_finisher_that_a_mechanic_immunity_refuses_keeps_its_combo_points() {
    let (standalone, wolf) = stage_finisher("combo-finisher-immune");
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET mechanic = {MECHANIC_STUN} WHERE spell_id = {FINISHER}"
    ));
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &STUN_IMMUNITY),
            ("name", &"Fixture Stun Immunity"),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(STUN_IMMUNITY, 0)),
            ("spell_id", &STUN_IMMUNITY),
            ("kind", &A_IMMUNITY),
            ("target", &T_SELF),
            ("p_0", &i32::from(MECHANIC_STUN)),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
    standalone.assert_call(
        "debug_force_cast",
        &[&wolf.to_string(), &STUN_IMMUNITY.to_string()],
    );
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
    assert_eq!(number::<u32>(&go, "miss_info"), MISS_IMMUNE);
    let points = standalone.query_rows(&format!(
        "SELECT points FROM game_combo_point WHERE owner_guid = {CHARACTER}"
    ));
    assert_eq!(
        number::<u32>(&points[0], "points"),
        3,
        "vmangos Spell::finish keeps the points when an enemy target has a miss condition, Immune included"
    );
}
