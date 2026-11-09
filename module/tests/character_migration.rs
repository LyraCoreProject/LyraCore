mod support;

use serde_json::{json, Value};
use support::Standalone;

const AREA_BIT: u32 = 37;
const FACTION: u32 = 5_099_400;
const FACTION_BASE: i32 = 100;
const SKILL: u32 = 5_099_500;
const TAXI_CLIENT_NODE: u32 = 300;
const TALENT: u32 = 5_099_100;
const ACTIVE_QUEST: u32 = 5_099_200;
const REWARDED_QUEST: u32 = 5_099_201;

struct Catalogue {
    bag: u32,
    stack: u32,
    spell: u32,
    talent_spell: u32,
}

fn realm(name: &str) -> (Standalone, Catalogue) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    if shard
        .query_rows("SELECT id FROM game_map WHERE id = 0")
        .is_empty()
    {
        shard.assert_sql(
            "INSERT INTO game_map (id, internal_name, name, map_type, linked_zone, parent, player_limit, reset_delay, ghost_entrance_map, ghost_entrance_x, ghost_entrance_y) \
             VALUES (0, 'Test', 'Test', 0, 0, 0, 0, 0, -1, 0, 0)",
        );
    }
    shard.assert_sql(
        "INSERT INTO game_map (id, internal_name, name, map_type, linked_zone, parent, player_limit, reset_delay, ghost_entrance_map, ghost_entrance_x, ghost_entrance_y) \
         VALUES (5099036, 'Dungeon', 'Dungeon', 1, 0, 0, 5, 0, -1, 0, 0)",
    );
    shard.assert_sql(&format!(
        "INSERT INTO game_area (id, map_id, parent_area_id, area_bit, flags, exploration_level, faction_group, name) \
         VALUES (5099300, 0, 0, {AREA_BIT}, 0, 0, 0, 'Migrated area')"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_faction (faction_id, reputation_index, base_standing) VALUES ({FACTION}, 50, {FACTION_BASE})"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_skill_line (skill_line, category, name) VALUES ({SKILL}, 0, 'Migrated skill')"
    ));
    shard.assert_sql(&format!(
        "INSERT INTO game_taxi_node (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) \
         VALUES (5099600,{TAXI_CLIENT_NODE},0,0,0,0,'Migrated node',0,1147)"
    ));
    for (quest, title) in [(ACTIVE_QUEST, "Active"), (REWARDED_QUEST, "Rewarded")] {
        shard.assert_sql(&format!(
            "INSERT INTO game_quest_template (entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,reward_money_max_level,quest_type) \
             VALUES ({quest},1,10,'{title}',0,0,0,0,0,0,0,0,0,0,0,0,false,0,0,0,0)"
        ));
    }
    shard.assert_sql(&format!(
        "INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) \
         VALUES (5099201,{ACTIVE_QUEST},0,0,1,3),(5099202,{ACTIVE_QUEST},1,1,1,1)"
    ));

    let items = shard.query_rows("SELECT entry, max_stack FROM game_item_template");
    let entry = |pick: fn(u32) -> bool| -> u32 {
        items
            .iter()
            .find(|row| pick(row["max_stack"].parse().unwrap()))
            .map(|row| row["entry"].parse().unwrap())
            .expect("the seeded world has a matching Item")
    };
    let bag = entry(|stack| stack == 1);
    let stack = entry(|stack| stack >= 5);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET container_slots = 6 WHERE entry = {bag}"
    ));
    let talent_spells: Vec<String> = shard
        .query_rows("SELECT spell_id FROM game_talent")
        .into_iter()
        .map(|row| row["spell_id"].clone())
        .collect();
    let spells: Vec<u32> = shard
        .query_rows("SELECT spell_id FROM game_spell")
        .iter()
        .filter(|row| !talent_spells.contains(&row["spell_id"]))
        .map(|row| row["spell_id"].parse().unwrap())
        .collect();
    let (spell, talent_spell) = (spells[0], spells[1]);
    shard.assert_sql(&format!(
        "INSERT INTO game_talent (talent_id,name,tree_id,tier,column,max_rank,spell_id,required_talent_id,required_points_in_tree) \
         VALUES ({TALENT},'Migrated talent',0,0,0,1,{talent_spell},0,0)"
    ));
    (
        shard,
        Catalogue {
            bag,
            stack,
            spell,
            talent_spell,
        },
    )
}

fn account(salt_byte: u8) -> Value {
    json!({
        "username": "migrant",
        "salt": format!("{salt_byte:02x}").repeat(32),
        "verifier": "22".repeat(32),
        "banned": false,
    })
}

fn position(map_id: u32, x: f32) -> Value {
    json!({"map_id": map_id, "zone_id": 12, "x": x, "y": 20.0, "z": 30.0, "orientation": 1.5})
}

fn item(entry: u32, location: Value, stack_count: u32) -> Value {
    json!({
        "entry": entry,
        "location": location,
        "stack_count": stack_count,
        "durability": 0,
        "enchant_id": 0,
        "random_property_id": 0,
        "soulbound": true,
        "text": {"none": []},
    })
}

fn migration(catalogue: &Catalogue, source_guid: u64, name: &str) -> Value {
    json!({
        "source": "vmangos-main",
        "source_guid": source_guid,
        "account": account(0x11),
        "name": name,
        "race": 1, "class": 1, "gender": 0,
        "skin": 1, "face": 2, "hair_style": 3, "hair_color": 4, "facial_hair": 5,
        "level": 20,
        "xp": 1000,
        "money": 12345,
        "position": {"some": position(0, 10.0)},
        "home": position(0, 50.0),
        "played_total_secs": 3600,
        "rested_xp": 500,
        "logout_resting": true,
        "last_logout_secs": 1_700_000_000u64,
        "bank_bag_slots": 1,
        "watched_faction_index": 50,
        "items": [
            item(catalogue.bag, json!({"Slot": 19}), 1),
            item(catalogue.stack, json!({"Bag": {"bag_slot": 19, "slot": 3}}), 5),
            item(catalogue.bag, json!({"Slot": 63}), 1),
            item(catalogue.stack, json!({"Bag": {"bag_slot": 63, "slot": 0}}), 2),
        ],
        "spells": [catalogue.spell, catalogue.talent_spell],
        "skills": [{"skill_line": SKILL, "current": 40, "max_rank": 100}],
        "reputations": [{"faction_id": FACTION, "earned_standing": 250, "at_war": false}],
        "actions": [{"button": 0, "action": catalogue.spell, "action_type": 0}],
        "quests": [
            {"quest_entry": REWARDED_QUEST, "rewarded": true, "log": {"some": {
                "event_credit": false,
                "failed": false,
                "deadline_secs": 1_700_000_000u64,
            }}},
            {"quest_entry": ACTIVE_QUEST, "rewarded": false, "log": {"some": {
                "event_credit": false,
                "failed": false,
                "deadline_secs": 1_700_000_000u64,
            }}},
        ],
        "explored_zones": [0, 1u32 << (AREA_BIT - 32)],
        "taxi_nodes": [TAXI_CLIENT_NODE],
    })
}

fn migrate(shard: &Standalone, migration: &Value) -> std::process::Output {
    shard.call("migrate_character", &[&migration.to_string()])
}

fn guid_of(shard: &Standalone, name: &str) -> String {
    shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE name = '{name}'"
    ))[0]["guid"]
        .clone()
}

fn refusal(output: &std::process::Output) -> String {
    assert!(!output.status.success(), "the migration should be refused");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_migrated_character_arrives_with_its_source_state() {
    let (shard, catalogue) = realm("character-migration-arrival");
    let output = migrate(&shard, &migration(&catalogue, 7, "Migrant"));
    shard.assert_output_success(&output, "migrate_character");

    let accounts =
        shard.query_rows("SELECT id, banned FROM game_account WHERE username = 'MIGRANT'");
    assert_eq!(accounts.len(), 1);
    let character = &shard.query_rows("SELECT * FROM game_character WHERE name = 'Migrant'")[0];
    assert_eq!(character["account_id"], accounts[0]["id"]);
    assert_eq!(character["level"], "20");
    assert_eq!(character["money"], "12345");
    assert_eq!(character["x"], "10");
    assert_eq!(character["home_x"], "50");
    assert_eq!(character["first_login"], "false");
    let guid = &character["guid"];

    let items = shard.query_rows(&format!(
        "SELECT entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}"
    ));
    let at = |slot: &str| {
        items
            .iter()
            .find(|row| row["slot"] == slot)
            .unwrap_or_else(|| panic!("no Item in slot {slot}: {items:?}"))
    };
    assert_eq!(at("19")["entry"], catalogue.bag.to_string());
    assert_eq!(at("123")["stack_count"], "5");
    assert_eq!(at("39")["entry"], catalogue.bag.to_string());
    assert_eq!(at("40")["stack_count"], "2");
    assert_eq!(items.len(), 4);

    let spells = shard.query_rows(&format!(
        "SELECT spell_id FROM game_player_spell WHERE character_guid = {guid}"
    ));
    assert_eq!(spells.len(), 1);
    assert_eq!(spells[0]["spell_id"], catalogue.spell.to_string());
    let talents = shard.query_rows(&format!(
        "SELECT talent_id, rank FROM game_character_talent WHERE character_guid = {guid}"
    ));
    assert_eq!(talents.len(), 1);
    assert_eq!(talents[0]["talent_id"], TALENT.to_string());
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT standing FROM game_player_reputation WHERE character_guid = {guid}"
        ))[0]["standing"],
        (FACTION_BASE + 250).to_string()
    );
    let quests = shard.query_rows(&format!(
        "SELECT quest_entry, rewarded, failed, deadline_micros, counts FROM game_character_quest WHERE character_guid = {guid}"
    ));
    let quest = |entry: u32| {
        quests
            .iter()
            .find(|row| row["quest_entry"] == entry.to_string())
            .expect("the quest arrived")
    };
    assert_eq!(quest(REWARDED_QUEST)["rewarded"], "true");
    let counts: Vec<u32> = quest(ACTIVE_QUEST)["counts"]
        .trim_matches(['[', ']'])
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.parse().expect("quest count is a number"))
        .collect();
    assert_eq!(counts, [0, 0]);
    assert_eq!(quest(ACTIVE_QUEST)["deadline_micros"], "0");
    assert_eq!(quest(ACTIVE_QUEST)["failed"], "false");
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT area_bit FROM game_character_explored WHERE character_guid = {guid}"
        ))[0]["area_bit"],
        AREA_BIT.to_string()
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT node_id FROM game_character_taxi_node WHERE character_guid = {guid}"
            ))
            .len(),
        1
    );
    assert_eq!(
        shard.query_rows("SELECT character_guid FROM game_character_migration WHERE source_key = 'vmangos-main/7'")
            [0]["character_guid"],
        *guid
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rerun_of_the_same_source_character_changes_nothing() {
    let (shard, catalogue) = realm("character-migration-rerun");
    let plan = migration(&catalogue, 7, "Migrant");
    shard.assert_output_success(&migrate(&shard, &plan), "first migrate_character");
    let guid = guid_of(&shard, "Migrant");
    shard.assert_output_success(&migrate(&shard, &plan), "second migrate_character");

    assert_eq!(
        shard
            .query_rows("SELECT guid FROM game_character WHERE name = 'Migrant'")
            .len(),
        1
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_item_instance WHERE owner_guid = {guid}"
            ))
            .len(),
        4
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_refused_migration_leaves_no_account_or_character() {
    let (shard, catalogue) = realm("character-migration-refusal");
    let mut plan = migration(&catalogue, 7, "Migrant");
    plan["items"][0]["entry"] = json!(4_000_000_000u32);

    assert!(refusal(&migrate(&shard, &plan)).contains("is not imported"));
    assert!(shard
        .query_rows("SELECT id FROM game_account WHERE username = 'MIGRANT'")
        .is_empty());
    assert!(shard
        .query_rows("SELECT guid FROM game_character WHERE name = 'Migrant'")
        .is_empty());
    assert!(shard
        .query_rows("SELECT source_key FROM game_character_migration")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_account_name_with_other_credentials_is_refused() {
    let (shard, catalogue) = realm("character-migration-account");
    shard.assert_call("migrate_account", &[&account(0x33).to_string()]);

    let text = refusal(&migrate(&shard, &migration(&catalogue, 7, "Migrant")));
    assert!(
        text.contains("already exists with other credentials"),
        "{text}"
    );
    assert!(shard
        .query_rows("SELECT guid FROM game_character WHERE name = 'Migrant'")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn characters_of_one_source_account_share_the_migrated_account() {
    let (shard, catalogue) = realm("character-migration-shared-account");
    shard.assert_output_success(
        &migrate(&shard, &migration(&catalogue, 7, "Migrant")),
        "first",
    );
    shard.assert_output_success(&migrate(&shard, &migration(&catalogue, 8, "Alt")), "second");

    let accounts = shard.query_rows("SELECT id FROM game_account WHERE username = 'MIGRANT'");
    assert_eq!(accounts.len(), 1);
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_character WHERE account_id = {}",
                accounts[0]["id"]
            ))
            .len(),
        2
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_saved_in_an_instance_arrives_at_its_home_bind() {
    let (shard, catalogue) = realm("character-migration-instance");
    let mut plan = migration(&catalogue, 7, "Migrant");
    plan["position"] = json!({"some": position(5099036, 10.0)});
    shard.assert_output_success(&migrate(&shard, &plan), "migrate_character");

    let character =
        &shard.query_rows("SELECT map_id, x FROM game_character WHERE name = 'Migrant'")[0];
    assert_eq!(character["map_id"], "0");
    assert_eq!(character["x"], "50");
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn letter(catalogue: &Catalogue, source_mail_id: u64, sender_kind: u8) -> Value {
    json!({
        "source": "vmangos-main",
        "source_mail_id": source_mail_id,
        "source_item_guid": 0,
        "recipient_source_guid": 7,
        "sender_kind": sender_kind,
        "sender_source_guid": 8,
        "sender_entry": 1234,
        "subject": "Your reward",
        "body": "Well done.",
        "attachment": {"some": {
            "entry": catalogue.stack,
            "stack_count": 3,
            "durability": 0,
            "enchant_id": 0,
            "random_property_id": 0,
            "soulbound": false,
            "text": {"some": "A copied letter"},
        }},
        "money": 500,
        "cod": 0,
        "checked": 0x11,
        "mail_template_id": 0,
        "expire_secs": now_secs() + 10 * 86_400,
        "deliver_secs": 1_700_000_000u64,
    })
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_migrated_letter_reaches_its_migrated_recipient_once() {
    let (shard, catalogue) = realm("character-migration-letter");
    shard.assert_output_success(
        &migrate(&shard, &migration(&catalogue, 7, "Migrant")),
        "Migrant",
    );
    shard.assert_output_success(&migrate(&shard, &migration(&catalogue, 8, "Alt")), "Alt");
    let plan = letter(&catalogue, 40, 0).to_string();
    shard.assert_call("migrate_letter", &[&plan]);
    shard.assert_call("migrate_letter", &[&plan]);

    let recipient = guid_of(&shard, "Migrant");
    let mail = shard.query_rows(&format!(
        "SELECT sender_guid, sender_kind, item_entry, item_text_id, money, was_read, check_flags FROM game_mail WHERE recipient_guid = {recipient}"
    ));
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0]["sender_guid"], guid_of(&shard, "Alt"));
    assert_eq!(mail[0]["item_entry"], catalogue.stack.to_string());
    assert_eq!(mail[0]["money"], "500");
    assert_eq!(mail[0]["was_read"], "true");
    assert_eq!(mail[0]["check_flags"], "16");
    let text = shard.query_rows(&format!(
        "SELECT text FROM game_item_text WHERE id = {}",
        mail[0]["item_text_id"]
    ));
    assert_eq!(text[0]["text"], "A copied letter");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_letter_for_a_character_that_was_not_migrated_is_refused() {
    let (shard, catalogue) = realm("character-migration-letter-recipient");
    let text = refusal(&shard.call("migrate_letter", &[&letter(&catalogue, 40, 3).to_string()]));
    assert!(text.contains("is not a migrated Character"), "{text}");
    assert!(shard.query_rows("SELECT id FROM game_mail").is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_letter_copy_in_the_inventory_keeps_its_text() {
    let (shard, catalogue) = realm("character-migration-letter-copy");
    let mut plan = migration(&catalogue, 7, "Migrant");
    plan["items"][1]["text"] = json!({"some": "Dear Migrant"});
    shard.assert_output_success(&migrate(&shard, &plan), "migrate_character");

    let guid = guid_of(&shard, "Migrant");
    let item = &shard.query_rows(&format!(
        "SELECT item_text_id FROM game_item_instance WHERE owner_guid = {guid} AND slot = 123"
    ))[0];
    let text = shard.query_rows(&format!(
        "SELECT text, letters FROM game_item_text WHERE id = {}",
        item["item_text_id"]
    ));
    assert_eq!(text[0]["text"], "Dear Migrant");
    assert_eq!(text[0]["letters"], "1");
}
