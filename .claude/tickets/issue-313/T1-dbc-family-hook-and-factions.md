# T1 — the `dbc` Import Family, its replay hook and provenance, and the faction/auction tables

Model: opus

Depends on: nothing in this chain. Rebase onto the #312 chain head before starting; this ticket
needs #311's `DeltaError::InsertNotSupported`, which #312 also assumes.

## Goal

Give the standalone `--dbc` catalogue pass everything the spell stream already has: one Import
Family, a Package Delta replay hook, an import provenance stamp, and a `--packages` flag that is
legal to pass to it. Then land the three simplest Client Mirror Tables through it, so the shell is
exercised by real tables rather than by tests alone.

Nothing in this ticket can invent a row. The identifier range arrives in T3.

## Read first

- `.claude/tickets/issue-313/README.md`, in full. It carries the inventory, the family argument, the
  Client Mirror rule, the routing rule and the exclusion list.
- `importer/src/spell.rs` around line 2228: the reapply call this ticket copies, and its comment
  about running on the dry-run path too.
- `importer/src/spell.rs` around line 2176: the `stamp_family` call, including why `file_hash` is
  `""` for a client-DBC stream.
- `importer/src/dbc.rs` lines 99 to 283: the whole of `run`. The apply arm is 204 to 252 and the
  coverage prints are 254 to 282.
- `importer/src/main.rs` lines 1348 to 1361 (the `--packages` validation) and 5153 to 5170 (the
  `--dbc` dispatch the validation has to agree with).
- `module/src/package_import.rs`, `Family` and `ClaimFamily`, and `module/src/package_import/
  items.rs` as the smallest claim-family module to model the new one on.

## The family

`DBC_FAMILY: &str = "dbc"` in `crates/lyracore-package-delta/src/schema.rs`, beside the other family
name consts. `ClaimFamily::Dbc` in `module/src/package_import.rs`, wired through `Family::ALL`,
`as_str` and the four dispatch matches. New `module/src/package_import/dbc.rs`.

The module doc comment states, once, the four things a reader needs and cannot derive:

1. Why the seventeen tables of `dbc::run` are one family and not several: one invocation, one
   unconditional clear+reload block, and no `--family` selector for the DBC stream.
2. That `--dbc --talents` is a second clear+reload unit which deliberately gains no family, because
   both its tables are excluded.
3. The Client Mirror rule, in the README's words.
4. The `game_graveyard` / `game_graveyard_zone` ordering hazard from the README. Write it now even
   though `game_graveyard` arrives in T3: the doc comment is the family's, not the table's, and T3
   should not have to remember.

## Tables

Three, all update-only, all Client Mirror Tables. Struct sources are
`module/src/faction.rs` (`FactionTemplate`, `Faction`) and `module/src/auction.rs`
(`AuctionHouseDefinition`).

**`game_faction`** — `Faction.dbc`, importer `dbc::faction_sql`

- Claim key: `{ faction_id: u32 }`. The `Faction.dbc` record id.
- Claimable columns: `reputation_index` (i32), `base_standing` (i32).
- Client note for the doc comment: the reputation pane's faction NAME comes from the client's own
  `Faction.dbc`, the standing does not. A `base_standing` claim is honoured end to end.

**`game_faction_template`** — `FactionTemplate.dbc`, importer `dbc::faction_template_sql`

- Claim key: `{ id: u32 }`.
- Claimable columns, in declaration order: `faction`, `faction_group`, `friend_group`,
  `enemy_group`, `enemy_0`, `enemy_1`, `enemy_2`, `enemy_3`, `friend_0`, `friend_1`, `friend_2`,
  `friend_3`, all u32.
- This is the hostility fabric: `module/src/faction.rs::compute_hostile`/`compute_friendly`,
  `combat/engage.rs`, `trade.rs`, `creatures/eventai/engine.rs`. Fully server-authoritative, so a
  claim here is honoured end to end and a careless one makes a zone hostile. Say both.

**`game_auction_house`** — `AuctionHouse.dbc`, importer `dbc::auction_house_sql`

- Claim key: `{ id: u32 }`.
- Claimable columns: `faction` (u32), `deposit_rate` (u32), `consignment_rate` (u32), `name`
  (string).
- Client note: the fee rates are server-authoritative and the client renders the resulting cost;
  `name` is stored but the client prints its own.

## The `--packages` validation

Today `--packages` is refused unless the run is `--spells` or has an active
`PACKAGE_DELTA_DUMP_FAMILIES` family. That refusal now has a third legal shape: the standalone DBC
catalogue pass.

Derive the predicate from the same conditions `main` already dispatches on, rather than restating
them, so the two can never disagree: the standalone pass is the run where `--dbc` is given, `--dump`
is absent, and none of `--talents`, `--spells` or `--spell-snapshot` claims the invocation. A small
named helper both sites call is the right shape.

The refusal message grows one name. Do not reword the rest of it; the existing text is asserted.

Two runs must stay refused, and both need a test:

- `--dbc --talents --packages <root>`: the talents pass has no Package Delta stage.
- `--dbc --spell-snapshot <path> --packages <root>`: already refused by the snapshot's own "run it
  on its own" guard; assert the guard still fires first.

## Provenance

Inside `dbc::run`'s `args.apply` arm, after the last `run_sql_statements` and before the reapply:

```rust
stamp_family(args, DBC_FAMILY, &args.source_sha, "", total_rows)
    .context("stamp_import_meta(dbc)")?;
```

`file_hash` is `""` for the same reason `spell.rs` leaves it empty: the source is the operator's own
MPQ chain, not a file this run hashes. `total_rows` is the sum of the per-table counts `run` already
computes for its prints, so it is a number the operator can check against the same run's output.
`--source-sha` names the cmangos dump rather than the client; `spell.rs` passes it anyway and this
matches. Put that in a one-line comment, not a paragraph.

## The replay hook

After the `if args.apply { … } else { … }` block and before the coverage prints, so it runs on both
paths:

```rust
match &args.packages {
    Some(root) => package_delta::reapply(args, DBC_FAMILY, root)?,
    None => package_delta::warn_not_reapplied(DBC_FAMILY),
}
```

Do not add `"dbc"` to `PACKAGE_DELTA_DUMP_FAMILIES`. That const names `--dump` families and is read
by `run_package_stage` and by the validation above; the spell family is not in it either, for the
same reason.

## Files owned

- `crates/lyracore-package-delta/src/schema.rs`, `delta.rs`, `error.rs`, `lib.rs`
- `crates/lyracore-package-delta/tests/families.rs`
- `crates/lyracore-package-delta/tests/dbc_identifiers.rs` (new)
- `module/src/package_import.rs`, `module/src/package_import/dbc.rs` (new)
- `importer/src/dbc.rs`, `importer/src/main.rs`
- `CONTEXT.md`

## Out of scope

- Every other DBC table. T2 and T3 own them.
- The Package DBC Range. T3 introduces it; `check_inventable` for these three tables says "never".
- The precise unsupported-table refusal. T4 owns it; an excluded name still gets the generic
  `UnknownTable` message after this ticket.
- `import-world.sh`. It threads `--packages` to nothing today and this ticket does not change that.
- `pack_client.rs`. Name the seam in the doc comment; write no code.

## Acceptance tests

1. `tests/dbc_identifiers.rs`: all three tables refuse every insert with `InsertNotSupported`, and
   the refusal names the table.
2. `tests/families.rs`: the three names parse back to themselves, report family `dbc`, and each has
   claimable columns.
3. `a_dbc_plan_claims_only_dbc_family_tables` in `module/src/package_import.rs`, matching the
   existing per-family scoping tests.
4. Setter and column-coverage tests per table in `module/src/package_import/dbc.rs`: every column in
   `columns()` has a setter, and no key column appears in `columns()`.
5. `check_references`: an `game_faction_template.faction` claim naming a `game_faction` row that
   does not exist is refused and names both tables. `game_auction_house.faction` names a
   `game_faction_template` row. `deposit_rate` and `consignment_rate` above 100 are refused, the
   same bound `dbc::auction_house_sql` already enforces on the base import.
6. Importer parse tests: `--dbc --packages <root>` parses; `--dbc --talents --packages <root>` is
   refused naming `--packages`; `--dbc --spell-snapshot <p> --packages <root>` is refused.
7. An importer test that a dry-run `--dbc --packages` prints the plan and calls no reducer, matching
   the spell family's own dry-run behaviour.
8. Crate, module lib and importer suites pass; wasm release build succeeds; clippy and rustfmt clean
   on touched files.

## Definition of done

`importer --db <shard> --dbc <Data> --packages packages/ --apply` reloads the DBC catalogues, stamps
`game_import_meta` for family `dbc`, reapplies every enabled Package's `dbc` claims, and writes one
`game_package_import` row per contributing Package. A Package can retune a faction's base standing,
flip a faction template's hostility, and change an auction house's fees, and all of it survives the
next `--dbc` reload. `CONTEXT.md` carries **Client Mirror Table**.
