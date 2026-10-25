# 0002. Pull classic-db with an opt-in, pinned script

## Status

Accepted, 2026-06-17.

## Context

[ADR 0001](./0001-ship-tools-not-data.md) keeps world data out of the repository, so the Operator
must fetch it. A git submodule would put cmangos content in the repository graph and fetch it for
every clone. A floating `master` changes under the importer, and the EventAI compatibility gate
needs the exact source it was reviewed against.

## Decision

`importer/scripts/pull-classic-db.sh` fetches `classic-db` only when the Operator runs it, and
assembles the dump under `.import/` without executing any of its SQL. `importer/scripts/classic-db.lock`
pins the commit and the SHA-256 of the decompressed SQL, and the script refuses a mismatch. A
different ref needs `--skip-verify`. The EventAI Source Profile in
`importer/manifests/cmangos-classic-z2815.json` pins the same snapshot.

## Consequences

- A casual clone never downloads Blizzard-derived content.
- Imports are reproducible from the lock, and moving the pin is a reviewed change to two files.
- A floating fetch can still be imported, but it cannot pass EventAI apply until a matching
  profile is reviewed and committed.
