mod support;

use lyracore_shared::item_property::COMBAT_SPELL;
use lyracore_shared::spell::taxonomy::{
    E_DAMAGE, E_ENCHANT_ITEM_TEMPORARY, P_ENCHANT_ID, P_NONE, T_SELF, T_TARGET_ENEMY,
};

use std::collections::BTreeMap;

use support::{event_rows, poll_until, Standalone, TableSubscription, POLL_TIMEOUT};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const MAINHAND: &str = "15";
const TEST_WOLF_ENTRY: u32 = 51000;
const WOLF_HEALTH: u32 = 100_000;
const FIXTURE_BLADE: u32 = 5_090_050;
const FIXTURE_CONSUMABLE: u32 = 5_090_052;
const POISON: u32 = 5_090_200;
// Instant Poison: enchantment 323 casts 8680 and puts 40 charges on the weapon.
const INSTANT_POISON_ENCHANTMENT: u32 = 323;
const INSTANT_POISON_PROC: u32 = 8680;
const INSTANT_POISON_CHARGES: u32 = 40;
const THIRTY_MINUTES_S: i32 = 1800;
// 30 procs per minute on a 2 second weapon is a certain proc.
const CERTAIN_PPM: f32 = 30.0;
const WEAPON_DELAY_MS: u32 = 2000;
const CAST_EVENTS: &str = "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_poison_coats_the_weapon_and_each_proc_spends_a_charge() {
    let mut standalone = Standalone::start("temporary-enchantment-poison");
    let wolf = stage(&mut standalone, THIRTY_MINUTES_S);
    let blade = item_guid(&standalone, FIXTURE_BLADE);

    standalone.assert_call(
        "debug_use_item_on_item",
        &["1", &item_slot(&standalone, FIXTURE_CONSUMABLE), MAINHAND],
    );

    assert_eq!(
        coating(&standalone, blade).map(|row| (row["enchant_id"].clone(), charges(&row))),
        Some((
            INSTANT_POISON_ENCHANTMENT.to_string(),
            INSTANT_POISON_CHARGES
        )),
        "the poison puts its enchantment and charges on the weapon"
    );

    let events = standalone.subscribe(&[CAST_EVENTS]);
    standalone.assert_call("debug_engage", &["1", &wolf.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || go_of(&events, INSTANT_POISON_PROC)
            .is_some()),
        "a certain poison proc must cast its spell when a white swing connects"
    );
    assert_eq!(
        go_of(&events, INSTANT_POISON_PROC).unwrap()["target_guid"],
        wolf.to_string(),
        "the poison strikes the victim"
    );
    assert!(
        poll_until(POLL_TIMEOUT, || coating(&standalone, blade)
            .is_some_and(|row| charges(&row) < INSTANT_POISON_CHARGES)),
        "a proc spends a charge"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_temporary_enchantment_falls_off_when_its_time_runs_out() {
    let mut standalone = Standalone::start("temporary-enchantment-expiry");
    stage(&mut standalone, 1);
    let blade = item_guid(&standalone, FIXTURE_BLADE);

    standalone.assert_call(
        "debug_use_item_on_item",
        &["1", &item_slot(&standalone, FIXTURE_CONSUMABLE), MAINHAND],
    );

    assert!(
        poll_until(POLL_TIMEOUT, || coating(&standalone, blade).is_none()),
        "a one-second enchantment must leave the weapon"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_poison_without_an_item_target_is_refused_and_not_spent() {
    let mut standalone = Standalone::start("temporary-enchantment-refused");
    stage(&mut standalone, THIRTY_MINUTES_S);

    let result = standalone.call(
        "debug_use_item",
        &["1", &item_slot(&standalone, FIXTURE_CONSUMABLE)],
    );

    assert!(!result.status.success(), "a poison needs a weapon to coat");
    assert_eq!(
        standalone.query_rows(&format!(
            "SELECT stack_count FROM game_item_instance \
             WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_CONSUMABLE}"
        ))[0]["stack_count"],
        "1",
        "a refused poison stays in the bag"
    );
}

fn stage(standalone: &mut Standalone, seconds: i32) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
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
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));

    stage_spell(
        standalone,
        POISON,
        "Instant Poison",
        (E_ENCHANT_ITEM_TEMPORARY, T_SELF),
        seconds,
        (INSTANT_POISON_ENCHANTMENT as i32, P_ENCHANT_ID),
    );
    stage_spell(
        standalone,
        INSTANT_POISON_PROC,
        "Instant Poison",
        (E_DAMAGE, T_TARGET_ENEMY),
        5,
        (0, P_NONE),
    );
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_enchant_charges (spell_id, charges) \
         VALUES ({POISON}, {INSTANT_POISON_CHARGES})"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_item_enchantment (id, enchant_id, effect_index, kind, amount, spell_id, \
         school_mask) VALUES ({}, {INSTANT_POISON_ENCHANTMENT}, 0, {COMBAT_SPELL}, 0, \
         {INSTANT_POISON_PROC}, 0)",
        u64::from(INSTANT_POISON_ENCHANTMENT) << 8
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_enchant_proc_rate (spell_id, ppm_rate) \
         VALUES ({INSTANT_POISON_PROC}, {CERTAIN_PPM:.1})"
    ));

    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET delay_ms = {WEAPON_DELAY_MS}, spellid_1 = 0 \
         WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET spellid_1 = {POISON}, spelltrigger_1 = 0 \
         WHERE entry = {FIXTURE_CONSUMABLE}"
    ));
    standalone.assert_call(
        "debug_grant_item",
        &["1", &FIXTURE_CONSUMABLE.to_string(), "1"],
    );
    wolf
}

fn stage_spell(
    standalone: &Standalone,
    spell_id: u32,
    name: &str,
    (kind, target): (u8, u8),
    base_points: i32,
    (p0, p0_kind): (i32, u8),
) {
    let is_negative = target != T_SELF;
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES ({spell_id}, '{name}', 0, 0, 0, 0, 0, 5, 0, 1, 0, 0, 0, 0, 0, \
         0, 0, {is_negative}, 0, 0, 0, 0, 0, 0, 0, 0)"
    ));
    standalone.assert_sql(&format!(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES ({}, {spell_id}, 0, {kind}, {base_points}, 0, 0.0, 0, {target}, \
         0.0, 0, 0, 0, {p0}, {p0_kind}, 0, 0, false, 0.0)",
        u64::from(spell_id) << 2
    ));
}

fn item_guid(standalone: &Standalone, entry: u32) -> u64 {
    standalone.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = {PLAYER} AND entry = {entry}"
    ))[0]["guid"]
        .parse()
        .unwrap()
}

fn item_slot(standalone: &Standalone, entry: u32) -> String {
    standalone.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = {PLAYER} AND entry = {entry}"
    ))[0]["slot"]
        .clone()
}

fn coating(standalone: &Standalone, item_guid: u64) -> Option<SqlRow> {
    standalone
        .query_rows(&format!(
            "SELECT enchant_id, charges FROM game_temporary_enchantment \
             WHERE item_guid = {item_guid}"
        ))
        .into_iter()
        .next()
}

fn charges(row: &SqlRow) -> u32 {
    row["charges"].parse().unwrap()
}

fn go_of(events: &TableSubscription, spell_id: u32) -> Option<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .find(|row| row["spell_id"] == spell_id.to_string() && row["kind"] == "2")
}
