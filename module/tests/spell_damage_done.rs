mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_MOD_DAMAGE_DONE_PCT, A_SEAL, CAST_FLAG_ALWAYS_HIT, E_JUDGEMENT, P_SCHOOL_MASK, T_SELF,
    T_TARGET_ENEMY,
};
use support::Standalone;

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 10_000;
const MANA_BURN: u32 = 8129;
const DAMAGE_DONE_BUFF: u32 = 5090400;
const SEAL: u32 = 5090401;
const JUDGEMENT: u32 = 5090402;
const HOLY_AND_SHADOW: i32 = 2 | 32;

fn stage() -> (Standalone, u64) {
    let mut shard = Standalone::start("spell-damage-done");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    let unit_bytes_0: u32 = shard.query_rows(&format!(
        "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {wolf}"
    ))[0]["unit_bytes_0"]
        .parse()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, unit_bytes_0 = {}, \
         power = 1000, max_power = 1000 WHERE guid = {wolf}",
        unit_bytes_0 & 0x00FF_FFFF
    ));
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = {CAST_FLAG_ALWAYS_HIT}, gcd_ms = 0 WHERE spell_id = {MANA_BURN}"
    ));
    self_aura(
        &shard,
        DAMAGE_DONE_BUFF,
        "Damage done fixture",
        A_MOD_DAMAGE_DONE_PCT,
        20,
    );
    self_aura(&shard, SEAL, "Seal fixture", A_SEAL, 50);
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &JUDGEMENT),
            ("name", &"Judgement fixture"),
            ("range_yd", &10),
            ("school_mask", &2),
            ("is_negative", &true),
            ("cast_flags", &CAST_FLAG_ALWAYS_HIT),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(JUDGEMENT, 0)),
            ("spell_id", &JUDGEMENT),
            ("kind", &E_JUDGEMENT),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    (shard, wolf)
}

fn self_aura(shard: &Standalone, spell_id: u32, name: &str, kind: u8, amount: i32) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("school_mask", &2),
            ("duration_ms", &600_000),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &kind),
            ("base_points", &amount),
            ("target", &T_SELF),
            ("p_0", &HOLY_AND_SHADOW),
            ("p_0_kind", &P_SCHOOL_MASK),
        ],
    );
}

fn damage_from(shard: &Standalone, wolf: u64, spell_id: u32) -> u32 {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH}, \
         power = 1000 WHERE guid = {wolf}"
    ));
    shard.assert_call(
        "debug_cast_at",
        &["1", &spell_id.to_string(), &wolf.to_string()],
    );
    let health: u32 = shard.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {wolf}"
    ))[0]["health"]
        .parse()
        .unwrap();
    WOLF_HEALTH - health
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn percent_damage_done_raises_mana_burn_and_judgement() {
    let (shard, wolf) = stage();
    // Mana Burn drains 100 mana and deals half of it.
    assert_eq!(damage_from(&shard, wolf, MANA_BURN), 50);
    shard.assert_call("debug_cast_at", &["1", &SEAL.to_string(), "1"]);
    assert_eq!(damage_from(&shard, wolf, JUDGEMENT), 50);

    shard.assert_call("debug_cast_at", &["1", &DAMAGE_DONE_BUFF.to_string(), "1"]);
    assert_eq!(damage_from(&shard, wolf, MANA_BURN), 60);
    shard.assert_call("debug_cast_at", &["1", &SEAL.to_string(), "1"]);
    assert_eq!(damage_from(&shard, wolf, JUDGEMENT), 60);
}
