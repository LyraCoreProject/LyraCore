# T1: the casts and trainers Import Families, and the update-only refusal

Parent: issue #311. **First slice. Blocks T2 to T5. Run alone.**
Model: sonnet.

## Goal

Add two small Import Families, and with them the one piece of machinery every later ticket needs: a
table that permits no inserts at all.

`casts` and `trainers` ride together because they are three tables between them, they share the same
cross-reference (a spell) and the same owning entity (a creature template entry), and one of them is
the worked example of the update-only shape.

## Read first

- `crates/lyracore-package-delta/src/schema.rs`, `ids.rs`, `delta.rs`, `error.rs`
- `module/src/package_import.rs` and `module/src/package_import/loot.rs` (the closest template: a
  family whose band is checked against a row's own surrogate identifier)
- `importer/src/main.rs` lines 4896 to 4928 (the two `--dump` blocks) and 3660 to 3680
- `module/src/creatures/spawn.rs` (`CreatureCast`, `CreatureSpell`) and `module/src/trainer.rs`
  (`TrainerSpell`)

## Tables and v1 policy

### `casts` (family name `"casts"`)

**`game_creature_cast`** (`module/src/creatures/spawn.rs`, struct `CreatureCast`)

- Primary key: `creature_entry: u32`, the creature template entry. Not auto-inc.
- Claimable columns: `spell_id` (u32).
- v1 policy: **partial update only. No inserts.** The key names a creature template, which belongs
  to the spatial families out of this issue's scope, so a Package cannot invent one. An insert
  inside a Package band would name a creature that does not exist and the row would never be read.
- Borderline note worth carrying in the doc comment: both columns describe the creature-to-spell
  relation, which is the shape that got `game_creature_quest` excluded from the quest family. It
  stays in this catalogue because `spell_id` is not a key column, so a claim on it sets a real
  value: retargeting which spell a caster mob uses.

**`game_creature_spell`** (same file, struct `CreatureSpell`)

- Primary key: `id: u64`, `#[auto_inc]` surrogate.
- Claimable columns, in declaration order: `creature_entry` (u32), `spell_id` (u32), `priority`
  (u8), `condition` (u8), `condition_value` (u8).
- v1 policy: **insert and partial update.** Band checked against the row's own `id`, the loot shape.
  A Package adds a rotation entry to a caster mob that already exists.

Base import: `DELETE FROM game_creature_cast WHERE creature_entry > 0` and
`DELETE FROM game_creature_spell WHERE id > 0`, both wholesale. That is the same shape the loot
family already lives with: the Package stage runs immediately after in the same invocation and
rewrites the band.

### `trainers` (family name `"trainers"`)

**`game_trainer_spell`** (`module/src/trainer.rs`, struct `TrainerSpell`)

- Primary key: `id: u64`, `#[auto_inc]` surrogate.
- Claimable columns, in declaration order: `trainer_entry` (u32), `spell_id` (u32), `cost` (u32),
  `required_level` (u8), `learn_skill_line` (u32), `learn_skill_cap` (u32).
- v1 policy: **insert and partial update.** Band checked against `id`.
- Base import: `DELETE FROM game_trainer_spell WHERE id > 0`, wholesale, same as above.

## Identifier bands

In `ids.rs`, in a new `casts` section and a new `trainers` section, following the file's own header
formula and the loot section's layout:

```
PACKAGE_CAST_ID_FLOOR: u64 = 10_000_000
PACKAGE_CAST_ID_CEIL:  u64 = 10_999_999
PACKAGE_TRAINER_ID_FLOOR: u64 = 11_000_000
PACKAGE_TRAINER_ID_CEIL:  u64 = 11_999_999
```

with `is_package_cast_id`, `is_fixture_reserved_cast_id`, and the trainer pair, mirroring
`is_package_loot_id` / `is_fixture_reserved_loot_id`. Both bands need the two `const _: () =
assert!(...)` lines: floor at or under ceiling, and `RESERVED_ID_CEIL` strictly under the floor.

Neither family has a fixture cluster of its own, so the project-wide `509xxxx` band is the whole
fixture check, the same as items, quests and loot.

Name the constants `CAST` and `TRAINER` singular, matching `PACKAGE_LOOT_ID_FLOOR`'s singular form
even though the family name is plural.

Beware: `CURATED_RESERVED_ID_CEIL` is documented as the curated trainer overrides at 5,200,499
(`importer/src/spell.rs`, `CURATED_TRAINER_ID_BASE`). That band is a different concept from the
Package Trainer Range added here. Do not merge them, and say so in the new constant's doc comment so
the next reader does not.

## The update-only refusal

`check_inventable` in `delta.rs` is a match over `PrimaryKey` with one arm per family, and every arm
today reads "inside the band, or a `...NotClientSafe` refusal". `game_creature_cast` needs a third
answer: this table is never inventable.

Add to `error.rs`:

```rust
/// A Package tried to INSERT into a table that v1 permits no inserts on.
InsertNotSupported { table: Table },
```

The message names the table and why, in the shape the other refusals use. Suggested wording, adjust
to match the file's voice:

> `game_creature_cast` rows cannot be inserted: the key names a creature template, which no Package
> may invent

Then in `check_inventable`, the `PrimaryKey::CreatureCast { .. }` arm returns that error
unconditionally. Keep the match wildcard-free.

Also check `crates/lyracore-package-delta/src/lib.rs` and `tests/refusals.rs`: a refusal variant
usually needs a re-export and a message test.

## Module family

New file `module/src/package_import/casts.rs` and `module/src/package_import/trainers.rs`, each
mirroring `loot.rs`:

- `update_target` returns `Present`, `Absent` or `Uninvented`. `Uninvented` applies only to tables
  that can be inserted, so `game_creature_cast` never returns it.
- `clear_package_range` removes `game_creature_spell` rows in the Package Cast Range, and
  `game_trainer_spell` rows in the Package Trainer Range. `game_creature_cast` has no band, so
  nothing to clear there.
- `write_row` builds a blank row plus per-column setters, and `check_insert_is_whole` on an insert.
- `check_references`:
  - casts: `spell_id` must be a `game_spell` row, on both tables; `game_creature_spell.creature_entry`
    must be a `game_creature_template` row.
  - trainers: `trainer_entry` must be a `game_creature_template` row. **`spell_id` is checked
    against `game_spell` only when the row's final `learn_skill_line` is 0.** A profession offering
    (`learn_skill_line > 0`) carries a synthetic marker `spell_id` (50080 or 50081, see
    `module/src/trainer.rs`) that is deliberately never resolved to a `game_spell` row, so an
    unconditional check would refuse a legitimate claim. Read the merged final value the same way
    `loot.rs`'s `final_u32` does, so an update that changes only one of the two columns is judged on
    what the row will hold after the apply.

Wire both into `package_import.rs`: `ClaimFamily::Cast` and `ClaimFamily::Trainer`, `Family::ALL`,
`as_str`, and the four dispatch matches. `check_references` gets real arms for both, not `Ok(())`.

## Importer

Add `"casts"` and `"trainers"` to `PACKAGE_DELTA_DUMP_FAMILIES` in `importer/src/main.rs`. Both are
already in `FAMILIES` and already have `--dump` blocks, so the Package Delta stage needs no new
invocation shape: `run_package_stage` reads the constant.

## Files owned

- `crates/lyracore-package-delta/src/{schema.rs,ids.rs,delta.rs,error.rs,lib.rs}`
- `crates/lyracore-package-delta/tests/{families.rs,refusals.rs,claim_counts.rs}` and two new files
  `cast_identifiers.rs`, `trainer_identifiers.rs`
- `module/src/package_import.rs`, `module/src/package_import/fixtures.rs`, and new
  `module/src/package_import/{casts.rs,trainers.rs}`
- `module/src/package_import/{spell,items,quest,loot}.rs` only for the foreign-family match arms the
  new `Table` variants force
- `importer/src/main.rs` (`PACKAGE_DELTA_DUMP_FAMILIES` and its validation tests)
- `CONTEXT.md` (two new Package Identifier Range entries)

## Out of scope

- Any other family. T2 to T4 own those.
- `packages replay`, authoring-library helpers, the spell-shaped plan report. See the README.
- Any Module durable schema change.

## Acceptance tests

1. `crates/.../tests/cast_identifiers.rs` and `trainer_identifiers.rs`: band membership at both
   ends and one outside each end, fixture-reserved refusal, a malformed key, and an insert outside
   the band refused by name.
2. A test that `game_creature_cast` refuses every insert with `InsertNotSupported` naming the
   table, and accepts an update on a real creature entry.
3. `tests/families.rs`: the three new tables parse back to themselves, belong to `casts` and
   `trainers`, and each has at least one claimable column.
4. `module/src/package_import.rs`: `a_cast_plan_claims_only_cast_family_tables` and the trainer
   equivalent, following `a_loot_plan_claims_only_loot_family_tables`.
5. In `casts.rs` / `trainers.rs`: every claimable column has a setter (the
   `every_claimable_item_column_has_a_setter` shape), and the blank-row builder covers every column.
6. A trainer `check_references` test proving a profession offering
   (`learn_skill_line > 0`, marker `spell_id`) is accepted while a class offering
   (`learn_skill_line == 0`) with a missing `spell_id` is refused.
7. Importer `--packages` validation accepts `--family casts` and `--family trainers`.
8. `cargo test -p lyracore-package-delta`, `cargo test -p lyracore-module --lib`,
   `cargo test -p lyracore-importer --bin lyracore-importer` all pass.
9. `cargo build --release --target wasm32-unknown-unknown -p lyracore-module` succeeds.
10. `cargo clippy` clean on every touched file, `rustfmt` clean.

## Definition of done

Both families apply end to end in the native tests, the update-only refusal is enforced at parse
time, `CONTEXT.md` carries the two new ranges, and the pinned canonical artifact bytes are
unchanged.
