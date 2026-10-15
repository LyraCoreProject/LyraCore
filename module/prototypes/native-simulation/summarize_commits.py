#!/usr/bin/env python3
import collections
import json
import math
import statistics
import sys


def summarize(path):
    with open(path, encoding="utf-8") as stream:
        rows = [json.loads(line) for line in stream]
    if not rows or rows[0]["kind"] != "metadata" or rows[-1]["kind"] != "complete":
        raise ValueError("incomplete committed benchmark")
    metadata = rows[0]
    groups = collections.defaultdict(list)
    seen = set()
    digests = {}
    updates = {}
    for row in rows[1:-1]:
        if row["kind"] != "tick":
            raise ValueError("unexpected benchmark record")
        key = row["scenario"], row["creatures"], row["sample"]
        identity = *key, row["implementation"], row["sequence"]
        if identity in seen:
            raise ValueError("duplicate tick")
        seen.add(identity)
        if row["warmup"] != (row["sample"] < metadata["warmups"]):
            raise ValueError("incorrect warmup marker")
        if digests.setdefault(key, row["final_digest"]) != row["final_digest"]:
            raise ValueError("committed state mismatch")
        tick = *key, row["sequence"]
        digest = row["timing"]["subscription_digest"]
        if updates.setdefault(tick, digest) != digest:
            raise ValueError("subscription row mismatch")
        if not row["warmup"]:
            groups[(row["scenario"], row["creatures"], row["implementation"])].append(row["timing"])
    expected = {
        (name, count, sample, implementation, sequence)
        for name in metadata["scenarios"]
        for count in metadata["counts"]
        for sample in range(metadata["samples"] + metadata["warmups"])
        for implementation in ("module", "native_host")
        for sequence in range(1, metadata["ticks_per_replay"][name] + 1)
    }
    if seen != expected:
        raise ValueError("missing or unexpected tick samples")
    print("One tick per durable transaction, including confirmed subscription output.")
    print("Mean and p95 use individual ticks. Requests use binary WebSockets; publication includes CLI JSON decoding.\n")
    print("| Scenario | Creatures | Module mean ms | Native host mean ms | Ratio | Module p95 ms | Native host p95 ms |")
    print("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for name in sorted(metadata["scenarios"]):
        for count in metadata["counts"]:
            module = sorted(t["elapsed_ns"] / 1e6 for t in groups[(name, count, "module")])
            native = sorted(t["elapsed_ns"] / 1e6 for t in groups[(name, count, "native_host")])
            p95 = math.ceil(len(module) * 0.95) - 1
            ratio = statistics.mean(module) / statistics.mean(native)
            print(f"| {name} | {count} | {statistics.mean(module):.3f} | {statistics.mean(native):.3f} | "
                  f"{ratio:.2f}x | {module[p95]:.3f} | {native[p95]:.3f} |")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: summarize_commits.py <output.jsonl>")
    summarize(sys.argv[1])
