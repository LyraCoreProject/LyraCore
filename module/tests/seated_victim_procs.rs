mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{A_INERT, E_NEXT_SWING, P_NONE, T_SELF, T_TARGET_ENEMY};
use support::{
    actor, event_rows, fence_subscription, number, subscribe_with_fence, Row, Standalone,
    TableSubscription,
};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 100_000;
const CRIT_WATCH: u32 = 5_136_402;
const WOLF_STRIKE: u32 = 5_136_403;
const WATCH_CHARGES: u8 = 3;
// vmangos SpellDefines.h ProcFlags and ProcFlagsEx.
const TAKEN_MELEE_HIT: u32 = 0x08;
const PROC_EX_CRITICAL_HIT: u32 = 0x02;
const CRIT: &str = "1";
const SITTING: &str = "1";
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live";
const WOLF_SWING_ATTEMPTS: usize = 20;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_sitting_character_gains_no_crit_proc_from_a_forced_white_crit_but_does_from_a_queued_strike() {
    let mut standalone = Standalone::start("seated-victim-procs");
    let wolf = stage(&mut standalone);
    standalone.assert_call("debug_engage", &[&wolf.to_string(), "1"]);

    let white = wolf_swing_at_the_sitting_player(&standalone, wolf);
    assert_eq!(white["hit_info"], CRIT, "{white:?}");
    assert_eq!(white["spell_swing"], "false", "{white:?}");
    assert_eq!(
        stand_state(&standalone),
        0,
        "the hit stands the Character up"
    );
    assert_eq!(
        watch_charges(&standalone),
        Some(WATCH_CHARGES),
        "vmangos drops the crit of a white swing from a sitting victim's procs"
    );

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET next_swing_spell = {WOLF_STRIKE} WHERE guid = {wolf}"
    ));
    let strike = wolf_swing_at_the_sitting_player(&standalone, wolf);
    assert_eq!(strike["hit_info"], CRIT, "{strike:?}");
    assert_eq!(strike["spell_swing"], "true", "{strike:?}");
    assert_eq!(
        watch_charges(&standalone),
        Some(WATCH_CHARGES - 1),
        "a queued strike's crit on a sitting victim keeps its crit proc"
    );
}

// The wolf stands at the Character's feet and never moves; the Character holds a Charged Aura
// that spends a charge on a melee hit crit it takes.
fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
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
        "UPDATE game_world_entity SET simulation_indexed = false WHERE guid = {wolf}"
    ));
    stage_crit_watch(standalone);
    stage_wolf_strike(standalone);
    standalone.assert_call("debug_advance_roll_batch", &["1"]);
    wolf
}

fn stage_crit_watch(standalone: &Standalone) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &CRIT_WATCH),
            ("name", &"Crit watch fixture"),
            ("school_mask", &1),
            ("range_yd", &30),
            ("duration_ms", &600_000),
            ("spell_level", &1),
            ("proc_flags", &TAKEN_MELEE_HIT),
            ("proc_chance", &100),
            ("proc_charges", &WATCH_CHARGES),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(CRIT_WATCH, 0)),
            ("spell_id", &CRIT_WATCH),
            ("effect_index", &0),
            ("kind", &A_INERT),
            ("base_points", &0),
            ("target", &T_SELF),
            ("p_0", &0),
            ("p_0_kind", &P_NONE),
        ],
    );
    standalone.insert_row(
        "game_spell_proc_event",
        &[
            ("spell_id", &CRIT_WATCH),
            ("proc_ex", &PROC_EX_CRITICAL_HIT),
        ],
    );
    standalone.assert_call("debug_force_cast_at", &["1", &CRIT_WATCH.to_string(), "1"]);
}

fn stage_wolf_strike(standalone: &Standalone) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &WOLF_STRIKE),
            ("name", &"Wolf strike fixture"),
            ("range_yd", &5),
            ("school_mask", &1),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(WOLF_STRIKE, 0)),
            ("spell_id", &WOLF_STRIKE),
            ("kind", &E_NEXT_SWING),
            ("base_points", &11),
            ("target", &T_TARGET_ENEMY),
        ],
    );
}

// Sits the Character and lets the wolf swing once; returns that swing's combat event.
fn wolf_swing_at_the_sitting_player(standalone: &Standalone, wolf: u64) -> Row {
    standalone.assert_call("gw_set_stand_state", &[&actor(PLAYER), SITTING]);
    let events = subscribe_with_fence(standalone, &[SWING_EVENTS]);
    for _ in 0..WOLF_SWING_ATTEMPTS {
        if let Some(swing) = wolf_swing(&events, wolf) {
            return swing;
        }
        standalone.assert_sql(&format!(
            "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {wolf}"
        ));
        standalone.assert_call("debug_advance_roll_batch", &["4"]);
        fence_subscription(standalone, &events);
    }
    wolf_swing(&events, wolf).expect("the wolf must swing")
}

fn wolf_swing(events: &TableSubscription, wolf: u64) -> Option<Row> {
    event_rows(events, "game_combat_event_live")
        .into_iter()
        .find(|row| row["attacker_guid"] == wolf.to_string())
}

fn stand_state(standalone: &Standalone) -> u32 {
    number::<u32>(
        &standalone.query_rows(&format!(
            "SELECT unit_bytes_1 FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0],
        "unit_bytes_1",
    ) & 0xFF
}

fn watch_charges(standalone: &Standalone) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT proc_charges FROM game_aura WHERE target_guid = {PLAYER} \
             AND spell_id = {CRIT_WATCH}"
        ))
        .first()
        .map(|row| row["proc_charges"].parse().unwrap())
}
