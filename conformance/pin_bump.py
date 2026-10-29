#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tomllib


ROOT = Path(__file__).resolve().parent.parent
TAG = re.compile(r"refs/tags/(v0\.2\.0-conformance\.([0-9]+))(\^\{\})?$")
DEPENDENCY = re.compile(
    r'(wire-client\s*=\s*\{\s*git\s*=\s*"https://github.com/LyraCoreProject/wire-harness"\s*,\s*rev\s*=\s*")'
    r'([0-9a-f]{40})("\s*\})'
)
SHA = re.compile(r"[0-9a-f]{40}")
DIGEST = re.compile(r"[0-9a-f]{64}")
TAG_NAME = re.compile(r"v0\.2\.0-conformance\.[0-9]+")


def latest_tag(refs):
    tags = {}
    for line in refs.splitlines():
        sha, ref = line.split("\t", 1)
        match = TAG.fullmatch(ref)
        if match:
            tag, number, peeled = match.groups()
            entry = tags.setdefault(int(number), [tag, None])
            if peeled or entry[1] is None:
                entry[1] = sha
    if not tags:
        raise ValueError("no conformance prerelease tags found")
    return tags[max(tags)]


def remote_tag():
    refs = subprocess.check_output(
        ["git", "ls-remote", "--tags", "https://github.com/LyraCoreProject/wire-harness",
         "v0.2.0-conformance.*"],
        text=True,
    )
    return latest_tag(refs)


def apply_pin(tag, sha):
    if not TAG_NAME.fullmatch(tag) or not SHA.fullmatch(sha):
        raise ValueError("invalid conformance tag or commit")
    pin_path = ROOT / ".wire-harness-rev"
    pin = pin_path.read_text()
    current_tag, current_sha = next(
        line.split() for line in pin.splitlines() if line and not line.startswith("#")
    )
    current_number = int(current_tag.rsplit(".", 1)[1])
    next_number = int(tag.rsplit(".", 1)[1])
    if next_number < current_number or (next_number == current_number and sha != current_sha):
        raise ValueError("remote conformance tag disagrees with the current pin")
    changed = next_number > current_number
    if changed:
        updated_pin = pin.replace(f"{current_tag} {current_sha}", f"{tag} {sha}")
        if updated_pin == pin:
            raise ValueError("could not replace the harness pin")
        manifest_path = ROOT / "Cargo.toml"
        manifest = manifest_path.read_text()
        updated_manifest, count = DEPENDENCY.subn(rf"\g<1>{sha}\g<3>", manifest)
        if count != 1:
            raise ValueError("expected one workspace wire-client dependency")
        pin_path.write_text(updated_pin)
        manifest_path.write_text(updated_manifest)
    return changed


def prepare(output):
    tag, sha = remote_tag()
    changed = apply_pin(tag, sha)
    with open(output, "a") as stream:
        stream.write(f"new={str(changed).lower()}\ntag={tag}\nsha={sha}\n")


def missing_scenarios():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], text=True,
    ))
    clients = [package for package in metadata["packages"]
               if package["name"] == "wire-client"
               and package["source"].startswith("git+https://github.com/LyraCoreProject/wire-harness")]
    if len(clients) != 1:
        raise ValueError("expected one resolved wire-client package")
    catalogue = (Path(clients[0]["manifest_path"]).parent / "src/catalogue.rs").read_text()
    entries = re.findall(r'^\s+Entry\s*\{\s*id:\s*"([a-z0-9_]+)"', catalogue, re.MULTILINE)
    if not entries or len(entries) != catalogue.count("    Entry {") or len(entries) != len(set(entries)):
        raise ValueError("could not read the harness scenario catalogue")
    tests = (ROOT / "module/tests/conformance/tier1.rs").read_text()
    tested = set(re.findall(r'run_scenario\(\s*"([a-z0-9_]+)"', tests))
    ledger = set()
    for path in (ROOT / "conformance/features").glob("*/*.toml"):
        ledger.update(tomllib.loads(path.read_text())["scenarios"])
    return sorted(set(entries) - (tested & ledger))


def check_catalogue(output):
    missing = missing_scenarios()
    with open(output, "a") as stream:
        stream.write(f"missing={str(bool(missing)).lower()}\n")
    if missing:
        message = "New harness scenarios need a combined core PR: " + ", ".join(missing)
        print(message)
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as stream:
            stream.write(f"{message}\n")


def signature_changes(before, after):
    old = {entry["scenario"]: entry for entry in before["scenarios"]}
    new = {entry["scenario"]: entry for entry in after["scenarios"]}
    changes = []
    for scenario in sorted(old.keys() | new.keys()):
        for classification in ("rule", "data"):
            old_set = {json.dumps(item, sort_keys=True, separators=(",", ":"))
                       for item in old.get(scenario, {}).get(classification, [])}
            new_set = {json.dumps(item, sort_keys=True, separators=(",", ":"))
                       for item in new.get(scenario, {}).get(classification, [])}
            added = sorted(new_set - old_set)
            removed = sorted(old_set - new_set)
            if added or removed:
                changes.append((scenario, classification, added, removed))
    return changes


def strict_json(path):
    if path.stat().st_size > 1_000_000:
        raise ValueError(f"artifact file is too large: {path}")

    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    return json.loads(path.read_text(), object_pairs_hook=unique)


def validate_baseline(before, after, sha):
    if set(after) != {"schema_version", "scenarios"} or after["schema_version"] != 1:
        raise ValueError("invalid Tier 1 baseline schema")
    if not isinstance(after["scenarios"], list) or len(after["scenarios"]) != len(before["scenarios"]):
        raise ValueError("Tier 1 baseline scenario set changed")
    before_by_id = {entry["scenario"]: entry for entry in before["scenarios"]}
    if len(before_by_id) != len(before["scenarios"]):
        raise ValueError("committed Tier 1 baseline has duplicate scenarios")
    ids = []
    for entry in after["scenarios"]:
        if not isinstance(entry, dict) or set(entry) != {
            "scenario", "behavior", "harness_revision", "scenario_sha256", "rule", "data"
        }:
            raise ValueError("invalid Tier 1 scenario entry")
        scenario = entry["scenario"]
        if not isinstance(scenario, str) or scenario not in before_by_id:
            raise ValueError("Tier 1 baseline contains an unknown scenario")
        old = before_by_id[scenario]
        if entry["behavior"] != old["behavior"] or entry["harness_revision"] != sha:
            raise ValueError(f"{scenario}: Behavior or harness revision changed unexpectedly")
        if not isinstance(entry["scenario_sha256"], str) or not DIGEST.fullmatch(entry["scenario_sha256"]):
            raise ValueError(f"{scenario}: invalid scenario digest")
        for classification in ("rule", "data"):
            signatures = entry[classification]
            if not isinstance(signatures, list):
                raise ValueError(f"{scenario}: invalid {classification} signatures")
            keys = []
            for signature in signatures:
                if not isinstance(signature, dict) or set(signature) != {
                    "rule", "checkpoint", "stream", "opcode", "field_path"
                }:
                    raise ValueError(f"{scenario}: invalid signature")
                if not all(isinstance(signature[field], str) and signature[field]
                           for field in ("rule", "checkpoint")):
                    raise ValueError(f"{scenario}: invalid signature names")
                stream = signature["stream"]
                stream_order = {"opening": 0, "reply": 1, "broadcast": 2,
                                "unexpected": 3, "malformed": 4}
                if stream is not None:
                    if not isinstance(stream, dict) or stream.get("kind") not in stream_order:
                        raise ValueError(f"{scenario}: invalid stream")
                    if stream["kind"] in ("reply", "broadcast"):
                        if set(stream) != {"kind", "id"} or not isinstance(stream["id"], str):
                            raise ValueError(f"{scenario}: invalid stream id")
                    elif set(stream) != {"kind"}:
                        raise ValueError(f"{scenario}: invalid stream fields")
                opcode = signature["opcode"]
                if opcode is not None and (type(opcode) is not int or not 0 <= opcode <= 65535):
                    raise ValueError(f"{scenario}: invalid opcode")
                path = signature["field_path"]
                if not isinstance(path, list) or not all(isinstance(part, str) for part in path):
                    raise ValueError(f"{scenario}: invalid field path")
                keys.append((signature["rule"], signature["checkpoint"],
                             (-1, "") if stream is None else (stream_order[stream["kind"]], stream.get("id", "")),
                             -1 if opcode is None else opcode, tuple(path)))
            if any(left >= right for left, right in zip(keys, keys[1:])):
                raise ValueError(f"{scenario}: signatures must be unique and sorted")
        ids.append(scenario)
    if ids != sorted(before_by_id):
        raise ValueError("Tier 1 baseline scenarios must be unique and sorted")


def summary_data(before, after, tag, sha):
    changes = [{"scenario": scenario, "classification": classification,
                "added": [json.loads(item) for item in added],
                "removed": [json.loads(item) for item in removed]}
               for scenario, classification, added, removed in signature_changes(before, after)]
    return {"tag": tag, "sha": sha, "changes": changes, "human_review": bool(changes)}


def describe(body_path, output, summary_path):
    before = json.loads(subprocess.check_output(
        ["git", "show", "origin/main:conformance/tier1-baseline.json"], text=True,
    ))
    after = json.loads((ROOT / "conformance/tier1-baseline.json").read_text())
    pin = next(line for line in (ROOT / ".wire-harness-rev").read_text().splitlines()
               if line and not line.startswith("#"))
    tag, sha = pin.split()
    summary = summary_data(before, after, tag, sha)
    if summary_path:
        Path(summary_path).write_text(json.dumps(summary, indent=2) + "\n")
    changes = signature_changes(before, after)
    old_scenarios = {entry["scenario"] for entry in before["scenarios"]}
    new_scenarios = {entry["scenario"] for entry in after["scenarios"]}
    catalogue_changes = sorted(old_scenarios ^ new_scenarios)
    lines = [
        "## What and why", "",
        "Core must use the same conformance harness revision for dev smoke and tier 1. "
        "This automated PR advances both pins and regenerates the Tier 1 baseline from the "
        f"tested comparison at `{pin}`.", "",
        "## What you verified", "",
        "- All tier 1 scenarios ran against a disposable Shard before and after baseline regeneration.",
        "- `./conformance/baseline` regenerated every catalogue entry.", "",
        "## Signature changes", "",
    ]
    if changes:
        for scenario, classification, added, removed in changes:
            lines.extend([f"### `{scenario}` {classification}", ""])
            lines.extend(f"- Added `{item}`" for item in added)
            lines.extend(f"- Removed `{item}`" for item in removed)
            lines.append("")
        lines.append("Human review is required for these signature changes.")
    else:
        lines.append("The rule and data signature sets did not change.")
    if catalogue_changes:
        lines.extend(["", "## Scenario catalogue changes", ""])
        lines.extend(f"- Added `{scenario}`" for scenario in sorted(new_scenarios - old_scenarios))
        lines.extend(f"- Removed `{scenario}`" for scenario in sorted(old_scenarios - new_scenarios))
    if not changes and not catalogue_changes:
        lines.append("Only harness and scenario provenance changed.")
    lines.extend([
        "", "## What you did NOT verify", "",
        "- CI and a real client check are pending review of this PR.", "",
        "## Risk", "",
        "The new harness revision can change tier 1 comparisons. The signature diff above and "
        "CI checks expose those changes before merge. This PR never merges itself.", "",
        "## Notes for the reviewer", "",
        "Review the harness tag and any signature changes. GitHub Actions generated this pin "
        "change. Model: none. Conformance harness: wire-harness at the tag above.", "",
    ])
    if body_path:
        Path(body_path).write_text("\n".join(lines))
    with open(output, "a") as stream:
        stream.write(f"human_review={str(bool(changes or catalogue_changes)).lower()}\n")


def publish(baseline_path, summary_path, body_path, output):
    summary = strict_json(Path(summary_path))
    if not isinstance(summary, dict) or set(summary) != {"tag", "sha", "changes", "human_review"}:
        raise ValueError("invalid pin proposal summary")
    tag, sha = remote_tag()
    if (summary["tag"], summary["sha"]) != (tag, sha) or not SHA.fullmatch(sha):
        raise ValueError("pin proposal does not match the latest conformance tag")
    before = strict_json(ROOT / "conformance/tier1-baseline.json")
    after = strict_json(Path(baseline_path))
    validate_baseline(before, after, sha)
    if Path(baseline_path).read_text() != json.dumps(after, indent=2, ensure_ascii=False) + "\n":
        raise ValueError("Tier 1 baseline is not canonical")
    if summary != summary_data(before, after, tag, sha):
        raise ValueError("pin proposal summary disagrees with the baseline")
    if not apply_pin(tag, sha):
        raise ValueError("pin proposal is already current")
    shutil.copyfile(baseline_path, ROOT / "conformance/tier1-baseline.json")
    describe(body_path, output, None)
    with open(output, "a") as stream:
        stream.write(f"tag={tag}\nsha={sha}\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("prepare", "catalogue", "describe", "publish"))
    parser.add_argument("--output", required=True)
    parser.add_argument("--body")
    parser.add_argument("--summary")
    parser.add_argument("--baseline")
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.output)
    elif args.command == "catalogue":
        check_catalogue(args.output)
    elif args.command == "describe":
        if not (args.body or args.summary):
            parser.error("describe requires --body or --summary")
        describe(args.body, args.output, args.summary)
    else:
        if not args.body or not args.summary or not args.baseline:
            parser.error("publish requires --body, --summary and --baseline")
        publish(args.baseline, args.summary, args.body, args.output)


if __name__ == "__main__":
    main()
