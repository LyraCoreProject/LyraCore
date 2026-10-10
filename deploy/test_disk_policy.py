"""Exercise the disk policy at its filesystem and SpacetimeDB process boundaries."""
import importlib.util
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


guard = module('lyracore-disk-guard')
capture = module('lyracore-capture')


class DiskPolicyTests(unittest.TestCase):
    def test_cleanup_start_does_not_revoke_a_recently_verified_lease(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / 'config.json'
            config.write_text(json.dumps({
                'data_directory': directory, 'state_directory': directory,
                'spacetime': '/pinned/cli', 'databases': ['world'],
            }))
            (root / 'status.json').write_text(json.dumps({'state': 'healthy'}))
            cleanup = {'result': 'success', 'state': 'inactive', 'finished': 900_000_000}
            leases = []

            def run(args, **kwargs):
                if args[0] == 'systemctl':
                    output = (f"Result={cleanup['result']}\nActiveState={cleanup['state']}\n"
                              f"ExecMainExitTimestampMonotonic={cleanup['finished']}\n")
                    return subprocess.CompletedProcess(args, 0, stdout=output)
                leases.append(json.loads(args[-2]))
                return subprocess.CompletedProcess(args, 0)

            # systemd clears the current exit timestamp while its next oneshot is running.
            renew = guard.renew
            with patch.object(sys, 'argv', ['guard', str(config)]), \
                    patch.object(guard.subprocess, 'run', side_effect=run), \
                    patch.object(guard, 'renew', side_effect=lambda config, status: renew(config, status, run)), \
                    patch.object(guard.shutil, 'disk_usage', return_value=SimpleNamespace(free=50 * guard.GIB)), \
                    patch.object(guard.time, 'monotonic', return_value=1000), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(guard.main(), 0)
                cleanup.update(state='activating', finished=0)
                self.assertEqual(guard.main(), 0)
                self.assertTrue(all(int(value) > 0 for value in leases), leases)
                status = json.loads((root / 'status.json').read_text())
                self.assertEqual(status['state'], 'healthy')
                self.assertEqual(status['prune_age_seconds'], 100)

    def test_running_cleanup_cannot_extend_the_last_success_deadline(self):
        previous = {'boot_id': 'boot-one', 'prune_success_monotonic': 100}
        properties = {'Result': 'success', 'ActiveState': 'activating',
                      'ExecMainExitTimestampMonotonic': '0'}
        for now in [1000, 1800, 1900]:
            status = {'state': 'healthy', 'lease_until_micros': 300_000_000}
            self.assertTrue(guard.check_prune(status, previous, properties, now, 'boot-one'))
            previous = status
        self.assertFalse(guard.check_prune(status, previous, properties, 1901, 'boot-one'))
        self.assertEqual(status['lease_until_micros'], 0)

    def test_cleanup_failure_or_untrusted_history_cannot_renew_capacity(self):
        cases = [
            ('exit-code', 'failed', 'boot-one', 100),
            ('unknown', 'activating', 'boot-one', 100),
            ('success', 'inactive', 'boot-one', 100),
            ('success', 'activating', 'previous-boot', 100),
            ('success', 'activating', 'boot-one', 1001),
            ('success', 'activating', 'boot-one', 'broken'),
            ('success', 'activating', 'boot-one', True),
            ('success', 'activating', 'boot-one', float('nan')),
            ('success', 'activating', 'boot-one', 0),
        ]
        for result, active, boot, finished in cases:
            with self.subTest(result=result, active=active, boot=boot, finished=finished):
                previous = {'boot_id': boot, 'prune_success_monotonic': finished}
                properties = {'Result': result, 'ActiveState': active,
                              'ExecMainExitTimestampMonotonic': '0'}
                status = {'state': 'healthy', 'lease_until_micros': 300_000_000}
                self.assertFalse(guard.check_prune(status, previous, properties, 1000, 'boot-one'))
                self.assertEqual(status['lease_until_micros'], 0)

    def test_reserve_suspends_and_recovery_requires_explicit_resume(self):
        self.assertEqual(guard.decide(19 * guard.GIB, False), 'suspended')
        self.assertEqual(guard.decide(50 * guard.GIB, True), 'suspended')
        self.assertEqual(guard.decide(50 * guard.GIB, True, True), 'healthy')
        with self.assertRaises(ValueError):
            guard.decide(29 * guard.GIB, True, True)
        self.assertEqual(guard.decide(20 * guard.GIB, False), 'warning')
        self.assertEqual(guard.decide(30 * guard.GIB, False), 'healthy')

    def test_growth_and_expiring_lease_have_explicit_units(self):
        status = guard.sample({'databases': ['world']}, {'state': 'healthy', 'sampled_at': 100, 'free_bytes': 50 * guard.GIB}, 160, 49 * guard.GIB)
        self.assertEqual(status['lease_until_micros'], 340_000_000)
        self.assertAlmostEqual(status['growth_bytes_per_second'], guard.GIB / 60)
        self.assertAlmostEqual(status['hours_to_reserve'], 29 / 60)

    def test_lost_status_requires_explicit_resume_even_with_free_space(self):
        self.assertEqual(guard.sample({'databases': ['world']}, {}, 160, 50 * guard.GIB)['lease_until_micros'], 0)
        self.assertGreater(guard.sample({'databases': ['world']}, {}, 160, 50 * guard.GIB, True)['lease_until_micros'], 0)

    def test_failed_or_overdue_pruning_revokes_capacity_and_latches_suspension(self):
        for result, age in [('exit-code', 10), ('success', 1801), ('success', None)]:
            status = {'state': 'healthy', 'lease_until_micros': 300_000_000}
            self.assertFalse(guard.require_recent_prune(status, result, age))
            self.assertEqual(status['state'], 'suspended')
            self.assertEqual(status['lease_until_micros'], 0)
            recovered = guard.sample({'databases': ['world']}, status, 160, 50 * guard.GIB)
            self.assertEqual(recovered['state'], 'suspended')

    def test_invalid_config_names_the_offending_field(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'config.json'
            for value, message in [([], 'JSON object'), ({}, 'data_directory'), ({'data_directory': 10}, 'data_directory')]:
                path.write_text(json.dumps(value))
                with self.assertRaisesRegex(ValueError, message):
                    guard.load_config(path)

    def test_unrecognized_capture_metadata_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, meta in enumerate([[], {'format': capture.MARKER, 'finished_at': 'broken'}, {'format': capture.MARKER, 'finished_at': None}]):
                path = root / str(index)
                path.mkdir()
                (path / 'complete.json').write_text(json.dumps(meta))
            capture.prune(root, time.time(), budget=0)
            self.assertEqual(len(list(root.iterdir())), 3)

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

    def test_renewal_time_budget_covers_more_than_four_shards(self):
        timeouts = []
        elapsed = [0.0]
        def run(args, **kwargs):
            timeouts.append(kwargs['timeout'])
            elapsed[0] += kwargs['timeout'] + 0.5
            raise subprocess.TimeoutExpired(args, kwargs['timeout'])
        shards = ['shard-' + str(n) for n in range(8)]
        self.assertEqual(guard.renew({'spacetime': '/pinned/cli', 'databases': shards}, {'lease_until_micros': 0}, run, lambda: elapsed[0]), shards)
        self.assertEqual(len(timeouts), len(shards))
        self.assertLessEqual(sum(timeouts) + 0.5 * (len(shards) - 1), 45)

    def test_renewal_reports_unattempted_shards_when_deadline_is_exhausted(self):
        elapsed = [0.0]
        def run(args, **kwargs):
            elapsed[0] = 46
            return subprocess.CompletedProcess(args, 0)
        self.assertEqual(guard.renew({'spacetime': '/pinned/cli', 'databases': ['first', 'second']}, {'lease_until_micros': 0}, run, lambda: elapsed[0]), ['second'])

    def test_silent_command_times_out_without_holding_capture_open(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = capture.capture(root, 'silent', [sys.executable, '-c', 'import time; time.sleep(60)'], timeout=0.05)
            self.assertEqual(code, 124)
            self.assertTrue(json.loads((root / 'silent/complete.json').read_text())['timed_out'])

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
