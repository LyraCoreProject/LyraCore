# T4: the spellmeta Import Family

Parent: issue #311. **Depends on T3. Rebase onto it before starting. Run alone.**
Model: sonnet.

## Goal

The last family. Three tables that describe spells without being spells: the rank chain, the
auto-learn dependents, and the proc overlay.

## Read first

- `importer/src/main.rs` lines 1875 to 2037 (the three builders and `SYNTHETIC_PROC_EVENT_FLOOR`)
  and 4998 to 5015 (the `--dump` block).
- `module/src/spell/spellbook.rs` (`SpellChain`, `SpellLearn`), `module/src/spell/tables.rs`
  (`SpellProcEvent`).
- `crates/lyracore-package-delta/src/ids.rs`, the `quests` section. Its "one band, checked against
  the owning identifier, covers every child table" reasoning is the model for the two spell-keyed
  tables here.

## The band decision, and why it is not a fourth decade

Two of the three tables are keyed by `spell_id`. Such a row is Package-owned exactly when the spell
it describes is Package-owned, so it takes the **Package Spell Range** (6,000,000 to 6,999,999)
against its own `spell_id`. That is the same shape the Package Quest Range already uses for
`game_quest_text` and the other quest children: one band checked against the owning identifier
rather than a second band of the family's own.

Reusing the spell family's band across a family boundary is safe because a band is cleared per
table, not per identifier space. `spellmeta`'s `clear_package_range` deletes band rows from
`game_spell_chain` and `game_spell_proc_event`; the spell family's deletes band rows from
`game_spell` and `game_spell_effect`. The two never touch the same row.

The third table has a surrogate key of its own and needs a band of its own:

**Package Spell Metadata Range: 14,000,000 to 14,999,999**, checked against
`game_spell_learn.id` alone. Constants `PACKAGE_SPELLMETA_ID_FLOOR` / `_CEIL`,
`is_package_spellmeta_id`, `is_fixture_reserved_spellmeta_id`, the two `const` asserts. Type `u64`,
matching the key.

Say both facts in the `ids.rs` section header: this family carries one band and borrows another, and
which table takes which.

## Tables and v1 policy

**`game_spell_chain`** (`module/src/spell/spellbook.rs`, struct `SpellChain`)

- Primary key: `spell_id: u32`.
- Claimable columns: `prev_spell` (u32), `first_spell` (u32), `rank` (u8), `req_spell` (u32).
- v1 policy: **insert and partial update.** Insert band is the **Package Spell Range** against
  `spell_id`. `spell_id` 0 is a malformed key, matching `check_claimable_spell_id`.
- Base import: `DELETE FROM game_spell_chain WHERE spell_id > 0`, wholesale. The Package stage
  rewrites the band right after, the arrangement loot and trainers already live with.
- Named gap for the doc comment: a Package cannot ADD a chain row to a real spell that has none,
  because that would be an insert at a real identifier. It can tune a real spell's existing row, and
  it can chain its own Package spells. Recorded, not worked around.

**`game_spell_learn`** (same file, struct `SpellLearn`)

- Primary key: `id: u64`, `#[auto_inc]` surrogate.
- Claimable columns: `parent_spell` (u32), `learn_spell` (u32).
- v1 policy: **insert and partial update.** Band is the Package Spell Metadata Range against `id`.
- Note for the doc comment: the importer synthesizes two armor-proficiency wrapper rows
  (`build_spell_learn_sql`, plate 7109 to 750 and mail 8738 to 8737) at the tail of the dense id
  range. They are far below the band and a Package claim never meets them.

**`game_spell_proc_event`** (`module/src/spell/tables.rs`, struct `SpellProcEvent`)

- Primary key: `spell_id: u32`.
- Claimable columns: `proc_flags` (u32), `proc_ex` (u32), `school_mask` (u8), `family_name` (u8),
  `family_flags` (u64), `ppm_rate` (f32), `custom_chance` (u8), `icd_ms` (u32).
- v1 policy: **insert and partial update.** Insert band is the **Package Spell Range** against
  `spell_id`.
- `family_flags` is `u64`, so it travels as a decimal string. `FieldType::U64` already handles that
  and `canonical.rs` pins the spelling. This is the first non-spell family to use it; add a test
  that a `u64` claim round-trips through canonical form.
- The base import's DELETE stops at `SYNTHETIC_PROC_EVENT_FLOOR` (50,000), so it never reaches the
  Package Spell Range. Unlike the two tables above, band rows here survive the base import outright,
  and `clear_package_range` is the only thing that removes them. Say so: it is the property that
  makes reconciliation correct, not an accident.

`ppm_rate` is `f32`. The parser already refuses a non-finite float, and `module/src/spell/proc.rs`
treats a corrupt rate as poisoning every aura of the spell, so the importer guards `ppm_rate >= 0.0`.
Mirror that guard in `write_row` or in the key/field validation, and refuse a negative rate.

## Fixture identifiers

`is_fixture_reserved_spell_id` already covers the seeded fixture cluster 50,000 to 50,999 and the
project-wide `509xxxx` band. The two spell-keyed tables must use THAT function, not a new one: a
claim on a fixture spell's proc overlay would turn the Module's own proc tests into a report about
the Package. `game_spell_learn` has a surrogate key, so it takes the project-wide band alone, the
same as items, quests and loot.

## Cross-references

In `module/src/package_import/spellmeta.rs`, on final merged values, skipping 0 (which means "none"
in every one of these columns):

- `game_spell_chain.prev_spell`, `first_spell` and `req_spell` must each be a `game_spell` row.
- `game_spell_learn.parent_spell` and `learn_spell` must each be a `game_spell` row.

`game_spell_proc_event` references nothing outside its own key, and `update_target` already answers
for that.

A reference may point at a spell the same realm-wide plan inserts, but the spell family is a
different apply, so it must already be on the Shard. Refuse otherwise, with the existing
"the base import has to run before its Package Deltas" phrasing as the model.

## Module family and importer

New `module/src/package_import/spellmeta.rs`. Wire `ClaimFamily::SpellMeta` through
`package_import.rs`. Add `"spellmeta"` to `PACKAGE_DELTA_DUMP_FAMILIES` in `importer/src/main.rs`.

`clear_package_range` deletes: `game_spell_chain` and `game_spell_proc_event` rows whose `spell_id`
is in the Package Spell Range, and `game_spell_learn` rows whose `id` is in the Package Spell
Metadata Range.

## Files owned

Same set as T1 to T3, plus `module/src/package_import/spellmeta.rs` and
`crates/lyracore-package-delta/tests/spellmeta_identifiers.rs`.

## Out of scope

- Adding a chain row to a real spell that has none. See the named gap above.
- The `Active` column of cmangos `spell_learn_spell`, which the importer parses and deliberately
  does not filter on. Nothing here changes that.
- Any change to the spell family.

## Acceptance tests

1. `tests/spellmeta_identifiers.rs`: the two spell-keyed tables accept an insert inside the Package
   Spell Range and refuse one outside it; `game_spell_learn` accepts one inside the Package Spell
   Metadata Range and refuses one outside; fixture-reserved refusals on all three; malformed keys.
2. A test that a Package Spell Range identifier is inventable in BOTH the spell family and the
   spellmeta family, and that `check_claims_belong_to` still keeps the two applies apart.
3. A `u64` `family_flags` claim round-trips through canonical form unchanged.
4. A negative `ppm_rate` is refused.
5. `tests/families.rs`: three new tables parse back to themselves, belong to `spellmeta`, each has
   claimable columns.
6. `a_spellmeta_plan_claims_only_spellmeta_family_tables` in `module/src/package_import.rs`.
7. Setter and column-coverage tests per table in `spellmeta.rs`.
8. `check_references` tests for the five spell references, including the 0-means-none skip.
9. Importer `--packages` validation accepts `--family spellmeta`.
10. Crate, module lib and importer suites pass; wasm release build succeeds; clippy and rustfmt
    clean on touched files.

## Definition of done

All five remaining families are in the build. `Family::parse` on an unknown name lists ten families
(`spell`, `items`, `quests`, `loot`, `casts`, `trainers`, `gossip`, `globals`, `spellmeta`,
`script`). `CONTEXT.md` carries the Package Spell Metadata Range and records that the two spell-keyed
spellmeta tables borrow the Package Spell Range.
