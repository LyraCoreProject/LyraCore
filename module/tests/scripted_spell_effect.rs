mod support;

use lyracore_shared::spell::taxonomy::{E_ENERGIZE, E_SCRIPTED, T_SELF, T_TARGET_ANY};

use std::collections::BTreeMap;

use lyracore_shared::constants::unit_flags;
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const TEST_WOLF_ENTRY: u32 = 51000;

const E_HEAL_SCRIPT_SPELL: u32 = 50_900;
const E_NOOP_SCRIPT_ID_ZERO_SPELL: u32 = 50_901;
const E_GATE_SPELL: u32 = 50_902;
const E_PARTIAL_FAILURE_SPELL: u32 = 50_903;

const HEAL_SCRIPT: u32 = 100_900;
const GATE_SCRIPT: u32 = 100_902;
const FAILING_SCRIPT: u32 = 100_903;

const HEAL_SCRIPT_COST: u32 = 30;
const GATE_SPELL_COST: u32 = 40;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_enabled_scripted_effect_heals_the_resolved_target_and_credits_xp_to_the_caster() {
    let mut standalone = Standalone::start("scripted-effect-heal");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    prevent_scheduled_regeneration(&standalone, PLAYER);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "1000"]);

    insert_spell(
        &standalone,
        E_HEAL_SCRIPT_SPELL,
        "Fixture Scripted Heal",
        HEAL_SCRIPT_COST,
    );
    insert_effect(
        &standalone,
        E_HEAL_SCRIPT_SPELL,
        0,
        u32::from(E_SCRIPTED),
        0,
        u32::from(T_TARGET_ANY),
        HEAL_SCRIPT,
    );
    insert_script(
        &standalone,
        HEAL_SCRIPT,
        "fixture.spellscript.heal",
        true,
        "heal(event.target, 15)\ngrant_xp(event.actor, 7)",
    );

    let wolf = spawn_wolf(&standalone);
    prevent_scheduled_regeneration(&standalone, wolf);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 50, max_health = 1000 WHERE guid = {wolf}"
    ));
    let xp_before = xp(&standalone, PLAYER);

    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &E_HEAL_SCRIPT_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );

    assert_eq!(
        health(&standalone, wolf),
        65,
        "the script's heal(event.target, 15) must land on the effect's resolved unit"
    );
    assert_eq!(
        xp(&standalone, PLAYER) - xp_before,
        7,
        "the script's grant_xp(event.actor, 7) must credit the caster"
    );
    assert_eq!(
        power(&standalone, PLAYER),
        1000 - HEAL_SCRIPT_COST,
        "an applied scripted effect still spends the spell's own cost"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_scripted_effect_with_script_id_zero_stays_the_vanilla_no_op() {
    let mut standalone = Standalone::start("scripted-effect-noop");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);

    insert_spell(
        &standalone,
        E_NOOP_SCRIPT_ID_ZERO_SPELL,
        "Fixture Scripted Noop",
        0,
    );
    insert_effect(
        &standalone,
        E_NOOP_SCRIPT_ID_ZERO_SPELL,
        0,
        u32::from(E_SCRIPTED),
        0,
        u32::from(T_TARGET_ANY),
        0,
    );

    let wolf = spawn_wolf(&standalone);
    prevent_scheduled_regeneration(&standalone, wolf);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 50, max_health = 1000 WHERE guid = {wolf}"
    ));
    let xp_before = xp(&standalone, PLAYER);

    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &E_NOOP_SCRIPT_ID_ZERO_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );

    assert_eq!(
        health(&standalone, wolf),
        50,
        "no game_script row exists for id 0 — nothing to run"
    );
    assert_eq!(xp(&standalone, PLAYER) - xp_before, 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missing_or_disabled_script_refuses_the_cast_before_any_cost_is_spent() {
    let mut standalone = Standalone::start("scripted-effect-gate");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    prevent_scheduled_regeneration(&standalone, PLAYER);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "1000"]);

    insert_spell(
        &standalone,
        E_GATE_SPELL,
        "Fixture Scripted Gate",
        GATE_SPELL_COST,
    );
    insert_effect(
        &standalone,
        E_GATE_SPELL,
        0,
        u32::from(E_SCRIPTED),
        0,
        u32::from(T_TARGET_ANY),
        GATE_SCRIPT,
    );
    let wolf = spawn_wolf(&standalone);

    let casts = subscribe_with_fence(&standalone, &[&format!(
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = {PLAYER} AND spell_id = {E_GATE_SPELL}"
    )]);
    let refused = standalone.call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &E_GATE_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );
    assert!(
        !refused.status.success(),
        "a cast naming a missing script must be refused"
    );
    assert_eq!(
        power(&standalone, PLAYER),
        1000,
        "a refused cast spends nothing"
    );
    fence_subscription(&standalone, &casts);
    assert!(event_rows(&casts, "game_spell_cast_event_live").is_empty());

    insert_script(
        &standalone,
        GATE_SCRIPT,
        "fixture.spellscript.gate",
        false,
        "local x = 1",
    );
    let refused = standalone.call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &E_GATE_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );
    assert!(
        !refused.status.success(),
        "a cast naming a disabled script must be refused"
    );
    assert_eq!(
        power(&standalone, PLAYER),
        1000,
        "a refused cast spends nothing"
    );
    fence_subscription(&standalone, &casts);
    assert!(event_rows(&casts, "game_spell_cast_event_live").is_empty());

    standalone.assert_sql(&format!(
        "UPDATE game_script SET enabled = true WHERE script_id = {GATE_SCRIPT}"
    ));
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &E_GATE_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );
    assert_eq!(power(&standalone, PLAYER), 1000 - GATE_SPELL_COST);
    fence_subscription(&standalone, &casts);
    assert_eq!(event_rows(&casts, "game_spell_cast_event_live").len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_failing_scripted_effect_discards_only_its_own_staged_effects() {
    let mut standalone = Standalone::start("scripted-effect-partial-failure");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);

    insert_spell(
        &standalone,
        E_PARTIAL_FAILURE_SPELL,
        "Fixture Scripted Partial Failure",
        0,
    );
    insert_effect(
        &standalone,
        E_PARTIAL_FAILURE_SPELL,
        0,
        u32::from(E_ENERGIZE),
        50,
        u32::from(T_SELF),
        0,
    );
    insert_effect(
        &standalone,
        E_PARTIAL_FAILURE_SPELL,
        1,
        u32::from(E_SCRIPTED),
        0,
        u32::from(T_TARGET_ANY),
        FAILING_SCRIPT,
    );
    insert_script(
        &standalone,
        FAILING_SCRIPT,
        "fixture.spellscript.failing",
        true,
        "heal(event.target, 999)\nerror(\"boom\")",
    );

    prevent_scheduled_regeneration(&standalone, PLAYER);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "1000"]);
    standalone.assert_call("debug_set_power", &[&PLAYER.to_string(), "800"]);
    const FLOOR: u32 = 800;
    let wolf = spawn_wolf(&standalone);
    prevent_scheduled_regeneration(&standalone, wolf);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 100, max_health = 100000 WHERE guid = {wolf}",
    ));

    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &E_PARTIAL_FAILURE_SPELL.to_string(),
            &wolf.to_string(),
        ],
    );

    assert_eq!(
        power(&standalone, PLAYER),
        FLOOR + 50,
        "the sibling ENERGIZE effect must apply regardless of the scripted effect's failure"
    );
    assert_eq!(
        health(&standalone, wolf),
        100,
        "the failed Invocation's staged heal must not commit"
    );
}

fn insert_spell(standalone: &Standalone, spell_id: u32, name: &str, cost: u32) {
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({spell_id}, '{name}', 0, {cost}, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, \
         false, 0, 0, 0, 0, 0, 0, 0, 0)"
    ));
}

#[allow(clippy::too_many_arguments)]
fn insert_effect(
    standalone: &Standalone,
    spell_id: u32,
    effect_index: u8,
    kind: u32,
    base_points: i32,
    target: u32,
    script_id: u32,
) {
    let id = ((spell_id as u64) << 2) | effect_index as u64;
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({id}, {spell_id}, {effect_index}, {kind}, {base_points}, 0, 0.0, 0, {target}, \
         0.0, 0, 0, 0, 0, 0, 0, {script_id}, false, 0.0)"
    ));
}

fn insert_script(standalone: &Standalone, script_id: u32, name: &str, enabled: bool, source: &str) {
    standalone.assert_sql(&format!(
        "INSERT INTO game_script (script_id, name, package, source_hash, content_hash, event, \
         priority, enabled, source) \
         VALUES ({script_id}, '{name}', 'fixture.spellscript', '', '', 'on_login', 0, {enabled}, '{source}')"
    ));
}

fn spawn_wolf(standalone: &Standalone) -> u64 {
    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&PLAYER.to_string(), &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist")
}

fn entity(standalone: &Standalone, guid: u64) -> SqlRow {
    standalone
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        ))
        .pop()
        .expect("the entity must be live")
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    entity(standalone, guid)["health"]
        .parse()
        .expect("health is a number")
}

fn power(standalone: &Standalone, guid: u64) -> u32 {
    entity(standalone, guid)["power"]
        .parse()
        .expect("power is a number")
}

fn prevent_scheduled_regeneration(standalone: &Standalone, guid: u64) {
    let flags = entity(standalone, guid)["unit_flags"]
        .parse::<u32>()
        .expect("unit flags are a number");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_flags = {}, combat_until_ms = {} WHERE guid = {guid}",
        flags | unit_flags::IN_COMBAT,
        u64::MAX,
    ));
}

fn xp(standalone: &Standalone, guid: u64) -> u32 {
    entity(standalone, guid)["xp"]
        .parse()
        .expect("xp is a number")
}
