import copy
from pathlib import Path
import runpy
import unittest


RATCHET = runpy.run_path(str(Path(__file__).resolve().parents[1] / "check_ratchet.py"))


def baseline():
    return {
        "schema_version": 1,
        "scenarios": [{
            "scenario": "sample", "behavior": "sample",
            "scenario_sha256": "a" * 64, "harness_revision": "b" * 40,
            "rule": [{"rule": "missing_packet", "checkpoint": "reply", "stream": None,
                      "opcode": 1, "field_path": []}],
            "data": [],
        }],
    }


class RatchetTests(unittest.TestCase):
    def setUp(self):
        self.base = baseline()
        self.current = copy.deepcopy(self.base)

    def compare(self, labels=None):
        return RATCHET["compare"](self.base, self.current, labels or [])

    def add_signature(self):
        self.current["scenarios"][0]["rule"].append({
            **self.current["scenarios"][0]["rule"][0], "opcode": 2,
        })

    def test_added_rule_signature_fails(self):
        self.add_signature()
        result = self.compare()
        self.assertFalse(result["passes"])
        self.assertEqual(result["added_rule_signatures"]["sample"][0]["opcode"], 2)

    def test_removed_rule_signature_passes(self):
        self.current["scenarios"][0]["rule"] = []
        result = self.compare()
        self.assertTrue(result["passes"])
        self.assertEqual(result["removed_rule_signatures"], {"sample": 1})

    def test_replacing_signature_fails_even_with_same_count(self):
        self.current["scenarios"][0]["rule"][0]["opcode"] = 2
        self.assertFalse(self.compare()["passes"])

    def test_new_scenario_is_reported_as_new_measurement(self):
        new = copy.deepcopy(self.current["scenarios"][0])
        new["scenario"] = "new"
        self.current["scenarios"].append(new)
        result = self.compare()
        self.assertTrue(result["passes"])
        self.assertEqual(result["new_measurements"], {"new": ["new scenario"]})

    def test_changed_hash_is_reported_as_new_measurement(self):
        self.add_signature()
        self.current["scenarios"][0]["scenario_sha256"] = "c" * 64
        result = self.compare()
        self.assertTrue(result["passes"])
        self.assertEqual(result["new_measurements"], {"sample": ["scenario_sha256"]})

    def test_changed_harness_is_reported_as_new_measurement(self):
        self.add_signature()
        self.current["scenarios"][0]["harness_revision"] = "c" * 40
        result = self.compare()
        self.assertTrue(result["passes"])
        self.assertEqual(result["new_measurements"], {"sample": ["harness_revision"]})

    def test_label_accepts_and_keeps_added_signatures_visible(self):
        self.add_signature()
        result = self.compare(["conformance-accept-signature"])
        self.assertTrue(result["passes"])
        self.assertTrue(result["accepted_by_label"])
        self.assertEqual(len(result["added_rule_signatures"]["sample"]), 1)

    def test_label_in_baseline_cannot_accept_additions(self):
        self.add_signature()
        self.current["labels"] = ["conformance-accept-signature"]
        self.assertFalse(self.compare()["passes"])

    def test_other_label_does_not_accept_additions(self):
        self.add_signature()
        self.assertFalse(self.compare(["conformance-accept-signature-please"])["passes"])

    def test_data_addition_passes(self):
        self.current["scenarios"][0]["data"] = self.current["scenarios"][0]["rule"]
        self.assertTrue(self.compare()["passes"])

    def test_absent_character_and_null_character_are_same_signature(self):
        self.current["scenarios"][0]["rule"][0]["character"] = None
        self.assertTrue(self.compare()["passes"])

    def test_duplicate_scenario_fails(self):
        self.current["scenarios"] *= 2
        with self.assertRaisesRegex(ValueError, "unique"):
            self.compare()

    def test_removed_scenario_passes(self):
        self.current["scenarios"] = []
        result = self.compare()
        self.assertTrue(result["passes"])
        self.assertEqual(result["removed_scenarios"], ["sample"])


if __name__ == "__main__":
    unittest.main()
