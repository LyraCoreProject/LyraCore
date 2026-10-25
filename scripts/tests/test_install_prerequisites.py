import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[2] / "install.sh"

# Keep these banners in step with proc/openssl.rs in lyracore-cli.
ACCEPTED_OPENSSL = (
    "OpenSSL 3.0.0 7 Sep 2021",
    "OpenSSL 3.6.5",
    "OpenSSL 4.0.0",
    "OpenSSL 3.6.0-dev",
    "OpenSSL 3.1.5+quic",
    "OpenSSL 3.2.0-beta1 (Library: OpenSSL 3.2.0-beta1)",
)
REFUSED_OPENSSL = (
    None,
    "LibreSSL 3.3.6",
    "OpenSSL 1.1.1w",
    "OpenSSL 2.9.9",
    "OpenSSL 3.bad",
    "OpenSSL 3",
    "OpenSSL 3.0",
    "OpenSSL 3.0.0.1",
    "OpenSSL 3..0",
    "OpenSSL 3.0.",
    "OpenSSL 3.0.x",
    "OpenSSL +3.0.0",
    "OpenSSL 3.0.0-",
    "OpenSSL 3.0.0+",
    "OpenSSL 3.0.0-dev!",
    "OpenSSL 3.0.0-d\u00e9v",
    "OpenSSL 3.0-dev",
    "OpenSSL",
    "unknown",
    "",
)


class InstallerPrerequisiteTests(unittest.TestCase):
    def run_installer(self, banner, platform="Linux", openssl_status=0, compiler=True,
                      python=True):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tools = root / "tools"
            tools.mkdir()
            home = root / "home"
            home.mkdir()
            spacetime_calls = root / "spacetime-calls"
            commands = {
                "uname": f'case "$1" in -s) echo {platform};; *) echo x86_64;; esac',
                "git": 'echo "TEST: clone reached"; exit 1',
                "curl": 'echo "TEST: unexpected download"; exit 99',
                **({"cc": "exit 0"} if compiler else {}),
                **({"python3": "exit 0"} if python else {}),
                "pkg-config": "exit 0",
                "apt-get": "exit 99",
                "cargo": "echo 'cargo 1.93.0'",
                "spacetime": (
                    'case "$1" in --version) echo "spacetimedb tool version 2.7.1";; '
                    f'*) echo "$*" >> {shlex.quote(str(spacetime_calls))}; exit 98;; esac'
                ),
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
                env={**os.environ, "HOME": str(home), "PATH": str(tools)},
                text=True,
                capture_output=True,
                timeout=10,
            )
            self.assertNotIn("TEST: unexpected download", result.stdout + result.stderr)
            self.assertFalse((root / "LyraCore").exists())
            self.assertFalse(spacetime_calls.exists())
            return result

    def test_missing_or_unsupported_openssl_stops_before_clone(self):
        for banner in REFUSED_OPENSSL:
            with self.subTest(banner=banner):
                result = self.run_installer(banner)
                self.assertEqual(result.returncode, 1)
                self.assertIn("OpenSSL 3 or newer command", result.stderr)
                self.assertIn("sudo apt-get install -y openssl", result.stderr)
                self.assertNotIn("TEST: clone reached", result.stdout)

    def test_supported_openssl_passes_prerequisites(self):
        for platform in ("Linux", "Darwin"):
            for banner in ACCEPTED_OPENSSL:
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

    def test_macos_names_xcode_only_when_the_compiler_is_missing(self):
        for compiler in (True, False):
            with self.subTest(compiler=compiler):
                result = self.run_installer("OpenSSL 1.1.1w", "Darwin", compiler=compiler)
                self.assertIn("brew install openssl@3", result.stderr)
                self.assertEqual("xcode-select --install" in result.stderr, not compiler)

    def test_macos_libressl_has_homebrew_and_path_instructions(self):
        result = self.run_installer("LibreSSL 3.3.6", "Darwin")
        self.assertIn("brew install openssl@3", result.stderr)
        self.assertIn('export PATH="$(brew --prefix openssl@3)/bin:$PATH"', result.stderr)
        self.assertIn("LibreSSL is not supported", result.stderr)
        self.assertNotIn("TEST: clone reached", result.stdout)

    def test_missing_python_stops_before_clone(self):
        for platform, install in (("Linux", "sudo apt-get install -y python3"),
                                  ("Darwin", "brew install python3")):
            with self.subTest(platform=platform):
                result = self.run_installer("OpenSSL 3.6.5", platform, python=False)
                self.assertEqual(result.returncode, 1)
                self.assertIn("missing prerequisite(s): python3", result.stderr)
                self.assertIn(install, result.stderr)
                self.assertNotIn("TEST: clone reached", result.stdout)


if __name__ == "__main__":
    unittest.main()
