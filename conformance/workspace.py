import os
from pathlib import Path
import re
import subprocess
import tomllib


TIER1_TESTS = "module/tests/conformance/tier1.rs"
TIER1_TEST_SUFFIX = "_matches_its_golden_trace"
TIER1_TEST = re.compile(rf"\bfn ([a-z0-9_]+){TIER1_TEST_SUFFIX}\(")


def harness_pin(root):
    """Return the (tag, sha) pair from `.wire-harness-rev`."""
    line = next(line for line in (root / ".wire-harness-rev").read_text().splitlines()
                if line.strip() and not line.startswith("#"))
    tag, sha = line.split()
    return tag, sha


def scratch(task, variable=None):
    """Return `$HOME/.cache/lyracore-scratch/<task>`, or the directory `variable` names when set."""
    override = os.environ.get(variable) if variable else None
    return Path(override) if override else Path.home() / ".cache/lyracore-scratch" / task


def tier1_scratch():
    return scratch("conformance-run", "LYRACORE_CONFORMANCE_SCRATCH")


def tier1_test(scenario):
    return f"tier1::{scenario}{TIER1_TEST_SUFFIX}"


def tier1_scenarios(root):
    """Return the scenario ids that have a tier 1 test `./conformance/run` can select."""
    return set(TIER1_TEST.findall((root / TIER1_TESTS).read_text()))


def check_spacetime_version(root, executable=None):
    manifest = tomllib.loads((root / "module/Cargo.toml").read_text())
    pin = manifest["dependencies"]["spacetimedb"]["version"]
    if not re.fullmatch(r"=\d+\.\d+\.\d+", pin):
        raise RuntimeError("module/Cargo.toml must pin an exact SpacetimeDB version")
    required = pin[1:]
    executable = executable if executable is not None else os.environ.get("SPACETIME_BIN", "spacetime")
    remedy = (
        f"Install with `spacetime version install {required}`; put its bin/{required} directory "
        "first on PATH (with spacetime linked to spacetimedb-cli) and set SPACETIME_BIN "
        "to that spacetimedb-cli."
    )
    try:
        version = subprocess.run([str(executable), "--version"], capture_output=True,
                                 text=True, check=False, timeout=10)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(
            f"SpacetimeDB CLI {executable}: found unavailable; required {required}: {error}. {remedy}"
        ) from error
    output = (version.stdout + version.stderr).strip()
    match = re.search(r"spacetimedb tool version ([^;\s]+)", output)
    found = match[1] if match else "unknown"
    if version.returncode or found != required:
        raise RuntimeError(
            f"SpacetimeDB CLI {executable}: found {found}; required {required} "
            f"(module/Cargo.toml). {remedy} CLI output: {output}"
        )
