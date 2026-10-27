#!/usr/bin/env bash
# The fetch report describes the downloaded snapshot; the lock may name an older one.
if ! command -v python3 >/dev/null 2>&1; then
    echo "vmangos source: python3 is required to validate the snapshot" >&2
    return 1
fi
vmangos_source_info="$(python3 - <<'PY'
import json
import os
from pathlib import Path
import re
import sys

try:
    path = os.environ.get("DUMP")
    commit = os.environ.get("SOURCE_SHA")
    if not path and not commit:
        report = json.loads(Path(".import/vmangos/current.json").read_text())
        path, commit = report["sqlite_path"], report["commit"]
    if not path or not commit or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("set DUMP and SOURCE_SHA together, or run pull-vmangos.py")
    if not Path(path).is_file() or "\n" in path:
        raise ValueError(f"invalid SQLite path: {path}")
    print(path)
    print(commit)
except (OSError, ValueError, KeyError) as error:
    sys.exit(f"vmangos source: {error}")
PY
)" || return 1
export DUMP="${vmangos_source_info%$'\n'*}"
export SOURCE_SHA="${vmangos_source_info##*$'\n'}"
unset vmangos_source_info
