mod support;

use serde_json::{json, Value};
use support::{number, poll_until, Standalone, POLL_TIMEOUT};

const FIRST: u32 = 5_200_500;
const CHICKEN: u64 = (0xF130_u64 << 48) | (620 << 24) | 1;

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_sql("INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (50909,0,0,0,0,0,0,0,0,0,0,0,0)");
    shard
}

fn template(shard: &Standalone, entry: u32) -> Option<support::Row> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_creature_template WHERE entry = {entry}"
        ))
        .pop()
}

fn fields(shard: &Standalone) -> Value {
    let mut fields = Value::Object(
        template(shard, 620)
            .unwrap()
            .into_iter()
            .filter(|(key, _)| key != "entry")
            .map(|(key, value)| {
                let value = if key == "name" || key == "subname" {
                    json!(value)
                } else {
                    serde_json::from_str(&value).unwrap()
                };
                (key, value)
            })
            .collect(),
    );
    fields["faction_template"] = json!(50909);
    fields["max_level"] = fields["level"].clone();
    fields["max_level_health"] = fields["health"].clone();
    fields
}

fn apply(shard: &Standalone, edits: Value) {
    shard.assert_call("operator_apply_creature_edits", &[&edits.to_string()]);
}

fn refusal(shard: &Standalone, edits: Value, expected: &str) {
    let output = shard.call("operator_apply_creature_edits", &[&edits.to_string()]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(text.contains(expected), "{text}");
}

fn row(shard: &Standalone, table: &str, guid: u64) -> Option<support::Row> {
    shard
        .query_rows(&format!("SELECT * FROM {table} WHERE guid = {guid}"))
        .pop()
}

fn loot() -> Value {
    json!({"item_entry": 25, "chance_bp": 7500, "count": 2, "group_id": 1, "quest_only": false})
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn only_the_operator_can_edit_creatures_even_in_an_empty_batch() {
    let shard = realm("operator-creatures-authority");
    assert!(!shard
        .call_anonymous("operator_apply_creature_edits", &["[]"])
        .status
        .success());
    apply(&shard, json!([]));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn create_uses_the_operator_range_and_edits_survive_plain_publish() {
    let mut shard = realm("operator-creatures-publish");
    apply(
        &shard,
        json!([
            {"createCreature": {"fields": fields(&shard)}},
            {"createCreature": {"fields": fields(&shard)}},
            {"setCreatureLoot": {"creature_entry": FIRST, "rows": [loot()]}}
        ]),
    );
    assert!(template(&shard, FIRST + 1).is_some());
    let before = template(&shard, FIRST).unwrap();
    let query = format!("SELECT * FROM game_creature_loot WHERE creature_entry = {FIRST}");
    let before_loot = shard.query_rows(&query);
    let mut wasm = support::module_bytes().to_vec();
    // The custom section changes the program hash without changing its definition.
    wasm.extend_from_slice(&[0, 2, 1, b'c']);
    shard.publish_module_bytes(&wasm);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(template(&shard, FIRST).unwrap(), before);
    assert_eq!(shard.query_rows(&query), before_loot);
    apply(
        &shard,
        json!([{"createCreature": {"fields": fields(&shard)}}]),
    );
    assert!(template(&shard, FIRST + 2).is_some());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn template_and_display_edits_respawn_world_creatures_but_leave_instance_copies() {
    let shard = realm("operator-creatures-respawn");
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_create_fixture_instance", &["1"]);
    let copy = shard
        .query_rows(&format!(
            "SELECT * FROM game_creature_spawn WHERE source_guid = {CHICKEN}"
        ))
        .pop()
        .unwrap();
    let copy_guid = number::<u64>(&copy, "guid");
    assert!(poll_until(POLL_TIMEOUT, || row(
        &shard,
        "game_world_entity",
        copy_guid
    )
    .is_some()));
    let copy_before = row(&shard, "game_world_entity", copy_guid).unwrap();
    let life = number::<u64>(
        &row(&shard, "game_creature_spawn", CHICKEN).unwrap(),
        "life_seq",
    );
    let mut fields = fields(&shard);
    fields["display_id"] = json!(100);
    fields["npc_flags"] = json!(129);
    fields["health"] = json!(234);
    fields["max_level_health"] = json!(234);
    apply(
        &shard,
        json!([{"updateCreature": {"entry": 620, "fields": fields}}]),
    );
    assert!(poll_until(POLL_TIMEOUT, || row(
        &shard,
        "game_world_entity",
        CHICKEN
    )
    .is_some_and(
        |row| number::<u32>(&row, "display_id") == 100 && number::<u32>(&row, "npc_flags") == 129
    )));
    assert_eq!(
        number::<u64>(
            &row(&shard, "game_creature_spawn", CHICKEN).unwrap(),
            "life_seq"
        ),
        life + 1
    );
    assert_eq!(
        number::<u32>(&template(&shard, 620).unwrap(), "health"),
        234
    );
    let displays = json!([
        {"display_id": 200, "scale": 2.0, "weight": 0, "gender": 1, "bounding_radius": 0.5},
        {"display_id": 300, "scale": 3.0, "weight": 1, "gender": 2, "bounding_radius": 0.75}
    ]);
    apply(
        &shard,
        json!([{"setCreatureDisplays": {"entry": 620, "displays": displays}}]),
    );
    assert!(poll_until(POLL_TIMEOUT, || row(
        &shard,
        "game_world_entity",
        CHICKEN
    )
    .is_some_and(
        |row| number::<u32>(&row, "display_id") == 300 && number::<f32>(&row, "scale_x") == 3.0
    )));
    assert_eq!(
        number::<u32>(&template(&shard, 620).unwrap(), "display_id"),
        200
    );
    assert_eq!(
        shard
            .query_rows("SELECT id FROM game_creature_display WHERE entry = 620")
            .len(),
        2
    );
    let copy_after = row(&shard, "game_world_entity", copy_guid).unwrap();
    for key in ["display_id", "npc_flags", "health", "max_health", "scale_x"] {
        assert_eq!(copy_before[key], copy_after[key], "{key}");
    }
    assert_eq!(
        number::<u64>(
            &row(&shard, "game_creature_spawn", copy_guid).unwrap(),
            "life_seq"
        ),
        number::<u64>(&copy, "life_seq")
    );
    apply(
        &shard,
        json!([{"setCreatureDisplays": {"entry": 620, "displays": []}}]),
    );
    assert!(shard
        .query_rows("SELECT id FROM game_creature_display WHERE entry = 620")
        .is_empty());
    assert!(poll_until(POLL_TIMEOUT, || row(
        &shard,
        "game_world_entity",
        CHICKEN
    )
    .is_some_and(|row| number::<u32>(&row, "display_id") == 200)));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn loot_lists_replace_imported_ids_skip_bands_and_preserve_owned_rows() {
    let shard = realm("operator-creatures-loot");
    for (table, key, variant, extra) in [
        (
            "game_creature_loot",
            "creature_entry",
            "setCreatureLoot",
            ",quest_only",
        ),
        (
            "game_pickpocket_loot",
            "creature_entry",
            "setPickpocketLoot",
            ",quest_only",
        ),
        ("game_skinning_loot", "skin_loot_id", "setSkinningLoot", ""),
    ] {
        let flag = if extra.is_empty() { "" } else { ",false" };
        shard.assert_sql(&format!("DELETE FROM {table} WHERE {key} = 620"));
        shard.assert_sql(&format!("INSERT INTO {table} (id,{key},item_entry,chance_bp,count,group_id{extra}) VALUES (5089999,620,25,10000,1,0{flag})"));
        let mut item = loot();
        if extra.is_empty() {
            item.as_object_mut().unwrap().remove("quest_only");
        }
        let edit = json!({variant: {key: 620, "rows": [item]}});
        apply(&shard, json!([edit]));
        let query = format!("SELECT * FROM {table} WHERE {key} = 620");
        let rows = shard.query_rows(&query);
        assert_eq!(rows.len(), 1);
        assert_eq!(number::<u64>(&rows[0], "id"), 5_100_000);
        assert_eq!(number::<u32>(&rows[0], "count"), 2);
        shard.assert_sql(&format!("INSERT INTO {table} (id,{key},item_entry,chance_bp,count,group_id{extra}) VALUES (8999999,621,25,10000,1,0{flag}),(9999999,621,25,10000,1,0{flag})"));
        apply(&shard, json!([edit]));
        assert_eq!(
            number::<u64>(&shard.query_rows(&query)[0], "id"),
            10_000_000
        );
        if variant != "setCreatureLoot" {
            shard.assert_sql(&format!("INSERT INTO {table} (id,{key},item_entry,chance_bp,count,group_id{extra}) VALUES (9000001,620,25,10000,1,0{flag})"));
            let before = shard.query_rows(&query);
            refusal(&shard, json!([edit]), "Package or Fixture-Reserved");
            assert_eq!(shard.query_rows(&query), before);
            shard.assert_sql(&format!("DELETE FROM {table} WHERE id = 9000001"));
        }
        apply(&shard, json!([{variant: {key: 620, "rows": []}}]));
        assert!(shard.query_rows(&query).is_empty());
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn spell_lists_and_fallback_cast_replace_and_remove() {
    let shard = realm("operator-creatures-spells");
    shard.assert_sql("INSERT INTO game_creature_spell (id,creature_entry,spell_id,priority,condition,condition_value) VALUES (9999999,620,133,0,0,0),(10999999,621,133,0,0,0)");
    let spells = json!([
        {"spell_id": 133, "priority": 7, "condition": 1, "condition_value": 25},
        {"spell_id": 133, "priority": 0, "condition": 2, "condition_value": 0}
    ]);
    apply(
        &shard,
        json!([
            {"setCreatureSpells": {"creature_entry": 620, "spells": spells}},
            {"setCreatureCast": {"creature_entry": 620, "spell_id": {"some": 133}}}
        ]),
    );
    let mut rows = shard.query_rows("SELECT * FROM game_creature_spell WHERE creature_entry = 620");
    rows.sort_by_key(|row| number::<u64>(row, "id"));
    assert_eq!(rows.len(), 2);
    assert_eq!(number::<u64>(&rows[0], "id"), 11_000_000);
    assert_eq!(number::<u8>(&rows[0], "priority"), 7);
    assert_eq!(number::<u8>(&rows[0], "condition_value"), 25);
    assert_eq!(
        number::<u32>(
            &shard.query_rows("SELECT spell_id FROM game_creature_cast WHERE creature_entry = 620")
                [0],
            "spell_id"
        ),
        133
    );
    for bad in [
        json!({"spell_id": 4000000000u32, "priority": 0, "condition": 0, "condition_value": 0}),
        json!({"spell_id": 133, "priority": 0, "condition": 4, "condition_value": 0}),
        json!({"spell_id": 133, "priority": 0, "condition": 1, "condition_value": 101}),
    ] {
        refusal(
            &shard,
            json!([{"setCreatureSpells": {"creature_entry": 620, "spells": [bad]}}]),
            "creature edit 0",
        );
    }
    apply(
        &shard,
        json!([
            {"setCreatureSpells": {"creature_entry": 620, "spells": []}},
            {"setCreatureCast": {"creature_entry": 620, "spell_id": null}}
        ]),
    );
    assert!(shard
        .query_rows("SELECT id FROM game_creature_spell WHERE creature_entry = 620")
        .is_empty());
    assert!(shard
        .query_rows("SELECT spell_id FROM game_creature_cast WHERE creature_entry = 620")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn refused_batches_keep_rows_lives_and_logs_and_unchanged_imported_values_pass() {
    let shard = realm("operator-creatures-rollback");
    let fields = fields(&shard);
    let before = template(&shard, 620).unwrap();
    let life = number::<u64>(
        &row(&shard, "game_creature_spawn", CHICKEN).unwrap(),
        "life_seq",
    );
    for (entry, reason) in [
        (15_000_000, "Package Creature Range"),
        (51_000, "Fixture-Reserved"),
        (5_090_000, "Fixture-Reserved"),
        (16_777_216, "16777215"),
        (999_999, "does not exist"),
    ] {
        refusal(
            &shard,
            json!([
                {"createCreature": {"fields": fields}},
                {"updateCreature": {"entry": 620, "fields": fields}},
                {"setCreatureLoot": {"creature_entry": FIRST, "rows": [loot()]}},
                {"updateCreature": {"entry": entry, "fields": fields}}
            ]),
            reason,
        );
        assert!(template(&shard, FIRST).is_none());
        assert_eq!(template(&shard, 620).unwrap(), before);
        assert_eq!(
            number::<u64>(
                &row(&shard, "game_creature_spawn", CHICKEN).unwrap(),
                "life_seq"
            ),
            life
        );
    }
    assert!(!String::from_utf8_lossy(&shard.module_logs(100).stdout)
        .contains("create creature 5200500"));
    apply(&shard, json!([{"createCreature": {"fields": fields}}]));
    shard.assert_sql(&format!("UPDATE game_creature_template SET faction_template = 4000000000, creature_type = 99 WHERE entry = {FIRST}"));
    let mut imported = fields.clone();
    imported["faction_template"] = json!(4_000_000_000u32);
    imported["creature_type"] = json!(99);
    imported["name"] = json!("Renamed");
    apply(
        &shard,
        json!([{"updateCreature": {"entry": FIRST, "fields": imported}}]),
    );
    imported["creature_type"] = json!(98);
    refusal(
        &shard,
        json!([{"updateCreature": {"entry": FIRST, "fields": imported}}]),
        "creature_type",
    );
    let mut invalid_loot = loot();
    invalid_loot["count"] = json!(0);
    for rows in [json!([invalid_loot]), json!([loot(), loot()])] {
        refusal(
            &shard,
            json!([{"setCreatureLoot": {"creature_entry": FIRST, "rows": rows}}]),
            "creature edit 0",
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn respawn_cap_refuses_the_whole_batch() {
    let shard = realm("operator-creatures-cap");
    const CAP: usize = 2_000;
    let spawn = row(&shard, "game_creature_spawn", CHICKEN).unwrap();
    let placement = json!({"map_id": number::<u32>(&spawn, "map_id"), "x": number::<f32>(&spawn, "x"),
        "y": number::<f32>(&spawn, "y"), "z": number::<f32>(&spawn, "z"), "orientation": 0.0,
        "movement_type": 0, "respawn_secs": 120});
    let edits = vec![json!({"createCreature": {"entry": 620, "placement": placement}}); CAP];
    for batch in edits.chunks(256) {
        shard.assert_call("operator_apply_spawn_edits", &[&json!(batch).to_string()]);
    }
    assert!(poll_until(POLL_TIMEOUT, || shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 620")
        .len()
        == CAP + 1));
    let before = template(&shard, 620).unwrap();
    let lives = shard.query_rows("SELECT guid,life_seq FROM game_creature_spawn WHERE entry = 620");
    refusal(
        &shard,
        json!([
            {"createCreature": {"fields": fields(&shard)}},
            {"updateCreature": {"entry": 620, "fields": fields(&shard)}}
        ]),
        "entry 620 exceeds the batch limit; a batch may respawn at most 2000 creatures",
    );
    assert!(template(&shard, FIRST).is_none());
    assert_eq!(template(&shard, 620).unwrap(), before);
    assert_eq!(
        shard.query_rows("SELECT guid,life_seq FROM game_creature_spawn WHERE entry = 620"),
        lives
    );
    let guid = lives
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .unwrap();
    shard.assert_call("operator_delete_creature_spawn", &[&guid.to_string()]);
    let mut updated = fields(&shard);
    updated["health"] = json!(234);
    updated["max_level_health"] = json!(234);
    let edit = json!({"updateCreature": {"entry": 620, "fields": updated}});
    apply(&shard, json!([edit, edit]));
    assert!(poll_until(POLL_TIMEOUT, || shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 620 AND health = 234")
        .len()
        == CAP));
    for life in shard.query_rows("SELECT guid,life_seq FROM game_creature_spawn WHERE entry = 620")
    {
        let before = lives
            .iter()
            .find(|row| row["guid"] == life["guid"])
            .unwrap();
        assert_eq!(
            number::<u64>(&life, "life_seq"),
            number::<u64>(before, "life_seq") + 1
        );
    }
}

#[test]
#[ignore = "capacity measurement; see docs/operator-content-api.md"]
fn benchmark_creature_template_respawns() {
    let shard = realm("operator-creatures-benchmark");
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/operator_creature_benchmark.py"
        ))
        .arg(shard.server())
        .arg(shard.shard_name())
        .env("LYRACORE_BENCH_TOKEN", shard.owner_token())
        .env("LYRACORE_BENCH_FIELDS", fields(&shard).to_string())
        .status()
        .expect("failed to start the creature benchmark");
    assert!(status.success(), "creature benchmark failed");
}
