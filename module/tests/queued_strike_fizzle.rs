//! The on-next-swing strike at the main-hand swing: what happens when the caster can no longer pay
//! the deferred cost, and what still happens when it can. Drives the real reducers against a
//! published module and reads the durable outcome back, because the queue, the power pool, the swing
//! roll and the cast-event output only meet inside the swing tick.

mod support;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{poll_until, Standalone, POLL_TIMEOUT};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const TEST_WOLF_ENTRY: u32 = 51000;
const HEROIC_STRIKE: u32 = 78;
const CLEAVE: u32 = 845;
const BATTLE_SHOUT: u32 = 6673;
/// Costs are internal rage units (rage is stored ×10): Heroic Strike 15 rage, Cleave 20, Battle
/// Shout 10 — the vanilla rank-1 costs.
const HEROIC_STRIKE_COST: u32 = 150;
const CLEAVE_COST: u32 = 200;
const BATTLE_SHOUT_COST: u32 = 100;
/// The module's `CAST_FAIL_NO_POWER`, which the gateway reports as `SPELL_FAILED_NO_POWER`.
const NO_POWER: &str = "1";

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_queued_strike_fizzles_into_a_white_swing_when_the_rage_is_gone() {
    let mut standalone = Standalone::start("queued-strike-fizzle");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        HEROIC_STRIKE,
        "Heroic Strike",
        HEROIC_STRIKE_COST,
    );
    stage_on_next_swing_spell(&standalone, CLEAVE, "Cleave", CLEAVE_COST);
    // The sandbox seeds Battle Shout free; give it its vanilla rage cost. Its global cooldown is
    // zeroed with it — the scenario stages several casts back to back and the global cooldown is
    // not what it is about.
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET cost = {BATTLE_SHOUT_COST}, gcd_ms = 0 WHERE spell_id = {BATTLE_SHOUT}"
    ));

    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    // The Warrior is staged untouchable for the whole scenario: the wolf's own swings would
    // otherwise kill a level-1 player mid-scenario, and rage taken from a hit would blur every
    // power reading. What the Warrior DEALS is untouched, so the white swing below is a real one.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);

    // --- The reported rotation: queue Heroic Strike with the rage for it, then shout it away.
    stage_power(&standalone, HEROIC_STRIKE_COST);
    cast(&standalone, HEROIC_STRIKE, wolf);
    cast(&standalone, BATTLE_SHOUT, PLAYER);
    let staged = player(&standalone);
    assert_eq!(queued_spell(&staged), HEROIC_STRIKE.to_string());
    let rage_at_the_swing = power(&staged);
    assert!(
        rage_at_the_swing < HEROIC_STRIKE_COST,
        "the shout must leave the Warrior short of Heroic Strike's cost, had {rage_at_the_swing}"
    );

    swing(&standalone);

    let after = player(&standalone);
    assert!(
        power(&after) >= rage_at_the_swing,
        "the fizzle must not charge the strike; rage went {rage_at_the_swing} -> {}",
        power(&after)
    );
    assert_eq!(
        queued_spell(&after),
        "0",
        "the queued strike must be cleared, so the ability can be queued again"
    );
    let casts = cast_events(&standalone);
    assert!(
        casts.iter().any(|row| is_power_failure(row, HEROIC_STRIKE)),
        "the caster must be told the strike failed for lack of rage, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| fired(row, HEROIC_STRIKE)),
        "a fizzled strike must not report a cast, got {casts:?}"
    );
    // The attack itself survives the fizzle as an ordinary white swing — it rolls the normal attack
    // table, so it may land or be avoided, but it is never presented as the queued ability.
    assert_eq!(
        own_swing(&standalone)["spell_swing"],
        "false",
        "the attack must resolve as an ordinary white swing"
    );

    // --- The same contract for a second on-next-swing ability, at the cost boundary. Both sides
    // are staged untouchable here so no rage flows from the swing itself and the pool arithmetic is
    // exact on either side of the boundary.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {wolf}"
    ));

    // Rage exactly equal to the cost fires the strike and spends the pool to zero.
    stage_power(&standalone, CLEAVE_COST);
    cast(&standalone, CLEAVE, wolf);
    swing(&standalone);

    let after = player(&standalone);
    assert_eq!(
        power(&after),
        0,
        "an exactly affordable strike spends it all"
    );
    assert_eq!(queued_spell(&after), "0");
    let casts = cast_events(&standalone);
    assert!(
        casts.iter().any(|row| fired(row, CLEAVE)),
        "rage equal to the cost must fire the strike, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| is_power_failure(row, CLEAVE)),
        "an affordable strike must not report a failure, got {casts:?}"
    );
    assert_eq!(
        own_swing(&standalone)["spell_swing"],
        "true",
        "a fired strike replaces the white swing"
    );

    // One point below the cost fizzles, exactly like Heroic Strike did.
    stage_power(&standalone, CLEAVE_COST);
    cast(&standalone, CLEAVE, wolf);
    stage_power(&standalone, CLEAVE_COST - 1);
    swing(&standalone);

    let after = player(&standalone);
    assert_eq!(
        power(&after),
        CLEAVE_COST - 1,
        "a fizzle leaves the pool exactly as it found it"
    );
    assert_eq!(queued_spell(&after), "0");
    let casts = cast_events(&standalone);
    assert!(
        casts.iter().any(|row| is_power_failure(row, CLEAVE)),
        "one point short must fizzle with the same failure, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| fired(row, CLEAVE)),
        "a fizzled strike must not report a cast, got {casts:?}"
    );
    assert_eq!(own_swing(&standalone)["spell_swing"], "false");
}

/// An on-next-swing ability the sandbox has no import for: a rage-costed melee header plus one
/// `E_NEXT_SWING` effect on the target, which is the shape the importer gives Heroic Strike and
/// Cleave. No global cooldown, so the staging casts below never wait on one.
fn stage_on_next_swing_spell(standalone: &Standalone, spell_id: u32, name: &str, cost: u32) {
    const E_NEXT_SWING: u32 = 0x13;
    const T_TARGET_ENEMY: u32 = 1;
    const RAGE: u32 = 1;
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges) \
         VALUES ({spell_id}, '{name}', {RAGE}, {cost}, 0, 0, 0, 5, 0, 1, 0, 0, 0, 0, 0, 0, 0, \
         false, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat) \
         VALUES ({}, {spell_id}, 0, {E_NEXT_SWING}, 11, 0, 0.0, 0, {T_TARGET_ENEMY}, 0.0, 0, 0, 0, \
         0, 0, 0, 0, false)",
        (spell_id as u64) << 2
    ));
}

/// A Test Wolf within melee reach, tough enough to outlive the scenario's swings.
fn spawn_target(standalone: &Standalone) -> u64 {
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let guid = standalone
        .query_rows(&format!(
            "SELECT guid, x FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 100000, max_health = 100000 WHERE guid = {guid}"
    ));
    guid
}

/// Arm the melee engagement and wait until it is swinging, which is also what stops the Warrior's
/// rage from decaying out of combat while the scenario stages each case.
fn engage(standalone: &Standalone, target: u64) {
    standalone.assert_call("debug_engage", &["1", &target.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(standalone).is_empty()),
        "the engagement never swung"
    );
}

/// Set the Warrior's rage, holding the swing off so the staging call cannot be overtaken.
fn stage_power(standalone: &Standalone, rage: u32) {
    hold_the_swing(standalone);
    standalone.assert_call("debug_set_power", &["1", &rage.to_string()]);
}

/// Cast `spell_id` at `target`, holding the swing off for the same reason.
fn cast(standalone: &Standalone, spell_id: u32, target: u64) {
    hold_the_swing(standalone);
    standalone.assert_call(
        "debug_cast_at",
        &["1", &spell_id.to_string(), &target.to_string()],
    );
}

/// Let exactly one main-hand swing through and wait for it to land. The swing clock is the
/// engagement's own `last_swing_ms` against the module's millisecond wall clock: stamping it now
/// buys a full swing interval of quiet, stamping it at 1 makes the next tick due immediately. The
/// event tables are emptied first so the assertions read this swing alone.
fn swing(standalone: &Standalone) {
    hold_the_swing(standalone);
    standalone.assert_sql("DELETE FROM game_spell_cast_event");
    standalone.assert_sql("DELETE FROM game_combat_event");
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(standalone).is_empty()),
        "the swing never fired"
    );
    hold_the_swing(standalone);
}

fn hold_the_swing(standalone: &Standalone) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32;
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET last_swing_ms = {now_ms} WHERE attacker_guid = 1"
    ));
}

fn player(standalone: &Standalone) -> SqlRow {
    standalone
        .query_rows(&format!(
            "SELECT power, next_swing_spell FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .pop()
        .expect("the player entity must be live")
}

fn power(row: &SqlRow) -> u32 {
    row["power"].parse().expect("power must be a number")
}

fn queued_spell(row: &SqlRow) -> String {
    row["next_swing_spell"].clone()
}

fn cast_events(standalone: &Standalone) -> Vec<SqlRow> {
    standalone.query_rows(&format!(
        "SELECT spell_id, is_interrupted, is_completion, failure_reason FROM \
         game_spell_cast_event WHERE caster_guid = {PLAYER}"
    ))
}

/// The caster-private failure a queued strike reports when it cannot pay at the swing: the cast-bar
/// teardown carrying the Not Enough Power reason.
fn is_power_failure(row: &SqlRow, spell_id: u32) -> bool {
    row["spell_id"] == spell_id.to_string()
        && row["is_interrupted"] == "true"
        && row["failure_reason"] == NO_POWER
}

/// The cast a queued strike reports when it does fire.
fn fired(row: &SqlRow, spell_id: u32) -> bool {
    row["spell_id"] == spell_id.to_string() && row["is_completion"] == "true"
}

fn own_swings(standalone: &Standalone) -> Vec<SqlRow> {
    standalone.query_rows(&format!(
        "SELECT damage, hit_info, spell_swing FROM game_combat_event WHERE attacker_guid = {PLAYER}"
    ))
}

fn own_swing(standalone: &Standalone) -> SqlRow {
    own_swings(standalone)
        .pop()
        .expect("the swing must have been logged")
}
