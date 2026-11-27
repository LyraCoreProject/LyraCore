import copy
import hashlib
import json
from pathlib import Path
import unittest

from conformance import evidence_format as evidence


BASELINE = json.loads((Path(__file__).resolve().parents[1] / "tier1-baseline.json").read_text())


def document():
    return {
        "schema_version": 1,
        "merge_base": "a" * 40,
        "head": "b" * 40,
        "tests": [{
            "package": "lyracore-module",
            "target": "lib",
            "name": "spell::tests::frostbolt_slows_on_impact",
            "features": ["debug_reducers"],
            "status": "shown",
            "base": "failed",
            "head": "passed",
            "reason": None,
        }, {
            "package": "lyracore-module",
            "target": "weather",
            "name": "weather_changes_with_the_season",
            "features": [],
            "status": "not_shown",
            "base": "not_run",
            "head": "passed",
            "reason": "weather_changes_with_the_season is defined in module/src/weather.rs, "
                      "which this pull request changes outside its test files",
        }],
        "scenarios": [{"id": "bank_slot_purchase", "exit_code": 0, "signature_digest": "c" * 64},
                      {"id": "taxi_single_hop", "exit_code": 2, "signature_digest": None}],
        "tools": {"rustc": "1.97.1 (5ef14bb0d 2026-08-04)", "spacetime": "2.14.0"},
        "platform": {"os": "linux", "arch": "x86_64"},
    }


class ValidationTests(unittest.TestCase):
    def assertRejected(self, change):
        candidate = document()
        change(candidate)
        with self.assertRaises(evidence.EvidenceError):
            evidence.validate(candidate)

    def test_a_complete_document_survives_render_and_parse(self):
        self.assertEqual(evidence.parse(evidence.render(document())), document())

    def test_unknown_fields_are_rejected_at_every_level(self):
        for owner in (lambda d: d, lambda d: d["tests"][0], lambda d: d["scenarios"][0],
                      lambda d: d["tools"], lambda d: d["platform"]):
            with self.subTest(owner=owner):
                self.assertRejected(lambda d: owner(d).__setitem__("hostname", "laptop"))

    def test_missing_fields_are_rejected(self):
        self.assertRejected(lambda d: d.pop("platform"))
        self.assertRejected(lambda d: d["tests"][0].pop("reason"))
        self.assertRejected(lambda d: d["scenarios"][0].pop("signature_digest"))

    def test_only_schema_version_1_is_accepted(self):
        self.assertRejected(lambda d: d.__setitem__("schema_version", 2))
        self.assertRejected(lambda d: d.__setitem__("schema_version", True))

    def test_status_must_follow_the_base_and_head_outcomes(self):
        def passed_at_base(d):
            d["tests"][0]["base"] = "passed"
        self.assertRejected(passed_at_base)

        def failed_at_head(d):
            d["tests"][0]["head"] = "failed"
        self.assertRejected(failed_at_head)

        compiled = document()
        compiled["tests"][0]["base"] = "did_not_compile"
        evidence.validate(compiled)

    def test_a_reason_is_required_exactly_when_a_test_is_not_shown(self):
        self.assertRejected(lambda d: d["tests"][0].__setitem__("reason", "because"))
        self.assertRejected(lambda d: d["tests"][1].__setitem__("reason", None))
        self.assertRejected(lambda d: d["tests"][1].__setitem__("reason", "line\nbreak"))

    def test_a_merge_base_outcome_of_not_run_never_counts_as_shown(self):
        def not_run(d):
            d["tests"][0]["base"] = "not_run"
        self.assertRejected(not_run)

    def test_a_digest_is_present_exactly_when_the_comparison_completed(self):
        self.assertRejected(lambda d: d["scenarios"][0].__setitem__("signature_digest", None))
        self.assertRejected(lambda d: d["scenarios"][1].__setitem__("signature_digest", "c" * 64))
        self.assertRejected(lambda d: d["scenarios"][0].__setitem__("exit_code", False))
        timed_out = document()
        timed_out["scenarios"][1]["exit_code"] = None
        evidence.validate(timed_out)

    def test_repeated_entries_and_empty_evidence_are_rejected(self):
        self.assertRejected(lambda d: d["scenarios"].append(copy.deepcopy(d["scenarios"][0])))
        self.assertRejected(lambda d: d.update(tests=[], scenarios=[]))

    def test_selectors_and_versions_cannot_carry_paths(self):
        self.assertRejected(lambda d: d["tests"][0].__setitem__("package", "/home/someone/crate"))
        self.assertRejected(lambda d: d["tools"].__setitem__("rustc", "/usr/bin/rustc"))

    def test_duplicate_json_keys_are_rejected(self):
        text = evidence.render(document()).replace('"schema_version": 1',
                                                   '"schema_version": 1, "schema_version": 1')
        with self.assertRaisesRegex(evidence.EvidenceError, "duplicate"):
            evidence.parse(text)


class SignatureDigestTests(unittest.TestCase):
    def test_digest_hashes_the_documented_canonical_form(self):
        signature = {"rule": "r", "checkpoint": "c", "stream": {"kind": "reply", "id": "c"},
                     "opcode": 395, "field_path": ["text"]}
        canonical = ('{"data":[],"harness_revision":"h","rule":["{\\"checkpoint\\":\\"c\\",'
                     '\\"field_path\\":[\\"text\\"],\\"opcode\\":395,\\"rule\\":\\"r\\",'
                     '\\"stream\\":{\\"id\\":\\"c\\",\\"kind\\":\\"reply\\"}}"],'
                     '"scenario_sha256":"s"}')
        self.assertEqual(evidence.signature_digest("s", "h", [signature], []),
                         hashlib.sha256(canonical.encode()).hexdigest())

    def test_digest_treats_signatures_as_sets_and_an_absent_character_as_null(self):
        entry = next(e for e in BASELINE["scenarios"] if e["scenario"] == "quest_completion")
        expected = evidence.baseline_signature_digest(BASELINE, "quest_completion")
        shuffled = [dict(signature, character=None) for signature in reversed(entry["data"])]
        self.assertEqual(evidence.signature_digest(entry["scenario_sha256"],
                                                   entry["harness_revision"], entry["rule"],
                                                   shuffled + shuffled[:1]), expected)

    def test_digest_changes_with_provenance_or_classification(self):
        entry = next(e for e in BASELINE["scenarios"] if e["scenario"] == "quest_completion")
        expected = evidence.baseline_signature_digest(BASELINE, "quest_completion")
        for changed in (
            ("other", entry["harness_revision"], entry["rule"], entry["data"]),
            (entry["scenario_sha256"], "other", entry["rule"], entry["data"]),
            (entry["scenario_sha256"], entry["harness_revision"], entry["data"], entry["rule"]),
        ):
            self.assertNotEqual(evidence.signature_digest(*changed), expected)

    def test_a_scenario_without_a_baseline_entry_has_no_digest(self):
        self.assertIsNone(evidence.baseline_signature_digest(BASELINE, "no_such_scenario"))


class TestFileTests(unittest.TestCase):
    def test_test_files_are_test_directories_and_test_modules(self):
        for path in ("module/tests/weather.rs", "module/tests/fixtures/upgrade-baseline.patch",
                     "module/src/spell/tests.rs", "importer/src/spell_aura_tests.rs",
                     "conformance/tests/test_run.py"):
            self.assertTrue(evidence.is_test_file(path), path)
        for path in ("module/src/spell/mod.rs", "module/src/package_test.rs",
                     "conformance/tier1-baseline.json", "tests"):
            self.assertFalse(evidence.is_test_file(path), path)


class PullRequestSectionTests(unittest.TestCase):
    def test_a_body_without_a_section_has_no_document(self):
        self.assertIsNone(evidence.extract("Closes #1384.\n"))
        self.assertIsNone(evidence.extract(None))

    def test_the_section_is_appended_after_the_existing_body(self):
        body = evidence.replace_section("Closes #1384.\n\nNotes.", document())
        self.assertTrue(body.startswith("Closes #1384.\n\nNotes.\n\n" + evidence.SECTION_START))
        self.assertTrue(body.endswith(evidence.SECTION_END + "\n"))
        self.assertIn("<details>", body)
        self.assertEqual(evidence.extract(body), document())

    def test_replacing_removes_an_earlier_section_and_keeps_the_rest(self):
        older = document()
        older["scenarios"] = older["scenarios"][:1]
        first = evidence.replace_section("Summary.", older)
        edited = (first.replace("Summary.", "Summary.\n\n- a reviewer's note")
                  + "\nTrailing words.\n")
        replaced = evidence.replace_section(edited.replace("\n", "\r\n"), document())
        self.assertEqual(replaced.count(evidence.SECTION_START), 1)
        self.assertTrue(replaced.startswith(
            "Summary.\n\n- a reviewer's note\n\n\nTrailing words.\n\n" + evidence.SECTION_START))
        self.assertEqual(evidence.extract(replaced), document())

    def test_ambiguous_or_broken_sections_are_rejected(self):
        section = evidence.render_section(document())
        for body in (f"{section}\n{section}", section.replace(evidence.SECTION_END, ""),
                     section.replace('"schema_version": 1', '"schema_version": 1,'),
                     section.replace("```json", "```")):
            with self.subTest(body=body[-80:]):
                with self.assertRaises(evidence.EvidenceError):
                    evidence.extract(body)
        with self.assertRaises(evidence.EvidenceError):
            evidence.replace_section(section.replace(evidence.SECTION_END, ""), document())


if __name__ == "__main__":
    unittest.main()
