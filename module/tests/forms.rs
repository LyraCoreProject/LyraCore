mod support;

use std::collections::BTreeSet;

use lyracore_package_delta::fixture_ids::RECRUITS_TUNIC;
use lyracore_package_delta::{packed_form_spell_id, packed_spell_form_id};
use lyracore_shared::spell::taxonomy::*;
use serde_json::json;
use support::{number, ReducerConnection, Row, Standalone};

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

/// Writes a spell's Form Set, keeping its verbatim DBC masks in step.
fn require_forms(shard: &Standalone, spell: u32, required: &[u32], excluded: &[u32]) {
    let mask = |forms: &[u32]| forms.iter().fold(0u32, |mask, form| mask | 1 << (form - 1));
    shard.assert_sql(&format!(
        "UPDATE game_spell SET form_mask = {}, form_exclude = {} WHERE spell_id = {spell}",
        mask(required),
        mask(excluded)
    ));
    for (forms, excluded) in [(required, false), (excluded, true)] {
        for &form in forms {
            shard.insert_row(
                "game_spell_form",
                &[
                    ("id", &packed_spell_form_id(spell, form)),
                    ("spell_id", &spell),
                    ("form_id", &form),
                    ("excluded", &excluded),
                ],
            );
        }
    }
}

fn shard_without_forms(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard
}

fn fixture(name: &str) -> Standalone {
    let shard = shard_without_forms(name);
    shard.assert_call("debug_seed_warrior_forms", &[]);
    shard
}

/// A level 20 warrior body that left the Battle Stance every loaded warrior holds.
fn spawned(name: &str) -> Standalone {
    let shard = fixture(name);
    shard.assert_call("debug_spawn_player_entity", &[WARRIOR]);
    shard.assert_call("debug_set_level", &[WARRIOR, "20"]);
    shard.assert_call(
        "debug_shed_spell_auras",
        &[WARRIOR, &BATTLE_STANCE.to_string()],
    );
    shard
}

/// Enters the world through a native World Session, as a client login does.
fn log_in(shard: &Standalone, server_seed: u32) -> ReducerConnection {
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([server_seed]))
        .unwrap();
    wire.call("debug_native_enter_world", json!([1])).unwrap();
    wire
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
        set_rage(&shard, 600);
        enter(&shard, spell);
        assert_eq!(rage(&shard), expected_rage, "recasting {form:?}");

        for other in forms
            .iter()
            .filter(|other| other["form_id"] != form["form_id"])
        {
            enter(&shard, number(other, "spell_id"));
            let held = aura_spells(&shard);
            assert!(held.is_disjoint(&passives), "{other:?} holds {held:?}");
            assert!(
                passives_of(&other["form_id"]).is_subset(&held),
                "{other:?} after {form:?} holds {held:?}"
            );
        }
        enter(&shard, spell);
        assert!(passives.is_subset(&aura_spells(&shard)), "back to {form:?}");

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
    let _wire = log_in(&shard, 42);
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
    let _first = log_in(&shard, 42);
    enter(&shard, DEFENSIVE_STANCE);
    shard.assert_call("debug_logout_character", &[WARRIOR]);
    let _second = log_in(&shard, 43);
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

// Dispel is not tested: `dispel_target` skips every spell with dispel type 0, as the stances are.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stance_cannot_be_cancelled() {
    const BUFF: u32 = 5090996;
    let shard = fixture("forms-no-cancel");
    let mut wire = log_in(&shard, 42);
    insert_spell(
        &shard,
        BUFF,
        "Cancellable buff",
        (0, 0, 0),
        &[(A_FLAG, 0, 0, P_FLAG)],
    );
    cast(&shard, BUFF);
    let cancel = |wire: &mut ReducerConnection, spell: u32| {
        let _ = wire.call(
            "native_world_receive",
            json!([0x136u32, spell.to_le_bytes().to_vec()]),
        );
    };
    cancel(&mut wire, BUFF);
    assert!(
        !aura_spells(&shard).contains(&BUFF),
        "the request cancels an ordinary buff"
    );
    cancel(&mut wire, BATTLE_STANCE);
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
    for (spell, required, excluded) in [
        (BATTLE_ONLY, &[17][..], &[][..]),
        (NOT_IN_DEFENSIVE, &[][..], &[18][..]),
    ] {
        insert_spell(&shard, spell, "Gated", (0, 0, 0), &[(A_FLAG, 0, 0, P_FLAG)]);
        require_forms(&shard, spell, required, excluded);
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

const PACKAGE_FORM: u32 = 18_000_001;
const PACKAGE_SHIFT: u32 = 6_000_001;
const PACKAGE_PASSIVE: u32 = 6_000_002;
const PACKAGE_ABILITY: u32 = 6_000_003;
const PACKAGE_CLIENT_FORM: u32 = 5;

fn package_spell_claims(spell: u32, name: &str, attributes: u32, effect: (u8, i32, u8)) -> String {
    let (kind, parameter, parameter_kind) = effect;
    let u8_ = |value: u8| json!({"type": "u8", "value": value});
    let u32_ = |value: u32| json!({"type": "u32", "value": value});
    let i32_ = |value: i32| json!({"type": "i32", "value": value});
    let spell_row = json!({
        "name": {"type": "string", "value": name},
        "power_type": u8_(RAGE), "cost": u32_(0), "cast_time_ms": u32_(0), "gcd_ms": u32_(0),
        "cooldown_ms": u32_(0), "range_yd": u32_(0), "duration_ms": u32_(PERMANENT_MS as u32),
        "school_mask": u8_(1), "dispel_type": u8_(0), "mechanic": u8_(0), "max_stacks": u8_(0),
        "aura_interrupt": {"type": "u16", "value": 0}, "attributes": u32_(attributes),
        "spell_level": u8_(1), "max_level": u8_(0),
        "is_negative": {"type": "bool", "value": false}, "cast_flags": u32_(0),
        "stances": u8_(0), "family_name": u8_(0), "family_flags": {"type": "u64", "value": "0"},
        "proc_flags": u32_(0), "proc_chance": u8_(0), "proc_charges": u8_(0),
    });
    let effect_row = json!({
        "kind": u8_(kind), "base_points": i32_(0), "die_sides": i32_(0),
        "per_level": {"type": "f32", "value": 0.0}, "period_ms": u32_(0), "target": u8_(T_SELF),
        "radius_yd": {"type": "f32", "value": 0.0}, "chain_targets": u8_(0),
        "trigger_spell": u32_(0), "effect_mechanic": u8_(0), "p0": i32_(parameter),
        "p0_kind": u8_(parameter_kind), "p1": i32_(0), "script_id": u32_(0),
        "enters_combat": {"type": "bool", "value": false},
    });
    [
        json!({"table": "game_spell", "key": {"spell_id": spell}, "operation": "insert",
               "fields": spell_row}),
        json!({"table": "game_spell_effect", "key": {"spell_id": spell, "effect_index": 0},
               "operation": "insert", "fields": effect_row}),
    ]
    .map(|claim| claim.to_string())
    .join(",")
}

fn apply_package_form(shard: &Standalone) {
    let form = json!({"table": "game_form", "key": {"form_id": PACKAGE_FORM},
    "operation": "insert", "fields": {
        "name": {"type": "string", "value": "Kindled Aspect"},
        "spell_id": {"type": "u32", "value": PACKAGE_SHIFT},
        "acts_as_shapeshifted": {"type": "bool", "value": true},
        "power_type": {"type": "u8", "value": RAGE},
        "power_on_entry": {"type": "u8", "value": 1},
        "default_for_class": {"type": "u8", "value": 0},
        "client_form": {"type": "u8", "value": PACKAGE_CLIENT_FORM},
    }});
    let passive = json!({"table": "game_form_spell",
        "key": {"form_id": PACKAGE_FORM, "spell_id": PACKAGE_PASSIVE},
        "operation": "insert", "fields": {}});
    let form_set = json!({"table": "game_spell_form",
        "key": {"spell_id": PACKAGE_ABILITY, "form_id": PACKAGE_FORM},
        "operation": "insert", "fields": {"excluded": {"type": "bool", "value": false}}});
    let claims = [
        package_spell_claims(
            PACKAGE_SHIFT,
            "Kindled Aspect",
            0,
            (A_SHAPESHIFT, PACKAGE_FORM as i32, P_FORM),
        ),
        package_spell_claims(
            PACKAGE_PASSIVE,
            "Kindled Aspect Passive",
            0x0005_00D0,
            (A_FLAG, 0, P_FLAG),
        ),
        package_spell_claims(PACKAGE_ABILITY, "Kindled Strike", 0, (A_FLAG, 0, P_FLAG)),
        form.to_string(),
        passive.to_string(),
        form_set.to_string(),
    ]
    .join(",");
    let artifact = format!(
        r#"{{"version":1,"package":"example.aspect","source_hash":"{}","claims":[{claims}]}}"#,
        "0".repeat(64)
    );
    shard.assert_call(
        "apply_package_deltas",
        &[&json!("spell").to_string(), &json!(artifact).to_string()],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_form_projects_its_passives_and_gates_its_spells() {
    let shard = spawned("forms-package");
    apply_package_form(&shard);
    let casts = |spell: u32| {
        ready(&shard);
        shard
            .call("debug_force_cast", &[WARRIOR, &spell.to_string()])
            .status
            .success()
    };
    assert!(
        !casts(PACKAGE_ABILITY),
        "the ability needs the Package Form"
    );

    set_rage(&shard, 600);
    enter(&shard, PACKAGE_SHIFT);
    assert_eq!(client_form(&shard), PACKAGE_CLIENT_FORM);
    assert!(aura_spells(&shard).contains(&PACKAGE_PASSIVE));
    assert_eq!(rage(&shard), 0, "the Form's power rule zeroes rage");
    assert!(casts(PACKAGE_ABILITY));

    shard.assert_call(
        "debug_shed_spell_auras",
        &[WARRIOR, &PACKAGE_SHIFT.to_string()],
    );
    assert!(shapeshift_spells(&shard).is_empty());
    assert_eq!(client_form(&shard), 0);
    assert!(!aura_spells(&shard).contains(&PACKAGE_PASSIVE));
    assert!(!casts(PACKAGE_ABILITY));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_forms_fallback_byte_never_stands_for_a_druid_form() {
    const BEAR_FORM: u32 = 5487;
    const MAUL: u32 = 6807;
    let shard = fixture("forms-package-fallback");
    shard.assert_sql("UPDATE game_character SET class = 11 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &[WARRIOR]);
    shard.assert_call("debug_set_level", &[WARRIOR, "20"]);
    apply_package_form(&shard);
    insert_spell(
        &shard,
        BEAR_FORM,
        "Bear Form",
        (0, 0, 0),
        &[(E_SET_STANCE, 0, i32::from(STANCE_BEAR), P_NONE)],
    );
    insert_spell(&shard, MAUL, "Maul", (0, 0, 0), &[(A_FLAG, 0, 0, P_FLAG)]);
    require_forms(&shard, MAUL, &[5], &[]);
    cast(&shard, BEAR_FORM);
    assert_eq!(client_form(&shard), PACKAGE_CLIENT_FORM);

    ready(&shard);
    let maul = shard.call("debug_force_cast", &[WARRIOR, &MAUL.to_string()]);
    assert!(maul.status.success(), "Bear Form allows Maul");
    shard.assert_call("debug_repair_after_publish", &[]);
    assert!(shapeshift_spells(&shard).is_empty());
    assert_eq!(client_form(&shard), PACKAGE_CLIENT_FORM);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_form_change_never_strips_an_aura_another_unit_cast() {
    const TEST_POLY: u32 = 50023;
    let shard = spawned("forms-foreign-aura");
    let wolf = wolf(&shard);
    shard.assert_call(
        "debug_force_cast_at",
        &[&wolf, &TEST_POLY.to_string(), WARRIOR],
    );
    shard.insert_row(
        "game_form_spell",
        &[
            ("id", &packed_form_spell_id(18, TEST_POLY)),
            ("form_id", &18u32),
            ("spell_id", &TEST_POLY),
        ],
    );

    shard.assert_call("debug_repair_after_publish", &[]);

    assert_eq!(shapeshift_spells(&shard), vec![BATTLE_STANCE]);
    assert!(aura_spells(&shard).contains(&TEST_POLY));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_learned_passive_a_form_also_grants_survives_leaving_that_form() {
    const SHARPENED: u32 = 16942;
    let shard = spawned("forms-learned-passive");
    insert_spell(
        &shard,
        SHARPENED,
        "Sharpened passive",
        (0x1D0, 0, 0),
        &[(A_MOD_COMBAT, 3, i32::from(COMBAT_THREAT), P_COMBAT_FIELD)],
    );
    shard.insert_row(
        "game_form_spell",
        &[
            ("id", &packed_form_spell_id(18, SHARPENED)),
            ("form_id", &18u32),
            ("spell_id", &SHARPENED),
        ],
    );
    shard.assert_call("debug_learn_spell", &[WARRIOR, &SHARPENED.to_string()]);
    for stance in [DEFENSIVE_STANCE, BATTLE_STANCE, BERSERKER_STANCE] {
        enter(&shard, stance);
        assert!(aura_spells(&shard).contains(&SHARPENED), "stance {stance}");
    }
    shard.assert_call("debug_repair_after_publish", &[]);
    assert!(aura_spells(&shard).contains(&SHARPENED));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn removing_a_package_form_takes_its_rows_and_repair_restores_the_unit() {
    let shard = spawned("forms-package-removal");
    apply_package_form(&shard);
    enter(&shard, PACKAGE_SHIFT);

    shard.assert_call(
        "apply_package_deltas",
        &[&json!("spell").to_string(), &json!("").to_string()],
    );
    for table in ["game_form", "game_form_spell", "game_spell_form"] {
        let rows = shard.query_rows(&format!(
            "SELECT * FROM {table} WHERE form_id = {PACKAGE_FORM}"
        ));
        assert!(rows.is_empty(), "{table}: {rows:?}");
    }

    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(shapeshift_spells(&shard), vec![BATTLE_STANCE]);
    assert!(!aura_spells(&shard).contains(&PACKAGE_PASSIVE));
    assert_eq!(client_form(&shard), 17);
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
        require_forms(&shard, spell, &[17], &[]);
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn defiance_comes_and_goes_with_defensive_stance() {
    const DEFIANCE: u32 = 12303;
    let shard = spawned("forms-defiance");
    insert_spell(
        &shard,
        DEFIANCE,
        "Defiance",
        (0x1D0, 0, 0),
        &[(A_MOD_COMBAT, 3, i32::from(COMBAT_THREAT), P_COMBAT_FIELD)],
    );
    require_forms(&shard, DEFIANCE, &[18], &[]);
    shard.assert_call("debug_learn_spell", &[WARRIOR, &DEFIANCE.to_string()]);
    for (stance, held) in [
        (BATTLE_STANCE, false),
        (DEFENSIVE_STANCE, true),
        (BATTLE_STANCE, false),
        (DEFENSIVE_STANCE, true),
    ] {
        enter(&shard, stance);
        assert_eq!(
            aura_spells(&shard).contains(&DEFIANCE),
            held,
            "stance {stance}"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_form_passive_that_needs_its_form_survives_the_switch_into_it() {
    let shard = spawned("forms-switch-order");
    shard.assert_sql(&format!(
        "UPDATE game_spell SET attributes = {} WHERE spell_id = {BATTLE_PASSIVE}",
        0x0004_00D0
    ));
    require_forms(&shard, BATTLE_PASSIVE, &[17], &[]);
    enter(&shard, DEFENSIVE_STANCE);
    enter(&shard, BATTLE_STANCE);
    let id = |spell: u32| -> u64 {
        number(
            &shard.query_rows(&format!(
                "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = {spell}"
            ))[0],
            "id",
        )
    };
    let newest: u64 = shard
        .query_rows("SELECT id FROM game_aura")
        .iter()
        .map(|row| number(row, "id"))
        .max()
        .unwrap();
    assert_eq!(
        (id(BATTLE_PASSIVE), newest),
        (id(BATTLE_STANCE) + 1, id(BATTLE_STANCE) + 1),
        "the passive is applied once, after the old Form's Auras are gone"
    );
}

const BERSERK: u32 = 23397;

/// Nefarian's warrior call as the spell import writes it.
fn install_berserk(shard: &Standalone) {
    insert_spell(
        shard,
        BERSERK,
        "Berserk",
        (0, 0, CAST_FLAG_LOCKS_FORM),
        &[(A_SHAPESHIFT, 0, 19, P_FORM)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET trigger_spell = {BERSERKER_STANCE} WHERE spell_id = {BERSERK}"
    ));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn nefarians_berserk_caps_rage_and_ends_in_berserker_stance() {
    let shard = spawned("forms-berserk");
    install_berserk(&shard);
    enter(&shard, BERSERKER_STANCE);
    set_rage(&shard, 600);
    enter(&shard, BERSERK);
    assert_eq!(
        rage(&shard),
        0,
        "Berserk applies the Form's power rule again"
    );
    assert_eq!(client_form(&shard), 19);
    shard.assert_call("debug_shed_spell_auras", &[WARRIOR, &BERSERK.to_string()]);
    assert_eq!(shapeshift_spells(&shard), vec![BERSERKER_STANCE]);
    assert!(aura_spells(&shard).contains(&BERSERKER_PASSIVE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_without_a_stance_cannot_use_a_stance_ability() {
    const SHIELD_BASH: u32 = 5090994;
    let shard = spawned("forms-creature-gate");
    let wolf = wolf(&shard);
    insert_spell(
        &shard,
        SHIELD_BASH,
        "Shield Bash",
        (0, 0, 0),
        &[(A_FLAG, 0, 0, P_FLAG)],
    );
    require_forms(&shard, SHIELD_BASH, &[17, 18], &[]);
    let casts = |spell: u32| {
        shard.assert_sql(&format!(
            "DELETE FROM game_spell_cooldown WHERE caster_guid = {wolf}"
        ));
        shard.assert_sql(&format!(
            "DELETE FROM game_spell_cd WHERE caster_guid = {wolf}"
        ));
        shard
            .call("debug_force_cast", &[&wolf, &spell.to_string()])
            .status
            .success()
    };
    assert!(
        !casts(SHIELD_BASH),
        "vmangos GetErrorAtShapeshiftedCast has no player exception"
    );
    assert!(casts(BATTLE_STANCE));
    assert!(casts(SHIELD_BASH));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_shard_without_forms_keeps_the_battle_stance_byte() {
    let shard = shard_without_forms("forms-before-reimport");
    let _wire = log_in(&shard, 42);
    assert!(shapeshift_spells(&shard).is_empty());
    assert_eq!(client_form(&shard), 17);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn berserk_locks_the_warrior_out_of_other_stances() {
    let shard = spawned("forms-berserk-lock");
    install_berserk(&shard);
    enter(&shard, BERSERK);
    ready(&shard);
    assert!(!shard
        .call("debug_force_cast", &[WARRIOR, &BATTLE_STANCE.to_string()])
        .status
        .success());
    assert_eq!(shapeshift_spells(&shard), vec![BERSERK]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn recasting_a_stance_ends_the_auras_that_need_a_form() {
    const SWEEPING_STRIKES: u32 = 12292;
    let shard = spawned("forms-recast");
    insert_spell(
        &shard,
        SWEEPING_STRIKES,
        "Sweeping Strikes",
        (0x0004_0010, 0, 0),
        &[(A_FLAG, 0, 0, P_FLAG)],
    );
    require_forms(&shard, SWEEPING_STRIKES, &[17], &[]);
    enter(&shard, BATTLE_STANCE);
    cast(&shard, SWEEPING_STRIKES);
    assert!(aura_spells(&shard).contains(&SWEEPING_STRIKES));
    enter(&shard, BATTLE_STANCE);
    assert!(!aura_spells(&shard).contains(&SWEEPING_STRIKES));
    assert!(aura_spells(&shard).contains(&BATTLE_PASSIVE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_warrior_body_built_without_login_holds_battle_stance() {
    let shard = fixture("forms-body");
    shard.assert_call("debug_spawn_player_entity", &[WARRIOR]);
    assert_eq!(shapeshift_spells(&shard), vec![BATTLE_STANCE]);
    assert!(aura_spells(&shard).contains(&BATTLE_PASSIVE));
    assert_eq!(client_form(&shard), 17);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_equipment_aura_bound_to_a_form_stays_with_the_equipment() {
    const GUARD: u32 = 5090997;
    let shard = spawned("forms-equip-aura");
    insert_spell(
        &shard,
        GUARD,
        "Defensive equip power",
        (0xC0, 0, 0),
        &[(
            A_MOD_COMBAT,
            10,
            i32::from(COMBAT_ATTACK_POWER),
            P_COMBAT_FIELD,
        )],
    );
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET equip_supported = true WHERE spell_id = {GUARD}"
    ));
    require_forms(&shard, GUARD, &[18], &[]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET spellid_1 = {GUARD}, spelltrigger_1 = 1 WHERE entry = {RECRUITS_TUNIC}"
    ));
    shard.assert_call(
        "debug_grant_item",
        &[WARRIOR, &RECRUITS_TUNIC.to_string(), "1"],
    );
    let slot = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = {RECRUITS_TUNIC}"
    ))[0]["slot"]
        .clone();
    shard.assert_call("debug_equip_item", &[WARRIOR, &slot]);
    enter(&shard, DEFENSIVE_STANCE);
    let aura = || {
        shard.query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = {GUARD}"
        ))
    };
    let held = aura();
    assert_eq!(
        held.len(),
        1,
        "the equipment applies its Aura in Defensive Stance"
    );
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        aura(),
        held,
        "the Form projection leaves the equipment's Aura alone"
    );
}

const SHARPENED_CLAWS: u32 = 16942;

/// A druid in Cat Form whose Feral Combat tree offers Sharpened Claws, a passive whose Form Set
/// names only druid forms, which have no Form rows yet.
fn druid_in_cat_form(name: &str) -> Standalone {
    const CAT_FORM: u32 = 768;
    const FERAL_COMBAT: u32 = 281;
    let shard = fixture(name);
    shard.assert_sql("UPDATE game_character SET class = 11 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &[WARRIOR]);
    shard.assert_call("debug_set_level", &[WARRIOR, "20"]);
    insert_spell(
        &shard,
        CAT_FORM,
        "Cat Form",
        (0, 0, 0),
        &[(E_SET_STANCE, 0, 4, P_NONE)],
    );
    cast(&shard, CAT_FORM);
    assert_eq!(client_form(&shard), 1);
    insert_spell(
        &shard,
        SHARPENED_CLAWS,
        "Sharpened Claws",
        (0x1D0, 0, 0),
        &[(A_MOD_COMBAT, 2, i32::from(COMBAT_CRIT), P_COMBAT_PERCENT)],
    );
    require_forms(&shard, SHARPENED_CLAWS, &[1, 5, 8], &[]);
    shard.insert_row(
        "game_talent_tab",
        &[
            ("tab_id", &FERAL_COMBAT),
            ("name", &"Feral Combat"),
            ("class_mask", &(1u32 << 10)),
            ("race_mask", &0u32),
            ("order_index", &1u8),
        ],
    );
    shard.insert_row(
        "game_talent",
        &[
            ("talent_id", &SHARPENED_CLAWS),
            ("name", &"Sharpened Claws"),
            ("tree_id", &0u8),
            ("tier", &0u8),
            ("column", &0u8),
            ("max_rank", &1u8),
            ("required_talent_id", &0u32),
            ("required_points_in_tree", &0u32),
            ("spell_id", &SHARPENED_CLAWS),
            ("tab_id", &FERAL_COMBAT),
        ],
    );
    shard
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_druid_learns_a_talent_passive_bound_to_a_druid_form() {
    let shard = druid_in_cat_form("forms-druid-talent");
    shard.assert_call(
        "debug_learn_talent",
        &[WARRIOR, &SHARPENED_CLAWS.to_string()],
    );
    assert!(aura_spells(&shard).contains(&SHARPENED_CLAWS));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_druid_keeps_a_talent_passive_bound_to_a_druid_form_across_world_entry() {
    let shard = druid_in_cat_form("forms-druid-relog");
    shard.assert_call(
        "debug_learn_talent",
        &[WARRIOR, &SHARPENED_CLAWS.to_string()],
    );
    shard.assert_call("debug_logout_character", &[WARRIOR]);
    let _wire = log_in(&shard, 42);
    assert!(aura_spells(&shard).contains(&SHARPENED_CLAWS));
}
