mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_PERIODIC_DAMAGE, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, E_PERSISTENT_AREA, T_TARGET_ENEMY,
};
use support::{number, Field, Standalone};

const CHARACTER: u64 = 1;
const CHARACTER_HEALTH: u32 = 100;
const LETHAL_DAMAGE: u32 = 1_000;
const FIRE: u32 = 0x04;
const DIRECT_FIXTURE: u32 = 5_123_301;
const PERIODIC_FIXTURE: u32 = 5_123_302;
const GROUND_AREA_FIXTURE: u32 = 5_123_303;
const PERIOD_MS: u32 = 1_000;
// 50 ms scheduler ticks that pass two periods.
const TWO_PERIOD_TICKS: &str = "41";
// A cast at an equal level target misses at most 4% of the time.
const CAST_ATTEMPTS: usize = 20;
const CHARACTER_FACTION: u32 = 9_201;
const WOLF_FACTION: u32 = 9_202;
const DUEL: u64 = 509_0002;
// `state::ACTIVE` in `module/src/duel.rs`.
const ACTIVE: u8 = 2;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_spell_kills_a_character() {
    let (shard, wolf) = stage("creature-spell-kills-character");
    insert_spell(
        &shard,
        DIRECT_FIXTURE,
        &[],
        &[("kind", &E_DAMAGE), ("base_points", &LETHAL_DAMAGE)],
    );

    cast_until(&shard, wolf, DIRECT_FIXTURE, CHARACTER, || {
        is_dead(&shard, CHARACTER)
    });

    assert_killed(&shard, CHARACTER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_damage_tick_kills_a_character() {
    let (shard, wolf) = stage("periodic-tick-kills-character");
    insert_spell(
        &shard,
        PERIODIC_FIXTURE,
        &[("duration_ms", &30_000)],
        &[
            ("kind", &A_PERIODIC_DAMAGE),
            ("base_points", &LETHAL_DAMAGE),
            ("period_ms", &PERIOD_MS),
        ],
    );
    cast_until(&shard, wolf, PERIODIC_FIXTURE, CHARACTER, || {
        !shard
            .query_rows(&format!(
                "SELECT id FROM game_aura WHERE target_guid = {CHARACTER} \
                 AND spell_id = {PERIODIC_FIXTURE}"
            ))
            .is_empty()
    });
    assert_eq!(health(&shard, CHARACTER), CHARACTER_HEALTH);

    shard.assert_call("debug_advance_roll_batch", &[TWO_PERIOD_TICKS]);

    assert_killed(&shard, CHARACTER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ground_area_tick_kills_a_character() {
    let (shard, wolf) = stage("ground-area-kills-character");
    insert_spell(
        &shard,
        GROUND_AREA_FIXTURE,
        &[("duration_ms", &10_000)],
        &[
            ("kind", &E_PERSISTENT_AREA),
            ("base_points", &LETHAL_DAMAGE),
            ("period_ms", &PERIOD_MS),
            ("radius_yd", &10.0),
        ],
    );
    shard.assert_call(
        "debug_spawn_ground_area",
        &[&wolf.to_string(), &GROUND_AREA_FIXTURE.to_string()],
    );

    shard.assert_call("debug_advance_roll_batch", &[TWO_PERIOD_TICKS]);

    assert_killed(&shard, CHARACTER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_lethal_spell_from_a_duel_opponent_ends_the_duel_at_one_health() {
    let mut shard = Standalone::start("duel-spell-ends-at-one-health");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call(
        "debug_create_package_character",
        &[r#""duel-test""#, r#""Rival""#, "1", "1"],
    );
    let rival: u64 = number(
        &shard
            .query_rows("SELECT guid FROM game_character WHERE name = 'Rival'")
            .remove(0),
        "guid",
    );
    shard.assert_call("debug_spawn_player_entity", &[&rival.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let at = shard
        .query_rows(&format!(
            "SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    let (x, y, z): (f32, f32, f32) = (number(&at, "x"), number(&at, "y"), number(&at, "z"));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, x = {}, y = {y}, z = {z}, \
         level = 1 WHERE guid = {rival}",
        x + 1.0
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0, level = 1 WHERE guid = {CHARACTER}"
    ));
    shard.insert_row(
        "game_duel",
        &[
            ("id", &DUEL),
            ("flag_entry", &21680),
            ("initiator_guid", &CHARACTER),
            ("challenged_guid", &rival),
            ("map_id", &at["map_id"]),
            ("instance_id", &at["instance_id"]),
            ("flag_x", &x),
            ("flag_y", &y),
            ("flag_z", &z),
            ("state", &ACTIVE),
        ],
    );
    insert_spell(
        &shard,
        DIRECT_FIXTURE,
        &[],
        &[("kind", &E_DAMAGE), ("base_points", &LETHAL_DAMAGE)],
    );

    cast_until(&shard, CHARACTER, DIRECT_FIXTURE, rival, || {
        shard
            .query_rows(&format!("SELECT id FROM game_duel WHERE id = {DUEL}"))
            .is_empty()
    });

    let loser = shard
        .query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {rival}"
        ))
        .remove(0);
    assert_eq!(loser["dead"], "false", "the Duel loser must survive");
    assert_eq!(loser["health"], "1");
}

fn stage(name: &str) -> (Standalone, u64) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&CHARACTER.to_string(), &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = false, level = 60, \
         health = {CHARACTER_HEALTH}, max_health = {CHARACTER_HEALTH} WHERE guid = {CHARACTER}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60 WHERE guid = {wolf}"
    ));
    hostile_factions(&shard, wolf);
    (shard, wolf)
}

// An enemy area effect picks only hostile units, and the bare Standalone has no faction rows.
fn hostile_factions(shard: &Standalone, wolf: u64) {
    shard.insert_rows(
        "game_faction_template",
        &[
            &[
                ("id", &CHARACTER_FACTION),
                ("faction", &CHARACTER_FACTION),
                ("faction_group", &1),
                ("friend_group", &1),
                ("enemy_group", &2),
            ],
            &[
                ("id", &WOLF_FACTION),
                ("faction", &WOLF_FACTION),
                ("faction_group", &2),
                ("friend_group", &2),
                ("enemy_group", &1),
            ],
        ],
    );
    for (guid, faction) in [(CHARACTER, CHARACTER_FACTION), (wolf, WOLF_FACTION)] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
        ));
    }
}

fn insert_spell(shard: &Standalone, spell_id: u32, spell: &[Field], effect: &[Field]) {
    let name = format!("Character damage death fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("school_mask", &FIRE),
        ("range_yd", &30),
        ("is_negative", &true),
        ("gcd_ms", &0),
        ("cooldown_ms", &0),
        ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
    ];
    spell_row.extend_from_slice(spell);
    shard.insert_row("game_spell", &spell_row);
    let effect_id = packed_spell_effect_id(spell_id, 0);
    let mut effect_row: Vec<Field> = vec![
        ("id", &effect_id),
        ("spell_id", &spell_id),
        ("target", &T_TARGET_ENEMY),
    ];
    effect_row.extend_from_slice(effect);
    shard.insert_row("game_spell_effect", &effect_row);
}

fn cast_until(
    shard: &Standalone,
    caster: u64,
    spell_id: u32,
    target: u64,
    landed: impl Fn() -> bool,
) {
    for _ in 0..CAST_ATTEMPTS {
        shard.assert_call(
            "debug_force_cast_at",
            &[
                &caster.to_string(),
                &spell_id.to_string(),
                &target.to_string(),
            ],
        );
        if landed() {
            return;
        }
    }
    panic!("spell {spell_id} from {caster} did not land on {target} in {CAST_ATTEMPTS} casts");
}

fn assert_killed(shard: &Standalone, guid: u64) {
    let row = shard
        .query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0);
    assert_eq!(row["dead"], "true", "the Character must die: {row:?}");
    assert_eq!(row["health"], "0");
}

fn is_dead(shard: &Standalone, guid: u64) -> bool {
    shard.query_rows(&format!(
        "SELECT dead FROM game_world_entity WHERE guid = {guid}"
    ))[0]["dead"]
        == "true"
}

fn health(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard
            .query_rows(&format!(
                "SELECT health FROM game_world_entity WHERE guid = {guid}"
            ))
            .remove(0),
        "health",
    )
}
