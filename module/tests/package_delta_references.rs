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
