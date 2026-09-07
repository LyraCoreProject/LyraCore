#!/usr/bin/env python3
"""Regenerate every Gateway binding and compare it with the committed tree."""

import argparse
import difflib
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


SPACETIME_VERSION = "2.7.1"
GENERATED_RELATIVE_PATH = Path("gateway/src/stdb/bindings")
IDENTIFIER_FIXES = {
    "aura_type.rs": {
        "eff_p_0_kind": "eff_p0_kind",
        "eff_p_0": "eff_p0",
        "eff_p_1": "eff_p1",
    },
    "gossip_menu_profile_option_type.rs": {
        "cond_value_1": "cond_value1",
        "cond_value_2": "cond_value2",
    },
    "gossip_option_type.rs": {
        "cond_value_1": "cond_value1",
        "cond_value_2": "cond_value2",
    },
}
FACING_FIELDS = "    pub facing: bool,\n    pub facing_angle: f32,"
FACING_FIELDS_WITH_COMMENT = """    // Hand-appended under the END-append rule in docs/danger-zones.md: a facing-only leg does not
    // move (`sx/sy/sz == dx/dy/dz`, `dur_ms == 0`). The angle is ignored unless `facing` is true.
    pub facing: bool,
    pub facing_angle: f32,"""


@dataclass(frozen=True)
class Drift:
    missing: tuple[Path, ...]
    unexpected: tuple[Path, ...]
    changed: tuple[Path, ...]

    def found(self):
        return bool(self.missing or self.unexpected or self.changed)


def files_below(root):
    return {path.relative_to(root) for path in root.rglob("*") if path.is_file()}


def find_drift(committed, generated):
    committed_files = files_below(committed)
    generated_files = files_below(generated)
    common = committed_files & generated_files
    return Drift(
        missing=tuple(sorted(committed_files - generated_files)),
        unexpected=tuple(sorted(generated_files - committed_files)),
        changed=tuple(
            path
            for path in sorted(common)
            if (committed / path).read_bytes() != (generated / path).read_bytes()
        ),
    )


def replace_generator_identifier(path, generated, committed):
    generated_token = re.compile(rf"\b{re.escape(generated)}\b")
    text = path.read_text()
    if generated_token.search(text):
        path.write_text(generated_token.sub(committed, text))
    elif not re.search(rf"\b{re.escape(committed)}\b", text):
        raise RuntimeError(f"{path.name}: expected {generated} or {committed}")


def apply_documented_exceptions(generated):
    for relative, fixes in IDENTIFIER_FIXES.items():
        path = generated / relative
        for generated_name, committed_name in fixes.items():
            replace_generator_identifier(path, generated_name, committed_name)

    spline = generated / "creature_spline_type.rs"
    text = spline.read_text()
    if FACING_FIELDS_WITH_COMMENT not in text:
        if text.count(FACING_FIELDS) != 1:
            raise RuntimeError(
                "creature_spline_type.rs: expected one facing/facing_angle field pair"
            )
        spline.write_text(text.replace(FACING_FIELDS, FACING_FIELDS_WITH_COMMENT))


def format_generated(root, generated):
    paths = sorted(str(path) for path in generated.rglob("*.rs"))
    for start in range(0, len(paths), 100):
        subprocess.run(
            ["rustfmt", "--edition", "2021", *paths[start : start + 100]],
            cwd=root,
            check=True,
        )


def check_spacetime_version():
    result = subprocess.run(
        ["spacetime", "--version"],
        check=True,
        capture_output=True,
        text=True,
    )
    match = re.search(r"spacetimedb tool version ([0-9.]+)", result.stdout)
    found = match.group(1) if match else "unknown"
    if found != SPACETIME_VERSION:
        raise RuntimeError(
            f"spacetime {SPACETIME_VERSION} is required; found {found}"
        )
    print(f"Using spacetime {found}", flush=True)


def generate_bindings(root, output):
    check_spacetime_version()
    result = subprocess.run(
        [
            "spacetime",
            "generate",
            "--lang",
            "rust",
            "--out-dir",
            str(output),
            "--module-path",
            str(root / "module"),
            "--include-private",
            "--build-options=--features=debug_reducers",
            "-y",
        ],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise RuntimeError("spacetime generate failed")


def print_drift(drift, committed, generated):
    for path in drift.missing:
        print(f"Missing generated binding: {path}", file=sys.stderr)
    for path in drift.unexpected:
        print(f"Uncommitted generated binding: {path}", file=sys.stderr)
    for path in drift.changed:
        print(f"Changed generated binding: {path}", file=sys.stderr)
        before = (committed / path).read_text().splitlines(keepends=True)
        after = (generated / path).read_text().splitlines(keepends=True)
        diff = difflib.unified_diff(
            before,
            after,
            fromfile=f"committed/{path}",
            tofile=f"generated/{path}",
        )
        for line in list(diff)[:200]:
            print(line, end="", file=sys.stderr)


def check(root, committed, supplied_generated=None):
    with tempfile.TemporaryDirectory(prefix="lyracore-gateway-bindings-") as directory:
        generated = Path(directory) / "bindings"
        if supplied_generated:
            shutil.copytree(supplied_generated, generated)
        else:
            generate_bindings(root, generated)
        apply_documented_exceptions(generated)
        drift = find_drift(committed, generated)
        if drift.missing or drift.unexpected:
            print_drift(drift, committed, generated)
            return 1
        format_generated(root, generated)
        drift = find_drift(committed, generated)
        if drift.found():
            print_drift(drift, committed, generated)
            return 1
        print(f"All {len(files_below(committed))} Gateway binding files are current")
        return 0


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--generated-dir",
        type=Path,
        help="Check an existing generated tree instead of invoking spacetime",
    )
    parser.add_argument(
        "--bindings-dir",
        type=Path,
        default=root / GENERATED_RELATIVE_PATH,
        help="Committed binding tree",
    )
    args = parser.parse_args()
    return check(root, args.bindings_dir, args.generated_dir)


if __name__ == "__main__":
    raise SystemExit(main())
