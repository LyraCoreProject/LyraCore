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
