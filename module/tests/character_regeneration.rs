mod support;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_MANA_REGEN_INTERRUPT, A_MOD_POWER_REGEN, A_MOD_POWER_REGEN_PCT, A_PERIODIC_ENERGIZE,
    P_PCT_MAX_POWER, P_POWER_TYPE, T_SELF,
};
use support::Standalone;

const MAGE: u8 = 8;
const MP5: u32 = 5090300;
const CASTING_ALLOWANCE: u32 = 5090301;
const RAGE_REGEN: u32 = 5090302;
const EVOCATION_SHAPE: u32 = 5090303;
const FIVE_SECOND_RULE_UNTIL_MS: &str = "1800001000000";

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET class = {MAGE} WHERE guid = 1"
    ));
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard
}

fn self_aura(
    shard: &Standalone,
    spell_id: u32,
    kind: u8,
    points: i32,
    period_ms: u32,
    p0: (i32, u8),
) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &"Regeneration fixture"),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &kind),
            ("base_points", &points),
            ("period_ms", &period_ms),
            ("target", &T_SELF),
            ("p_0", &p0.0),
            ("p_0_kind", &p0.1),
        ],
    );
}

fn cast(shard: &Standalone, spell_id: u32) {
    shard.assert_call("debug_force_cast", &["1", &spell_id.to_string()]);
}

fn mana(shard: &Standalone) -> u32 {
    shard.query_rows("SELECT power FROM game_world_entity WHERE guid = 1")[0]["power"]
        .parse()
        .unwrap()
}

fn set_mana_pool(shard: &Standalone, five_second_rule_until_ms: &str) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, max_power = 100000, spirit = 100, mana_regen_paused_until_ms = {five_second_rule_until_ms} WHERE guid = 1"
    ));
    shard.assert_call("debug_set_power", &["1", "100"]);
}

// Any two-second window holds exactly one Character tick once the first has landed.
fn one_tick_gain(shard: &Standalone) -> u32 {
    shard.assert_call("debug_advance_roll_batch", &["80"]);
    let before = mana(shard);
    shard.assert_call("debug_advance_roll_batch", &["40"]);
    mana(shard) - before
}

fn aura(shard: &Standalone, spell_id: u32) -> (String, String, String) {
    let row = shard
        .query_rows(&format!(
            "SELECT eff_kind, amount, period_ms FROM game_aura WHERE target_guid = 1 AND spell_id = {spell_id}"
        ))
        .pop()
        .expect("the cast Aura must exist");
    (
        row["eff_kind"].clone(),
        row["amount"].clone(),
        row["period_ms"].clone(),
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn mp5_and_the_casting_allowance_reach_the_two_second_mana_tick() {
    let shard = stage("character-regeneration-mana");
    self_aura(&shard, MP5, A_MOD_POWER_REGEN, 12, 0, (0, P_POWER_TYPE));
    self_aura(
        &shard,
        CASTING_ALLOWANCE,
        A_MANA_REGEN_INTERRUPT,
        30,
        0,
        (0, 0),
    );

    cast(&shard, MP5);
    set_mana_pool(&shard, FIVE_SECOND_RULE_UNTIL_MS);
    assert_eq!(
        one_tick_gain(&shard),
        4,
        "12 MP5 gives 4.8 mana per tick during the five-second rule, truncated to 4"
    );

    cast(&shard, CASTING_ALLOWANCE);
    set_mana_pool(&shard, FIVE_SECOND_RULE_UNTIL_MS);
    assert_eq!(
        one_tick_gain(&shard),
        16,
        "a mage with 100 Spirit keeps 30% of 37.5 spirit mana plus 4.8 MP5 while casting"
    );

    set_mana_pool(&shard, "0");
    assert_eq!(
        one_tick_gain(&shard),
        42,
        "outside the five-second rule full spirit mana adds to MP5"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn publish_repair_moves_stored_rage_and_evocation_auras_to_source_units_once() {
    let shard = stage("character-regeneration-repair");
    self_aura(
        &shard,
        RAGE_REGEN,
        A_PERIODIC_ENERGIZE,
        15,
        5000,
        (1, P_POWER_TYPE),
    );
    self_aura(
        &shard,
        EVOCATION_SHAPE,
        A_PERIODIC_ENERGIZE,
        15,
        2000,
        (0, P_PCT_MAX_POWER),
    );
    cast(&shard, RAGE_REGEN);
    cast(&shard, EVOCATION_SHAPE);

    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET base_points = 9, period_ms = 3000 WHERE spell_id = {RAGE_REGEN}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET kind = {A_MOD_POWER_REGEN_PCT}, base_points = 1500, period_ms = 0, p_0_kind = {P_POWER_TYPE} WHERE spell_id = {EVOCATION_SHAPE}"
    ));
    for _ in 0..2 {
        shard.assert_call("debug_repair_after_publish", &[]);
        assert_eq!(
            aura(&shard, RAGE_REGEN),
            (A_PERIODIC_ENERGIZE.to_string(), "9".into(), "3000".into()),
            "Anger Management restores 3/5 of its five-second rage every three seconds"
        );
        assert_eq!(
            aura(&shard, EVOCATION_SHAPE),
            (A_MOD_POWER_REGEN_PCT.to_string(), "1500".into(), "0".into()),
            "Evocation's frozen 15% energize becomes the source 1500% spirit bonus"
        );
    }
}
