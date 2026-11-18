mod support;

use std::collections::BTreeSet;

use lyracore_shared::spell::taxonomy::*;
use support::{actor, number, Row, Standalone};

const WARRIOR: &str = "1";
const BATTLE_STANCE: u32 = 2457;
const DEFENSIVE_STANCE: u32 = 71;
const BERSERKER_STANCE: u32 = 2458;
const BATTLE_PASSIVE: u32 = 21156;
const DEFENSIVE_PASSIVE: u32 = 7376;
const BERSERKER_PASSIVE: u32 = 7381;
const TACTICAL_MASTERY: u32 = 12295;
const PERMANENT_MS: u64 = 4294967295;
const RAGE: u8 = 1;

fn insert_spell(
    shard: &Standalone,
    spell: u32,
    name: &str,
    (attributes, attributes_ex_3, cast_flags): (u32, u32, u32),
    effects: &[(u8, i32, i32, u8)],
) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell),
            ("name", &name),
            ("duration_ms", &PERMANENT_MS),
            ("school_mask", &1),
            ("gcd_ms", &0),
            ("attributes", &attributes),
            ("attributes_ex_3", &attributes_ex_3),
            ("cast_flags", &cast_flags),
        ],
    );
    for (index, (kind, amount, parameter, parameter_kind)) in effects.iter().copied().enumerate() {
        shard.insert_row(
            "game_spell_effect",
            &[
                ("id", &((spell as u64) << 2 | index as u64)),
                ("spell_id", &spell),
                ("effect_index", &index),
                ("kind", &kind),
                ("base_points", &amount),
                ("target", &T_SELF),
                ("p_0", &parameter),
                ("p_0_kind", &parameter_kind),
            ],
        );
    }
}

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_seed_warrior_forms", &[]);
    shard
}

fn spawned(name: &str) -> Standalone {
    let shard = fixture(name);
    shard.assert_call("debug_spawn_player_entity", &[WARRIOR]);
    shard.assert_call("debug_set_level", &[WARRIOR, "20"]);
    shard
}

fn bind_session(shard: &Standalone) {
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &[WARRIOR, &key, r#"{"__identity__":"0x1"}"#],
    );
}

fn log_in(shard: &Standalone) {
    bind_session(shard);
    shard.assert_call("gw_player_login", &["1", &actor(WARRIOR)]);
}

fn ready(shard: &Standalone) {
    shard.assert_sql("DELETE FROM game_spell_cooldown WHERE caster_guid = 1");
    shard.assert_sql("DELETE FROM game_spell_cd WHERE caster_guid = 1");
}

fn cast(shard: &Standalone, spell: u32) {
    ready(shard);
    shard.assert_call("debug_force_cast", &[WARRIOR, &spell.to_string()]);
}

fn warrior(shard: &Standalone) -> Row {
    shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .remove(0)
}

fn client_form(shard: &Standalone) -> u32 {
    number::<u32>(&warrior(shard), "unit_bytes_1") >> 16 & 0xFF
}

fn aura_spells(shard: &Standalone) -> BTreeSet<u32> {
    shard
        .query_rows("SELECT spell_id FROM game_aura WHERE target_guid = 1")
        .iter()
        .map(|row| number(row, "spell_id"))
        .collect()
}

fn shapeshift_spells(shard: &Standalone) -> Vec<u32> {
    shard
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = 1 AND eff_kind = {A_SHAPESHIFT}"
        ))
        .iter()
        .map(|row| number(row, "spell_id"))
        .collect()
}

fn sheet(shard: &Standalone) -> Row {
    warrior(shard)
        .into_iter()
        .filter(|(column, _)| {
            column.starts_with("sheet_")
                || ["armor", "max_health", "max_power", "run_speed_mult_bp"]
                    .contains(&column.as_str())
        })
        .collect()
}

fn set_rage(shard: &Standalone, rage: u32) {
    shard.assert_call("debug_set_power", &[WARRIOR, &rage.to_string()]);
}

fn rage(shard: &Standalone) -> u32 {
    number(&warrior(shard), "power")
}

fn enter(shard: &Standalone, stance: u32) {
    cast(shard, stance);
    assert_eq!(shapeshift_spells(shard), vec![stance]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn every_form_projects_its_rules_and_leaving_it_restores_the_unit() {
    let shard = spawned("forms-shared-contract");
    let forms = shard.query_rows("SELECT * FROM game_form");
    assert!(!forms.is_empty());
    let passives_of = |form: &str| -> BTreeSet<u32> {
        shard
            .query_rows(&format!(
                "SELECT spell_id FROM game_form_spell WHERE form_id = {form}"
            ))
            .iter()
            .map(|row| number(row, "spell_id"))
            .collect()
    };
    let all_passives: BTreeSet<u32> = forms
        .iter()
        .flat_map(|form| passives_of(&form["form_id"]))
        .collect();
    let unit_power_type = number::<u32>(&warrior(&shard), "unit_bytes_0") >> 24;
    for form in &forms {
        let spell: u32 = number(form, "spell_id");
        let baseline = sheet(&shard);
        assert!(shapeshift_spells(&shard).is_empty());

        set_rage(&shard, 600);
        enter(&shard, spell);
        assert_eq!(client_form(&shard), number::<u32>(form, "client_form"));
        let passives = passives_of(&form["form_id"]);
        let held = aura_spells(&shard);
        assert!(passives.is_subset(&held), "{form:?} holds {held:?}");
        assert!(held
            .intersection(&all_passives)
            .all(|p| passives.contains(p)));
        let expected_rage = if number::<u32>(form, "power_type") != unit_power_type {
            600
        } else {
            match number::<u8>(form, "power_on_entry") {
                0 => 600,
                // Zero, and keeping up to a retained amount the unit does not have.
                1 | 2 => 0,
                rule => panic!("Form {form:?} has unknown power rule {rule}"),
            }
        };
        assert_eq!(rage(&shard), expected_rage, "{form:?}");

        for other in forms
            .iter()
            .filter(|other| other["form_id"] != form["form_id"])
        {
            enter(&shard, number(other, "spell_id"));
            assert!(aura_spells(&shard).is_disjoint(&passives), "{other:?}");
        }
        enter(&shard, spell);

        shard.assert_call("debug_shed_spell_auras", &[WARRIOR, &spell.to_string()]);
        assert!(shapeshift_spells(&shard).is_empty());
        assert_eq!(client_form(&shard), 0);
        assert!(aura_spells(&shard).is_disjoint(&all_passives));
        assert_eq!(sheet(&shard), baseline, "leaving {form:?}");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_warrior_without_a_stance_enters_the_world_in_battle_stance() {
    let shard = fixture("forms-default-stance");
    log_in(&shard);
    assert_eq!(shapeshift_spells(&shard), vec![BATTLE_STANCE]);
    assert!(aura_spells(&shard).contains(&BATTLE_PASSIVE));
    assert_eq!(client_form(&shard), 17);
    let slot = |spell: u32| -> u32 {
        number(
            &shard.query_rows(&format!(
                "SELECT slot FROM game_aura WHERE target_guid = 1 AND spell_id = {spell}"
            ))[0],
            "slot",
        )
    };
    assert_ne!(slot(BATTLE_STANCE), 255, "peers see the stance Aura");
    assert_eq!(slot(BATTLE_PASSIVE), 255, "the passive stays hidden");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stance_survives_logout() {
    let shard = fixture("forms-logout");
    log_in(&shard);
    enter(&shard, DEFENSIVE_STANCE);
    shard.assert_call("debug_logout_character", &[WARRIOR]);
    log_in(&shard);
    assert_eq!(shapeshift_spells(&shard), vec![DEFENSIVE_STANCE]);
    let held = aura_spells(&shard);
    assert!(held.contains(&DEFENSIVE_PASSIVE) && !held.contains(&BATTLE_PASSIVE));
    assert_eq!(client_form(&shard), 18);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stance_and_its_passive_survive_death() {
    let shard = spawned("forms-death");
    enter(&shard, BERSERKER_STANCE);
    shard.assert_call("debug_set_health", &[WARRIOR, "0"]);
    assert_eq!(warrior(&shard)["dead"], "true");
    assert_eq!(shapeshift_spells(&shard), vec![BERSERKER_STANCE]);
    assert!(aura_spells(&shard).contains(&BERSERKER_PASSIVE));
    assert_eq!(client_form(&shard), 19);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stance_cannot_be_cancelled_or_dispelled() {
    let shard = spawned("forms-no-cancel");
    bind_session(&shard);
    enter(&shard, BATTLE_STANCE);
    assert!(!shard
        .call(
            "gw_cancel_aura",
            &[&actor(WARRIOR), &BATTLE_STANCE.to_string()]
        )
        .status
        .success());
    const DISPEL_ALL: u32 = 5090990;
    insert_spell(
        &shard,
        DISPEL_ALL,
        "Dispel all",
        (0, 0, 0),
        &[(E_DISPEL, 1, 0, P_NONE)],
    );
    cast(&shard, DISPEL_ALL);
    assert_eq!(shapeshift_spells(&shard), vec![BATTLE_STANCE]);
    assert!(aura_spells(&shard).contains(&BATTLE_PASSIVE));
}

fn wolf(shard: &Standalone) -> String {
    support::wolves_beside(shard, WARRIOR, &[3.0]).remove(0)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn each_stance_scales_the_threat_its_passive_names() {
    const MARK: u32 = 5090991;
    let shard = spawned("forms-threat");
    shard.assert_sql("UPDATE game_world_entity SET godmode = true WHERE guid = 1");
    let wolf = wolf(&shard);
    insert_spell(
        &shard,
        MARK,
        "Threat mark",
        (0, 0, CAST_FLAG_ALWAYS_HIT),
        &[(A_FLAG, 0, 0, P_FLAG)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_spell SET is_negative = true, range_yd = 30 WHERE spell_id = {MARK}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET target = {T_TARGET_ENEMY} WHERE spell_id = {MARK}"
    ));
    let threat = || -> i64 {
        shard
            .query_rows(&format!(
                "SELECT threat FROM game_threat WHERE creature_guid = {wolf} AND source_guid = 1"
            ))
            .first()
            .map_or(0, |row| number(row, "threat"))
    };
    let mark = || {
        let before = threat();
        ready(&shard);
        shard.assert_call("debug_force_cast_at", &[WARRIOR, &MARK.to_string(), &wolf]);
        threat() - before
    };
    // vmangos adds level * 5 threat for a negative Aura; the stance passive scales it.
    assert_eq!(mark(), 100);
    for (stance, expected) in [
        (BATTLE_STANCE, 80),
        (DEFENSIVE_STANCE, 130),
        (BERSERKER_STANCE, 80),
    ] {
        enter(&shard, stance);
        assert_eq!(mark(), expected, "stance {stance}");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn defensive_and_berserker_stance_change_damage_taken_and_done() {
    let shard = spawned("forms-damage");
    let wolf = wolf(&shard);
    let taken = || -> i32 {
        shard.assert_call("debug_compute_swing", &[&wolf, WARRIOR]);
        number(
            &shard
                .query_rows("SELECT damage_taken_pct FROM game_debug_readout WHERE key = 'swing'")
                [0],
            "damage_taken_pct",
        )
    };
    let dealt_max = || -> u32 { number(&warrior(&shard), "sheet_dmg_max") };
    shard.assert_sql("UPDATE game_world_entity SET strength = 2000 WHERE guid = 1");
    enter(&shard, BATTLE_STANCE);
    shard.assert_call(
        "debug_shed_spell_auras",
        &[WARRIOR, &BATTLE_STANCE.to_string()],
    );
    let unshifted = dealt_max();
    assert!(
        unshifted >= 100,
        "a 1% step must be visible, got {unshifted}"
    );
    assert_eq!(taken(), 0);
    enter(&shard, BATTLE_STANCE);
    assert_eq!((taken(), dealt_max()), (0, unshifted));
    enter(&shard, DEFENSIVE_STANCE);
    assert_eq!(taken(), -10);
    let defensive = dealt_max() as f32;
    assert!(
        (defensive - unshifted as f32 * 0.9).abs() <= 1.0,
        "Defensive Stance deals 90% of {unshifted}, got {defensive}"
    );
    enter(&shard, BERSERKER_STANCE);
    assert_eq!((taken(), dealt_max()), (10, unshifted));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn berserker_stance_adds_three_percent_crit() {
    let shard = spawned("forms-crit");
    let crit = || -> u32 { number(&warrior(&shard), "sheet_crit_bp") };
    let unshifted = crit();
    enter(&shard, BATTLE_STANCE);
    assert_eq!(crit(), unshifted);
    enter(&shard, BERSERKER_STANCE);
    assert_eq!(crit(), unshifted + 300);
    enter(&shard, DEFENSIVE_STANCE);
    assert_eq!(crit(), unshifted);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn tactical_mastery_keeps_rage_through_a_stance_change() {
    let shard = spawned("forms-tactical-mastery");
    enter(&shard, BATTLE_STANCE);
    set_rage(&shard, 800);
    enter(&shard, DEFENSIVE_STANCE);
    assert_eq!(rage(&shard), 0);
    insert_spell(
        &shard,
        TACTICAL_MASTERY,
        "Tactical Mastery",
        (0, 0, 0),
        &[(A_FORM_POWER_RETAINED, 50, i32::from(RAGE), P_POWER_TYPE)],
    );
    cast(&shard, TACTICAL_MASTERY);
    set_rage(&shard, 800);
    enter(&shard, BERSERKER_STANCE);
    assert_eq!(rage(&shard), 50);
    set_rage(&shard, 30);
    enter(&shard, BATTLE_STANCE);
    assert_eq!(rage(&shard), 30);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_cast_gate_reads_the_spell_form_set() {
    const BATTLE_ONLY: u32 = 5090992;
    const NOT_IN_DEFENSIVE: u32 = 5090993;
    let shard = spawned("forms-cast-gate");
    for (spell, form_mask, form_exclude) in [
        (BATTLE_ONLY, 1u32 << 16, 0u32),
        (NOT_IN_DEFENSIVE, 0, 1 << 17),
    ] {
        insert_spell(&shard, spell, "Gated", (0, 0, 0), &[(A_FLAG, 0, 0, P_FLAG)]);
        shard.assert_sql(&format!(
            "UPDATE game_spell SET form_mask = {form_mask}, form_exclude = {form_exclude} \
             WHERE spell_id = {spell}"
        ));
    }
    let casts = |spell: u32| {
        ready(&shard);
        shard
            .call("debug_force_cast", &[WARRIOR, &spell.to_string()])
            .status
            .success()
    };
    assert!(
        !casts(BATTLE_ONLY),
        "a Battle Stance ability needs the stance"
    );
    assert!(casts(NOT_IN_DEFENSIVE));
    enter(&shard, BATTLE_STANCE);
    assert!(casts(BATTLE_ONLY));
    enter(&shard, DEFENSIVE_STANCE);
    assert!(!casts(BATTLE_ONLY));
    assert!(!casts(NOT_IN_DEFENSIVE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn leaving_a_stance_ends_the_auras_that_need_it() {
    const SWEEPING_STRIKES: u32 = 12292;
    const RETALIATION: u32 = 20230;
    let shard = spawned("forms-shape-lost");
    for (spell, attributes) in [(SWEEPING_STRIKES, 0x0004_0010), (RETALIATION, 0x0005_0010)] {
        insert_spell(
            &shard,
            spell,
            &format!("Battle ability {spell}"),
            (attributes, 0, 0),
            &[(A_FLAG, 0, 0, P_FLAG)],
        );
        shard.assert_sql(&format!(
            "UPDATE game_spell SET form_mask = {} WHERE spell_id = {spell}",
            1u32 << 16
        ));
    }
    enter(&shard, BATTLE_STANCE);
    cast(&shard, SWEEPING_STRIKES);
    cast(&shard, RETALIATION);
    enter(&shard, BERSERKER_STANCE);
    let held = aura_spells(&shard);
    assert!(!held.contains(&SWEEPING_STRIKES), "{held:?}");
    assert!(held.contains(&RETALIATION), "{held:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn publish_repair_turns_a_shown_stance_into_its_aura() {
    let shard = spawned("forms-repair");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_1 = {} WHERE guid = 1",
        18u32 << 16
    ));
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(shapeshift_spells(&shard), vec![DEFENSIVE_STANCE]);
    assert!(aura_spells(&shard).contains(&DEFENSIVE_PASSIVE));
    assert_eq!(client_form(&shard), 18);
}
