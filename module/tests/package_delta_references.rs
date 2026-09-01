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
const MISSING_GOSSIP_ROW: u32 = 12_999_999; // Inside the Package Gossip Range but nothing inserts it.
const PACKAGE_NPC_TEXT: u32 = 12_000_001;
const PACKAGE_NPC_TEXT_SLOT: u64 = 12_000_002;
const PACKAGE_GOSSIP_MENU_PROFILE: u32 = 12_000_003;
const PACKAGE_GOSSIP_MENU_PROFILE_OPTION: u32 = 12_000_004;
const PACKAGE_GOSSIP_OPTION: u32 = 12_000_005;

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

fn artifact_many(package: &str, claims: &[String]) -> String {
    format!(
        r#"{{"version":1,"package":"{package}","source_hash":"{HASH}","claims":[{}]}}"#,
        claims.join(",")
    )
}

fn npc_text_insert(text_id: u32) -> String {
    format!(
        r#"{{"table":"game_npc_text","key":{{"text_id":{text_id}}},"operation":"insert","fields":{{"text":{{"type":"string","value":"New words."}}}}}}"#
    )
}

fn npc_text_slot_insert(id: u64, text_id: u32) -> String {
    format!(
        r#"{{"table":"game_npc_text_slot","key":{{"id":{id}}},"operation":"insert","fields":{{"text_id":{{"type":"u32","value":{text_id}}},"slot_index":{{"type":"u8","value":0}},"text_male":{{"type":"string","value":"New words."}},"text_female":{{"type":"string","value":"New words."}},"probability":{{"type":"f32","value":1.0}}}}}}"#
    )
}

fn gossip_menu_profile_insert(menu_id: u32, text_id: u32) -> String {
    format!(
        r#"{{"table":"game_gossip_menu_profile","key":{{"menu_id":{menu_id}}},"operation":"insert","fields":{{"text_id":{{"type":"u32","value":{text_id}}}}}}}"#
    )
}

fn gossip_menu_profile_option_insert(row_id: u32, menu_id: u32) -> String {
    format!(
        r#"{{"table":"game_gossip_menu_profile_option","key":{{"row_id":{row_id}}},"operation":"insert","fields":{{"menu_id":{{"type":"u32","value":{menu_id}}},"option_index":{{"type":"u32","value":0}},"icon":{{"type":"u32","value":0}},"text":{{"type":"string","value":"Train me."}},"action":{{"type":"u32","value":5}},"action_menu_id":{{"type":"u32","value":0}},"cond_type":{{"type":"u32","value":0}},"cond_value1":{{"type":"u32","value":0}},"cond_value2":{{"type":"u32","value":0}}}}}}"#
    )
}

fn gossip_option_insert(row_id: u32, entry: u32) -> String {
    format!(
        r#"{{"table":"game_gossip_option","key":{{"row_id":{row_id}}},"operation":"insert","fields":{{"entry":{{"type":"u32","value":{entry}}},"option_index":{{"type":"u32","value":0}},"icon":{{"type":"u32","value":0}},"text":{{"type":"string","value":"Train me."}},"action":{{"type":"u32","value":5}},"action_menu_id":{{"type":"u32","value":0}},"cond_type":{{"type":"u32","value":0}},"cond_value1":{{"type":"u32","value":0}},"cond_value2":{{"type":"u32","value":0}}}}}}"#
    )
}

fn gossip_menu_update(entry: u32, text_id: u32) -> String {
    format!(
        r#"{{"table":"game_gossip_menu","key":{{"entry":{entry}}},"operation":"update","fields":{{"text_id":{{"type":"u32","value":{text_id}}}}}}}"#
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

/// `game_npc_text_slot.text_id` and `game_gossip_menu_profile.text_id` both name a `game_npc_text`
/// row that the SAME plan is free to insert — the Package gossip range is cleared right before the
/// write pass, so a Package-range `text_id` can only exist because this plan inserts it.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn gossip_claims_may_reference_a_row_the_same_plan_inserts() {
    let standalone = Standalone::start("package-delta-gossip-same-plan-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused = apply(
        &standalone,
        "gossip",
        &npc_text_slot_insert(PACKAGE_NPC_TEXT_SLOT, MISSING_GOSSIP_ROW),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("text_id"),
        "{}",
        refusal_text(&refused)
    );

    let accepted = apply(
        &standalone,
        "gossip",
        &artifact_many(
            "example.gossip",
            &[
                npc_text_insert(PACKAGE_NPC_TEXT),
                npc_text_slot_insert(PACKAGE_NPC_TEXT_SLOT, PACKAGE_NPC_TEXT),
            ],
        ),
    );
    assert!(accepted.status.success(), "{}", refusal_text(&accepted));
    let slot = standalone.query_rows(&format!(
        "SELECT * FROM game_npc_text_slot WHERE id = {PACKAGE_NPC_TEXT_SLOT}"
    ));
    assert_eq!(slot[0]["text_id"], PACKAGE_NPC_TEXT.to_string());

    let refused_profile_option = apply(
        &standalone,
        "gossip",
        &gossip_menu_profile_option_insert(PACKAGE_GOSSIP_MENU_PROFILE_OPTION, MISSING_GOSSIP_ROW),
    );
    assert!(!refused_profile_option.status.success());
    assert!(
        refusal_text(&refused_profile_option).contains("menu_id"),
        "{}",
        refusal_text(&refused_profile_option)
    );

    let accepted_profile_option = apply(
        &standalone,
        "gossip",
        &artifact_many(
            "example.gossip-profile",
            &[
                gossip_menu_profile_insert(PACKAGE_GOSSIP_MENU_PROFILE, PACKAGE_NPC_TEXT),
                gossip_menu_profile_option_insert(
                    PACKAGE_GOSSIP_MENU_PROFILE_OPTION,
                    PACKAGE_GOSSIP_MENU_PROFILE,
                ),
            ],
        ),
    );
    assert!(
        accepted_profile_option.status.success(),
        "{}",
        refusal_text(&accepted_profile_option)
    );
    let profile_option = standalone.query_rows(&format!(
        "SELECT * FROM game_gossip_menu_profile_option WHERE row_id = {PACKAGE_GOSSIP_MENU_PROFILE_OPTION}"
    ));
    assert_eq!(
        profile_option[0]["menu_id"],
        PACKAGE_GOSSIP_MENU_PROFILE.to_string()
    );
}

/// `game_gossip_option.entry` must be a `game_gossip_menu` row. Unlike the other four inventable
/// gossip tables, `game_gossip_menu` is update-only, so this reference can never be satisfied by an
/// insert in the SAME plan — only by a row a base import already put there. No scenario fixture
/// seeds one (`debug_seed_scenario_fixtures` seeds no `game_gossip_menu` row at all), so this test
/// only covers the refusal; see the T2 handoff notes for the accepted-path gap.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_gossip_option_referencing_no_gossip_menu_is_refused() {
    let standalone = Standalone::start("package-delta-gossip-option-references");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused = apply(
        &standalone,
        "gossip",
        &gossip_option_insert(PACKAGE_GOSSIP_OPTION, FIXTURE_CREATURE),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("entry"),
        "{}",
        refusal_text(&refused)
    );
}

/// `game_gossip_menu` is update-only and carries no Package band, so its own preflight is the
/// generic "not in this shard" check every claim family shares, not a `check_references` refusal.
/// No scenario fixture creature carries a `game_gossip_menu` row, so every update on it is refused
/// this way today; see the T2 handoff notes for the gap that would let this test also cover the
/// accepted "point a real NPC at newly inserted text" path.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_update_on_an_absent_gossip_menu_row_is_refused() {
    let standalone = Standalone::start("package-delta-gossip-menu-update");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);

    let refused = apply(
        &standalone,
        "gossip",
        &gossip_menu_update(FIXTURE_CREATURE, PACKAGE_NPC_TEXT),
    );
    assert!(!refused.status.success());
    assert!(
        refusal_text(&refused).contains("not in this shard"),
        "{}",
        refusal_text(&refused)
    );
}
