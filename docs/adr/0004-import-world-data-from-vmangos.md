# 0004. Import world data from vmangos

## Status

Accepted, 2026-10-27. Supersedes [ADR 0002](./0002-opt-in-pinned-classic-db-pull.md) and amends
[ADR 0003](./0003-parse-world-data-never-execute.md) for SQLite.
[ADR 0001](./0001-ship-tools-not-data.md) stays unchanged.

## Context

The planned 1.12 conformance checks use vmangos packet traces as their reference. Using vmangos as
the World Source lets those checks compare gameplay against content from the same project.
vmangos replaces its `db_latest` release in place, so an old release asset may disappear.
The maintainer accepts that loss of byte reproducibility.

## Decision

vmangos is the sole World Source. Read `sqlite-dump/mangos.sqlite` from the current
`db-sqlite-<commit>.zip` release asset. Verify GitHub's asset digest before extraction. Keep downloads
under the ignored `.import/` directory. Open SQLite read-only with trusted schemas disabled and
read named columns with checked types. Never execute source SQL or scripts.

Select patch 10 and client build 5875 in one module. Versioned rows use the greatest eligible
revision, ranges include both endpoints, and exact-build tables require build 5875. The same
module owns forbidden-item and WOW_PATCH condition selection.

The lock records the accepted vmangos commit and the migration fingerprint, the greatest migration
id and the number of migrations. It does not pin file bytes. The Source Profile checks consumed
column names and types and approved behavior values. Count changes are informational. An admitted
import records the actual commit supplied by fetch, even when it differs from the lock.

`importer/scripts/pull-vmangos.py` performs the fetch and records the actual snapshot in
`.import/vmangos/current.json`. `vmangos.lock` records the accepted snapshot.
`importer/manifests/vmangos-4641790.json` defines the column and value contracts. Fetch verifies
integrity; the importer evaluates the Source Profile. A missing column holds back its table.

vmangos supplies world content, the server spell catalogue and its replacements for client DBC
tables. The Operator's client supplies the other tables and geometry. Each table has one source,
with one exception: `map_template` supplies Map gameplay columns, while `Map.dbc` supplies only
`internal_name`, joined by map id, for WDT and ADT archive lookup. A missing client Map row refuses
the whole Map catalogue before any rows are replaced. This keeps a missing archive name from
silently removing a map during a re-import. Stored player limits, reset delays and ghost entrances await gameplay support.

The spell Base Snapshot uses the same selected vmangos rows as the server import. Package spells
still reach the client through the player's own `Spell.dbc`. Source condition numbers translate
to LyraCore-owned kinds. Spell coefficients, custom flags and target-level rules are independently
implemented. Source code is read as a reference and never copied.

The port preserves supported behavior where vmangos supplies equivalent data. Accepted omissions
include creatures whose behavior lives in C++ AI, absent source rules and refused translations.
The generated per-creature ledger records causes and marks playable content. Additional unsupported
source behavior appears in the Compatibility Manifest and the unsupported behavior ledger.

A source event flag adds a quest requirement only when the scoped import emits working EventAI or
script credit. Otherwise normal objectives retain their completion path. Previously script-gated
quests 2460 and 2608 retain their gates; their unavailable scenes remain explicit gaps. Generic
script lists reached from accepted EventAI run, but standalone quest, gossip, event, movement and
GameObject script dispatch remains unsupported.

## Consequences

- The Operator fetches locally after consent. The notice identifies GPL-2.0 migrations from
  `vmangos/core` applied on top of base data from `brotalnia/database`, which has no license file.
  LyraCore distributes tools and no world or client data.
- A changed snapshot can stop an import until its column contract or values receive review.
  An admitted snapshot remains traceable through Import Revisions.
- Source spawn identifiers change. Package API version 5 is an unreleased draft requiring spatial
  claims to be rebuilt against the new Import Revision. The maintainer owns collection tagging.
  This port does not migrate a deployed Realm.
- Generated censuses explain count and identity differences. Import floors use those measurements;
  passing a floor does not establish complete gameplay or client acceptance.
- Classic-db's reader, fixtures, fetch and Source Profile are retired. The earlier ADRs remain as
  the record of the replaced decision.
