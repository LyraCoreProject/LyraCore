mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_IMMUNITY, E_DAMAGE, E_NEXT_SWING, P_SCHOOL_MASK, T_SELF, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const PHYSICAL: u32 = 1;
const FROST: u32 = 0x10;
const SHADOW: u32 = 0x20;
// vmangos SpellSchools index of SHADOW, as game_item_template.damage_school stores it.
const SHADOW_SCHOOL: u8 = 5;
// AttackOutcome::wire codes in game_combat_event_live.hit_info.
const HIT_INFO_IMMUNE: &str = "8";
// vmangos SpellMissInfo SPELL_MISS_IMMUNE in game_spell_cast_event_live.miss_info.
const MISS_IMMUNE: &str = "7";
// Source aura id in an immunity Aura's p1: docs/schema.md#damage-shields.
const SOURCE_SCHOOL_IMMUNITY: i32 = 39;
const SWING_EVENTS: &str = "SELECT * FROM game_combat_event_live";
const FROSTBOLT_FIXTURE: u32 = 5_117_201;
const QUEUED_STRIKE_FIXTURE: u32 = 5_117_202;
const PHYSICAL_IMMUNITY_FIXTURE: u32 = 5_117_203;
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
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &FROSTBOLT_FIXTURE),
            ("name", &"Frostbolt fixture"),
            ("school_mask", &(FROST as u8)),
            ("range_yd", &30),
            ("is_negative", &true),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(FROSTBOLT_FIXTURE, 0)),
            ("spell_id", &FROSTBOLT_FIXTURE),
            ("kind", &E_DAMAGE),
            ("base_points", &50),
            ("target", &T_TARGET_ENEMY),
        ],
    );
    set_template_school_immunity(&standalone, FROST);

    let events = subscribe_with_fence(
        &standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live \
             WHERE spell_id = {FROSTBOLT_FIXTURE} AND kind = 2"
        )],
    );
    standalone.assert_call(
        "debug_cast_at",
        &["1", &FROSTBOLT_FIXTURE.to_string(), &wolf.to_string()],
    );
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);
    fence_subscription(&standalone, &events);
    let go = event_rows(&events, "game_spell_cast_event_live")
        .pop()
        .expect("the cast completes");
    assert_eq!(
        go["miss_info"], MISS_IMMUNE,
        "the template refuses the bolt before its hit roll"
    );
    assert_eq!(go["damage"], "0");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_strike_on_a_creature_under_a_physical_immunity_aura_deals_nothing() {
    let mut standalone = Standalone::start("school-immunity-queued-strike");
    let wolf = stage(&mut standalone);
    stage_physical_immunity_aura(&standalone, wolf);
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &QUEUED_STRIKE_FIXTURE),
            ("name", &"Queued strike fixture"),
            ("school_mask", &(PHYSICAL as u8)),
            ("range_yd", &5),
            ("is_negative", &true),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(QUEUED_STRIKE_FIXTURE, 0)),
            ("spell_id", &QUEUED_STRIKE_FIXTURE),
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

fn stage_physical_immunity_aura(standalone: &Standalone, wolf: u64) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &PHYSICAL_IMMUNITY_FIXTURE),
            ("name", &"Physical immunity fixture"),
            ("school_mask", &2u8),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PHYSICAL_IMMUNITY_FIXTURE, 0)),
            ("spell_id", &PHYSICAL_IMMUNITY_FIXTURE),
            ("kind", &A_IMMUNITY),
            ("target", &T_SELF),
            ("p_0", &(PHYSICAL as i32)),
            ("p_0_kind", &P_SCHOOL_MASK),
            ("p_1", &SOURCE_SCHOOL_IMMUNITY),
        ],
    );
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &wolf.to_string(),
            &PHYSICAL_IMMUNITY_FIXTURE.to_string(),
            &wolf.to_string(),
        ],
    );
    assert_eq!(
        standalone
            .query_rows(&format!(
                "SELECT spell_id FROM game_aura WHERE target_guid = {wolf} \
                 AND spell_id = {PHYSICAL_IMMUNITY_FIXTURE}"
            ))
            .len(),
        1,
        "the wolf carries the immunity Aura"
    );
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
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
    standalone.assert_sql(
        "UPDATE game_world_entity SET health = 10000, max_health = 10000 WHERE guid = 1",
    );
    standalone.insert_row(
        "game_conformance_swing_random",
        &[("character_guid", &1), ("state", &1)],
    );
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
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
