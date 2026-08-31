# T2: the gossip Import Family

Parent: issue #311. **Depends on T1. Rebase onto it before starting. Run alone.**
Model: sonnet.

## Goal

Let a Package write what an NPC says. Six tables, one Package Identifier Range, and the first
family where a cross-reference web inside one family matters more than a reference out of it.

## Read first

- T1's diff, especially the `InsertNotSupported` refusal it introduced. `game_gossip_menu` needs it.
- `module/src/creatures/spawn.rs` lines 137 to 190: `GossipMenu`, `GossipMenuProfile`,
  `GossipMenuProfileOption`, `NpcText`, `NpcTextSlot`, `GossipOption`
- `importer/src/main.rs` lines 4777 to 4824 (the `--dump` gossip block and its reserved-range
  comment)
- `module/src/package_import/quest.rs` (the closest template: several tables in one family with
  references between them)

## Tables and v1 policy

All six live in `module/src/creatures/spawn.rs`.

**`game_gossip_menu`** (struct `GossipMenu`)

- Primary key: `entry: u32`. Read the struct's doc comment carefully: this is the **creature
  template entry**, not the cmangos `gossip_menu.entry`. The importer collapsed that indirection.
- Claimable columns: `text_id` (u32).
- v1 policy: **partial update only. No inserts.** The key names a creature template, so the T1
  refusal applies. Note the consequence in the doc comment: the way a Package gives an NPC new
  words is to insert a `game_npc_text` row in the band and then update this row's `text_id` to
  point at it.

**`game_npc_text`** (struct `NpcText`)

- Primary key: `text_id: u32`.
- Claimable columns: `text` (string).
- v1 policy: **insert and partial update.** Band checked against `text_id`.

**`game_npc_text_slot`** (struct `NpcTextSlot`)

- Primary key: `id: u64`, `#[auto_inc]` surrogate.
- Claimable columns: `text_id` (u32), `slot_index` (u8), `text_male` (string), `text_female`
  (string), `probability` (f32).
- v1 policy: **insert and partial update.** Band checked against `id`.

**`game_gossip_option`** (struct `GossipOption`)

- Primary key: `row_id: u32`, a surrogate the importer assigns densely.
- Claimable columns: `entry` (u32), `option_index` (u32), `icon` (u32), `text` (string), `action`
  (u32), `action_menu_id` (u32), `cond_type` (u32), `cond_value1` (u32), `cond_value2` (u32).
- v1 policy: **insert and partial update.** Band checked against `row_id`. This is the most useful
  insert in the family: a new clickable option on an NPC that already exists.

**`game_gossip_menu_profile`** (struct `GossipMenuProfile`)

- Primary key: `menu_id: u32`, a runtime-selectable menu identifier.
- Claimable columns: `text_id` (u32).
- v1 policy: **insert and partial update.** Band checked against `menu_id`. Unlike
  `game_gossip_menu.entry`, this identifier space belongs to the gossip family alone, so the family
  may invent one.

**`game_gossip_menu_profile_option`** (struct `GossipMenuProfileOption`)

- Primary key: `row_id: u32`, surrogate.
- Claimable columns: the same nine as `game_gossip_option`, with `menu_id` in place of `entry`.
- v1 policy: **insert and partial update.** Band checked against `row_id`.

No table here has zero claimable columns, so nothing repeats the `game_creature_quest` exclusion.

## Identifier band

**Package Gossip Range: 12,000,000 to 12,999,999**, checked against each row's OWN primary key, the
loot shape rather than the quest shape. The six tables are independent SpacetimeDB tables with
independent key spaces, so one band across all of them cannot collide. `game_gossip_menu` has no
band of its own because it permits no inserts.

Constants `PACKAGE_GOSSIP_ID_FLOOR` / `_CEIL`, `is_package_gossip_id`,
`is_fixture_reserved_gossip_id`, plus the two `const _: () = assert!(...)` lines.

The widths differ across the family: `game_npc_text_slot.id` is `u64` and the other five keys are
`u32`. Pick one band type and convert at the boundary, the way `is_fixture_reserved_loot_id` casts
`FIXTURE_RESERVED_ID_FLOOR`. A `u32` band is the simpler choice here since five of six keys are
`u32`; state the reasoning in the doc comment.

### Check this before you commit to 12,000,000

The gossip ETL block already reserves high identifier bands for Packages that mint gossip through
some other path (`importer/src/main.rs` line 4783: menu `entry` at or above 1,000,000, and
option `row_id` / `text_id` / slot `id` at or above 50,000). Those floors are open-ended upward, so
12,000,000 sits inside every one of them and the base import's DELETEs cannot reach it. That is the
property the band needs, and it is why `RESERVED_ID_FLOOR` / `RESERVED_ID_CEIL` must NOT grow to
cover those floors: doing so would swallow every Package band in the file.

Confirm no shipped Package already occupies 12,000,000 to 12,999,999 in any of the six tables.
Search `packages/` and `content/` for gossip rows. If one does, move the band up a decade and say
why in the constant's doc comment.

## Cross-references

`check_references` in `module/src/package_import/gossip.rs`, judged on each row's FINAL merged
value, the way `loot.rs`'s `final_u32` does:

- `game_gossip_menu.text_id` must be a `game_npc_text` row.
- `game_npc_text_slot.text_id` must be a `game_npc_text` row.
- `game_gossip_option.entry` must be a `game_gossip_menu` row.
- `game_gossip_menu_profile.text_id` must be a `game_npc_text` row.
- `game_gossip_menu_profile_option.menu_id` must be a `game_gossip_menu_profile` row.

A reference may point at a row the SAME plan inserts, so the check must consider the plan's own
rows as well as what the Shard holds. `quest.rs` already solves this for a Package quest and its
children. Follow it.

Do NOT check `action_menu_id` in v1. It is only a menu reference for some values of `action`, and
guessing which would refuse valid claims. Name it as a gap in the doc comment.

## Module family and importer

New `module/src/package_import/gossip.rs` with `update_target`, `clear_package_range` (all five
inventable tables), `write_row` and `check_references`. Wire `ClaimFamily::Gossip` through
`package_import.rs`: `ALL`, `as_str`, and the four dispatch matches.

Add `"gossip"` to `PACKAGE_DELTA_DUMP_FAMILIES` in `importer/src/main.rs`.

One existing test moves: `an_import_family_with_no_artifact_schema_is_refused_by_name` in
`module/src/package_import.rs` uses `"gossip"` as its example of an unsupported family. Pick another
name that this build still does not carry, and keep the assertion that the refusal lists the
families it does.

## Files owned

Same set as T1, plus `module/src/package_import/gossip.rs` and
`crates/lyracore-package-delta/tests/gossip_identifiers.rs`.

## Out of scope

- `globals` and `spellmeta`. T3 and T4 own those.
- Inserting a `game_gossip_menu` row at a real creature entry. That is the "insert at a real
  identifier" question the README defers.
- Anything in `gateway/`. The gossip reader does not change.

## Acceptance tests

1. `tests/gossip_identifiers.rs`: band membership at both ends for each of the five inventable
   tables, fixture-reserved refusal, malformed keys, and an out-of-band insert refused by name.
2. `game_gossip_menu` refuses every insert with `InsertNotSupported` and accepts an update.
3. `tests/families.rs`: six new tables parse back to themselves, belong to `gossip`, each has
   claimable columns.
4. `a_gossip_plan_claims_only_gossip_family_tables` in `module/src/package_import.rs`.
5. Setter and column-coverage tests per table in `gossip.rs`.
6. A `check_references` test for each of the five references, including one where the referenced row
   is inserted by the same plan and must therefore be accepted.
7. Importer `--packages` validation accepts `--family gossip`.
8. Crate, module lib and importer suites pass; wasm release build succeeds; clippy and rustfmt clean
   on touched files.

## Definition of done

A Package can insert an `game_npc_text` row plus its slots and a `game_gossip_option`, point a real
NPC at them by updating `game_gossip_menu.text_id`, and have all of it survive a base reimport.
`CONTEXT.md` carries the Package Gossip Range.
