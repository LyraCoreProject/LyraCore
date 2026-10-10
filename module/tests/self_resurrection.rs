//! Self-Resurrection Option: chosen at death from a Soulstone aura, used through
//! `debug_self_resurrect` (the `gw_self_resurrect` core), and spent by every other way back.

mod support;

use std::collections::BTreeMap;

use lyracore_shared::constants::{player_flags, unit_flags, unit_vis_flags};
use support::Standalone;

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;

/// Minor Soulstone, from ClassicDB 1.12.1 `spell_template`: the aura rank and the spell it grants.
const SOULSTONE_AURA: u32 = 20707;
const SOULSTONE_SELF_RES: u32 = 3026;
const SOULSTONE_HEALTH: u32 = 400;
const SOULSTONE_MANA: u32 = 700;
const RESURRECTION_SICKNESS: u32 = 15007;

const E_SELF_RESURRECT: u32 = 0x26;
const A_SELF_RESURRECT: u32 = 0xB5;
const P_SPELL_ID: u32 = 15;
const P_FLAT_MANA: u32 = 16;
const T_SELF: u32 = 0;
const T_TARGET_ALLY: u32 = 2;

const WARRIOR: u32 = 1;
const ROGUE: u32 = 4;
const WARLOCK: u32 = 9;
const MANA: u32 = 0;
const RAGE: u32 = 1;
const ENERGY: u32 = 3;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_soulstone_death_holds_the_option_and_using_it_revives_in_place() {
    let standalone = start("self-res-soulstone");
    set_class(&standalone, WARLOCK, MANA, 2000);
    soulstone(&standalone);

    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);

    assert_eq!(option_spell(&standalone), Some(SOULSTONE_SELF_RES));
    assert!(
        !has_aura(&standalone, SOULSTONE_AURA),
        "the Soulstone aura does not survive death"
    );

    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);

    let player = entity(&standalone);
    assert_alive_in_place(&player);
    assert_eq!(number(&player, "health"), SOULSTONE_HEALTH);
    assert_eq!(number(&player, "power"), SOULSTONE_MANA);
    assert_eq!(
        option_spell(&standalone),
        None,
        "using the option spends it"
    );
    assert!(
        !has_aura(&standalone, RESURRECTION_SICKNESS),
        "self-resurrection applies no Resurrection Sickness"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn without_an_option_self_resurrection_is_refused_and_changes_nothing() {
    let standalone = start("self-res-refused");
    soulstone(&standalone);
    let alive = standalone.call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert!(
        !alive.status.success(),
        "a living Character holds no option to use"
    );

    standalone.assert_sql(&format!(
        "DELETE FROM game_aura WHERE target_guid = {PLAYER} AND spell_id = {SOULSTONE_AURA}"
    ));
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    assert_eq!(option_spell(&standalone), None);

    let dead = standalone.call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert!(
        !dead.status.success(),
        "a death without a Soulstone gives no option"
    );
    let player = entity(&standalone);
    assert_eq!(player["dead"], "true");
    assert_eq!(number(&player, "health"), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn the_option_survives_release_spirit_and_revives_the_ghost() {
    let standalone = start("self-res-ghost");
    set_class(&standalone, WARLOCK, MANA, 2000);
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    standalone.assert_call("debug_repop", &[&PLAYER.to_string()]);

    let ghost = entity(&standalone);
    assert_ne!(number(&ghost, "player_flags") & player_flags::GHOST, 0);
    assert_eq!(option_spell(&standalone), Some(SOULSTONE_SELF_RES));
    assert_eq!(corpse_count(&standalone), 1);

    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);

    let player = entity(&standalone);
    assert_alive_in_place(&player);
    assert_eq!(number(&player, "health"), SOULSTONE_HEALTH);
    assert_eq!(corpse_count(&standalone), 0, "the corpse is gone");
    assert_eq!(option_spell(&standalone), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_spirit_healer_resurrection_or_a_logout_spends_the_option() {
    let standalone = start("self-res-other-paths");
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    standalone.assert_call("debug_repop", &[&PLAYER.to_string()]);
    standalone.assert_call("debug_spirit_healer_res", &[&PLAYER.to_string()]);
    assert_eq!(
        option_spell(&standalone),
        None,
        "a Spirit Healer res spends it"
    );

    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    assert_eq!(option_spell(&standalone), Some(SOULSTONE_SELF_RES));
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_logout_character", &[&PLAYER.to_string()]);
    assert_eq!(option_spell(&standalone), None, "a logout spends it");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn rage_comes_back_empty_and_energy_comes_back_full() {
    let standalone = start("self-res-power-types");
    set_class(&standalone, WARRIOR, RAGE, 1000);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "500"]);
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert_eq!(number(&entity(&standalone), "power"), 0);

    set_class(&standalone, ROGUE, ENERGY, 100);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "10"]);
    soulstone(&standalone);
    standalone.assert_call("debug_set_health", &[&PLAYER.to_string(), "0"]);
    hold_vitals(&standalone);
    standalone.assert_call("debug_self_resurrect", &[&PLAYER.to_string()]);
    assert_eq!(number(&entity(&standalone), "power"), 100);
}

/// Publish, materialize the Character with room above the flat Soulstone health, and stage the
/// Minor Soulstone spells. The aura has no cast time so the debug cast lands at once.
fn start(name: &str) -> Standalone {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET max_health = 1000, health = 1000 WHERE guid = {PLAYER}"
    ));
    insert_spell(
        &standalone,
        SOULSTONE_AURA,
        "Soulstone Resurrection",
        1_800_000,
    );
    insert_effect(
        &standalone,
        SOULSTONE_AURA,
        A_SELF_RESURRECT,
        0,
        T_TARGET_ALLY,
        SOULSTONE_SELF_RES as i32,
        P_SPELL_ID,
    );
    insert_spell(&standalone, SOULSTONE_SELF_RES, "Use Soulstone", 0);
    insert_effect(
        &standalone,
        SOULSTONE_SELF_RES,
        E_SELF_RESURRECT,
        -(SOULSTONE_HEALTH as i32),
        T_SELF,
        SOULSTONE_MANA as i32,
        P_FLAT_MANA,
    );
    standalone
}

fn soulstone(standalone: &Standalone) {
    standalone.assert_call(
        "debug_cast_at",
        &[
            &PLAYER.to_string(),
            &SOULSTONE_AURA.to_string(),
            &PLAYER.to_string(),
        ],
    );
    assert!(has_aura(standalone, SOULSTONE_AURA));
}

/// Rewrite the class and power-type bytes of `unit_bytes_0` and the power pool.
fn set_class(standalone: &Standalone, class: u32, power_type: u32, max_power: u32) {
    let bytes = number(&entity(standalone), "unit_bytes_0");
    let bytes = (bytes & 0x00FF_00FF) | (class << 8) | (power_type << 24);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {bytes}, max_power = {max_power}, \
         power = {max_power} WHERE guid = {PLAYER}"
    ));
}

/// Keep scheduled regeneration and rage decay from moving an exact vitals observation: in combat
/// until the end of time, and inside the mana regen pause.
fn hold_vitals(standalone: &Standalone) {
    let flags = number(&entity(standalone), "unit_flags");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {}, combat_until_ms = {}, \
         mana_regen_paused_until_ms = {} WHERE guid = {PLAYER}",
        flags | unit_flags::IN_COMBAT,
        u64::MAX,
        u64::MAX,
    ));
}

fn assert_alive_in_place(player: &SqlRow) {
    assert_eq!(player["dead"], "false");
    assert_eq!(number(player, "player_flags") & player_flags::GHOST, 0);
    assert_eq!(number(player, "unit_bytes_1") & unit_vis_flags::GHOST, 0);
}

fn insert_spell(standalone: &Standalone, spell_id: u32, name: &str, duration_ms: u32) {
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges) \
         VALUES ({spell_id}, '{name}', 0, 0, 0, 0, 0, 0, {duration_ms}, 32, 0, 0, 0, 0, 0, 0, 0, \
         false, 0, 0, 0, 0, 0, 0, 0)"
    ));
}

/// One effect row at index 0. The self-resurrect row keeps vanilla's one-sided die, which the rule
/// must ignore.
fn insert_effect(
    standalone: &Standalone,
    spell_id: u32,
    kind: u32,
    base_points: i32,
    target: u32,
    p0: i32,
    p0_kind: u32,
) {
    let id = (spell_id as u64) << 2;
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat) \
         VALUES ({id}, {spell_id}, 0, {kind}, {base_points}, 1, 0.0, 0, {target}, \
         0.0, 0, 0, 0, {p0}, {p0_kind}, 0, 0, false)"
    ));
}

fn entity(standalone: &Standalone) -> SqlRow {
    standalone
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .pop()
        .expect("the Character must be live")
}

fn number(row: &SqlRow, column: &str) -> u32 {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is a number: {}", row[column]))
}

fn option_spell(standalone: &Standalone) -> Option<u32> {
    standalone
        .query_rows(&format!(
            "SELECT spell_id FROM game_self_resurrect_option WHERE character_guid = {PLAYER}"
        ))
        .pop()
        .map(|row| number(&row, "spell_id"))
}

fn has_aura(standalone: &Standalone, spell_id: u32) -> bool {
    !standalone
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {PLAYER} AND spell_id = {spell_id}"
        ))
        .is_empty()
}

fn corpse_count(standalone: &Standalone) -> usize {
    standalone
        .query_rows(&format!(
            "SELECT guid FROM game_corpse WHERE owner_guid = {PLAYER}"
        ))
        .len()
}
