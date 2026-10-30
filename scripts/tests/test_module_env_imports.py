import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/module-env-imports.py"
spec = importlib.util.spec_from_file_location("module_env_imports", SCRIPT)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


def wasm(*entries):
    def text(value):
        return bytes([len(value)]) + value.encode()

    body = bytes([len(entries)]) + b"".join(
        text(module) + text(field) + kind for module, field, kind in entries
    )
    custom = b"\x00\x03" + text("hi")
    return b"\0asm\x01\0\0\0" + custom + bytes([2, len(body)]) + body


FUNCTION = b"\x00\x00"
MEMORY_WITH_MAX = b"\x02\x01\x01\x02"
GLOBAL = b"\x03\x7f\x00"


class ModuleEnvImportTests(unittest.TestCase):
    def test_lists_imports_of_every_kind(self):
        module = wasm(
            ("spacetime_10.0", "row_iter", FUNCTION),
            ("env", "memory", MEMORY_WITH_MAX),
            ("env", "__stack", GLOBAL),
            ("env", "deflate", FUNCTION),
        )
        self.assertEqual(
            list(checker.imports(module)),
            [
                ("spacetime_10.0", "row_iter"),
                ("env", "memory"),
                ("env", "__stack"),
                ("env", "deflate"),
            ],
        )

    def test_exit_status_reports_env_imports(self):
        with tempfile.TemporaryDirectory() as directory:
            clean = Path(directory, "clean.wasm")
            clean.write_bytes(wasm(("spacetime_10.0", "row_iter", FUNCTION)))
            linked = Path(directory, "linked.wasm")
            linked.write_bytes(wasm(("env", "deflate", FUNCTION), ("env", "inflate", FUNCTION)))

            run = lambda path: subprocess.run(
                [sys.executable, str(SCRIPT), str(path)], capture_output=True, text=True
            )
            self.assertEqual(run(clean).returncode, 0)
            failed = run(linked)
            self.assertEqual(failed.returncode, 1)
            self.assertIn("deflate, inflate", failed.stderr)


if __name__ == "__main__":
    unittest.main()
