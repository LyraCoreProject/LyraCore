//! Distract against a real shard: the ground destination Gate, the Distraction each idle Creature
//! near the point receives, and the creature behavior cycle holding and resuming its idle movement.
//! The spell rows carry the client data for spell 1725: one effect 69 at the enemies around the
//! point, 10 yd radius, amount 10, 30 yd range, 30 energy, 30 s cooldown and stealth safe.

mod support;

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::time::Duration;

use lyracore_shared::constants::unit_flags;
use support::{poll_until, Standalone};

type Row = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const TEST_WOLF_ENTRY: u32 = 51000;
const DISTRACT: u32 = 1725;
const FIXTURE_STEALTH: u32 = 509_0700;

const E_DISTRACT: u32 = 0x25;
const A_STEALTH: u32 = 0xA8;
const T_SELF: u32 = 0;
const T_AREA_ENEMY: u32 = 4;
const SPELL_ATTR_STEALTH_SAFE: u32 = 0x0004;
const POWER_ENERGY: u32 = 3;
const MOVEMENT_RANDOM: u32 = 1;

const DISTRACT_MS: u64 = 10_000;
const ENERGY: u32 = 100;
const COST: u32 = 30;

/// The player's position and facing. Creatures spawn along the facing; the point sits 20 yd ahead
/// and 3 yd to the side, so facing the point differs from the spawn orientation.
#[derive(Clone, Copy)]
struct Line {
    x: f32,
    y: f32,
    z: f32,
    o: f32,
}

impl Line {
    fn at(self, ahead: f32, side: f32) -> (f32, f32, f32) {
        let (sin, cos) = self.o.sin_cos();
        (
            self.x + ahead * cos - side * sin,
            self.y + ahead * sin + side * cos,
            self.z,
        )
    }
}

fn number<T: std::str::FromStr>(row: &Row, column: &str) -> T {
    row[column]
        .parse()
        .unwrap_or_else(|_| panic!("{column} is not a number in {row:?}"))
}

fn same_angle(a: f32, b: f32) -> bool {
    let d = (a - b).rem_euclid(TAU);
    d < 1e-3 || TAU - d < 1e-3
}

fn heading(from: (f32, f32), to: (f32, f32, f32)) -> f32 {
    (to.1 - from.1).atan2(to.0 - from.0)
}

/// A fresh shard with a level 60 player, so the level 1 Test Wolves never aggro it, and the
/// Distract rows staged.
fn shard(name: &str) -> (Standalone, Line) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    shard.assert_call("debug_set_level", &[&PLAYER.to_string(), "60"]);
    shard.assert_call(
        "debug_set_power",
        &[&PLAYER.to_string(), &ENERGY.to_string()],
    );
    insert_spell(
        &shard,
        DISTRACT,
        "Distract",
        POWER_ENERGY,
        COST,
        30_000,
        30,
        0,
        SPELL_ATTR_STEALTH_SAFE,
    );
    insert_effect(&shard, DISTRACT, E_DISTRACT, 10, 1, T_AREA_ENEMY, 10.0);
    seed_hostile_factions(&shard);
    let player = entity(&shard, PLAYER);
    let line = Line {
        x: number(&player, "x"),
        y: number(&player, "y"),
        z: number(&player, "z"),
        o: number(&player, "orientation"),
    };
    (shard, line)
}

#[allow(clippy::too_many_arguments)]
fn insert_spell(
    shard: &Standalone,
    spell_id: u32,
    name: &str,
    power_type: u32,
    cost: u32,
    cooldown_ms: u32,
    range_yd: u32,
    duration_ms: u32,
    cast_flags: u32,
) {
    shard.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges) \
         VALUES ({spell_id}, '{name}', {power_type}, {cost}, 0, 0, {cooldown_ms}, {range_yd}, \
         {duration_ms}, 1, 0, 0, 0, 0, 0, 22, 0, false, {cast_flags}, 0, 8, 0, 0, 0, 0)"
    ));
}

fn insert_effect(
    shard: &Standalone,
    spell_id: u32,
    kind: u32,
    base_points: i32,
    die_sides: i32,
    target: u32,
    radius_yd: f32,
) {
    let id = (spell_id as u64) << 2;
    shard.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat) \
         VALUES ({id}, {spell_id}, 0, {kind}, {base_points}, {die_sides}, 0.0, 0, {target}, \
         {radius_yd}, 0, 0, 0, 0, 0, 0, 0, false)"
    ));
}

/// The player's template 1 and the Test Wolf's template 14, as their FactionTemplate.dbc group
/// masks: player 1, alliance 2, horde 4, monster 8. The area selection only turns hostile units.
fn seed_hostile_factions(shard: &Standalone) {
    for (id, faction, group, friends, enemies) in [(1, 1, 3, 2, 12), (14, 16, 8, 0, 1)] {
        shard.assert_sql(&format!(
            "DELETE FROM game_faction_template WHERE id = {id}"
        ));
        shard.assert_sql(&format!(
            "INSERT INTO game_faction_template (id, faction, faction_group, friend_group, \
             enemy_group, enemy_0, enemy_1, enemy_2, enemy_3, friend_0, friend_1, friend_2, \
             friend_3) VALUES ({id}, {faction}, {group}, {friends}, {enemies}, 0, 0, 0, 0, 0, 0, \
             0, 0)"
        ));
    }
}

/// A Test Wolf `ahead` yards along the player's facing, facing the same way.
fn spawn_wolf(shard: &Standalone, ahead: f32) -> u64 {
    shard.assert_call(
        "debug_spawn_at_feet",
        &[
            &PLAYER.to_string(),
            &TEST_WOLF_ENTRY.to_string(),
            &ahead.to_string(),
        ],
    );
    shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist")
}

fn entity(shard: &Standalone, guid: u64) -> Row {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        ))
        .pop()
        .expect("the entity must be live")
}

fn distraction(shard: &Standalone, guid: u64) -> Option<u64> {
    shard
        .query_rows(&format!(
            "SELECT ends_ms FROM game_creature_distraction WHERE creature_guid = {guid}"
        ))
        .pop()
        .map(|row| number(&row, "ends_ms"))
}

fn spline(shard: &Standalone, guid: u64) -> Option<Row> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_creature_spline WHERE guid = {guid}"
        ))
        .pop()
}

fn cast_at(shard: &Standalone, point: (f32, f32, f32)) -> std::process::Output {
    shard.call(
        "debug_cast_spell_at",
        &[
            &PLAYER.to_string(),
            &DISTRACT.to_string(),
            "0",
            &point.0.to_string(),
            &point.1.to_string(),
            &point.2.to_string(),
        ],
    )
}

fn assert_refused(output: &std::process::Output, reason: &str) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success() && text.contains(reason), "{text}");
}

/// Keep a unit flagged in combat past the test, so the combat-drop pass never clears it.
fn hold_in_combat(shard: &Standalone, guid: u64) {
    let flags: u32 = number(&entity(shard, guid), "unit_flags");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {}, combat_until_ms = {} WHERE guid = {guid}",
        flags | unit_flags::IN_COMBAT,
        u64::MAX,
    ));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn assert_orientation(shard: &Standalone, guid: u64, angle: f32, when: &str) {
    let orientation: f32 = number(&entity(shard, guid), "orientation");
    assert!(
        same_angle(orientation, angle),
        "{when}: {guid} faces {orientation}, expected {angle}"
    );
}

/// The stored orientation and the facing row the Gateway relays. The next spline advance reaps a
/// zero-duration row, so only a shard without the creature tick can read it reliably.
fn assert_faces(shard: &Standalone, guid: u64, angle: f32, when: &str) {
    assert_orientation(shard, guid, angle, when);
    let leg = spline(shard, guid).unwrap_or_else(|| panic!("{when}: {guid} has no spline row"));
    assert_eq!(leg["facing"], "true", "{when}: {leg:?}");
    assert!(
        same_angle(number(&leg, "facing_angle"), angle),
        "{when}: facing row {leg:?}, expected {angle}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn distract_turns_only_the_idle_creatures_near_the_point_and_keeps_the_rogue_hidden() {
    let (shard, line) = shard("distract-cast");
    // No creature tick: nothing reaps the facing rows, and nothing else moves the wolves.
    shard.assert_sql("DELETE FROM game_creature_move_schedule");
    let point = line.at(20.0, 3.0);
    let inside = spawn_wolf(&shard, 14.0);
    let fighting = spawn_wolf(&shard, 18.0);
    let outside = spawn_wolf(&shard, 32.0);
    // The wolf fights; the player's flag only keeps regeneration off its exact energy readings.
    hold_in_combat(&shard, fighting);
    hold_in_combat(&shard, PLAYER);

    insert_spell(
        &shard,
        FIXTURE_STEALTH,
        "Fixture Stealth",
        0,
        0,
        0,
        0,
        600_000,
        0,
    );
    insert_effect(&shard, FIXTURE_STEALTH, A_STEALTH, 0, 0, T_SELF, 0.0);
    shard.assert_call(
        "debug_force_cast_at",
        &[
            &PLAYER.to_string(),
            &FIXTURE_STEALTH.to_string(),
            &PLAYER.to_string(),
        ],
    );
    let stealth =
        format!("SELECT id FROM game_aura WHERE target_guid = {PLAYER} AND eff_kind = {A_STEALTH}");
    assert_eq!(
        shard.query_rows(&stealth).len(),
        1,
        "the fixture stealth must land"
    );

    // Both Refusals come first: they spend nothing and start no cooldown.
    assert_refused(
        &shard.call(
            "debug_force_cast_at",
            &[
                &PLAYER.to_string(),
                &DISTRACT.to_string(),
                &PLAYER.to_string(),
            ],
        ),
        "only target a ground point",
    );
    assert_refused(&cast_at(&shard, line.at(40.0, 0.0)), "out of range");
    assert_eq!(number::<u32>(&entity(&shard, PLAYER), "power"), ENERGY);

    let orientations: Vec<f32> = [fighting, outside]
        .iter()
        .map(|&guid| number(&entity(&shard, guid), "orientation"))
        .collect();
    let cast_ms = now_ms();
    let cast = cast_at(&shard, point);
    assert!(cast.status.success(), "{cast:?}");

    let ends_ms = distraction(&shard, inside).expect("the idle wolf near the point is distracted");
    assert!(
        ends_ms >= cast_ms + DISTRACT_MS && ends_ms <= now_ms() + DISTRACT_MS,
        "the Distraction lasts the effect amount, 10 s: ends {ends_ms}, cast at {cast_ms}"
    );
    let at = entity(&shard, inside);
    assert_faces(
        &shard,
        inside,
        heading((number(&at, "x"), number(&at, "y")), point),
        "after the cast",
    );
    for (guid, before) in [fighting, outside].into_iter().zip(orientations) {
        assert_eq!(
            distraction(&shard, guid),
            None,
            "{guid} must not be distracted"
        );
        assert_eq!(number::<f32>(&entity(&shard, guid), "orientation"), before);
    }

    assert_eq!(
        number::<u32>(&entity(&shard, PLAYER), "power"),
        ENERGY - COST
    );
    assert_eq!(
        shard.query_rows(&stealth).len(),
        1,
        "Distract is stealth safe"
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_spell_cd WHERE caster_guid = {PLAYER} AND spell_id = {DISTRACT}"
            ))
            .len(),
        1,
        "the cast starts its cooldown"
    );
    for guid in [inside, outside] {
        assert!(shard
            .query_rows(&format!(
                "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {guid}"
            ))
            .is_empty());
        assert!(shard
            .query_rows(&format!(
                "SELECT id FROM game_threat WHERE creature_guid = {guid}"
            ))
            .is_empty());
    }

    // A second Distract refreshes the expiry and faces the second point.
    std::thread::sleep(Duration::from_millis(1_500));
    shard.assert_sql(&format!(
        "DELETE FROM game_spell_cd WHERE caster_guid = {PLAYER}"
    ));
    let second_point = line.at(16.0, -4.0);
    let second = cast_at(&shard, second_point);
    assert!(second.status.success(), "{second:?}");
    let refreshed = distraction(&shard, inside).expect("the wolf stays distracted");
    assert!(
        refreshed >= ends_ms + 1_500,
        "{refreshed} must move past {ends_ms}"
    );
    let at = entity(&shard, inside);
    assert_faces(
        &shard,
        inside,
        heading((number(&at, "x"), number(&at, "y")), second_point),
        "after the second cast",
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_distraction_holds_idle_movement_until_its_expiry_and_ends_on_an_engagement() {
    let (shard, line) = shard("distract-cycle");
    let point = line.at(20.0, 3.0);

    let patroller = spawn_wolf(&shard, 16.0);
    for (id, ahead) in [(509_0701u64, 16.0f32), (509_0702, 24.0)] {
        let (x, y, z) = line.at(ahead, 0.0);
        shard.assert_sql(&format!(
            "DELETE FROM game_creature_waypoint WHERE id = {id}"
        ));
        shard.assert_sql(&format!(
            "INSERT INTO game_creature_waypoint (id, creature_guid, x, y, z) \
             VALUES ({id}, {patroller}, {x}, {y}, {z})"
        ));
    }
    let wanderer = spawn_wolf(&shard, 20.0);
    shard.assert_sql(&format!(
        "UPDATE game_creature_spawn SET movement_type = {MOVEMENT_RANDOM} WHERE guid = {wanderer}"
    ));
    let idle = spawn_wolf(&shard, 14.0);
    let spawn_orientation = line.o;

    // Cast while both the patroller and the wanderer walk a leg. A wander leg starts on about one
    // sense firing in three, so this can take several firings.
    let walking = |guid: u64| {
        spline(&shard, guid).is_some_and(|leg| {
            let ends_micros = number::<u64>(&leg, "start_micros")
                + number::<u64>(&leg, "dur_ms") * 1_000
                - 600_000;
            leg["facing"] == "false" && now_ms() * 1_000 < ends_micros
        })
    };
    assert!(
        poll_until(Duration::from_secs(60), || walking(patroller)
            && walking(wanderer)),
        "the patroller and the wanderer must both walk a leg"
    );
    let cast = cast_at(&shard, point);
    assert!(cast.status.success(), "{cast:?}");
    let cursor: u64 = number(&entity(&shard, patroller), "wp_target");
    let mut held = Vec::new();
    for guid in [patroller, wanderer, idle] {
        assert!(
            distraction(&shard, guid).is_some(),
            "{guid} must be distracted"
        );
        let at = entity(&shard, guid);
        let (x, y): (f32, f32) = (number(&at, "x"), number(&at, "y"));
        assert_orientation(&shard, guid, heading((x, y), point), "after the cast");
        held.push((guid, x, y));
    }

    // Keep every Distraction active past the hold window, however slow the runner.
    shard.assert_sql(&format!(
        "UPDATE game_creature_distraction SET ends_ms = {} WHERE ends_ms > 0",
        now_ms() + 600_000
    ));

    // Patrol and wander hold across two sense firings: no leg, only the facing row until the
    // advance reaps it.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        for guid in [patroller, wanderer] {
            if let Some(leg) = spline(&shard, guid) {
                assert_eq!(
                    leg["facing"], "true",
                    "{guid} moved while distracted: {leg:?}"
                );
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for (guid, x, y) in held {
        let at = entity(&shard, guid);
        assert_eq!(
            (number::<f32>(&at, "x"), number::<f32>(&at, "y")),
            (x, y),
            "{guid} moved while distracted"
        );
    }

    // At the expiry an idle Creature turns back to its spawn orientation and the Distraction ends.
    shard.assert_sql(&format!(
        "UPDATE game_creature_distraction SET ends_ms = 1 WHERE creature_guid = {idle}"
    ));
    assert!(
        poll_until(Duration::from_secs(5), || distraction(&shard, idle)
            .is_none()),
        "the expired Distraction must end"
    );
    assert_orientation(&shard, idle, spawn_orientation, "after the expiry");

    // A patroller turns back the same way, then walks on toward the waypoint it was walking to.
    shard.assert_sql(&format!(
        "UPDATE game_creature_distraction SET ends_ms = 1 WHERE creature_guid = {patroller}"
    ));
    assert!(
        poll_until(Duration::from_secs(5), || distraction(&shard, patroller)
            .is_none()),
        "the expired Distraction must end"
    );
    assert!(
        poll_until(Duration::from_secs(5), || spline(&shard, patroller)
            .is_some_and(|leg| leg["facing"] == "false")),
        "the patrol must resume"
    );
    assert_eq!(
        number::<u64>(&entity(&shard, patroller), "wp_target"),
        cursor,
        "the patrol continues toward the same waypoint"
    );

    // An Engagement ends the wanderer's Distraction long before its expiry.
    assert!(
        distraction(&shard, wanderer).is_some_and(|ends| ends > now_ms() + 60_000),
        "the wanderer must still be distracted before the Engagement"
    );
    shard.assert_call(
        "debug_engage",
        &[&wanderer.to_string(), &PLAYER.to_string()],
    );
    assert!(
        poll_until(Duration::from_secs(5), || distraction(&shard, wanderer)
            .is_none()),
        "a new Engagement must end the Distraction"
    );
}
