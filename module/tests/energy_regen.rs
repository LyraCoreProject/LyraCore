mod support;

use std::time::Duration;

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
        "UPDATE game_world_entity SET unit_bytes_0 = {energy_user}, max_power = 100 \
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

fn energy_steps_until(shard: &Standalone, target: u32) -> Vec<u32> {
    let mut readings = vec![energy(shard)];
    let reached = poll_until(ENERGY_TICK * 8, || {
        let now = energy(shard);
        if readings.last() != Some(&now) {
            readings.push(now);
        }
        now >= target
    });
    assert!(reached, "energy stalled: {readings:?}");
    readings.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_rogue_regains_twenty_energy_every_two_seconds() {
    let shard = rogue_at_zero_energy("energy-regen-ticks");
    let steps = energy_steps_until(&shard, 60);
    assert!(steps.iter().all(|&step| step == 20), "steps {steps:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn adrenaline_rush_doubles_each_energy_tick() {
    let shard = rogue_at_zero_energy("energy-regen-adrenaline-rush");
    shard.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges) \
         VALUES ({ADRENALINE_RUSH}, 'Adrenaline Rush', {ENERGY}, 0, 0, 0, 300000, 0, 15000, 1, \
         0, 0, 0, 0, 0, 20, 0, false, 0, 0, 8, 0, 0, 0, 0)"
    ));
    let effect_id = u64::from(ADRENALINE_RUSH) << 2;
    shard.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat) \
         VALUES ({effect_id}, {ADRENALINE_RUSH}, 0, {A_MOD_POWER_REGEN_PCT}, 100, 0, 0.0, 0, \
         {T_SELF}, 0.0, 0, 0, 0, {ENERGY}, {P_POWER_TYPE}, 0, 0, false)"
    ));
    shard.assert_call(
        "debug_force_cast",
        &[&PLAYER.to_string(), &ADRENALINE_RUSH.to_string()],
    );
    shard.assert_call("debug_set_power", &[&PLAYER.to_string(), "0"]);

    let steps = energy_steps_until(&shard, 80);
    assert!(steps.iter().all(|&step| step == 40), "steps {steps:?}");
}
