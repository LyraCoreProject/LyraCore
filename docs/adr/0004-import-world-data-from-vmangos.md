# 0004. Import world data from vmangos

## Status

Draft, 2026-10-26. The port keeps classic-db as the default until its parity checks pass.
On completion, this supersedes [ADR 0002](./0002-opt-in-pinned-classic-db-pull.md) and amends
[ADR 0003](./0003-parse-world-data-never-execute.md) for SQLite. [ADR 0001](./0001-ship-tools-not-data.md)
stays unchanged.

## Context

The planned 1.12 conformance checks use vmangos packet traces as their reference. Using vmangos as
the World Source lets those checks compare gameplay against content from the same project.
vmangos replaces its `db_latest` release in place, so an old release asset may disappear.
The maintainer accepts that loss of byte reproducibility.

## Decision

Read `sqlite-dump/mangos.sqlite` from the current `db-sqlite-<commit>.zip` release asset. Verify
GitHub's asset digest before extraction. Keep downloads under the ignored `.import/` directory.
Open SQLite read-only and read columns by name with checked types. Never execute source SQL or
scripts. The classic-db text reader also reads columns by name while both sources coexist.

Select patch 10 and client build 5875 in one module. Versioned rows use the greatest eligible
revision, ranges include both endpoints, and exact-build tables require build 5875. The same
module owns forbidden-item and WOW_PATCH condition selection.

The lock records the accepted vmangos commit and the migration fingerprint, the greatest migration
id and the number of migrations. It does not pin file bytes. A newer snapshot must pass a Source
Profile that checks consumed column names and types and approved behavior values. Count changes
are reported, not used as an admission rule. Import provenance records the actual commit read.
The existing EventAI Source Profile keeps its classic-db contract until that gate is replaced.

Milestone 1 implements fetch in `importer/scripts/pull-vmangos.py`, using Python's standard library
for the later CLI's script-based fetch step. `vmangos.lock` beside it records the accepted commit
and migration fingerprint. Fetch verifies the GitHub digest and records the actual snapshot in
`.import/vmangos/current.json`; it does not evaluate the Source Profile. `--source vmangos` currently
permits the globals census and refuses family import until the later slices are ready.

vmangos supplies world content, the server spell catalogue and its replacements for client DBC
tables. The Operator's client supplies the other tables and geometry. A table must have one
source; missing columns stop its port. The spell Base Snapshot uses the same selected vmangos
spell rows as the import. Package spells still reach the client through its own `Spell.dbc`.

Preserve every behavior the classic-db import supports. Record additional source behavior that
the Module cannot express as unsupported in the Compatibility Manifest. Source condition numbers
translate to LyraCore-owned kinds. Implement the extra spell coefficients, custom flags and target
level rule independently; never copy vmangos code.

## Consequences

- The Operator fetches data locally after consent. The later CLI change must state that GPL-2.0
  migrations from `vmangos/core` are applied to base data from `brotalnia/database`, which has no
  license file. LyraCore distributes the tools and no world data.
- A changed snapshot can stop an import until its column contract or values receive review.
  A passed gate records which commit was imported even when it differs from the accepted lock.
- Source spawn identifiers can change. Moving an existing Realm and reviewing Package spatial
  claims require separate work; this port does not migrate a deployed Realm.
- Per-family censuses compare counts, entry sets, consumed floors and Compatibility Manifest
  findings. They make source differences reviewable before classic-db is removed.
