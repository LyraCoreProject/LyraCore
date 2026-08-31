# Issue #311 remaining Import Families, broken into tickets

Source: `gh issue view 311`, "Package Deltas: support dump-backed catalogue Import Families".

Already on `main`: the family-generic core (PR #355), the items family (PR #357), the quests and
loot families (PR #359). This plan covers the five families that are left: `casts`, `trainers`,
`gossip`, `globals`, `spellmeta`. Twenty durable tables.

## Execution order

Strictly sequential. Every family appends to the same closed enums, so two slices running at once
conflict on every file:

- `crates/lyracore-package-delta/src/schema.rs` (`Table`, `Table::ALL`, `as_str`, `parse`,
  `family`, `columns`, the column lists, the family name const)
- `crates/lyracore-package-delta/src/delta.rs` (`PrimaryKey`, its constructors, `table`, `row_id`,
  `Display`, `check_inventable`, `parse_key`)
- `crates/lyracore-package-delta/src/ids.rs` (the family's Package Identifier Range and its
  compile-time asserts)
- `crates/lyracore-package-delta/src/error.rs` and `lib.rs` (refusals and re-exports)
- `module/src/package_import.rs` (`ClaimFamily`, `Family::ALL`, `as_str`, four dispatch matches)
- `importer/src/main.rs` (`PACKAGE_DELTA_DUMP_FAMILIES`)
- `CONTEXT.md` (one Package Identifier Range entry per family)

```text
T1 (casts + trainers, introduces the update-only refusal)
 └── T2 (gossip)
      └── T3 (globals)
           └── T4 (spellmeta)
                └── T5 (verification and PR prep)
```

Each ticket rebases onto the previous one before it starts.

| # | Ticket | Model | Tables | New band |
|---|--------|-------|--------|----------|
| T1 | `casts` and `trainers` | sonnet | 3 | Package Cast Range, Package Trainer Range |
| T2 | `gossip` | sonnet | 6 | Package Gossip Range |
| T3 | `globals` | opus | 8 | Package Globals Range |
| T4 | `spellmeta` | sonnet | 3 | Package Spell Metadata Range, reuses Package Spell Range |
| T5 | verification and PR prep | sonnet | 0 | none |

T3 takes opus because `globals` carries three distinct primary-key shapes in one family, including
three packed composite keys whose formulas must match both the importer and the Module byte for
byte, and because `game_start_item` has two importer writers with disjoint identifier bands.

## The fill-in checklist

Each family repeats the same six steps. Every step is compile-enforced: the enums are closed and no
match carries a wildcard, so a family that arrives half-finished does not build.

1. `Table` variants, column lists and the family name const in `schema.rs`.
2. `PrimaryKey` variants, constructors, `row_id`, `Display`, `parse_key` and `check_inventable`
   arms in `delta.rs`; refusals in `error.rs`.
3. The Package Identifier Range and its `const _: () = assert!(...)` pair in `ids.rs`.
4. A `ClaimFamily` variant plus `module/src/package_import/<family>.rs` with `update_target`,
   `clear_package_range`, `write_row` and `check_references`.
5. The family name into `PACKAGE_DELTA_DUMP_FAMILIES` in `importer/src/main.rs`.
6. Tests: a per-family identifier test in the crate, a `check_claims_belong_to` scoping test in
   `module/src/package_import.rs`, setter and column-coverage tests in the family module, and the
   `--packages` validation extension in the importer.

## The insert rule this plan settles

The issue requires every table to state whether v1 permits partial updates, inserts, or neither.
One rule decides it for all twenty:

> A table whose primary key names an entity another Import Family owns (a creature template entry,
> an `AreaTrigger.dbc` trigger id, a packed race/class/level tuple) is UPDATE-ONLY in v1. A table
> with its own surrogate key, or keyed by an identifier its own family may invent, permits INSERT
> inside that family's Package Identifier Range, and UPDATE.

No table in these five families is update-only for lack of claimable columns, so nothing repeats the
`game_creature_quest` / `game_gameobject_quest` exclusion. `game_creature_cast` is the closest
shape: both its columns describe the creature-to-spell relation. It stays in the catalogue because
`spell_id` is not a key column, so retargeting a caster mob's spell sets a real column.

Update-only is new machinery. `check_inventable` today has one arm per family and every arm says
"inside the band or refused". T1 adds the arm shape that says "never", plus the refusal that names
the table. Every later ticket depends on it.

## Identifier bands

The decade convention continues past the four bands on `main` (spell 6M, item 7M, quest 8M, loot
9M). Past 9,999,999 the millions column takes two digits, which is the cost of keeping the
non-overlap the `const` asserts enforce:

| Family | Range | Checked against |
|--------|-------|-----------------|
| casts | 10,000,000 to 10,999,999 | `game_creature_spell.id` |
| trainers | 11,000,000 to 11,999,999 | `game_trainer_spell.id` |
| gossip | 12,000,000 to 12,999,999 | each row's own primary key |
| globals | 13,000,000 to 13,999,999 | each row's own primary key |
| spellmeta | 14,000,000 to 14,999,999 | `game_spell_learn.id` only |

`spellmeta`'s two spell-keyed tables (`game_spell_chain`, `game_spell_proc_event`) reuse the
**Package Spell Range** against their own `spell_id`, because such a row is Package-owned exactly
when the spell it describes is. That is the shape the Package Quest Range already uses for the quest
child tables. See T4.

Every reserved band stays where it is. `RESERVED_ID_FLOOR` and `RESERVED_ID_CEIL` do not move: the
gossip ETL's "packages mint above here" floors (menu entry at 1,000,000, option/text/slot at 50,000)
are open-ended upward, so folding them into the reserved span would swallow every Package band.

## Shared rules

- Follow `CODING_STANDARDS.md`. Match the surrounding code's naming and idiom, not its comment
  density: the essay comments in the older files are legacy.
- Use `CONTEXT.md` terms in prose, comments and identifiers. Add one **Package Identifier Range**
  entry per family in the same change that introduces the band.
- Column lists are hand-maintained against the Module struct, in declaration order, with matching
  names and types. A primary-key column, and any column the key already names, stays out of
  `columns()`.
- A packed key formula must match the importer builder and the Module reader exactly. State both
  sources in the doc comment.
- Do not change canonical form, `apply_package_deltas`'s signature, or any existing refusal message.
  `crates/lyracore-package-delta/tests/canonical_form.rs` pins one artifact's exact bytes and a
  Shard stores that digest in `game_package_import.artifact_hash`.
- New columns on the Module's own tables are not part of this work. Nothing here changes a durable
  schema.

## Out of scope for the whole plan

- **`packages replay` generalization.** The CLI verb in the sibling `lyracore-cli` repository
  (`src/cmd/packages/replay.rs`) is hardcoded to the spell family. Items, quests and loot all
  shipped without it and reapply through the importer instead. Generalizing it is a cross-repo
  change with its own pin bump. File a follow-up issue; do not do it here.
- **Authoring-library helpers.** `datascripts/lib/authoring.ts` still exposes `.spell(id)` alone.
  Items, quests and loot shipped without a helper each. Same follow-up.
- **The plan report's fixed "spells"/"effects" columns** in `lyracore-delta-check` and
  `importer/src/package_delta.rs`. Pre-existing, named in PR #357.
- Spatial creatures, spawns, game objects and creature AI (issue #312).
- DBC-backed catalogues (issue #313).
