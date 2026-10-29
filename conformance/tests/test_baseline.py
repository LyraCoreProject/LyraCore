import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


BASELINE = runpy.run_path(
    str(Path(__file__).resolve().parents[1] / "baseline"), run_name="conformance_baseline"
)
COMMITTED = Path(__file__).resolve().parents[1] / "tier1-baseline.json"


class BaselineFailureTests(unittest.TestCase):
    def test_no_argument_form_uses_full_regeneration(self):
        with mock.patch.object(sys, "argv", ["baseline"]):
            with mock.patch.object(os, "chdir"):
                with mock.patch.object(os, "execvp", side_effect=RuntimeError("exec")) as execvp:
                    with self.assertRaisesRegex(RuntimeError, "exec"):
                        BASELINE["main"]()
        self.assertEqual(execvp.call_args.args[0], "cargo")
        self.assertEqual(execvp.call_args.args[1][-1], "regenerate")

    def test_setup_failure_does_not_run_generator_or_write_baseline(self):
        original = COMMITTED.read_bytes()
        with tempfile.TemporaryDirectory() as scratch:
            with mock.patch.dict(os.environ, {"LYRACORE_CONFORMANCE_SCRATCH": scratch}):
                with mock.patch.object(sys, "argv", ["baseline", "taxi_single_hop"]):
                    with mock.patch.object(
                        subprocess, "run", return_value=subprocess.CompletedProcess([], 2)
                    ) as run:
                        self.assertEqual(BASELINE["main"](), 2)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(COMMITTED.read_bytes(), original)

    def test_missing_comparison_does_not_run_generator_or_write_baseline(self):
        original = COMMITTED.read_bytes()
        with tempfile.TemporaryDirectory() as scratch:
            with mock.patch.dict(os.environ, {"LYRACORE_CONFORMANCE_SCRATCH": scratch}):
                with mock.patch.object(sys, "argv", ["baseline", "taxi_single_hop"]):
                    with mock.patch.object(
                        subprocess, "run", return_value=subprocess.CompletedProcess([], 1)
                    ) as run:
                        self.assertEqual(BASELINE["main"](), 2)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(COMMITTED.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
