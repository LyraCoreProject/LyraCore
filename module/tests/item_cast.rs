mod support;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{E_CREATE_ITEM, E_HEAL, T_SELF};
use support::{
    actor, event_rows, fence_subscription, number, subscribe_with_fence, Row, Standalone,
    TableSubscription,
};

const PLAYER: &str = "1";
const CRAFT_SPELL: u32 = 5_098_301;
const PRODUCT: u32 = 5_098_302;
const CRAFT_ITEM: u32 = 5_098_303;
const HEAL_SPELL: u32 = 5_098_304;
const HEAL_ITEM: u32 = 5_098_305;
const CAST_TIME_MS: u32 = 5000;
// `CastRefusalReason::ItemGone` in `game_spell_cast_event_live.failure_reason`.
const ITEM_GONE: &str = "29";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_crafting_item_cast_uses_up_its_item_through_its_storage_plan() {
    let shard = stage("item-cast-craft");
    craft_fixture(&shard, CAST_TIME_MS);
    let slot = grant(&shard, CRAFT_ITEM, 2);
    let casts = subscribe_casts(&shard, CRAFT_SPELL);
    use_item(&shard, &slot);
    assert_eq!(stack(&shard, CRAFT_ITEM), Some(2));

    shard.assert_call("debug_advance_roll_batch", &["120"]);
    let rows = cast_rows(&shard, &casts);
    assert!(rows.iter().any(is_completion), "{rows:?}");
    assert_eq!(stack(&shard, CRAFT_ITEM), Some(1));
    assert_eq!(stack(&shard, PRODUCT), Some(1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_crafting_item_cast_whose_product_is_refused_still_uses_up_its_item() {
    let shard = stage("item-cast-craft-refused");
    craft_fixture(&shard, 0);
    grant(&shard, PRODUCT, 1);
    let slot = grant(&shard, CRAFT_ITEM, 2);
    shard.assert_sql("UPDATE game_world_entity SET health = 5000 WHERE guid = 1");

    use_item(&shard, &slot);
    assert!(health(&shard) > 5000, "the heal effect applies");
    assert_eq!(
        stack(&shard, PRODUCT),
        Some(1),
        "the unique product is refused"
    );
    assert_eq!(stack(&shard, CRAFT_ITEM), Some(1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cast_item_destroyed_mid_cast_fails_the_completion_as_item_gone() {
    let shard = stage("item-cast-destroyed");
    heal_fixture(&shard);
    let slot = grant(&shard, HEAL_ITEM, 2);
    let casts = subscribe_casts(&shard, HEAL_SPELL);
    use_item(&shard, &slot);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = 1 AND entry = {HEAL_ITEM}"
    ));

    shard.assert_call("debug_advance_roll_batch", &["120"]);
    assert_item_gone(&shard, &casts);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cast_item_offered_in_trade_mid_cast_fails_the_completion_and_is_kept() {
    let shard = stage("item-cast-traded");
    heal_fixture(&shard);
    let slot = grant(&shard, HEAL_ITEM, 2);
    open_trade(&shard);
    let casts = subscribe_casts(&shard, HEAL_SPELL);
    use_item(&shard, &slot);
    shard.assert_call("gw_set_trade_item", &[&actor(PLAYER), "0", &slot]);
    assert_eq!(
        shard
            .query_rows("SELECT id FROM game_trade_slot WHERE owner_guid = 1")
            .len(),
        1
    );

    shard.assert_call("debug_advance_roll_batch", &["120"]);
    assert_item_gone(&shard, &casts);
    assert_eq!(stack(&shard, HEAL_ITEM), Some(2));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_naming_itself_as_its_target_is_refused_and_kept() {
    let shard = stage("item-cast-self-target");
    heal_fixture(&shard);
    let slot = grant(&shard, HEAL_ITEM, 2);
    let refused = shard.call("debug_use_item_on_item", &[PLAYER, &slot, &slot]);
    assert!(!refused.status.success());
    assert!(pending_casts(&shard).is_empty());
    assert_eq!(stack(&shard, HEAL_ITEM), Some(2));
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
        "UPDATE game_world_entity SET health = 10000, max_health = 10000 WHERE guid = 1",
    );
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard
}

fn craft_fixture(shard: &Standalone, cast_time_ms: u32) {
    spell(shard, CRAFT_SPELL, "Crafting item fixture", cast_time_ms);
    effect(shard, CRAFT_SPELL, 0, (E_CREATE_ITEM, 1), PRODUCT as i32);
    effect(shard, CRAFT_SPELL, 1, (E_HEAL, 100), 0);
    shard.insert_row(
        "game_item_template",
        &[
            ("entry", &PRODUCT),
            ("name", &"Unique product fixture"),
            ("max_stack", &1),
            ("max_count", &1),
        ],
    );
    item(shard, CRAFT_ITEM, "Crafting item fixture item", CRAFT_SPELL);
}

fn heal_fixture(shard: &Standalone) {
    spell(shard, HEAL_SPELL, "Timed item heal fixture", CAST_TIME_MS);
    effect(shard, HEAL_SPELL, 0, (E_HEAL, 100), 0);
    item(shard, HEAL_ITEM, "Timed item heal fixture item", HEAL_SPELL);
}

fn spell(shard: &Standalone, spell_id: u32, name: &str, cast_time_ms: u32) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("cast_time_ms", &cast_time_ms),
            ("spell_level", &1),
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

fn item(shard: &Standalone, entry: u32, name: &str, spell_id: u32) {
    shard.insert_row(
        "game_item_template",
        &[
            ("entry", &entry),
            ("name", &name),
            ("max_stack", &5),
            ("spellid_1", &spell_id),
        ],
    );
}

fn grant(shard: &Standalone, entry: u32, count: u32) -> String {
    shard.assert_call(
        "debug_grant_item",
        &[PLAYER, &entry.to_string(), &count.to_string()],
    );
    items(shard, entry).remove(0)["slot"].clone()
}

fn items(shard: &Standalone, entry: u32) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT slot, stack_count FROM game_item_instance WHERE owner_guid = 1 AND entry = {entry}"
    ))
}

fn stack(shard: &Standalone, entry: u32) -> Option<u32> {
    let items = items(shard, entry);
    assert!(items.len() <= 1, "{items:?}");
    items.first().map(|item| number(item, "stack_count"))
}

fn health(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1")[0],
        "health",
    )
}

fn use_item(shard: &Standalone, slot: &str) {
    shard.assert_call("gw_use_item", &[&actor(PLAYER), slot]);
}

fn open_trade(shard: &Standalone) {
    shard.assert_call(
        "create_character",
        &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let other =
        shard.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")[0]["guid"].clone();
    shard.assert_call("debug_spawn_player_entity", &[&other]);
    for guid in [PLAYER, other.as_str()] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = {guid}"
        ));
    }
    shard.assert_call("gw_initiate_trade", &[&actor(PLAYER), &other]);
    shard.assert_call("gw_begin_trade", &[&actor(&other)]);
}

fn subscribe_casts(shard: &Standalone, spell_id: u32) -> TableSubscription {
    let query = format!(
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND spell_id = {spell_id}"
    );
    subscribe_with_fence(shard, &[query.as_str()])
}

fn cast_rows(shard: &Standalone, casts: &TableSubscription) -> Vec<Row> {
    fence_subscription(shard, casts);
    event_rows(casts, "game_spell_cast_event_live")
}

fn is_completion(row: &Row) -> bool {
    row["is_completion"] == "true"
}

fn assert_item_gone(shard: &Standalone, casts: &TableSubscription) {
    let rows = cast_rows(shard, casts);
    assert!(!rows.iter().any(is_completion), "{rows:?}");
    let refusals: Vec<_> = rows
        .iter()
        .filter(|row| row["is_interrupted"] == "true")
        .collect();
    assert_eq!(refusals.len(), 1, "{rows:?}");
    assert_eq!(refusals[0]["failure_reason"], ITEM_GONE);
    assert!(pending_casts(shard).is_empty());
}

fn pending_casts(shard: &Standalone) -> Vec<Row> {
    shard.query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
}
