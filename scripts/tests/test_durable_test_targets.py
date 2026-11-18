import importlib.util
import json
import re
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/durable-test-targets.py"
WORKFLOW = ROOT / ".github/workflows/module-durable.yml"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


selector = load("durable_test_targets", SCRIPT)
refresher = load("durable_test_seconds", ROOT / "scripts/durable-test-seconds.py")
JOB_SECONDS = json.loads(selector.WEIGHTS.read_text())["job_seconds"]


class DurableTargetTests(unittest.TestCase):
    def test_ci_selects_every_current_target_once(self):
        selected = []
        for job in range(len(JOB_SECONDS)):
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(job)],
                check=True,
                capture_output=True,
                text=True,
            )
            targets = result.stdout.splitlines()
            self.assertTrue(targets)
            selected.extend(targets)
        expected = [path.stem for path in (ROOT / "module/tests").glob("*.rs")]
        self.assertCountEqual(selected, expected)

    def test_the_workflow_runs_one_job_per_shard_the_script_assigns(self):
        workflow = WORKFLOW.read_text()
        matrix = re.search(r"shard: \[([\d, ]+)\]", workflow).group(1)
        self.assertEqual(
            [int(shard) for shard in matrix.split(",")], list(range(len(JOB_SECONDS)))
        )
        self.assertIn(f"shard ${{{{ matrix.shard }}}} of {len(JOB_SECONDS)}", workflow)

    def test_every_step_pinned_to_a_shard_names_a_shard_that_exists(self):
        pinned = {int(shard) for shard in re.findall(r"matrix\.shard == (\d+)", WORKFLOW.read_text())}
        self.assertTrue(pinned)
        self.assertLess(max(pinned), len(JOB_SECONDS))

    def test_the_workflow_runs_the_gate_after_every_other_target(self):
        workflow = WORKFLOW.read_text()
        self.assertLess(
            workflow.index('cargo test -p lyracore-module "${args[@]}" -- '),
            workflow.index('cargo test -p lyracore-module "${gate[@]}" -- '),
        )

    def test_new_targets_are_included_and_deleted_targets_are_not(self):
        jobs, _ = selector.partition(
            ["new", "existing"], {"existing": 30, "deleted": 600}, (0, 0)
        )
        self.assertCountEqual([target for job in jobs for target in job], ["new", "existing"])

    def test_assignment_does_not_depend_on_directory_order(self):
        targets = ["a", "b", "c", "d"]
        self.assertEqual(
            selector.partition(targets, {}, (0, 0, 0)),
            selector.partition(reversed(targets), {}, (0, 0, 0)),
        )

    def test_the_longest_target_goes_to_the_least_loaded_job_counting_its_other_work(self):
        jobs, totals = selector.partition(
            ["c", "a", "b"], {"a": 50, "b": 40, "c": 30}, (0, 0, 82)
        )
        self.assertEqual(jobs, [["a"], ["b", "c"], []])
        self.assertEqual(totals, [50, 70, 82])


SUITE_LOG = """\
2026-11-18T10:00:00.0000000Z echo "targets: a c b"
2026-11-18T10:00:01.0000000Z targets: a c b
2026-11-18T10:00:11.0000000Z      Running tests/a.rs (target/debug/deps/a-1)
2026-11-18T10:00:21.0000000Z      Running tests/c.rs (target/debug/deps/c-1)
2026-11-18T10:03:21.0000000Z      Running tests/b.rs (target/debug/deps/b-1)
2026-11-18T10:05:21.0000000Z test result: ok. 1 passed
2026-11-18T10:05:22.0000000Z ##[group]Run actions/cache@v6
2026-11-18T10:09:00.0000000Z      Running tests/a.rs (target/debug/deps/a-1)
"""


class RefreshTests(unittest.TestCase):
    def test_a_job_extra_counts_only_the_steps_some_jobs_run(self):
        def step(name, start, end):
            return {"name": name, "startedAt": f"2026-11-18T10:{start}Z", "completedAt": f"2026-11-18T10:{end}Z"}

        job = {
            "steps": [
                step("Build the module wasm", "00:00", "01:30"),
                step("Check populated items reimports", "01:30", "02:00"),
                step("Produce the conformance scoreboard", "02:00", "02:20"),
                step("Verify the populated Module upgrade", "02:20", "03:00"),
            ],
        }
        self.assertEqual(refresher.job_extra_seconds(job), 70)

    def test_every_step_the_workflow_pins_to_a_shard_is_counted_or_a_cache_miss_step(self):
        pinned = re.findall(r"- name: (.+)\n(?:\s+id: .+\n)?\s+if: matrix\.shard == \d+", WORKFLOW.read_text())
        cache_miss = {"Check out the populated-upgrade baseline", "Build the populated-upgrade baseline"}
        self.assertTrue(pinned)
        self.assertLessEqual(set(pinned), refresher.SHARD_STEPS | cache_miss)

    def test_a_log_without_a_targets_line_is_refused(self):
        with self.assertRaisesRegex(ValueError, "targets"):
            refresher.suite_target_seconds("2026-11-18T10:00:00.0000000Z nothing here")

    def test_a_target_runs_until_the_next_one_starts_and_the_last_until_its_step_ends(self):
        seconds = refresher.suite_target_seconds(SUITE_LOG)
        self.assertEqual([seconds["a"], seconds["c"], seconds["b"]], [10, 180, 120])


if __name__ == "__main__":
    unittest.main()
