# Issue #313 DBC catalogues, broken into tickets

Source: `gh issue view 313`, "Package Deltas: support non-spell DBC catalogues".

The spell stream already has a Package Delta stage. Every OTHER client DBC the importer loads has
none. This plan gives the standalone `--dbc` catalogue pass one Import Family, a replay hook, a
provenance stamp, and a closed set of twelve claimable tables, and it writes down why the other
seven are refused.

**This chain stacks on #312, which stacks on #311.** Branch `feat/313-package-delta-dbc` is cut from
`origin/feat/312-package-delta-spatial`. Every ticket here rebases onto the chain head once #311 and
#312 have landed their code: all three chains append to the same closed enums in the same files, and
#313 assumes #311's `DeltaError::InsertNotSupported` and #312's `game_gameobject_template` already
exist.

## Execution order

Strictly sequential, for the same reason #311 and #312 are: every table appends to the same closed
enums.

```text
T1 (the dbc family, the replay hook, provenance, and the three faction/auction tables)
 └── T2 (the remaining Client Mirror Tables: race info, creature family, taxi)
      └── T3 (the inventable tables: graveyard, skill line, skill ability, lock)
           └── T4 (the precise refusal for unsupported DBC tables)
                └── T5 (verification and PR prep)
```

| # | Ticket | Model | Tables | New range |
|---|--------|-------|--------|-----------|
| T1 | the `dbc` family, hook, provenance | opus | 3 | none |
| T2 | remaining Client Mirror Tables | sonnet | 5 | none |
| T3 | the inventable tables | opus | 4 | Package DBC Range |
| T4 | the unsupported-table refusal | sonnet | 0 | none |
| T5 | verification and PR prep | sonnet | 0 | none |

T1 takes opus: it settles the family boundary, the first replay hook outside `spell.rs`, the first
provenance stamp the `--dbc` pass has ever had, the `--packages` validation split, and the Client
Mirror rule every later ticket leans on. T3 takes opus: it carries the only identifier range, two
tables whose durable key is assigned in import order and therefore cannot be named by a claim, and
two cross-family references whose base imports run in different importer invocations.

## The inventory

Nineteen game tables are loaded from client DBCs outside the spell stream, across two importer
invocations.

### `importer --dbc <Data> [--apply]` — `importer/src/dbc.rs::run`, seventeen tables

| Game table | DBC source | Durable key | v1 |
|---|---|---|---|
| `game_faction` | `Faction.dbc` | `faction_id: u32` | update |
| `game_faction_template` | `FactionTemplate.dbc` | `id: u32` | update |
| `game_auction_house` | `AuctionHouse.dbc` | `id: u32` | update |
| `game_race_info` | `ChrRaces.dbc` | `race: u8` | update |
| `game_creature_family` | `CreatureFamily.dbc` | `family_id: u32` | update |
| `game_taxi_node` | `TaxiNodes.dbc` | `id: u32` | update |
| `game_taxi_path` | `TaxiPath.dbc` | `id: u32` | update |
| `game_taxi_path_node` | `TaxiPathNode.dbc` | `id: u32` | update |
| `game_graveyard` | `WorldSafeLocs.dbc` | `id: u32` | insert + update |
| `game_skill_line` | `SkillLine.dbc` | `skill_line: u32` | insert + update |
| `game_skill_ability` | `SkillLineAbility.dbc` | `id: u64` | insert, update of a Package row |
| `game_lock` | `Lock.dbc` | `id: u64` | insert, update of a Package row |
| `game_char_base_info` | `CharBaseInfo.dbc` | `race_class: u16` | EXCLUDED |
| `game_start_item` | `CharStartOutfit.dbc` | `id: u64` | covered by `globals` |
| `game_skill_availability` | `SkillRaceClassInfo.dbc` | `id: u64` | EXCLUDED |
| `game_area` | `AreaTable.dbc` | `id: u32` | EXCLUDED |
| `game_area_trigger` | `AreaTrigger.dbc` | `id: u32` | EXCLUDED |

### `importer --dbc <Data> --talents` — `importer/src/talent.rs::run_talents`, two tables

| Game table | DBC source | Durable key | v1 |
|---|---|---|---|
| `game_talent_tab` | `TalentTab.dbc` | `tab_id` | EXCLUDED |
| `game_talent` | `Talent.dbc` | `talent_id` | EXCLUDED |

### DBC sources with no game table of their own

Named so a reader does not go looking for them: `SkillTiers.dbc` feeds
`dbc::profession_tier_values` and the trainer tier-cap parity guard, `CreatureDisplayInfo.dbc` feeds
`dbc::load_display_scales`, `LockType.dbc` is resolved at import time by `dbc::locktype_skill_line`
so the Module never needs it. None of the three reaches a durable row a Claim could name.

`Spell.dbc` (`--dbc --spells`) is the spell family and is out of this issue's scope.

## Why every excluded table is excluded

T4 turns this list into one refusal per name. One line each, because the refusal prints one line.

| Table | Reason |
|---|---|
| `game_char_base_info` | Its only non-key columns, `race` and `class`, are the components of its own key, so a claim on it could set nothing. |
| `game_start_item` | Already claimable under the `globals` Import Family (#311). Not unsupported: it parses, and its family is `globals`. |
| `game_skill_availability` | No Module game logic reads it; only `debug/fingerprint.rs` counts it, so a claim would change no behaviour. |
| `game_area` | A zone's name, map art and `area_bit` exploration slot live in the client's own `AreaTable.dbc`; a server-side edit diverges from what the player sees. |
| `game_area_trigger` | Trigger volume geometry with no Module reader. The behaviour bound to a trigger is claimable through `game_areatrigger_teleport` under `globals`. |
| `game_talent_tab` | The client draws the talent panes from its own `TalentTab.dbc` and sends the ids it read there. |
| `game_talent` | Same, and retuning what a talent rank grants is better done on the spell itself, under the `spell` family. |

`game_areatrigger_teleport` is not on this list because it is not DBC-backed: it is loaded from the
cmangos `areatrigger_teleport` dump table by `importer/src/main.rs::build_areatrigger_teleport_sql`.
Only its KEY is a `AreaTrigger.dbc` identifier, which is why #311 made it update-only under
`globals`. It needs nothing from this issue.

## Why the DBC catalogues are ONE Import Family

`CONTEXT.md` defines an Import Family as "one named slice of base data the importer loads as a unit,
cleared and reloaded whole". `dbc::run` is exactly one such unit:

- One invocation, one unconditional block. Every one of the seventeen tables is cleared and reloaded
  in the same `if args.apply` arm (`importer/src/dbc.rs` lines 204 to 252). Nothing selects a subset.
- There is no `--family` for the DBC stream. `importer/src/main.rs` refuses `--family` without
  `--dump`, in as many words: "`--family` is only valid with `--dump`".
- Splitting the pass into subject families would invent a selector the importer does not have, and
  give the operator N provenance rows for one atomic reload.

So: **one family, named `dbc`**, the same word the flag, the module file and `import-world.sh`'s own
step name already use. One `ClaimFamily` variant, one `module/src/package_import/dbc.rs`, one
`stamp_family` call, one `package_delta::reapply` call, one identifier range.

`--dbc --talents` is a SECOND clear+reload unit and would be a second family. Both its tables are
excluded, so no second family is needed. Say so in T1's doc comment: it is the reason the "one
family" answer holds.

## The Client Mirror rule

#311 settled one rule for whether v1 permits inserts:

> A table whose primary key names an entity another Import Family owns is UPDATE-ONLY in v1. A table
> with its own key, or keyed by an identifier its own family may invent, permits INSERT inside that
> family's Package Identifier Range, and UPDATE.

That rule is necessary here but not sufficient: the `dbc` family owns every one of its keys, so the
rule alone would permit inserts everywhere. DBC catalogues need a second rule, and it is the whole
point of this issue.

> **A DBC-backed table whose rows the CLIENT also enumerates from its own copy of the same DBC is
> UPDATE-ONLY.** A Package-invented row of such a table exists only in the realm's data; the client
> reads its own file to decide what exists, so the player can never see, name, or select the row.

Call such a table a **Client Mirror Table** and add the term to `CONTEXT.md`. Eight of the twelve
included tables are Client Mirror Tables, and every one of them refuses inserts with
`DeltaError::InsertNotSupported`, the refusal #311 T1 built.

Two of them are ALSO structurally barred from a range, which is worth stating separately because a
maintainer who later repeals the Client Mirror rule for them still cannot lift the bar:

- `game_race_info.race` is a `u8`. No range value fits.
- `game_taxi_node.client_node_id` is a one-based bit position in the vanilla 256-bit known-node mask
  (`lyracore_shared::constants::taxi_protocol`, `NODE_MASK_WORDS * 32`), and the headless flight
  fixture already holds the top two positions. There is no room to invent a node that crosses the
  protocol intact.

## The client divergence, named rather than solved

The issue asks for the divergence to be handled the way the spell family handles it. The spell
family NAMES it and does not solve it, in `crates/lyracore-package-delta/src/ids.rs` (the Package
Spell Range doc comment) and in `CONTEXT.md`'s **Package Spell** entry:

> A real client renders a spell by looking the identifier up in its OWN `Spell.dbc`, so a Package
> spell shows no tooltip on an unmodified client. That is expected: the identifier is safe because
> it can never collide with real or fixture data, not because the client knows it.

Match that exactly. Every included table's doc comment states, in one sentence, what the client
still reads from its own DBC and therefore what a claim does not change. The four kinds:

1. **Fully server-authoritative.** The Module computes the behaviour and the wire carries the
   result. `game_faction`, `game_faction_template`, `game_graveyard`, `game_lock`,
   `game_taxi_path_node` geometry, `game_auction_house` fee rates, `game_creature_family` food mask
   and tameability. A claim here is honoured end to end.
2. **Honoured, but the label diverges.** `game_auction_house.name`, `game_taxi_node.name`,
   `game_skill_line.name`: the server stores the new string, the client prints the old one.
3. **Honoured, but the client will not offer it.** `game_taxi_path` between two nodes the client's
   own `TaxiPath.dbc` does not connect: the Module resolves the route and plays the passenger
   spline, but the flight map never lists it as a destination. This is why the taxi tables are
   update-only.
4. **Invisible until the operator patches their own client.** A Package skill line shows a blank
   name in the skill pane, exactly as a Package Spell shows no tooltip.

`importer/src/pack_client.rs` already re-serializes a DBC into the operator's OWN `patch-3.MPQ`, but
its DBC-overlay stage is an identity round-trip spike today, marked as such in the source. Pushing
Package rows through it is OUT of scope: the issue forbids generating or distributing DBC binaries,
and the packer writes only into the operator's own install. Name the seam in T1's doc comment and
file a follow-up; do not build it here.

## Shard routing

**Every Shard, unconditionally. No World Import Scope filter.** These are global catalogues, and
that is already the shipped arrangement, stated twice in `importer/scripts/import-world.sh`:

- Its header: "The non-spatial families (`Spell.dbc`, the DBC character-creation/faction/area/
  graveyard tables, the world-wide `areatrigger_teleport` portal table, quest/item/loot templates)
  are deliberately on BOTH shards".
- Its map-switch preflight: "Non-spatial tables (spells/items/quests/DBC/areatrigger_teleport) are
  deliberately NOT consulted: every shard carries those."

`dbc::run` has no map fence and no bounding box; it loads every row of every table it reads. So
#312's `WorldImportScope` claim filter has nothing to do here, and the tickets must not reach for
it. Every Shard runs the same `--dbc` pass and gets the same claims.

## The base-import owner and the replay hook

**Base-import owner: `importer --db <shard> --dbc <client Data> --apply`**, the standalone DBC
catalogue pass, `importer/scripts/import-world.sh` line 438. It is the SOLE loader of all seventeen
tables, and it is gated on `--dump` being absent, so a combined `--dump --dbc` run skips it entirely
(that run uses the DBCs only for `Scale==0` resolution and profession tier caps).

**The hook, following the spell precedent exactly.** `importer/src/spell.rs` ends `run_spells` with:

```rust
match &args.packages {
    Some(root) => package_delta::reapply(args, "spell", root)?,
    None => package_delta::warn_not_reapplied("spell"),
}
```

placed after the apply/dry-run block so it runs on BOTH paths: the dry run IS the read-only check,
and it must print the plan an apply would send. `dbc::run` takes the same call, in the same place,
with `DBC_FAMILY`, after the `if args.apply { … } else { … }` block and before the coverage prints.

The `dbc` family does NOT join `PACKAGE_DELTA_DUMP_FAMILIES`. That list names `--dump` families and
is read by `run_package_stage` and by the `--packages` validation; the spell family is not in it
either. See T1 for the validation change the new hook needs.

## Provenance

`dbc::run` stamps nothing today: `stamp_family` is called only from `importer/src/main.rs`'s dump
loop and from `spell.rs`. T1 adds the `dbc` stamp inside the `args.apply` arm, before the reapply,
matching `spell.rs`'s call shape. Per-Package provenance in `game_package_import` then arrives for
free: the `apply_package_deltas` reducer writes it per family.

## Identifier range

One range, because the family is one family.

| Range | Value | Checked against |
|---|---|---|
| Package DBC Range | 17,000,000 to 17,999,999 | `game_graveyard.id`, `game_skill_line.skill_line`, `game_skill_ability.id`, `game_lock.id` |

The four identifier spaces are independent, which is the argument the Package Loot Range already
makes for its four tables. 17,000,000 continues the decade convention (#311 took 10M to 14M, #312
took 15M and 16M), clears `RESERVED_ID_CEIL` (5,200,499) by more than a decade, and fits every one
of the four key types. Same two `const _: () = assert!(…)` lines every other range carries.

No bit-layout assert is needed, unlike #312: no key here is a field inside a packed guid.

## The fill-in checklist

The #311 and #312 checklist, unchanged, minus the two spatial additions:

1. `Table` variants, column lists and the family name const in `schema.rs`.
2. `PrimaryKey` variants, constructors, `row_id`, `Display`, `parse_key`, `is_key_column` and
   `check_inventable` arms in `delta.rs`; refusals in `error.rs`.
3. The Package DBC Range and its `const` asserts in `ids.rs` (T3 only).
4. A `ClaimFamily::Dbc` variant plus `module/src/package_import/dbc.rs`.
5. **Different from #311 and #312:** the hook goes in `importer/src/dbc.rs`, not into
   `PACKAGE_DELTA_DUMP_FAMILIES`, and the `--packages` validation in `importer/src/main.rs` grows
   an arm for it.
6. Tests at every rung.

No `partition_discipline_tripwire::WHITELIST` entry is needed: none of these twelve tables is a
watched spatial accessor.

## Cross-family references, and the one ordering hazard

Three references leave the family. Two are soft by existing doctrine; one is a real constraint the
tickets must record.

- **`game_skill_ability.spell_id` → `game_spell`.** Do NOT hard-check it. `import-world.sh` runs the
  standalone `--dbc` pass BEFORE the full `Spell.dbc` import, so on a fresh Shard `game_spell` is
  empty when the `dbc` reapply runs. The Module already takes this line for talents: "a talent's
  rank-1 spell may reference a `Spell.dbc` row that hasn't been imported yet — a missing header is a
  graceful no-op at learn time, never a hard error" (`importer/src/talent.rs` header, and
  `module/src/talent.rs::do_learn_talent`). Match it and cite it.
- **`game_lock.property` when `kind == 1` → an item entry.** Same softness, same reason: the items
  family loads in a different invocation.
- **`game_graveyard` ← `game_graveyard_zone.safe_loc_id`, and this one bites.** #311's `globals`
  family hard-refuses a `game_graveyard_zone` claim whose `safe_loc_id` is not a `game_graveyard`
  row, deliberately. The `globals` reapply runs inside the `--dump` invocation; the `dbc` reapply
  runs in the LATER standalone `--dbc` invocation. So a Package that ships "a new graveyard, and a
  zone that releases to it" has its `globals` half refused, because the `dbc` half has not run yet.
  Record it in `module/src/package_import/dbc.rs`'s doc comment as a constraint on the pass order,
  with the operator's workaround (run the world import twice, or run the standalone `--dbc` pass
  first). Do NOT reorder `import-world.sh` here — see the open question below.

## Decisions taken here, with the reason

Settled. A ticket that disagrees should say so out loud rather than quietly diverge.

1. **One Import Family, `dbc`, not one per subject.** A family is the unit a base import clears and
   reloads whole; `dbc::run` is exactly one such unit and has no `--family` selector. Splitting it
   would invent a selector the importer does not have.
2. **The Client Mirror rule.** #311's insert rule is necessary but not sufficient here, because this
   family owns every one of its keys. Without a second rule every DBC table would be inventable,
   and eight of the twelve would then let an author create rows no player can see.
3. **A claim key is the DBC record id.** True for ten of the twelve tables, and it is what makes a
   claim on a client catalogue nameable at all. The two exceptions are stated as exceptions.
4. **`game_skill_ability` and `game_lock` take updates only inside the range.** Their durable key is
   a 1-based `enumerate` over the DBC, so the number an author writes down addresses a different row
   after the client's data changes. Refusing is better than a claim that silently lands elsewhere.
5. **An inserted `game_lock` names a Package `lock_id` too.** A Package invents a whole lock rather
   than adding a ninth alternative to a vanilla one. This makes `clear_package_range` correct by
   scanning `id` alone and removes every duplicate-`(lock_id, index)` question. The dropped
   capability is small and is named in T3.
6. **`game_skill_ability.spell_id` and `game_lock.property` are not checked.** Their base imports run
   in later invocations, and the Module already treats a missing spell header as a graceful no-op.
7. **No World Import Scope filter.** #312 needed one because its rows are map-owned. These are global
   catalogues that every Shard carries, which `import-world.sh` states twice.
8. **The client divergence is named, not solved.** The spell family's precedent, matched exactly.

## Open questions for the maintainer

Recommendation first, in each case.

1. **Is the twelve-table set right?**
   Recommended: yes as listed. Four inclusions are settled and would be strange to drop:
   `game_graveyard` (no client divergence, and #311 already lets a Package link a zone to a
   graveyard it cannot create), `game_skill_ability` with `game_skill_line`, `game_auction_house`,
   and the two faction tables. Three are debatable and would be clean cuts if the set feels wide:
   - **The taxi trio.** Fare tuning and flight geometry are genuinely server-authoritative, and the
     issue brief names taxi routes. Against: three tables and their tests buy one fare column, one
     landing point and one waypoint set, and a Package cannot add a route the client will offer.
   - **`game_race_info`.** Cheap (three columns, no range) and its `faction_template` column is a
     real referential check. Against: niche.
   - **`game_skill_line`.** No Module game logic reads it today. It is in mostly as
     `game_skill_ability`'s reference target. If it goes, an ability may only name a vanilla line.
2. **Should `import-world.sh` run the standalone `--dbc` pass BEFORE the combined `--dump --dbc`
   pass?**
   Recommended: yes, but in a follow-up, not in this chain. It is the clean fix for the
   `game_graveyard_zone` ordering hazard, and it is safe on the evidence: the two passes' only
   shared table is `game_start_item` and their DELETEs are bounded disjointly, and no other table is
   written by both. It is still an operator-visible reorder of a shipped script and belongs with its
   own verification run, not inside a Package Delta chain.
3. **Family name: `dbc`?**
   Recommended: yes. It is the word the flag, the module file and the script step already use, and
   the family name is operator-facing (`game_import_meta.family`, `--family`-style prose in
   refusals). The alternative, `catalogues`, reads better in prose and matches nothing in the tree.
4. **Should `game_skill_availability` come in later?**
   Recommended: leave it out until something reads it. It is excluded for having no reader, not for
   a shape problem, so the exclusion reason will go stale the day weapon-master or profession
   availability lands. T4's list is one line to change.

## Shared rules

- Follow `CODING_STANDARDS.md`. Match the surrounding code's naming and idiom, not its comment
  density.
- Use `CONTEXT.md` terms. Add the **Package DBC Range** and **Client Mirror Table** entries in the
  same change that introduces each.
- Column lists are hand-maintained against the Module struct, in declaration order, with matching
  names and types. A key column, and any column the key already names, stays out of `columns()`.
- Do not change canonical form, `apply_package_deltas`'s signature, or any existing refusal message.
  `crates/lyracore-package-delta/tests/canonical_form.rs` pins one artifact's exact bytes.
- No Module durable schema change. Nothing here adds or renames a column.
- The licensing firewall is unchanged and unchallenged: a Package Delta is a typed row artifact.
  Nothing in this plan reads, writes, commits or distributes a Blizzard-format file. T5 asserts it.

## Out of scope for the whole plan

- **Spell headers and effects.** The issue says so; the spell family already has its stage.
- **Generating or distributing DBC binaries**, and extending `pack_client.rs`'s identity overlay
  stage to carry Package rows. The issue says so.
- **Terrain, navigation, vmap and model assets.** The issue says so.
- **`--dbc --talents`.** Both its tables are excluded, so it gains no family and no hook.
- **`packages replay` generalization.** The CLI verb in the sibling `lyracore-cli` repository is
  still hardcoded to the spell family. Same follow-up #311 and #312 named, and the `dbc` family is
  the second family outside `--dump` that wants it.
- **Base Snapshots for the `dbc` family.** The convention is settled (one file per family) and only
  the spell family has one. It matters more here than for #311 or #312: a claim on this family is
  keyed by a DBC record id, and an author has no repo-local place to read those ids from. Same
  follow-up, and say so out loud in T5's PR text.
- **Authoring-library helpers.** `datascripts/lib/authoring.ts` still exposes `.spell(id)` alone.
- **Threading `--packages` through `import-world.sh`.** The script passes it to no invocation today,
  not even the spell one; `lyracore import` in the sibling CLI repository drives it. Nothing here
  regresses, and fixing it is that repository's change.
