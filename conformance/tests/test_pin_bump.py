import importlib.util
import base64
import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "pin_bump.py"
SPEC = importlib.util.spec_from_file_location("pin_bump", SCRIPT)
PIN_BUMP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PIN_BUMP)
SCRATCH = Path(os.environ.get("LYRACORE_PIN_TEST_SCRATCH",
                              Path.home() / ".cache/lyracore-scratch/conformance-pin-tests"))


class PinBumpTests(unittest.TestCase):
    def test_catalogue_skips_new_scenarios_without_core_evidence(self):
        scratch = SCRATCH
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
        del after["scenarios"][0]["extra"]
        after["schema_version"] = True
        with self.assertRaisesRegex(ValueError, "baseline schema"):
            PIN_BUMP.validate_baseline(before, after, revision)

    def test_publish_accepts_empty_checkpoint_but_requires_rule_name(self):
        before = json.loads((SCRIPT.parent / "tier1-baseline.json").read_text())
        after = copy.deepcopy(before)
        revision = "b" * 40
        for entry in after["scenarios"]:
            entry["harness_revision"] = revision
        signature = {"rule": "unmatched_declaration", "checkpoint": "", "stream": None,
                     "opcode": None, "field_path": []}
        after["scenarios"][0]["rule"] = [signature]
        PIN_BUMP.validate_baseline(before, after, revision)
        signature["rule"] = ""
        with self.assertRaisesRegex(ValueError, "signature names"):
            PIN_BUMP.validate_baseline(before, after, revision)
        signature["rule"] = "unmatched_declaration"
        signature["checkpoint"] = None
        with self.assertRaisesRegex(ValueError, "signature names"):
            PIN_BUMP.validate_baseline(before, after, revision)

    def test_publish_validates_named_character_signatures(self):
        before = json.loads((SCRIPT.parent / "tier1-baseline.json").read_text())
        after = copy.deepcopy(before)
        revision = "b" * 40
        for entry in after["scenarios"]:
            entry["harness_revision"] = revision
        PIN_BUMP.validate_baseline(before, after, revision)
        signature = after["scenarios"][-1]["rule"][1]
        signature["character"] = ""
        with self.assertRaisesRegex(ValueError, "signature Character"):
            PIN_BUMP.validate_baseline(before, after, revision)

    def test_publish_validates_job_output_before_writing(self):
        scratch = SCRATCH
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            (root / "conformance").mkdir()
            before = json.loads((SCRIPT.parent / "tier1-baseline.json").read_text())
            baseline = root / "conformance/tier1-baseline.json"
            baseline.write_text(json.dumps(before, indent=2) + "\n")
            after = copy.deepcopy(before)
            revision = "b" * 40
            for entry in after["scenarios"]:
                entry["harness_revision"] = revision
            encoded = base64.b64encode((json.dumps(after, indent=2) + "\n").encode()).decode()
            with mock.patch.object(PIN_BUMP, "ROOT", root):
                with mock.patch.object(PIN_BUMP, "remote_tag",
                                       return_value=("v0.2.0-conformance.9", revision)):
                    with mock.patch.object(PIN_BUMP, "apply_pin") as apply_pin:
                        with self.assertRaisesRegex(ValueError, "latest conformance tag"):
                            PIN_BUMP.publish(encoded, "c" * 40, root / "body.md", root / "output")
                        duplicate = base64.b64encode(b'{"schema_version":1,"schema_version":1}').decode()
                        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
                            PIN_BUMP.publish(duplicate, revision, root / "body.md", root / "output")
                        with self.assertRaisesRegex(ValueError, "job output limit"):
                            PIN_BUMP.publish("a" * (4 * ((PIN_BUMP.MAX_BASELINE_BYTES + 2) // 3) + 1),
                                             revision, root / "body.md", root / "output")
                        with self.assertRaisesRegex(ValueError, "encoded Tier 1 baseline"):
                            PIN_BUMP.publish("../../../", revision, root / "body.md", root / "output")
                        apply_pin.assert_not_called()
                    with mock.patch.object(PIN_BUMP, "describe"):
                        with mock.patch.object(PIN_BUMP, "apply_pin", return_value=True):
                            PIN_BUMP.publish(encoded, revision, root / "body.md", root / "output")
            self.assertEqual(baseline.read_text(), json.dumps(after, indent=2) + "\n")

    def test_proposal_exports_only_revision_and_bounded_baseline(self):
        scratch = SCRATCH
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            (root / "conformance").mkdir()
            (root / ".wire-harness-rev").write_text("v0.2.0-conformance.9 " + "b" * 40 + "\n")
            baseline = b'{"schema_version": 1, "scenarios": []}\n'
            (root / "conformance/tier1-baseline.json").write_bytes(baseline)
            with mock.patch.object(PIN_BUMP, "ROOT", root):
                PIN_BUMP.proposal(root / "output")
                (root / "conformance/tier1-baseline.json").write_bytes(
                    b" " * (PIN_BUMP.MAX_BASELINE_BYTES + 1)
                )
                with self.assertRaisesRegex(ValueError, "job output limit"):
                    PIN_BUMP.proposal(root / "output")
            self.assertEqual((root / "output").read_text(),
                             "sha=" + "b" * 40 + "\nbaseline="
                             + base64.b64encode(baseline).decode() + "\n")

    def test_checkout_updates_existing_bot_branch_with_matching_pin(self):
        scratch = SCRATCH
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory) / "core"
            remote = Path(directory) / "origin.git"
            saved = Path(directory) / "saved"
            root.mkdir()
            saved.mkdir()
            subprocess.run(["git", "init", "--bare", "--initial-branch=main", str(remote)],
                           check=True, capture_output=True)
            subprocess.run(["git", "init", "--initial-branch=main", str(root)],
                           check=True, capture_output=True)
            env = dict(os.environ, GIT_AUTHOR_NAME="Pin test", GIT_AUTHOR_EMAIL="pin@test.invalid",
                       GIT_COMMITTER_NAME="Pin test", GIT_COMMITTER_EMAIL="pin@test.invalid")

            def git(*args):
                return subprocess.run(["git", *args], cwd=root, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("remote", "add", "origin", str(remote))
            for name in PIN_BUMP.PROPOSAL_FILES:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("v0.2.0-conformance.8\n")
            git("add", ".")
            git("commit", "-m", "main")
            git("push", "origin", "main")

            def stage_proposal(lock, tag):
                for name in PIN_BUMP.PROPOSAL_FILES:
                    content = lock if name == "Cargo.lock" else f"v0.2.0-conformance.{tag}\n"
                    path = saved / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                    (root / name).write_text(content)

            with mock.patch.object(PIN_BUMP, "ROOT", root), mock.patch.dict(os.environ, env):
                stage_proposal("first\n", 9)
                PIN_BUMP.checkout_proposal(saved)
                self.assertEqual(git("branch", "--show-current"), "pin-proposal")
                self.assertEqual((root / "Cargo.lock").read_text(), "first\n")
                git("add", ".")
                git("commit", "-m", "first proposal")
                git("push", "origin", "HEAD:refs/heads/ci/auto-harness-pin")
                git("switch", "main")
                git("branch", "-D", "pin-proposal")
                for name in PIN_BUMP.PROPOSAL_FILES:
                    (root / name).write_text("v0.2.0-conformance.10\n")
                git("add", ".")
                git("commit", "-m", "combined main update")
                git("push", "origin", "main")
                stage_proposal("second\n", 11)
                PIN_BUMP.checkout_proposal(saved)
                self.assertEqual((root / ".wire-harness-rev").read_text(),
                                 "v0.2.0-conformance.11\n")
                self.assertEqual((root / "Cargo.lock").read_text(), "second\n")
                self.assertEqual(git("branch", "--show-current"), "pin-proposal")
                self.assertEqual(git("rev-list", "--parents", "-n", "1", "HEAD").count(" "), 2)
                self.assertEqual(git("diff", "--cached", "--name-only"), "")
                git("push", "origin", "HEAD:refs/heads/ci/auto-harness-pin")
                self.assertEqual(git("rev-parse", "HEAD"),
                                 git("ls-remote", "origin", "refs/heads/ci/auto-harness-pin").split()[0])
                git("switch", "main")
                git("branch", "-D", "pin-proposal")
                (root / "unrelated").write_text("new main content\n")
                git("add", "unrelated")
                git("commit", "-m", "unrelated main update")
                git("push", "origin", "main")
                stage_proposal("second\n", 11)
                PIN_BUMP.checkout_proposal(saved)
                self.assertEqual(git("diff", "--name-only"), "")
                self.assertEqual(git("diff", "--cached", "--name-only"), "")
                self.assertNotEqual(git("rev-parse", "HEAD"),
                                    git("rev-parse", "origin/ci/auto-harness-pin"))
                branch = "ci/auto-harness-pin"
                subprocess.run(
                    ["bash", "-euo", "pipefail", "-c",
                     'if ! git show-ref --verify --quiet "refs/remotes/origin/$branch" || '
                     '[ "$(git rev-parse HEAD)" != "$(git rev-parse "origin/$branch")" ]; then '
                     'git push origin "HEAD:refs/heads/$branch"; fi'],
                    cwd=root, env=dict(env, branch=branch), check=True, capture_output=True,
                )
                self.assertEqual(git("rev-parse", "HEAD"),
                                 git("ls-remote", "origin", f"refs/heads/{branch}").split()[0])
                (root / "unrelated").write_text("bot edit\n")
                git("add", "unrelated")
                git("commit", "-m", "bot edit outside proposal")
                git("push", "origin", "HEAD:refs/heads/ci/auto-harness-pin")
                git("switch", "main")
                git("branch", "-D", "pin-proposal")
                (root / "unrelated").write_text("main edit\n")
                git("add", "unrelated")
                git("commit", "-m", "main edit outside proposal")
                git("push", "origin", "main")
                stage_proposal("second\n", 11)
                with self.assertRaisesRegex(RuntimeError, "unrelated"):
                    PIN_BUMP.checkout_proposal(saved)
                self.assertEqual(git("status", "--porcelain"), "")

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
        scratch = SCRATCH
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
