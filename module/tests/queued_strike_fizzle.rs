mod support;

use lyracore_shared::spell::taxonomy::{E_NEXT_SWING, T_TARGET_ENEMY};

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{event_rows, poll_until, Standalone, TableSubscription, POLL_TIMEOUT};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const TEST_WOLF_ENTRY: u32 = 51000;
const HEROIC_STRIKE: u32 = 78;
const CLEAVE: u32 = 845;
const BATTLE_SHOUT: u32 = 6673;
const HEROIC_STRIKE_COST: u32 = 150;
const CLEAVE_COST: u32 = 200;
const BATTLE_SHOUT_COST: u32 = 100;
const NO_POWER: &str = "1";

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
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET cost = {BATTLE_SHOUT_COST}, gcd_ms = 0 WHERE spell_id = {BATTLE_SHOUT}"
    ));

    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);

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

    let (casts, landed) = swing(&standalone);

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
    assert!(
        casts.iter().any(|row| is_power_failure(row, HEROIC_STRIKE)),
        "the caster must be told the strike failed for lack of rage, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| fired(row, HEROIC_STRIKE)),
        "a fizzled strike must not report a cast, got {casts:?}"
    );
    assert_eq!(
        landed["spell_swing"], "false",
        "the attack must resolve as an ordinary white swing"
    );

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {wolf}"
    ));

    stage_power(&standalone, CLEAVE_COST);
    cast(&standalone, CLEAVE, wolf);
    let (casts, landed) = swing(&standalone);

    let after = player(&standalone);
    assert_eq!(
        power(&after),
        0,
        "an exactly affordable strike spends it all"
    );
    assert_eq!(queued_spell(&after), "0");
    assert!(
        casts.iter().any(|row| fired(row, CLEAVE)),
        "rage equal to the cost must fire the strike, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| is_power_failure(row, CLEAVE)),
        "an affordable strike must not report a failure, got {casts:?}"
    );
    assert_eq!(
        landed["spell_swing"], "true",
        "a fired strike replaces the white swing"
    );

    stage_power(&standalone, CLEAVE_COST);
    cast(&standalone, CLEAVE, wolf);
    stage_power(&standalone, CLEAVE_COST - 1);
    let (casts, landed) = swing(&standalone);

    let after = player(&standalone);
    assert_eq!(
        power(&after),
        CLEAVE_COST - 1,
        "a fizzle leaves the pool exactly as it found it"
    );
    assert_eq!(queued_spell(&after), "0");
    assert!(
        casts.iter().any(|row| is_power_failure(row, CLEAVE)),
        "one point short must fizzle with the same failure, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| fired(row, CLEAVE)),
        "a fizzled strike must not report a cast, got {casts:?}"
    );
    assert_eq!(landed["spell_swing"], "false");

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = false WHERE guid = {wolf}"
    ));

    let after = landed_swing(&standalone, || stage_power(&standalone, 0));
    assert!(
        power(&after) > 0,
        "a landed white swing must reward rage, so the strike check below is not vacuous"
    );

    let after = landed_swing(&standalone, || {
        stage_power(&standalone, HEROIC_STRIKE_COST);
        cast(&standalone, HEROIC_STRIKE, wolf);
    });
    assert_eq!(
        power(&after),
        0,
        "the landed strike must spend its cost and reward no rage"
    );
}

fn landed_swing(standalone: &Standalone, mut stage: impl FnMut()) -> SqlRow {
    for _ in 0..20 {
        stage();
        let (_, landed) = swing(standalone);
        if landed["damage"] != "0" {
            return player(standalone);
        }
    }
    panic!("no swing landed in 20 attempts");
}

fn stage_on_next_swing_spell(standalone: &Standalone, spell_id: u32, name: &str, cost: u32) {
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
        "UPDATE game_world_entity SET simulation_indexed = false, health = 100000, max_health = 100000 WHERE guid = {guid}"
    ));
    guid
}

fn engage(standalone: &Standalone, target: u64) {
    let events =
        standalone.subscribe(&["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"]);
    standalone.assert_call("debug_engage", &["1", &target.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "the engagement never swung"
    );
}

fn stage_power(standalone: &Standalone, rage: u32) {
    hold_the_swing(standalone);
    standalone.assert_call("debug_set_power", &["1", &rage.to_string()]);
}

fn cast(standalone: &Standalone, spell_id: u32, target: u64) {
    hold_the_swing(standalone);
    standalone.assert_call(
        "debug_cast_at",
        &["1", &spell_id.to_string(), &target.to_string()],
    );
}

fn swing(standalone: &Standalone) -> (Vec<SqlRow>, SqlRow) {
    hold_the_swing(standalone);
    let events = standalone.subscribe(&[
        "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1",
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1",
    ]);
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "the swing never fired"
    );
    hold_the_swing(standalone);
    (cast_events(&events), own_swing(&events))
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

fn cast_events(events: &TableSubscription) -> Vec<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
}

fn is_power_failure(row: &SqlRow, spell_id: u32) -> bool {
    row["spell_id"] == spell_id.to_string()
        && row["is_interrupted"] == "true"
        && row["failure_reason"] == NO_POWER
}

fn fired(row: &SqlRow, spell_id: u32) -> bool {
    row["spell_id"] == spell_id.to_string() && row["is_completion"] == "true"
}

fn own_swings(events: &TableSubscription) -> Vec<SqlRow> {
    event_rows(events, "game_combat_event_live")
}

fn own_swing(events: &TableSubscription) -> SqlRow {
    own_swings(events)
        .pop()
        .expect("the swing must have been logged")
}
