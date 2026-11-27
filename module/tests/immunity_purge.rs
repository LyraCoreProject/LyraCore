mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_FLAG, A_IMMUNITY, A_MOD_DAMAGE_TAKEN, M_FEAR, P_FLAG, P_MECHANIC, P_RAW,
    P_SCHOOL_MASK, T_SELF, T_TARGET_ENEMY,
};
use support::Standalone;

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const DURATION_MS: u32 = 600_000;
// vmangos SPELL_ATTR_EX_IMMUNITY_PURGES_EFFECT and SPELL_ATTR_NO_IMMUNITIES.
const PURGES_EFFECT: u32 = 0x8000;
const NO_IMMUNITIES: u32 = 0x2000_0000;
// Spell.dbc mechanics, charm, fear and sleep, as Will of the Forsaken 7744 names them.
const MECHANIC_CHARM: i32 = 1;
const MECHANIC_FEAR: u8 = 5;
const MECHANIC_SLEEP: i32 = 10;
// Source aura ids in an effect's p1: docs/schema.md#damage-shields and #flag-auras.
const AURA_MOD_FEAR: i32 = 7;
const AURA_MOD_SILENCE: i32 = 27;
const AURA_MECHANIC_IMMUNITY: i32 = 77;
const AURA_STATE_IMMUNITY: i32 = 38;
const AURA_SCHOOL_IMMUNITY: i32 = 39;
const AURA_DISPEL_IMMUNITY: i32 = 41;
const AURA_MECHANIC_IMMUNITY_MASK: i32 = 147;
const DISPEL_MAGIC: u8 = 1;
const DISPEL_CURSE: u8 = 2;
const SHADOW: u8 = 0x20;
const HOLY: u8 = 0x02;
const ALL_SCHOOLS: i32 = 0x7f;
const FEAR: u32 = 5_138_001;
const WARD: u32 = 5_138_002;
const SHIELDED_DEBUFF: u32 = 5_138_003;
const SHADOW_DEBUFF: u32 = 5_138_004;
const SHADOW_BUFF: u32 = 5_138_005;
const MAGIC_DEBUFF: u32 = 5_138_006;
const CURSE_DEBUFF: u32 = 5_138_007;
const SILENCE: u32 = 5_138_008;

type Effect = (u8, u8, i32, u8, i32);

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn will_of_the_forsaken_removes_a_fear_and_keeps_its_own_aura() {
    let mut standalone = Standalone::start("immunity-purge-will-of-the-forsaken");
    let wolf = stage(&mut standalone);
    insert_fear(&standalone);
    insert_will_of_the_forsaken(&standalone, PURGES_EFFECT);
    cast(&standalone, wolf, FEAR, PLAYER);
    assert_eq!(held(&standalone, FEAR), 1, "the fear lands");

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(
        held(&standalone, FEAR),
        0,
        "vmangos removes the fear it makes the unit immune to"
    );
    assert_eq!(
        held(&standalone, WARD),
        3,
        "the immunity keeps all three of its own effects"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_immunity_without_the_attribute_removes_nothing() {
    let mut standalone = Standalone::start("immunity-purge-no-attribute");
    let wolf = stage(&mut standalone);
    insert_will_of_the_forsaken(&standalone, 0);
    // A fear-mechanic debuff that leaves its holder free to cast.
    insert_spell(
        &standalone,
        SHIELDED_DEBUFF,
        &[("mechanic", &MECHANIC_FEAR)],
        &[(A_MOD_DAMAGE_TAKEN, T_TARGET_ENEMY, 0, P_RAW, 0)],
    );
    cast(&standalone, wolf, SHIELDED_DEBUFF, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, SHIELDED_DEBUFF), 1);
    assert_eq!(held(&standalone, WARD), 3);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_aura_whose_spell_has_no_immunities_survives_the_mechanic_purge() {
    let mut standalone = Standalone::start("immunity-purge-no-immunities");
    let wolf = stage(&mut standalone);
    insert_fear(&standalone);
    insert_will_of_the_forsaken(&standalone, PURGES_EFFECT);
    insert_spell(
        &standalone,
        SHIELDED_DEBUFF,
        &[("mechanic", &MECHANIC_FEAR), ("attributes", &NO_IMMUNITIES)],
        &[(A_MOD_DAMAGE_TAKEN, T_TARGET_ENEMY, 0, P_RAW, 0)],
    );
    cast(&standalone, wolf, FEAR, PLAYER);
    cast(&standalone, wolf, SHIELDED_DEBUFF, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, FEAR), 0);
    assert_eq!(held(&standalone, SHIELDED_DEBUFF), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mechanic_mask_immunity_removes_the_auras_of_the_mechanics_in_its_mask() {
    let mut standalone = Standalone::start("immunity-purge-mechanic-mask");
    let wolf = stage(&mut standalone);
    insert_fear(&standalone);
    insert_spell(
        &standalone,
        WARD,
        &[("attributes_ex", &PURGES_EFFECT)],
        &[(
            A_IMMUNITY,
            T_SELF,
            1 << (MECHANIC_FEAR - 1),
            P_MECHANIC,
            AURA_MECHANIC_IMMUNITY_MASK,
        )],
    );
    cast(&standalone, wolf, FEAR, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, FEAR), 0);
    assert_eq!(held(&standalone, WARD), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_state_immunity_removes_the_auras_of_its_aura_type_only() {
    let mut standalone = Standalone::start("immunity-purge-state");
    let wolf = stage(&mut standalone);
    insert_spell(
        &standalone,
        SILENCE,
        &[],
        &[(A_FLAG, T_TARGET_ENEMY, 0, P_FLAG, AURA_MOD_SILENCE)],
    );
    insert_spell(
        &standalone,
        SHIELDED_DEBUFF,
        &[],
        &[(A_MOD_DAMAGE_TAKEN, T_TARGET_ENEMY, 0, P_RAW, 0)],
    );
    // A state immunity does not let a feared caster act, so the purged state is a silence.
    insert_spell(
        &standalone,
        WARD,
        &[("attributes_ex", &PURGES_EFFECT)],
        &[(
            A_IMMUNITY,
            T_SELF,
            AURA_MOD_SILENCE,
            P_RAW,
            AURA_STATE_IMMUNITY,
        )],
    );
    cast(&standalone, wolf, SILENCE, PLAYER);
    cast(&standalone, wolf, SHIELDED_DEBUFF, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, SILENCE), 0);
    assert_eq!(held(&standalone, SHIELDED_DEBUFF), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dispel_immunity_removes_the_auras_of_its_dispel_type_only() {
    let mut standalone = Standalone::start("immunity-purge-dispel");
    let wolf = stage(&mut standalone);
    insert_spell(
        &standalone,
        MAGIC_DEBUFF,
        &[("dispel_type", &DISPEL_MAGIC)],
        &[(A_MOD_DAMAGE_TAKEN, T_TARGET_ENEMY, 0, P_RAW, 0)],
    );
    insert_spell(
        &standalone,
        CURSE_DEBUFF,
        &[("dispel_type", &DISPEL_CURSE)],
        &[(A_MOD_DAMAGE_TAKEN, T_TARGET_ENEMY, 0, P_RAW, 0)],
    );
    insert_spell(
        &standalone,
        WARD,
        &[("attributes_ex", &PURGES_EFFECT)],
        &[(
            A_FLAG,
            T_SELF,
            i32::from(DISPEL_MAGIC),
            P_FLAG,
            AURA_DISPEL_IMMUNITY,
        )],
    );
    cast(&standalone, wolf, MAGIC_DEBUFF, PLAYER);
    cast(&standalone, wolf, CURSE_DEBUFF, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, MAGIC_DEBUFF), 0);
    assert_eq!(held(&standalone, CURSE_DEBUFF), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_helpful_school_immunity_removes_harmful_auras_of_its_school_and_keeps_helpful_ones() {
    let mut standalone = Standalone::start("immunity-purge-school-helpful");
    let wolf = stage(&mut standalone);
    insert_school_fixtures(&standalone, false);
    cast(&standalone, wolf, SHADOW_DEBUFF, PLAYER);
    cast(&standalone, PLAYER, SHADOW_BUFF, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, SHADOW_DEBUFF), 0);
    assert_eq!(held(&standalone, SHADOW_BUFF), 1);
    assert_eq!(held(&standalone, WARD), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_harmful_school_immunity_removes_nothing() {
    let mut standalone = Standalone::start("immunity-purge-school-harmful");
    let wolf = stage(&mut standalone);
    insert_school_fixtures(&standalone, true);
    cast(&standalone, wolf, SHADOW_DEBUFF, PLAYER);

    cast(&standalone, PLAYER, WARD, PLAYER);

    assert_eq!(held(&standalone, WARD), 1);
    assert_eq!(held(&standalone, SHADOW_DEBUFF), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_feared_character_casts_will_of_the_forsaken_and_the_fear_is_gone() {
    let mut standalone = Standalone::start("immunity-purge-feared-cast");
    let wolf = stage(&mut standalone);
    insert_fear(&standalone);
    insert_will_of_the_forsaken(&standalone, PURGES_EFFECT);
    cast(&standalone, wolf, FEAR, PLAYER);
    assert_eq!(held(&standalone, FEAR), 1, "the fear lands");

    standalone.assert_call("debug_begin_cast", &["1", &WARD.to_string(), "1"]);
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);

    assert_eq!(held(&standalone, FEAR), 0);
    assert_eq!(held(&standalone, WARD), 3);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_feared_character_cannot_cast_an_immunity_that_does_not_cover_fear() {
    let mut standalone = Standalone::start("immunity-purge-feared-refused");
    let wolf = stage(&mut standalone);
    insert_fear(&standalone);
    insert_spell(
        &standalone,
        WARD,
        &[("attributes_ex", &PURGES_EFFECT)],
        &[(
            A_IMMUNITY,
            T_SELF,
            MECHANIC_CHARM,
            P_MECHANIC,
            AURA_MECHANIC_IMMUNITY,
        )],
    );
    cast(&standalone, wolf, FEAR, PLAYER);

    let output = standalone.call("debug_begin_cast", &["1", &WARD.to_string(), "1"]);

    assert!(
        !output.status.success(),
        "a feared caster cannot cast an immunity to charm"
    );
    assert_eq!(held(&standalone, FEAR), 1);
    assert_eq!(held(&standalone, WARD), 0);
}

fn insert_fear(standalone: &Standalone) {
    insert_spell(
        standalone,
        FEAR,
        &[("mechanic", &MECHANIC_FEAR)],
        &[(A_CONTROL, T_TARGET_ENEMY, M_FEAR, P_MECHANIC, AURA_MOD_FEAR)],
    );
}

// Will of the Forsaken 7744: mechanic immunity to charm, fear and sleep.
fn insert_will_of_the_forsaken(standalone: &Standalone, attributes_ex: u32) {
    let effects: Vec<Effect> = [MECHANIC_CHARM, i32::from(MECHANIC_FEAR), MECHANIC_SLEEP]
        .map(|mechanic| {
            (
                A_IMMUNITY,
                T_SELF,
                mechanic,
                P_MECHANIC,
                AURA_MECHANIC_IMMUNITY,
            )
        })
        .to_vec();
    insert_spell(
        standalone,
        WARD,
        &[("attributes_ex", &attributes_ex)],
        &effects,
    );
}

fn insert_school_fixtures(standalone: &Standalone, harmful_immunity: bool) {
    insert_spell(
        standalone,
        SHADOW_DEBUFF,
        &[("school_mask", &SHADOW)],
        &[(
            A_MOD_DAMAGE_TAKEN,
            T_TARGET_ENEMY,
            i32::from(SHADOW),
            P_SCHOOL_MASK,
            0,
        )],
    );
    insert_spell(
        standalone,
        SHADOW_BUFF,
        &[("school_mask", &SHADOW)],
        &[(
            A_MOD_DAMAGE_TAKEN,
            T_SELF,
            i32::from(SHADOW),
            P_SCHOOL_MASK,
            0,
        )],
    );
    insert_spell(
        standalone,
        WARD,
        &[
            ("attributes_ex", &PURGES_EFFECT),
            ("school_mask", &HOLY),
            ("is_negative", &harmful_immunity),
        ],
        &[(
            A_IMMUNITY,
            T_SELF,
            ALL_SCHOOLS,
            P_SCHOOL_MASK,
            AURA_SCHOOL_IMMUNITY,
        )],
    );
}

fn insert_spell(
    standalone: &Standalone,
    spell_id: u32,
    header: &[support::Field],
    effects: &[Effect],
) {
    let name = format!("Immunity purge fixture {spell_id}");
    let is_negative = effects.iter().any(|effect| effect.1 == T_TARGET_ENEMY);
    let mut spell_row: Vec<support::Field> = header.to_vec();
    let defaults: [support::Field; 6] = [
        ("spell_id", &spell_id),
        ("name", &name),
        ("range_yd", &30),
        ("duration_ms", &DURATION_MS),
        ("spell_level", &1),
        ("is_negative", &is_negative),
    ];
    spell_row.extend(
        defaults
            .into_iter()
            .filter(|(column, _)| !header.iter().any(|(set, _)| set == column)),
    );
    standalone.insert_row("game_spell", &spell_row);
    for (index, &(kind, target, p0, p0_kind, p1)) in effects.iter().enumerate() {
        let index = index as u8;
        let effect_id = packed_spell_effect_id(spell_id, index);
        standalone.insert_row(
            "game_spell_effect",
            &[
                ("id", &effect_id),
                ("spell_id", &spell_id),
                ("effect_index", &index),
                ("kind", &kind),
                ("target", &target),
                ("p_0", &p0),
                ("p_0_kind", &p0_kind),
                ("p_1", &p1),
            ],
        );
    }
}

fn cast(standalone: &Standalone, caster: u64, spell_id: u32, target: u64) {
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &caster.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
}

fn held(standalone: &Standalone, spell_id: u32) -> usize {
    standalone
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {PLAYER} AND spell_id = {spell_id}"
        ))
        .len()
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
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = 1, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    wolf
}
