mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::unit_flags;
use lyracore_shared::spell::taxonomy::{E_HEAL, T_SELF};
use support::{
    event_rows, fence_subscription, number, subscribe_with_fence, Row, Standalone,
    TableSubscription,
};

const PLAYER: &str = "1";
const PEACEFUL_SPELL: u32 = 5_098_201;
const COMBAT_SPELL: u32 = 5_098_202;
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

fn cast_then_take_a_hit(shard: &Standalone, spell_id: u32) -> TableSubscription {
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
    let query = format!(
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1 AND spell_id = {spell_id}"
    );
    let casts = subscribe_with_fence(shard, &[query.as_str()]);
    shard.assert_call("debug_cast_spell", &[PLAYER, &spell_id.to_string(), "0"]);
    assert_eq!(pending_casts(shard).len(), 1);
    shard.assert_call(
        "debug_stage_virtual_spell_impact",
        &[&wolf, PLAYER, "50", "50000"],
    );
    shard.assert_call("debug_advance_roll_batch", &["2"]);
    let player = shard.query_rows("SELECT unit_flags FROM game_world_entity WHERE guid = 1");
    assert_ne!(
        number::<u32>(&player[0], "unit_flags") & unit_flags::IN_COMBAT,
        0
    );
    casts
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
