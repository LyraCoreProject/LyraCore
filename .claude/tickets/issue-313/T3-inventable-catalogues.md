# T3 — the inventable catalogues: graveyards, skill lines, skill abilities, locks

Model: opus

Depends on: T2. Rebase onto T2 before starting.

## Goal

Add the four `dbc` tables a Package may INVENT rows in, and the one identifier range that covers all
four. This is the ticket that makes the `dbc` family more than a tuning surface: a Package can ship
a graveyard, a skill line, a spell-to-skill binding, and a lock for the chest #312 lets it spawn.

Opus because two of the four tables have a durable key the importer assigns in import order, which
no claim can name, and because the graveyard has a cross-family reference whose base import runs in
a different importer invocation.

## Read first

- `.claude/tickets/issue-313/README.md`, the identifier range and the cross-reference section.
- `crates/lyracore-package-delta/src/ids.rs`, the whole header and the Package Loot Range, which is
  the precedent for one range spanning several independent key spaces.
- `importer/src/dbc.rs::skill_ability_sql` and `dbc::lock_sql`. Both number their rows with a
  1-based `enumerate`, which is the fact this ticket is built around.
- `importer/src/dbc.rs::graveyard_sql` and `module/src/graveyard.rs::resolve_graveyard`.
- `module/src/gameobject.rs` around lines 627 and 714: how a lock gates opening.
- `.claude/tickets/issue-311/T3-globals.md`, the cross-references section, for the
  `game_graveyard_zone.safe_loc_id` check this ticket has to reason about.

## The identifier range

**Package DBC Range: 17,000,000 to 17,999,999.** Constants `PACKAGE_DBC_ID_FLOOR` / `_CEIL`,
`is_package_dbc_id`, `is_fixture_reserved_dbc_id`, and the two `const _: () = assert!(…)` lines every
other range carries: floor at or under ceiling, and `RESERVED_ID_CEIL` strictly under the floor.

Checked against four independent identifier spaces:

| Table | Identifier |
|---|---|
| `game_graveyard` | `id` (u32) |
| `game_skill_line` | `skill_line` (u32) |
| `game_skill_ability` | `id` (u64) |
| `game_lock` | `id` (u64), and `lock_id` (u32) on an insert |

Four spaces, one range, no collision: the same argument the Package Loot Range already makes for its
four tables. No bit-layout assert is needed; unlike #312 no key here is a field inside a packed guid.

Add the **Package DBC Range** entry to `CONTEXT.md` in this change.

## Tables

Struct sources are `module/src/graveyard.rs` (`GraveyardLoc`), `module/src/skilldata.rs`
(`SkillLine`, `SkillAbility`) and `module/src/gameobject.rs` (`GameLock`).

**`game_graveyard`** — `WorldSafeLocs.dbc`, importer `dbc::graveyard_sql`

- Claim key: `{ id: u32 }`, the `WorldSafeLocs.dbc` record id.
- Claimable columns: `map_id` (u32), `x`, `y`, `z`, `o` (f32), `name` (string).
- v1: insert inside the range, and update of any row.
- No client divergence at all. `module/src/graveyard.rs::resolve_graveyard` picks the nearest safe
  location and the server teleports the releasing player there; the client reads nothing from
  `WorldSafeLocs.dbc` to make that happen. This is the strongest inclusion in the family and the doc
  comment should say why.
- `o` is 0.0 on every imported row: `WorldSafeLocs.dbc` carries no orientation, and only the seeded
  rows in `module/src/seed.rs` ever set it. Keep it claimable, and say it is normally 0.

**`game_skill_line`** — `SkillLine.dbc`, importer `dbc::skill_line_sql`

- Claim key: `{ skill_line: u32 }`, the `SkillLine.dbc` record id.
- Claimable columns: `category` (u32), `name` (string).
- v1: insert inside the range, and update of any row.
- Honest note for the doc comment: no Module game logic reads this table today; `module/src/skill.rs`
  still uses hardcoded skill lines and names this table as the intended source of truth. It is in the
  catalogue because it is the reference target `game_skill_ability.skill_line` needs, and because a
  Package that invents an ability wants to name its own line.
- Client divergence, the fourth kind: a Package skill line shows a blank name in the client's skill
  pane, exactly as a Package Spell shows no tooltip. Cite the Package Spell Range doc comment.

**`game_skill_ability`** — `SkillLineAbility.dbc`, importer `dbc::skill_ability_sql`

- Claim key: `{ id: u64 }`.
- Claimable columns, in declaration order: `spell_id` (u32), `skill_line` (u32), `race_mask` (u32),
  `class_mask` (u32), `min_skill` (i32), `acquire_method` (i32), `gray` (i32), `green` (i32).
- v1: insert inside the range, and update ONLY of a row inside the range. See below.
- Read by `module/src/skill.rs` (skill-up gray/green band), `module/src/mount.rs` (the riding-skill
  gate) and `module/src/spell/cast/targeting.rs` (the cast-time skill requirement). Fully
  server-authoritative.
- The column names are the REMAPPED ones, not `wow_dbc`'s. `dbc::skill_ability_sql` reads
  `SkillLineAbility.dbc` by content because `wow_dbc` 0.3 mis-names every field from position 8.
  The Module's column names are already the corrected ones, so nothing to do; note it so a reader
  comparing the two files does not think one is wrong.

**`game_lock`** — `Lock.dbc`, importer `dbc::lock_sql`

- Claim key: `{ id: u64 }`.
- Claimable columns: `lock_id` (u32), `index` (u8), `kind` (u8), `property` (u32),
  `required_skill` (u32).
- v1: insert inside the range, and update ONLY of a row inside the range.
- One `Lock.dbc` record explodes into up to eight `game_lock` rows, one per non-`None` alternative,
  so `(lock_id, index)` is the natural identity and `id` is a counter.
- `kind` is the importer's own taxonomy, not the DBC's: 1 = an item is required (`property` is an
  item entry), 2 = a skill is required (`property` is a skill line, resolved from `LockType.dbc` at
  import time by `dbc::locktype_skill_line`). A claim states the resolved value; the Module never
  sees `LockType.dbc`. Say this, because it is the single most surprising column in the family.
- Client note: the client's own `Lock.dbc` drives the cursor and tooltip; the server decides whether
  the open succeeds. So a Package lock gates correctly and may show the wrong hint.

### Import-order keys, and what that costs

`skill_ability_sql` and `lock_sql` both number their rows with a 1-based `enumerate` over the DBC.
The value therefore depends on how many rows precede it, and changes whenever the operator's client
data changes. A claim cannot name a base row of either table: the number it wrote down last month
may address a different row today.

So both tables take one extra rule, and it needs a new refusal:

> On a table whose durable key is assigned in import order, an UPDATE must also name a key inside
> the Package DBC Range. A Package may update only a row it inserted.

`DeltaError::UpdateNotAddressable { table }`, whose message says why in one sentence: the rows
outside the range are numbered in import order, so no claim can name one. This is narrow on purpose;
`game_graveyard` and `game_skill_line` carry the DBC's own record id and take updates on any row.

Note in the doc comment that this is what stops `game_skill_ability` from being an ordinary
retuning surface: retuning a VANILLA ability's gray/green band is not expressible in v1, and the
fix, if one is ever wanted, is a claim key of `(spell_id, skill_line)` resolved by index lookup,
which the current `PrimaryKey::row_id` (a `const fn`, derivation only) cannot express.

### Insert constraints beyond the range

- `game_lock`: an inserted row's `lock_id` must ALSO be inside the range, so a Package invents a
  whole lock rather than adding a ninth alternative to a vanilla one. That keeps
  `clear_package_range` correct by scanning `id` alone, and removes every question about a duplicate
  `(lock_id, index)`. State the capability this drops.
- `game_lock`: `index` must be 0..=7, the `Lock.dbc` array width the Module's readers assume.
- `game_lock`: `kind` must be 1 or 2. `0` is not a value `lock_sql` ever emits.

## Cross-references

In `module/src/package_import/dbc.rs`, on final merged values:

- `game_skill_ability.skill_line` must be a `game_skill_line` row, OR a row the same plan inserts.
  That is #312's template-rides-its-spawns rule applied here, and it is what lets a Package ship a
  line and an ability together.
- `game_skill_ability.spell_id` is NOT checked. `import-world.sh` runs the standalone `--dbc` pass
  BEFORE the full `Spell.dbc` import, so `game_spell` is empty on a fresh Shard when this reapply
  runs. The Module already takes this line for talents: a missing spell header is a graceful no-op
  at learn time, never a hard error. Cite `importer/src/talent.rs`'s header and
  `module/src/talent.rs::do_learn_talent`, and say the check is omitted deliberately rather than
  forgotten.
- `game_lock.property` is NOT checked, for the same reason: when `kind == 1` it is an item entry
  owned by the `items` family, loaded in a different invocation.
- `game_graveyard` needs no outbound check. It is the TARGET of one, and that is the hazard below.

### The `game_graveyard_zone` ordering hazard

#311's `globals` family hard-refuses a `game_graveyard_zone` claim whose `safe_loc_id` is not a
`game_graveyard` row. The `globals` reapply runs inside the `--dump` invocation; the `dbc` reapply
runs in the LATER standalone `--dbc` invocation. So a Package that ships "a new graveyard, and a
zone that releases to it" has its `globals` half refused on a fresh Shard, because its `dbc` half
has not run yet.

Do not fix it here. Record it in the family doc comment with the operator's workaround: run the
world import a second time, or run the standalone `--dbc` pass before the `--dump` pass. Reordering
`import-world.sh` is safe on the evidence (the two passes' `game_start_item` DELETEs are bounded
disjointly and no other table is written by both) but it is an operator-visible change to a shipped
script, so it is the maintainer's call. See the README's open questions.

## Files owned

- `crates/lyracore-package-delta/src/schema.rs`, `delta.rs`, `error.rs`, `ids.rs`, `lib.rs`
- `crates/lyracore-package-delta/tests/families.rs`, `tests/dbc_identifiers.rs`
- `module/src/package_import/dbc.rs`
- `CONTEXT.md`

## Out of scope

- Changing `skill_ability_sql`'s or `lock_sql`'s numbering. It is the importer's, it is stable
  within one client's data, and rewriting it is a base-import change this issue does not need.
- A lookup-shaped claim key. `PrimaryKey::row_id` is a `const fn`; adding a lookup is a change to
  the claim machinery, not to this family.
- Reordering `import-world.sh`.
- `game_skill_availability`. Excluded; T4 gives it its refusal.

## Acceptance tests

1. `tests/dbc_identifiers.rs`: band membership for all four identifier spaces; an insert below the
   floor or above the ceiling is refused and names the identifier; a reserved-band identifier is
   refused as fixture-reserved.
2. An update naming a `game_skill_ability` or `game_lock` key outside the range is refused with
   `UpdateNotAddressable`, and an update inside the range succeeds.
3. An update naming any `game_graveyard` or `game_skill_line` row succeeds, inside or outside the
   range, so the narrowness of rule 2 is pinned.
4. `game_lock` insert refusals: `lock_id` outside the range, `index` above 7, `kind` outside 1..=2.
5. `check_references`: an ability whose `skill_line` names neither an existing row nor one this plan
   inserts is refused naming both tables; an ability and its line in one plan are accepted.
6. A test asserting `spell_id` and `property` are NOT checked, with a comment pointing at the
   ordering reason, so a later reader does not "fix" it.
7. `tests/families.rs`: four new names parse back to themselves, report family `dbc`, each has
   claimable columns.
8. `clear_package_range` covers exactly the four inventable tables, and a test that a base reload
   followed by a reapply leaves no orphan Package row.
9. Crate, module lib and importer suites pass; wasm release build succeeds; clippy and rustfmt clean
   on touched files.

## Definition of done

A Package can add a graveyard and release to it, invent a skill line and bind a spell to it with its
own skill-up band, and invent a lock for a gameobject it spawns, and all of it survives the next
`--dbc` reload. `CONTEXT.md` carries the Package DBC Range. Twelve tables now belong to the `dbc`
family.
