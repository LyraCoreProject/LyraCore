# T2: the gameobjects Import Family

Parent: issue #312. **Depends on T1. Rebase onto it before starting.**
Model: sonnet.

**Also rebase onto the #311 chain head**, the same way T1 does. See the README.

## Goal

Give the `gameobjects` Import Family a Package Delta schema, reusing every shape T1 settled. Three
tables, one new identifier range, no new mechanism: T1 already built the World Import Scope claim
filter and this ticket adds the gameobject predicate to it.

## Read first

- `.claude/tickets/issue-312/README.md`
- T1's finished diff. This ticket is a pattern copy of it.
- `module/src/gameobject.rs`: `GameObjectTemplate` (line 80), `GameObjectTrap` (line 122),
  `GameObject` (line 212), `load_go_batch` (line 1158) and whatever derives `grid_x`/`grid_y`/`cell`,
  `POOL_TAG` and `pool_point_guid`
- `module/src/instance.rs` lines 227 to 230: `GO_COPY_BAND`
- `importer/src/main.rs`: `go_guid` (line 518), `classify_go_type` (line 597), the gameobject SQL
  block (line 4885 onward) and the GO spawn scope filter around line 4394

## Tables and v1 policy

Family name const: `GAMEOBJECT_FAMILY: &str = "gameobjects"`, the name the `--dump` importer's
`gameobjects` `--family` block already stamps.

Beware three similar names. `game_gameobject_template` is the template, `game_gameobject` is the
SPAWN (no suffix), and `game_gameobject_loot` already belongs to the loot family. Name the `Table`
variants `GameobjectTemplate`, `GameobjectSpawn` and `GameobjectTrap` so no reader has to guess.

### `game_gameobject_template` (struct `GameObjectTemplate`)

- Primary key: `entry: u32`.
- Claimable columns, in declaration order: `type_id` (u8), `display_id` (u32), `name` (str), `data0`
  (u32), `data1` (u32), `gather_skill_line` (u32), `respawn_secs` (u32), `gather_gray` (u32),
  `lock_id` (u32), `size` (f32).
- v1 policy: **insert and partial update**, band checked against `entry`.
- Carry one fact into the doc comment: `type_id` is the CLASSIFIED type, not the raw cmangos one.
  `classify_go_type` drops real cmangos type 25 so it cannot collide with this repo's synthetic
  `go_type::GATHER` marker at 25. A Package claiming `type_id` states the classified value.

### `game_gameobject` (struct `GameObject`), the spawn

- Primary key: `guid: u64`, DERIVED: `(0xF110 << 48) | spawn_id`, the layout `importer/src/main.rs`'s
  `go_guid` builds.
- Key member: `{ spawn_id: u64 }`, bounded below `1 << 46`. `spawn_id` is under 2^53, so unlike the
  creature spawn it needs no component split, a JSON key number carries it exactly.
- Claimable columns: `template_entry` (u32), `map_id` (u32), `x` (f32), `y` (f32), `z` (f32),
  `orientation` (f32), `state` (u8), `rotation_0` (f32), `rotation_1` (f32), `rotation_2` (f32),
  `rotation_3` (f32).
- NOT claimable, each with a stated reason:
  - `guid`: the key names it.
  - `created_at`: a `Timestamp`; the Claim schema has no timestamp type.
  - `respawn_at_micros`: respawn machinery state.
  - `instance_id`: partition state. A static or imported GO always sits at `instance_id = 0`;
    per-instance copies are minted live in `GO_COPY_BAND` by `module/src/instance.rs`.
  - `grid_x`, `grid_y`, `cell`: derived spatial index columns. The writer computes them from
    `map_id`/`x`/`y` exactly the way `load_go_batch` does. A claim that set them could put a row in a
    cell it is not in.
- v1 policy: **insert and partial update**, band checked against `spawn_id`.
- **`map_id` is mandatory on every claim**, under both operations, for the routing reason T1 states.
  Reuse T1's `SpawnMapNotStated` refusal.

### `game_gameobject_trap` (struct `GameObjectTrap`)

- Primary key: `entry: u32`, a gameobject template entry.
- Claimable columns: `spell_id` (u32), `cooldown_secs` (u32).
- v1 policy: **insert and partial update**, band checked against `entry`, the shape the quest family
  uses for its child tables. A trap row is Package-owned exactly when the template it describes is.
  It is not update-only, because the entity its key names belongs to THIS family.

## Identifier range

In `ids.rs`, a new `gameobjects` section:

```rust
PACKAGE_GAMEOBJECT_ID_FLOOR: u64 = 16_000_000
PACKAGE_GAMEOBJECT_ID_CEIL:  u64 = 16_999_999
```

One range for all three tables. They cannot collide: a template entry, a trap entry and a spawn id
are independent key spaces, the argument the Package Loot Range already makes for its four tables.

Asserts:

```rust
const _: () = assert!(PACKAGE_GAMEOBJECT_ID_FLOOR <= PACKAGE_GAMEOBJECT_ID_CEIL);
const _: () = assert!((RESERVED_ID_CEIL as u64) < PACKAGE_GAMEOBJECT_ID_FLOOR);
// Bit 46 of a gameobject guid is `module/src/instance.rs`'s GO_COPY_BAND (a per-instance copy) and
// bit 47 is `module/src/gameobject.rs`'s POOL_TAG (a pool point). Every Package spawn id stays
// below both, so no Package gameobject can be mistaken for either.
const _: () = assert!(PACKAGE_GAMEOBJECT_ID_CEIL < (1 << 46));
```

Unlike the creature spawn index, this ceiling clears the reserved span AND the decade convention
holds, so the doc comment reads like the loot section's rather than needing the spawn section's
explanation. Add the Module-side test that `PACKAGE_GAMEOBJECT_ID_CEIL < instance::GO_COPY_BAND` and
`< gameobject::POOL_TAG`, so moving either constant fails a test.

`is_package_gameobject_id` and `is_fixture_reserved_gameobject_id`, mirroring the loot pair. No
fixture cluster of its own; the project-wide `509xxxx` band is the whole check.

## Refusals

`GameobjectIdNotClientSafe { id }` and `GameobjectIdFixtureReserved { id }`, the loot-family shapes.
Reuse T1's `SpawnMapNotStated`. `check_inventable` gets one arm covering all three tables, each
checked against its own identifier.

## Module family: `module/src/package_import/gameobjects.rs`

Mirror T1's `creatures.rs`.

- `update_target`: point lookups. `Uninvented` when the update lands on a Package-range row no
  enabled Package inserts.
- `clear_package_range`: delete Package-range rows from all three tables. For the spawn the band
  check is on `guid & 0x0000_FFFF_FFFF_FFFF`, the 48-bit low, not the whole guid.
- `write_row`: blank-row builder plus setters, `check_insert_is_whole` on an insert. A new spawn row
  is born the way `load_go_batch` births one: `created_at = ctx.timestamp`, `instance_id = 0`,
  `respawn_at_micros = 0`, and `grid_x`/`grid_y`/`cell` derived through the SAME helper
  `load_go_batch` calls. Do not reimplement the derivation.
- `check_references`: a spawn's final `template_entry` must name a `game_gameobject_template` row
  this Shard holds or this plan inserts; a trap's `entry` must do the same; a trap's `spell_id` must
  name a `game_spell` row. Read merged final values the way `loot.rs`'s `final_u32` does.

Wire into `package_import.rs`: `ClaimFamily::Gameobject`, `Family::ALL`, `as_str`, the four dispatch
matches, and the three new tables into `ClaimCounts::claim_counts`'s `inserted_rows` arm.

**Tripwire.** `game_gameobject` is a watched spatial accessor and `clear_package_range` scans it. Add
a `partition_discipline_tripwire::WHITELIST` entry for `module/src/package_import/gameobjects.rs`
with the same one-line justification T1 used.

## The scope filter

T1 built it. This ticket adds one predicate and one family arm.

- `WorldImportScope::admits_gameobject_spawn(&self, map_id, position: Option<(f64, f64, f64)>)`:
  `contains_map` plus, when a position is stated, `contains`. Gameobjects have no forced-entry
  bypass; the ETL's GO block does not have one either, so this predicate is the simpler of the two.
- The two-pass template rule is the same: a template or trap claim rides the spawn claims kept in
  pass 1, or the run's imported gameobject template entry set.
- `DumpPlan` gains `pub(crate) gameobject_entries: BTreeSet<u32>`: the classified `go_meta` keys the
  ETL already builds, which is the run's real answer to "which gameobject templates does this Shard
  hold".
- `run_package_stage` builds `ClaimScope::Spatial` for `"gameobjects"` too.
- Add `"gameobjects"` to `PACKAGE_DELTA_DUMP_FAMILIES`.

## Files owned

- `crates/lyracore-package-delta/src/{schema.rs,ids.rs,delta.rs,error.rs,lib.rs}`
- `crates/lyracore-package-delta/tests/{families.rs,refusals.rs,claim_counts.rs}` and a new
  `gameobject_identifiers.rs`
- `module/src/package_import.rs`, `module/src/package_import/fixtures.rs`, new
  `module/src/package_import/gameobjects.rs`
- every other `module/src/package_import/*.rs`, only for the foreign-family match arms the new
  `Table` variants force
- `module/src/tripwires.rs` (one `WHITELIST` entry)
- `importer/src/{main.rs,package_delta.rs,world_import_scope.rs}`
- `CONTEXT.md` (one Package Identifier Range entry)

## Out of scope

- Gameobject pools. `game_gameobject_pool` and `game_gameobject_pool_member` have no dump ETL behind
  them, so there is no base import for a claim to replay onto. See the README's named gaps.
- `game_lock`, `game_gameobject_unlocked`, `game_gameobject_trap_cooldown`. The first belongs to
  another family's block; the other two are runtime state.
- The cross-Shard preflight mode. T3 owns it.
- Any Module durable schema change.

## Acceptance tests

1. `crates/.../tests/gameobject_identifiers.rs`: the range at both ends and one outside each end;
   fixture-reserved refusal; an insert outside the band refused by name on each of the three tables;
   a `spawn_id` at or above `1 << 46` refused as `MalformedKey`.
2. The derived guid against a worked example taken from `go_guid`, not recomputed with the same
   expression.
3. A GO spawn `update` without `map_id` is refused with `SpawnMapNotStated`.
4. `tests/families.rs`: the three new tables parse back to themselves, belong to `gameobjects`, and
   each has at least one claimable column.
5. `module/src/package_import.rs`: `a_gameobject_plan_claims_only_gameobject_family_tables`.
6. In `gameobjects.rs`: every claimable column has a setter; the blank-row builder covers every
   column; a new spawn row is born at `instance_id = 0` with `grid_x`/`grid_y`/`cell` matching what
   `load_go_batch` derives for the same position.
7. `check_references`: a spawn whose template neither the Shard nor the plan provides is refused; one
   the same plan inserts is accepted; a trap whose `spell_id` names no spell is refused.
8. Scope filter: a GO spawn insert inside `alliance-eastern`'s Elwynn slice is kept for
   `alliance-eastern` and dropped for `instances`; one on map 36 is kept for `instances` only; a
   template claim rides a kept spawn claim.
9. `cargo test -p lyracore-package-delta`, `cargo test -p lyracore-module --lib`,
   `cargo test -p lyracore-importer --bin lyracore-importer` all pass, including the tripwire.
10. `cargo build --release --target wasm32-unknown-unknown -p lyracore-module` succeeds.
11. `cargo clippy` clean on every touched file, `rustfmt` clean.

## Definition of done

The `gameobjects` family applies end to end in the native tests, its range carries both layout
asserts, `CONTEXT.md` carries the range, and the pinned canonical artifact bytes are unchanged.
