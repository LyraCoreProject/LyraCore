mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::unit_flags;
use lyracore_shared::item::ItemRefusal;
use lyracore_shared::spell::taxonomy::{E_HEAL, E_TRIGGER, T_SELF};
use support::{
    actor, event_rows, fence_subscription, number, subscribe_with_fence, Row, Standalone,
    TableSubscription,
};

const PLAYER: &str = "1";
const PEACEFUL_SPELL: u32 = 5_098_201;
const COMBAT_SPELL: u32 = 5_098_202;
const PEACEFUL_ITEM: u32 = 5_098_203;
const TRIGGERING_SPELL: u32 = 5_098_204;
const PEACEFUL_TRIGGER_CHILD: u32 = 5_098_205;
// Spell.dbc Attributes bit SPELL_ATTR_NOT_IN_COMBAT_ONLY_PEACEFUL.
const NOT_IN_COMBAT_ONLY_PEACEFUL: u32 = 0x1000_0000;
const CAST_TIME_MS: u32 = 5000;
// `CastRefusalReason::Interrupted` in `game_spell_cast_event_live.failure_reason`.
const INTERRUPTED: &str = "2";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn entering_combat_interrupts_a_timed_non_combat_spell_at_once() {
    let shard = stage("non-combat-cast-interrupted");
    let casts = cast_then_take_a_hit(&shard, PEACEFUL_SPELL);
    let rows = cast_rows(&shard, &casts);
    let interrupts: Vec<_> = rows.iter().filter(|row| is_interrupt(row)).collect();
    assert_eq!(interrupts.len(), 1, "{rows:?}");
    assert_eq!(interrupts[0]["failure_reason"], INTERRUPTED);
    assert!(pending_casts(&shard).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn entering_combat_leaves_a_timed_spell_without_the_attribute_to_complete() {
    let shard = stage("non-combat-cast-kept");
    let casts = cast_then_take_a_hit(&shard, COMBAT_SPELL);
    assert!(!cast_rows(&shard, &casts).iter().any(is_interrupt));
    assert_eq!(pending_casts(&shard).len(), 1);

    shard.assert_call("debug_advance_roll_batch", &["120"]);
    let rows = cast_rows(&shard, &casts);
    assert!(
        rows.iter().any(|row| row["is_completion"] == "true"),
        "{rows:?}"
    );
    assert!(!rows.iter().any(is_interrupt), "{rows:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn entering_combat_interrupts_a_timed_item_cast_and_keeps_the_item() {
    let shard = stage("non-combat-item-kept");
    let slot = grant_peaceful_items(&shard);
    let wolf = spawn_wolf(&shard);
    let casts = subscribe_casts(&shard, PEACEFUL_SPELL);
    use_item(&shard, &slot);
    assert_eq!(pending_casts(&shard).len(), 1);
    assert_eq!(peaceful_stack(&shard), Some(2));

    take_a_hit(&shard, &wolf);
    let rows = cast_rows(&shard, &casts);
    assert_eq!(
        rows.iter().filter(|row| is_interrupt(row)).count(),
        1,
        "{rows:?}"
    );
    assert_eq!(peaceful_stack(&shard), Some(2));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_cast_uses_up_its_item_when_the_cast_goes_off() {
    let shard = stage("non-combat-item-used-up");
    let slot = grant_peaceful_items(&shard);
    let casts = subscribe_casts(&shard, PEACEFUL_SPELL);
    use_item(&shard, &slot);
    assert_eq!(peaceful_stack(&shard), Some(2));
    shard.assert_call("debug_advance_roll_batch", &["120"]);
    let rows = cast_rows(&shard, &casts);
    assert!(
        rows.iter().any(|row| row["is_completion"] == "true"),
        "{rows:?}"
    );
    assert_eq!(peaceful_stack(&shard), Some(1));

    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_time_ms = 0 WHERE spell_id = {PEACEFUL_SPELL}"
    ));
    use_item(&shard, &slot);
    assert_eq!(
        peaceful_stack(&shard),
        None,
        "an instant item cast uses it up at once"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_with_a_non_combat_spell_is_refused_as_not_in_combat() {
    let shard = stage("non-combat-item-refused");
    let slot = grant_peaceful_items(&shard);
    let player = shard.query_rows("SELECT unit_flags FROM game_world_entity WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {}, combat_until_ms = {} WHERE guid = 1",
        number::<u32>(&player[0], "unit_flags") | unit_flags::IN_COMBAT,
        u64::MAX
    ));
    let refused = shard.call("gw_use_item", &[&actor(PLAYER), &slot]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!refused.status.success(), "{text}");
    assert!(text.contains(ItemRefusal::NotInCombat.as_tag()), "{text}");
    assert!(pending_casts(&shard).is_empty());
    assert_eq!(peaceful_stack(&shard), Some(2));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_cast_with_the_non_combat_attribute_applies_in_combat() {
    let shard = stage("non-combat-trigger-child");
    trigger_fixture(&shard);
    let player = shard.query_rows("SELECT unit_flags FROM game_world_entity WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 5000, unit_flags = {}, combat_until_ms = {} \
         WHERE guid = 1",
        number::<u32>(&player[0], "unit_flags") | unit_flags::IN_COMBAT,
        u64::MAX
    ));

    cast_triggering_spell(&shard);
    assert!(health(&shard) > 5000, "the trigger child heals");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_trigger_child_cast_starts_no_global_cooldown() {
    let shard = stage("non-combat-trigger-gcd");
    trigger_fixture(&shard);
    shard.assert_sql("UPDATE game_world_entity SET health = 5000 WHERE guid = 1");

    cast_triggering_spell(&shard);
    assert!(health(&shard) > 5000, "the trigger child heals");
    assert!(shard
        .query_rows("SELECT id FROM game_global_cooldown WHERE caster_guid = 1")
        .is_empty());
}

fn trigger_fixture(shard: &Standalone) {
    for (spell_id, attributes, gcd_ms) in [
        (TRIGGERING_SPELL, 0, 0),
        (PEACEFUL_TRIGGER_CHILD, NOT_IN_COMBAT_ONLY_PEACEFUL, 1500),
    ] {
        shard.insert_row(
            "game_spell",
            &[
                ("spell_id", &spell_id),
                ("name", &format!("Trigger fixture {spell_id}")),
                ("attributes", &attributes),
                ("gcd_ms", &gcd_ms),
                ("spell_level", &1),
            ],
        );
    }
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(TRIGGERING_SPELL, 0)),
            ("spell_id", &TRIGGERING_SPELL),
            ("effect_index", &0u8),
            ("kind", &E_TRIGGER),
            ("target", &T_SELF),
            ("trigger_spell", &PEACEFUL_TRIGGER_CHILD),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PEACEFUL_TRIGGER_CHILD, 0)),
            ("spell_id", &PEACEFUL_TRIGGER_CHILD),
            ("effect_index", &0u8),
            ("kind", &E_HEAL),
            ("base_points", &100),
            ("target", &T_SELF),
        ],
    );
    shard.assert_call(
        "debug_learn_spell",
        &[PLAYER, &TRIGGERING_SPELL.to_string()],
    );
}

fn cast_triggering_spell(shard: &Standalone) {
    shard.assert_call(
        "debug_cast_spell",
        &[PLAYER, &TRIGGERING_SPELL.to_string(), "0"],
    );
}

fn health(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT health FROM game_world_entity WHERE guid = 1")[0],
        "health",
    )
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
    timed_spell(&shard, PEACEFUL_SPELL, NOT_IN_COMBAT_ONLY_PEACEFUL);
    timed_spell(&shard, COMBAT_SPELL, 0);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard
}

fn timed_spell(shard: &Standalone, spell_id: u32, attributes: u32) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Timed self heal fixture {spell_id}")),
            ("cast_time_ms", &CAST_TIME_MS),
            ("attributes", &attributes),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("effect_index", &0u8),
            ("kind", &E_HEAL),
            ("base_points", &1),
            ("target", &T_SELF),
        ],
    );
    shard.assert_call("debug_learn_spell", &[PLAYER, &spell_id.to_string()]);
}

fn grant_peaceful_items(shard: &Standalone) -> String {
    shard.insert_row(
        "game_item_template",
        &[
            ("entry", &PEACEFUL_ITEM),
            ("name", &"Timed peaceful fixture item"),
            ("max_stack", &5),
            ("spellid_1", &PEACEFUL_SPELL),
            ("spellcharges_1", &-1),
        ],
    );
    shard.assert_call(
        "debug_grant_item",
        &[PLAYER, &PEACEFUL_ITEM.to_string(), "2"],
    );
    peaceful_items(shard).remove(0)["slot"].clone()
}

fn peaceful_items(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT slot, stack_count FROM game_item_instance WHERE owner_guid = 1 \
         AND entry = {PEACEFUL_ITEM}"
    ))
}

fn peaceful_stack(shard: &Standalone) -> Option<u32> {
    let items = peaceful_items(shard);
    assert!(items.len() <= 1, "{items:?}");
    items.first().map(|item| number(item, "stack_count"))
}

fn use_item(shard: &Standalone, slot: &str) {
    shard.assert_call("gw_use_item", &[&actor(PLAYER), slot]);
}

fn spawn_wolf(shard: &Standalone) -> String {
    shard.assert_call(
        "debug_spawn_at_feet",
        &[PLAYER, &TEST_WOLF_ENTRY.to_string(), "5"],
    );
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .pop()
        .expect("the spawned wolf must exist")["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false WHERE guid = {wolf}"
    ));
    wolf
}

fn subscribe_casts(shard: &Standalone, spell_id: u32) -> TableSubscription {
    let query = format!(
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND spell_id = {spell_id}"
    );
    subscribe_with_fence(shard, &[query.as_str()])
}

fn cast_then_take_a_hit(shard: &Standalone, spell_id: u32) -> TableSubscription {
    let wolf = spawn_wolf(shard);
    let casts = subscribe_casts(shard, spell_id);
    shard.assert_call("debug_cast_spell", &[PLAYER, &spell_id.to_string(), "0"]);
    assert_eq!(pending_casts(shard).len(), 1);
    take_a_hit(shard, &wolf);
    casts
}

fn take_a_hit(shard: &Standalone, wolf: &str) {
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &[wolf, PLAYER, "50", "50000"],
    );
    shard.assert_call("debug_advance_roll_batch", &["2"]);
    let player = shard.query_rows("SELECT unit_flags FROM game_world_entity WHERE guid = 1");
    assert_ne!(
        number::<u32>(&player[0], "unit_flags") & unit_flags::IN_COMBAT,
        0
    );
}

fn cast_rows(shard: &Standalone, casts: &TableSubscription) -> Vec<Row> {
    fence_subscription(shard, casts);
    event_rows(casts, "game_spell_cast_event_live")
}

fn is_interrupt(row: &Row) -> bool {
    row["is_interrupted"] == "true"
}

fn pending_casts(shard: &Standalone) -> Vec<Row> {
    shard.query_rows("SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = 1")
}
