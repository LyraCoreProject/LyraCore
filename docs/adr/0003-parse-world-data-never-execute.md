# 0003. Parse the world dump; never execute it

## Status

Accepted, 2026-06-17.

## Context

`classic-db` is a MySQL dump with scripts. Running its SQL or scripts would run GPL code and would
accept whatever shape a new upstream commit has. The Module's tables are a denormalized subset of
the mangos shape, so the transform is a mapping, not a redesign.

## Decision

The importer parses the rows it uses from the dump text and writes them through the Module. It
never executes cmangos SQL or scripts. It validates what it reads and fails on a changed shape,
an unknown value or a broken reference before it writes, rather than importing a partial or
reinterpreted family.

## Consequences

- Schema drift upstream stops the import with a named error instead of corrupting content.
- Each family clears and reloads as a unit, so a rerun repairs a failed one.
- Supporting a new source column or value is an importer change with its own review.
