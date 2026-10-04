#!/usr/bin/env python3
"""Renew Package capacity leases while the host retains its disk reserve."""
import argparse
import fcntl
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time

GIB = 1024 ** 3
WARN_BYTES = 30 * GIB
STOP_BYTES = 20 * GIB
LEASE_SECONDS = 180


def atomic_json(path, value):
    temp = path.with_suffix('.tmp')
    temp.write_text(json.dumps(value, indent=2) + '\n')
    temp.replace(path)


def decide(free, latched, resume=False):
    if resume and free < WARN_BYTES:
        raise ValueError('Resume requires at least 30 GiB free')
    suspended = free < STOP_BYTES or (latched and not resume)
    return 'suspended' if suspended else 'warning' if free < WARN_BYTES else 'healthy'


def sample(config, previous, now, free, resume=False):
    state = decide(free, previous.get('suspended', False), resume)
    elapsed = now - previous.get('sampled_at', now)
    growth = max(0, previous.get('free_bytes', free) - free) / elapsed if elapsed > 0 else 0
    return {
        'sampled_at': now, 'free_bytes': free, 'growth_bytes_per_second': growth,
        'hours_to_reserve': (free - STOP_BYTES) / growth / 3600 if growth and free > STOP_BYTES else None,
        'state': state, 'suspended': state == 'suspended',
        'lease_until_micros': 0 if state == 'suspended' else int((now + LEASE_SECONDS) * 1_000_000),
        'databases': config['databases'],
    }


def renew(config, status, run=subprocess.run):
    failures = []
    for database in config['databases']:
        args = [config['spacetime'], 'call', '--server', 'local', database, '--',
                'set_package_config', json.dumps('playerbots'),
                json.dumps('capacity_until_micros'),
                json.dumps(str(status['lease_until_micros'])), 'true']
        try:
            result = run(args, capture_output=True, timeout=10)
            if result.returncode:
                failures.append(database)
        except (OSError, subprocess.TimeoutExpired):
            failures.append(database)
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    parser.add_argument('--resume', action='store_true', help='Clear the low-disk latch; frozen bots still need Operator activation')
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    databases = config['databases']
    if not databases or len(set(databases)) != len(databases) or any(
        not isinstance(name, str) or not name or name.startswith('-') for name in databases
    ):
        raise ValueError('Configure every Shard exactly once')
    root = Path(config['state_directory'])
    root.mkdir(parents=True, exist_ok=True)
    with (root / 'lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        path = root / 'status.json'
        previous = json.loads(path.read_text()) if path.exists() else {}
        status = sample(config, previous, time.time(), shutil.disk_usage(config['data_directory']).free, args.resume)
        # Persist the latch before network calls, so a partial update cannot resume bots later.
        atomic_json(path, status)
        failures = renew(config, status)
        status['failed_databases'] = failures
        prune = subprocess.run(['systemctl', 'show', 'spacetimedb-prune.service',
                                '--property=Result', '--property=ExecMainExitTimestampMonotonic'],
                               capture_output=True, text=True, timeout=5)
        properties = dict(line.split('=', 1) for line in prune.stdout.splitlines() if '=' in line)
        status['prune_result'] = properties.get('Result', 'unknown') if prune.returncode == 0 else 'unknown'
        finished = int(properties.get('ExecMainExitTimestampMonotonic', '0')) / 1_000_000
        status['prune_age_seconds'] = time.monotonic() - finished if finished else None
        atomic_json(path, status)
        unhealthy = failures or status['prune_result'] != 'success' or not finished or status['prune_age_seconds'] > 1800
        priority = '<3>' if unhealthy or status['suspended'] else '<4>' if status['state'] == 'warning' else '<6>'
        print(priority + json.dumps(status), flush=True)
        if unhealthy:
            return 1
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        print(f'disk guard failed: {type(error).__name__}; inspect configuration and service state', file=sys.stderr)
        sys.exit(1)
