#!/usr/bin/env python3
"""Keep bounded, disposable command output; acceptance evidence belongs in a separate archive."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

TOTAL_BYTES = 2 * 1024 ** 3
CAPTURE_BYTES = 256 * 1024 ** 2
MAX_AGE = 7 * 24 * 3600
MARKER = 'lyracore-routine-capture-v1'


def retained(root):
    result = []
    for path in root.iterdir():
        if path.is_symlink() or not path.is_dir():
            continue
        marker = path / 'complete.json'
        if marker.is_symlink() or not marker.is_file():
            continue
        try:
            meta = json.loads(marker.read_text())
            if meta.get('format') == MARKER:
                size = sum(p.stat().st_size for p in path.iterdir() if p.is_file() and not p.is_symlink())
                result.append((meta['finished_at'], size, path))
        except (ValueError, KeyError):
            continue
    return sorted(result)


def prune(root, now, budget=TOTAL_BYTES):
    captures = retained(root)
    total = sum(size for _, size, _ in captures)
    for finished, size, path in captures:
        if now - finished > MAX_AGE or total > budget:
            shutil.rmtree(path)
            total -= size
    return total


def used_bytes(root):
    total = 0
    for directory, _, files in os.walk(root, followlinks=False):
        for name in files:
            path = Path(directory) / name
            if not path.is_symlink():
                total += path.stat().st_size
    return total


def capture(root, name, command, limit=CAPTURE_BYTES):
    if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._-]{0,100}', name):
        raise ValueError('Use a simple capture name')
    path = root / name
    path.mkdir()
    started = time.time()
    saved = 0
    omitted = 0
    tail = b''
    # Command arguments are deliberately not recorded; callers may carry credentials there.
    with (path / 'output.log').open('wb') as output:
        with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as proc:
            while chunk := proc.stdout.read(65536):
                keep = chunk[:max(0, limit - saved)]
                output.write(keep)
                saved += len(keep)
                omitted += len(chunk) - len(keep)
                tail = (tail + chunk)[-65536:]
            code = proc.wait()
    if omitted:
        (path / 'tail.log').write_bytes(tail)
    (path / 'complete.json').write_text(json.dumps({
        'format': MARKER, 'started_at': started, 'finished_at': time.time(),
        'exit_status': code, 'saved_bytes': saved, 'omitted_bytes': omitted,
    }, indent=2) + '\n')
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path('/var/lib/lyracore/routine-captures'))
    parser.add_argument('--prune', action='store_true')
    parser.add_argument('name', nargs='?')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    root = args.root
    if root.is_symlink():
        raise ValueError('Capture root must not be a symlink')
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (root / '.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        prune(root, time.time(), TOTAL_BYTES if args.prune else TOTAL_BYTES - CAPTURE_BYTES - 131072)
        if args.prune:
            return 0
        if used_bytes(root) + CAPTURE_BYTES + 131072 > TOTAL_BYTES:
            raise ValueError('Incomplete or unmanaged captures consume the reserve; archive them before recording more')
        command = args.command[1:] if args.command[:1] == ['--'] else args.command
        if not args.name or not command:
            parser.error('Provide a name and command, or --prune')
        code = capture(root, args.name, command)
        prune(root, time.time())
        return code


if __name__ == '__main__':
    os.umask(0o077)
    sys.exit(main())
