#!/usr/bin/env python3

import json
from pathlib import Path

profile = json.loads(Path("importer/manifests/vmangos-4641790.json").read_text())
columns = sum(
    len(names)
    for table in profile["tables"].values()
    for names in table["columns"].values()
)
lines = [
    "# Milestone 4 approved source values",
    "",
    f'The `{profile["name"]}` Source Profile checks {len(profile["tables"])} tables '
    f"and {columns:,} consumed columns. "
    "`importer/manifests/vmangos-4641790.json` holds the exact column names and affinities. "
    "The gate checks raw source values before patch/build selection. Approval means the importer "
    "has classified a value; it does not mean every behavior using it is implemented. "
    "Counts are reported and never gate import.",
    "",
    "Generated from the profile JSON by `audit-approved-sets.py`. Values are listed in decimal; "
    "masks are hexadecimal. A value mask permits combinations of its bits. Command and target "
    "masks apply only to the stated selector and optional destination field.",
    "",
]
for name, table in profile["tables"].items():
    groups = [
        group
        for group in ["values", "bits", "command_values", "command_bits", "target_bits"]
        if group in table
    ]
    if not groups:
        continue
    lines += [f"## `{name}`", ""]
    for group in groups:
        if group in ("values", "bits"):
            for column, allowed in table[group].items():
                kind = "values" if group == "values" else "mask"
                values = ", ".join(map(str, allowed)) if group == "values" else f"0x{allowed:x}"
                lines.append(f"- `{column}` {kind}: {values}.")
        else:
            for rule in table[group]:
                selector = (
                    f'command {rule["command"]}'
                    if "command" in rule
                    else f'target {rule["target"]}'
                )
                if "field" in rule:
                    selector += f', destination field {rule["field"]}'
                allowed = rule["allowed"]
                kind = "values" if isinstance(allowed, list) else "mask"
                values = (
                    ", ".join(map(str, allowed))
                    if isinstance(allowed, list)
                    else f"0x{allowed:x}"
                )
                lines.append(f'- {selector}, `{rule["column"]}` {kind}: {values}.')
    lines.append("")
Path(".claude/tickets/vmangos-port/approved-sets-m4.md").write_text("\n".join(lines))
