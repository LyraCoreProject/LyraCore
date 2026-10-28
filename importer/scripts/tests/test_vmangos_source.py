import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "vmangos-source.sh"


class VmangosSourceTests(unittest.TestCase):
    def test_report_uses_downloaded_commit_and_overrides_require_a_pair(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "snapshot with spaces.sqlite"
            source.touch()
            report = root / ".import/vmangos/current.json"
            report.parent.mkdir(parents=True)
            commit = "1" * 40
            report.write_text(json.dumps({"sqlite_path": str(source), "commit": commit}))
            env = {k: v for k, v in os.environ.items() if k not in {"DUMP", "SOURCE_SHA"}}

            def resolve(**overrides):
                return subprocess.run(
                    ["bash", "-c", 'source "$1" && printf "%s\\n%s\\n" "$DUMP" "$SOURCE_SHA"', "bash", str(SCRIPT)],
                    cwd=root, env={**env, **overrides}, text=True, capture_output=True,
                )

            fetched = resolve()
            self.assertEqual(fetched.returncode, 0, fetched.stderr)
            self.assertEqual(fetched.stdout.splitlines(), [str(source), commit])
            self.assertNotEqual(resolve(DUMP=str(source)).returncode, 0)
            self.assertNotEqual(resolve(SOURCE_SHA=commit).returncode, 0)
            self.assertNotEqual(resolve(DUMP=str(source), SOURCE_SHA="invalid").returncode, 0)
            override = resolve(DUMP=str(source), SOURCE_SHA="2" * 40)
            self.assertEqual(override.returncode, 0, override.stderr)
            self.assertEqual(override.stdout.splitlines(), [str(source), "2" * 40])


if __name__ == "__main__":
    unittest.main()
