mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::{player_flags, unit_flags};
use lyracore_shared::spell::taxonomy::{
    A_MOD_SPEED, A_PERIODIC_DAMAGE, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, E_PERSISTENT_AREA,
    T_TARGET_ENEMY,
};
use support::{actor, number, Field, Standalone};

const CHARACTER: u64 = 1;
const CHARACTER_HEALTH: u32 = 100;
const LETHAL_DAMAGE: u32 = 1_000;
const FIRE: u32 = 0x04;
const DIRECT_FIXTURE: u32 = 5_123_301;
const PERIODIC_FIXTURE: u32 = 5_123_302;
const GROUND_AREA_FIXTURE: u32 = 5_123_303;
const STRIKE_AND_SLOW_FIXTURE: u32 = 5_123_304;
const PERIOD_MS: u32 = 1_000;
// 50 ms scheduler ticks that pass three periods.
const THREE_PERIOD_TICKS: &str = "61";
// A cast at an equal level target misses at most 4% of the time.
const CAST_ATTEMPTS: usize = 20;
const CHARACTER_FACTION: u32 = 9_201;
const WOLF_FACTION: u32 = 9_202;
const DUEL: u64 = 509_0002;
// `state::ACTIVE` in `module/src/duel.rs`.
const ACTIVE: u8 = 2;
const MAIN_HAND_SLOT: u8 = 15;
const BLADE_DURABILITY: u32 = 100;
const FALL_LAND: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND;
// `fall_damage_curve` in module/src/world.rs: a 1.5 s fall costs 14 of 100 health.
const FALL_MS: u32 = 1_500;
const FALL_DAMAGE: u32 = 14;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_spell_kills_a_character_with_the_death_durability_loss() {
    let (shard, wolf) = stage("creature-spell-kills-character");
    equip_blade(&shard);
    insert_direct_spell(&shard);

    cast_until(&shard, wolf, DIRECT_FIXTURE, CHARACTER, || {
        is_dead(&shard, CHARACTER)
    });

    assert_killed(&shard, CHARACTER);
    assert_eq!(
        blade_durability(&shard),
        BLADE_DURABILITY * 9 / 10,
        "a death to a creature costs ten percent of the maximum"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_kill_by_a_characters_pet_costs_no_durability() {
    let (shard, wolf) = stage("pet-spell-kills-character");
    let owner = spawn_rival(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET owner_guid = {owner} WHERE guid = {wolf}"
    ));
    let flags = &shard.query_rows(&format!(
        "SELECT unit_flags, player_flags FROM game_world_entity WHERE guid = {CHARACTER}"
    ))[0];
    let unit: u32 = flags["unit_flags"].parse().unwrap();
    let player: u32 = flags["player_flags"].parse().unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {}, player_flags = {} WHERE guid = {CHARACTER}",
        unit | unit_flags::PVP,
        player | player_flags::PVP_DESIRED
    ));
    equip_blade(&shard);
    insert_direct_spell(&shard);

    cast_until(&shard, wolf, DIRECT_FIXTURE, CHARACTER, || {
        is_dead(&shard, CHARACTER)
    });

    assert_killed(&shard, CHARACTER);
    assert_eq!(
        blade_durability(&shard),
        BLADE_DURABILITY,
        "vmangos Unit::Kill spares the loss when a Character controls the killer"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_debuff_after_a_lethal_hit_in_the_same_cast_stays_off_the_corpse() {
    let (shard, wolf) = stage("lethal-cast-debuff-skips-corpse");
    insert_spell(
        &shard,
        STRIKE_AND_SLOW_FIXTURE,
        &[("duration_ms", &10_000)],
        &[("kind", &E_DAMAGE), ("base_points", &LETHAL_DAMAGE)],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(STRIKE_AND_SLOW_FIXTURE, 1)),
            ("spell_id", &STRIKE_AND_SLOW_FIXTURE),
            ("effect_index", &1),
            ("kind", &A_MOD_SPEED),
            ("base_points", &-50),
            ("target", &T_TARGET_ENEMY),
        ],
    );

    cast_until(&shard, wolf, STRIKE_AND_SLOW_FIXTURE, CHARACTER, || {
        is_dead(&shard, CHARACTER)
    });

    assert_killed(&shard, CHARACTER);
    assert!(shard
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {CHARACTER} \
             AND spell_id = {STRIKE_AND_SLOW_FIXTURE}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_damage_tick_kills_a_character() {
    let (shard, wolf) = stage("periodic-tick-kills-character");
    apply_lethal_periodic_damage(&shard, wolf);

    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);

    assert_killed(&shard, CHARACTER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_damage_tick_from_a_dead_caster_still_kills_a_character() {
    let (shard, wolf) = stage("periodic-tick-dead-caster-kills-character");
    apply_lethal_periodic_damage(&shard, wolf);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true WHERE guid = {wolf}"
    ));

    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);

    assert_killed(&shard, CHARACTER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_damage_tick_from_a_gone_caster_leaves_the_character_alive() {
    let (shard, wolf) = stage("periodic-tick-gone-caster-spares-character");
    apply_lethal_periodic_damage(&shard, wolf);
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {wolf}"
    ));

    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);

    assert_eq!(health(&shard, CHARACTER), CHARACTER_HEALTH);
    assert!(!is_dead(&shard, CHARACTER));
    assert_eq!(
        periodic_aura_ids(&shard).len(),
        1,
        "the effect idles until it expires"
    );
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

    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);

    assert_killed(&shard, CHARACTER);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_lethal_spell_from_a_duel_opponent_ends_the_duel_at_one_health() {
    let (shard, _) = stage("duel-spell-ends-at-one-health");
    let rival = spawn_rival(&shard);
    start_duel(&shard, rival);
    insert_direct_spell(&shard);

    cast_until(&shard, rival, DIRECT_FIXTURE, CHARACTER, || {
        duel_ended(&shard)
    });

    assert_duel_loser_at_one_health(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_lethal_spell_from_a_duel_opponents_pet_ends_the_duel_at_one_health() {
    let (shard, wolf) = stage("duel-pet-spell-ends-at-one-health");
    let rival = spawn_rival(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET owner_guid = {rival} WHERE guid = {wolf}"
    ));
    start_duel(&shard, rival);
    insert_direct_spell(&shard);

    cast_until(&shard, wolf, DIRECT_FIXTURE, CHARACTER, || {
        duel_ended(&shard)
    });

    assert_duel_loser_at_one_health(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fall_that_leaves_a_duelist_at_one_health_ends_the_duel() {
    let (shard, _) = stage("duel-fall-ends-at-one-health");
    let rival = spawn_rival(&shard);
    start_duel(&shard, rival);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {} WHERE guid = {CHARACTER}",
        FALL_DAMAGE + 1
    ));

    land(&shard, FALL_MS);

    assert!(duel_ended(&shard), "the fall must end the Duel");
    assert_duel_loser_at_one_health(&shard);
}

fn stage(name: &str) -> (Standalone, u64) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
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

/// A second level 60 Character one yard from the first, in the first Character's faction.
fn spawn_rival(shard: &Standalone) -> u64 {
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
    let at = shard
        .query_rows(&format!(
            "SELECT x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    let (x, y, z): (f32, f32, f32) = (number(&at, "x"), number(&at, "y"), number(&at, "z"));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, x = {}, y = {y}, z = {z}, \
         level = 60, faction_template = {CHARACTER_FACTION} WHERE guid = {rival}",
        x + 1.0
    ));
    rival
}

fn start_duel(shard: &Standalone, rival: u64) {
    let at = shard
        .query_rows(&format!(
            "SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    shard.insert_row(
        "game_duel",
        &[
            ("id", &DUEL),
            ("flag_entry", &21680),
            ("initiator_guid", &rival),
            ("challenged_guid", &CHARACTER),
            ("map_id", &at["map_id"]),
            ("instance_id", &at["instance_id"]),
            ("flag_x", &at["x"]),
            ("flag_y", &at["y"]),
            ("flag_z", &at["z"]),
            ("state", &ACTIVE),
        ],
    );
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

// A movement body with no transport, swimming or jumping tail: flags, timestamp, pose and fall time.
fn land(shard: &Standalone, fall_time_ms: u32) {
    let at = shard
        .query_rows(&format!(
            "SELECT x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    let [x, y, z]: [f32; 3] = ["x", "y", "z"].map(|axis| number(&at, axis));
    let mut body = vec![0u8; 24];
    body.extend_from_slice(&fall_time_ms.to_le_bytes());
    let body = format!(
        "[{}]",
        body.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
    );
    let args = [
        actor(CHARACTER),
        FALL_LAND.to_string(),
        body,
        x.to_string(),
        y.to_string(),
        z.to_string(),
        "1.0".to_string(),
        "10".to_string(),
    ];
    shard.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn equip_blade(shard: &Standalone) {
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_durability = {BLADE_DURABILITY} \
         WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_call(
        "debug_equip_weapon",
        &[&CHARACTER.to_string(), &FIXTURE_BLADE.to_string()],
    );
    assert_eq!(blade_durability(shard), BLADE_DURABILITY);
}

fn blade_durability(shard: &Standalone) -> u32 {
    number(
        &shard
            .query_rows(&format!(
                "SELECT durability FROM game_item_instance \
                 WHERE owner_guid = {CHARACTER} AND slot = {MAIN_HAND_SLOT}"
            ))
            .remove(0),
        "durability",
    )
}

fn insert_direct_spell(shard: &Standalone) {
    insert_spell(
        shard,
        DIRECT_FIXTURE,
        &[],
        &[("kind", &E_DAMAGE), ("base_points", &LETHAL_DAMAGE)],
    );
}

// A zero range skips the range check and the projectile flight, so a direct hit lands in its cast.
fn insert_spell(shard: &Standalone, spell_id: u32, spell: &[Field], effect: &[Field]) {
    let name = format!("Character damage death fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("school_mask", &FIRE),
        ("range_yd", &0),
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

fn apply_lethal_periodic_damage(shard: &Standalone, wolf: u64) {
    insert_spell(
        shard,
        PERIODIC_FIXTURE,
        &[("duration_ms", &30_000)],
        &[
            ("kind", &A_PERIODIC_DAMAGE),
            ("base_points", &LETHAL_DAMAGE),
            ("period_ms", &PERIOD_MS),
        ],
    );
    cast_until(shard, wolf, PERIODIC_FIXTURE, CHARACTER, || {
        !periodic_aura_ids(shard).is_empty()
    });
    assert_eq!(health(shard, CHARACTER), CHARACTER_HEALTH);
}

fn periodic_aura_ids(shard: &Standalone) -> Vec<support::Row> {
    shard.query_rows(&format!(
        "SELECT id FROM game_aura WHERE target_guid = {CHARACTER} \
         AND spell_id = {PERIODIC_FIXTURE}"
    ))
}

fn duel_ended(shard: &Standalone) -> bool {
    shard
        .query_rows(&format!("SELECT id FROM game_duel WHERE id = {DUEL}"))
        .is_empty()
}

fn assert_duel_loser_at_one_health(shard: &Standalone) {
    let loser = shard
        .query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    assert_eq!(loser["dead"], "false", "the Duel loser must survive");
    assert_eq!(loser["health"], "1");
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
