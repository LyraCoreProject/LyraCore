# Issue #312 spatial Import Families, broken into tickets

Source: `gh issue view 312`, "Package Deltas: support spatial creature and game-object families",
plus the maintainer scope note of 2026-08-27 on the issue.

EventAI (`creature-ai`) claims are OUT: they moved to their own follow-up issue. What is left is the
two spatial Import Families the dump ETL already loads, `creatures` and `gameobjects`, and the
mechanism that makes a Claim on a map-owned row reach only the World Shards whose World Import Scope
owns that map.

**This chain stacks on #311.** Branch `feat/312-package-delta-spatial` is cut from
`origin/feat/311-package-delta-remaining-families`, which today carries only the #311 tickets. Every
ticket here must rebase onto the #311 chain head once that chain has landed its code: #311 and #312
append to the same closed enums in the same files, and #312 assumes #311's update-only refusal
(`DeltaError::InsertNotSupported`) already exists.

## Execution order

Strictly sequential, for the same reason #311 is: every family appends to the same closed enums.

```text
T1 (creatures + the World Import Scope claim filter)
 └── T2 (gameobjects)
      └── T3 (cross-Shard preflight)
           └── T4 (verification and PR prep)
```

| # | Ticket | Model | Tables | New ranges |
|---|--------|-------|--------|------------|
| T1 | `creatures` and the scope filter | opus | 2 | Package Creature Range, Package Creature Spawn Range |
| T2 | `gameobjects` | sonnet | 3 | Package Gameobject Range |
| T3 | cross-Shard preflight | sonnet | 0 | none |
| T4 | verification and PR prep | sonnet | 0 | none |

T1 takes opus: it settles the spatial key shape, two ranges whose ceilings are pinned by guid layout
bits rather than by the decade convention, and the routing filter every later ticket reuses.

## The tables, and the v1 policy for each

#311's insert rule still decides the operations:

> A table whose primary key names an entity another Import Family owns is UPDATE-ONLY in v1. A table
> with its own key, or keyed by an identifier its own family may invent, permits INSERT inside that
> family's Package Identifier Range, and UPDATE.

Nothing here is update-only: both families own their own entities.

| Table | Family | Key | v1 |
|---|---|---|---|
| `game_creature_template` | `creatures` | `entry: u32` | insert + update |
| `game_creature_spawn` | `creatures` | `guid: u64`, derived from `(entry, spawn_index)` | insert + update |
| `game_gameobject_template` | `gameobjects` | `entry: u32` | insert + update |
| `game_gameobject` (the GO spawn) | `gameobjects` | `guid: u64`, derived from `spawn_id` | insert + update |
| `game_gameobject_trap` | `gameobjects` | `entry: u32`, a GO template entry | insert + update |

### Named gaps, deliberately not in the catalogue

- **`game_creature_waypoint`.** Its key is a surrogate `id` the importer assigns in dump order, so it
  has no stable identity an author can name, and its row states no map, only a `creature_guid`,
  so a claim on it cannot be routed from the artifact alone. Same shape as the
  `game_creature_quest` exclusion the quest family already documents.
- **`game_gameobject_pool` and `game_gameobject_pool_member`.** These tables exist, but no dump ETL
  writes them: rows come only from `module/src/seed.rs` and the debug reducers. A Package Delta apply
  replays after a base import, and these tables have no base import behind them, so there is nothing
  to reapply onto.
- **Creature pools.** They have no durable representation at all. cmangos `creature.id = 0` pool
  slots are resolved through `creature_spawn_entry` and `spawn_group`/`spawn_group_spawn` at ETL
  time, and reach the Module as ordinary standalone `game_creature_spawn` rows. No `pool_id`
  survives the import, so there is no pool row for a Package to claim.

## Key shapes

A spawn's durable primary key is a packed 64-bit guid, far above the 2^53 a JSON number carries
exactly. A claim therefore names the guid's COMPONENTS and the key derives the packed value, the same
way `packed_spell_effect_id` derives `game_spell_effect.id` from `(spell_id, effect_index)`.

```text
creature spawn guid = (0xF130 << 48) | (entry << 24) | spawn_index
    key members: { entry: u32, spawn_index: u32 }     both < 2^24

gameobject spawn guid = (0xF110 << 48) | spawn_id
    key member: { spawn_id: u64 }                     < 2^48, so a JSON number holds it exactly
```

Both formulas are already load-bearing: `world_guid` and `go_guid` in `importer/src/main.rs`, and
`encounter::wave_guid` in the Module. State both sources in the doc comment, as #311 requires.

## Identifier ranges

Three new ranges. Every one of them has a third assert the earlier families never needed: a spawn
identifier is a FIELD inside a packed guid, so a bit in the guid layout, not the decimal decade
convention, decides how high the range may go.

| Range | Value | Checked against | Ceiling pinned by |
|---|---|---|---|
| Package Creature Range | 15,000,000 to 15,999,999 | `game_creature_template.entry`, and a creature spawn key's `entry` | `<= 0xFF_FFFF`: the guid carries the entry in bits 24..47 |
| Package Creature Spawn Range | 6,000,000 to 6,999,999 | a creature spawn key's `spawn_index` | `< 0x80_0000`: bit 23 of the low is `instance::INSTANCE_POP_LOW_BAND` |
| Package Gameobject Range | 16,000,000 to 16,999,999 | `game_gameobject_template.entry`, `game_gameobject_trap.entry`, and a GO spawn key's `spawn_id` | `< 1 << 46`: `instance::GO_COPY_BAND`, below `gameobject::POOL_TAG` at bit 47 |

Every range gets the two `const _: () = assert!(...)` lines #311 uses (floor at or under ceiling,
`RESERVED_ID_CEIL` strictly under the floor) plus the layout assert in the table above. The layout
asserts are the point: a maintainer who moves a number gets a compile error rather than a Package
creature that the instance code mistakes for an instance-population copy.

Two notes a reader will otherwise trip on, and which belong in the doc comments:

- **The Package Creature Spawn Range reuses a decade on purpose.** 6,000,000 is also the Package
  Spell Range's floor, and that is not a collision: a spawn index is a 24-bit FIELD inside a guid, not
  a row identifier, so the decade convention (which exists so the millions column names a family
  across ROW identifier spaces) has nothing to say about it. The binding constraint is bit 23, which
  caps any spawn index at 8,388,607 and makes a 15,000,000-shaped number unspellable. 6,000,000 is
  the readable number that clears bit 23, still clears the reserved span so the range keeps the same
  two asserts every other range has, and sits far above every real cmangos spawn guid and every
  seeded or debug low.
- **The Package Gameobject Range covers three tables and cannot collide across them.** A template
  entry, a trap entry and a spawn id live in independent key spaces, exactly the argument the Package
  Loot Range already makes for its four tables.

## The routing mechanism

**The claim filter lives in the importer, inside the Package Delta stage, and reuses the World Import
Scope the base import already built.** No new routing concept, and the Module stays scope-blind.

Why that is enough, and why it is the only place it can live:

- One importer invocation targets one Shard (`--db`), with one World Import Profile
  (`--world-profile`). `Args::world_import_scope()` rebuilds the same `WorldImportScope` anywhere in
  the run, and `run_package_stage` runs inside `main()` with `args` and the finished `DumpPlan` in
  hand. Nothing new has to be threaded to reach the scope.
- The Module must not learn about maps-per-Shard. `module/src/region.rs` and
  `partition_discipline_tripwire` make it a build failure for Module game logic to read a shard id,
  and `CODING_STANDARDS.md` says the Module stays shard-agnostic. So the Module keeps its existing
  strict behaviour: an `update` whose target row is absent refuses the whole plan.
- Those two facts together force the filter into the importer. Without it, a Package that tunes a
  spawn on map 1 would REFUSE the whole import of every Shard that does not own map 1, and a Package
  that inserts a spawn on map 1 would land it on every Shard.

The filter, in one sentence: a spatial claim is sent to this Shard when the base import's own
in-scope predicate accepts the row the claim describes.

- **Creature spawn**: `creature_row_kept_in_scope(&scope, map, x, y, z, entry)`: the same function
  `build_dump_plan` uses. Forced creature dependencies and the whole-map fence come along for free,
  which is what the acceptance criterion "forced creature dependencies and whole-map ownership follow
  the base import rules" asks for.
- **Gameobject spawn**: `scope.contains_map(map) && scope.contains(map, x, y, z)`, the predicate the
  GO block of the ETL uses.
- **An update** is judged on the map fence alone when it does not move the row: the row is only on
  this Shard because the base import put it there. When the update DOES state a new `x`/`y`/`z`, the
  new position must also be in scope, so a Package cannot walk a spawn off the Shard that owns it.
- **A template claim** rides its spawns: it is in scope when this run's imported template entry set
  holds the entry, or when a spawn claim this same plan keeps names it. That is the base import's own
  rule ("a template imports because something in scope spawns it") applied to Package rows.
- **Dropped claims are printed, never silent.** The plan report gains one line per dropped claim
  naming the table, the key and the World Import Scope that did not want it.

Because a Package-invented template and its spawns must be judged together, templates and spawns
share one Import Family. That is also what lets `check_references` accept a spawn whose template the
same plan inserts.

## What each ticket adds, end to end

The #311 checklist still applies, with two spatial additions:

1. `Table` variants, column lists and the family name const in `schema.rs`.
2. `PrimaryKey` variants, constructors, `row_id`, `Display`, `parse_key`, `is_key_column` and
   `check_inventable` arms in `delta.rs`; refusals in `error.rs`.
3. The Package Identifier Ranges and their `const` asserts in `ids.rs`.
4. A `ClaimFamily` variant plus `module/src/package_import/<family>.rs`.
5. The family name into `PACKAGE_DELTA_DUMP_FAMILIES` in `importer/src/main.rs`.
6. Tests at every rung.
7. **New:** the `WorldImportScope` claim filter and its plan-report lines.
8. **New:** a `partition_discipline_tripwire::WHITELIST` entry for the new Module file. Both
   `game_creature_spawn` and `game_gameobject` are watched spatial accessors, and
   `clear_package_range` scans them by definition, the same justification
   `module/src/creatures/spawn.rs` and `module/src/gameobject.rs` already carry for their import
   reducers.

## Shared rules

- Follow `CODING_STANDARDS.md`. Match the surrounding code's idiom, not its comment density.
- Use `CONTEXT.md` terms. Add one **Package Identifier Range** entry per new range in the same change
  that introduces it, and sharpen **World Import Scope** in the change that gives it a claim filter.
- Column lists are hand-maintained against the Module struct, in declaration order, with matching
  names and types. A key column, and any column the key already names, stays out of `columns()`.
- Do not change canonical form, `apply_package_deltas`'s signature, or any existing refusal message.
  `crates/lyracore-package-delta/tests/canonical_form.rs` pins one artifact's exact bytes.
- No Module durable schema change. Nothing here adds or renames a column.

## Out of scope for the whole plan

- **EventAI / `creature-ai` claims.** Split into their own issue by the maintainer.
- **Terrain, navigation, vmap and model files.** The issue says so.
- **New routing concepts outside World Import Scope.** The issue says so, and the plan above needs
  none.
- **`packages replay` generalization.** The CLI verb in the sibling `lyracore-cli` repository is
  still hardcoded to the spell family. Same follow-up #311 named. It matters more here: see the
  standing hazard below.
- **Base Snapshots for the spatial families.** The convention is settled (one file per family), but
  the only snapshot that exists today is the spell family's, and #311 shipped five families without
  one. Same follow-up.
- **Authoring-library helpers.** `datascripts/lib/authoring.ts` still exposes `.spell(id)` alone.
- **A dump-aware preflight.** T3's preflight reads artifacts and the World Import Profiles, and no
  dump. Checking a claim against the real imported template entry set needs `--dump` and is a second
  step.

## One standing hazard to carry forward

`encounter::spawn_wave` mints a wave add's guid low as `max(low of this entry) + 1`, and writes a
`game_creature_spawn` row for it. If a Package spawns entry E at low 6,000,000, a later wave add of
E takes low 6,000,001, inside the Package Creature Spawn Range, which
`creatures::clear_package_range` deletes.

It is harmless today: `apply_package_deltas` for the `creatures` family is only reachable from the
importer's reapply stage, which runs immediately after `import_creature_spawns` deleted every spawn
row and every creature entity, so no wave row can be standing. Record it in
`module/src/package_import/creatures.rs`'s doc comment as a constraint on the `packages replay`
follow-up: a realm-wide replay for this family must either skip live rows or move the wave allocator
off the band.
