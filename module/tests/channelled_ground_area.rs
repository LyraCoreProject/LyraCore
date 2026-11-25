mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spatial::grid_cell;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_CHANNELED, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, E_PERSISTENT_AREA, T_AREA_ENEMY,
    T_SELF, T_TARGET_ENEMY,
};
use support::{actor, number, Field, Row, Standalone};

const CHARACTER: u64 = 1;
const CHARACTER_HEALTH: u32 = 100_000;
const WOLF_HEALTH: u32 = 100_000;
const FIRE: u32 = 0x04;
const CHANNELLED_AREA: u32 = 5_123_401;
const PLAIN_AREA: u32 = 5_123_402;
const SHORT_CHANNELLED_AREA: u32 = 5_123_403;
const LETHAL_STRIKE: u32 = 5_123_404;
const BLIZZARD_LIKE_AREA: u32 = 5_123_405;
const BLIZZARD_RANGE_YD: u32 = 30;
const AREA_RADIUS_YD: f32 = 10.0;
const OUTSIDE_YD: f32 = 2.0 * AREA_RADIUS_YD;
const AREA_DURATION_MS: u32 = 6_000;
const SHORT_DURATION_MS: u32 = 3_000;
const PERIOD_MS: u32 = 1_000;
const TICK_DAMAGE: i32 = 10;
// 50 ms scheduler ticks: past one period, past three periods, past the short area's reap.
const ONE_PERIOD_TICKS: &str = "25";
const THREE_PERIOD_TICKS: &str = "61";
const PAST_SHORT_EXPIRY_TICKS: &str = "90";
// A cast at an equal level target misses at most 4% of the time.
const CAST_ATTEMPTS: usize = 20;
const CHARACTER_FACTION: u32 = 9_201;
const WOLF_FACTION: u32 = 9_202;
const START_FORWARD: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD;
const FORWARD: &str = "[1,0,0,0]";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_channelled_ground_area_ends_when_its_caster_moves() {
    let (shard, wolf) = stage("channelled-area-caster-moves");
    cast_area(&shard, CHANNELLED_AREA);
    assert_area_placed(&shard);

    step_forward(&shard);

    assert_area_gone(&shard);
    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);
    assert_eq!(
        health(&shard, wolf),
        WOLF_HEALTH,
        "a broken channel deals no tick"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_channelled_ground_area_ends_when_its_caster_dies() {
    let (shard, wolf) = stage("channelled-area-caster-dies");
    cast_area(&shard, CHANNELLED_AREA);
    assert_area_placed(&shard);

    for _ in 0..CAST_ATTEMPTS {
        shard.assert_call(
            "debug_force_cast_at",
            &[
                &wolf.to_string(),
                &LETHAL_STRIKE.to_string(),
                &CHARACTER.to_string(),
            ],
        );
        if is_dead(&shard, CHARACTER) {
            break;
        }
    }
    assert!(
        is_dead(&shard, CHARACTER),
        "the wolf's strike must kill the caster"
    );

    assert_area_gone(&shard);
    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);
    assert_eq!(
        health(&shard, wolf),
        WOLF_HEALTH,
        "a dead caster's area deals no tick"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_new_cast_replaces_the_casters_channelled_ground_area() {
    let (shard, _) = stage("channelled-area-new-cast");
    cast_area(&shard, CHANNELLED_AREA);
    let first = assert_area_placed(&shard);

    cast_area(&shard, CHANNELLED_AREA);

    let second = assert_area_placed(&shard);
    assert_ne!(
        second, first,
        "the new cast must end the first channel's area"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ground_area_without_a_channel_keeps_ticking_after_its_caster_casts_and_moves() {
    let (shard, wolf) = stage("plain-area-outlives-cast");
    cast_area(&shard, PLAIN_AREA);
    let plain = assert_area_placed(&shard);

    cast_area(&shard, CHANNELLED_AREA);
    step_forward(&shard);
    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);

    let ids: Vec<u64> = areas(&shard).iter().map(|row| number(row, "id")).collect();
    assert_eq!(
        ids,
        [plain],
        "only the burn outlives the next cast and the move"
    );
    assert!(
        health(&shard, wolf) < WOLF_HEALTH,
        "the burn keeps dealing damage"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_channelled_ground_area_deals_its_last_tick_and_removes_its_dynamic_object_at_expiry() {
    let (shard, wolf) = stage("channelled-area-expiry");
    cast_area(&shard, SHORT_CHANNELLED_AREA);
    assert_area_placed(&shard);
    let amount: u32 = number(&areas(&shard)[0], "amount");
    assert!(amount > 0, "the area must deal damage");

    shard.assert_call("debug_advance_roll_batch", &[PAST_SHORT_EXPIRY_TICKS]);

    let ticks = SHORT_DURATION_MS / PERIOD_MS;
    assert_eq!(health(&shard, wolf), WOLF_HEALTH - ticks * amount);
    assert_area_gone(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_unit_that_walks_into_a_ground_area_after_the_cast_takes_its_ticks() {
    let (shard, wolf) = stage("ground-area-walk-in");
    let (x, y, z) = position(&shard, CHARACTER);
    let centre = (x + OUTSIDE_YD, y);
    cast_area_at(&shard, BLIZZARD_LIKE_AREA, (centre.0, centre.1, z));
    assert_area_placed(&shard);

    shard.assert_call("debug_advance_roll_batch", &[ONE_PERIOD_TICKS]);
    assert_eq!(
        health(&shard, wolf),
        WOLF_HEALTH,
        "a unit outside the area takes no tick"
    );

    move_unit(&shard, wolf, centre);
    shard.assert_call("debug_advance_roll_batch", &[ONE_PERIOD_TICKS]);

    assert!(
        health(&shard, wolf) < WOLF_HEALTH,
        "a unit that walks in takes the next tick"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_unit_that_walks_out_of_a_ground_area_takes_no_more_ticks() {
    let (shard, wolf) = stage("ground-area-walk-out");
    let (x, y, z) = position(&shard, wolf);
    cast_area_at(&shard, BLIZZARD_LIKE_AREA, (x, y, z));
    assert_area_placed(&shard);
    shard.assert_call("debug_advance_roll_batch", &[ONE_PERIOD_TICKS]);
    let after_first_tick = health(&shard, wolf);
    assert!(
        after_first_tick < WOLF_HEALTH,
        "a unit inside the area takes its tick"
    );

    move_unit(&shard, wolf, (x + OUTSIDE_YD, y));
    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);

    assert_eq!(areas(&shard).len(), 1, "the area is still ticking");
    assert_eq!(
        health(&shard, wolf),
        after_first_tick,
        "a unit that walked out takes no more ticks"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creatures_ground_area_without_a_ground_point_lands_on_its_target() {
    let (shard, wolf) = stage("ground-area-creature-target");
    shard.assert_call(
        "debug_force_cast_at",
        &[
            &wolf.to_string(),
            &BLIZZARD_LIKE_AREA.to_string(),
            &CHARACTER.to_string(),
        ],
    );

    let rows = shard.query_rows(&format!(
        "SELECT x, y FROM game_ground_area WHERE caster_guid = {wolf}"
    ));
    assert_eq!(
        rows.len(),
        1,
        "the wolf must place one ground area: {rows:?}"
    );
    let area: (f32, f32) = (number(&rows[0], "x"), number(&rows[0], "y"));
    let (x, y, _) = position(&shard, CHARACTER);
    assert!(
        (area.0 - x).abs() < 0.01 && (area.1 - y).abs() < 0.01,
        "the area must land on the wolf's target, not on the wolf: {area:?}"
    );
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
         health = {CHARACTER_HEALTH}, max_health = {CHARACTER_HEALTH} WHERE guid = {CHARACTER}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    hostile_factions(&shard, wolf);
    insert_area_spell(
        &shard,
        CHANNELLED_AREA,
        AREA_DURATION_MS,
        CAST_FLAG_CHANNELED,
    );
    insert_area_spell(&shard, PLAIN_AREA, AREA_DURATION_MS, 0);
    insert_area_spell(
        &shard,
        SHORT_CHANNELLED_AREA,
        SHORT_DURATION_MS,
        CAST_FLAG_CHANNELED,
    );
    insert_spell(
        &shard,
        BLIZZARD_LIKE_AREA,
        &[
            ("duration_ms", &AREA_DURATION_MS),
            (
                "cast_flags",
                &(CAST_FLAG_IGNORE_RESISTANCES | CAST_FLAG_CHANNELED),
            ),
            ("range_yd", &BLIZZARD_RANGE_YD),
        ],
        &[
            ("kind", &E_PERSISTENT_AREA),
            ("target", &T_AREA_ENEMY),
            ("base_points", &TICK_DAMAGE),
            ("period_ms", &PERIOD_MS),
            ("radius_yd", &AREA_RADIUS_YD),
        ],
    );
    insert_spell(
        &shard,
        LETHAL_STRIKE,
        &[],
        &[
            ("kind", &E_DAMAGE),
            ("target", &T_TARGET_ENEMY),
            ("base_points", &1_000_000),
        ],
    );
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

fn insert_area_spell(shard: &Standalone, spell_id: u32, duration_ms: u32, channel: u32) {
    let cast_flags = CAST_FLAG_IGNORE_RESISTANCES | channel;
    insert_spell(
        shard,
        spell_id,
        &[("duration_ms", &duration_ms), ("cast_flags", &cast_flags)],
        &[
            ("kind", &E_PERSISTENT_AREA),
            ("target", &T_SELF),
            ("base_points", &TICK_DAMAGE),
            ("period_ms", &PERIOD_MS),
            ("radius_yd", &AREA_RADIUS_YD),
        ],
    );
}

// A zero range skips the range check and the projectile flight, so a direct hit lands in its cast.
fn insert_spell(shard: &Standalone, spell_id: u32, spell: &[Field], effect: &[Field]) {
    let name = format!("Channelled ground area fixture {spell_id}");
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
    spell_row.retain(|(column, _)| !spell.iter().any(|(given, _)| given == column));
    spell_row.extend_from_slice(spell);
    shard.insert_row("game_spell", &spell_row);
    let effect_id = packed_spell_effect_id(spell_id, 0);
    let mut effect_row: Vec<Field> = vec![("id", &effect_id), ("spell_id", &spell_id)];
    effect_row.extend_from_slice(effect);
    shard.insert_row("game_spell_effect", &effect_row);
}

fn cast_area(shard: &Standalone, spell_id: u32) {
    cast_area_at(shard, spell_id, position(shard, CHARACTER));
}

fn cast_area_at(shard: &Standalone, spell_id: u32, (x, y, z): (f32, f32, f32)) {
    shard.assert_call(
        "debug_cast_spell_at",
        &[
            &CHARACTER.to_string(),
            &spell_id.to_string(),
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
}

// The tick finds units by their grid cell, so a move must update it with the position.
fn move_unit(shard: &Standalone, guid: u64, (x, y): (f32, f32)) {
    let (grid_x, grid_y) = grid_cell(x, y);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {x}, y = {y}, grid_x = {grid_x}, grid_y = {grid_y} \
         WHERE guid = {guid}"
    ));
}

fn step_forward(shard: &Standalone) {
    let (x, y, z) = position(shard, CHARACTER);
    let args = [
        actor(CHARACTER),
        START_FORWARD.to_string(),
        FORWARD.to_string(),
        (x + 3.0).to_string(),
        y.to_string(),
        z.to_string(),
        "0".to_string(),
        "10".to_string(),
    ];
    shard.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(position(shard, CHARACTER).0, x + 3.0, "the move must land");
}

/// Returns the id of the caster's one ground area, after checking its DynamicObject exists.
fn assert_area_placed(shard: &Standalone) -> u64 {
    let rows = areas(shard);
    assert_eq!(
        rows.len(),
        1,
        "the caster must hold one ground area: {rows:?}"
    );
    assert_eq!(
        dynamic_objects(shard).len(),
        1,
        "the ground area must show its DynamicObject"
    );
    number(&rows[0], "id")
}

fn assert_area_gone(shard: &Standalone) {
    assert!(areas(shard).is_empty(), "the ground area must end");
    assert!(
        dynamic_objects(shard).is_empty(),
        "the ground area's DynamicObject must go with it"
    );
}

fn areas(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT id, amount FROM game_ground_area WHERE caster_guid = {CHARACTER}"
    ))
}

fn dynamic_objects(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT guid FROM game_dynamic_object WHERE caster_guid = {CHARACTER}"
    ))
}

fn position(shard: &Standalone, guid: u64) -> (f32, f32, f32) {
    let row = shard
        .query_rows(&format!(
            "SELECT x, y, z FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0);
    (number(&row, "x"), number(&row, "y"), number(&row, "z"))
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

fn is_dead(shard: &Standalone, guid: u64) -> bool {
    shard.query_rows(&format!(
        "SELECT dead FROM game_world_entity WHERE guid = {guid}"
    ))[0]["dead"]
        == "true"
}
