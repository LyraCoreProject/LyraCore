#!/usr/bin/env python3
"""Check PB-011 action lifecycle source references and observed executions."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


LIFECYCLES = {"success", "waiting", "refusal", "cancellation", "expiry"}
UNREACHABLE_CONTRACTS = {
    ("Resurrect", "refusal"): "resurrection-valid-states-cannot-refuse",
    ("Resurrect", "expiry"): "resurrection-owns-no-deadline",
}


def enum_variants(source: str, enum_name: str) -> list[str]:
    match = re.search(rf"\bpub enum {re.escape(enum_name)}\s*\{{", source)
    if match is None:
        raise ValueError(f"missing {enum_name} enum")
    start = match.end()
    depth = 1
    end = start
    while end < len(source) and depth:
        depth += source[end] == "{"
        depth -= source[end] == "}"
        end += 1
    if depth:
        raise ValueError(f"unterminated {enum_name} enum")
    body = source[start : end - 1]
    variants: list[str] = []
    token = []
    nested = 0
    for character in body + ",":
        if character in "([{<":
            nested += 1
        elif character in ")]}>" and nested:
            nested -= 1
        if character == "," and nested == 0:
            text = "".join(token).strip()
            token = []
            if text:
                variant = re.match(r"([A-Z][A-Za-z0-9_]*)", text)
                if variant is None:
                    raise ValueError(f"cannot parse {enum_name} variant: {text!r}")
                variants.append(variant.group(1))
        else:
            token.append(character)
    return variants


def current_actions(package_root: Path) -> set[str]:
    source = (package_root / "playerbots/src/decision.rs").read_text()
    actions = enum_variants(source, "Action")
    moves = enum_variants(source, "MoveTarget")
    return {f"Move.{move}" for move in moves} | {
        action for action in actions if action not in {"Hold", "Move"}
    }


def load_json(path: Path) -> object:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {path}: {error}") from error


def source_case_exists(core_root: Path, case: dict[str, str]) -> bool:
    path = core_root / case["source"]
    if not path.is_file():
        return False
    function_name = case["name"].rsplit("::", 1)[-1]
    pattern = rf"\bfn\s+{re.escape(function_name)}\s*\("
    return re.search(pattern, path.read_text()) is not None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--core-root", type=Path, required=True)
    parser.add_argument("--package-root", type=Path, required=True)
    parser.add_argument("--core", required=True)
    parser.add_argument("--collection", required=True)
    parser.add_argument("--observed", type=Path, action="append", default=[])
    parser.add_argument("--source-only", action="store_true")
    args = parser.parse_args()

    manifest = load_json(args.manifest)
    if not isinstance(manifest, dict) or manifest.get("schema") != "playerbots-action-lifecycle-v1":
        raise ValueError("unsupported lifecycle manifest")
    cells = manifest.get("cells")
    if not isinstance(cells, list):
        raise ValueError("manifest cells must be an array")

    actions = current_actions(args.package_root)
    expected = {(action, lifecycle) for action in actions for lifecycle in LIFECYCLES}
    actual: set[tuple[str, str]] = set()
    references: set[tuple[str, str]] = set()
    dispositions = 0
    unreachable: set[tuple[str, str]] = set()
    for cell in cells:
        if not isinstance(cell, dict):
            raise ValueError("each lifecycle cell must be an object")
        key = (cell.get("action"), cell.get("lifecycle"))
        if key in actual:
            raise ValueError(f"duplicate lifecycle cell {key}")
        actual.add(key)
        applicability = cell.get("applicability", "required")
        if applicability not in {"required", "unreachable"}:
            raise ValueError(f"lifecycle cell {key} has invalid applicability {applicability!r}")
        if applicability == "unreachable":
            expected_contract = UNREACHABLE_CONTRACTS.get(key)
            if expected_contract is None:
                raise ValueError(f"lifecycle cell {key} cannot be marked unreachable")
            if cell.get("contract") != expected_contract:
                raise ValueError(
                    f"unreachable lifecycle cell {key} must name contract {expected_contract!r}"
                )
            reason = cell.get("reason")
            if not isinstance(reason, str) or len(reason.strip()) < 40:
                raise ValueError(f"unreachable lifecycle cell {key} needs a precise reason")
            dispositions += 1
            unreachable.add(key)
        elif "reason" in cell or "contract" in cell:
            raise ValueError(
                f"required lifecycle cell {key} cannot carry an unreachable disposition"
            )
        case = cell.get("case")
        if not isinstance(case, dict) or set(case) != {"target", "name", "source"}:
            raise ValueError(f"lifecycle cell {key} has an invalid case reference")
        if not source_case_exists(args.core_root, case):
            raise ValueError(f"missing source case {case['target']}::{case['name']} at {case['source']}")
        references.add((case["target"], case["name"]))
    guards = manifest.get("guards", [])
    if not isinstance(guards, list):
        raise ValueError("manifest guards must be an array")
    for case in guards:
        if not isinstance(case, dict) or set(case) != {"target", "name", "source"}:
            raise ValueError("lifecycle guard has an invalid case reference")
        if not source_case_exists(args.core_root, case):
            raise ValueError(f"missing source guard {case['target']}::{case['name']} at {case['source']}")
        references.add((case["target"], case["name"]))
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise ValueError(f"lifecycle matrix mismatch; missing={missing}, extra={extra}")
    if unreachable != set(UNREACHABLE_CONTRACTS):
        raise ValueError(
            "unreachable lifecycle contracts differ; "
            f"missing={sorted(set(UNREACHABLE_CONTRACTS) - unreachable)}, "
            f"extra={sorted(unreachable - set(UNREACHABLE_CONTRACTS))}"
        )

    if args.source_only:
        if args.observed:
            raise ValueError("--source-only cannot be combined with --observed")
    else:
        if not args.observed:
            raise ValueError("at least one --observed manifest is required")
        observed: dict[tuple[str, str], str] = {}
        for path in args.observed:
            record = load_json(path)
            if not isinstance(record, dict) or record.get("schema") != "playerbots-test-runs-v1":
                raise ValueError(f"unsupported observed manifest {path}")
            if (record.get("core"), record.get("collection")) != (args.core, args.collection):
                raise ValueError(f"observed manifest {path} has different source pins")
            for run in record.get("runs", []):
                if not isinstance(run, dict):
                    raise ValueError(f"observed manifest {path} has a non-object run")
                key = (run.get("target"), run.get("name"))
                status = run.get("status")
                if key in observed and observed[key] != status:
                    raise ValueError(f"conflicting observed status for {key}")
                observed[key] = status
        missing_runs = sorted(reference for reference in references if observed.get(reference) != "passed")
        if missing_runs:
            raise ValueError(f"lifecycle source cases did not pass at the final pins: {missing_runs}")

    print(json.dumps({
        "schema": manifest["schema"],
        "actions": len(actions),
        "cells": len(actual),
        "source_cases": len(references),
        "unreachable_contracts": dispositions,
        "guards": len(guards),
        "execution_checked": not args.source_only,
        "core": args.core,
        "collection": args.collection,
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
