mod support;

use serde_json::{json, Value};
use support::Standalone;

const NEW_SPELL: u32 = 5_200_500;

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard
}

fn row(shard: &Standalone, table: &str, id: u32) -> Vec<Value> {
    shard
        .query_rows(&format!("SELECT * FROM {table} WHERE spell_id = {id}"))
        .into_iter()
        .map(|row| {
            Value::Object(
                row.into_iter()
                    .map(|(key, value)| {
                        let parsed = if key == "name" {
                            json!(value)
                        } else {
                            serde_json::from_str(&value).unwrap()
                        };
                        (key, parsed)
                    })
                    .collect(),
            )
        })
        .collect()
}

fn edit(shard: &Standalone, id: u32, create: bool) -> Value {
    let mut spell = row(shard, "game_spell", 133).remove(0);
    spell["spell_id"] = json!(id);
    spell["name"] = json!("Editor bolt");
    let effects: Vec<_> = row(shard, "game_spell_effect", 133)
        .into_iter()
        .map(|mut effect| {
            effect["spell_id"] = json!(id);
            effect["id"] = json!((u64::from(id) << 2) | effect["effect_index"].as_u64().unwrap());
            effect
        })
        .collect();
    json!({"create":create, "spell":spell, "effects":effects, "reagents":[], "trainers":[], "metadata":[],
        "source":[{"name":"entry","value":id.to_string()}, {"name":"name","value":"Editor bolt"}, {"name":"build","value":"5875"}]})
}

fn apply(shard: &Standalone, edits: Value) {
    shard.assert_call("operator_apply_spell_edits", &[&edits.to_string()]);
}

fn refusal(shard: &Standalone, edits: Value, expected: &str) {
    let out = shard.call("operator_apply_spell_edits", &[&edits.to_string()]);
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{message}");
    assert!(message.contains(expected), "{message}");
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn spell_edits_require_the_operator_even_for_empty_batches() {
    let shard = realm("operator-spells-authority");
    assert!(!shard
        .call_anonymous("operator_apply_spell_edits", &["[]"])
        .status
        .success());
    apply(&shard, json!([]));
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn spell_edits_replace_all_owned_rows_and_keep_known_spells_and_cooldowns() {
    let shard = realm("operator-spells-replace");
    let mut created = edit(&shard, NEW_SPELL, true);
    created["reagents"] =
        json!([{"id":u64::from(NEW_SPELL)<<3,"spell_id":NEW_SPELL,"item_entry":25,"count":2}]);
    created["metadata"] = json!([{"spell_id":NEW_SPELL,"exclude_caster":true}]);
    apply(&shard, json!([created]));
    assert_eq!(row(&shard, "game_spell_reagent", NEW_SPELL).len(), 1);
    assert_eq!(
        row(&shard, "game_creature_ai_spell_metadata", NEW_SPELL).len(),
        1
    );
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_learn_spell", &["1", &NEW_SPELL.to_string()]);
    shard.assert_sql(&format!("INSERT INTO game_trainer_spell (id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap) VALUES (90000000,620,{NEW_SPELL},100,1,0,75)"));
    let known = shard.query_rows("SELECT * FROM game_player_spell");
    assert!(!known.is_empty());
    let cooldowns = shard.query_rows("SELECT * FROM game_spell_cd");
    let auras = shard.query_rows("SELECT * FROM game_aura");
    let untouched = row(&shard, "game_spell", 133);
    let mut replacement = edit(&shard, NEW_SPELL, false);
    replacement["effects"] = json!([]);
    apply(&shard, json!([replacement]));
    for table in [
        "game_spell_effect",
        "game_spell_reagent",
        "game_creature_ai_spell_metadata",
        "game_trainer_spell",
    ] {
        assert!(row(&shard, table, NEW_SPELL).is_empty(), "{table}");
    }
    assert_eq!(shard.query_rows("SELECT * FROM game_player_spell"), known);
    assert_eq!(shard.query_rows("SELECT * FROM game_spell_cd"), cooldowns);
    assert_eq!(shard.query_rows("SELECT * FROM game_aura"), auras);
    assert_eq!(row(&shard, "game_spell", 133), untouched);
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT spell_id FROM game_operator_spell_source WHERE spell_id = {NEW_SPELL}"
            ))
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB CLI and Wasm toolchain"]
fn invalid_spell_batches_leave_templates_and_sources_unchanged() {
    let shard = realm("operator-spells-refusals");
    let first = edit(&shard, NEW_SPELL, true);
    for (id, create, message) in [
        (6_000_000, true, "Package-owned"),
        (50_000, false, "Fixture-Reserved"),
        (40_000, true, "Operator Template Range"),
        (40_000, false, "does not exist"),
    ] {
        refusal(&shard, json!([first, edit(&shard, id, create)]), message);
        assert!(row(&shard, "game_spell", NEW_SPELL).is_empty());
    }
    let mut invalid = edit(&shard, NEW_SPELL + 1, true);
    invalid["effects"][0]["id"] = json!(0);
    refusal(&shard, json!([first, invalid]), "effect key");
    refusal(&shard, json!([first, first]), "duplicate spell");
    refusal(&shard, Value::Array(vec![first.clone(); 65]), "at most 64");
    assert!(shard
        .query_rows("SELECT * FROM game_operator_spell_source")
        .is_empty());
    apply(&shard, json!([first]));
    refusal(&shard, json!([first]), "already used");
}
