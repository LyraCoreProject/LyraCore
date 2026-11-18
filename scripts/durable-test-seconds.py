#!/usr/bin/env python3
"""Refresh .github/durable-test-seconds.json from successful Module durable test runs."""

import argparse
import json
import re
import statistics
import subprocess
import sys
from datetime import datetime
from math import ceil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WEIGHTS = ROOT / ".github/durable-test-seconds.json"
# The steps one job runs alone; the others cost every job the same.
SHARD_STEPS = {
    "Test the CI scripts",
    "Check the IDC bindings match the Module's idc_ reducers",
    "Check populated items reimports",
    "Verify the populated Module upgrade",
}
JOB_NAME = re.compile(r"durable \((\d+)\)")
LOG_LINE = re.compile(r"^\ufeff?(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z (.*)")
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def timestamp(text):
    return datetime.fromisoformat(text[:26])


def suite_target_seconds(log):
    """Seconds per target, from each `Running tests/` line to the next one."""
    lines = []
    for raw in log.splitlines():
        match = LOG_LINE.match(ANSI.sub("", raw))
        if match:
            lines.append((timestamp(match.group(1)), match.group(2)))
    start = next((i for i, (_, text) in enumerate(lines) if text.startswith("targets: ")), None)
    if start is None:
        raise ValueError("the log has no `targets:` line")
    end = next(
        (i for i in range(start, len(lines)) if lines[i][1].startswith("##[group]Run ")),
        len(lines),
    )
    runs = [
        (time, running.group(1))
        for time, text in lines[start:end]
        if (running := re.match(r"\s*Running tests/(\w+)\.rs", text))
    ]
    finished = lines[end - 1][0]
    seconds = {}
    for index, (time, target) in enumerate(runs):
        next_time = runs[index + 1][0] if index + 1 < len(runs) else finished
        seconds[target] = (next_time - time).total_seconds()
    return seconds


def job_extra_seconds(job):
    """Seconds a job spends in the steps that only some jobs run."""
    return sum(
        (timestamp(step["completedAt"]) - timestamp(step["startedAt"])).total_seconds()
        for step in job["steps"]
        if step["name"] in SHARD_STEPS
    )


def gh(*args):
    return subprocess.run(
        ["gh", *args], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout


def measure_run(run):
    data = json.loads(gh("run", "view", str(run), "--json", "conclusion,jobs"))
    if data["conclusion"] != "success":
        sys.exit(f"run {run} did not succeed")
    targets, extra = {}, {}
    for job in data["jobs"]:
        name = JOB_NAME.fullmatch(job["name"])
        if name is None:
            sys.exit(f"run {run} has a job named {job['name']!r}, not 'durable (N)'")
        log = gh(
            "api", "--allow-escape-sequences", f"repos/{{owner}}/{{repo}}/actions/jobs/{job['databaseId']}/logs"
        )
        try:
            targets.update(suite_target_seconds(log))
        except ValueError as error:
            sys.exit(f"run {run}, {job['name']}: {error}")
        extra[int(name.group(1))] = job_extra_seconds(job)
    return targets, [ceil(extra[shard]) for shard in sorted(extra)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runs", nargs="+", type=int, help="successful Module durable test run ids")
    args = parser.parse_args()
    measured = [measure_run(run) for run in args.runs]
    if len({len(jobs) for _, jobs in measured}) != 1:
        sys.exit("the runs have different numbers of durable jobs")
    names = sorted({name for targets, _ in measured for name in targets})
    weights = {
        "source_runs": args.runs,
        "job_seconds": [
            ceil(statistics.median(column)) for column in zip(*(jobs for _, jobs in measured))
        ],
        "target_seconds": {
            name: ceil(statistics.median(targets[name] for targets, _ in measured if name in targets))
            for name in names
        },
    }
    WEIGHTS.write_text(json.dumps(weights, indent=2) + "\n")


if __name__ == "__main__":
    main()
