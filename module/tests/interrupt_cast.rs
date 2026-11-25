mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_PERIODIC_DAMAGE, CAST_FLAG_CHANNELED, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, E_INTERRUPT,
    E_PERSISTENT_AREA, T_SELF, T_TARGET_ENEMY,
};
use support::{number, Field, Row, Standalone};

const CHARACTER: u64 = 1;
const FIRE: u8 = 0x04;
const CHANNEL_AURA: u32 = 5_124_101;
const CHANNELLED_AREA: u32 = 5_124_102;
const SLOW_CAST: u32 = 5_124_103;
const KICK: u32 = 5_124_104;
const KICK_LOCKOUT_MS: u32 = 5_000;
const CHANNEL_DURATION_MS: u32 = 6_000;
const PERIOD_MS: u32 = 1_000;
// A cast at an equal level target misses at most 4% of the time.
const CAST_ATTEMPTS: usize = 20;
const CHARACTER_FACTION: u32 = 9_301;
const WOLF_FACTION: u32 = 9_302;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_ends_a_channel_aura_and_locks_its_school() {
    let (shard, wolf) = stage("interrupt-channel-aura");
    cast_until(&shard, CHARACTER, CHANNEL_AURA, wolf, || {
        !channel_auras(&shard).is_empty()
    });
    assert!(!channel_auras(&shard).is_empty(), "the channel must start");

    cast_until(&shard, wolf, KICK, CHARACTER, || {
        channel_auras(&shard).is_empty()
    });

    assert!(
        channel_auras(&shard).is_empty(),
        "the interrupt must end the channel"
    );
    assert_eq!(locked_schools(&shard), [FIRE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_ends_a_channelled_ground_area_and_locks_its_school() {
    let (shard, wolf) = stage("interrupt-channelled-area");
    cast_area(&shard);
    assert_eq!(areas(&shard).len(), 1, "the channelled area must be placed");

    cast_until(&shard, wolf, KICK, CHARACTER, || areas(&shard).is_empty());

    assert!(
        areas(&shard).is_empty(),
        "the interrupt must end the channelled area"
    );
    assert_eq!(locked_schools(&shard), [FIRE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_cancels_a_cast_in_progress_and_locks_its_school() {
    let (shard, wolf) = stage("interrupt-cast-in-progress");
    shard.assert_call(
        "debug_begin_cast",
        &[
            &CHARACTER.to_string(),
            &SLOW_CAST.to_string(),
            &wolf.to_string(),
        ],
    );
    assert_eq!(
        pending_casts(&shard).len(),
        1,
        "the cast must be in progress"
    );

    cast_until(&shard, wolf, KICK, CHARACTER, || {
        pending_casts(&shard).is_empty()
    });

    assert!(
        pending_casts(&shard).is_empty(),
        "the interrupt must cancel the cast"
    );
    assert_eq!(locked_schools(&shard), [FIRE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_on_a_unit_that_is_not_casting_writes_no_lockout() {
    let (shard, wolf) = stage("interrupt-idle-unit");

    for _ in 0..5 {
        shard.assert_call(
            "debug_force_cast_at",
            &[&wolf.to_string(), &KICK.to_string(), &CHARACTER.to_string()],
        );
    }

    assert!(locked_schools(&shard).is_empty());
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
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
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
         health = 100000, max_health = 100000 WHERE guid = {CHARACTER}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, \
         health = 100000, max_health = 100000 WHERE guid = {wolf}"
    ));
    hostile_factions(&shard, wolf);
    insert_spell(
        &shard,
        CHANNEL_AURA,
        &[
            ("duration_ms", &CHANNEL_DURATION_MS),
            (
                "cast_flags",
                &(CAST_FLAG_IGNORE_RESISTANCES | CAST_FLAG_CHANNELED),
            ),
        ],
        &[
            ("kind", &A_PERIODIC_DAMAGE),
            ("target", &T_TARGET_ENEMY),
            ("base_points", &10),
            ("period_ms", &PERIOD_MS),
        ],
    );
    insert_spell(
        &shard,
        CHANNELLED_AREA,
        &[
            ("duration_ms", &CHANNEL_DURATION_MS),
            (
                "cast_flags",
                &(CAST_FLAG_IGNORE_RESISTANCES | CAST_FLAG_CHANNELED),
            ),
        ],
        &[
            ("kind", &E_PERSISTENT_AREA),
            ("target", &T_SELF),
            ("base_points", &10),
            ("period_ms", &PERIOD_MS),
            ("radius_yd", &10.0),
        ],
    );
    insert_spell(
        &shard,
        SLOW_CAST,
        &[("cast_time_ms", &10_000)],
        &[
            ("kind", &E_DAMAGE),
            ("target", &T_TARGET_ENEMY),
            ("base_points", &10),
        ],
    );
    insert_spell(
        &shard,
        KICK,
        &[("duration_ms", &KICK_LOCKOUT_MS)],
        &[("kind", &E_INTERRUPT), ("target", &T_TARGET_ENEMY)],
    );
    (shard, wolf)
}

// An enemy spell picks only hostile units, and the bare Standalone has no faction rows.
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

// A zero range skips the range check and the projectile flight, so a direct hit lands in its cast.
fn insert_spell(shard: &Standalone, spell_id: u32, spell: &[Field], effect: &[Field]) {
    let name = format!("Interrupt fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("school_mask", &FIRE),
        ("range_yd", &0),
        ("is_negative", &true),
        ("gcd_ms", &0),
        ("cooldown_ms", &0),
    ];
    if !spell.iter().any(|(column, _)| *column == "cast_flags") {
        spell_row.push(("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES));
    }
    spell_row.extend_from_slice(spell);
    shard.insert_row("game_spell", &spell_row);
    let effect_id = packed_spell_effect_id(spell_id, 0);
    let mut effect_row: Vec<Field> = vec![("id", &effect_id), ("spell_id", &spell_id)];
    effect_row.extend_from_slice(effect);
    shard.insert_row("game_spell_effect", &effect_row);
}

// A miss changes nothing, so a cast repeats until its outcome shows.
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
}

fn cast_area(shard: &Standalone) {
    let row = shard
        .query_rows(&format!(
            "SELECT x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    shard.assert_call(
        "debug_cast_spell_at",
        &[
            &CHARACTER.to_string(),
            &CHANNELLED_AREA.to_string(),
            "0",
            &number::<f32>(&row, "x").to_string(),
            &number::<f32>(&row, "y").to_string(),
            &number::<f32>(&row, "z").to_string(),
        ],
    );
}

fn channel_auras(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT id FROM game_aura WHERE caster_guid = {CHARACTER} AND spell_id = {CHANNEL_AURA}"
    ))
}

fn areas(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT id FROM game_ground_area WHERE caster_guid = {CHARACTER}"
    ))
}

fn pending_casts(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {CHARACTER}"
    ))
}

fn locked_schools(shard: &Standalone) -> Vec<u8> {
    shard
        .query_rows(&format!(
            "SELECT school FROM game_school_lockout WHERE caster_guid = {CHARACTER}"
        ))
        .iter()
        .map(|row| number(row, "school"))
        .collect()
}
