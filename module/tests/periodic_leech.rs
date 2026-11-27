mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_shared::guid;
use lyracore_shared::spell::taxonomy::{A_ABSORB, A_PERIODIC_DAMAGE, CAST_FLAG_CHANNELED};
use lyracore_shared::spell_custom::CHAN_NO_DIST_LIMIT;
use support::{event_rows, number, poll_until, Row, Standalone, POLL_TIMEOUT};

const CHARACTER: u64 = 1;
const WOLF: u64 = guid::creature(TEST_WOLF_ENTRY, 1);
const CHARACTER_HEALTH: u32 = 50;
/// `debug_fill_aura_slots` gives buff slot n spell id `9_000_000 + n` and debuff slots start at 32.
const SHIELD: u32 = 9_000_000;
const DRAIN: u32 = 9_000_032;
/// `SpellCastEventKind::LeechLog`: the damage log, on the drained unit.
const LEECH_LOG: u8 = 7;
/// `SpellCastEventKind::LeechHealLog`: the heal log, on the caster.
const LEECH_HEAL_LOG: u8 = 8;
const ALL_SCHOOLS: u32 = 127;

struct Drain {
    wolf_health: u32,
    tick: i32,
    multiple_hundredths: i32,
    shield: i32,
}

impl Drain {
    fn plain() -> Self {
        Self {
            wolf_health: 10_000,
            tick: 30,
            multiple_hundredths: 100,
            shield: 0,
        }
    }
}

/// The Character channels a Drain Life stand-in on the wolf; the tick is due only when
/// `tick_once` makes it so.
fn stage_drain(name: &str, drain: &Drain) -> Standalone {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, godmode = false, map_id = 0, \
         instance_id = 0, health = {CHARACTER_HEALTH}, max_health = 10000 WHERE guid = {CHARACTER}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, map_id = 0, instance_id = 0, \
         health = {}, max_health = 10000 WHERE guid = {WOLF}",
        drain.wolf_health
    ));
    standalone.insert_rows(
        "game_spell",
        &[&[
            ("spell_id", &DRAIN),
            ("name", &"Drain fixture"),
            ("cast_flags", &CAST_FLAG_CHANNELED),
            ("custom_flags", &CHAN_NO_DIST_LIMIT),
        ]],
    );
    if drain.shield > 0 {
        standalone.assert_call(
            "debug_fill_aura_slots",
            &[&WOLF.to_string(), &WOLF.to_string(), "false", "1"],
        );
        standalone.assert_sql(&format!(
            "UPDATE game_aura SET eff_kind = {A_ABSORB}, amount_remaining = {}, \
             eff_p0 = {ALL_SCHOOLS} WHERE spell_id = {SHIELD}",
            drain.shield
        ));
    }
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[&WOLF.to_string(), &CHARACTER.to_string(), "true", "1"],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET eff_kind = {A_PERIODIC_DAMAGE}, amount = {}, eff_p1 = {} \
         WHERE spell_id = {DRAIN}",
        drain.tick, drain.multiple_hundredths
    ));
    standalone
}

struct Tick {
    damage: Row,
    heal: Row,
}

fn tick_once(standalone: &Standalone) -> Tick {
    let damage_query = format!(
        "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {DRAIN} AND kind = {LEECH_LOG}"
    );
    let heal_query = format!(
        "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {DRAIN} AND kind = {LEECH_HEAL_LOG}"
    );
    let ticks = standalone.subscribe(&[&damage_query, &heal_query]);
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        - 1_000_000;
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET period_ms = 600000, next_tick_micros = {due} WHERE spell_id = {DRAIN}"
    ));
    let mut rows = Vec::new();
    assert!(
        poll_until(POLL_TIMEOUT, || {
            rows = event_rows(&ticks, "game_spell_cast_event_live");
            rows.len() >= 2
        }),
        "the leech tick must log its damage and its heal"
    );
    assert_eq!(rows.len(), 2);
    let of_kind = |kind: u8| {
        rows.iter()
            .find(|row| number::<u8>(row, "kind") == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no event of kind {kind}"))
    };
    Tick {
        damage: of_kind(LEECH_LOG),
        heal: of_kind(LEECH_HEAL_LOG),
    }
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_drain_life_tick_heals_its_caster_by_the_damage_dealt() {
    let standalone = stage_drain("leech-heals-caster", &Drain::plain());

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick.damage, "damage"), 30);
    assert_eq!(number::<u32>(&tick.heal, "healed"), 30);
    assert_eq!(health(&standalone, WOLF), 9_970);
    assert_eq!(health(&standalone, CHARACTER), CHARACTER_HEALTH + 30);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_leech_tick_heals_the_damage_dealt_times_the_effects_multiple_value() {
    let drain = Drain {
        multiple_hundredths: 250,
        ..Drain::plain()
    };
    let standalone = stage_drain("leech-multiple", &drain);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick.damage, "damage"), 30);
    assert_eq!(number::<u32>(&tick.heal, "healed"), 75);
    assert_eq!(health(&standalone, CHARACTER), CHARACTER_HEALTH + 75);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fully_absorbed_leech_tick_heals_nothing() {
    let drain = Drain {
        shield: 100,
        ..Drain::plain()
    };
    let standalone = stage_drain("leech-absorbed", &drain);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick.damage, "damage"), 0);
    assert_eq!(number::<u32>(&tick.damage, "absorbed"), 30);
    assert_eq!(number::<u32>(&tick.heal, "healed"), 0);
    assert_eq!(health(&standalone, WOLF), 10_000);
    assert_eq!(health(&standalone, CHARACTER), CHARACTER_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_leech_tick_adds_half_the_amount_gained_as_heal_threat() {
    let standalone = stage_drain("leech-heal-threat", &Drain::plain());

    tick_once(&standalone);

    let threat = standalone.query_rows(&format!(
        "SELECT threat FROM game_threat WHERE creature_guid = {WOLF} AND source_guid = {CHARACTER}"
    ));
    assert_eq!(threat.len(), 1);
    // 30 from the damage and half of the 30 gained.
    assert_eq!(number::<i64>(&threat[0], "threat"), 45);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_killing_leech_tick_heals_only_the_health_left() {
    let drain = Drain {
        wolf_health: 20,
        ..Drain::plain()
    };
    let standalone = stage_drain("leech-kill", &drain);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick.damage, "damage"), 30);
    assert_eq!(number::<u32>(&tick.heal, "healed"), 20);
    assert_eq!(health(&standalone, CHARACTER), CHARACTER_HEALTH + 20);
    assert!(poll_until(POLL_TIMEOUT, || {
        standalone.query_rows(&format!(
            "SELECT dead FROM game_world_entity WHERE guid = {WOLF}"
        ))[0]["dead"]
            == "true"
    }));
}
