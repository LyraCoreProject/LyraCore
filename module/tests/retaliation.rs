mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_RETALIATE, CAST_FLAG_ALWAYS_HIT, E_WEAPON_STRIKE, M_STUN, P_MECHANIC, P_RAW,
    T_SELF, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const CHARGES: u8 = 30;
// vmangos spells 20230 (the Aura) and 22858 (the strike it casts).
const RETALIATION: u32 = 20_230;
const RETALIATION_STRIKE: u32 = 22_858;
const WOLF_STUN: u32 = 5_231_000;
const WOLF_STRIKE: u32 = 5_231_001;
// vmangos ProcFlags: Retaliation 20230 procs on a melee hit and a melee ability hit taken (0x28).
// SpellDamageClass melee, and SpellMissInfo Immune.
const TAKE_MELEE_SWING: u32 = 0x8;
const TAKE_MELEE_ABILITY: u32 = 0x20;
const DAMAGE_CLASS_MELEE: u8 = 2;
const MISS_IMMUNE: &str = "7";
const MECHANIC_STUN: u8 = 12;
const PHYSICAL: u8 = 0x01;
// Spell.dbc Attributes bit SPELL_ATTR_NO_ACTIVE_DEFENSE.
const ATTR_NO_ACTIVE_DEFENSE: u32 = 0x0020_0000;
const DURATION_MS: u32 = 15_000;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retaliation_strikes_an_attacker_in_front_and_spends_a_charge() {
    let mut standalone = Standalone::start("retaliation-front");
    let wolf = stage(&mut standalone, WolfAt::InFront);

    hit_player(&standalone, wolf);

    assert!(health(&standalone, wolf) < WOLF_HEALTH, "the strike lands");
    assert_eq!(charges(&standalone), Some(CHARGES - 1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retaliation_neither_strikes_nor_spends_on_a_hit_from_behind() {
    let mut standalone = Standalone::start("retaliation-behind");
    let wolf = stage(&mut standalone, WolfAt::Behind);

    hit_player(&standalone, wolf);

    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
    assert_eq!(charges(&standalone), Some(CHARGES));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retaliation_neither_strikes_nor_spends_while_its_warrior_is_stunned() {
    let mut standalone = Standalone::start("retaliation-stunned");
    let wolf = stage(&mut standalone, WolfAt::InFront);
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &WOLF_STUN),
            ("name", &"Fixture Stun"),
            ("school_mask", &PHYSICAL),
            ("range_yd", &30),
            ("is_negative", &true),
            ("duration_ms", &DURATION_MS),
            ("mechanic", &MECHANIC_STUN),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(WOLF_STUN, 0)),
            ("spell_id", &WOLF_STUN),
            ("kind", &A_CONTROL),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &M_STUN),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
    standalone.assert_call(
        "debug_cast_at",
        &[
            &wolf.to_string(),
            &WOLF_STUN.to_string(),
            &PLAYER.to_string(),
        ],
    );
    advance(&standalone);

    hit_player(&standalone, wolf);

    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
    assert_eq!(charges(&standalone), Some(CHARGES));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retaliation_spends_a_charge_on_an_attacker_immune_to_physical_damage_and_reports_immune() {
    let mut standalone = Standalone::start("retaliation-immune");
    let wolf = stage(&mut standalone, WolfAt::InFront);
    standalone.assert_sql(&format!(
        "UPDATE game_creature_template SET school_immune_mask = {PHYSICAL} \
         WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    let events = subscribe_with_fence(
        &standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live \
             WHERE spell_id = {RETALIATION_STRIKE} AND kind = 2"
        )],
    );

    hit_player(&standalone, wolf);

    fence_subscription(&standalone, &events);
    let strikes = event_rows(&events, "game_spell_cast_event_live");
    assert_eq!(strikes.len(), 1, "the strike is cast");
    assert_eq!(strikes[0]["miss_info"], MISS_IMMUNE);
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
    assert_eq!(charges(&standalone), Some(CHARGES - 1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn retaliation_strikes_back_at_a_melee_ability_hit() {
    let mut standalone = Standalone::start("retaliation-melee-ability");
    let wolf = stage(&mut standalone, WolfAt::InFront);
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &WOLF_STRIKE),
            ("name", &"Fixture Wolf Strike"),
            ("school_mask", &PHYSICAL),
            ("range_yd", &30),
            ("is_negative", &true),
            ("damage_class", &DAMAGE_CLASS_MELEE),
            ("attributes", &ATTR_NO_ACTIVE_DEFENSE),
            ("cast_flags", &CAST_FLAG_ALWAYS_HIT),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(WOLF_STRIKE, 0)),
            ("spell_id", &WOLF_STRIKE),
            ("kind", &E_WEAPON_STRIKE),
            ("base_points", &100),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &100),
            ("p_0_kind", &P_RAW),
        ],
    );
    standalone.assert_call(
        "debug_cast_at",
        &[
            &wolf.to_string(),
            &WOLF_STRIKE.to_string(),
            &PLAYER.to_string(),
        ],
    );
    advance(&standalone);

    assert!(health(&standalone, wolf) < WOLF_HEALTH, "the strike lands");
    assert_eq!(charges(&standalone), Some(CHARGES - 1));
}

enum WolfAt {
    InFront,
    Behind,
}

fn stage(standalone: &mut Standalone, wolf_at: WolfAt) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.clear_schedule("game_melee_schedule");
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
    let wolf_x = match wolf_at {
        WolfAt::InFront => 1,
        WolfAt::Behind => -1,
    };
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 0, y = 0, orientation = 0, health = 10000, \
         max_health = 10000 WHERE guid = {PLAYER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = 1, \
         x = {wolf_x}, y = 0, health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} \
         WHERE guid = {wolf}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    insert_retaliation(standalone);
    standalone.assert_call("debug_force_cast", &["1", &RETALIATION.to_string()]);
    advance(standalone);
    wolf
}

fn insert_retaliation(standalone: &Standalone) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &RETALIATION),
            ("name", &"Retaliation"),
            ("school_mask", &PHYSICAL),
            ("duration_ms", &DURATION_MS),
            ("spell_level", &1),
            ("proc_flags", &(TAKE_MELEE_SWING | TAKE_MELEE_ABILITY)),
            ("proc_chance", &100),
            ("proc_charges", &CHARGES),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(RETALIATION, 0)),
            ("spell_id", &RETALIATION),
            ("kind", &A_RETALIATE),
            ("target", &T_SELF),
        ],
    );
    // The imported 22858 rolls the melee table. This fixture always hits, so a dodge, a parry or
    // a miss cannot decide a test; its effect and target match the real spell's.
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &RETALIATION_STRIKE),
            ("name", &"Retaliation"),
            ("school_mask", &PHYSICAL),
            ("range_yd", &30),
            ("is_negative", &true),
            ("damage_class", &DAMAGE_CLASS_MELEE),
            ("attributes", &ATTR_NO_ACTIVE_DEFENSE),
            ("cast_flags", &CAST_FLAG_ALWAYS_HIT),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(RETALIATION_STRIKE, 0)),
            ("spell_id", &RETALIATION_STRIKE),
            ("kind", &E_WEAPON_STRIKE),
            ("base_points", &100),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &100),
            ("p_0_kind", &P_RAW),
        ],
    );
}

// A proc never fires off an Aura applied in the same virtual instant.
fn advance(standalone: &Standalone) {
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);
}

fn hit_player(standalone: &Standalone, wolf: u64) {
    standalone.assert_call(
        "debug_apply_damage",
        &[&PLAYER.to_string(), "1", &wolf.to_string()],
    );
    advance(standalone);
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}

fn charges(standalone: &Standalone) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT proc_charges FROM game_aura WHERE target_guid = {PLAYER} \
             AND spell_id = {RETALIATION}"
        ))
        .first()
        .map(|row| row["proc_charges"].parse().unwrap())
}
