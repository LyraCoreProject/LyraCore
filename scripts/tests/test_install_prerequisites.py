import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[2] / "install.sh"


class InstallerPrerequisiteTests(unittest.TestCase):
    def run_installer(self, banner, platform="Linux", openssl_status=0):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tools = root / "tools"
            tools.mkdir()
            commands = {
                "uname": f'case "$1" in -s) echo {platform};; *) echo x86_64;; esac',
                "git": 'echo "TEST: clone reached"; exit 1',
                "curl": 'echo "TEST: unexpected download"; exit 99',
                "cc": "exit 0",
                "pkg-config": "exit 0",
                "apt-get": "exit 99",
                "cargo": "echo 'cargo 1.93.0'",
                "spacetime": "echo 'spacetimedb tool version 2.7.1'",
                "wasm-opt": "echo 'wasm-opt version 131'",
            }
            if banner is not None:
                commands["openssl"] = (
                    f"printf '%s\\n' {shlex.quote(banner)}; exit {openssl_status}"
                )
            for name, body in commands.items():
                path = tools / name
                path.write_text("#!/bin/sh\n" + body + "\n")
                path.chmod(0o755)
            for name in ("sed", "head"):
                (tools / name).symlink_to(shutil.which(name))
            result = subprocess.run(
                ["/bin/sh", str(INSTALLER), "--yes"],
                cwd=root,
                env={**os.environ, "PATH": str(tools)},
                text=True,
                capture_output=True,
                timeout=10,
            )
            self.assertNotIn("TEST: unexpected download", result.stdout + result.stderr)
            self.assertFalse((root / "LyraCore").exists())
            return result

    def test_missing_or_unsupported_openssl_stops_before_clone(self):
        for banner in (None, "LibreSSL 3.3.6", "OpenSSL 1.1.1w", "OpenSSL 3.bad", "unknown"):
            with self.subTest(banner=banner):
                result = self.run_installer(banner)
                self.assertEqual(result.returncode, 1)
                self.assertIn("OpenSSL 3 or newer command", result.stderr)
                self.assertIn("sudo apt-get install -y openssl", result.stderr)
                self.assertNotIn("TEST: clone reached", result.stdout)

    def test_supported_openssl_passes_prerequisites(self):
        for platform in ("Linux", "Darwin"):
            for banner in ("OpenSSL 3.0.0 7 Sep 2021", "OpenSSL 3.6.5", "OpenSSL 4.0.0"):
                with self.subTest(platform=platform, banner=banner):
                    result = self.run_installer(banner, platform)
                    self.assertIn("system packages", result.stdout)
                    self.assertIn("TEST: clone reached", result.stdout)
                    self.assertIn("git clone failed", result.stderr)
                    self.assertNotIn("missing prerequisite", result.stderr)

    def test_failed_version_command_cannot_pass_on_its_banner(self):
        result = self.run_installer("OpenSSL 3.6.5", openssl_status=1)
        self.assertIn("OpenSSL 3 or newer command", result.stderr)
        self.assertNotIn("TEST: clone reached", result.stdout)

    def test_macos_libressl_has_homebrew_and_path_instructions(self):
        result = self.run_installer("LibreSSL 3.3.6", "Darwin")
        self.assertIn("brew install openssl@3", result.stderr)
        self.assertIn('export PATH="$(brew --prefix openssl@3)/bin:$PATH"', result.stderr)
        self.assertIn("LibreSSL is not supported", result.stderr)
        self.assertNotIn("TEST: clone reached", result.stdout)


if __name__ == "__main__":
    unittest.main()
