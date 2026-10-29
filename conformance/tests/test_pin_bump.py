import importlib.util
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
