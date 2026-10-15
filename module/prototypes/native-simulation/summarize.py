#!/usr/bin/env python3
import collections
import json
import math
import re
import statistics
import sys


def module_nanoseconds(sample):
    message = sample["timer"].split(sample["timer_name"], 1)[1]
    durations = re.findall(r"([0-9]+(?:\.[0-9]+)?)\s*(ns|[µμu]s|ms|s)\b", message)
    if len(durations) != 1:
        raise ValueError(f"unrecognized Module stopwatch output: {message!r}")
    value, unit = durations[0]
    scale = {"ns": 1, "us": 1_000, "µs": 1_000, "μs": 1_000, "ms": 1_000_000, "s": 1_000_000_000}
    return float(value) * scale[unit]


def summarize(path):
    with open(path, encoding="utf-8") as stream:
        rows = [json.loads(line) for line in stream]
    if not rows or rows[0]["kind"] != "metadata" or rows[-1]["kind"] != "complete":
        raise ValueError("incomplete benchmark run")
    metadata = rows[0]
    groups = collections.defaultdict(list)
    for row in rows:
        if row["kind"] != "sample":
            continue
        if row["native"]["digest"] != row["module"]["digest"]:
            raise ValueError("parity mismatch")
        if not row["warmup"]:
            groups[(row["scenario"], row["creatures"])].append(row)
    if len(groups) != len(metadata["scenarios"]) * len(metadata["counts"]):
        raise ValueError("missing benchmark groups")
    print("Times are averages per tick within a seven-tick replay. P95 is across replay averages.")
    print("Setup, snapshots, cleanup, transaction commit, subscriptions and network work are excluded.\n")
    print("| Scenario | Creatures | Native median ms | Module median ms | Ratio | Native p95 ms | Module p95 ms |")
    print("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for (name, creatures), samples in sorted(groups.items()):
        if len(samples) != metadata["samples"] or len({row["sample"] for row in samples}) != len(samples):
            raise ValueError(f"wrong sample count for {name}/{creatures}")
        native = sorted(row["native"]["elapsed_ns"] / row["native"]["ticks"] / 1_000_000 for row in samples)
        module = sorted(module_nanoseconds(row["module"]) / row["module"]["ticks"] / 1_000_000 for row in samples)
        median_native, median_module = statistics.median(native), statistics.median(module)
        p95 = math.ceil(len(samples) * 0.95) - 1
        print(f"| {name} | {creatures} | {median_native:.4f} | {median_module:.4f} | "
              f"{median_module / median_native:.2f}x | {native[p95]:.4f} | {module[p95]:.4f} |")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: summarize.py <output.jsonl>")
    summarize(sys.argv[1])
