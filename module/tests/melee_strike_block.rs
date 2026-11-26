mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_INERT, CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, E_WEAPON_STRIKE, P_NONE, T_SELF,
    T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Row, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const WOLF_LEVEL: u32 = 63;
const STRIKE: u32 = 5_131_701;
const PROC_WATCH: u32 = 5_131_702;
const PHYSICAL: u32 = 0x01;
const DAMAGE_CLASS_MELEE: u8 = 2;
// Spell.dbc Attributes of an instant melee ability: ability, not while shapeshifted, no sheath.
const STRIKE_ATTRIBUTES: u32 = 0x0041_0010;
const SPELL_ATTR_NO_ACTIVE_DEFENSE: u32 = 0x0020_0000;
const WATCH_CHARGES: u8 = 3;
const DURATION_MS: u32 = 600_000;
// vmangos SpellDefines.h ProcFlags and ProcFlagsEx.
const TAKEN_MELEE_SPELL_HIT: u32 = 0x20;
const PROC_EX_BLOCK: u32 = 0x40;
const GO_EVENTS: &str =
    "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND kind = 2";
// A cast melee strike draws the Caster stream: its hit roll, the effect points, then the block.
// Splitmix states whose first draw, 8467 and 8682, falls above the wolf's 3650 basis point dodge
// band, since the fixture wolf cannot parry. The third draw, 3 and 9487, is the block draw against
// the wolf's 500 basis point block chance; draws 2 to 5 of the blocking state each fall under 200.
const BLOCKING_CASTER_STATE: u64 = 12_934_727;
const CLEAR_CASTER_STATE: u64 = 4;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cast_melee_strike_is_partly_blocked_by_a_facing_creature_and_reports_it() {
    let mut standalone = Standalone::start("melee-strike-block");
    let wolf = stage(&mut standalone);
    stage_strike(&standalone, 40, STRIKE_ATTRIBUTES);
    face_each_other(&standalone, wolf);
    seed_caster(&standalone, wolf, BLOCKING_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    assert_eq!(revenge_until_ms(&standalone, wolf), 0);
    strike(&standalone, wolf);
    assert!(
        revenge_until_ms(&standalone, wolf) > 0,
        "vmangos arms Revenge on the target of a blocked melee ability"
    );
    reseed_caster(&standalone, CLEAR_CASTER_STATE);
    strike(&standalone, wolf);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(
        logs.len(),
        2,
        "each landed strike logs its damage: {logs:?}"
    );
    let (blocked, unblocked) = (&logs[0], &logs[1]);
    assert_eq!(unblocked["blocked_amount"], "0");
    assert!(number(blocked, "blocked_amount") > 0, "{blocked:?}");
    assert!(number(blocked, "damage") > 0, "{blocked:?}");
    assert_eq!(
        number(blocked, "damage") + number(blocked, "blocked_amount"),
        number(unblocked, "damage"),
        "the same strike loses exactly the blocked amount"
    );
    assert_eq!(
        health(&standalone, wolf),
        WOLF_HEALTH - number(blocked, "damage") - number(unblocked, "damage")
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cast_melee_strike_with_no_active_defense_is_never_blocked() {
    let mut standalone = Standalone::start("melee-strike-no-active-defense");
    let wolf = stage(&mut standalone);
    stage_strike(
        &standalone,
        40,
        STRIKE_ATTRIBUTES | SPELL_ATTR_NO_ACTIVE_DEFENSE,
    );
    face_each_other(&standalone, wolf);
    seed_caster(&standalone, wolf, BLOCKING_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);

    strike(&standalone, wolf);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(logs[0]["blocked_amount"], "0");
    assert!(number(&logs[0], "damage") > 0, "{logs:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fully_blocked_cast_melee_strike_fires_the_victims_block_proc() {
    assert_full_block_spends_a_watch_charge("melee-strike-full-block", Some(PROC_EX_BLOCK));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fully_blocked_cast_melee_strike_also_raises_a_normal_hit_for_a_default_proc() {
    assert_full_block_spends_a_watch_charge("melee-strike-default-proc", None);
}

fn assert_full_block_spends_a_watch_charge(name: &str, proc_ex: Option<u32>) {
    let mut standalone = Standalone::start(name);
    let wolf = stage(&mut standalone);
    stage_strike(&standalone, 5, STRIKE_ATTRIBUTES);
    face_each_other(&standalone, wolf);
    stage_proc_watch(&standalone, wolf, proc_ex);
    seed_caster(&standalone, wolf, BLOCKING_CASTER_STATE);
    let events = subscribe_with_fence(&standalone, &[GO_EVENTS]);
    assert_eq!(watch_charges(&standalone, wolf), Some(WATCH_CHARGES));

    strike(&standalone, wolf);
    fence_subscription(&standalone, &events);

    let logs = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(logs[0]["damage"], "0", "the wolf blocks the whole strike");
    assert!(number(&logs[0], "blocked_amount") > 0, "{logs:?}");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
    assert_eq!(
        watch_charges(&standalone, wolf),
        Some(WATCH_CHARGES - 1),
        "vmangos raises PROC_EX_BLOCK with the hit's quality on the victim for a full block"
    );
}

fn number(row: &Row, column: &str) -> u32 {
    row[column].parse().unwrap()
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
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
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = {WOLF_LEVEL}, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
    ));
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &PLAYER), ("state", &1)],
    );
    standalone.insert_row(
        "game_creature_defense",
        &[
            ("entry", &TEST_WOLF_ENTRY),
            ("no_parry", &true),
            ("no_block", &false),
        ],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

// The caster stands 15 yards east of the wolf, and both face each other.
fn face_each_other(standalone: &Standalone, wolf: u64) {
    let wolf_row = standalone.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {wolf}"
    ));
    let wolf_x: f32 = wolf_row[0]["x"].parse().unwrap();
    let wolf_y: f32 = wolf_row[0]["y"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0 WHERE guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {wolf_y}, orientation = {} WHERE guid = {PLAYER}",
        wolf_x + 15.0,
        std::f32::consts::PI
    ));
}

fn stage_strike(standalone: &Standalone, weapon_damage: u32, attributes: u32) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &STRIKE),
            ("name", &"Melee strike fixture"),
            ("range_yd", &30),
            ("school_mask", &PHYSICAL),
            ("attributes", &attributes),
            ("damage_class", &DAMAGE_CLASS_MELEE),
            (
                "cast_flags",
                &(CAST_FLAG_ALWAYS_HIT | CAST_FLAG_CANNOT_CRIT),
            ),
            ("is_negative", &true),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(STRIKE, 0)),
            ("spell_id", &STRIKE),
            ("kind", &E_WEAPON_STRIKE),
            ("base_points", &0),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &100),
            ("p_0_kind", &255),
        ],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET damage_min = {weapon_damage}.0, \
         damage_max = {weapon_damage}.0 WHERE entry = {FIXTURE_BLADE}"
    ));
}

// A Charged Aura on the wolf that spends a charge on a melee spell hit it takes; without a
// proc_ex the default applies.
fn stage_proc_watch(standalone: &Standalone, wolf: u64, proc_ex: Option<u32>) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &PROC_WATCH),
            ("name", &"Proc watch fixture"),
            ("school_mask", &PHYSICAL),
            ("range_yd", &30),
            ("duration_ms", &DURATION_MS),
            ("spell_level", &1),
            ("proc_flags", &TAKEN_MELEE_SPELL_HIT),
            ("proc_chance", &100),
            ("proc_charges", &WATCH_CHARGES),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PROC_WATCH, 0)),
            ("spell_id", &PROC_WATCH),
            ("effect_index", &0),
            ("kind", &A_INERT),
            ("base_points", &0),
            ("target", &T_SELF),
            ("p_0", &0),
            ("p_0_kind", &P_NONE),
        ],
    );
    if let Some(proc_ex) = proc_ex {
        standalone.insert_row(
            "game_spell_proc_event",
            &[("spell_id", &PROC_WATCH), ("proc_ex", &proc_ex)],
        );
    }
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &wolf.to_string(),
            &PROC_WATCH.to_string(),
            &wolf.to_string(),
        ],
    );
}

fn watch_charges(standalone: &Standalone, wolf: u64) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT proc_charges FROM game_aura WHERE target_guid = {wolf} \
             AND spell_id = {PROC_WATCH}"
        ))
        .first()
        .map(|row| row["proc_charges"].parse().unwrap())
}

fn seed_caster(standalone: &Standalone, wolf: u64, state: u64) {
    standalone.insert_row(
        "game_conformance_spell_random",
        &[
            ("character_guid", &PLAYER),
            ("target_guid", &wolf),
            ("state", &state),
        ],
    );
}

fn reseed_caster(standalone: &Standalone, state: u64) {
    standalone.assert_sql(&format!(
        "UPDATE game_conformance_spell_random SET state = {state} WHERE character_guid = {PLAYER}"
    ));
}

fn strike(standalone: &Standalone, wolf: u64) {
    standalone.assert_call(
        "debug_cast_at",
        &[&PLAYER.to_string(), &STRIKE.to_string(), &wolf.to_string()],
    );
}

fn revenge_until_ms(standalone: &Standalone, guid: u64) -> u64 {
    standalone.query_rows(&format!(
        "SELECT revenge_until_ms FROM game_world_entity WHERE guid = {guid}"
    ))[0]["revenge_until_ms"]
        .parse()
        .unwrap()
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
