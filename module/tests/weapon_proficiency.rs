mod support;

use support::{actor, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const TWO_HANDED_SWORDS: &str = "202";

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_item_instance");
    shard
}

fn spellbook(shard: &Standalone, guid: &str) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT spell_id FROM game_player_spell WHERE character_guid = {guid}"
        ))
        .into_iter()
        .map(|row| row["spell_id"].clone())
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_warrior_equips_a_two_handed_sword_only_after_learning_the_proficiency() {
    let shard = fixture("weapon-proficiency-equip");
    assert!(!spellbook(&shard, "1").contains(&TWO_HANDED_SWORDS.to_string()));
    shard.assert_sql(
        "UPDATE game_item_template SET class = 2, subclass = 8, inventory_type = 17 WHERE entry = 5090050",
    );
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);

    let before = shard.query_rows("SELECT * FROM game_item_instance");
    let refused = shard.call("gw_equip_item", &[ACTOR, "23"]);
    assert!(!refused.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(output.contains("item:no_proficiency"), "{output}");
    assert_eq!(shard.query_rows("SELECT * FROM game_item_instance"), before);

    shard.assert_call("debug_learn_spell", &["1", TWO_HANDED_SWORDS]);
    shard.assert_call("gw_equip_item", &[ACTOR, "23"]);
    let equipped = shard.query_rows("SELECT slot FROM game_item_instance WHERE owner_guid = 1");
    assert_eq!(equipped[0]["slot"], "15");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_new_character_learns_the_weapon_proficiencies_of_its_race_and_class() {
    let shard = fixture("weapon-proficiency-kit");
    shard.assert_call(
        "create_character",
        &["1", "\"Grunt\"", "2", "1", "0", "0", "0", "0", "0", "0"],
    );
    let orc =
        shard.query_rows("SELECT guid FROM game_character WHERE name = 'Grunt'")[0]["guid"].clone();
    let book = spellbook(&shard, &orc);
    for proficiency in ["196", "197", "201"] {
        assert!(
            book.contains(&proficiency.to_string()),
            "orc warrior lacks {proficiency}: {book:?}"
        );
    }
    assert!(!book.contains(&TWO_HANDED_SWORDS.to_string()));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn login_restores_a_missing_kit_weapon_proficiency() {
    let shard = fixture("weapon-proficiency-login");
    shard.assert_sql("DELETE FROM game_player_spell WHERE character_guid = 1 AND spell_id = 201");
    shard.assert_call("debug_grant_item", &["1", "5090050", "1"]);
    assert!(!shard.call("gw_equip_item", &[ACTOR, "23"]).status.success());

    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    shard.assert_call("gw_player_login", &["1", &actor("1")]);

    assert!(spellbook(&shard, "1").contains(&"201".to_string()));
    shard.assert_call("gw_equip_item", &[&actor("1"), "23"]);
    let blade = shard.query_rows("SELECT slot FROM game_item_instance WHERE entry = 5090050");
    assert_eq!(blade[0]["slot"], "15");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn repair_after_publish_adds_missing_weapon_kit_rows_once() {
    let shard = fixture("weapon-proficiency-repair");
    let kit = "SELECT * FROM game_createinfo_spell";
    let full = shard.query_rows(kit).len();
    for spell in ["196", "198", "201", "202"] {
        shard.assert_sql(&format!(
            "DELETE FROM game_createinfo_spell WHERE spell_id = {spell}"
        ));
    }
    assert!(shard.query_rows(kit).len() < full);

    shard.assert_call("debug_repair_after_publish", &[]);
    shard.assert_call("debug_repair_after_publish", &[]);

    assert_eq!(shard.query_rows(kit).len(), full);
    let human_warrior =
        shard.query_rows("SELECT spell_id FROM game_createinfo_spell WHERE race = 1 AND class = 1");
    for spell in ["196", "198", "201"] {
        assert!(
            human_warrior.iter().any(|row| row["spell_id"] == spell),
            "human warrior kit lacks {spell}"
        );
    }
    let undead_warrior =
        shard.query_rows("SELECT spell_id FROM game_createinfo_spell WHERE race = 5 AND class = 1");
    assert!(undead_warrior
        .iter()
        .any(|row| row["spell_id"] == TWO_HANDED_SWORDS));
}
