# T3: the globals Import Family

Parent: issue #311. **Depends on T2. Rebase onto it before starting. Run alone.**
Model: opus. Eight tables, three distinct primary-key shapes, three packed composite keys whose
formulas must match the importer and the Module byte for byte, and one table with two importer
writers.

## Goal

Let a Package tune the world-wide reference data a realm starts from: the stat curve, start
positions, graveyard links, dungeon portals, and the character-creation kit.

## Read first

- T1's `InsertNotSupported` refusal and T2's multi-table `check_references`.
- `importer/src/main.rs` lines 1548 to 1873 (the seven `globals` builders) and 4954 to 4996 (the
  `--dump` block).
- `module/src/stats.rs` (`ClassLevelStats`, `LevelStats`), `module/src/config.rs`
  (`StartPosition`, `StartItem`), `module/src/graveyard.rs` (`GraveyardZone`),
  `module/src/quest.rs` around line 2295 (`AreatriggerTeleport`),
  `module/src/spell/spellbook.rs` (`CreateinfoSpell`), `module/src/action_bar.rs`
  (`CreateinfoAction`).
- `crates/lyracore-package-delta/src/ids.rs`, the `packed_quest_objective_id` family of helpers.
  The three packed keys below follow that shape.

## Why globals stays one Import Family

The issue requires every supported table to map to exactly one Import Family, and the importer
already treats these eight as one clear plus reload unit under `--family globals`. Splitting them
for the Package Delta side would put a table in a family no base import stamps, so a Package apply
would run out of turn. Keep the family whole even though its members have little in common.

## Tables and v1 policy

### Update-only: the three packed-key tables

Their keys pack a tuple that names a class, a race and a level. A Package cannot invent a class, a
race or a level, so `InsertNotSupported` applies to all three. In every one, the unpacked component
columns are named by the key and therefore stay OUT of `columns()`, the same rule
`game_spell_effect` follows for `spell_id` and `effect_index`.

**`game_class_level_stats`** (`module/src/stats.rs`, struct `ClassLevelStats`)

- Durable key: `class_level: u32`. Claim key: `{ class: u8, level: u32 }`.
- Packed formula, matching `build_stats_sql`: `(class << 8) | (level & 0xFF)`.
- Claimable columns: `base_health` (u32), `base_mana` (u32).

**`game_level_stats`** (same file, struct `LevelStats`)

- Durable key: `race_class_level: u32`. Claim key: `{ race: u8, class: u8, level: u32 }`.
- Packed formula: `(race << 16) | (class << 8) | (level & 0xFF)`.
- Claimable columns: `strength`, `agility`, `stamina`, `intellect`, `spirit`, all u32.

**`game_start_position`** (`module/src/config.rs`, struct `StartPosition`)

- Durable key: `race_class: u16`. Claim key: `{ race: u8, class: u8 }`.
- Packed formula: `(race << 8) | class`.
- Claimable columns: `map_id` (u32), `zone_id` (u32), `x` (f32), `y` (f32), `z` (f32),
  `orientation` (f32), `display_id` (u32).
- `display_id` is vestigial (the importer emits 0 and player display comes from `game_race_info`).
  Keep it claimable, since it is a real column, and say it is vestigial in the doc comment.

The `& 0xFF` in the first two formulas is the importer's, and it truncates a level above 255.
Reproduce the formula exactly rather than improving it, and bound `level` in the key constructor so
a claim that would truncate is refused instead of silently landing on another row. Add
`MAX_STAT_LEVEL` to `ids.rs` alongside `MAX_SPELL_EFFECT_INDEX`, with the same "the real domain is
narrower than the packed field" reasoning. Read the Module's own level cap and use it.

### Update-only: the DBC-keyed table

**`game_areatrigger_teleport`** (`module/src/quest.rs`, struct `AreatriggerTeleport`)

- Primary key: `trigger_id: u32`, the `AreaTrigger.dbc` trigger id.
- Claimable columns: `target_map` (u32), `x` (f32), `y` (f32), `z` (f32), `o` (f32), `name`
  (string).
- v1 policy: **partial update only.** The key is a client DBC identifier, and DBC-backed catalogues
  are out of this issue's scope (issue #313 owns them). Retargeting an existing portal is the whole
  operation v1 offers.

### Insert and partial update: the four surrogate-key tables

**`game_graveyard_zone`** (`module/src/graveyard.rs`, struct `GraveyardZone`)

- Primary key: `row_id: u64`, `#[auto_inc]`.
- Claimable columns: `safe_loc_id` (u32), `zone_id` (u32), `faction` (u32).

**`game_createinfo_spell`** (`module/src/spell/spellbook.rs`, struct `CreateinfoSpell`)

- Primary key: `id: u64`, `#[auto_inc]`.
- Claimable columns: `race` (u8), `class` (u8), `spell_id` (u32).
- Note for the doc comment: `race` and `class` are wildcards at 0 on read
  (`createinfo_row_matches`), so a claim setting either to 0 is meaningful, not a mistake.

**`game_createinfo_action`** (`module/src/action_bar.rs`, struct `CreateinfoAction`)

- Primary key: `row_id: u64`, `#[auto_inc]`.
- Claimable columns: `race` (u8), `class` (u8), `button` (u8), `action` (u32), `action_type` (u8).

**`game_start_item`** (`module/src/config.rs`, struct `StartItem`)

- Primary key: `id: u64`, `#[auto_inc]`.
- Claimable columns: `race_class` (u16... see below), `item_entry` (u32).
- `FieldType` has no `u16` gap: `FieldType::U16` exists and `FieldValue::U16` with it. Use it.
- **Two writers own this table.** `importer/src/dbc.rs::start_item_sql` clears
  `id >= 0 AND id < 1000000` and loads the `CharStartOutfit.dbc` outfit;
  `importer/src/main.rs::build_createinfo_item_sql` clears `id >= 1000000` and loads the dump's
  extras under the `globals` family. The `--dbc` pass is a separate invocation with no Package Delta
  stage, and its DELETE is bounded below 1,000,000, so a Package band above that is safe from it.
  The `globals` DELETE is open-ended upward and DOES reach the band, but the Package stage runs
  immediately after it in the same invocation and rewrites the band, which is the same arrangement
  loot lives with. Record both facts in the doc comment; do not change either DELETE.

## Identifier band

**Package Globals Range: 13,000,000 to 13,999,999**, checked against each row's own primary key,
across the four inventable tables only. Constants `PACKAGE_GLOBALS_ID_FLOOR` / `_CEIL`,
`is_package_globals_id`, `is_fixture_reserved_globals_id`, and the two `const` asserts.

All four inventable keys are `u64`, so a `u64` band is the natural type, matching
`PACKAGE_LOOT_ID_FLOOR`.

The band sits above `CURATED_RESERVED_ID_CEIL` (5,200,499) and therefore above
`CREATEINFO_ITEM_ID_FLOOR` (1,000,000), which is what keeps a Package `game_start_item` row out of
the createinfo extras' own identifier space. The assert already enforces this; say why it matters
for this family specifically.

## Cross-references

In `module/src/package_import/globals.rs`, on final merged values:

- `game_graveyard_zone.safe_loc_id` must be a `game_graveyard` row. That table is loaded from
  `WorldSafeLocs.dbc` by the separate `--dbc` pass, so on a Shard that has never run `--dbc` it is
  empty and every claim would refuse. Decide and document: refuse (correct, and the operator's fix
  is to run the base imports in order) rather than skip the check when the table is empty.
- `game_createinfo_spell.spell_id` must be a `game_spell` row.
- `game_start_item.item_entry` must be a `game_item_template` row.

Do not check `zone_id`, `map_id` or `action`. No durable table answers for them.

## Module family and importer

New `module/src/package_import/globals.rs`. Eight tables makes this the largest family module in the
tree; keep the per-table blocks in the same order as `Table::ALL` and follow `quest.rs`'s layout
(blank builder, field setter, built-row helper, per table). Wire `ClaimFamily::Globals` through
`package_import.rs`. Add `"globals"` to `PACKAGE_DELTA_DUMP_FAMILIES`.

`clear_package_range` covers the four inventable tables only.

## Files owned

Same set as T1 and T2, plus `module/src/package_import/globals.rs` and
`crates/lyracore-package-delta/tests/globals_identifiers.rs`.

## Out of scope

- `spellmeta`. T4 owns it.
- Changing either `game_start_item` DELETE, or the `& 0xFF` truncation in the stat key formulas.
- Any DBC-backed catalogue (issue #313).

## Acceptance tests

1. `tests/globals_identifiers.rs`: band membership for the four inventable tables, fixture-reserved
   refusal, out-of-band insert refused by name, malformed keys.
2. Packed-key derivation tests for all three composite keys, asserting the exact value the
   importer's own formula produces for a worked tuple. Assert against a constant, not against a
   reimplementation.
3. A level above `MAX_STAT_LEVEL` is refused rather than truncated.
4. All four update-only tables refuse every insert with `InsertNotSupported` naming the table.
5. `tests/families.rs`: eight new tables parse back to themselves, belong to `globals`, each has
   claimable columns.
6. `a_globals_plan_claims_only_globals_family_tables` in `module/src/package_import.rs`.
7. Setter and column-coverage tests per table in `globals.rs`.
8. `check_references` tests for all three checks, including the empty-`game_graveyard` case.
9. Importer `--packages` validation accepts `--family globals`.
10. Crate, module lib and importer suites pass; wasm release build succeeds; clippy and rustfmt
    clean on touched files.

## Definition of done

A Package can move a start position, retune the stat curve, retarget a dungeon portal, add a
graveyard link, and add a starting spell, action and item, and all of it survives a base reimport.
`CONTEXT.md` carries the Package Globals Range.
