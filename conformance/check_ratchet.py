import argparse
import html
import json
from pathlib import Path
import sys


ACCEPT_LABEL = "conformance-accept-signature"


def signatures(entries):
    result = set()
    for entry in entries:
        signature = dict(entry)
        signature.setdefault("character", None)
        result.add(json.dumps(signature, sort_keys=True))
    return result


def scenarios(baseline):
    if baseline["schema_version"] != 1:
        raise ValueError("unsupported Tier 1 baseline schema")
    result = {}
    for entry in baseline["scenarios"]:
        name = entry["scenario"]
        if not isinstance(name, str) or not name or name in result:
            raise ValueError("baseline scenarios must have unique nonempty names")
        for field in ("scenario_sha256", "harness_revision"):
            if not isinstance(entry[field], str) or not entry[field]:
                raise ValueError(f"{name}: invalid {field}")
        if not isinstance(entry["rule"], list):
            raise ValueError(f"{name}: rule signatures must be an array")
        result[name] = entry
    return result


def compare(base, current, labels):
    if not isinstance(labels, list) or not all(isinstance(label, str) for label in labels):
        raise ValueError("labels must be an array of GitHub label names")
    previous = scenarios(base)
    candidate = scenarios(current)
    additions = {}
    removed = {}
    measurements = {}
    for name, entry in candidate.items():
        old = previous.get(name)
        reasons = ["new scenario"] if old is None else [
            field for field in ("scenario_sha256", "harness_revision")
            if old[field] != entry[field]
        ]
        if reasons:
            measurements[name] = reasons
            continue
        before = signatures(old["rule"])
        after = signatures(entry["rule"])
        if after - before:
            additions[name] = [json.loads(value) for value in sorted(after - before)]
        if before - after:
            removed[name] = len(before - after)
    accepted = ACCEPT_LABEL in labels
    return {
        "passes": not additions or accepted,
        "accepted_by_label": accepted,
        "added_rule_signatures": additions,
        "removed_rule_signatures": removed,
        "removed_scenarios": sorted(previous.keys() - candidate.keys()),
        "new_measurements": measurements,
    }


def main():
    parser = argparse.ArgumentParser(description="Compare Tier 1 baselines against the merge base")
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--current", type=Path, required=True)
    parser.add_argument("--labels", type=Path, required=True)
    args = parser.parse_args()
    print("## Tier 1 signature ratchet\n")
    try:
        result = compare(*(json.loads(path.read_text()) for path in (
            args.base, args.current, args.labels,
        )))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Cannot compare baselines: {html.escape(str(error))}")
        return 2
    if result["passes"]:
        print("Passed.")
    else:
        print(f"Failed: a maintainer must apply `{ACCEPT_LABEL}` to accept added rule signatures.")
    print("\n<pre>" + html.escape(json.dumps(result, indent=2)) + "</pre>")
    return 0 if result["passes"] else 1


if __name__ == "__main__":
    sys.exit(main())
