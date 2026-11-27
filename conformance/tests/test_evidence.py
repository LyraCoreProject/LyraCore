import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


CONFORMANCE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONFORMANCE))
EVIDENCE = runpy.run_path(str(CONFORMANCE / "evidence"), run_name="conformance_evidence")
BASELINE = json.loads((CONFORMANCE / "tier1-baseline.json").read_text())
GIT_ENVIRONMENT = {
    "GIT_AUTHOR_NAME": "Evidence Test", "GIT_AUTHOR_EMAIL": "evidence@example.invalid",
    "GIT_COMMITTER_NAME": "Evidence Test", "GIT_COMMITTER_EMAIL": "evidence@example.invalid",
    "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
}
TOOLS = {"rustc": "1.97.1 (5ef14bb0d 2026-08-04)", "spacetime": "2.14.0"}


def summary(passed, failed):
    return (f"running {passed + failed} test\n"
            f"test result: {'FAILED' if failed else 'ok'}. {passed} passed; {failed} failed; "
            "0 ignored; 0 measured; 12 filtered out; finished in 0.01s\n")


def git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)


def write(repo, path, text):
    (repo / path).parent.mkdir(parents=True, exist_ok=True)
    (repo / path).write_text(text)


class Repository:
    """A merge-base on `main` and a pull request head that changes source and test files."""

    def __init__(self, root):
        self.root = root
        git(root, "init", "--quiet", "--initial-branch=main")
        write(root, "module/tests/conformance/tier1.rs",
              "fn bank_slot_purchase_matches_its_golden_trace() {}\n")
        write(root, "crate/Cargo.toml", "[package]\nname = \"crate\"\n")
        write(root, "crate/src/lib.rs", "pub fn answer() -> u32 { 41 }\n")
        write(root, "crate/tests/answer.rs", "\n")
        write(root, "crate/tests/obsolete.rs", "fn obsolete() {}\n")
        git(root, "add", ".")
        git(root, "commit", "--quiet", "-m", "merge-base")
        git(root, "checkout", "--quiet", "-b", "feature")
        write(root, "crate/src/lib.rs",
              "pub fn answer() -> u32 { 42 }\n#[test]\nfn answer_is_inline() {}\n")
        write(root, "crate/tests/answer.rs", "#[test]\nfn answer_is_42() {}\n")
        (root / "crate/tests/obsolete.rs").unlink()
        git(root, "add", "-A")
        git(root, "commit", "--quiet", "-m", "head")

    def metadata(self):
        return {
            "packages": [{"name": "crate", "manifest_path": str(self.root / "crate/Cargo.toml"),
                          "targets": [{"kind": ["lib"], "name": "crate"},
                                      {"kind": ["test"], "name": "answer"}]}],
            "target_directory": str(self.root / "target"),
        }


def arguments(tests=(), scenarios=()):
    return argparse.Namespace(test=[list(test) for test in tests], features=None,
                              scenario=list(scenarios), base="main", timeout=60, pr=None)


class EvidenceTestCase(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.scratch = Path(directory.name) / "scratch"
        environment = mock.patch.dict(os.environ, {
            **GIT_ENVIRONMENT,
            "LYRACORE_EVIDENCE_SCRATCH": str(self.scratch / "evidence"),
            "LYRACORE_CONFORMANCE_SCRATCH": str(self.scratch / "conformance-run"),
        })
        environment.start()
        self.addCleanup(environment.stop)
        repository = Path(directory.name) / "repo"
        repository.mkdir()
        self.repository = Repository(repository)
        self.commands = []

    def collect(self, args, run_bounded):
        def recorded(command, cwd, env, timeout, log_dir):
            self.commands.append((command, Path(cwd), env))
            return run_bounded(command, Path(cwd), env)

        with mock.patch.dict(EVIDENCE["collect"].__globals__, {
            "run_bounded": recorded,
            "workspace_packages": lambda root: self.repository.metadata(),
            "tool_versions": lambda root: TOOLS,
        }), contextlib.redirect_stderr(io.StringIO()):
            return EVIDENCE["collect"](self.repository.root, args)


class TestEvidenceTests(EvidenceTestCase):
    def test_a_new_test_that_fails_at_the_merge_base_is_shown(self):
        root = self.repository.root

        def cargo(command, cwd, env):
            if "--no-run" in command:
                return 0, ""
            if cwd == root:
                return 0, summary(1, 0)
            self.assertEqual((cwd / "crate/tests/answer.rs").read_text(),
                             "#[test]\nfn answer_is_42() {}\n")
            self.assertEqual((cwd / "crate/src/lib.rs").read_text(),
                             "pub fn answer() -> u32 { 41 }\n")
            self.assertFalse((cwd / "crate/tests/obsolete.rs").exists())
            self.assertTrue(env["CARGO_TARGET_DIR"].startswith(str(self.scratch / "evidence")))
            return 101, summary(0, 1)

        document = self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)
        self.assertEqual(document["tests"], [{
            "package": "crate", "target": "answer", "name": "answer_is_42", "features": [],
            "status": "shown", "base": "failed", "head": "passed", "reason": None,
        }])
        self.assertEqual(document["merge_base"],
                         subprocess.run(["git", "rev-parse", "main"], cwd=root, text=True,
                                        capture_output=True, check=True).stdout.strip())
        self.assertEqual(document["tools"], TOOLS)
        self.assertEqual([command[-1] for command, _, _ in self.commands if "--no-run" in command],
                         ["--no-run", "--no-run"])
        test_command = next(command for command, _, _ in self.commands if "--" in command)
        self.assertEqual(test_command[test_command.index("--"):],
                         ["--", "--exact", "answer_is_42", "--include-ignored", "--test-threads=1"])

    def test_a_reused_merge_base_checkout_carries_only_the_current_head_test_files(self):
        root = self.repository.root
        seen = []

        def cargo(command, cwd, env):
            if cwd != root and "--no-run" not in command:
                seen.append(sorted(path.relative_to(cwd).as_posix()
                                   for path in (cwd / "crate/tests").iterdir()))
            return 0, "" if "--no-run" in command else summary(int(cwd == root), int(cwd != root))

        self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)
        write(root, "crate/tests/second.rs", "#[test]\nfn second() {}\n")
        git(root, "add", "-A")
        git(root, "commit", "--quiet", "-m", "second head")
        (next((self.scratch / "evidence").glob("*/checkout")) / "stray.txt").write_text("left over")
        self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)
        git(root, "rm", "--quiet", "crate/tests/second.rs")
        git(root, "commit", "--quiet", "-m", "third head")
        self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)
        self.assertEqual(seen, [["crate/tests/answer.rs"],
                                ["crate/tests/answer.rs", "crate/tests/second.rs"],
                                ["crate/tests/answer.rs"]])
        self.assertFalse(any((self.scratch / "evidence").glob("*/checkout/stray.txt")))

    def test_a_test_in_a_changed_source_file_is_not_shown_and_never_runs_at_the_merge_base(self):
        def cargo(command, cwd, env):
            self.assertEqual(cwd, self.repository.root)
            return 0, "" if "--no-run" in command else summary(1, 0)

        document = self.collect(arguments(tests=[("crate", "lib", "answer_is_inline")]), cargo)
        [test] = document["tests"]
        self.assertEqual((test["status"], test["base"], test["head"]),
                         ("not_shown", "not_run", "passed"))
        self.assertIn("crate/src/lib.rs", test["reason"])
        self.assertFalse(any((self.scratch / "evidence").glob("*/checkout")))

    def test_a_merge_base_that_cannot_compile_the_test_is_reported_as_did_not_compile(self):
        def cargo(command, cwd, env):
            if cwd == self.repository.root:
                return 0, "" if "--no-run" in command else summary(1, 0)
            return 101, "error[E0425]: cannot find function\nerror: could not compile `crate`\n"

        document = self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)
        self.assertEqual(document["tests"][0]["base"], "did_not_compile")
        self.assertEqual(document["tests"][0]["status"], "shown")

    def test_a_test_that_passes_at_the_merge_base_is_not_shown(self):
        def cargo(command, cwd, env):
            return 0, "" if "--no-run" in command else summary(1, 0)

        document = self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)
        self.assertEqual(document["tests"][0]["status"], "not_shown")
        self.assertIn("passes at the merge-base", document["tests"][0]["reason"])

    def test_a_cargo_failure_that_is_not_a_compile_error_stops_the_run(self):
        def cargo(command, cwd, env):
            return 101, "error: failed to download from `https://index.crates.io`\n"

        with self.assertRaisesRegex(EVIDENCE["SetupFailure"], "could not build crate"):
            self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]), cargo)

    def test_a_dirty_working_tree_is_refused_before_cargo(self):
        write(self.repository.root, "crate/src/lib.rs", "pub fn answer() -> u32 { 43 }\n")
        with self.assertRaisesRegex(EVIDENCE["SetupFailure"], "dirty"):
            self.collect(arguments(tests=[("crate", "answer", "answer_is_42")]),
                         lambda *args: self.fail("cargo must not run"))

    def test_an_unknown_test_target_is_refused(self):
        with self.assertRaisesRegex(EVIDENCE["SetupFailure"], "no weather test target"):
            self.collect(arguments(tests=[("crate", "weather", "answer_is_42")]),
                         lambda *args: self.fail("cargo must not run"))


class TestOutcomeTests(unittest.TestCase):
    def test_libtest_summaries_name_the_outcome(self):
        outcome = EVIDENCE["test_outcome"]
        self.assertEqual(outcome(0, summary(1, 0)), "passed")
        self.assertEqual(outcome(101, summary(0, 1)), "failed")
        self.assertEqual(outcome(0, summary(0, 0)), "not_found")
        self.assertEqual(outcome(None, ""), "timed_out")
        self.assertEqual(outcome(101, "running 1 test\nerror: test failed (signal: 6, SIGABRT)\n"),
                         "failed")
        with self.assertRaises(EVIDENCE["SetupFailure"]):
            outcome(101, "error: no such command\n")


class ScenarioEvidenceTests(EvidenceTestCase):
    def report(self, scenario):
        entry = next(e for e in BASELINE["scenarios"] if e["scenario"] == scenario)
        differences = [
            {**signature, "classification": classification, "packet_index": index,
             "deviations": [], "reason": "differs", "expected": None, "actual": None}
            for classification in ("data", "rule")
            for index, signature in enumerate(reversed(entry[classification]))
        ]
        return {
            "schema_version": 2, "tier": "tier1", "core_commit": "b" * 40,
            "working_tree_dirty": False, "harness_revision": entry["harness_revision"],
            "recorded_at_unix_ms": 0,
            "scenarios": [{"id": scenario, "behavior": entry["behavior"],
                           "scenario_sha256": entry["scenario_sha256"],
                           "comparison": {"differences": differences}, "properties": []}],
        }

    def test_a_matching_run_has_the_digest_the_baseline_gives(self):
        def run(command, cwd, env):
            self.assertEqual(Path(command[0]).name, "run")
            result = Path(command[command.index("--result") + 1])
            result.write_text(json.dumps(self.report("quest_completion")))
            return 0, "{}"

        with mock.patch.dict(EVIDENCE["collect"].__globals__,
                             {"tier1_scenarios": lambda root: {"quest_completion"}}):
            document = self.collect(arguments(scenarios=["quest_completion"]), run)
        self.assertEqual(document["scenarios"], [{
            "id": "quest_completion", "exit_code": 0,
            "signature_digest": EVIDENCE["evidence"].baseline_signature_digest(
                BASELINE, "quest_completion"),
        }])

    def test_a_setup_failure_has_no_digest(self):
        document = self.collect(arguments(scenarios=["bank_slot_purchase"]),
                                lambda command, cwd, env: (2, '{"error": {}}'))
        self.assertEqual(document["scenarios"],
                         [{"id": "bank_slot_purchase", "exit_code": 2, "signature_digest": None}])

    def test_a_scenario_without_a_tier1_test_is_refused(self):
        with self.assertRaisesRegex(EVIDENCE["SetupFailure"], "no tier 1 test"):
            self.collect(arguments(scenarios=["taxi_single_hop"]),
                         lambda *args: self.fail("run must not start"))


class BoundedRunTests(unittest.TestCase):
    def assertStopped(self, pid):
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    def test_a_finished_run_stops_processes_it_left_behind(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "pid"
            returncode, output = EVIDENCE["run_bounded"](
                ["sh", "-c", 'sleep 300 & echo $! > "$1"; echo done', "sh", str(pid_file)],
                directory, os.environ.copy(), 30, directory)
            self.assertEqual((returncode, output), (0, "done\n"))
            self.assertStopped(int(pid_file.read_text()))

    def test_a_timed_out_run_is_stopped_and_has_no_exit_code(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / "pid"
            returncode, _ = EVIDENCE["run_bounded"](
                ["sh", "-c", 'echo $$ > "$1"; sleep 300', "sh", str(pid_file)],
                directory, os.environ.copy(), 0.5, directory)
            self.assertIsNone(returncode)
            self.assertStopped(int(pid_file.read_text()))


class PublishTests(unittest.TestCase):
    def document(self):
        return {"schema_version": 1, "merge_base": "a" * 40, "head": "b" * 40, "tests": [],
                "scenarios": [{"id": "bank_slot_purchase", "exit_code": 0,
                               "signature_digest": "c" * 64}],
                "tools": TOOLS, "platform": {"os": "linux", "arch": "x86_64"}}

    def gh(self, head):
        calls = []

        def run(command, **kwargs):
            calls.append((command, kwargs))
            if command[:3] == ["gh", "pr", "view"]:
                body = json.dumps({"body": "Closes #1384.", "headRefOid": head})
                return subprocess.CompletedProcess(command, 0, body, "")
            return subprocess.CompletedProcess(command, 0, "", "")
        return calls, run

    def test_the_section_replaces_the_body_through_gh(self):
        calls, run = self.gh("b" * 40)
        with mock.patch.object(subprocess, "run", side_effect=run):
            EVIDENCE["publish"](Path("."), 7, self.document())
        command, kwargs = calls[-1]
        self.assertEqual(command, ["gh", "pr", "edit", "7", "--body-file", "-"])
        self.assertTrue(kwargs["input"].startswith("Closes #1384.\n\n"))
        self.assertEqual(EVIDENCE["evidence"].extract(kwargs["input"]), self.document())

    def test_a_pull_request_at_another_head_is_not_edited(self):
        calls, run = self.gh("d" * 40)
        with mock.patch.object(subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(EVIDENCE["SetupFailure"], "push first"):
                EVIDENCE["publish"](Path("."), 7, self.document())
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
