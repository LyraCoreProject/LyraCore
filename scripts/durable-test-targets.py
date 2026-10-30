#!/usr/bin/env python3
"""Distribute Module targets by duration, including each CI job's other work."""

import argparse
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
# Durations from https://github.com/LyraCoreProject/LyraCore/actions/runs/40829225789.
# Job 2 also runs the populated-upgrade check from a cached baseline Wasm.
JOB_SECONDS = (0, 0, 33)
DEFAULT_TARGET_SECONDS = 20


def partition(targets, seconds, job_seconds=JOB_SECONDS):
    jobs = [[] for _ in job_seconds]
    totals = list(job_seconds)
    for target in sorted(
        targets, key=lambda name: (-seconds.get(name, DEFAULT_TARGET_SECONDS), name)
    ):
        job = min(range(len(jobs)), key=lambda index: totals[index])
        jobs[job].append(target)
        totals[job] += seconds.get(target, DEFAULT_TARGET_SECONDS)
    return jobs, totals


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("job", type=int, choices=range(len(JOB_SECONDS)))
    args = parser.parse_args()
    seconds = json.loads((ROOT / ".github/durable-test-seconds.json").read_text())
    targets = [path.stem for path in (ROOT / "module/tests").glob("*.rs")]
    jobs, totals = partition(targets, seconds)
    if not all(jobs):
        parser.error("every durable job must select at least one Module target")
    print(f"Estimated job durations in seconds: {totals}", file=sys.stderr)
    print("\n".join(sorted(jobs[args.job])))


if __name__ == "__main__":
    main()
