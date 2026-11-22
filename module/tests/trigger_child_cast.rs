mod support;

use lyracore_package_delta::fixture_ids::{
    FIXTURE_MOUNT_SPELL, LEARN_APPRENTICE_RIDING_SPELL_ID, RIDING_TRAINER_ENTRY, TEST_WOLF_ENTRY,
};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::guid;
use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_STEALTH, CAST_FLAG_STEALTH_SAFE, E_HEAL, E_TRIGGER, M_STUN, P_MECHANIC, T_SELF,
    T_TARGET_ANY,
};
use support::{actor, number, Standalone};

const PLAYER: &str = "1";
const WOLF_GUID: u64 = guid::creature(TEST_WOLF_ENTRY, 1);
const PARENT: u32 = 5_098_401;
const CHILD: u32 = 5_098_402;
const STEALTH: u32 = 5_098_403;
const DEATH_RULE_ID: u64 = 5_098_404;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_cast_on_cooldown_still_applies() {
    let shard = stage("trigger-child-cooldown");
    spell(&shard, PARENT, &[]);
    spell(&shard, CHILD, &[("cooldown_ms", &60_000u32)]);
    trigger_effect(&shard, 0, T_SELF);
    heal_child(&shard);
    learn_parent(&shard);

    cast_parent(&shard, PLAYER);
    let healed_once = health(&shard);
    assert!(healed_once > 5000, "the first trigger child heals");
    assert_eq!(cooldowns(&shard, "1", CHILD), 1);

    cast_parent(&shard, PLAYER);
    assert!(health(&shard) > healed_once, "the child on cooldown heals");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dead_creatures_trigger_child_cast_resolves() {
    let shard = stage("trigger-child-dead-caster");
    shard.clear_schedule("game_melee_schedule");
    spell(&shard, PARENT, &[]);
    spell(&shard, CHILD, &[("cooldown_ms", &60_000u32)]);
    trigger_effect(&shard, 0, T_SELF);
    let rules = format!(
        "{DEATH_RULE_ID},death:always,100,4294967295,once,all,ordinary,any-posture,\
         cast:{PARENT}:self:0:1:0:0:0"
    );
    shard.assert_call(
        "debug_stage_eventai_revision_fixture",
        &[
            &WOLF_GUID.to_string(),
            PLAYER,
            &lyracore_shared::eventai::encode_definition(
                &format!("entry:{TEST_WOLF_ENTRY}"),
                &rules,
            ),
        ],
    );

    shard.assert_call("debug_kill_creature", &[PLAYER, &WOLF_GUID.to_string()]);
    assert_eq!(
        cooldowns(&shard, &WOLF_GUID.to_string(), CHILD),
        1,
        "the child of the wolf's death cast resolves and starts its cooldown"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_cast_out_of_its_range_still_applies() {
    let shard = stage("trigger-child-range");
    spell(&shard, PARENT, &[("range_yd", &40u32)]);
    spell(&shard, CHILD, &[("range_yd", &5u32)]);
    trigger_effect(&shard, 0, T_TARGET_ANY);
    heal_child(&shard);
    learn_parent(&shard);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[PLAYER, &TEST_WOLF_ENTRY.to_string(), "30"],
    );
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY} \
             AND guid != {WOLF_GUID}"
        ))
        .pop()
        .expect("the spawned wolf must exist")["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false WHERE guid = {wolf}"
    ));

    cast_parent(&shard, &wolf);
    assert!(health(&shard) > 5000, "the child heals its caster");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_cast_applies_after_its_parent_spends_all_power() {
    let shard = stage("trigger-child-power");
    let power_type = number::<u32>(
        &shard.query_rows("SELECT unit_bytes_0 FROM game_world_entity WHERE guid = 1")[0],
        "unit_bytes_0",
    ) >> 24;
    spell(
        &shard,
        PARENT,
        &[("power_type", &power_type), ("cost", &100u32)],
    );
    spell(
        &shard,
        CHILD,
        &[("power_type", &power_type), ("cost", &50u32)],
    );
    trigger_effect(&shard, 0, T_SELF);
    heal_child(&shard);
    learn_parent(&shard);
    shard.assert_call("debug_set_power", &[PLAYER, "100"]);

    cast_parent(&shard, PLAYER);
    assert!(health(&shard) > 5000, "the child heals without power");
    assert_eq!(power(&shard), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_cast_applies_after_its_parent_stuns_the_caster() {
    let shard = stage("trigger-child-stunned");
    spell(&shard, PARENT, &[("duration_ms", &60_000u32)]);
    spell(&shard, CHILD, &[]);
    effect(
        &shard,
        PARENT,
        0,
        &[
            ("kind", &A_CONTROL),
            ("target", &T_SELF),
            ("p_0", &M_STUN),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
    trigger_effect(&shard, 1, T_SELF);
    heal_child(&shard);
    learn_parent(&shard);

    cast_parent(&shard, PLAYER);
    assert_eq!(auras(&shard, PARENT), 1, "the parent stuns its caster");
    assert!(health(&shard) > 5000, "the stunned caster's child heals");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mount_spells_trigger_child_cast_leaves_its_caster_mounted() {
    let shard = stage("trigger-child-mount");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_set_level", &[PLAYER, "20"]);
    shard.assert_call("debug_set_money", &[PLAYER, "1000"]);
    train_riding(&shard);
    shard.assert_sql(
        "UPDATE game_world_entity SET health = 5000, max_health = 10000 WHERE guid = 1",
    );
    spell(&shard, CHILD, &[]);
    heal_child(&shard);
    effect(
        &shard,
        FIXTURE_MOUNT_SPELL,
        2,
        &[
            ("kind", &E_TRIGGER),
            ("target", &T_SELF),
            ("trigger_spell", &CHILD),
        ],
    );
    shard.assert_call(
        "debug_learn_spell",
        &[PLAYER, &FIXTURE_MOUNT_SPELL.to_string()],
    );

    shard.assert_call(
        "debug_cast_spell",
        &[PLAYER, &FIXTURE_MOUNT_SPELL.to_string(), "0"],
    );
    assert!(health(&shard) > 5000, "the mount's child heals");
    assert_eq!(
        auras(&shard, FIXTURE_MOUNT_SPELL),
        2,
        "the rider stays mounted"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stealth_safe_spells_trigger_child_cast_leaves_its_caster_stealthed() {
    let shard = stage("trigger-child-stealth");
    spell(&shard, STEALTH, &[("duration_ms", &600_000u32)]);
    effect(
        &shard,
        STEALTH,
        0,
        &[("kind", &A_STEALTH), ("target", &T_SELF)],
    );
    spell(&shard, PARENT, &[("cast_flags", &CAST_FLAG_STEALTH_SAFE)]);
    spell(&shard, CHILD, &[]);
    trigger_effect(&shard, 0, T_SELF);
    heal_child(&shard);
    learn_parent(&shard);
    shard.assert_call("debug_force_cast", &[PLAYER, &STEALTH.to_string()]);
    assert_eq!(auras(&shard, STEALTH), 1);

    cast_parent(&shard, PLAYER);
    assert!(health(&shard) > 5000, "the child heals");
    assert_eq!(auras(&shard, STEALTH), 1, "the caster stays stealthed");
}

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.clear_schedule("game_creature_move_schedule");
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[PLAYER]);
    shard.assert_sql(
        "UPDATE game_world_entity SET health = 5000, max_health = 10000 WHERE guid = 1",
    );
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard
}

fn spell(shard: &Standalone, spell_id: u32, extra: &[(&str, &dyn std::fmt::Display)]) {
    let name = format!("Trigger child fixture {spell_id}");
    let mut fields: Vec<(&str, &dyn std::fmt::Display)> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("gcd_ms", &0u32),
        ("spell_level", &1u32),
    ];
    fields.extend_from_slice(extra);
    shard.insert_row("game_spell", &fields);
}

fn effect(shard: &Standalone, spell_id: u32, index: u8, extra: &[(&str, &dyn std::fmt::Display)]) {
    let id = packed_spell_effect_id(spell_id, index);
    let mut fields: Vec<(&str, &dyn std::fmt::Display)> = vec![
        ("id", &id),
        ("spell_id", &spell_id),
        ("effect_index", &index),
    ];
    fields.extend_from_slice(extra);
    shard.insert_row("game_spell_effect", &fields);
}

fn trigger_effect(shard: &Standalone, index: u8, target: u8) {
    effect(
        shard,
        PARENT,
        index,
        &[
            ("kind", &E_TRIGGER),
            ("target", &target),
            ("trigger_spell", &CHILD),
        ],
    );
}

fn heal_child(shard: &Standalone) {
    effect(
        shard,
        CHILD,
        0,
        &[
            ("kind", &E_HEAL),
            ("base_points", &100),
            ("target", &T_SELF),
        ],
    );
}

fn learn_parent(shard: &Standalone) {
    shard.assert_call("debug_learn_spell", &[PLAYER, &PARENT.to_string()]);
}

fn cast_parent(shard: &Standalone, target: &str) {
    shard.assert_call("debug_cast_spell", &[PLAYER, &PARENT.to_string(), target]);
}

fn train_riding(shard: &Standalone) {
    shard.assert_call(
        "debug_spawn_at_feet",
        &[PLAYER, &RIDING_TRAINER_ENTRY.to_string(), "5"],
    );
    let trainer = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {RIDING_TRAINER_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_call(
        "gw_trainer_buy",
        &[
            &actor(PLAYER),
            &trainer,
            &LEARN_APPRENTICE_RIDING_SPELL_ID.to_string(),
        ],
    );
}

fn health(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1")[0],
        "health",
    )
}

fn power(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT power FROM game_world_entity WHERE guid = 1")[0],
        "power",
    )
}

fn auras(shard: &Standalone, spell_id: u32) -> usize {
    shard
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = {spell_id}"
        ))
        .len()
}

fn cooldowns(shard: &Standalone, caster_guid: &str, spell_id: u32) -> usize {
    shard
        .query_rows(&format!(
            "SELECT id FROM game_spell_cd WHERE caster_guid = {caster_guid} \
             AND spell_id = {spell_id}"
        ))
        .len()
}
