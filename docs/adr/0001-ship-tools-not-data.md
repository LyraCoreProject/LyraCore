# 0001. Ship the tools, never the world data

## Status

Accepted, 2026-06-17.

## Context

A vanilla Realm needs two kinds of data. Client data (spells, talents, areas, factions, terrain,
model geometry) lives in the 1.12.1 client's MPQ archives, and Blizzard owns it. World content
(creatures, spawns, items, loot, quests, vendors, trainers) comes from cmangos `classic-db`, a
GPL-3.0 dump of content that is still Blizzard's. GPL obligations start at distribution, not at
use. Data compiled into the Module could make the Module a derivative work. The repository is
public, so Blizzard's copyright, not the GPL, is the larger risk.

## Decision

The repository ships zero third-party or Blizzard-derived bulk data, optional or otherwise. It
ships the importer, the pull script and the ETL under `importer/`. The Operator fetches
`classic-db` to their own disk and reads client data from their own client with `wow-mpq` and
`wow_dbc`. No third-party data is ever compiled into the Module. `lyracore import` states this and
requires an explicit `yes` or `--accept` before it fetches or reads anything.

The seeded fixture is a narrow exception. It is hand-authored one row at a time and reuses real
vanilla ids, and for a few rows real names and coordinates, so the client renders them and a later
import does not collide with invented ids. `module/src/seed.rs` holds it.

## Consequences

- A fresh checkout gives only the fixture. A full world exists only where an Operator ran the
  import against their own client and dump.
- `.import/` is gitignored. No database built by the pipeline is distributed or hosted by the
  project; what an Operator does with theirs is their responsibility.
- Rule data that exists only in GPL content stays out of the repository. The starter aura
  families in `seed_spell_groups` are hand-authored and checked against client data; cmangos
  `spell_group` rows are not transcribed.
- `--source-sha` and `game_import_meta` record what each import loaded, from which upstream commit.
