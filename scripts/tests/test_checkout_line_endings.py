from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class WindowsCheckoutTests(unittest.TestCase):
    def test_autocrlf_checkout_keeps_lf_line_endings(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "LyraCore"
            subprocess.run(
                ["git", "-c", "core.autocrlf=true", "clone", "--quiet", "--no-checkout",
                 str(ROOT), str(checkout)],
                check=True,
            )
            subprocess.run(
                ["git", "-c", "core.autocrlf=true", "checkout", "--quiet", "HEAD"],
                cwd=checkout,
                check=True,
            )
            report = subprocess.run(
                ["git", "-c", "core.autocrlf=true", "ls-files", "--eol"],
                cwd=checkout,
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            crlf = [line.split("\t")[-1] for line in report.splitlines() if "w/crlf" in line]
            self.assertEqual(crlf, [])
            self.assertNotIn(b"\r", (checkout / "lyracore").read_bytes())
            self.assertNotIn(b"\r", (checkout / ".lyracore-cli-rev").read_bytes())


if __name__ == "__main__":
    unittest.main()
