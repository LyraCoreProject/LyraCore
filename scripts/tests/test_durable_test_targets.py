import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/durable-test-targets.py"
spec = importlib.util.spec_from_file_location("durable_test_targets", SCRIPT)
selector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(selector)


class DurableTargetTests(unittest.TestCase):
    def test_ci_selects_every_current_target_once(self):
        selected = []
        for job in range(3):
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

    def test_new_targets_are_included_and_deleted_targets_are_not(self):
        jobs, _ = selector.partition(
            ["new", "existing"], {"existing": 30, "deleted": 600}
        )
        self.assertCountEqual([target for job in jobs for target in job], ["new", "existing"])

    def test_assignment_does_not_depend_on_directory_order(self):
        targets = ["a", "b", "c", "d"]
        self.assertEqual(
            selector.partition(targets, {}), selector.partition(reversed(targets), {})
        )

    def test_measured_work_is_balanced_including_gateway_checks(self):
        seconds = json.loads((ROOT / ".github/durable-test-seconds.json").read_text())
        targets = sorted(path.stem for path in (ROOT / "module/tests").glob("*.rs"))
        _, totals = selector.partition(targets, seconds)
        previous = list(selector.JOB_SECONDS)
        for position, target in enumerate(targets, 1):
            previous[position % 3] += seconds.get(target, selector.DEFAULT_TARGET_SECONDS)
        self.assertLess(max(totals), max(previous))
        self.assertLessEqual(max(totals) - min(totals), selector.DEFAULT_TARGET_SECONDS)


if __name__ == "__main__":
    unittest.main()
