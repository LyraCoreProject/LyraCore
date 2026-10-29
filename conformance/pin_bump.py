#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parent.parent
TAG = re.compile(r"refs/tags/(v0\.2\.0-conformance\.([0-9]+))(\^\{\})?$")
DEPENDENCY = re.compile(
    r'(wire-client\s*=\s*\{\s*git\s*=\s*"https://github.com/LyraCoreProject/wire-harness"\s*,\s*rev\s*=\s*")'
    r'([0-9a-f]{40})("\s*\})'
)


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


def prepare(output):
    refs = subprocess.check_output(
        ["git", "ls-remote", "--tags", "https://github.com/LyraCoreProject/wire-harness",
         "v0.2.0-conformance.*"],
        text=True,
    )
    tag, sha = latest_tag(refs)
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
    with open(output, "a") as stream:
        stream.write(f"new={str(changed).lower()}\ntag={tag}\nsha={sha}\n")


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


def describe(body_path, output):
    before = json.loads(subprocess.check_output(
        ["git", "show", "origin/main:conformance/tier1-baseline.json"], text=True,
    ))
    after = json.loads((ROOT / "conformance/tier1-baseline.json").read_text())
    changes = signature_changes(before, after)
    old_scenarios = {entry["scenario"] for entry in before["scenarios"]}
    new_scenarios = {entry["scenario"] for entry in after["scenarios"]}
    catalogue_changes = sorted(old_scenarios ^ new_scenarios)
    pin = next(line for line in (ROOT / ".wire-harness-rev").read_text().splitlines()
               if line and not line.startswith("#"))
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
    Path(body_path).write_text("\n".join(lines))
    with open(output, "a") as stream:
        stream.write(f"human_review={str(bool(changes or catalogue_changes)).lower()}\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("prepare", "describe"))
    parser.add_argument("--output", required=True)
    parser.add_argument("--body")
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.output)
    else:
        if not args.body:
            parser.error("describe requires --body")
        describe(args.body, args.output)


if __name__ == "__main__":
    main()
