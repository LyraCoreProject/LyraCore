import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-gateway-bindings.py"
ROOT = SCRIPT.parent.parent
COMMITTED = ROOT / "gateway/src/stdb/bindings"
FIXTURE_FILE = Path("advance_taxi_flight_reducer.rs")


class BindingCheckTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.generated = Path(self.temporary.name) / "generated"
        shutil.copytree(COMMITTED, self.generated)

    def tearDown(self):
        self.temporary.cleanup()

    def run_check(self):
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--generated-dir",
                str(self.generated),
                "--bindings-dir",
                str(COMMITTED),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def test_omitted_generated_definition_fails_the_check(self):
        (self.generated / FIXTURE_FILE).unlink()

        result = self.run_check()

        self.assertNotEqual(0, result.returncode)
        self.assertIn(f"Missing generated binding: {FIXTURE_FILE}", result.stderr)

    def test_modified_generated_definition_fails_the_check(self):
        with (self.generated / FIXTURE_FILE).open("a") as fixture:
            fixture.write("\n// Drift fixture.\n")

        result = self.run_check()

        self.assertNotEqual(0, result.returncode)
        self.assertIn(f"Changed generated binding: {FIXTURE_FILE}", result.stderr)


if __name__ == "__main__":
    unittest.main()
