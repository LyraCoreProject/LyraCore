mod support;

use std::io::Cursor;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_PERIODIC_TRIGGER, E_CREATE_ITEM, E_HEAL, E_TRIGGER, T_SELF,
};
use lyracore_test_support::ReducerConnection;
use serde_json::json;
use support::{
    actor, event_rows, fence_subscription, number, subscribe_with_fence, Row, Standalone,
};
use wow_world_messages::vanilla::{
    ClientMessage, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags, CMSG_CAST_SPELL,
};

const PLAYER: &str = "1";
const MENDING: u32 = 5_098_501;
const PARENT: u32 = 5_098_502;
const CRAFT: u32 = 5_098_503;
const REAGENT: u32 = 5_098_504;
const PARENT_REAGENT: u32 = 5_098_505;
const PRODUCT: u32 = 5_098_506;
const SELF_REAGENT_ITEM: u32 = 5_098_507;
const REAGENT_COUNT: u32 = 2;
const MANA_COST: u32 = 10;
const CAST_TIME_MS: u32 = 1000;
const CMSG_CAST_SPELL_OPCODE: u32 = 0x12e;
// 1.12.1 SpellCastResult SPELL_FAILED_ITEM_NOT_READY from vmangos SpellDefines.h.
const ITEM_NOT_READY: u8 = 0x28;
// `CastRefusalReason::MissingReagent` in `game_spell_cast_event_live.failure_reason`.
const MISSING_REAGENT: &str = "34";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_short_of_its_reagents_is_refused_before_it_spends_power() {
    let (shard, mage) = native_realm("reagents-missing");
    mending(&shard, 0, MANA_COST);
    let mut connection = enter(&shard, &mage);
    shard.assert_call("debug_learn_spell", &[&mage, &MENDING.to_string()]);
    grant(&shard, &mage, REAGENT, REAGENT_COUNT - 1);
    shard.assert_call("debug_set_power", &[&mage, "100"]);

    let refused = cast(&mut connection, MENDING).expect_err("one reagent short");
    assert!(
        refused.contains(&cast_result(MENDING, ITEM_NOT_READY)),
        "vmangos Spell::CheckItems: {refused}"
    );
    assert_eq!(power(&shard, &mage), 100);
    assert_eq!(stack(&shard, &mage, REAGENT), Some(REAGENT_COUNT - 1));

    grant(&shard, &mage, REAGENT, 1);
    cast(&mut connection, MENDING).expect("the reagents are carried");
    assert_eq!(power(&shard, &mage), 100 - MANA_COST);
    assert_eq!(stack(&shard, &mage, REAGENT), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_with_its_reagents_goes_off_and_takes_their_count() {
    let shard = stage("reagents-taken");
    mending(&shard, 0, 0);
    grant(&shard, PLAYER, REAGENT, 5);

    cast_spell(&shard, MENDING);
    assert!(health(&shard) > 5000, "the spell heals");
    assert_eq!(stack(&shard, PLAYER, REAGENT), Some(5 - REAGENT_COUNT));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_timed_spell_takes_its_reagents_when_it_completes() {
    let shard = stage("reagents-timed");
    mending(&shard, CAST_TIME_MS, 0);
    grant(&shard, PLAYER, REAGENT, 5);

    cast_spell(&shard, MENDING);
    assert_eq!(
        stack(&shard, PLAYER, REAGENT),
        Some(5),
        "the cast has begun"
    );
    shard.assert_call("debug_advance_roll_batch", &["40"]);
    assert!(health(&shard) > 5000, "the cast completes");
    assert_eq!(stack(&shard, PLAYER, REAGENT), Some(5 - REAGENT_COUNT));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_timed_spell_whose_reagents_are_gone_at_completion_is_refused_missing_reagent() {
    let shard = stage("reagents-gone-at-completion");
    shard.clear_schedule("game_event_reaper_schedule");
    mending(&shard, CAST_TIME_MS, 0);
    let slot = grant(&shard, PLAYER, REAGENT, 5);
    let casts = subscribe_with_fence(
        &shard,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND spell_id = {MENDING}"
        )],
    );

    cast_spell(&shard, MENDING);
    shard.assert_call("gw_destroy_item", &[&actor(PLAYER), &slot, "4"]);
    shard.assert_call("debug_advance_roll_batch", &["40"]);
    fence_subscription(&shard, &casts);
    let rows = event_rows(&casts, "game_spell_cast_event_live");
    let refusals: Vec<&Row> = rows
        .iter()
        .filter(|row| row["is_interrupted"] == "true")
        .collect();
    assert_eq!(refusals.len(), 1, "{rows:?}");
    assert_eq!(refusals[0]["failure_reason"], MISSING_REAGENT);
    assert_eq!(health(&shard), 5000, "the refused cast does not heal");
    assert_eq!(stack(&shard, PLAYER, REAGENT), Some(1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_takes_none_of_its_reagents_when_its_parent_has_some() {
    let shard = stage("reagents-trigger-child-paid");
    triggering_parent(&shard);
    reagent(&shard, PARENT, PARENT_REAGENT, 1);
    item(&shard, PARENT_REAGENT, "Parent reagent fixture", 0);
    grant(&shard, PLAYER, PARENT_REAGENT, 1);
    grant(&shard, PLAYER, REAGENT, 5);

    cast_spell(&shard, PARENT);
    assert!(health(&shard) > 5000, "the trigger child heals");
    assert_eq!(stack(&shard, PLAYER, PARENT_REAGENT), None);
    assert_eq!(stack(&shard, PLAYER, REAGENT), Some(5));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_of_a_parent_without_reagents_takes_its_own() {
    let shard = stage("reagents-trigger-child-own");
    triggering_parent(&shard);
    grant(&shard, PLAYER, REAGENT, 5);

    cast_spell(&shard, PARENT);
    assert!(health(&shard) > 5000, "the trigger child heals");
    assert_eq!(stack(&shard, PLAYER, REAGENT), Some(5 - REAGENT_COUNT));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_trigger_casts_a_reagent_spell_without_its_reagents() {
    let shard = stage("reagents-triggered-cast");
    mending(&shard, 0, 0);
    spell(&shard, PARENT, 0, 0);
    shard.assert_sql(&format!(
        "UPDATE game_spell SET duration_ms = 10000 WHERE spell_id = {PARENT}"
    ));
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PARENT, 0)),
            ("spell_id", &PARENT),
            ("effect_index", &0u8),
            ("kind", &A_PERIODIC_TRIGGER),
            ("target", &T_SELF),
            ("period_ms", &1_000u32),
            ("trigger_spell", &MENDING),
        ],
    );

    cast_spell(&shard, PARENT);
    shard.assert_call("debug_advance_roll_batch", &["40"]);
    assert!(health(&shard) > 5000, "the Triggered Cast heals");
    assert_eq!(stack(&shard, PLAYER, REAGENT), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_craft_takes_its_reagents_once() {
    let shard = stage("reagents-craft");
    spell(&shard, CRAFT, 0, 0);
    effect(&shard, CRAFT, 0, (E_CREATE_ITEM, 1), PRODUCT as i32);
    reagent(&shard, CRAFT, REAGENT, REAGENT_COUNT);
    item(&shard, REAGENT, "Reagent fixture", 0);
    item(&shard, PRODUCT, "Product fixture", 0);
    grant(&shard, PLAYER, REAGENT, 5);

    cast_spell(&shard, CRAFT);
    assert_eq!(stack(&shard, PLAYER, PRODUCT), Some(1));
    assert_eq!(stack(&shard, PLAYER, REAGENT), Some(5 - REAGENT_COUNT));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_used_up_as_its_own_spells_reagent_is_taken_with_the_reagents() {
    let shard = stage("reagents-own-item");
    spell(&shard, MENDING, 0, 0);
    effect(&shard, MENDING, 0, (E_HEAL, 100), 0);
    reagent(&shard, MENDING, SELF_REAGENT_ITEM, REAGENT_COUNT);
    item(&shard, SELF_REAGENT_ITEM, "Own reagent fixture", MENDING);
    let slot = grant(&shard, PLAYER, SELF_REAGENT_ITEM, 5);

    shard.assert_call("debug_use_item", &[PLAYER, &slot]);
    assert!(health(&shard) > 5000, "the item's spell heals");
    // vmangos Spell::TakeReagents takes the count and one more for the used-up cast item.
    assert_eq!(
        stack(&shard, PLAYER, SELF_REAGENT_ITEM),
        Some(5 - REAGENT_COUNT - 1)
    );
}

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[PLAYER]);
    shard.assert_sql(
        "UPDATE game_world_entity SET health = 5000, max_health = 10000 WHERE guid = 1",
    );
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard
}

fn native_realm(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"REAGENTS\"", &credentials, &credentials],
    );
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'REAGENTS'")[0]
        ["id"]
        .clone();
    // Race 1 and class 8 make a Human Mage, who spends mana.
    shard.assert_call(
        "create_character",
        &[&account, "\"Mage\"", "1", "8", "0", "0", "0", "0", "0", "0"],
    );
    let mage =
        shard.query_rows("SELECT guid FROM game_character WHERE name = 'Mage'")[0]["guid"].clone();
    (shard, mage)
}

fn enter(shard: &Standalone, guid: &str) -> ReducerConnection {
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call(
            "debug_native_enter_world",
            json!([guid.parse::<u64>().unwrap()]),
        )
        .unwrap();
    connection
}

fn cast(connection: &mut ReducerConnection, spell: u32) -> Result<(), String> {
    let mut frame = Vec::new();
    CMSG_CAST_SPELL {
        spell,
        targets: SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::empty(),
        },
    }
    .write_unencrypted_client(&mut Cursor::new(&mut frame))
    .unwrap();
    // A client frame is a two-byte size and a four-byte opcode before the body.
    connection
        .call(
            "native_world_receive",
            json!([CMSG_CAST_SPELL_OPCODE, &frame[6..]]),
        )
        .map(|_| ())
}

/// The refused cast's last reply: SMSG_CAST_RESULT (0x130) with the spell, status 2 and `result`.
fn cast_result(spell: u32, result: u8) -> String {
    let body = spell
        .to_le_bytes()
        .iter()
        .chain(&[2, result])
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("[304, [{body}]]]")
}

/// A self heal that needs `REAGENT_COUNT` of the reagent fixture.
fn mending(shard: &Standalone, cast_time_ms: u32, mana_cost: u32) {
    spell(shard, MENDING, cast_time_ms, mana_cost);
    effect(shard, MENDING, 0, (E_HEAL, 100), 0);
    reagent(shard, MENDING, REAGENT, REAGENT_COUNT);
    item(shard, REAGENT, "Reagent fixture", 0);
}

fn triggering_parent(shard: &Standalone) {
    mending(shard, 0, 0);
    spell(shard, PARENT, 0, 0);
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PARENT, 0)),
            ("spell_id", &PARENT),
            ("effect_index", &0u8),
            ("kind", &E_TRIGGER),
            ("target", &T_SELF),
            ("trigger_spell", &MENDING),
        ],
    );
}

fn spell(shard: &Standalone, spell_id: u32, cast_time_ms: u32, mana_cost: u32) {
    let name = format!("Reagent spell fixture {spell_id}");
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("cast_time_ms", &cast_time_ms),
            ("cost", &mana_cost),
            ("gcd_ms", &0u32),
            ("spell_level", &1u32),
        ],
    );
}

fn effect(shard: &Standalone, spell_id: u32, index: u8, (kind, points): (u8, i32), p0: i32) {
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, index)),
            ("spell_id", &spell_id),
            ("effect_index", &index),
            ("kind", &kind),
            ("base_points", &points),
            ("target", &T_SELF),
            ("p_0", &p0),
        ],
    );
}

fn reagent(shard: &Standalone, spell_id: u32, item_entry: u32, count: u32) {
    // Spell.dbc reagent slot 0 under the importer's `spell_id << 3 | slot` key.
    let id = u64::from(spell_id) << 3;
    shard.insert_row(
        "game_spell_reagent",
        &[
            ("id", &id),
            ("spell_id", &spell_id),
            ("item_entry", &item_entry),
            ("count", &count),
        ],
    );
}

fn item(shard: &Standalone, entry: u32, name: &str, spell_id: u32) {
    let charges: i32 = if spell_id == 0 { 0 } else { -1 };
    shard.insert_row(
        "game_item_template",
        &[
            ("entry", &entry),
            ("name", &name),
            ("max_stack", &20u32),
            ("spellid_1", &spell_id),
            ("spellcharges_1", &charges),
        ],
    );
}

fn grant(shard: &Standalone, guid: &str, entry: u32, count: u32) -> String {
    shard.assert_call(
        "debug_grant_item",
        &[guid, &entry.to_string(), &count.to_string()],
    );
    shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ))[0]["slot"]
        .clone()
}

fn cast_spell(shard: &Standalone, spell_id: u32) {
    shard.assert_call("debug_learn_spell", &[PLAYER, &spell_id.to_string()]);
    shard.assert_call("debug_cast_spell", &[PLAYER, &spell_id.to_string(), "0"]);
}

fn stack(shard: &Standalone, guid: &str, entry: u32) -> Option<u32> {
    let items = shard.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = {entry}"
    ));
    assert!(items.len() <= 1, "{items:?}");
    items.first().map(|item| number(item, "stack_count"))
}

fn power(shard: &Standalone, guid: &str) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT power FROM game_world_entity WHERE guid = {guid}"
        ))[0],
        "power",
    )
}

fn health(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1")[0],
        "health",
    )
}
