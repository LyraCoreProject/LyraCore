use lyracore_conformance::FeatureLedger;

const EXAMPLE: &str = include_str!("fixtures/behavior.toml");

fn load(source: &str) -> anyhow::Result<FeatureLedger> {
    FeatureLedger::from_sources([("quests/quest_reward.toml", source)])
}

#[test]
fn a_behavior_has_no_derived_status_and_keeps_evidence_requirements() {
    let ledger = load(EXAMPLE).unwrap();
    let behavior = &ledger.behaviors()[0];
    assert_eq!(behavior.id, "quest_reward");
    assert_eq!(behavior.requests, ["0x0189", "0x018E"]);
    assert!(behavior.scenarios.is_empty());
    for status in ["status", "support", "remaining"] {
        assert!(load(&format!("{status} = \"done\"\n{EXAMPLE}")).is_err());
    }
}

#[test]
fn malformed_metadata_does_not_enter_the_ledger() {
    for (old, new, expected) in [
        ("schema_version = 1", "schema_version = 2", "schema_version"),
        ("id = \"quest_reward\"", "id = \"../quest_reward\"", "identifier"),
        ("system = \"quests\"", "system = \"other\"", "expected other"),
        ("0x0189", "0xGGGG", "opcode"),
        ("\"0x0189\", \"0x018E\"", "\"0x0189\", \"0x0189\"", "duplicate requests"),
        ("d9173eca3ea08dd985c52dabbcc826898d1d0b77", "development", "full nonzero SHA"),
        ("src/game/Handlers/QuestHandler.cpp", "../QuestHandler.cpp", "relative source path"),
        ("scenarios = []", "scenarios = [\"\", \"choice\"]", "scenario id is empty"),
        ("scenarios = []", "scenarios = [\"choice\", \"choice\"]", "duplicate scenarios"),
        ("required_rungs = [\"durable_module\", \"tier1\", \"conformance_audit\", \"real_client\"]", "required_rungs = []", "required_rungs is empty"),
        ("rung = \"durable_module\"", "rung = \"real_client\"", "Rust source evidence"),
    ] {
        assert!(EXAMPLE.contains(old), "mutation did not match {old}");
        let failure = load(&EXAMPLE.replace(old, new)).unwrap_err();
        assert!(format!("{failure:#}").contains(expected), "{failure:#}");
    }
}

#[test]
fn dependencies_must_exist_and_form_an_acyclic_graph() {
    let first = EXAMPLE.replace("dependencies = []", "dependencies = [\"other\"]");
    assert!(format!("{:#}", load(&first).unwrap_err()).contains("unknown Behavior dependency"));
    let other = EXAMPLE.replace("id = \"quest_reward\"", "id = \"other\"");
    assert!(FeatureLedger::from_sources([
        ("quests/quest_reward.toml", first.as_str()),
        ("quests/other.toml", other.as_str())
    ])
    .is_ok());
    let cycle = other.replace("dependencies = []", "dependencies = [\"quest_reward\"]");
    let failure = FeatureLedger::from_sources([
        ("quests/quest_reward.toml", first.as_str()),
        ("quests/other.toml", cycle.as_str()),
    ])
    .unwrap_err();
    assert!(format!("{failure:#}").contains("dependency cycle"));
    let self_cycle = EXAMPLE.replace("dependencies = []", "dependencies = [\"quest_reward\"]");
    assert!(format!("{:#}", load(&self_cycle).unwrap_err()).contains("dependency cycle"));
}

#[test]
fn duplicate_ids_and_wrong_file_locations_fail() {
    let other = EXAMPLE.replace("system = \"quests\"", "system = \"other\"");
    assert!(FeatureLedger::from_sources([
        ("quests/quest_reward.toml", EXAMPLE),
        ("other/quest_reward.toml", other.as_str())
    ])
    .unwrap_err()
    .to_string()
    .contains("duplicate Behavior"));
    assert!(FeatureLedger::from_sources([("quests/wrong.toml", EXAMPLE)]).is_err());
    assert!(FeatureLedger::from_sources([]).is_err());
}

#[test]
fn scenario_cross_check_detects_unknown_and_duplicate_ids() {
    let source = EXAMPLE.replace(
        "scenarios = []",
        "scenarios = [\"quests/choice-reward.v1\"]",
    );
    let ledger = load(&source).unwrap();
    assert!(ledger
        .validate_scenarios([("quests/choice-reward.v1", "quest_reward")])
        .is_ok());
    assert!(ledger
        .validate_scenarios([("different", "quest_reward")])
        .unwrap_err()
        .to_string()
        .contains("absent scenario"));
    assert!(ledger
        .validate_scenarios([
            ("quests/choice-reward.v1", "quest_reward"),
            ("quests/choice-reward.v1", "quest_reward")
        ])
        .is_err());
    assert!(ledger.validate_scenarios([("", "quest_reward")]).is_err());
    assert!(ledger
        .validate_scenarios([("quests/choice-reward.v1", "different_behavior")])
        .unwrap_err()
        .to_string()
        .contains("but quest_reward lists it"));
}

#[test]
fn real_client_verification_needs_a_valid_date_reviewer_and_evidence() {
    let verified = EXAMPLE.replace("status = \"unverified\"", "status = \"verified\"\ndate = \"2024-02-29\"\nchecked_by = \"maintainer\"\nevidence = \"conformance/reports/client-check.md\"");
    assert!(load(&verified).is_ok());
    for (old, new) in [
        ("2024-02-29", "2025-02-29"),
        ("2024-02-29", "2024-13-01"),
        ("maintainer", ""),
        ("conformance/reports/client-check.md", ""),
    ] {
        assert!(load(&verified.replace(old, new)).is_err());
    }
    assert!(load(&EXAMPLE.replace("status = \"unverified\"", "status = \"verified\"")).is_err());
}

#[test]
fn a_deviation_needs_a_result_reason_and_source() {
    let source = EXAMPLE.replace("deliberate_deviations = []", "deliberate_deviations = [{id = \"example\", result = \"A different reply\", reason = \"An explicit product decision\", source = \"https://example.com/decision\"}]");
    assert!(load(&source).is_ok());
    assert!(load(&source.replace("An explicit product decision", "")).is_err());
    assert!(load(&source.replace("https://example.com/decision", "")).is_err());
}

#[test]
fn unknown_nested_fields_fail_instead_of_disappearing() {
    for table in ["vmangos", "real_client"] {
        let source = EXAMPLE.replace(
            &format!("[{table}]"),
            &format!("[{table}]\ntypo = \"unexpected\""),
        );
        assert!(load(&source).is_err(), "unknown field in {table}");
    }
}

#[test]
fn ledger_cannot_classify_data() {
    assert!(load(&format!(
        "{EXAMPLE}\n[[data_fields]]\nscenario = \"choice\"\n"
    ))
    .is_err());
}

#[test]
fn known_gaps_require_locations_and_evidence_without_status() {
    assert!(load(EXAMPLE).unwrap().behaviors()[0].known_gaps.is_empty());
    let gap = r#"
[[known_gaps]]
description = "Reward reply is absent."
path = "module/src/quests.rs"
symbol = "reward_quest"
evidence = "Comparison record 1: missing_packet."
"#;
    let source = format!("{EXAMPLE}\n{gap}");
    let ledger = load(&source).unwrap();
    assert_eq!(ledger.behaviors()[0].known_gaps.len(), 1);
    for (old, new) in [
        ("Reward reply is absent.", " "),
        ("module/src/quests.rs", "../quests.rs"),
        ("reward_quest", ""),
        ("Comparison record 1: missing_packet.", ""),
    ] {
        assert!(load(&source.replace(old, new)).is_err());
    }
    assert!(load(&format!("{source}\nstatus = \"open\"\n")).is_err());
    assert!(load(&format!("known_gaps = [\"missing reply\"]\n{EXAMPLE}")).is_err());
}

#[test]
fn world_data_behavior_needs_a_gap_when_it_claims_no_opcode() {
    let no_opcodes = EXAMPLE
        .replace("requests = [\"0x0189\", \"0x018E\"]", "requests = []")
        .replace("replies = [\"0x0191\"]", "replies = []");
    assert!(load(&no_opcodes).is_err());
    let with_gap = format!(
        "{no_opcodes}\n[[known_gaps]]\ndescription = \"Source gate is absent.\"\npath = \"module/src/quest.rs\"\nsymbol = \"quest_gate\"\nevidence = \"vmangos manifest quests.condition\"\n"
    );
    assert_eq!(load(&with_gap).unwrap().behaviors()[0].known_gaps.len(), 1);
}
