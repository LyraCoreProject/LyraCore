# T1: the creatures Import Family, and the World Import Scope claim filter

Parent: issue #312. **First slice. Blocks T2 to T4. Run alone.**
Model: opus.

**Rebase onto the #311 chain head before starting.** This branch was cut from
`origin/feat/311-package-delta-remaining-families` when that branch held only tickets. #311 and #312
append to the same closed enums in the same files, and this ticket assumes #311's
`DeltaError::InsertNotSupported` and the 10M to 14M ranges already exist.

## Goal

Give the `creatures` Import Family a Package Delta schema, and with it the one mechanism the whole
issue turns on: a Claim on a map-owned row reaches only the World Shards whose World Import Scope
owns that map.

Two tables, three new things: a derived spawn key, two identifier ranges whose ceilings are pinned by
guid layout bits, and the scope filter in the importer's Package Delta stage.

## Read first

- `.claude/tickets/issue-312/README.md`: the whole plan, the key shapes, the ranges, the routing
  design. This ticket implements its first half.
- `crates/lyracore-package-delta/src/{schema.rs,ids.rs,delta.rs,error.rs}`
- `module/src/package_import.rs` and `module/src/package_import/loot.rs` (the closest template: a
  family whose `check_references` reads merged final values)
- `module/src/creatures/spawn.rs`: `CreatureTemplate` (line 25), `CreatureSpawn` (line 526),
  `import_creature_spawns` / `load_spawn_batch` (line 1073 onward: exactly what a spawn row must be
  born holding), `current_life_seq`
- `module/src/creatures/tick/lifecycle.rs` around line 214: `pass_respawn` is what turns a spawn row
  into a live creature, and it silently does nothing when the template is missing
- `module/src/encounter.rs`: `wave_guid`, `entry_of_unit_guid`, `spawn_wave`
- `module/src/instance.rs` lines 55 to 65 and 225 to 230: `INSTANCE_POP_LOW_BAND`, `GO_COPY_BAND`
- `module/src/tripwires.rs`: `partition_discipline_tripwire`, its `SPATIAL_ACCESSORS` and `WHITELIST`
- `importer/src/world_import_scope.rs`: the whole file
- `importer/src/main.rs`: `world_guid` (line 512), `creature_row_kept_in_scope` (line 3424),
  `Args::world_import_scope` (line 1424), `build_dump_plan`'s scope block (line 3729 onward) and its
  `entries` set, `DumpPlan` (line 3687), `PACKAGE_DELTA_DUMP_FAMILIES` and `run_package_stage`
  (line 3664 onward), the two call sites at lines 5261 and 5354
- `importer/src/package_delta.rs`: the whole file

## Tables and v1 policy

Family name const: `CREATURE_FAMILY: &str = "creatures"`, the name the `--dump` importer's
`creatures` `--family` block already stamps.

### `game_creature_template` (`module/src/creatures/spawn.rs`, struct `CreatureTemplate`)

- Primary key: `entry: u32`.
- Claimable columns, in declaration order, 26 of the struct's 27: `name` (str), `subname` (str),
  `display_id` (u32), `level` (u32), `health` (u32), `faction_template` (u32), `npc_flags` (u32),
  `unit_flags` (u32), `creature_type` (u8), `creature_family` (u8), `type_flags` (u32), `rank` (u8),
  `scale` (f32), `base_attack_time_ms` (u32), `money_min` (u32), `money_max` (u32), `max_level`
  (u32), `max_level_health` (u32), `aggro_range` (u32), `damage_min` (u32), `damage_max` (u32),
  `armor` (u32), `pickpocket_loot_id` (u32), `skin_loot_id` (u32), `trainer_type` (u8),
  `trainer_class` (u8).
- v1 policy: **insert and partial update**, band checked against `entry`.

### `game_creature_spawn` (same file, struct `CreatureSpawn`)

- Primary key: `guid: u64`, DERIVED. See the key shape below.
- Claimable columns: `map_id` (u32), `x` (f32), `y` (f32), `z` (f32), `orientation` (f32),
  `movement_type` (u8), `respawn_secs` (u32).
- NOT claimable, and each for a stated reason in the doc comment:
  - `guid` and `entry`: the key names both.
  - `respawn_at`, `despawn_at`: `Timestamp` columns. The Claim schema has no timestamp type, and
    these are the respawn machinery's own state, not data. A writer sets them the way
    `load_spawn_batch` does.
  - `life_seq`: which life of the spawn is standing. Runtime state.
- v1 policy: **insert and partial update**, band checked against the key's `spawn_index`.
- **`map_id` is mandatory on every claim, under both operations.** An insert carries it because an
  insert carries every column. An update must state it too, and that is a new rule: the importer
  routes a spawn claim by the map it names, and it has no database to read the map from. Add a
  refusal for an update on `game_creature_spawn` whose fields do not include `map_id`.

## The spawn key

The durable guid is `world_guid(entry, db_guid) = (0xF130 << 48) | (entry << 24) | (db_guid &
0xFF_FFFF)` in `importer/src/main.rs`, and `encounter::wave_guid` in the Module builds the same
layout. It is roughly 1.7e19, so a JSON key number cannot carry it exactly, a `u64` only travels
through this artifact as a decimal string, and a key member is a number.

So the claim names the components and the key derives the packed value, mirroring
`packed_spell_effect_id`:

```rust
/// Packed `game_creature_spawn` primary key: `(HIGHGUID_UNIT << 48) | (entry << 24) | spawn_index`.
///
/// The same layout `importer/src/main.rs`'s `world_guid` builds and `module/src/encounter.rs`'s
/// `wave_guid` mints. A Package Delta never authors the packed value.
pub const fn packed_creature_spawn_guid(entry: u32, spawn_index: u32) -> u64
```

- `PrimaryKey::CreatureSpawn { entry: u32, spawn_index: u32 }`, key members `{ entry, spawn_index }`.
- `row_id()` returns the packed guid.
- `is_key_column(Table::CreatureSpawn, name)` is true for `guid`, `entry` and `spawn_index`.
- Both components are bounded: `entry <= 0xFF_FFFF` and `spawn_index <= 0xFF_FFFF`. A claim naming a
  wider value is a `MalformedKey`, because the guid layout would silently truncate it.
- An UPDATE names an existing spawn the same way: `entry` is the creature, `spawn_index` is the low
  24 bits of the cmangos `creature.guid` the importer folded in.

Do not add a "the whole guid as a decimal string" key form. One shape per table.

## Identifier ranges

In `ids.rs`, a new `creatures` section following the file's own header formula and the loot section's
layout.

```rust
PACKAGE_CREATURE_ID_FLOOR: u32 = 15_000_000
PACKAGE_CREATURE_ID_CEIL:  u32 = 15_999_999

PACKAGE_CREATURE_SPAWN_FLOOR: u32 = 6_000_000
PACKAGE_CREATURE_SPAWN_CEIL:  u32 = 6_999_999
```

with `is_package_creature_id`, `is_fixture_reserved_creature_id`, `is_package_creature_spawn_index`,
mirroring the loot pair. Neither table has a fixture cluster of its own, so the project-wide
`509xxxx` band is the whole fixture check.

Asserts, all five required:

```rust
const _: () = assert!(PACKAGE_CREATURE_ID_FLOOR <= PACKAGE_CREATURE_ID_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_CREATURE_ID_FLOOR);
// The guid carries the creature entry in bits 24..47. An entry above this ceiling would be
// truncated into another creature's guid.
const _: () = assert!(PACKAGE_CREATURE_ID_CEIL <= 0xFF_FFFF);

const _: () = assert!(PACKAGE_CREATURE_SPAWN_FLOOR <= PACKAGE_CREATURE_SPAWN_CEIL);
const _: () = assert!(RESERVED_ID_CEIL < PACKAGE_CREATURE_SPAWN_FLOOR);
// Bit 23 of a guid's low 24 is `module/src/instance.rs`'s INSTANCE_POP_LOW_BAND: a low at or above
// it names a per-instance population copy, not a spawn. Every Package spawn index stays below it.
const _: () = assert!(PACKAGE_CREATURE_SPAWN_CEIL < 0x80_0000);
```

The Package Creature Spawn Range reuses the Package Spell Range's decade, and its doc comment has to
say why that is not a collision: a spawn index is a 24-bit FIELD inside a guid, not a row identifier,
so the millions-column signal has no meaning in that space. Bit 23 caps any spawn index at 8,388,607,
which makes a 15,000,000-shaped number unspellable; 6,000,000 is the readable number that clears bit
23, still clears the reserved span so the range keeps the same two asserts every other range has, and
sits far above every real cmangos spawn guid and every seeded or debug low.

Add a Module-side test that the two builds agree: `PACKAGE_CREATURE_SPAWN_CEIL <
instance::INSTANCE_POP_LOW_BAND`. The `const` assert above pins the literal; this test pins the
literal to the Module's own constant, so moving `INSTANCE_POP_LOW_BAND` fails a test rather than
quietly widening the band.

## Refusals

In `error.rs`, following the file's voice:

- `CreatureIdNotClientSafe { entry }` and `CreatureIdFixtureReserved { entry }`: the item-family
  shapes.
- `SpawnIndexNotClientSafe { spawn_index }`: an insert whose spawn index sits outside the Package
  Creature Spawn Range.
- `SpawnMapNotStated { table }`: an update on a spawn table that does not state `map_id`. The
  message says why: the importer routes a spawn claim to a Shard by the map it names, so a claim that
  does not name one cannot be routed.

`check_inventable`'s `PrimaryKey::CreatureTemplate` arm checks `entry`; its `PrimaryKey::CreatureSpawn`
arm checks `spawn_index`, not `entry`: a Package may add a spawn of a REAL creature, and that is the
common case. Keep the match wildcard-free.

`SpawnMapNotStated` is enforced in `Claim::new`, next to `EmptyUpdate`, because it is a property of
the claim rather than of the key.

## Module family: `module/src/package_import/creatures.rs`

Mirror `loot.rs`.

- `update_target`: point lookups (`.entry().find(...)`, `.guid().find(...)`). `Uninvented` when the
  update lands on a Package-range row no enabled Package inserts, for the template that is
  `is_package_creature_id(entry)`, for the spawn `is_package_creature_spawn_index(low 24 of guid)`.
- `clear_package_range`: delete `game_creature_template` rows whose `entry` is in the Package Creature
  Range, and `game_creature_spawn` rows whose `guid & 0xFF_FFFF` is in the Package Creature Spawn
  Range. Note in the doc comment that the band check is on the low bits, not the whole guid, which is
  what lets a Package spawn a real creature without the clear touching the dump's own spawns.
- `write_row`: a blank-row builder plus per-column setters, `check_insert_is_whole` on an insert. A
  new spawn row is born exactly the way `load_spawn_batch` births one: `respawn_at = ctx.timestamp`
  (ARMED, so `pass_respawn` builds the entity on the next tick), `despawn_at = timer_never(ctx)`,
  `life_seq = 0`. Getting this wrong produces a spawn row that never becomes a creature.
- `check_references`:
  - a spawn's `entry` must name a `game_creature_template` row this Shard holds, OR one this same
    plan inserts. Both live in this family, which is why they share it. Without the template the
    spawn silently never materializes (`pass_respawn` just skips it), so this check is the only thing
    standing between a Package and an invisible failure.
  - a template's final `pickpocket_loot_id` and `skin_loot_id`, when non-zero, must name a
    `game_pickpocket_loot` / `game_skinning_loot` row. Read the merged final value the way
    `loot.rs`'s `final_u32` does, so an update that changes only one column is judged on what the row
    will hold after the apply. Zero means "no table" by convention and is always accepted.

Wire into `package_import.rs`: `ClaimFamily::Creature`, `Family::ALL`, `as_str`, the four dispatch
matches, and the new tables into `ClaimCounts::claim_counts`'s `inserted_rows` arm.

**Tripwire.** `game_creature_spawn` is in `partition_discipline_tripwire::SPATIAL_ACCESSORS`, and
`clear_package_range` scans it. Add one `WHITELIST` entry for
`module/src/package_import/creatures.rs` with a one-line justification in the file's own voice: the
Package Delta stage is a realm-wide reload stage that runs immediately after the base import it
follows, the same reason `module/src/creatures/spawn.rs` is whitelisted. Keep the budget at the
smallest number that passes.

## The World Import Scope claim filter

This is the half the rest of the issue depends on. Put the predicate on `WorldImportScope` itself, so
the codebase gains no second routing concept.

In `importer/src/world_import_scope.rs`:

```rust
/// True when this scope wants the row a spatial Claim describes.
///
/// The same predicate the base import applies to a dump row, so a Package spawn is judged by the
/// rule that decided whether its neighbours imported at all.
pub(crate) fn admits_creature_spawn(&self, map_id: i64, position: Option<(f64, f64, f64)>, entry: u64) -> bool
```

- `position: Some(..)` for an insert, and for an update that states `x`, `y` and `z`: the row must
  clear `creature_row_kept_in_scope`'s whole predicate, so a Package cannot walk a spawn off the
  Shard that owns it.
- `position: None` for an update that does not move the row: the map fence alone
  (`contains_map`), because the row is only on this Shard because the base import put it there.
- An update that states some but not all of `x`/`y`/`z` is a partial move. Treat it as `None` and say
  so in the doc comment: judging a half-stated position against geometry would compare a new value
  against an unknown one.

`creature_row_kept_in_scope` currently lives in `main.rs`. Move it, or call it from the new method.
Either is fine, but the two must be one predicate, not two that can drift.

In `importer/src/package_delta.rs`, `reapply` gains the scope and the run's imported template entry
set, and filters after `artifacts_for_family`:

```rust
pub(crate) fn reapply(args: &Args, family: &str, root: &str, scope: &ClaimScope) -> Result<()>
```

`ClaimScope` is a small importer type with two states: `Unscoped` (every non-spatial family, and
`spell.rs`'s call site) and `Spatial { scope: WorldImportScope, imported_entries: BTreeSet<u32> }`.
Do not give it a trait; there are exactly two cases and both are known here.

The filter runs in two passes, because a template rides its spawns:

1. Keep every spawn claim the scope admits.
2. Keep a template claim when `imported_entries` holds its `entry`, or when a spawn claim kept in
   pass 1 names it.

Everything dropped is REPORTED. Extend `plan_report` with one line per dropped claim:

```text
  out of scope alliance-kalimdor  game_creature_spawn {entry=15000001, spawn_index=4000001} (map 0)
```

Silence here would be the worst outcome the whole issue can produce: an operator would see a clean
import and a missing NPC.

The Package identity and source hash survive the filter the same way `artifacts_for_family` preserves
them, and an artifact left with no claims drops out.

### Plumbing

- `DumpPlan` gains `pub(crate) creature_entries: BTreeSet<u32>`: the `entries` set `build_dump_plan`
  already computes to the EventAI fixpoint, narrowed to `u32`. It is the run's real answer to "which
  creature templates does this Shard hold", so nothing recomputes it.
- `run_package_stage(args, family, plan)` builds the `ClaimScope` for the family: `Spatial` for
  `"creatures"`, `Unscoped` otherwise. It already has `args`, and `args.world_import_scope()` rebuilds
  the scope with no new threading.
- `spell.rs`'s call site passes `ClaimScope::Unscoped`. `--spells` is a `--dbc` path with no dump and
  no scope, and the spell catalogue is map-agnostic on purpose.
- Add `"creatures"` to `PACKAGE_DELTA_DUMP_FAMILIES`.
- `main.rs`'s test `an_enabled_packages_root_without_a_package_delta_family_active_parses` uses
  `--family creatures` as its negative case. Move it to `--family creature-ai`, the one family left
  with no Package Delta schema.

## Files owned

- `crates/lyracore-package-delta/src/{schema.rs,ids.rs,delta.rs,error.rs,lib.rs}`
- `crates/lyracore-package-delta/tests/{families.rs,refusals.rs,claim_counts.rs}` and a new
  `creature_identifiers.rs`
- `module/src/package_import.rs`, `module/src/package_import/fixtures.rs`, new
  `module/src/package_import/creatures.rs`
- `module/src/package_import/{spell,items,quest,loot}.rs` and #311's family modules, only for the
  foreign-family match arms the new `Table` variants force
- `module/src/tripwires.rs` (one `WHITELIST` entry)
- `importer/src/{main.rs,package_delta.rs,world_import_scope.rs,spell.rs}`
- `CONTEXT.md` (two Package Identifier Range entries, and a sharpened World Import Scope entry)

## Out of scope

- The `gameobjects` family. T2 owns it.
- The cross-Shard preflight mode. T3 owns it.
- `game_creature_waypoint`, creature pools, EventAI. See the README's named gaps.
- `packages replay`, authoring-library helpers, Base Snapshots, the spell-shaped plan report columns.
- Any Module durable schema change.

## Acceptance tests

1. `crates/.../tests/creature_identifiers.rs`: both ranges at each end and one outside each end;
   fixture-reserved refusal; an insert outside each band refused by name; a key whose `entry` or
   `spawn_index` exceeds `0xFF_FFFF` refused as `MalformedKey`.
2. `packed_creature_spawn_guid` against a worked example taken from the importer, not recomputed with
   the same expression: assert the packed value for a known `(entry, spawn_index)` equals what
   `world_guid` produces for it, and that `encounter::entry_of_unit_guid` reads the entry back.
3. A spawn `update` that does not state `map_id` is refused with `SpawnMapNotStated` naming the
   table; the same update with `map_id` is accepted.
4. `tests/families.rs`: both new tables parse back to themselves, belong to `creatures`, and have at
   least one claimable column.
5. `module/src/package_import.rs`: `a_creature_plan_claims_only_creature_family_tables`, following
   `a_loot_plan_claims_only_loot_family_tables`.
6. In `creatures.rs`: every claimable column has a setter, the blank-row builder covers every column,
   and a new spawn row is born with an armed `respawn_at` and a not-armed `despawn_at`.
7. `check_references`: a spawn whose template neither the Shard nor the plan provides is refused by
   name; a spawn whose template the SAME plan inserts is accepted; a template whose non-zero
   `skin_loot_id` names no row is refused; `0` is accepted.
8. Scope filter, in `importer/src/world_import_scope.rs` or `package_delta.rs` unit tests, one
   behaviour each:
   - a spawn insert inside `alliance-eastern`'s Elwynn slice is kept for `alliance-eastern` and
     dropped for `alliance-kalimdor`;
   - a spawn insert on map 36 is kept for `instances` and dropped for `alliance-eastern`;
   - a spawn insert on an in-scope map but outside every bounded slice is dropped, and the same
     insert for a forced creature entry is kept, the forced-dependency rule following the base
     import;
   - a spawn update that states only `map_id` is kept on the map fence alone;
   - a spawn update that states `map_id` and a new out-of-scope position is dropped;
   - a template claim is kept when a kept spawn claim in the same plan names it, and dropped when
     nothing in scope does.
9. The plan report names every dropped claim, its table, its key and the scope that dropped it.
10. `cargo test -p lyracore-package-delta`, `cargo test -p lyracore-module --lib`,
    `cargo test -p lyracore-importer --bin lyracore-importer` all pass, including the tripwire.
11. `cargo build --release --target wasm32-unknown-unknown -p lyracore-module` succeeds.
12. `cargo clippy` clean on every touched file, `rustfmt` clean.

## Definition of done

The `creatures` family applies end to end in the native tests; a spatial claim reaches only the
profiles whose World Import Scope owns its map, and the ones it does not reach are printed; both new
ranges carry their layout asserts; `CONTEXT.md` carries them; the pinned canonical artifact bytes are
unchanged.
