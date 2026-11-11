mod support;

use std::time::{Duration, Instant};

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::packing::power_type::ENERGY;
use lyracore_shared::spell::taxonomy::{A_MOD_POWER_REGEN_PCT, P_POWER_TYPE, T_SELF};
use support::{number, poll_until, Standalone};

const PLAYER: u64 = 1;
const ADRENALINE_RUSH: u32 = 13750;
const ENERGY_TICK: Duration = Duration::from_secs(2);

fn rogue_at_zero_energy(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    shard.assert_call("debug_set_level", &[&PLAYER.to_string(), "60"]);
    let player = shard
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    let bytes: u32 = number(&player, "unit_bytes_0");
    let energy_user = (bytes & 0x00FF_FFFF) | (u32::from(ENERGY) << 24);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {energy_user}, max_power = 400 \
         WHERE guid = {PLAYER}"
    ));
    shard.assert_call("debug_set_power", &[&PLAYER.to_string(), "0"]);
    shard
}

fn energy(shard: &Standalone) -> u32 {
    let row = shard
        .query_rows(&format!(
            "SELECT power FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    number(&row, "power")
}

struct Reading {
    at: Instant,
    energy: u32,
}

// Starts at the first tick seen, so the readings span whole ticks.
fn watch_energy(shard: &Standalone, gain: u32) -> Vec<Reading> {
    let before = energy(shard);
    let mut readings: Vec<Reading> = Vec::new();
    let reached = poll_until(ENERGY_TICK * 12, || {
        let now = energy(shard);
        let changed = readings
            .last()
            .map_or(now != before, |last| now != last.energy);
        if changed {
            readings.push(Reading {
                at: Instant::now(),
                energy: now,
            });
        }
        readings
            .first()
            .is_some_and(|first| now >= first.energy + gain)
    });
    let energies: Vec<u32> = readings.iter().map(|r| r.energy).collect();
    assert!(reached, "energy stalled: {energies:?}");
    readings
}

fn assert_ticks_of(readings: &[Reading], per_tick: u32) {
    let steps: Vec<u32> = readings
        .windows(2)
        .map(|pair| pair[1].energy - pair[0].energy)
        .collect();
    assert!(
        steps.iter().all(|step| step % per_tick == 0),
        "steps {steps:?}"
    );
    let single = steps.iter().filter(|&&step| step == per_tick).count();
    assert!(
        single * 2 > steps.len(),
        "most steps are one tick: {steps:?}"
    );
    let (first, last) = (&readings[0], &readings[readings.len() - 1]);
    let ticks = f64::from((last.energy - first.energy) / per_tick);
    let expected = last.at.duration_since(first.at).as_secs_f64() / ENERGY_TICK.as_secs_f64();
    assert!(
        (ticks - expected).abs() <= 1.0,
        "{ticks} ticks in {:?}",
        last.at.duration_since(first.at)
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rogue_regains_twenty_energy_every_two_seconds() {
    let shard = rogue_at_zero_energy("energy-regen-ticks");
    assert_ticks_of(&watch_energy(&shard, 80), 20);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn adrenaline_rush_doubles_each_energy_tick() {
    let shard = rogue_at_zero_energy("energy-regen-adrenaline-rush");
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &ADRENALINE_RUSH),
            ("name", &"Adrenaline Rush"),
            ("power_type", &ENERGY),
            ("cooldown_ms", &300000),
            ("duration_ms", &60000),
            ("school_mask", &1),
            ("spell_level", &20),
            ("family_name", &8),
        ],
    );
    let effect_id = packed_spell_effect_id(ADRENALINE_RUSH, 0);
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &effect_id),
            ("spell_id", &ADRENALINE_RUSH),
            ("kind", &A_MOD_POWER_REGEN_PCT),
            ("base_points", &100),
            ("target", &T_SELF),
            ("p_0", &ENERGY),
            ("p_0_kind", &P_POWER_TYPE),
        ],
    );
    shard.assert_call(
        "debug_force_cast",
        &[&PLAYER.to_string(), &ADRENALINE_RUSH.to_string()],
    );
    shard.assert_call("debug_set_power", &[&PLAYER.to_string(), "0"]);

    assert_ticks_of(&watch_energy(&shard, 160), 40);

    shard.assert_sql(&format!(
        "DELETE FROM game_aura WHERE target_guid = {PLAYER} AND spell_id = {ADRENALINE_RUSH}"
    ));
    let after = watch_energy(&shard, 40);
    assert_eq!(
        after[1].energy - after[0].energy,
        20,
        "regeneration returns to 20"
    );
}
