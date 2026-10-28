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


RUN = runpy.run_path(str(Path(__file__).resolve().parents[1] / "run"), run_name="conformance_run")


class RunFailureTests(unittest.TestCase):
    def test_incomplete_comparison_reports_test_failure_as_json(self):
        def failed_test(command, **kwargs):
            Path(kwargs["env"]["LYRACORE_CONFORMANCE_RESULT"]).write_text(
                json.dumps({"scenarios": []})
            )
            return subprocess.CompletedProcess(command, 101, "failed to start standalone\n", "")

        with tempfile.TemporaryDirectory() as scratch:
            with mock.patch.dict(os.environ, {"LYRACORE_CONFORMANCE_SCRATCH": scratch}):
                with mock.patch.object(sys, "argv", ["run", "connection_identity", "--json"]):
                    with mock.patch.object(subprocess, "run", side_effect=failed_test) as cargo:
                        stdout, stderr = io.StringIO(), io.StringIO()
                        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                            code = RUN["main"]()

        self.assertEqual(code, 2)
        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(cargo.call_count, 1)
        output = json.loads(stdout.getvalue())
        self.assertEqual(output["scenario"], "connection_identity")
        self.assertEqual(output["error"]["kind"], "setup_failed")
        self.assertIn("failed to start standalone", output["error"]["diagnostic"])


    def test_missing_cargo_reports_setup_failure_as_json(self):
        with tempfile.TemporaryDirectory() as scratch:
            with mock.patch.dict(os.environ, {"LYRACORE_CONFORMANCE_SCRATCH": scratch}):
                with mock.patch.object(sys, "argv", ["run", "connection_identity", "--json"]):
                    with mock.patch.object(subprocess, "run",
                                           side_effect=FileNotFoundError(2, "No such file", "cargo")):
                        stdout, stderr = io.StringIO(), io.StringIO()
                        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                            code = RUN["main"]()

        self.assertEqual(code, 2)
        self.assertEqual(stderr.getvalue(), "")
        output = json.loads(stdout.getvalue())
        self.assertEqual(output["error"]["kind"], "setup_failed")
        self.assertIn("could not start cargo", output["error"]["diagnostic"])


if __name__ == "__main__":
    unittest.main()
