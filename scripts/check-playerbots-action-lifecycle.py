#!/usr/bin/env python3
"""Check PB-011 action lifecycle source references and observed executions."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path, PurePosixPath


LIFECYCLES = {"success", "waiting", "refusal", "cancellation", "expiry"}
UNREACHABLE_CONTRACTS = {
    ("Move.AreaTrigger", "expiry"): "areatrigger-transfer-approach-owns-no-deadline",
    ("Resurrect", "refusal"): "resurrection-valid-states-cannot-refuse",
    ("Resurrect", "expiry"): "resurrection-owns-no-deadline",
    ("Transfer", "expiry"): "transfer-approach-owns-no-deadline",
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


def rust_code(source: str, *, keep_literals: bool = False) -> str:
    """Blank Rust comments and literals with the boundary used by module/build.rs."""
    output = list(source)
    index = 0

    def blank(start: int, end: int) -> None:
        for position in range(start, end):
            if output[position] != "\n":
                output[position] = " "

    while index < len(source):
        if source.startswith("//", index):
            end = source.find("\n", index)
            end = len(source) if end < 0 else end
            blank(index, end)
            index = end
        elif source.startswith("/*", index):
            depth = 0
            end = index
            while end < len(source):
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                    if depth == 0:
                        break
                else:
                    end += 1
            blank(index, end)
            index = end
        else:
            previous_is_identifier = index > 0 and (
                source[index - 1].isalnum() or source[index - 1] == "_"
            )
            raw = (
                None
                if previous_is_identifier
                else re.match(r'(?:br|cr|r)(?P<hashes>#+)?"', source[index:])
            )
            if raw is not None:
                delimiter = '"' + (raw["hashes"] or "")
                end = source.find(delimiter, index + raw.end())
                end = len(source) if end < 0 else end + len(delimiter)
                if not keep_literals:
                    blank(index, end)
                index = end
                continue
            quoted = source[index] == '"' or (
                not previous_is_identifier and source.startswith('b"', index)
            )
            if quoted:
                end = index + (2 if source.startswith('b"', index) else 1)
                while end < len(source):
                    if source[end] == "\\":
                        end += 2
                    elif source[end] == '"':
                        end += 1
                        break
                    else:
                        end += 1
                if not keep_literals:
                    blank(index, min(end, len(source)))
                index = min(end, len(source))
                continue
            character = re.match(
                r"(?:b)?'(?:\\(?:u\{[0-9A-Fa-f_]+\}|x[0-9A-Fa-f]{2}|.)|[^\\'\n])'",
                source[index:],
            )
            if character is not None and (not previous_is_identifier or source[index] == "'"):
                end = index + character.end()
                if not keep_literals:
                    blank(index, end)
                index = end
            else:
                index += 1
    return "".join(output)


def top_level(source: str, position: int) -> bool:
    depth = 0
    for character in source[:position]:
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
    return depth == 0


def has_test_function(source: str, function_name: str) -> bool:
    stripped = rust_code(source)
    functions = re.finditer(
        rf"(?P<attributes>(?:#\s*\[[^]]*\]\s*)+)"
        rf"(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(function_name)}\s*\(",
        stripped,
    )
    return any(
        top_level(stripped, match.start())
        and re.search(r"#\s*\[\s*test\s*\]", match["attributes"])
        for match in functions
    )


def declares_external_module(root_source: str, relative_source: PurePosixPath, module: str) -> bool:
    stripped = rust_code(root_source)
    comments_removed = rust_code(root_source, keep_literals=True)
    declaration = re.compile(
        rf"#\s*\[\s*path\s*=\s+\]\s*mod\s+{re.escape(module)}\s*;"
    )
    for match in declaration.finditer(stripped):
        declaration_source = comments_removed[match.start() : match.end()]
        path = re.search(r'path\s*=\s*"([^"\\]*)"', declaration_source)
        if (
            top_level(stripped, match.start())
            and path is not None
            and PurePosixPath(path.group(1)) == relative_source
        ):
            return True
    return False


def source_case_exists(core_root: Path, case: dict[str, str]) -> bool:
    identifier = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
    if any(not isinstance(case.get(field), str) for field in ("target", "name", "source")):
        return False
    target = case["target"]
    names = case["name"].split("::")
    source = PurePosixPath(case["source"])
    if (
        not identifier.fullmatch(target)
        or not names
        or any(not identifier.fullmatch(name) for name in names)
        or source.is_absolute()
        or ".." in source.parts
        or len(source.parts) < 3
        or source.parts[0] not in {"module", "gateway"}
        or source.parts[1] != "tests"
        or source.suffix != ".rs"
    ):
        return False
    tests_root = PurePosixPath(source.parts[0], "tests")
    target_source = tests_root / f"{target}.rs"
    path = core_root.joinpath(*source.parts)
    root_path = core_root.joinpath(*target_source.parts)
    if not path.is_file() or not root_path.is_file():
        return False
    if source == target_source:
        if len(names) != 1:
            return False
    else:
        if len(names) != 2:
            return False
        relative_source = source.relative_to(tests_root)
        if not declares_external_module(root_path.read_text(), relative_source, names[0]):
            return False
    return has_test_function(path.read_text(), names[-1])


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
