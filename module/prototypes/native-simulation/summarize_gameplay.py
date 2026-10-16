#!/usr/bin/env python3
import argparse
import hashlib
import json
import math
import statistics
from pathlib import Path


def summarize(path):
    contents = path.read_bytes()
    records = [json.loads(line) for line in contents.splitlines()]
    if not records or records[0]["kind"] != "metadata" or records[-1]["kind"] != "complete":
        raise ValueError("incomplete gameplay benchmark")
    metadata = records[0]
    if metadata["scenario"] != "creature_cycle" or metadata["candidate_implementation"] != "native_database":
        raise ValueError("expected a complete creature-cycle native comparison")
    samples = metadata["samples"]
    warmups = metadata["warmups"]
    complete = records[-1]
    if (complete["samples"], complete["warmups"]) != (samples, warmups):
        raise ValueError("completion counts do not match metadata")
    steps = {}
    frames = {}
    for record in records[1:-1]:
        if record["kind"] == "parity":
            if record["frame"] in frames:
                raise ValueError("duplicate parity frame")
            frames[record["frame"]] = record["digest"]
        elif record["kind"] == "step":
            key = record["sample"], record["sequence"], record["implementation"]
            if key in steps:
                raise ValueError("duplicate gameplay step")
            if record["warmup"] != (record["sample"] < warmups):
                raise ValueError("incorrect warmup marker")
            if record["native_first"] != (record["sample"] % 2 == 1):
                raise ValueError("execution order did not alternate")
            steps[key] = record
        else:
            raise ValueError("unexpected gameplay benchmark record")
    expected_steps = {(sample, sequence, implementation)
                      for sample in range(samples + warmups) for sequence in range(1, 13)
                      for implementation in ("module", "native_database")}
    expected_frames = {f"sample {sample} {suffix}"
                       for sample in range(samples + warmups)
                       for suffix in ("initial", "final durable state", *(f"step {n}" for n in range(1, 13)))}
    if steps.keys() != expected_steps or frames.keys() != expected_frames:
        raise ValueError("missing gameplay steps or parity frames")
    summaries = {}
    measurements = {}
    columns = ("elapsed_ns", "acknowledgment_ns", "publication_ns", "server_cpu_ticks",
               "driver_cpu_ticks", "server_rss_kib", "server_peak_rss_kib", "subscription_json_bytes")
    for implementation in ("module", "native_database"):
        rows = [steps[sample, sequence, implementation]
                for sample in range(warmups, samples + warmups) for sequence in range(1, 13)]
        elapsed = sorted(row["elapsed_ns"] / 1_000_000 for row in rows)
        summaries[implementation] = {
            "steps": len(rows), "mean_ms": statistics.mean(elapsed),
            "median_ms": statistics.median(elapsed), "p95_ms": elapsed[math.ceil(len(rows) * .95) - 1],
            "max_ms": elapsed[-1], "steps_over_1_second": sum(value > 1000 for value in elapsed),
            "server_cpu_seconds": sum(row["server_cpu_ticks"] for row in rows) / metadata["cpu_ticks_per_second"],
            "driver_cpu_seconds": sum(row["driver_cpu_ticks"] for row in rows) / metadata["cpu_ticks_per_second"],
            "maximum_sampled_rss_kib": max(row["server_rss_kib"] for row in rows),
            "process_peak_rss_kib": max(row["server_peak_rss_kib"] for row in rows),
            "subscription_json_bytes": sum(row["subscription_json_bytes"] for row in rows),
        }
        measurements[implementation] = [[row[column] for column in columns] for row in rows]
    reference, native = (summaries[name] for name in ("module", "native_database"))
    return {
        "metadata": metadata, "raw_sha256": hashlib.sha256(contents).hexdigest(),
        "paired_steps_including_warmups": len(steps) // 2, "application_tables": complete["tables"],
        "parity_frames": frames, "summary": summaries,
        "mean_time_reduction_percent": 100 * (1 - native["mean_ms"] / reference["mean_ms"]),
        "p95_time_reduction_percent": 100 * (1 - native["p95_ms"] / reference["p95_ms"]),
        "server_cpu_reduction_percent": 100 * (1 - native["server_cpu_seconds"] / reference["server_cpu_seconds"]),
        "measurement_order": "sample, then sequence; measured samples only",
        "measurement_columns": columns, "measurements": measurements,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("inputs", nargs="+", type=Path)
    args = parser.parse_args()
    print(json.dumps([summarize(path) for path in args.inputs], indent=2))
