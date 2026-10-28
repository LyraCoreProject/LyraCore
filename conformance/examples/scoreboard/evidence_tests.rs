use super::*;
use serde_json::{json, Value};

#[path = "../../support/scratch.rs"]
mod scratch;

fn example() -> (FeatureLedger, Value) {
    let ledger =
        FeatureLedger::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("features")).unwrap();
    let golden = wire_client::catalogue::get("taxi_single_hop").unwrap();
    (
        ledger,
        json!({
            "schema_version": 2, "tier": "tier1", "core_commit": "1".repeat(40),
            "harness_revision": harness_revision().to_string(), "working_tree_dirty": false, "recorded_at_unix_ms": 1,
            "scenarios": [{"id": "taxi_single_hop", "behavior": "taxi_single_hop",
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
                .find(|b| b.id == "taxi_single_hop")
                .unwrap(),
            &"1".repeat(40),
            &harness_revision().to_string(),
        )
        .unwrap()
}

fn audit_example() -> (FeatureLedger, Value) {
    let (ledger, mut value) = example();
    value["tier"] = json!("conformance_audit");
    value["scenarios"][0]["reference_comparison"] = json!({"differences": []});
    value["scenarios"][0]["reference_sha256"] = json!("c".repeat(64));
    value["scenarios"][0]["capture"] = json!({"sha256": "d".repeat(64), "core_commit": value["core_commit"],
        "native_host_sha256": "a".repeat(64), "module_sha256": "b".repeat(64)});
    let golden = wire_client::catalogue::get("taxi_single_hop").unwrap();
    let wire_client::trace::Origin::Golden { provenance } = &golden.trace().header.origin else {
        panic!()
    };
    value["audit"] = json!({"native_host_sha256": "a".repeat(64), "module_sha256": "b".repeat(64),
        "content": "hand-written fixture", "reference": provenance, "previous_report": null,
        "behaviors": {"taxi_single_hop": {"candidate": Summary::default(), "reference": Summary::default(), "changes": null}}});
    (ledger, value)
}

fn update_summary(value: &mut Value) {
    let report: Report = serde_json::from_value(value.clone()).unwrap();
    value["audit"]["behaviors"]["taxi_single_hop"]["candidate"] =
        json!(report.summary("taxi_single_hop", false));
    value["audit"]["behaviors"]["taxi_single_hop"]["reference"] =
        json!(report.summary("taxi_single_hop", true));
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
        "/audit/behaviors/taxi_single_hop/candidate/rule/raw_records",
        "/audit/behaviors/taxi_single_hop/candidate/rule/signatures/0/raw_records",
        "/audit/behaviors/taxi_single_hop/reference/rule/raw_records",
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(path).unwrap() = json!(99);
        assert!(parse(&invalid, &ledger).is_err());
    }
    value["audit"]["behaviors"]["taxi_single_hop"]["candidate"]["rule"]["signatures"][0]
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
    previous["harness_revision"] = json!("2".repeat(40));
    previous["scenarios"][0]["comparison"]["differences"] =
        json!([difference("rule"), difference("rule"), difference("data")]);
    update_summary(&mut previous);
    let (_, mut current) = audit_example();
    current["harness_revision"] = previous["harness_revision"].clone();
    current["recorded_at_unix_ms"] = json!(2);
    current["audit"]["previous_report"] = json!("previous.json");
    let changes = Changes::between(
        &parse(&previous, &ledger)
            .unwrap()
            .summary("taxi_single_hop", false),
        &Summary::default(),
    );
    assert_eq!(changes.rule.fixed.len(), 1);
    assert_eq!(changes.data.fixed.len(), 1);
    current["audit"]["behaviors"]["taxi_single_hop"]["changes"] = json!(changes);
    let directory = scratch::Scratch::new("changes-test").unwrap();
    std::fs::write(
        directory.join("previous.json"),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_ok());
    current["audit"]["behaviors"]["taxi_single_hop"]["changes"]["rule"]["fixed"] = json!([]);
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_err());
    current["audit"]["previous_report"] = json!("../previous.json");
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_err());
}

fn write_audit(directory: &Path, name: &str, recorded_at_unix_ms: u64) {
    let (_, mut value) = audit_example();
    value["recorded_at_unix_ms"] = json!(recorded_at_unix_ms);
    value["harness_revision"] = json!("2".repeat(40));
    std::fs::write(directory.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
}

#[test]
fn missing_predecessor_link_fails_when_an_earlier_audit_exists() {
    let directory = scratch::Scratch::new("missing-predecessor-test").unwrap();
    write_audit(&directory, "earlier.json", 1);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(2);
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_err());
}

#[test]
fn older_predecessor_link_fails_when_a_newer_audit_exists() {
    let directory = scratch::Scratch::new("older-predecessor-test").unwrap();
    write_audit(&directory, "older.json", 1);
    write_audit(&directory, "newer.json", 2);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(3);
    current["audit"]["previous_report"] = json!("older.json");
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_err());
}

#[test]
fn first_audit_has_no_predecessor_link() {
    let directory = scratch::Scratch::new("first-audit-test").unwrap();
    write_audit(&directory, "future.json", 3);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(2);
    parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .unwrap();
}

#[test]
fn newest_earlier_audit_is_the_predecessor() {
    let directory = scratch::Scratch::new("newest-predecessor-test").unwrap();
    write_audit(&directory, "older.json", 1);
    write_audit(&directory, "newer.json", 2);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(3);
    current["audit"]["previous_report"] = json!("newer.json");
    parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .unwrap();
}

#[test]
fn equal_timestamp_audit_is_not_a_predecessor() {
    let directory = scratch::Scratch::new("equal-time-audit-test").unwrap();
    write_audit(&directory, "equal.json", 2);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(2);
    parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .unwrap();
}

#[test]
fn report_cannot_select_itself_as_predecessor() {
    let directory = scratch::Scratch::new("self-audit-test").unwrap();
    write_audit(&directory, "current.json", 2);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(2);
    current["audit"]["previous_report"] = json!("current.json");
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_err());
}

#[test]
fn equal_earlier_timestamps_use_filename_tie_break() {
    let directory = scratch::Scratch::new("audit-tie-test").unwrap();
    write_audit(&directory, "a.json", 1);
    write_audit(&directory, "z.json", 1);
    let (ledger, mut current) = audit_example();
    current["recorded_at_unix_ms"] = json!(2);
    current["audit"]["previous_report"] = json!("z.json");
    parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .unwrap();
    current["audit"]["previous_report"] = json!("a.json");
    assert!(parse(&current, &ledger)
        .unwrap()
        .validate_changes_in(&directory)
        .is_err());
}

#[test]
fn every_committed_audit_validates_its_predecessor() -> Result<()> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("reports");
    for entry in std::fs::read_dir(&directory)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let report = Report::load_structure(&path, "conformance_audit")?;
            report
                .validate_changes()
                .with_context(|| path.display().to_string())?;
        }
    }
    Ok(())
}

#[test]
fn latest_audit_uses_capture_time_and_rejects_malformed_evidence() {
    let directory = scratch::Scratch::new("scoreboard-test").unwrap();
    let (ledger, mut value) = audit_example();
    let mut historical = value.clone();
    historical["scenarios"][0]["behavior"] = json!("retired_behavior");
    historical["audit"]["behaviors"] =
        json!({"retired_behavior": historical["audit"]["behaviors"]["taxi_single_hop"].clone()});
    std::fs::write(
        directory.join("z.json"),
        serde_json::to_vec(&historical).unwrap(),
    )
    .unwrap();
    value["recorded_at_unix_ms"] = json!(2);
    std::fs::write(
        directory.join("a.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    latest_audit(&directory, None)
        .unwrap()
        .unwrap()
        .1
        .validate("conformance_audit", &ledger)
        .unwrap();
    assert_eq!(
        latest_audit(&directory, None)
            .unwrap()
            .unwrap()
            .0
            .file_name()
            .unwrap(),
        "a.json"
    );
    std::fs::write(
        directory.join("a.json"),
        serde_json::to_vec(&example().1).unwrap(),
    )
    .unwrap();
    assert!(latest_audit(&directory, None).is_err());
}

#[test]
fn current_reports_reject_unknown_checkpoints_and_undeclared_data_fields() {
    let (ledger, mut value) = example();
    value["scenarios"][0]["comparison"]["differences"] = json!([difference("data")]);
    assert!(parse(&value, &ledger).is_ok());
    for (field, replacement) in [
        ("checkpoint", json!("absent")),
        ("rule", json!("absent")),
        ("opcode", json!(1)),
        ("field_path", json!(["absent"])),
    ] {
        let mut invalid = value.clone();
        invalid["scenarios"][0]["comparison"]["differences"][0][field] = replacement;
        assert!(parse(&invalid, &ledger).is_err(), "{field}");
    }
    value["scenarios"][0]["id"] = json!("connection_identity");
    value["scenarios"][0]["behavior"] = json!("connection_identity");
    value["scenarios"][0]["scenario_sha256"] = json!(
        wire_client::catalogue::get("connection_identity")
            .unwrap()
            .trace()
            .header
            .scenario_sha256
    );
    assert!(parse(&value, &ledger).is_err());
}

#[test]
fn changed_harnesses_or_scenario_hashes_cannot_claim_fixes_or_regressions() {
    let (ledger, value) = audit_example();
    let old = parse(&value, &ledger).unwrap();
    assert!(old.changes_since(&old, "taxi_single_hop").is_some());
    for pointer in ["/harness_revision", "/scenarios/0/scenario_sha256"] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() =
            json!("a".repeat(if pointer.contains("sha256") { 64 } else { 40 }));
        let changed = parse(&changed, &ledger).unwrap();
        assert!(changed.changes_since(&old, "taxi_single_hop").is_none());
    }
}

fn replay_fixture(path: &Path) -> Result<(FeatureLedger, Report)> {
    std::fs::create_dir_all(path.with_extension("traces"))?;
    let (ledger, mut value) = audit_example();
    let mut scenarios = Vec::new();
    for entry in wire_client::catalogue::ENTRIES {
        let golden = wire_client::catalogue::get(entry.id)?;
        let mut comparisons = Vec::new();
        let mut digests = Vec::new();
        for adapter in ["lyracore", "vmangos"] {
            let mut trace: Value = serde_json::from_str(entry.trace)?;
            trace["header"]["origin"] = json!({"kind": "candidate", "adapter": adapter});
            if entry.id == "taxi_single_hop" && adapter == "lyracore" {
                trace["packets"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|packet| packet["opcode"] != 427);
            }
            let trace = Trace::parse(&serde_json::to_vec(&trace)?)?;
            let bytes = serde_json::to_vec_pretty(&trace)?;
            if adapter == "lyracore" {
                std::fs::write(
                    path.with_extension("traces")
                        .join(format!("{}-{adapter}.json", entry.id)),
                    &bytes,
                )?;
            }
            digests.push(Sha256Digest::of(&bytes));
            comparisons.push(wire_client::compare::compare(&golden, &trace, &[])?);
        }
        scenarios.push(
            json!({"id": entry.id, "behavior": golden.scenario().behavior,
            "scenario_sha256": golden.trace().header.scenario_sha256,
            "comparison": comparisons[0], "reference_comparison": comparisons[1], "reference_sha256": digests[1],
            "capture": {"sha256": digests[0], "core_commit": value["core_commit"],
                "native_host_sha256": "a".repeat(64), "module_sha256": "b".repeat(64)}}),
        );
    }
    value["scenarios"] = json!(scenarios);
    let mut report: Report = serde_json::from_value(value)?;
    report.audit.as_mut().unwrap().behaviors = report
        .scenarios
        .iter()
        .map(|s| {
            (
                s.behavior.clone(),
                BehaviorAudit {
                    candidate: report.summary(&s.behavior, false),
                    reference: report.summary(&s.behavior, true),
                    changes: None,
                },
            )
        })
        .collect();
    Ok((ledger, report))
}

#[test]
fn replay_rejects_retagged_deleted_added_records_and_edited_markdown() -> Result<()> {
    let directory = scratch::Scratch::new("replay-test")?;
    let path = directory.join("current.json");
    let (ledger, report) = replay_fixture(&path)?;
    let write = |report: &Report| -> Result<()> {
        std::fs::write(&path, serde_json::to_vec(report)?)?;
        std::fs::write(path.with_extension("md"), markdown(report, "current")?)?;
        Ok(())
    };
    write(&report)?;
    Report::load(&path, "conformance_audit", &ledger)?.verify_capture(&path)?;
    for mutation in ["retag", "delete", "add"] {
        let mut changed: Report = serde_json::from_value(serde_json::to_value(&report)?)?;
        let differences = &mut changed
            .scenarios
            .iter_mut()
            .find(|s| s.id == "taxi_single_hop")
            .unwrap()
            .comparison
            .differences;
        assert!(!differences.is_empty());
        match mutation {
            "retag" => differences[0].classification = DifferenceClass::Data,
            "delete" => {
                differences.pop();
            }
            "add" => differences.push(differences[0].clone()),
            _ => unreachable!(),
        }
        changed
            .audit
            .as_mut()
            .unwrap()
            .behaviors
            .get_mut("taxi_single_hop")
            .unwrap()
            .candidate = changed.summary("taxi_single_hop", false);
        write(&changed)?;
        assert!(
            Report::load(&path, "conformance_audit", &ledger)
                .and_then(|r| r.verify_capture(&path))
                .is_err(),
            "{mutation}"
        );
        assert!(
            changed.verify_capture(&path).is_err(),
            "replay alone must catch {mutation}"
        );
    }
    write(&report)?;
    std::fs::write(path.with_extension("md"), "taxi passes")?;
    assert!(Report::load(&path, "conformance_audit", &ledger)?
        .verify_capture(&path)
        .is_err());
    Ok(())
}

#[test]
fn candidate_bytes_and_provenance_must_match_the_audit_run() -> Result<()> {
    let directory = scratch::Scratch::new("capture-test")?;
    let path = directory.join("current.json");
    let (ledger, mut report) = replay_fixture(&path)?;
    let value = serde_json::to_value(&report)?;
    for (field, replacement) in [
        ("core_commit", "2".repeat(40)),
        ("native_host_sha256", "c".repeat(64)),
        ("module_sha256", "c".repeat(64)),
    ] {
        let mut changed = value.clone();
        changed["scenarios"][0]["capture"][field] = json!(replacement);
        assert!(parse(&changed, &ledger)
            .unwrap_err()
            .to_string()
            .contains("candidate provenance"));
    }
    let candidate = path
        .with_extension("traces")
        .join("connection_identity-lyracore.json");
    let mut bytes = std::fs::read(&candidate)?;
    bytes.push(b' ');
    std::fs::write(candidate, bytes)?;
    for revision in [harness_revision().to_string(), "2".repeat(40)] {
        report.harness_revision = revision;
        std::fs::write(path.with_extension("md"), markdown(&report, "current")?)?;
        assert!(report
            .verify_capture(&path)
            .unwrap_err()
            .to_string()
            .contains("candidate SHA-256"));
    }
    Ok(())
}

#[test]
fn fixed_and_regressed_cannot_trust_an_edited_previous_comparison() -> Result<()> {
    let directory = scratch::Scratch::new("previous-replay-test")?;
    let previous_path = directory.join("previous.json");
    let (_, previous) = replay_fixture(&previous_path)?;
    let current_path = directory.join("current.json");
    let (_, mut current) = replay_fixture(&current_path)?;
    current.recorded_at_unix_ms += 1;
    current.audit.as_mut().unwrap().previous_report = Some("previous.json".into());
    let check_previous = |previous: &Report, current: &mut Report| -> Result<()> {
        for (behavior, summary) in &mut current.audit.as_mut().unwrap().behaviors {
            summary.changes = Some(Changes::between(
                &previous.summary(behavior, false),
                &summary.candidate,
            ));
        }
        std::fs::write(&previous_path, serde_json::to_vec(previous)?)?;
        std::fs::write(
            previous_path.with_extension("md"),
            markdown(previous, "previous")?,
        )?;
        current.validate_changes_in(&directory)
    };
    check_previous(&previous, &mut current)?;
    let mut changed = previous;
    changed
        .scenarios
        .iter_mut()
        .find(|s| s.id == "taxi_single_hop")
        .unwrap()
        .comparison
        .differences
        .clear();
    changed
        .audit
        .as_mut()
        .unwrap()
        .behaviors
        .get_mut("taxi_single_hop")
        .unwrap()
        .candidate = Summary::default();
    assert!(check_previous(&changed, &mut current)
        .unwrap_err()
        .to_string()
        .contains("candidate replay"));
    Ok(())
}
