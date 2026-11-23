mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use lyracore_shared::spell::taxonomy::{
    A_IMMUNITY, A_MOD_DAMAGE_TAKEN, A_PERIODIC_DAMAGE, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE,
    E_HEAL, E_NEXT_SWING, P_SCHOOL_MASK, T_AREA_ENEMY, T_SELF, T_TARGET_ENEMY,
};
use serde_json::json;
use std::time::{Duration, Instant};
use support::{event_rows, fence_subscription, subscribe_with_fence, Field, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const PHYSICAL: u32 = 0x01;
const HOLY: u32 = 0x02;
const FROST: u32 = 0x10;
const SHADOW: u32 = 0x20;
const ALL_SCHOOLS: u32 = 0x7f;
// vmangos SpellSchools index of SHADOW, as game_item_template.damage_school stores it.
const SHADOW_SCHOOL: u8 = 5;
// AttackOutcome::wire codes in game_combat_event_live.hit_info.
const HIT_INFO_IMMUNE: &str = "8";
// vmangos SpellMissInfo in game_spell_cast_event_live.miss_info.
const MISS_NONE: &str = "0";
const MISS_IMMUNE: &str = "7";
// Source aura id and both-polarities bit in an immunity Aura's p1: docs/schema.md#damage-shields.
const SOURCE_SCHOOL_IMMUNITY: i32 = 39;
const IMMUNE_TO_HOSTILE_AND_FRIENDLY: i32 = 0x100;
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live";
const FROSTBOLT_FIXTURE: u32 = 5_117_201;
const QUEUED_STRIKE_FIXTURE: u32 = 5_117_202;
const IMMUNITY_FIXTURE: u32 = 5_117_203;
const HEAL_FIXTURE: u32 = 5_117_204;
const FROST_AREA_FIXTURE: u32 = 5_117_205;
const DEBUFF_FIXTURE: u32 = 5_117_206;
const FROST_DOT_FIXTURE: u32 = 5_117_207;
const DOT_PERIOD_MS: u32 = 3_000;
// 50 ms scheduler ticks that pass one DoT period.
const ONE_PERIOD_TICKS: &str = "61";
const SMSG_SPELLORDAMAGE_IMMUNE: u64 = 0x263;
const PACKET_WAIT: Duration = Duration::from_secs(10);
const HEAL_COST: u32 = 100;
const MANA: u32 = 1_000;
const SHOOT: u32 = 5019;
const WAND: u8 = 19;
const RANGED_SLOT: u8 = 17;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_whose_template_is_immune_to_physical_takes_no_white_swing_damage() {
    let mut standalone = Standalone::start("school-immunity-white-swing");
    let wolf = stage(&mut standalone);
    set_template_school_immunity(&standalone, PHYSICAL);
    let events = subscribe_with_fence(&standalone, &[SWING_EVENTS]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);

    swing(&standalone);
    fence_subscription(&standalone, &events);
    let swings = player_swings(&events);
    assert_eq!(swings.len(), 1);
    assert_eq!(swings[0]["hit_info"], HIT_INFO_IMMUNE);
    assert_eq!(swings[0]["damage"], "0");
    assert_eq!(
        health(&standalone, wolf),
        WOLF_HEALTH,
        "an immune creature loses no health"
    );

    set_template_school_immunity(&standalone, 0);
    swing(&standalone);
    fence_subscription(&standalone, &events);
    let swings = player_swings(&events);
    assert_eq!(swings.len(), 2);
    assert!(swings[1]["damage"].parse::<u32>().unwrap() > 0);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "without the immunity the same swing lands"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_frost_spell_on_a_frost_immune_creature_reports_immune_and_deals_nothing() {
    let mut standalone = Standalone::start("school-immunity-frost-spell");
    let wolf = stage(&mut standalone);
    insert_frostbolt(&standalone);
    set_template_school_immunity(&standalone, FROST);

    let go = cast_go(&standalone, FROSTBOLT_FIXTURE, wolf);
    assert_eq!(
        go["miss_info"], MISS_IMMUNE,
        "the template refuses the bolt before its hit roll"
    );
    assert_eq!(go["damage"], "0");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);

    set_template_school_immunity(&standalone, 0);
    let go = cast_go(&standalone, FROSTBOLT_FIXTURE, wolf);
    assert_eq!(go["miss_info"], MISS_NONE);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "without the immunity the same bolt lands"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_refused_harmful_spell_draws_the_creature_into_combat() {
    let mut standalone = Standalone::start("school-immunity-refusal-engages");
    let wolf = stage(&mut standalone);
    insert_frostbolt(&standalone);
    set_template_school_immunity(&standalone, FROST);

    let go = cast_go(&standalone, FROSTBOLT_FIXTURE, wolf);
    assert_eq!(go["miss_info"], MISS_IMMUNE);
    let threat = standalone.query_rows(&format!(
        "SELECT threat FROM game_threat WHERE creature_guid = {wolf} AND source_guid = {PLAYER}"
    ));
    assert_eq!(
        threat.len(),
        1,
        "vmangos Spell::DoAllEffectOnTarget engages on an immune miss"
    );
    assert_eq!(threat[0]["threat"], "0");
    for unit in [PLAYER, wolf] {
        let flags: u32 = standalone.query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {unit}"
        ))[0]["unit_flags"]
            .parse()
            .unwrap();
        assert_ne!(flags & IN_COMBAT, 0, "unit {unit} is in combat");
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_damage_tick_on_a_creature_that_became_immune_deals_nothing() {
    let mut standalone = Standalone::start("school-immunity-periodic-tick");
    let wolf = stage(&mut standalone);
    let mut wire = standalone.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([PLAYER]))
        .unwrap();
    insert_spell(
        &standalone,
        FROST_DOT_FIXTURE,
        &[
            ("school_mask", &FROST),
            ("range_yd", &30),
            ("is_negative", &true),
            ("duration_ms", &30_000),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
        &[
            ("kind", &A_PERIODIC_DAMAGE),
            ("base_points", &9),
            ("period_ms", &DOT_PERIOD_MS),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    let go = cast_go(&standalone, FROST_DOT_FIXTURE, wolf);
    assert_eq!(
        go["miss_info"], MISS_NONE,
        "the DoT lands before the immunity"
    );
    give_immunity_aura(&standalone, wolf, FROST, SOURCE_SCHOOL_IMMUNITY);
    let first_tick = next_tick(&standalone, wolf);
    let health_before = health(&standalone, wolf);
    wire.drain_packets();

    standalone.assert_call("debug_advance_roll_batch", &[ONE_PERIOD_TICKS]);
    assert_eq!(health(&standalone, wolf), health_before);
    assert!(
        next_tick(&standalone, wolf) > first_tick,
        "the refused tick still moves the Aura to its next period"
    );
    let immune = format!("{:016x}", PLAYER.swap_bytes()) + &format!("{:016x}", wolf.swap_bytes());
    let reports_immunity = |row: &str| {
        let packet: serde_json::Value = serde_json::from_str(row).unwrap();
        packet[2][1] == SMSG_SPELLORDAMAGE_IMMUNE
            && packet[3]
                .as_str()
                .is_some_and(|body| body.to_ascii_lowercase().starts_with(&immune))
    };
    let deadline = Instant::now() + PACKET_WAIT;
    let mut packets = wire.wait_for_packets(Duration::from_secs(1)).unwrap();
    while !packets.iter().any(|row| reports_immunity(row.get())) && Instant::now() < deadline {
        packets.extend(wire.wait_for_packets(Duration::from_secs(1)).unwrap());
    }
    assert!(
        packets.iter().any(|row| reports_immunity(row.get())),
        "vmangos Aura::PeriodicTick reports the tick with SMSG_SPELLORDAMAGE_IMMUNE"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_harmful_spell_without_damage_is_refused_by_a_creature_immune_to_its_school() {
    let mut standalone = Standalone::start("school-immunity-harmful-aura");
    let wolf = stage(&mut standalone);
    insert_spell(
        &standalone,
        DEBUFF_FIXTURE,
        &[
            ("school_mask", &PHYSICAL),
            ("range_yd", &30),
            ("is_negative", &false),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
        &[
            ("kind", &A_MOD_DAMAGE_TAKEN),
            ("base_points", &20),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &(PHYSICAL as i32)),
            ("p_0_kind", &P_SCHOOL_MASK),
        ],
    );
    set_template_school_immunity(&standalone, PHYSICAL);

    let go = cast_go(&standalone, DEBUFF_FIXTURE, wolf);
    assert_eq!(
        go["miss_info"], MISS_IMMUNE,
        "a spell aimed at an enemy is harmful whatever its imported polarity"
    );
    assert!(auras(&standalone, wolf, DEBUFF_FIXTURE).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_area_spell_hits_the_creature_beside_one_immune_to_its_school() {
    let mut standalone = Standalone::start("school-immunity-area");
    let immune = stage(&mut standalone);
    let exposed = spawn_wolf(&standalone);
    give_immunity_aura(&standalone, immune, FROST, SOURCE_SCHOOL_IMMUNITY);
    insert_spell(
        &standalone,
        FROST_AREA_FIXTURE,
        &[
            ("school_mask", &FROST),
            ("range_yd", &30),
            ("is_negative", &true),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
        &[
            ("kind", &E_DAMAGE),
            ("base_points", &50),
            ("target", &T_AREA_ENEMY),
            ("radius_yd", &10.0),
        ],
    );

    cast_go(&standalone, FROST_AREA_FIXTURE, immune);
    assert_eq!(health(&standalone, immune), WOLF_HEALTH);
    assert!(health(&standalone, exposed) < WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_heal_lands_through_a_school_immunity_from_a_helpful_source() {
    let mut standalone = Standalone::start("school-immunity-heal-through-shield");
    stage(&mut standalone);
    give_immunity_aura(&standalone, PLAYER, ALL_SCHOOLS, SOURCE_SCHOOL_IMMUNITY);
    insert_heal(&standalone, 0);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {} WHERE guid = {PLAYER}",
        PLAYER_HEALTH / 2
    ));

    let go = cast_go(&standalone, HEAL_FIXTURE, PLAYER);
    assert_eq!(go["miss_info"], MISS_NONE);
    assert!(
        health(&standalone, PLAYER) > PLAYER_HEALTH / 2,
        "vmangos IsImmuneToSchool lets a helpful spell past a helpful source's immunity"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_helpful_spell_on_a_unit_immune_to_its_school_is_refused_before_its_cost() {
    let mut standalone = Standalone::start("school-immunity-helpful-refused");
    stage(&mut standalone);
    give_immunity_aura(
        &standalone,
        PLAYER,
        HOLY,
        SOURCE_SCHOOL_IMMUNITY | IMMUNE_TO_HOSTILE_AND_FRIENDLY,
    );
    insert_heal(&standalone, HEAL_COST);
    let bytes: u32 = standalone.query_rows(&format!(
        "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {PLAYER}"
    ))[0]["unit_bytes_0"]
        .parse()
        .unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {}, max_power = {MANA} WHERE guid = {PLAYER}",
        bytes & 0x00FF_FFFF
    ));
    standalone.assert_call("debug_set_power", &["1", &MANA.to_string()]);

    let output = standalone.call(
        "debug_begin_cast",
        &["1", &HEAL_FIXTURE.to_string(), &PLAYER.to_string()],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains("is immune to its school"),
        "vmangos Spell::CheckCast refuses it with TARGET_AURASTATE: {text}"
    );
    assert_eq!(
        standalone.query_rows(&format!(
            "SELECT power FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0]["power"],
        MANA.to_string(),
        "the refusal spends no mana"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_strike_on_a_creature_under_a_physical_immunity_aura_deals_nothing() {
    let mut standalone = Standalone::start("school-immunity-queued-strike");
    let wolf = stage(&mut standalone);
    give_immunity_aura(&standalone, wolf, PHYSICAL, SOURCE_SCHOOL_IMMUNITY);
    insert_spell(
        &standalone,
        QUEUED_STRIKE_FIXTURE,
        &[
            ("school_mask", &PHYSICAL),
            ("range_yd", &5),
            ("is_negative", &true),
        ],
        &[
            ("kind", &E_NEXT_SWING),
            ("base_points", &11),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);
    standalone.assert_call(
        "debug_cast_at",
        &["1", &QUEUED_STRIKE_FIXTURE.to_string(), &wolf.to_string()],
    );
    assert_eq!(
        standalone.query_rows(&format!(
            "SELECT next_swing_spell FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0]["next_swing_spell"],
        QUEUED_STRIKE_FIXTURE.to_string(),
        "the immunity is checked when the swing fires, not when the strike is queued"
    );

    let events = subscribe_with_fence(
        &standalone,
        &[
            SWING_EVENTS,
            &format!(
                "SELECT * FROM game_spell_cast_event_live \
                 WHERE spell_id = {QUEUED_STRIKE_FIXTURE} AND kind = 2"
            ),
        ],
    );
    swing(&standalone);
    fence_subscription(&standalone, &events);
    let swings = player_swings(&events);
    assert_eq!(swings.len(), 1);
    assert_eq!(swings[0]["spell_swing"], "true");
    assert_eq!(swings[0]["hit_info"], HIT_INFO_IMMUNE);
    let go = event_rows(&events, "game_spell_cast_event_live")
        .pop()
        .expect("the queued strike fires");
    assert_eq!(go["swing_hit_info"], HIT_INFO_IMMUNE);
    assert_eq!(go["damage"], "0");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_wand_shot_on_a_creature_whose_template_is_immune_to_the_wands_school_deals_nothing() {
    let mut standalone = Standalone::start("school-immunity-wand");
    let wolf = stage(&mut standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET subclass = {WAND}, damage_school = {SHADOW_SCHOOL} \
         WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {RANGED_SLOT} \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_BLADE}"
    ));
    set_template_school_immunity(&standalone, SHADOW);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET ranged_spell_id = {SHOOT} WHERE attacker_guid = {PLAYER}"
    ));
    let wolf_row = standalone.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {wolf}"
    ));
    let wolf_x: f32 = wolf_row[0]["x"].parse().unwrap();
    let wolf_y: f32 = wolf_row[0]["y"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {wolf_y}, orientation = {} WHERE guid = {PLAYER}",
        wolf_x + 15.0,
        std::f32::consts::PI
    ));

    let events = subscribe_with_fence(&standalone, &[SWING_EVENTS]);
    swing(&standalone);
    fence_subscription(&standalone, &events);
    let shots = player_swings(&events);
    assert_eq!(shots.len(), 1, "the wand fires once");
    assert_eq!(shots[0]["ranged_spell_id"], SHOOT.to_string());
    assert_eq!(shots[0]["hit_info"], HIT_INFO_IMMUNE);
    assert_eq!(shots[0]["damage"], "0");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

fn insert_spell(standalone: &Standalone, spell_id: u32, spell: &[Field], effect: &[Field]) {
    let name = format!("School immunity fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![("spell_id", &spell_id), ("name", &name)];
    spell_row.extend_from_slice(spell);
    standalone.insert_row("game_spell", &spell_row);
    let effect_id = packed_spell_effect_id(spell_id, 0);
    let mut effect_row: Vec<Field> = vec![("id", &effect_id), ("spell_id", &spell_id)];
    effect_row.extend_from_slice(effect);
    standalone.insert_row("game_spell_effect", &effect_row);
}

fn insert_frostbolt(standalone: &Standalone) {
    insert_spell(
        standalone,
        FROSTBOLT_FIXTURE,
        &[
            ("school_mask", &FROST),
            ("range_yd", &30),
            ("is_negative", &true),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
        &[
            ("kind", &E_DAMAGE),
            ("base_points", &50),
            ("target", &T_TARGET_ENEMY),
        ],
    );
}

fn next_tick(standalone: &Standalone, wolf: u64) -> i64 {
    standalone.query_rows(&format!(
        "SELECT next_tick_micros FROM game_aura \
         WHERE target_guid = {wolf} AND spell_id = {FROST_DOT_FIXTURE}"
    ))[0]["next_tick_micros"]
        .parse()
        .unwrap()
}

fn insert_heal(standalone: &Standalone, cost: u32) {
    insert_spell(
        standalone,
        HEAL_FIXTURE,
        &[
            ("school_mask", &HOLY),
            ("range_yd", &30),
            ("is_negative", &false),
            ("cost", &cost),
        ],
        &[
            ("kind", &E_HEAL),
            ("base_points", &500),
            ("target", &T_SELF),
        ],
    );
}

// A helpful Spell carries the immunity, as Divine Shield and Blessing of Protection do.
fn give_immunity_aura(standalone: &Standalone, unit: u64, school_mask: u32, source: i32) {
    insert_spell(
        standalone,
        IMMUNITY_FIXTURE,
        &[
            ("school_mask", &HOLY),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
        &[
            ("kind", &A_IMMUNITY),
            ("target", &T_SELF),
            ("p_0", &(school_mask as i32)),
            ("p_0_kind", &P_SCHOOL_MASK),
            ("p_1", &source),
        ],
    );
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &unit.to_string(),
            &IMMUNITY_FIXTURE.to_string(),
            &unit.to_string(),
        ],
    );
    assert_eq!(
        auras(standalone, unit, IMMUNITY_FIXTURE).len(),
        1,
        "the unit carries the immunity Aura"
    );
}

fn cast_go(standalone: &Standalone, spell_id: u32, target: u64) -> support::Row {
    let events = subscribe_with_fence(
        standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {spell_id} AND kind = 2"
        )],
    );
    standalone.assert_call(
        "debug_cast_at",
        &["1", &spell_id.to_string(), &target.to_string()],
    );
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);
    fence_subscription(standalone, &events);
    event_rows(&events, "game_spell_cast_event_live")
        .pop()
        .expect("the cast completes")
}

fn auras(standalone: &Standalone, unit: u64, spell_id: u32) -> Vec<support::Row> {
    standalone.query_rows(&format!(
        "SELECT spell_id FROM game_aura WHERE target_guid = {unit} AND spell_id = {spell_id}"
    ))
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let wolf = spawn_wolf(standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH} \
         WHERE guid = {PLAYER}"
    ));
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    wolf
}

fn spawn_wolf(standalone: &Standalone) -> u64 {
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = 1, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    wolf
}

fn set_template_school_immunity(standalone: &Standalone, mask: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_creature_template SET school_immune_mask = {mask} \
         WHERE entry = {TEST_WOLF_ENTRY}"
    ));
}

// A fixed swing roll stream connects against a level 1 wolf, as in the damage shield test.
fn swing(standalone: &Standalone) {
    standalone
        .assert_sql("UPDATE game_conformance_swing_random SET state = 1 WHERE character_guid = 1");
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    standalone.assert_call("debug_advance_roll_batch", &["4"]);
}

fn player_swings(
    events: &support::TableSubscription,
) -> Vec<std::collections::BTreeMap<String, String>> {
    event_rows(events, "game_combat_event_live")
        .into_iter()
        .filter(|row| row["attacker_guid"] == PLAYER.to_string())
        .collect()
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
