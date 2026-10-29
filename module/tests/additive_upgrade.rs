mod support;

use std::fs;

use serde_json::Value;
use support::{
    actor, event_rows, fence_subscription, module_bytes, poll_until, subscribe_with_fence, Row,
    Standalone, POLL_TIMEOUT,
};

const LEGACY_NOTIFICATIONS: &[&str] = &[
    "game_combat_event",
    "game_spell_cast_event",
    "game_spell_impact_event",
    "game_chat_event",
    "game_emote_event",
];

#[test]
#[ignore = "requires LYRACORE_TEST_BASELINE_WASM from 859c7257 and the SpacetimeDB 2.7.1 CLI"]
fn populated_old_module_upgrades_without_clearing() {
    let baseline_path = std::env::var_os("LYRACORE_TEST_BASELINE_WASM")
        .expect("set LYRACORE_TEST_BASELINE_WASM to the Module Wasm built at 859c7257c74d2c371dea8ed743cfbccd596cac11 with debug_reducers,package_test_fixture");
    let baseline = fs::read(baseline_path).expect("read the baseline Module Wasm");
    assert!(baseline.starts_with(b"\0asm"), "baseline is not Wasm");
    let current = module_bytes();
    let mut shard = populated_old_module(&baseline);
    let character_before = character(&shard);
    let entity_before = entity(&shard);
    let spell_before = shard.query_rows("SELECT * FROM game_spell WHERE spell_id = 5090100");
    let effect_before = shard.query_rows("SELECT * FROM game_spell_effect WHERE id = 20360400");
    for column in ["custom_flags", "min_target_level", "single_target_key"] {
        assert!(
            !spell_before[0].contains_key(column),
            "the baseline already has {column}"
        );
    }
    assert!(!effect_before[0].contains_key("bonus_coefficient"));
    let legacy_emotes = shard.query_rows("SELECT * FROM game_emote_event WHERE sender_guid = 1");
    assert!(legacy_emotes.is_empty(), "main emits only live emotes");
    let old_schema = shard.describe();

    shard.publish_module_bytes(current);

    assert_eq!(upgraded_character(&shard), character_before);
    assert_eq!(entity(&shard), entity_before);
    let mut spell_after = shard.query_rows("SELECT * FROM game_spell WHERE spell_id = 5090100");
    for column in ["custom_flags", "min_target_level", "single_target_key"] {
        assert_eq!(
            spell_after[0].remove(column).as_deref(),
            Some("0"),
            "{column}"
        );
    }
    assert_eq!(
        spell_after, spell_before,
        "published spell columns must survive"
    );
    let mut effect_after = shard.query_rows("SELECT * FROM game_spell_effect WHERE id = 20360400");
    assert_eq!(
        effect_after[0].remove("bonus_coefficient").as_deref(),
        Some("-1")
    );
    assert_eq!(
        effect_after, effect_before,
        "published effect columns must survive"
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_emote_event WHERE sender_guid = 1"),
        legacy_emotes
    );
    let new_schema = shard.describe();
    for name in LEGACY_NOTIFICATIONS {
        assert_eq!(
            table_schema(&new_schema, name),
            table_schema(&old_schema, name),
            "the published schema of {name} must stay intact"
        );
        table(&new_schema, &format!("{name}_live"));
    }

    shard.assert_call("debug_arm_instance_tick", &["0", "100"]);
    assert!(
        poll_until(POLL_TIMEOUT, || {
            let row = entity(&shard);
            row["simulation_indexed"] == "true"
                && row["regeneration_pending"] == "true"
                && row["combat_active"] == "true"
        }),
        "the scheduled backfill must derive simulation columns from preserved gameplay state"
    );
    assert_eq!(upgraded_character(&shard), character_before);

    let events = subscribe_with_fence(
        &shard,
        &[
            "SELECT * FROM game_emote_event_live WHERE sender_guid = 1",
            "SELECT * FROM game_emote_event WHERE sender_guid = 1",
        ],
    );
    send_emote(&shard, 35);
    fence_subscription(&shard, &events);
    let emotes = event_rows(&events, "game_emote_event_live");
    assert_eq!(emotes.len(), 1);
    assert_eq!(emotes[0]["sender_guid"], "1");
    assert_eq!(emotes[0]["text_emote"], "35");
    assert!(event_rows(&events, "game_emote_event").is_empty());
    assert!(shard
        .query_rows("SELECT * FROM game_emote_event_live")
        .is_empty());
    assert_eq!(
        shard.query_rows("SELECT * FROM game_emote_event WHERE sender_guid = 1"),
        legacy_emotes
    );
}

fn populated_old_module(baseline: &[u8]) -> Standalone {
    let mut shard = Standalone::start("additive-upgrade");
    shard.publish_module_bytes(baseline);
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_creature_move_schedule");
    shard.assert_sql("DELETE FROM game_melee_schedule");
    shard.assert_sql("DELETE FROM game_event_reaper_schedule");
    shard.assert_sql("UPDATE game_character SET money = 123456, xp = 37 WHERE guid = 1");
    let in_combat = lyracore_shared::constants::unit_flags::IN_COMBAT;
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 17, max_health = 100, power = 0, max_power = 0, \
         simulation_indexed = false, regeneration_pending = false, combat_active = false, \
         unit_flags = {in_combat}, combat_until_ms = {} WHERE guid = 1",
        u64::MAX
    ));
    shard.assert_sql_exact(
        "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
         cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
         aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
         family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
         VALUES (5090100, 'Upgrade fixture', 0, 5, 0, 0, 0, 30, 3000, 1, 0, 0, 0, 0, \
         0, 1, 0, false, 0, 0, 0, 0, 0, 0, 0, 18000)",
    );
    shard.assert_sql_exact(
        "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, die_sides, \
         per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, effect_mechanic, \
         p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
         VALUES (20360400, 5090100, 0, 1, 10, 0, 0.0, 0, 0, 0.0, 0, 0, 0, \
         0, 0, 0, 0, false, 2.5)",
    );
    send_emote(&shard, 34);
    shard
}

fn send_emote(shard: &Standalone, text_emote: u32) {
    shard.assert_call(
        "gw_send_emote",
        &[&actor("1"), &text_emote.to_string(), "66", "0"],
    );
}

fn character(shard: &Standalone) -> Row {
    shard
        .query_rows("SELECT * FROM game_character WHERE guid = 1")
        .remove(0)
}

fn upgraded_character(shard: &Standalone) -> Row {
    let mut row = character(shard);
    assert_eq!(row.remove("rest_state").as_deref(), Some("0"));
    row
}

fn entity(shard: &Standalone) -> Row {
    shard
        .query_rows("SELECT * FROM game_world_entity WHERE guid = 1")
        .remove(0)
}

fn table<'a>(schema: &'a Value, name: &str) -> &'a Value {
    schema["tables"]
        .as_array()
        .expect("Module tables")
        .iter()
        .find(|table| table["name"] == name)
        .unwrap_or_else(|| panic!("missing table {name}"))
}

fn table_schema(schema: &Value, name: &str) -> Value {
    let mut definition = table(schema, name).clone();
    let type_ref = definition
        .as_object_mut()
        .unwrap()
        .remove("product_type_ref")
        .expect("table row type")
        .as_u64()
        .expect("table row type reference") as usize;
    let types = schema["typespace"]["types"].as_array().expect("typespace");
    definition["row_type"] = resolve_type(&types[type_ref], types);
    definition
}

fn resolve_type(value: &Value, types: &[Value]) -> Value {
    match value {
        Value::Object(fields) => {
            if let Some(type_ref) = fields.get("Ref").and_then(Value::as_u64) {
                return resolve_type(&types[type_ref as usize], types);
            }
            Value::Object(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), resolve_type(value, types)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| resolve_type(value, types))
                .collect(),
        ),
        value => value.clone(),
    }
}
