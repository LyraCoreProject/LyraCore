#!/usr/bin/env python3
"""Distribute Module targets by duration, including each CI job's other work."""

import argparse
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WEIGHTS = ROOT / ".github/durable-test-seconds.json"
DEFAULT_TARGET_SECONDS = 20


def partition(targets, seconds, job_seconds):
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
    parser.add_argument("job", type=int)
    args = parser.parse_args()
    weights = json.loads(WEIGHTS.read_text())
    job_seconds = weights["job_seconds"]
    if args.job not in range(len(job_seconds)):
        parser.error(f"job must be below {len(job_seconds)}")
    targets = [path.stem for path in (ROOT / "module/tests").glob("*.rs")]
    jobs, totals = partition(targets, weights["target_seconds"], job_seconds)
    if not all(jobs):
        parser.error("every durable job must select at least one Module target")
    print(f"Estimated job durations in seconds: {totals}", file=sys.stderr)
    print("\n".join(sorted(jobs[args.job])))


if __name__ == "__main__":
    main()
