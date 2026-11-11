from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from conformance.workspace import check_spacetime_version, harness_pin


class SpacetimeVersionTests(unittest.TestCase):
    def test_required_version_comes_from_the_module_pin(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "module").mkdir()
            (root / "module/Cargo.toml").write_text('[dependencies]\nspacetimedb = { version = "=9.8.7" }\n')
            with patch("subprocess.run", return_value=subprocess.CompletedProcess(
                    [], 0, "spacetimedb tool version 9.8.7; spacetimedb-lib version 9.8.7;", "")):
                check_spacetime_version(root, executable="spacetime")

    def test_unavailable_or_unrecognized_cli_is_a_setup_failure(self):
        root = Path(__file__).resolve().parents[2]
        for response in [
            FileNotFoundError("missing CLI"),
            subprocess.TimeoutExpired("spacetime", 10),
            subprocess.CompletedProcess([], 0, "unrecognized version", ""),
            subprocess.CompletedProcess([], 1, "", "version command failed"),
        ]:
            with self.subTest(response=response):
                options = ({"side_effect": response} if isinstance(response, Exception)
                           else {"return_value": response})
                with patch("subprocess.run", **options):
                    with self.assertRaisesRegex(RuntimeError, "required .*version install"):
                        check_spacetime_version(root)


class HarnessPinTests(unittest.TestCase):
    def test_pin_skips_comment_and_blank_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".wire-harness-rev").write_text(
                "# Read the notes first.\n\nv0.2.0-conformance.9 " + "b" * 40 + "\n")
            self.assertEqual(harness_pin(root), ("v0.2.0-conformance.9", "b" * 40))
