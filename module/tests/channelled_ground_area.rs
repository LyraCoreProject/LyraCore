mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_CHANNELED, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, E_PERSISTENT_AREA, T_SELF,
    T_TARGET_ENEMY,
};
use support::{actor, number, Field, Row, Standalone};

const CHARACTER: u64 = 1;
const CHARACTER_HEALTH: u32 = 100_000;
const WOLF_HEALTH: u32 = 100_000;
const FIRE: u32 = 0x04;
const CHANNELLED_AREA: u32 = 5_123_401;
const PLAIN_AREA: u32 = 5_123_402;
const SHORT_CHANNELLED_AREA: u32 = 5_123_403;
const TIMED_AREA: u32 = 5_123_405;
const TIMED_CAST_MS: u32 = 10_000;
const OTHER_MAP: &str = "1";
const LETHAL_STRIKE: u32 = 5_123_404;
const AREA_DURATION_MS: u32 = 6_000;
const SHORT_DURATION_MS: u32 = 3_000;
const PERIOD_MS: u32 = 1_000;
const TICK_DAMAGE: i32 = 10;
// 50 ms scheduler ticks: just past three periods, and past the short area's expiry and reap.
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
fn a_channelled_ground_area_ends_when_its_caster_teleports_on_the_map() {
    let (shard, wolf) = stage("channelled-area-near-teleport");
    cast_area(&shard, CHANNELLED_AREA);
    assert_area_placed(&shard);

    teleport(&shard, None);

    assert_area_gone(&shard);
    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);
    assert_eq!(
        health(&shard, wolf),
        WOLF_HEALTH,
        "a teleported caster's channel deals no tick"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ground_area_without_a_channel_survives_a_teleport_on_the_map_and_ends_at_a_map_change() {
    let (shard, _) = stage("plain-area-teleports");
    cast_area(&shard, PLAIN_AREA);
    let plain = assert_area_placed(&shard);

    teleport(&shard, None);
    assert_eq!(
        assert_area_placed(&shard),
        plain,
        "a same-map teleport keeps the burn"
    );

    teleport(&shard, Some(OTHER_MAP));
    assert_area_gone(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ground_area_without_a_channel_ends_when_its_caster_enters_another_instance_of_the_map() {
    let (shard, _) = stage("plain-area-instance-change");
    cast_area(&shard, PLAIN_AREA);
    assert_area_placed(&shard);

    shard.assert_call("debug_create_fixture_instance", &[&CHARACTER.to_string()]);

    let instance = shard.query_rows(&format!(
        "SELECT instance_id FROM game_world_entity WHERE guid = {CHARACTER}"
    ))[0]["instance_id"]
        .clone();
    assert_ne!(instance, "0", "the caster must have entered a new Instance");
    assert_area_gone(&shard);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cast_in_progress_ends_when_its_caster_teleports() {
    let (shard, _) = stage("cast-teleport");
    begin_timed_cast(&shard);

    teleport(&shard, None);

    assert_eq!(pending_casts(&shard), 0, "a teleport interrupts the cast");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_logout_ends_the_casters_ground_areas() {
    let (shard, wolf) = stage("channelled-area-logout");
    cast_area(&shard, PLAIN_AREA);
    cast_area(&shard, CHANNELLED_AREA);
    assert_eq!(areas(&shard).len(), 2, "the burn and the channel's area");

    shard.assert_call("debug_logout_character", &[&CHARACTER.to_string()]);

    assert_area_gone(&shard);
    shard.assert_call("debug_advance_roll_batch", &[THREE_PERIOD_TICKS]);
    assert_eq!(
        health(&shard, wolf),
        WOLF_HEALTH,
        "a logged out caster's areas deal no tick"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_logout_ends_the_casters_cast_in_progress() {
    let (shard, _) = stage("cast-logout");
    begin_timed_cast(&shard);

    shard.assert_call("debug_logout_character", &[&CHARACTER.to_string()]);

    assert_eq!(pending_casts(&shard), 0, "a logout ends the cast");
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
    insert_spell(
        &shard,
        TIMED_AREA,
        &[
            ("duration_ms", &AREA_DURATION_MS),
            ("cast_time_ms", &TIMED_CAST_MS),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
        &[
            ("kind", &E_PERSISTENT_AREA),
            ("target", &T_SELF),
            ("base_points", &TICK_DAMAGE),
            ("period_ms", &PERIOD_MS),
            ("radius_yd", &10.0),
        ],
    );
    insert_area_spell(
        &shard,
        SHORT_CHANNELLED_AREA,
        SHORT_DURATION_MS,
        CAST_FLAG_CHANNELED,
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
            ("radius_yd", &10.0),
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

fn cast_area(shard: &Standalone, spell_id: u32) {
    let (x, y, z) = position(shard, CHARACTER);
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

// A same-map move by default; `map_id` sends the caster to that map instead.
fn teleport(shard: &Standalone, map_id: Option<&str>) {
    let row = shard
        .query_rows(&format!(
            "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    let (x, y, z) = (
        number::<f32>(&row, "x"),
        number::<f32>(&row, "y"),
        number::<f32>(&row, "z"),
    );
    shard.assert_call(
        "debug_teleport",
        &[
            &CHARACTER.to_string(),
            map_id.unwrap_or(&row["map_id"]),
            &(x + 3.0).to_string(),
            &y.to_string(),
            &z.to_string(),
            "0",
        ],
    );
}

fn begin_timed_cast(shard: &Standalone) {
    let (x, y, z) = position(shard, CHARACTER);
    shard.assert_call(
        "debug_cast_spell_at",
        &[
            &CHARACTER.to_string(),
            &TIMED_AREA.to_string(),
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
    assert_eq!(
        pending_casts(shard),
        1,
        "the timed cast must be in progress"
    );
}

fn pending_casts(shard: &Standalone) -> usize {
    shard
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {CHARACTER}"
        ))
        .len()
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
