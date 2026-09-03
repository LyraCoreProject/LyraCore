//! Package Delta reference preflight against a real Shard.

mod support;

use support::Standalone;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const FIXTURE_QUEST: u32 = 50_900;
const FIXTURE_CREATURE: u32 = 51_000; // Test Wolf.
const FIXTURE_TRAINER: u32 = 51_001; // Profession Trainer.
const REAL_ITEM: u32 = 25;
const MISSING_ITEM: u32 = 4_000_000;
const PACKAGE_LOOT: u64 = 9_000_001;
const REAL_SPELL: u32 = 50_310; // Test Riding Horse, seeded by the land-mount fixture.
const MISSING_SPELL: u32 = 4_000_000;
const MISSING_CREATURE: u32 = 4_000_000;
const PACKAGE_CREATURE_SPELL: u64 = 10_000_001;
const PACKAGE_TRAINER_SPELL: u64 = 11_000_001;
const PACKAGE_NPC_TEXT: u64 = 12_000_001;
const PACKAGE_NPC_TEXT_SLOT: u64 = 12_000_002;
const PACKAGE_GOSSIP_OPTION: u64 = 12_000_003;
const MISSING_NPC_TEXT: u32 = 4_000_000;
const PACKAGE_GRAVEYARD_ZONE: u64 = 13_000_001;
const PACKAGE_CREATEINFO_SPELL: u64 = 13_000_002;
const REAL_GRAVEYARD: u32 = 105; // Northshire Abbey, seeded by `init`.
const MISSING_GRAVEYARD: u32 = 4_000_000;
const PACKAGE_SPELL_LEARN: u64 = 14_000_001;
/// Inside the Package spell band, and no `game_spell` row: what a rank link keyed on a spell that
/// was never inserted looks like.
const PACKAGE_SPELL: u32 = 6_000_001;

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
            r#"{{"table":"game_quest_template","key":{{"entry":{FIXTURE_QUEST}}},"operation":"update","fields":{{"src_item":{{"type":"u32","value":{item_entry}}}}}}}"#
        ),
    )
}

fn pickpocket_loot(item_entry: u32) -> String {
    artifact(
        "example.loot",
        &format!(
            r#"{{"table":"game_pickpocket_loot","key":{{"id":{PACKAGE_LOOT}}},"operation":"insert","fields":{{"creature_entry":{{"type":"u32","value":{FIXTURE_CREATURE}}},"item_entry":{{"type":"u32","value":{item_entry}}},"chance_bp":{{"type":"u32","value":10000}},"count":{{"type":"u32","value":1}},"group_id":{{"type":"u32","value":0}},"quest_only":{{"type":"bool","value":false}}}}}}"#
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

fn trainer_spell_insert(spell_id: u32, learn_skill_line: u32) -> String {
    artifact(
        "example.trainer",
        &format!(
            r#"{{"table":"game_trainer_spell","key":{{"id":{PACKAGE_TRAINER_SPELL}}},"operation":"insert","fields":{{"trainer_entry":{{"type":"u32","value":{FIXTURE_TRAINER}}},"spell_id":{{"type":"u32","value":{spell_id}}},"cost":{{"type":"u32","value":500}},"required_level":{{"type":"u8","value":10}},"learn_skill_line":{{"type":"u32","value":{learn_skill_line}}},"learn_skill_cap":{{"type":"u32","value":75}}}}}}"#
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

/// An insert on an update-only table. The key names a real class and level, so only the operation
/// is wrong.
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn quest_and_loot_claims_refuse_missing_cross_table_references() {
    let standalone = Standalone::start("package-delta-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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
        "SELECT * FROM game_quest_template WHERE entry = {FIXTURE_QUEST}"
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
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn cast_claims_refuse_missing_cross_table_references() {
    let standalone = Standalone::start("package-delta-cast-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused_spell = apply(
        &standalone,
        "casts",
        &creature_spell_insert(FIXTURE_CREATURE, MISSING_SPELL),
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
        &creature_spell_insert(FIXTURE_CREATURE, REAL_SPELL),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let spell = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_spell WHERE id = {PACKAGE_CREATURE_SPELL}"
    ));
    assert_eq!(spell[0]["spell_id"], REAL_SPELL.to_string());
}

/// `game_trainer_spell.spell_id` is checked against `game_spell` only when the row's final
/// `learn_skill_line` is 0. A profession offering's `spell_id` is a synthetic marker
/// (`module/src/skill.rs`'s `LEARN_COOKING_SPELL_ID` and siblings) that never resolves to a
/// `game_spell` row, so an unconditional check would refuse a legitimate claim.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_profession_offerings_marker_spell_is_exempt_but_a_class_offerings_is_not() {
    let standalone = Standalone::start("package-delta-trainer-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    // A class offering (`learn_skill_line` 0) names a real spell.
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

    // A class offering naming no real spell is refused.
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

    // A profession offering (`learn_skill_line` 185, Cooking) carries a marker `spell_id` that is
    // never a `game_spell` row. The claim still succeeds because the gate exempts it.
    const COOKING: u32 = 185;
    const MARKER_SPELL_ID: u32 = 50_080;
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

/// A gossip slot points at a greeting body, and a gossip option points at a creature template the
/// creatures family owns. Both are checked against the Shard, and a Package-band body is satisfied
/// by an insert in the same plan.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn gossip_claims_refuse_missing_cross_table_references() {
    let standalone = Standalone::start("package-delta-gossip-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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
        gossip_option_insert(FIXTURE_CREATURE)
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
    assert_eq!(option[0]["entry"], FIXTURE_CREATURE.to_string());
}

/// A graveyard-zone link points at a `game_graveyard` row and a createinfo grant points at a
/// `game_spell` row. `game_class_level_stats` permits no insert at all, whatever the key.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn globals_claims_refuse_missing_references_and_every_insert_on_a_fixed_key_table() {
    let standalone = Standalone::start("package-delta-globals-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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

/// Spell metadata describes a spell, so both the columns and the KEY are checked against
/// `game_spell`: a rank link for a spell no Shard holds is a row nothing will ever read.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn spell_metadata_claims_refuse_a_missing_spell_in_the_key_and_in_a_column() {
    let standalone = Standalone::start("package-delta-spellmeta-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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

// ---- the two spatial families ----

/// Eastern Kingdoms. Every in-box World Import Scope but `instances` owns some of it, and the
/// Module never asks which — routing is the importer's, so a live apply takes the map as given.
const REAL_MAP: u32 = 0;
const PACKAGE_CREATURE_SPAWN: u32 = 15_000_001;
const PACKAGE_CREATURE_TEMPLATE: u32 = 15_000_002;
const PACKAGE_GAMEOBJECT_TEMPLATE: u32 = 16_000_001;
const PACKAGE_GAMEOBJECT_SPAWN: u32 = 16_000_002;
const MISSING_GAMEOBJECT: u32 = 4_000_000;
const SEEDED_GAMEOBJECT: u32 = 50_100; // Battered Chest, seeded by `init`.

/// The importer's `world_guid`, restated so the assertion derives the key the same way the artifact
/// does not: the test names the components, the Module packs them.
fn creature_guid(entry: u32, spawn_id: u32) -> u64 {
    (0xF130u64 << 48) | (u64::from(entry) << 24) | u64::from(spawn_id)
}

/// The importer's `go_guid`.
fn gameobject_guid(spawn_id: u32) -> u64 {
    (0xF110u64 << 48) | u64::from(spawn_id)
}

fn creature_spawn_insert(map_id: u32, entry: u32, spawn_id: u32) -> String {
    artifact(
        "example.creatures.place",
        &format!(
            r#"{{"table":"game_creature_spawn","key":{{"map_id":{map_id},"entry":{entry},"spawn_id":{spawn_id}}},"operation":"insert","fields":{{"x":{{"type":"f32","value":-8949.95}},"y":{{"type":"f32","value":-132.493}},"z":{{"type":"f32","value":83.5312}},"orientation":{{"type":"f32","value":0.0}},"movement_type":{{"type":"u8","value":0}},"respawn_secs":{{"type":"u32","value":300}}}}}}"#
        ),
    )
}

/// A whole Package creature, template and spawn together — the plan an author writes to add an NPC.
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

/// A creature spawn is spatial and its guid is derived, so the preflight has two jobs: the template
/// it places must exist after the plan lands, and no two claims may resolve to one durable
/// creature. The second is what an invalid map-ownership statement looks like from inside a
/// Shard, which never decides which maps it owns.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn creature_spawn_claims_refuse_a_missing_template_and_one_row_claimed_on_two_maps() {
    let standalone = Standalone::start("package-delta-creature-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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
        creature_spawn_insert(0, FIXTURE_CREATURE, PACKAGE_CREATURE_SPAWN),
        creature_spawn_insert(1, FIXTURE_CREATURE, PACKAGE_CREATURE_SPAWN)
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
        &creature_spawn_insert(REAL_MAP, FIXTURE_CREATURE, PACKAGE_CREATURE_SPAWN),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let guid = creature_guid(FIXTURE_CREATURE, PACKAGE_CREATURE_SPAWN);
    let spawn = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_spawn WHERE guid = {guid}"
    ));
    assert_eq!(spawn[0]["entry"], FIXTURE_CREATURE.to_string());
    assert_eq!(spawn[0]["map_id"], REAL_MAP.to_string());

    // A Package that leaves the enabled set takes its invented rows with it: the same family
    // applied with an empty plan clears the whole band.
    let cleared = apply(&standalone, "creatures", "");
    assert!(cleared.status.success(), "{}", refusal_text(&cleared));
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_creature_spawn WHERE guid = {guid}"
        ))
        .is_empty());

    // A Package may also invent the creature it places, in one plan.
    let invented = apply(&standalone, "creatures", &creature_template_and_spawn());
    assert!(invented.status.success(), "{}", refusal_text(&invented));
    let template = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_template WHERE entry = {PACKAGE_CREATURE_TEMPLATE}"
    ));
    // `spacetime sql` quotes a string column, so the expected value carries the quotes.
    assert_eq!(template[0]["name"], "\"Kindled Sentinel\"");
    let invented_guid = creature_guid(PACKAGE_CREATURE_TEMPLATE, PACKAGE_CREATURE_SPAWN);
    assert!(!standalone
        .query_rows(&format!(
            "SELECT * FROM game_creature_spawn WHERE guid = {invented_guid}"
        ))
        .is_empty());
}

/// A gameobject spawn names its template in a COLUMN rather than in the key, so the preflight has
/// to judge the row it will hold after the plan lands, not the column alone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn gameobject_claims_refuse_a_missing_template_and_place_an_invented_one() {
    let standalone = Standalone::start("package-delta-gameobject-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

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
    let guid = gameobject_guid(PACKAGE_GAMEOBJECT_SPAWN);
    let spawn = standalone.query_rows(&format!(
        "SELECT * FROM game_gameobject WHERE guid = {guid}"
    ));
    assert_eq!(spawn[0]["template_entry"], SEEDED_GAMEOBJECT.to_string());
    assert_ne!(
        spawn[0]["cell"], "0",
        "the AOI cell is derived from the claimed position, not left at cell (0, 0)"
    );

    // The template and its spawn in one plan: the Package-band template is satisfied by the insert
    // beside it, not by anything already on the Shard.
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
    assert_eq!(template[0]["name"], "\"Kindled Cache\"");

    let cleared = apply(&standalone, "gameobjects", "");
    assert!(cleared.status.success(), "{}", refusal_text(&cleared));
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_gameobject_template WHERE entry = {PACKAGE_GAMEOBJECT_TEMPLATE}"
        ))
        .is_empty());
}

// ---- the creature-ai family ----

/// Blackfathom Deeps' Kelris. His EventAI notifies an Encounter Binding, so an encounter Package
/// owns his fight.
const KELRIS: u32 = 4_832;
/// A broadcast text Kelris speaks, and one nothing encounter-owned names.
const KELRIS_TEXT: u32 = 900;
const ORDINARY_TEXT: u32 = 901;
const PACKAGE_QUEST_EVENT_REQUIREMENT: u64 = 17_000_001;
const MISSING_QUEST: u32 = 4_000_000;

/// Two imported broadcast texts, in the shape the `creature-ai` base import loads them.
fn seed_broadcast_texts(standalone: &Standalone) {
    for (id, line) in [
        (KELRIS_TEXT, "Ah, sweet innocence."),
        (ORDINARY_TEXT, "Halt."),
    ] {
        standalone.assert_sql(&format!(
            "INSERT INTO game_creature_ai_broadcast_text \
             (id, male_text, female_text, chat_type, language_id, emote_delay_1_ms, emote_id_1, \
             emote_delay_2_ms, emote_id_2, emote_delay_3_ms, emote_id_3) \
             VALUES ({id}, '{line}', '{line}', 1, 0, 0, 0, 0, 0, 0, 0)"
        ));
    }
}

/// Kelris's loaded definition: one rule that speaks [`KELRIS_TEXT`] and notifies the encounter.
/// That notification IS the Encounter Binding, so the text it speaks is encounter-owned.
fn kelris_definition() -> String {
    let subject = format!("entry:{KELRIS}");
    let rules = format!(
        "10,aggro,100,4294967295,once,all,ordinary,any-posture,\
         speak:yell:self:{KELRIS_TEXT}+notify-encounter:blackfathom-deeps-kelris:begin"
    );
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(format!("{subject}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(
        hasher.finalize().as_bytes()[..8]
            .try_into()
            .expect("a BLAKE3 digest has at least eight bytes"),
    );
    format!("{subject}@{revision}@{rules}")
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

/// The creature-ai family has two preflight jobs the whole plan answers. A quest event requirement
/// names a quest another family owns, and a catalogue row may already belong to an encounter whose
/// Package owns that creature's fight. The last apply also proves the base-family replay: a Package
/// that leaves the enabled set takes its invented rows with it.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn creature_ai_claims_are_checked_against_quests_and_encounter_ownership() {
    let standalone = Standalone::start("package-delta-creature-ai-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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
        &quest_event_requirement_insert(FIXTURE_QUEST),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let requirement = standalone.query_rows(&format!(
        "SELECT * FROM game_quest_event_requirement WHERE id = {PACKAGE_QUEST_EVENT_REQUIREMENT}"
    ));
    assert_eq!(requirement[0]["quest_entry"], FIXTURE_QUEST.to_string());

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

    // A line no encounter-owned definition speaks is the ordinary case, and tuning it is the point.
    let accepted = apply(
        &standalone,
        "creature-ai",
        &broadcast_text_update(ORDINARY_TEXT),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let text = standalone.query_rows(&format!(
        "SELECT * FROM game_creature_ai_broadcast_text WHERE id = {ORDINARY_TEXT}"
    ));
    // `spacetime sql` quotes a string column, so the expected value carries the quotes.
    assert_eq!(text[0]["male_text"], "\"You will burn.\"");

    // That plan carries no quest event requirement, so the Package that invented one is gone from
    // the enabled set and its row went with it.
    assert!(standalone
        .query_rows(&format!(
            "SELECT * FROM game_quest_event_requirement WHERE id = {PACKAGE_QUEST_EVENT_REQUIREMENT}"
        ))
        .is_empty());
}
