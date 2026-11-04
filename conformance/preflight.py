import os
import re
import subprocess
import tomllib


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
