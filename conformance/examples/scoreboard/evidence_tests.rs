use super::*;
use serde_json::{json, Value};

fn example() -> (FeatureLedger, Value) {
    let ledger =
        FeatureLedger::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("features")).unwrap();
    let golden = wire_client::catalogue::get("connection_identity").unwrap();
    (
        ledger,
        json!({
            "schema_version": 2, "tier": "tier1", "core_commit": "1".repeat(40),
            "harness_revision": "2".repeat(40), "working_tree_dirty": false, "recorded_at_unix_ms": 1,
            "scenarios": [{"id": "connection_identity", "behavior": "connection_identity",
                "scenario_sha256": golden.trace().header.scenario_sha256, "comparison": {"differences": []}}],
        }),
    )
}

fn difference(class: &str) -> Value {
    json!({"classification": class, "checkpoint": "flight", "stream": {"kind":"broadcast", "id":"zones"},
        "packet_index": 6, "field_path": ["sound"], "opcode": 756, "deviations": [],
        "rule": "zone_weather_sound", "reason": "World data determines the weather sound.",
        "expected": {"type":"unsigned", "value": 0}, "actual": {"type":"unsigned", "value": 8533}})
}

fn parse(value: &Value, ledger: &FeatureLedger) -> Result<Report> {
    Report::parse(
        &serde_json::to_vec(value)?,
        value["tier"].as_str().unwrap(),
        ledger,
    )
}

fn state(value: &Value, ledger: &FeatureLedger) -> State {
    parse(value, ledger)
        .unwrap()
        .state(
            ledger
                .behaviors()
                .iter()
                .find(|b| b.id == "connection_identity")
                .unwrap(),
            &"1".repeat(40),
            &"2".repeat(40),
        )
        .unwrap()
}

fn audit_example() -> (FeatureLedger, Value) {
    let (ledger, mut value) = example();
    value["tier"] = json!("conformance_audit");
    value["scenarios"][0]["reference_comparison"] = json!({"differences": []});
    let golden = wire_client::catalogue::get("connection_identity").unwrap();
    let wire_client::trace::Origin::Golden { provenance } = &golden.trace().header.origin else {
        panic!()
    };
    value["audit"] = json!({"native_host_sha256": "a".repeat(64), "module_sha256": "b".repeat(64),
        "content": "hand-written fixture", "reference": provenance, "previous_report": null,
        "behaviors": {"connection_identity": {"candidate": Summary::default(), "reference": Summary::default(), "changes": null}}});
    (ledger, value)
}

fn update_summary(value: &mut Value) {
    let report: Report = serde_json::from_value(value.clone()).unwrap();
    value["audit"]["behaviors"]["connection_identity"]["candidate"] =
        json!(report.summary("connection_identity", false));
    value["audit"]["behaviors"]["connection_identity"]["reference"] =
        json!(report.summary("connection_identity", true));
}

#[test]
fn signatures_ignore_indices_values_and_multiplicity_but_preserve_every_key() {
    let first: Difference = serde_json::from_value(difference("rule")).unwrap();
    let mut second = first.clone();
    second.packet_index = Some(97);
    second.actual = None;
    let summary = Summary::collect([&first, &second].into_iter());
    assert_eq!(summary.rule.raw_records, 2);
    assert_eq!(summary.rule.signatures.len(), 1);
    assert_eq!(summary.rule.signatures[0].raw_records, 2);
    for key in ["rule", "checkpoint", "stream", "opcode", "field_path"] {
        let mut changed = difference("rule");
        changed[key] = match key {
            "stream" => json!({"kind":"reply", "id":"zones"}),
            "opcode" => json!(706),
            "field_path" => json!(["weather_type"]),
            _ => json!("another"),
        };
        let changed: Difference = serde_json::from_value(changed).unwrap();
        assert_eq!(
            Summary::collect([&first, &changed].into_iter())
                .rule
                .signatures
                .len(),
            2,
            "{key}"
        );
    }
    let now = Summary::collect([&second].into_iter());
    let changes = Changes::between(&summary, &now);
    assert!(changes.rule.fixed.is_empty() && changes.rule.regressed.is_empty());
    second.rule = "missing_packet".into();
    let changes = Changes::between(&summary, &Summary::collect([&second].into_iter()));
    assert_eq!(changes.rule.fixed.len(), 1);
    assert_eq!(changes.rule.regressed.len(), 1);
}

#[test]
fn both_tiers_use_tags_and_missing_or_unknown_tags_fail() {
    let (ledger, mut value) = example();
    value["scenarios"][0]["comparison"]["differences"] = json!([difference("data")]);
    assert!(state(&value, &ledger).is_match());
    value["scenarios"][0]["comparison"]["differences"][0]["classification"] = json!("rule");
    assert!(!state(&value, &ledger).is_match());
    for replacement in [Value::Null, json!("unknown")] {
        value["scenarios"][0]["comparison"]["differences"][0]["classification"] = replacement;
        assert!(parse(&value, &ledger).is_err());
    }
    value["scenarios"][0]["comparison"]["differences"][0]
        .as_object_mut()
        .unwrap()
        .remove("classification");
    assert!(parse(&value, &ledger).is_err());
    let (ledger, mut value) = audit_example();
    value["scenarios"][0]["comparison"]["differences"] = json!([difference("data")]);
    assert!(parse(&value, &ledger).is_err());
    update_summary(&mut value);
    assert!(state(&value, &ledger).is_match());
    value["scenarios"][0]["comparison"]["differences"][0]["classification"] = json!("rule");
    assert!(parse(&value, &ledger)
        .unwrap_err()
        .to_string()
        .contains("cannot re-derive"));
}

#[test]
fn reports_reject_falsified_counts_signatures_and_reference_summaries() {
    let (ledger, mut value) = audit_example();
    value["scenarios"][0]["comparison"]["differences"] =
        json!([difference("rule"), difference("rule")]);
    update_summary(&mut value);
    assert!(parse(&value, &ledger).is_ok());
    for path in [
        "/audit/behaviors/connection_identity/candidate/rule/raw_records",
        "/audit/behaviors/connection_identity/candidate/rule/signatures/0/raw_records",
        "/audit/behaviors/connection_identity/reference/rule/raw_records",
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(path).unwrap() = json!(99);
        assert!(parse(&invalid, &ledger).is_err());
    }
    value["audit"]["behaviors"]["connection_identity"]["candidate"]["rule"]["signatures"][0]
        ["signature"]["rule"] = json!("fake");
    assert!(parse(&value, &ledger).is_err());
}

#[test]
fn evidence_requires_current_revisions_complete_scenarios_and_clean_capture() {
    let (ledger, value) = example();
    assert!(state(&value, &ledger).is_match());
    for field in ["core_commit", "harness_revision"] {
        let mut stale = value.clone();
        stale[field] = json!("3".repeat(40));
        assert_eq!(state(&stale, &ledger), State::Stale);
    }
    let mut changed = value.clone();
    changed["scenarios"][0]["scenario_sha256"] = json!("0".repeat(64));
    assert_eq!(state(&changed, &ledger), State::Stale);
    let mut missing = value.clone();
    missing["scenarios"] = json!([]);
    assert_eq!(state(&missing, &ledger), State::NotRun);
    let mut dirty = value.clone();
    dirty["working_tree_dirty"] = json!(true);
    assert!(matches!(state(&dirty, &ledger), State::Unverified(_)));
}

#[test]
fn reference_rule_failures_prevent_a_pass_but_data_records_do_not() {
    let (ledger, mut value) = audit_example();
    for class in ["rule", "data"] {
        value["scenarios"][0]["reference_comparison"]["differences"] = json!([difference(class)]);
        update_summary(&mut value);
        if class == "rule" {
            assert!(matches!(
                state(&value, &ledger),
                State::UnstableReference(_)
            ));
        } else {
            assert!(state(&value, &ledger).is_match());
        }
    }
}

#[test]
fn unknown_extra_duplicate_and_incomplete_results_fail() {
    let (ledger, value) = example();
    for (field, replacement) in [
        ("comparison", json!({})),
        ("behavior", json!("absent")),
        ("id", json!("unlisted")),
    ] {
        let mut invalid = value.clone();
        invalid["scenarios"][0][field] = replacement;
        assert!(parse(&invalid, &ledger).is_err());
    }
    let mut duplicate = value.clone();
    duplicate["scenarios"]
        .as_array_mut()
        .unwrap()
        .push(value["scenarios"][0].clone());
    assert!(parse(&duplicate, &ledger).is_err());
    let (ledger, mut audit) = audit_example();
    audit["scenarios"][0]
        .as_object_mut()
        .unwrap()
        .remove("reference_comparison");
    assert!(parse(&audit, &ledger).is_err());
}

#[test]
fn fixed_and_regressed_sets_are_checked_against_the_named_report() {
    let (ledger, mut previous) = audit_example();
    previous["scenarios"][0]["comparison"]["differences"] =
        json!([difference("rule"), difference("rule"), difference("data")]);
    update_summary(&mut previous);
    let (_, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(2);
    current["audit"]["previous_report"] = json!("previous.json");
    let changes = Changes::between(
        &parse(&previous, &ledger)
            .unwrap()
            .summary("connection_identity", false),
        &Summary::default(),
    );
    assert_eq!(changes.rule.fixed.len(), 1);
    assert_eq!(changes.data.fixed.len(), 1);
    current["audit"]["behaviors"]["connection_identity"]["changes"] = json!(changes);
    let directory = PathBuf::from(std::env::var_os("HOME").unwrap())
        .join(".cache/lyracore-scratch/conformance")
        .join(format!("changes-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("previous.json"),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes(&directory, &ledger)
        .is_ok());
    current["audit"]["behaviors"]["connection_identity"]["changes"]["rule"]["fixed"] = json!([]);
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes(&directory, &ledger)
        .is_err());
    current["audit"]["previous_report"] = json!("../previous.json");
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes(&directory, &ledger)
        .is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn latest_audit_uses_capture_time_and_rejects_malformed_evidence() {
    let directory = PathBuf::from(std::env::var_os("HOME").unwrap())
        .join(".cache/lyracore-scratch/conformance")
        .join(format!("scoreboard-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let (ledger, mut value) = audit_example();
    std::fs::write(
        directory.join("z.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    value["recorded_at_unix_ms"] = json!(2);
    std::fs::write(
        directory.join("a.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert_eq!(
        latest_audit(&directory, &ledger)
            .unwrap()
            .unwrap()
            .0
            .file_name()
            .unwrap(),
        "a.json"
    );
    std::fs::write(directory.join("a.json"), b"{}").unwrap();
    assert!(latest_audit(&directory, &ledger).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}
