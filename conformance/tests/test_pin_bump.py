import importlib.util
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "pin_bump.py"
SPEC = importlib.util.spec_from_file_location("pin_bump", SCRIPT)
PIN_BUMP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PIN_BUMP)


class PinBumpTests(unittest.TestCase):
    def test_catalogue_skips_new_scenarios_without_core_evidence(self):
        scratch = Path.home() / ".cache/lyracore-scratch/conformance-pin-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            harness = root / "harness"
            (harness / "src").mkdir(parents=True)
            (harness / "src/catalogue.rs").write_text(
                '    Entry { id: "existing", },\n    Entry { id: "new_scenario", },\n'
            )
            tests = root / "module/tests/conformance/tier1.rs"
            tests.parent.mkdir(parents=True)
            tests.write_text('run_scenario("existing", |_, _| Ok(()))\n')
            ledger = root / "conformance/features/system/existing.toml"
            ledger.parent.mkdir(parents=True)
            ledger.write_text('scenarios = ["existing"]\n')
            metadata = {"packages": [{
                "name": "wire-client", "manifest_path": str(harness / "Cargo.toml"),
                "source": "git+https://github.com/LyraCoreProject/wire-harness?rev=abc",
            }]}
            with mock.patch.object(PIN_BUMP, "ROOT", root):
                with mock.patch.object(PIN_BUMP.subprocess, "check_output",
                                       return_value=json.dumps(metadata)):
                    self.assertEqual(PIN_BUMP.missing_scenarios(), ["new_scenario"])

    def test_publish_accepts_only_baseline_provenance_and_signatures(self):
        before = json.loads((SCRIPT.parent / "tier1-baseline.json").read_text())
        after = copy.deepcopy(before)
        revision = "b" * 40
        for entry in after["scenarios"]:
            entry["harness_revision"] = revision
        PIN_BUMP.validate_baseline(before, after, revision)
        after["scenarios"][0]["behavior"] = "another_behavior"
        with self.assertRaisesRegex(ValueError, "Behavior"):
            PIN_BUMP.validate_baseline(before, after, revision)
        after["scenarios"][0]["behavior"] = before["scenarios"][0]["behavior"]
        after["scenarios"][0]["extra"] = "untrusted"
        with self.assertRaisesRegex(ValueError, "scenario entry"):
            PIN_BUMP.validate_baseline(before, after, revision)

    def test_summary_is_derived_from_the_baseline(self):
        before = json.loads((SCRIPT.parent / "tier1-baseline.json").read_text())
        after = copy.deepcopy(before)
        after["scenarios"][0]["rule"] = [{
            "rule": "missing_reply", "checkpoint": "opening", "stream": None,
            "opcode": None, "field_path": [],
        }]
        summary = PIN_BUMP.summary_data(before, after, "v0.2.0-conformance.9", "b" * 40)
        self.assertTrue(summary["human_review"])
        self.assertEqual(summary["changes"][0]["added"], after["scenarios"][0]["rule"])

    def test_publish_rejects_a_summary_that_disagrees_with_the_baseline(self):
        scratch = Path.home() / ".cache/lyracore-scratch/conformance-pin-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            (root / "conformance").mkdir()
            before = json.loads((SCRIPT.parent / "tier1-baseline.json").read_text())
            (root / "conformance/tier1-baseline.json").write_text(
                json.dumps(before, indent=2) + "\n"
            )
            after = copy.deepcopy(before)
            revision = "b" * 40
            for entry in after["scenarios"]:
                entry["harness_revision"] = revision
            baseline = root / "artifact-baseline.json"
            baseline.write_text(json.dumps(after, indent=2) + "\n")
            summary = PIN_BUMP.summary_data(before, after, "v0.2.0-conformance.9", revision)
            summary["human_review"] = True
            summary_path = root / "summary.json"
            summary_path.write_text(json.dumps(summary))
            with mock.patch.object(PIN_BUMP, "ROOT", root):
                with mock.patch.object(PIN_BUMP, "remote_tag",
                                       return_value=(summary["tag"], revision)):
                    with mock.patch.object(PIN_BUMP, "apply_pin") as apply_pin:
                        with self.assertRaisesRegex(ValueError, "summary disagrees"):
                            PIN_BUMP.publish(baseline, summary_path, root / "body.md", root / "output")
                        apply_pin.assert_not_called()

    def test_latest_tag_uses_annotated_tag_commit_and_numeric_order(self):
        refs = (
            "a" * 40 + "\trefs/tags/v0.2.0-conformance.9\n"
            + "b" * 40 + "\trefs/tags/v0.2.0-conformance.9^{}\n"
            + "c" * 40 + "\trefs/tags/v0.2.0-conformance.10\n"
        )
        self.assertEqual(PIN_BUMP.latest_tag(refs), ["v0.2.0-conformance.10", "c" * 40])
        self.assertEqual(
            PIN_BUMP.latest_tag(refs.splitlines()[0] + "\n" + refs.splitlines()[1]),
            ["v0.2.0-conformance.9", "b" * 40],
        )

    def test_prepare_changes_both_pins_once_and_is_idempotent(self):
        scratch = Path.home() / ".cache/lyracore-scratch/conformance-pin-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            pin = root / ".wire-harness-rev"
            manifest = root / "Cargo.toml"
            output = root / "output"
            pin.write_text("# keep\nv0.2.0-conformance.8 " + "a" * 40 + "\n")
            manifest.write_text(
                'wire-client = { git = "https://github.com/LyraCoreProject/wire-harness", rev = "'
                + "a" * 40 + '" }\n'
            )
            refs = "b" * 40 + "\trefs/tags/v0.2.0-conformance.9\n"
            with mock.patch.object(PIN_BUMP, "ROOT", root):
                with mock.patch.object(PIN_BUMP.subprocess, "check_output", return_value=refs):
                    PIN_BUMP.prepare(output)
                    first_pin = pin.read_text()
                    first_manifest = manifest.read_text()
                    PIN_BUMP.prepare(output)
            self.assertIn("v0.2.0-conformance.9 " + "b" * 40, first_pin)
            self.assertIn('rev = "' + "b" * 40 + '"', first_manifest)
            self.assertEqual(pin.read_text(), first_pin)
            self.assertEqual(manifest.read_text(), first_manifest)
            self.assertIn("new=true\n", output.read_text())
            self.assertIn("new=false\n", output.read_text())

    def test_signature_changes_reports_added_and_removed_per_scenario(self):
        first = {"rule": "packet", "checkpoint": "entry"}
        second = {"rule": "timing", "checkpoint": "entry"}
        before = {"scenarios": [{"scenario": "one", "rule": [first], "data": []}]}
        after = {"scenarios": [{"scenario": "one", "rule": [second], "data": []}]}
        self.assertEqual(
            PIN_BUMP.signature_changes(before, after),
            [("one", "rule", [json.dumps(second, sort_keys=True, separators=(",", ":"))],
              [json.dumps(first, sort_keys=True, separators=(",", ":"))])],
        )


if __name__ == "__main__":
    unittest.main()
