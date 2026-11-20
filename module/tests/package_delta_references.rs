mod support;

use lyracore_package_delta::fixture_ids::{
    BATTERED_CHEST, FIXTURE_MOUNT_SPELL, LEARN_COOKING_SPELL_ID, PROFESSION_TRAINER_ENTRY,
    SCENARIO_QUEST, TEST_WOLF_ENTRY,
};
use lyracore_shared::guid;
use lyracore_shared::spell::taxonomy::{
    A_FORM_ENTRY_CHANCE, A_MOD_RESISTANCE, A_SHAPESHIFT, P_FORM, P_SCHOOL_MASK, RESIST_ARMOR,
};
use support::Standalone;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const IMPORTED_CREATURE: u32 = 620;
const REAL_ITEM: u32 = 25;
const MISSING_ITEM: u32 = 4_000_000;
const PACKAGE_LOOT: u64 = 9_000_001;
const REAL_SPELL: u32 = FIXTURE_MOUNT_SPELL;
const MISSING_SPELL: u32 = 4_000_000;
const MISSING_CREATURE: u32 = 4_000_000;
const PACKAGE_CREATURE_SPELL: u64 = 10_000_001;
const FIXTURE_TARGETED_CREATURE_SPELL: u64 = 4_000_001;
const PACKAGE_TRAINER_SPELL: u64 = 11_000_001;
const PACKAGE_NPC_TEXT: u64 = 12_000_001;
const PACKAGE_NPC_TEXT_SLOT: u64 = 12_000_002;
const PACKAGE_GOSSIP_OPTION: u64 = 12_000_003;
const MISSING_NPC_TEXT: u32 = 4_000_000;
const PACKAGE_GRAVEYARD_ZONE: u64 = 13_000_001;
const PACKAGE_CREATEINFO_SPELL: u64 = 13_000_002;
const REAL_GRAVEYARD: u32 = 105;
const MISSING_GRAVEYARD: u32 = 4_000_000;
const PACKAGE_SPELL_LEARN: u64 = 14_000_001;
const PACKAGE_SPELL: u32 = 6_000_001;
const PACKAGE_QUEST: u32 = 8_000_001;

fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

fn artifact(package: &str, claim: &str) -> String {
    format!(r#"{{"version":1,"package":"{package}","source_hash":"{HASH}","claims":[{claim}]}}"#)
}

fn quest_source_item(item_entry: u32) -> String {
    artifact(
        "example.quest",
        &format!(
            r#"{{"table":"game_quest_template","key":{{"entry":{SCENARIO_QUEST}}},"operation":"update","fields":{{"src_item":{{"type":"u32","value":{item_entry}}}}}}}"#
        ),
    )
}

fn pickpocket_loot(item_entry: u32) -> String {
    artifact(
        "example.loot",
        &format!(
            r#"{{"table":"game_pickpocket_loot","key":{{"id":{PACKAGE_LOOT}}},"operation":"insert","fields":{{"creature_entry":{{"type":"u32","value":{TEST_WOLF_ENTRY}}},"item_entry":{{"type":"u32","value":{item_entry}}},"chance_bp":{{"type":"u32","value":10000}},"count":{{"type":"u32","value":1}},"group_id":{{"type":"u32","value":0}},"quest_only":{{"type":"bool","value":false}}}}}}"#
        ),
    )
}

fn creature_spell_insert(creature_entry: u32, spell_id: u32) -> String {
    artifact(
        "example.cast",
        &format!(
            r#"{{"table":"game_creature_spell","key":{{"id":{PACKAGE_CREATURE_SPELL}}},"operation":"insert","fields":{{"creature_entry":{{"type":"u32","value":{creature_entry}}},"spell_id":{{"type":"u32","value":{spell_id}}},"priority":{{"type":"u8","value":10}},"condition":{{"type":"u8","value":0}},"condition_value":{{"type":"u8","value":0}}}}}}"#
        ),
    )
}

fn creature_spell_update(id: u64, fields: &str) -> String {
    artifact(
        "example.cast",
        &format!(
            r#"{{"table":"game_creature_spell","key":{{"id":{id}}},"operation":"update","fields":{fields}}}"#
        ),
    )
}

fn trainer_spell_insert(spell_id: u32, learn_skill_line: u32) -> String {
    artifact(
        "example.trainer",
        &format!(
            r#"{{"table":"game_trainer_spell","key":{{"id":{PACKAGE_TRAINER_SPELL}}},"operation":"insert","fields":{{"trainer_entry":{{"type":"u32","value":{PROFESSION_TRAINER_ENTRY}}},"spell_id":{{"type":"u32","value":{spell_id}}},"cost":{{"type":"u32","value":500}},"required_level":{{"type":"u8","value":10}},"learn_skill_line":{{"type":"u32","value":{learn_skill_line}}},"learn_skill_cap":{{"type":"u32","value":75}}}}}}"#
        ),
    )
}

fn npc_text_insert() -> String {
    artifact(
        "example.gossip.text",
        &format!(
            r#"{{"table":"game_npc_text","key":{{"text_id":{PACKAGE_NPC_TEXT}}},"operation":"insert","fields":{{"text":{{"type":"string","value":"The forge is cold, friend."}}}}}}"#
        ),
    )
}

fn npc_text_slot_insert(text_id: u32) -> String {
    artifact(
        "example.gossip.slot",
        &format!(
            r#"{{"table":"game_npc_text_slot","key":{{"id":{PACKAGE_NPC_TEXT_SLOT}}},"operation":"insert","fields":{{"text_id":{{"type":"u32","value":{text_id}}},"slot_index":{{"type":"u8","value":0}},"text_male":{{"type":"string","value":"Well met."}},"text_female":{{"type":"string","value":"Well met."}},"probability":{{"type":"f32","value":1.0}}}}}}"#
        ),
    )
}

fn gossip_option_insert(entry: u32) -> String {
    artifact(
        "example.gossip.option",
        &format!(
            r#"{{"table":"game_gossip_option","key":{{"row_id":{PACKAGE_GOSSIP_OPTION}}},"operation":"insert","fields":{{"entry":{{"type":"u32","value":{entry}}},"option_index":{{"type":"u32","value":0}},"icon":{{"type":"u32","value":0}},"text":{{"type":"string","value":"Tell me of the forge."}},"action":{{"type":"u32","value":1}},"action_menu_id":{{"type":"u32","value":0}},"cond_type":{{"type":"u32","value":0}},"cond_value1":{{"type":"u32","value":0}},"cond_value2":{{"type":"u32","value":0}}}}}}"#
        ),
    )
}

fn graveyard_zone_insert(safe_loc_id: u32) -> String {
    artifact(
        "example.globals.graveyard",
        &format!(
            r#"{{"table":"game_graveyard_zone","key":{{"row_id":{PACKAGE_GRAVEYARD_ZONE}}},"operation":"insert","fields":{{"safe_loc_id":{{"type":"u32","value":{safe_loc_id}}},"zone_id":{{"type":"u32","value":12}},"faction":{{"type":"u32","value":469}}}}}}"#
        ),
    )
}

fn createinfo_spell_insert(spell_id: u32) -> String {
    artifact(
        "example.globals.createinfo",
        &format!(
            r#"{{"table":"game_createinfo_spell","key":{{"id":{PACKAGE_CREATEINFO_SPELL}}},"operation":"insert","fields":{{"race":{{"type":"u8","value":1}},"class":{{"type":"u8","value":1}},"spell_id":{{"type":"u32","value":{spell_id}}}}}}}"#
        ),
    )
}

fn class_level_stats_insert() -> String {
    artifact(
        "example.globals.curve",
        r#"{"table":"game_class_level_stats","key":{"class":1,"level":10},"operation":"insert","fields":{"base_health":{"type":"u32","value":300},"base_mana":{"type":"u32","value":0}}}"#,
    )
}

fn spell_chain_insert(spell_id: u32) -> String {
    artifact(
        "example.spellmeta.chain",
        &format!(
            r#"{{"table":"game_spell_chain","key":{{"spell_id":{spell_id}}},"operation":"insert","fields":{{"prev_spell":{{"type":"u32","value":0}},"first_spell":{{"type":"u32","value":{spell_id}}},"rank":{{"type":"u8","value":1}},"req_spell":{{"type":"u32","value":0}}}}}}"#
        ),
    )
}

fn spell_learn_insert(learn_spell: u32) -> String {
    artifact(
        "example.spellmeta.learn",
        &format!(
            r#"{{"table":"game_spell_learn","key":{{"id":{PACKAGE_SPELL_LEARN}}},"operation":"insert","fields":{{"parent_spell":{{"type":"u32","value":{REAL_SPELL}}},"learn_spell":{{"type":"u32","value":{learn_spell}}}}}}}"#
        ),
    )
}

fn apply(standalone: &Standalone, family: &str, delta: &str) -> std::process::Output {
    standalone.call("apply_package_deltas", &[&arg(family), &arg(delta)])
}

fn refusal_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn quest_and_loot_claims_refuse_missing_cross_table_references() {
    let mut standalone = Standalone::start("package-delta-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused_quest = apply(&standalone, "quests", &quest_source_item(MISSING_ITEM));
    assert!(!refused_quest.status.success());
    let quest_refusal = format!(
        "{}{}",
        String::from_utf8_lossy(&refused_quest.stdout),
        String::from_utf8_lossy(&refused_quest.stderr)
    );
    assert!(quest_refusal.contains("src_item"), "{quest_refusal}");

    let accepted_quest = apply(&standalone, "quests", &quest_source_item(REAL_ITEM));
    assert!(accepted_quest.status.success());
    let quest = standalone.query_rows(&format!(
        "SELECT * FROM game_quest_template WHERE entry = {SCENARIO_QUEST}"
    ));
    assert_eq!(quest[0]["src_item"], REAL_ITEM.to_string());

    let refused_loot = apply(&standalone, "loot", &pickpocket_loot(MISSING_ITEM));
    assert!(!refused_loot.status.success());
    let loot_refusal = format!(
        "{}{}",
        String::from_utf8_lossy(&refused_loot.stdout),
        String::from_utf8_lossy(&refused_loot.stderr)
    );
    assert!(loot_refusal.contains("item_entry"), "{loot_refusal}");

    let accepted_loot = apply(&standalone, "loot", &pickpocket_loot(REAL_ITEM));
    assert!(accepted_loot.status.success());
    let loot = standalone.query_rows(&format!(
        "SELECT * FROM game_pickpocket_loot WHERE id = {PACKAGE_LOOT}"
    ));
    assert_eq!(loot[0]["item_entry"], REAL_ITEM.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn tuning_a_package_quest_no_enabled_package_inserts_is_refused() {
    let mut standalone = Standalone::start("package-delta-uninvented-quest");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

    let tuning = artifact(
        "example.tuner",
        &format!(
            r#"{{"table":"game_quest_template","key":{{"entry":{PACKAGE_QUEST}}},"operation":"update","fields":{{"reward_money":{{"type":"u32","value":100}}}}}}"#
        ),
    );
    let refused = apply(&standalone, "quests", &tuning);
    let text = refusal_text(&refused);
    assert!(
        !refused.status.success()
            && text.contains("`game_quest_template` row")
            && text.contains("an enabled Package tunes it, but no enabled Package inserts it"),
        "{text}"
    );
    assert!(standalone
        .query_rows(&format!(
            "SELECT entry FROM game_quest_template WHERE entry = {PACKAGE_QUEST}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn cast_claims_refuse_missing_cross_table_references() {
    let mut standalone = Standalone::start("package-delta-cast-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused_spell = apply(
        &standalone,
        "casts",
        &creature_spell_insert(IMPORTED_CREATURE, MISSING_SPELL),
    );
    assert!(!refused_spell.status.success());
    assert!(
        refusal_text(&refused_spell).contains("spell_id"),
        "{}",
        refusal_text(&refused_spell)
    );

    let refused_creature = apply(
        &standalone,
        "casts",
        &creature_spell_insert(MISSING_CREATURE, REAL_SPELL),
    );
    assert!(!refused_creature.status.success());
    assert!(
        refusal_text(&refused_creature).contains("creature_entry"),
        "{}",
        refusal_text(&refused_creature)
    );

    let accepted = apply(
        &standalone,
        "casts",
        &creature_spell_insert(IMPORTED_CREATURE, REAL_SPELL),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let spell = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_spell WHERE id = {PACKAGE_CREATURE_SPELL}"
    ));
    assert_eq!(spell[0]["spell_id"], REAL_SPELL.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn creature_spell_updates_cannot_hide_their_fixture_creature_target() {
    let mut standalone = Standalone::start("cast-fixture-target");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    assert_eq!(
        standalone
            .query_rows(&format!(
                "SELECT * FROM game_creature_template WHERE entry = {IMPORTED_CREATURE}"
            ))
            .len(),
        1,
        "the imported target must exist"
    );
    standalone.insert_row(
        "game_creature_spell",
        &[
            ("id", &FIXTURE_TARGETED_CREATURE_SPELL),
            ("creature_entry", &TEST_WOLF_ENTRY),
            ("spell_id", &REAL_SPELL),
            ("priority", &10),
        ],
    );

    let partial = apply(
        &standalone,
        "casts",
        &creature_spell_update(
            FIXTURE_TARGETED_CREATURE_SPELL,
            r#"{"priority":{"type":"u8","value":20}}"#,
        ),
    );
    let retargeted = apply(
        &standalone,
        "casts",
        &creature_spell_update(
            FIXTURE_TARGETED_CREATURE_SPELL,
            &format!(r#"{{"creature_entry":{{"type":"u32","value":{IMPORTED_CREATURE}}}}}"#),
        ),
    );

    assert!(
        !partial.status.success() && !retargeted.status.success(),
        "partial update success: {}; retarget-away success: {}",
        partial.status.success(),
        retargeted.status.success()
    );
    for refusal in [&partial, &retargeted] {
        let text = refusal_text(refusal);
        assert!(text.contains("fixture-reserved"), "{text}");
        assert!(text.contains(&TEST_WOLF_ENTRY.to_string()), "{text}");
    }

    let spell = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_spell WHERE id = {FIXTURE_TARGETED_CREATURE_SPELL}"
    ));
    assert_eq!(spell[0]["creature_entry"], TEST_WOLF_ENTRY.to_string());
    assert_eq!(spell[0]["priority"], "10");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_profession_offerings_marker_spell_is_exempt_but_a_class_offerings_is_not() {
    let mut standalone = Standalone::start("package-delta-trainer-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let accepted_class = apply(
        &standalone,
        "trainers",
        &trainer_spell_insert(REAL_SPELL, 0),
    );
    assert!(
        accepted_class.status.success(),
        "{}",
        refusal_text(&accepted_class)
    );

    let refused_class = apply(
        &standalone,
        "trainers",
        &trainer_spell_insert(MISSING_SPELL, 0),
    );
    assert!(!refused_class.status.success());
    assert!(
        refusal_text(&refused_class).contains("spell_id"),
        "{}",
        refusal_text(&refused_class)
    );

    const COOKING: u32 = 185;
    const MARKER_SPELL_ID: u32 = LEARN_COOKING_SPELL_ID;
    let accepted_profession = apply(
        &standalone,
        "trainers",
        &trainer_spell_insert(MARKER_SPELL_ID, COOKING),
    );
    assert!(
        accepted_profession.status.success(),
        "{}",
        refusal_text(&accepted_profession)
    );
    let trainer = standalone.query_rows(&format!(
        "SELECT * FROM game_trainer_spell WHERE id = {PACKAGE_TRAINER_SPELL}"
    ));
    assert_eq!(trainer[0]["learn_skill_line"], COOKING.to_string());
    assert_eq!(trainer[0]["spell_id"], MARKER_SPELL_ID.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn gossip_claims_refuse_missing_cross_table_references() {
    let mut standalone = Standalone::start("package-delta-gossip-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused_slot = apply(
        &standalone,
        "gossip",
        &npc_text_slot_insert(MISSING_NPC_TEXT),
    );
    assert!(!refused_slot.status.success());
    assert!(
        refusal_text(&refused_slot).contains("text_id"),
        "{}",
        refusal_text(&refused_slot)
    );

    let refused_option = apply(
        &standalone,
        "gossip",
        &gossip_option_insert(MISSING_CREATURE),
    );
    assert!(!refused_option.status.success());
    assert!(
        refusal_text(&refused_option).contains("entry"),
        "{}",
        refusal_text(&refused_option)
    );

    let plan = format!(
        "{}\n{}\n{}",
        npc_text_insert(),
        npc_text_slot_insert(PACKAGE_NPC_TEXT as u32),
        gossip_option_insert(TEST_WOLF_ENTRY)
    );
    let accepted = apply(&standalone, "gossip", &plan);
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let slot = standalone.query_rows(&format!(
        "SELECT * FROM game_npc_text_slot WHERE id = {PACKAGE_NPC_TEXT_SLOT}"
    ));
    assert_eq!(slot[0]["text_id"], PACKAGE_NPC_TEXT.to_string());
    let option = standalone.query_rows(&format!(
        "SELECT * FROM game_gossip_option WHERE row_id = {PACKAGE_GOSSIP_OPTION}"
    ));
    assert_eq!(option[0]["entry"], TEST_WOLF_ENTRY.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn globals_claims_refuse_missing_references_and_every_insert_on_a_fixed_key_table() {
    let mut standalone = Standalone::start("package-delta-globals-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused_zone = apply(
        &standalone,
        "globals",
        &graveyard_zone_insert(MISSING_GRAVEYARD),
    );
    assert!(!refused_zone.status.success());
    assert!(
        refusal_text(&refused_zone).contains("safe_loc_id"),
        "{}",
        refusal_text(&refused_zone)
    );

    let refused_grant = apply(
        &standalone,
        "globals",
        &createinfo_spell_insert(MISSING_SPELL),
    );
    assert!(!refused_grant.status.success());
    assert!(
        refusal_text(&refused_grant).contains("spell_id"),
        "{}",
        refusal_text(&refused_grant)
    );

    let refused_curve = apply(&standalone, "globals", &class_level_stats_insert());
    assert!(!refused_curve.status.success());
    assert!(
        refusal_text(&refused_curve).contains("game_class_level_stats"),
        "{}",
        refusal_text(&refused_curve)
    );

    let plan = format!(
        "{}\n{}",
        graveyard_zone_insert(REAL_GRAVEYARD),
        createinfo_spell_insert(REAL_SPELL)
    );
    let accepted = apply(&standalone, "globals", &plan);
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let zone = standalone.query_rows(&format!(
        "SELECT * FROM game_graveyard_zone WHERE row_id = {PACKAGE_GRAVEYARD_ZONE}"
    ));
    assert_eq!(zone[0]["safe_loc_id"], REAL_GRAVEYARD.to_string());
    let grant = standalone.query_rows(&format!(
        "SELECT * FROM game_createinfo_spell WHERE id = {PACKAGE_CREATEINFO_SPELL}"
    ));
    assert_eq!(grant[0]["spell_id"], REAL_SPELL.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn spell_metadata_claims_refuse_a_missing_spell_in_the_key_and_in_a_column() {
    let mut standalone = Standalone::start("package-delta-spellmeta-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused_chain = apply(&standalone, "spellmeta", &spell_chain_insert(PACKAGE_SPELL));
    assert!(!refused_chain.status.success());
    assert!(
        refusal_text(&refused_chain).contains("spell_id"),
        "{}",
        refusal_text(&refused_chain)
    );

    let refused_learn = apply(&standalone, "spellmeta", &spell_learn_insert(MISSING_SPELL));
    assert!(!refused_learn.status.success());
    assert!(
        refusal_text(&refused_learn).contains("learn_spell"),
        "{}",
        refusal_text(&refused_learn)
    );

    let accepted = apply(&standalone, "spellmeta", &spell_learn_insert(REAL_SPELL));
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let learn = standalone.query_rows(&format!(
        "SELECT * FROM game_spell_learn WHERE id = {PACKAGE_SPELL_LEARN}"
    ));
    assert_eq!(learn[0]["learn_spell"], REAL_SPELL.to_string());
}

const REAL_MAP: u32 = 0;
const PACKAGE_CREATURE_SPAWN: u32 = 15_000_001;
const PACKAGE_CREATURE_TEMPLATE: u32 = 15_000_002;
const PACKAGE_GAMEOBJECT_TEMPLATE: u32 = 16_000_001;
const PACKAGE_GAMEOBJECT_SPAWN: u32 = 16_000_002;
const MISSING_GAMEOBJECT: u32 = 4_000_000;
const SEEDED_GAMEOBJECT: u32 = BATTERED_CHEST;

fn creature_spawn_insert(map_id: u32, entry: u32, spawn_id: u32) -> String {
    artifact(
        "example.creatures.place",
        &format!(
            r#"{{"table":"game_creature_spawn","key":{{"map_id":{map_id},"entry":{entry},"spawn_id":{spawn_id}}},"operation":"insert","fields":{{"x":{{"type":"f32","value":-8949.95}},"y":{{"type":"f32","value":-132.493}},"z":{{"type":"f32","value":83.5312}},"orientation":{{"type":"f32","value":0.0}},"movement_type":{{"type":"u8","value":0}},"respawn_secs":{{"type":"u32","value":300}}}}}}"#
        ),
    )
}

fn creature_template_and_spawn() -> String {
    let template = artifact(
        "example.creatures.invent",
        &format!(
            r#"{{"table":"game_creature_template","key":{{"entry":{PACKAGE_CREATURE_TEMPLATE}}},"operation":"insert","fields":{{"name":{{"type":"string","value":"Kindled Sentinel"}},"subname":{{"type":"string","value":"Forge Guard"}},"display_id":{{"type":"u32","value":1420}},"level":{{"type":"u32","value":12}},"health":{{"type":"u32","value":300}},"faction_template":{{"type":"u32","value":14}},"npc_flags":{{"type":"u32","value":0}},"unit_flags":{{"type":"u32","value":0}},"creature_type":{{"type":"u8","value":7}},"creature_family":{{"type":"u8","value":0}},"type_flags":{{"type":"u32","value":0}},"rank":{{"type":"u8","value":0}},"scale":{{"type":"f32","value":1.0}},"base_attack_time_ms":{{"type":"u32","value":2000}},"money_min":{{"type":"u32","value":10}},"money_max":{{"type":"u32","value":40}},"max_level":{{"type":"u32","value":13}},"max_level_health":{{"type":"u32","value":340}},"aggro_range":{{"type":"u32","value":0}},"damage_min":{{"type":"u32","value":6}},"damage_max":{{"type":"u32","value":9}},"armor":{{"type":"u32","value":120}},"pickpocket_loot_id":{{"type":"u32","value":0}},"skin_loot_id":{{"type":"u32","value":0}},"trainer_type":{{"type":"u8","value":0}},"trainer_class":{{"type":"u8","value":0}}}}}}"#
        ),
    );
    format!(
        "{template}\n{}",
        creature_spawn_insert(REAL_MAP, PACKAGE_CREATURE_TEMPLATE, PACKAGE_CREATURE_SPAWN)
    )
}

fn gameobject_spawn_insert(map_id: u32, spawn_id: u32, template_entry: u32) -> String {
    artifact(
        "example.gameobjects.place",
        &format!(
            r#"{{"table":"game_gameobject","key":{{"map_id":{map_id},"spawn_id":{spawn_id}}},"operation":"insert","fields":{{"template_entry":{{"type":"u32","value":{template_entry}}},"x":{{"type":"f32","value":-8949.95}},"y":{{"type":"f32","value":-132.493}},"z":{{"type":"f32","value":83.5312}},"orientation":{{"type":"f32","value":0.0}},"state":{{"type":"u8","value":0}},"rotation_0":{{"type":"f32","value":0.0}},"rotation_1":{{"type":"f32","value":0.0}},"rotation_2":{{"type":"f32","value":0.0}},"rotation_3":{{"type":"f32","value":0.0}}}}}}"#
        ),
    )
}

fn gameobject_template_insert() -> String {
    artifact(
        "example.gameobjects.invent",
        &format!(
            r#"{{"table":"game_gameobject_template","key":{{"entry":{PACKAGE_GAMEOBJECT_TEMPLATE}}},"operation":"insert","fields":{{"type_id":{{"type":"u8","value":3}},"display_id":{{"type":"u32","value":259}},"name":{{"type":"string","value":"Kindled Cache"}},"data0":{{"type":"u32","value":{REAL_ITEM}}},"data1":{{"type":"u32","value":0}},"gather_skill_line":{{"type":"u32","value":0}},"respawn_secs":{{"type":"u32","value":180}},"gather_gray":{{"type":"u32","value":0}},"lock_id":{{"type":"u32","value":0}},"size":{{"type":"f32","value":1.0}}}}}}"#
        ),
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn creature_spawn_claims_refuse_a_missing_template_and_one_row_claimed_on_two_maps() {
    let mut standalone = Standalone::start("package-delta-creature-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused = apply(
        &standalone,
        "creatures",
        &creature_spawn_insert(REAL_MAP, MISSING_CREATURE, PACKAGE_CREATURE_SPAWN),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("missing entry"),
        "{}",
        refusal_text(&refused)
    );

    let two_maps = format!(
        "{}\n{}",
        creature_spawn_insert(0, TEST_WOLF_ENTRY, PACKAGE_CREATURE_SPAWN),
        creature_spawn_insert(1, TEST_WOLF_ENTRY, PACKAGE_CREATURE_SPAWN)
            .replace("example.creatures.place", "example.creatures.elsewhere"),
    );
    let refused_twice = apply(&standalone, "creatures", &two_maps);
    assert!(!refused_twice.status.success());
    assert!(
        refusal_text(&refused_twice).contains("one durable creature"),
        "{}",
        refusal_text(&refused_twice)
    );

    let accepted = apply(
        &standalone,
        "creatures",
        &creature_spawn_insert(REAL_MAP, TEST_WOLF_ENTRY, PACKAGE_CREATURE_SPAWN),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let guid = guid::creature(TEST_WOLF_ENTRY, PACKAGE_CREATURE_SPAWN);
    let spawn = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_spawn WHERE guid = {guid}"
    ));
    assert_eq!(spawn[0]["entry"], TEST_WOLF_ENTRY.to_string());
    assert_eq!(spawn[0]["map_id"], REAL_MAP.to_string());

    let cleared = apply(&standalone, "creatures", "");
    assert!(cleared.status.success(), "{}", refusal_text(&cleared));
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_creature_spawn WHERE guid = {guid}"
        ))
        .is_empty());

    let invented = apply(&standalone, "creatures", &creature_template_and_spawn());
    assert!(invented.status.success(), "{}", refusal_text(&invented));
    let template = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_template WHERE entry = {PACKAGE_CREATURE_TEMPLATE}"
    ));
    assert_eq!(template[0]["name"], "Kindled Sentinel");
    let invented_guid = guid::creature(PACKAGE_CREATURE_TEMPLATE, PACKAGE_CREATURE_SPAWN);
    assert!(!standalone
        .query_rows(&format!(
            "SELECT * FROM game_creature_spawn WHERE guid = {invented_guid}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn gameobject_claims_refuse_a_missing_template_and_place_an_invented_one() {
    let mut standalone = Standalone::start("package-delta-gameobject-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    let refused = apply(
        &standalone,
        "gameobjects",
        &gameobject_spawn_insert(REAL_MAP, PACKAGE_GAMEOBJECT_SPAWN, MISSING_GAMEOBJECT),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("missing template entry"),
        "{}",
        refusal_text(&refused)
    );

    let accepted = apply(
        &standalone,
        "gameobjects",
        &gameobject_spawn_insert(REAL_MAP, PACKAGE_GAMEOBJECT_SPAWN, SEEDED_GAMEOBJECT),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let guid = guid::compose(guid::GAMEOBJECT, u64::from(PACKAGE_GAMEOBJECT_SPAWN));
    let spawn = standalone.query_rows(&format!(
        "SELECT * FROM game_gameobject WHERE guid = {guid}"
    ));
    assert_eq!(spawn[0]["template_entry"], SEEDED_GAMEOBJECT.to_string());
    assert_ne!(
        spawn[0]["cell"], "0",
        "the AOI cell is derived from the claimed position, not left at cell (0, 0)"
    );

    let plan = format!(
        "{}\n{}",
        gameobject_template_insert(),
        gameobject_spawn_insert(
            REAL_MAP,
            PACKAGE_GAMEOBJECT_SPAWN,
            PACKAGE_GAMEOBJECT_TEMPLATE
        )
    );
    let invented = apply(&standalone, "gameobjects", &plan);
    assert!(invented.status.success(), "{}", refusal_text(&invented));
    let template = standalone.query_rows(&format!(
        "SELECT * FROM game_gameobject_template WHERE entry = {PACKAGE_GAMEOBJECT_TEMPLATE}"
    ));
    assert_eq!(template[0]["name"], "Kindled Cache");

    let cleared = apply(&standalone, "gameobjects", "");
    assert!(cleared.status.success(), "{}", refusal_text(&cleared));
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_gameobject_template WHERE entry = {PACKAGE_GAMEOBJECT_TEMPLATE}"
        ))
        .is_empty());
}

const KELRIS: u32 = 4_832;
const KELRIS_TEXT: u32 = 900;
const ORDINARY_TEXT: u32 = 901;
const PACKAGE_QUEST_EVENT_REQUIREMENT: u64 = 17_000_001;
const MISSING_QUEST: u32 = 4_000_000;

fn seed_broadcast_texts(standalone: &Standalone) {
    for (id, line) in [
        (KELRIS_TEXT, "Ah, sweet innocence."),
        (ORDINARY_TEXT, "Halt."),
    ] {
        standalone.insert_row(
            "game_creature_ai_broadcast_text",
            &[
                ("id", &id),
                ("male_text", &line),
                ("female_text", &line),
                ("chat_type", &1),
            ],
        );
    }
}

fn kelris_definition() -> String {
    let subject = format!("entry:{KELRIS}");
    let rules = format!(
        "10,aggro,100,4294967295,once,all,ordinary,any-posture,\
         speak:yell:self:{KELRIS_TEXT}+notify-encounter:blackfathom-deeps-kelris:begin"
    );
    lyracore_shared::eventai::encode_definition(&subject, &rules)
}

fn quest_event_requirement_insert(quest_entry: u32) -> String {
    artifact(
        "example.quest.event",
        &format!(
            r#"{{"table":"game_quest_event_requirement","key":{{"id":{PACKAGE_QUEST_EVENT_REQUIREMENT}}},"operation":"insert","fields":{{"quest_entry":{{"type":"u32","value":{quest_entry}}}}}}}"#
        ),
    )
}

fn broadcast_text_update(id: u32) -> String {
    artifact(
        "example.voice",
        &format!(
            r#"{{"table":"game_creature_ai_broadcast_text","key":{{"id":{id}}},"operation":"update","fields":{{"male_text":{{"type":"string","value":"You will burn."}}}}}}"#
        ),
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn creature_ai_claims_are_checked_against_quests_and_encounter_ownership() {
    let mut standalone = Standalone::start("package-delta-creature-ai-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    seed_broadcast_texts(&standalone);

    let refused = apply(
        &standalone,
        "creature-ai",
        &quest_event_requirement_insert(MISSING_QUEST),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("quest_entry"),
        "{}",
        refusal_text(&refused)
    );

    let accepted = apply(
        &standalone,
        "creature-ai",
        &quest_event_requirement_insert(SCENARIO_QUEST),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let requirement = standalone.query_rows(&format!(
        "SELECT * FROM game_quest_event_requirement WHERE id = {PACKAGE_QUEST_EVENT_REQUIREMENT}"
    ));
    assert_eq!(requirement[0]["quest_entry"], SCENARIO_QUEST.to_string());

    standalone.assert_call(
        "import_creature_ai_definitions",
        &[&arg(&kelris_definition())],
    );

    let refused = apply(
        &standalone,
        "creature-ai",
        &broadcast_text_update(KELRIS_TEXT),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("BlackfathomDeepsKelris"),
        "{}",
        refusal_text(&refused)
    );

    let accepted = apply(
        &standalone,
        "creature-ai",
        &broadcast_text_update(ORDINARY_TEXT),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let text = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_ai_broadcast_text WHERE id = {ORDINARY_TEXT}"
    ));
    assert_eq!(text[0]["male_text"], "You will burn.");

    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_quest_event_requirement WHERE id = {PACKAGE_QUEST_EVENT_REQUIREMENT}"
        ))
        .is_empty());
}

const PACKAGE_FORM: u32 = 18_000_001;
const PACKAGE_SHIFT: u32 = 6_000_001;
const BATTLE_STANCE: u32 = 2457;
const BATTLE_STANCE_PASSIVE: u32 = 21156;

fn shift_insert(form_id: u32) -> String {
    format!(
        r#"{{"table":"game_spell","key":{{"spell_id":{PACKAGE_SHIFT}}},"operation":"insert","fields":{{"name":{{"type":"string","value":"Kindled Aspect"}},"power_type":{{"type":"u8","value":1}},"cost":{{"type":"u32","value":0}},"cast_time_ms":{{"type":"u32","value":0}},"gcd_ms":{{"type":"u32","value":0}},"cooldown_ms":{{"type":"u32","value":0}},"range_yd":{{"type":"u32","value":0}},"duration_ms":{{"type":"u32","value":4294967295}},"school_mask":{{"type":"u8","value":1}},"dispel_type":{{"type":"u8","value":0}},"mechanic":{{"type":"u8","value":0}},"max_stacks":{{"type":"u8","value":0}},"aura_interrupt":{{"type":"u16","value":0}},"attributes":{{"type":"u32","value":0}},"spell_level":{{"type":"u8","value":1}},"max_level":{{"type":"u8","value":0}},"is_negative":{{"type":"bool","value":false}},"cast_flags":{{"type":"u32","value":0}},"stances":{{"type":"u8","value":0}},"family_name":{{"type":"u8","value":0}},"family_flags":{{"type":"u64","value":"0"}},"proc_flags":{{"type":"u32","value":0}},"proc_chance":{{"type":"u8","value":0}},"proc_charges":{{"type":"u8","value":0}}}}}},{{"table":"game_spell_effect","key":{{"spell_id":{PACKAGE_SHIFT},"effect_index":0}},"operation":"insert","fields":{{"kind":{{"type":"u8","value":{A_SHAPESHIFT}}},"base_points":{{"type":"i32","value":0}},"die_sides":{{"type":"i32","value":0}},"per_level":{{"type":"f32","value":0.0}},"period_ms":{{"type":"u32","value":0}},"target":{{"type":"u8","value":0}},"radius_yd":{{"type":"f32","value":0.0}},"chain_targets":{{"type":"u8","value":0}},"trigger_spell":{{"type":"u32","value":0}},"effect_mechanic":{{"type":"u8","value":0}},"p0":{{"type":"i32","value":{form_id}}},"p0_kind":{{"type":"u8","value":{P_FORM}}},"p1":{{"type":"i32","value":0}},"script_id":{{"type":"u32","value":0}},"enters_combat":{{"type":"bool","value":false}}}}}}"#
    )
}

fn form_insert(spell_id: u32, power_on_entry: u8) -> String {
    format!(
        r#"{{"table":"game_form","key":{{"form_id":{PACKAGE_FORM}}},"operation":"insert","fields":{{"name":{{"type":"string","value":"Kindled Aspect"}},"spell_id":{{"type":"u32","value":{spell_id}}},"acts_as_shapeshifted":{{"type":"bool","value":true}},"power_type":{{"type":"u8","value":1}},"power_on_entry":{{"type":"u8","value":{power_on_entry}}},"default_for_class":{{"type":"u8","value":0}},"client_form":{{"type":"u8","value":0}}}}}}"#
    )
}

/// A Package spell like the shapeshift spell, under another id, optionally passive and without its
/// shapeshift effect.
fn other_spell_insert(spell_id: u32, attributes: u32, shapeshifts: bool) -> String {
    let mut claims = shift_insert(PACKAGE_FORM);
    if !shapeshifts {
        claims.truncate(claims.find(r#",{"table":"game_spell_effect""#).unwrap());
    }
    claims
        .replace(&PACKAGE_SHIFT.to_string(), &spell_id.to_string())
        .replace(
            r#""attributes":{"type":"u32","value":0}"#,
            &format!(r#""attributes":{{"type":"u32","value":{attributes}}}"#),
        )
}

fn form_spell_insert(spell_id: u32) -> String {
    format!(
        r#"{{"table":"game_form_spell","key":{{"form_id":{PACKAGE_FORM},"spell_id":{spell_id}}},"operation":"insert","fields":{{}}}}"#
    )
}

fn battle_stance_effect_update(fields: &str) -> String {
    format!(
        r#"{{"table":"game_spell_effect","key":{{"spell_id":{BATTLE_STANCE},"effect_index":0}},"operation":"update","fields":{fields}}}"#
    )
}

fn spell_form_update(spell_id: u32, form_id: u32) -> String {
    format!(
        r#"{{"table":"game_spell_form","key":{{"spell_id":{spell_id},"form_id":{form_id}}},"operation":"update","fields":{{"excluded":{{"type":"bool","value":true}}}}}}"#
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn form_claims_refuse_rows_the_form_tables_cannot_honour() {
    let mut standalone = Standalone::start("package-delta-form-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_seed_warrior_forms", &[]);
    let with_form = |rest: &str| {
        [
            shift_insert(PACKAGE_FORM),
            form_insert(PACKAGE_SHIFT, 0),
            rest.to_owned(),
        ]
        .join(",")
    };
    let with_form_field = |field: &str| {
        [
            shift_insert(PACKAGE_FORM),
            form_insert(PACKAGE_SHIFT, 0).replace(
                r#""client_form":{"type":"u8","value":0}"#,
                &format!(r#""client_form":{{"type":"u8","value":0}},{field}"#),
            ),
        ]
        .join(",")
    };

    for (claims, named) in [
        (form_spell_insert(BATTLE_STANCE_PASSIVE), "missing Form"),
        (form_insert(BATTLE_STANCE, 0), "does not enter the Form"),
        (form_insert(MISSING_SPELL, 0), "does not enter the Form"),
        (
            with_form(&form_spell_insert(MISSING_SPELL)),
            "missing spell",
        ),
        (
            with_form(&form_spell_insert(BATTLE_STANCE)),
            "not a passive",
        ),
        (
            with_form(&format!(
                "{},{}",
                other_spell_insert(PACKAGE_SHIFT + 1, 0, false),
                form_spell_insert(PACKAGE_SHIFT + 1)
            )),
            "not a passive",
        ),
        (
            with_form(&format!(
                "{},{}",
                other_spell_insert(PACKAGE_SHIFT + 2, 0x40, true),
                form_spell_insert(PACKAGE_SHIFT + 2)
            )),
            "which shapeshifts",
        ),
        (
            [shift_insert(PACKAGE_FORM), form_insert(PACKAGE_SHIFT, 9)].join(","),
            "unknown power rule",
        ),
        (
            [
                shift_insert(PACKAGE_FORM),
                form_insert(PACKAGE_SHIFT, 0).replace(
                    r#""power_type":{"type":"u8","value":1}"#,
                    r#""power_type":{"type":"u8","value":5}"#,
                ),
            ]
            .join(","),
            "unknown power type",
        ),
        (
            with_form_field(r#""natural_attack_ms":{"type":"u32","value":400}"#),
            "under the 500 ms floor",
        ),
        (
            with_form_field(&format!(
                r#""entry_chance_spell":{{"type":"u32","value":{MISSING_SPELL}}}"#
            )),
            "names entry chance spell 4000000, which does not exist",
        ),
        (
            entry_chance_insert(PACKAGE_SHIFT + 3, 0),
            "without SPELL_ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED",
        ),
        (shift_insert(PACKAGE_FORM + 1), "no `game_form` row"),
        (
            spell_form_update(BATTLE_STANCE, 17),
            "outside the Package Form range",
        ),
        (
            battle_stance_effect_update(r#"{"p0":{"type":"i32","value":18}}"#),
            "no longer entering Form 17",
        ),
        (
            format!(
                r#"{{"table":"game_spell_effect","key":{{"spell_id":{BATTLE_STANCE_PASSIVE},"effect_index":0}},"operation":"update","fields":{{"kind":{{"type":"u8","value":{A_SHAPESHIFT}}},"p0":{{"type":"i32","value":99}}}}}}"#
            ),
            "Form 99, which has no `game_form` row",
        ),
    ] {
        let refused = apply(&standalone, "spell", &artifact("example.aspect", &claims));
        assert!(!refused.status.success(), "{claims}");
        assert!(
            refusal_text(&refused).contains(named),
            "{named}: {}",
            refusal_text(&refused)
        );
    }
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_form WHERE form_id = {PACKAGE_FORM}"
        ))
        .is_empty());

    let unbound = entry_chance_insert(PACKAGE_SHIFT + 3, ALLOW_WHILE_NOT_SHAPESHIFTED);
    let accepted = apply(&standalone, "spell", &artifact("example.aspect", &unbound));
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));

    let tuned = battle_stance_effect_update(r#"{"base_points":{"type":"i32","value":5}}"#);
    let accepted = apply(&standalone, "spell", &artifact("example.aspect", &tuned));
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));

    let claims = with_form(&form_spell_insert(BATTLE_STANCE_PASSIVE));
    let accepted = apply(&standalone, "spell", &artifact("example.aspect", &claims));
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let passives = standalone.query_rows(&format!(
        "SELECT * FROM game_form_spell WHERE form_id = {PACKAGE_FORM}"
    ));
    assert_eq!(passives[0]["spell_id"], BATTLE_STANCE_PASSIVE.to_string());
}

const ALLOW_WHILE_NOT_SHAPESHIFTED: u32 = 0x80000;

/// A passive spell whose one effect is an `A_FORM_ENTRY_CHANCE`.
fn entry_chance_insert(spell_id: u32, attributes_ex_2: u32) -> String {
    other_spell_insert(spell_id, 0x40, true)
        .replace(
            &format!(r#""kind":{{"type":"u8","value":{A_SHAPESHIFT}}}"#),
            &format!(r#""kind":{{"type":"u8","value":{A_FORM_ENTRY_CHANCE}}}"#),
        )
        .replace(
            r#""attributes":{"type":"u32","value":64}"#,
            &format!(
                r#""attributes":{{"type":"u32","value":64}},"attributes_ex_2":{{"type":"u32","value":{attributes_ex_2}}}"#
            ),
        )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_plan_that_drops_a_forms_entry_chance_spell_is_refused() {
    const ENTRY_SPELL: u32 = PACKAGE_SHIFT + 4;
    let mut standalone = Standalone::start("package-delta-form-dropped-entry-spell");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_seed_warrior_forms", &[]);

    let pointed = [
        other_spell_insert(ENTRY_SPELL, 0, false),
        format!(
            r#"{{"table":"game_form","key":{{"form_id":17}},"operation":"update","fields":{{"entry_chance_spell":{{"type":"u32","value":{ENTRY_SPELL}}}}}}}"#
        ),
    ]
    .join(",");
    let accepted = apply(&standalone, "spell", &artifact("example.aspect", &pointed));
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));

    let refused = apply(&standalone, "spell", "");
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains(&format!(
            "`game_form` row {{form_id=17}} names entry chance spell {ENTRY_SPELL}"
        )),
        "{}",
        refusal_text(&refused)
    );
}

fn imported_form_spell_update(form_id: u32, spell_id: u32) -> String {
    format!(
        r#"{{"table":"game_form","key":{{"form_id":{form_id}}},"operation":"update","fields":{{"spell_id":{{"type":"u32","value":{spell_id}}}}}}}"#
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_plan_that_drops_the_spell_an_imported_form_was_pointed_at_is_refused() {
    let mut standalone = Standalone::start("package-delta-form-dropped-spell");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_seed_warrior_forms", &[]);

    let repoint = [
        shift_insert(17),
        imported_form_spell_update(17, PACKAGE_SHIFT),
    ]
    .join(",");
    let accepted = apply(&standalone, "spell", &artifact("example.aspect", &repoint));
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));

    let refused = apply(&standalone, "spell", "");
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("`game_form` row {form_id=17} names spell 6000001"),
        "{}",
        refusal_text(&refused)
    );
    let forms = standalone.query_rows("SELECT * FROM game_form WHERE form_id = 17");
    assert_eq!(forms[0]["spell_id"], PACKAGE_SHIFT.to_string());
    assert_eq!(
        standalone
            .query_rows(&format!(
                "SELECT * FROM game_spell WHERE spell_id = {PACKAGE_SHIFT}"
            ))
            .len(),
        1
    );

    // `lyracore packages apply` replays the base import, which writes the imported Forms afresh.
    standalone.assert_call("debug_seed_warrior_forms", &[]);
    let accepted = apply(&standalone, "spell", "");
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let forms = standalone.query_rows("SELECT * FROM game_form WHERE form_id = 17");
    assert_eq!(forms[0]["spell_id"], BATTLE_STANCE.to_string());
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_spell WHERE spell_id = {PACKAGE_SHIFT}"
        ))
        .is_empty());
}

const SEEDED_RENEW: u32 = 139;
const PACKAGE_FOLLOW_UP: u32 = 6_000_101;

/// A Package aura spell that raises its target's armour, built on the shapeshift spell's claims.
fn follow_up_spell_insert(spell_id: u32) -> String {
    other_spell_insert(spell_id, 0, true)
        .replace(
            &format!(r#""kind":{{"type":"u8","value":{A_SHAPESHIFT}}}"#),
            &format!(r#""kind":{{"type":"u8","value":{A_MOD_RESISTANCE}}}"#),
        )
        .replace(
            &format!(r#""p0":{{"type":"i32","value":{PACKAGE_FORM}}}"#),
            &format!(r#""p0":{{"type":"i32","value":{RESIST_ARMOR}}}"#),
        )
        .replace(
            &format!(r#""p0_kind":{{"type":"u8","value":{P_FORM}}}"#),
            &format!(r#""p0_kind":{{"type":"u8","value":{P_SCHOOL_MASK}}}"#),
        )
}

fn follow_up_claim(spell_id: u32, follow_up_spell_id: u32) -> String {
    format!(
        r#"{{"table":"game_spell_follow_up","key":{{"spell_id":{spell_id}}},"operation":"insert","fields":{{"follow_up_spell_id":{{"type":"u32","value":{follow_up_spell_id}}}}}}}"#
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_follow_up_on_an_imported_spell_casts_its_package_spell() {
    let mut standalone = Standalone::start("package-delta-follow-up");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_set_level", &["1", "10"]);

    let missing = apply(
        &standalone,
        "spell",
        &artifact(
            "example.follow_up",
            &follow_up_claim(SEEDED_RENEW, PACKAGE_FOLLOW_UP),
        ),
    );
    assert!(!missing.status.success());
    assert!(
        refusal_text(&missing).contains(&PACKAGE_FOLLOW_UP.to_string()),
        "{}",
        refusal_text(&missing)
    );
    let imported = apply(
        &standalone,
        "spell",
        &artifact("example.follow_up", &follow_up_claim(SEEDED_RENEW, 133)),
    );
    assert!(
        !imported.status.success(),
        "the import owns a follow-up between imported spells"
    );

    let plan = artifact(
        "example.follow_up",
        &[
            follow_up_spell_insert(PACKAGE_FOLLOW_UP),
            follow_up_claim(SEEDED_RENEW, PACKAGE_FOLLOW_UP),
        ]
        .join(","),
    );
    standalone.insert_row(
        "game_spell_follow_up",
        &[("spell_id", &SEEDED_RENEW), ("follow_up_spell_id", &133u32)],
    );
    let collides = apply(&standalone, "spell", &plan);
    assert!(
        !collides.status.success() && refusal_text(&collides).contains("did not insert"),
        "an import follow-up on the spell refuses the Package one: {}",
        refusal_text(&collides)
    );
    assert!(
        standalone
            .query_rows(&format!(
                "SELECT spell_id FROM game_spell WHERE spell_id = {PACKAGE_FOLLOW_UP}"
            ))
            .is_empty(),
        "the refused plan writes nothing"
    );
    standalone.assert_sql(&format!(
        "DELETE FROM game_spell_follow_up WHERE spell_id = {SEEDED_RENEW}"
    ));

    let accepted = apply(&standalone, "spell", &plan);
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    standalone.assert_call("debug_cast_at", &["1", &SEEDED_RENEW.to_string(), "1"]);
    assert!(
        !standalone
            .query_rows(&format!(
                "SELECT id FROM game_aura WHERE spell_id = {PACKAGE_FOLLOW_UP} AND target_guid = 1"
            ))
            .is_empty(),
        "Renew follows up with the Package spell on the unit it hit"
    );
}
