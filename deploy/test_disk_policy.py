"""Exercise the disk policy at its filesystem and SpacetimeDB process boundaries."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


guard = module('lyracore-disk-guard')
capture = module('lyracore-capture')


class DiskPolicyTests(unittest.TestCase):
    def test_reserve_suspends_and_recovery_requires_explicit_resume(self):
        self.assertEqual(guard.decide(19 * guard.GIB, False), 'suspended')
        self.assertEqual(guard.decide(50 * guard.GIB, True), 'suspended')
        self.assertEqual(guard.decide(50 * guard.GIB, True, True), 'healthy')
        with self.assertRaises(ValueError):
            guard.decide(29 * guard.GIB, True, True)
        self.assertEqual(guard.decide(20 * guard.GIB, False), 'warning')
        self.assertEqual(guard.decide(30 * guard.GIB, False), 'healthy')

    def test_growth_and_expiring_lease_have_explicit_units(self):
        status = guard.sample({'databases': ['world']}, {'sampled_at': 100, 'free_bytes': 50 * guard.GIB}, 160, 49 * guard.GIB)
        self.assertEqual(status['lease_until_micros'], 340_000_000)
        self.assertAlmostEqual(status['growth_bytes_per_second'], guard.GIB / 60)
        self.assertAlmostEqual(status['hours_to_reserve'], 29 / 60)

    def test_partial_reducer_failure_is_reported_and_remaining_shards_are_attempted(self):
        attempts = []
        def run(args, **kwargs):
            attempts.append(args)
            return subprocess.CompletedProcess(args, 1 if 'first' in args else 0)
        failed = guard.renew({'spacetime': '/pinned/cli', 'databases': ['first', 'second']}, {'lease_until_micros': 0}, run)
        self.assertEqual(failed, ['first'])
        self.assertEqual(len(attempts), 2)
        self.assertEqual(attempts[0][-2:], ['"0"', 'true'])
        self.assertIn('local', attempts[0])

    def test_capture_bounds_output_preserves_exit_status_and_last_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = capture.capture(root, 'failed-command', [sys.executable, '-c', 'print("x" * 10000); print("last diagnostic"); raise SystemExit(7)'], limit=100)
            self.assertEqual(code, 7)
            self.assertEqual((root / 'failed-command/output.log').stat().st_size, 100)
            self.assertIn('last diagnostic', (root / 'failed-command/tail.log').read_text())
            self.assertGreater(json.loads((root / 'failed-command/complete.json').read_text())['omitted_bytes'], 0)

    def test_retention_removes_oldest_completed_captures_and_preserves_other_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, finished in [('expired', 0), ('older', time.time() - 100), ('newer', time.time())]:
                path = root / name
                path.mkdir()
                (path / 'complete.json').write_text(json.dumps({'format': capture.MARKER, 'finished_at': finished}))
                (path / 'output.log').write_text('x' * 1000)
            (root / 'acceptance').mkdir()
            (root / 'incomplete').mkdir()
            (root / 'external').symlink_to(root / 'acceptance', target_is_directory=True)
            capture.prune(root, time.time(), budget=1500)
            self.assertFalse((root / 'expired').exists())
            self.assertFalse((root / 'older').exists())
            self.assertTrue((root / 'newer').exists())
            self.assertTrue((root / 'acceptance').exists())
            self.assertTrue((root / 'incomplete').exists())
            self.assertTrue((root / 'external').is_symlink())


if __name__ == '__main__':
    unittest.main()
