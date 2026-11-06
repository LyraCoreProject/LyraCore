use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lyracore_test_support::Standalone;
use rusqlite::Connection;
use serde_json::{json, Value};

const OPERATOR_ITEM: u32 = 5_200_500;

fn source(path: &Path) {
    let profile: Value =
        serde_json::from_str(include_str!("../manifests/vmangos-4641790.json")).unwrap();
    let db = Connection::open(path).unwrap();
    for (table, contract) in profile["tables"].as_object().unwrap() {
        let mut columns = Vec::new();
        for (kind, names) in contract["columns"].as_object().unwrap() {
            for name in names.as_array().unwrap() {
                let default = if kind == "text" { "''" } else { "0" };
                columns.push(format!(
                    "\"{}\" {kind} DEFAULT {default}",
                    name.as_str().unwrap()
                ));
            }
        }
        let key = match table.as_str() {
            "item_template" | "creature_template" => Some("entry,patch"),
            "spell_template" => Some("entry,build"),
            "npc_vendor" | "npc_vendor_template" => Some("entry,item"),
            "creature_loot_template" | "reference_loot_template" => {
                Some("entry,item,groupid,patch_min")
            }
            "forbidden_items" => Some("entry"),
            "item_enchantment_template" => Some("entry,ench"),
            _ => None,
        };
        if let Some(key) = key {
            columns.push(format!("PRIMARY KEY ({key})"));
        }
        db.execute_batch(&format!("CREATE TABLE \"{table}\" ({})", columns.join(",")))
            .unwrap();
    }
    db.execute_batch(
        "INSERT INTO item_template (entry,patch,name,display_id,quality,buy_count,stackable,allowable_class,allowable_race) VALUES
            (1000,10,'Source blade',1832,1,1,20,-1,-1),(1001,10,'Source cloth',1832,1,1,20,-1,-1);
         INSERT INTO creature_template (entry,patch,vendor_id,loot_id) VALUES (100,10,0,3);
         INSERT INTO npc_vendor (entry,item,slot) VALUES (100,1000,0),(100,1001,1);
         INSERT INTO creature_loot_template (entry,item,ChanceOrQuestChance,mincountOrRef,maxcount,patch_min,patch_max) VALUES (3,1000,100,1,1,0,10);",
    )
    .unwrap();
}

fn import(shard: &Standalone, path: &Path) -> Output {
    import_with_packages(shard, path, None)
}

fn import_with_packages(shard: &Standalone, path: &Path, packages: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lyracore-importer"));
    command
        .env("LYRACORE_SPACETIME_CONFIG_PATH", shard.cli_config_path())
        .args([
            "--dump",
            path.to_str().unwrap(),
            "--family",
            "items",
            "--world-profile",
            "full-world",
            "--source-sha",
            "1111111111111111111111111111111111111111",
            "--apply",
            "--server",
            shard.server(),
            "--db",
            shard.shard_name(),
        ]);
    if let Some(packages) = packages {
        command.arg("--packages").arg(packages);
    }
    command.output().unwrap()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn character(shard: &Standalone, name: &str, account: &str) -> String {
    shard.assert_call(
        "create_character",
        &[
            account,
            &json!(name).to_string(),
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE name = '{name}'"
    ))[0]["guid"]
        .clone()
}

fn edits(shard: &Standalone, value: Value) {
    shard.assert_call("operator_apply_item_edits", &[&value.to_string()]);
}

fn fields(shard: &Standalone) -> Value {
    let row = shard
        .query_rows("SELECT * FROM game_item_template WHERE entry = 1000")
        .pop()
        .unwrap();
    Value::Object(
        row.into_iter()
            .filter(|(key, _)| {
                key != "entry"
                    && key != "damage_school"
                    && !key.starts_with("spellppmrate_")
                    && !key.starts_with("spellcategory_")
            })
            .map(|(key, value)| {
                let value = if key == "name" {
                    json!("Operator blade")
                } else {
                    serde_json::from_str(&value).unwrap()
                };
                (key, value)
            })
            .collect(),
    )
}

fn populated(name: &str) -> (Standalone, PathBuf, String, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let credential = json!(vec![0u8; 32]).to_string();
    shard.assert_call(
        "provision_account",
        &[r#""REIMPORT""#, &credential, &credential],
    );
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'REIMPORT'")[0]
        ["id"]
        .clone();
    let first = character(&shard, "Tethys", &account);
    let second = character(&shard, "Phoebe", &account);
    shard.assert_sql("DELETE FROM game_item_instance");
    let path = shard.cli_config_path().with_file_name("items.sqlite");
    source(&path);
    let output = import(&shard, &path);
    assert!(output.status.success(), "{}", output_text(&output));
    edits(&shard, json!([{"createItem": {"fields": fields(&shard)}}]));
    shard.assert_call("debug_spawn_player_entity", &[&first]);
    shard.assert_call("debug_spawn_player_entity", &[&second]);
    shard.assert_call("debug_grant_item", &[&first, "1000", "2"]);
    shard.assert_call(
        "debug_grant_item",
        &[&second, &OPERATOR_ITEM.to_string(), "1"],
    );
    edits(
        &shard,
        json!([{"setVendorOffers": {"creature_entry": 620, "offers": [{"item_entry": OPERATOR_ITEM, "max_count": 0}]}}]),
    );
    shard.assert_sql(&format!("INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (20,620,{OPERATOR_ITEM},10000,1,0,false)"));
    (shard, path, first, second)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn populated_items_reimport_preserves_owned_items_and_operator_references() {
    let (shard, path, first, second) = populated("items-reimport-preserve");
    let items = shard.query_rows("SELECT * FROM game_item_instance");
    let operator = shard.query_rows(&format!(
        "SELECT * FROM game_item_template WHERE entry = {OPERATOR_ITEM}"
    ));
    let offers = shard.query_rows(&format!(
        "SELECT * FROM game_npc_vendor WHERE item_entry = {OPERATOR_ITEM}"
    ));
    let loot = shard.query_rows(&format!(
        "SELECT * FROM game_creature_loot WHERE item_entry = {OPERATOR_ITEM}"
    ));
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE item_template SET name = 'Updated source' WHERE entry = 1000",
            [],
        )
        .unwrap();
    let output = import(&shard, &path);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_item_template WHERE entry = {OPERATOR_ITEM}"
        )),
        operator
    );
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), items);
    assert_eq!(
        shard.query_rows("SELECT name FROM game_item_template WHERE entry = 1000")[0]["name"],
        "Updated source"
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_npc_vendor WHERE item_entry = {OPERATOR_ITEM}"
        )),
        offers
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT * FROM game_creature_loot WHERE item_entry = {OPERATOR_ITEM}"
        )),
        loot
    );
    for guid in [&first, &second] {
        assert_eq!(
            shard
                .query_rows(&format!(
                    "SELECT guid FROM game_character WHERE guid = {guid}"
                ))
                .len(),
            1
        );
    }
    edits(&shard, json!([{"createItem": {"fields": fields(&shard)}}]));
    assert_eq!(
        shard
            .query_rows("SELECT entry FROM game_item_template WHERE entry = 5200501")
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn populated_items_reimport_refuses_removed_owned_source_templates_before_writes() {
    let (shard, path, _, second) = populated("items-reimport-missing");
    shard.assert_call("debug_grant_item", &[&second, "1001", "1"]);
    Connection::open(&path).unwrap().execute_batch("DELETE FROM item_template WHERE entry = 1001; DELETE FROM npc_vendor WHERE item = 1001;").unwrap();
    let before = snapshot(&shard);
    let output = import(&shard, &path);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("1001"),
        "{}",
        output_text(&output)
    );
    assert_eq!(snapshot(&shard), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn populated_items_reimport_refuses_retained_row_id_collisions_before_writes() {
    for table in ["game_npc_vendor", "game_creature_loot"] {
        let (shard, path, _, _) = populated("items-reimport-collision");
        shard.assert_sql(&format!("DELETE FROM {table} WHERE id = 1"));
        let sql = if table == "game_npc_vendor" {
            format!("INSERT INTO game_npc_vendor (id,creature_entry,item_entry,slot,max_count,restock_seconds,stock_flags) VALUES (1,620,{OPERATOR_ITEM},0,0,0,0)")
        } else {
            format!("INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (1,620,{OPERATOR_ITEM},10000,1,0,false)")
        };
        shard.assert_sql(&sql);
        let before = snapshot(&shard);
        let output = import(&shard, &path);
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(
            output_text(&output).contains(table),
            "{}",
            output_text(&output)
        );
        assert_eq!(snapshot(&shard), before);
    }
}

fn snapshot(shard: &Standalone) -> Vec<Vec<std::collections::BTreeMap<String, String>>> {
    [
        "game_item_template",
        "game_item_instance",
        "game_creature_loot",
        "game_npc_vendor",
        "game_vendor_stock",
        "game_import_meta",
        "game_package_import",
    ]
    .map(|table| shard.query_rows(&format!("SELECT * FROM {table}")))
    .to_vec()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn populated_items_reimport_refuses_package_replay_that_removes_an_owned_template() {
    let (shard, path, _, second) = populated("items-reimport-packages");
    let row = shard
        .query_rows(&format!(
            "SELECT * FROM game_item_template WHERE entry = {OPERATOR_ITEM}"
        ))
        .pop()
        .unwrap();
    let fields: serde_json::Map<String, Value> = lyracore_package_delta::Table::Item
        .columns()
        .iter()
        .map(|column| {
            let value = if column.ty == lyracore_package_delta::FieldType::Str {
                json!(row[column.name])
            } else {
                serde_json::from_str(&row[column.name]).unwrap()
            };
            (
                column.name.to_owned(),
                json!({"type":column.ty.as_str(), "value":value}),
            )
        })
        .collect();
    let artifact = json!({"version":1,"package":"reimport","source_hash":"1".repeat(64),"claims":[{"table":"game_item_template","key":{"entry":7000000},"operation":"insert","fields":fields}]});
    shard.assert_call(
        "apply_package_deltas",
        &[r#""items""#, &json!(artifact.to_string()).to_string()],
    );
    shard.assert_call("debug_grant_item", &[&second, "7000000", "1"]);
    let packages = path.with_file_name("packages");
    std::fs::create_dir(&packages).unwrap();
    let before = snapshot(&shard);
    let output = import_with_packages(&shard, &path, Some(&packages));
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("7000000"),
        "{}",
        output_text(&output)
    );
    assert_eq!(snapshot(&shard), before);
    let generated = packages.join("reimport/data/.generated");
    std::fs::create_dir_all(&generated).unwrap();
    std::fs::write(generated.join("items.json"), artifact.to_string()).unwrap();
    let items = shard.query_rows("SELECT * FROM game_item_instance");
    let output = import_with_packages(&shard, &path, Some(&packages));
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), items);
    assert_eq!(
        shard
            .query_rows("SELECT entry FROM game_item_template WHERE entry = 7000000")
            .len(),
        1
    );
}
