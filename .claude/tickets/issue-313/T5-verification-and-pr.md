# T5 — verification and PR prep

Model: sonnet

Depends on: T4. Rebase onto T4, then onto the chain head (#312 then #311 then `main`), before
opening anything.

## Goal

Prove the five acceptance criteria of issue #313 on a real Shard, then open the PR. Write no new
document: the commands and their output go on the PR body.

## Acceptance criteria, and what proves each

| Criterion | Proof |
|---|---|
| Each supported table has one documented base-import owner and Shard routing rule | `module/src/package_import/dbc.rs`'s doc comment names `importer --dbc <Data> --apply` as the owner and "every Shard, no scope filter" as the rule, once for the family. Twelve tables, one family, one statement. |
| Reloading its DBC source reapplies enabled Package claims | The live run below: claim a row, reload, read it back. |
| Identifiers and references are checked before writes | The refusal tests from T1, T2, T3, plus one live run of a bad artifact that refuses without writing. |
| Unsupported DBC tables fail with a precise message | T4's tests, plus one live refusal in the run below. |
| No Package commits or distributes Blizzard-format DBC files | The firewall check below. |

## The live run

One Shard, one Package, one pass. Use a scratch database, never a production realm; `AGENTS.md`
requires naming what is touched before touching it, and this touches only the named scratch shard.

Author a scratch Package with one artifact per shape, then:

```bash
# 1. base import with the Package Delta stage, dry run first (prints the plan, writes nothing)
./target/debug/lyracore-importer --db <scratch> --dbc <client Data> --packages packages/

# 2. apply
./target/debug/lyracore-importer --db <scratch> --dbc <client Data> --packages packages/ --apply

# 3. read the claimed rows and the provenance back
spacetime sql <scratch> "SELECT * FROM game_auction_house WHERE id = 1"
spacetime sql <scratch> "SELECT * FROM game_graveyard WHERE id >= 17000000"
spacetime sql <scratch> "SELECT * FROM game_import_meta WHERE family = 'dbc'"
spacetime sql <scratch> "SELECT * FROM game_package_import"

# 4. reload the base catalogues WITHOUT --packages, then WITH, to show the replay is what restores
#    the claims rather than the base import leaving them alone
./target/debug/lyracore-importer --db <scratch> --dbc <client Data> --apply
spacetime sql <scratch> "SELECT * FROM game_graveyard WHERE id >= 17000000"   # empty, and warned
./target/debug/lyracore-importer --db <scratch> --dbc <client Data> --packages packages/ --apply
spacetime sql <scratch> "SELECT * FROM game_graveyard WHERE id >= 17000000"   # back
```

Step 4 is the one that matters. The warning `package_delta::warn_not_reapplied` prints on the
`--packages`-less run is the evidence that the operator is told, not silently left with base rows.

The artifact set to author, one per interesting shape:

1. An `game_auction_house` fee update. A Client Mirror Table update, honoured end to end.
2. A `game_graveyard` insert at 17,000,001 plus, if #311 has landed, a `globals`
   `game_graveyard_zone` pointing at it. If the `globals` half refuses, that IS the ordering hazard
   T3 documented; capture the refusal on the PR rather than working around it.
3. A `game_skill_line` insert plus a `game_skill_ability` insert naming it, in one artifact. Proves
   the same-plan reference.
4. A `game_taxi_path` fare update.
5. A refused artifact: an insert on `game_faction_template`. Must fail with `InsertNotSupported`
   naming the table, and must write nothing.
6. A refused artifact: a claim on `game_area`. Must fail with T4's `DbcTableNotSupported` and quote
   the reason.

## The firewall check

The issue's last criterion is a negative, so check it as one:

```bash
git ls-files packages/ client-patch/ | grep -iE '\.(dbc|mpq)$'   # must print nothing
```

and confirm by reading that nothing this chain added writes a `.dbc`: the whole change reads the
operator's MPQ chain through the existing `dbc::open_chain` / `read_table` path, which is read-only,
and emits typed row artifacts. `client-patch/.gitignore` already blocks `*.dbc` and `*.MPQ` and
`pack_client.rs`'s firewall test already refuses a committed one. Nothing here weakens either; say
so on the PR and point at the existing test rather than adding a new one.

## Full suite

```bash
cargo test -p lyracore-package-delta
cargo test -p lyracore-module --lib
cargo test -p lyracore-importer
cargo build -p lyracore-module --release --target wasm32-unknown-unknown
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## The PR

Use the `file-pr` skill. Title: `feat(packages): Package Deltas for the DBC catalogues`.

The body, in ASD-STE100 Simplified Technical English, using `CONTEXT.md` terms:

- The problem: the DBC catalogues had no Package Delta stage, so a base reload removed every
  Package change to them.
- The solution: one Import Family, `dbc`, for the standalone DBC catalogue pass; a replay hook and
  a provenance stamp in `importer/src/dbc.rs`; twelve claimable tables; one Package Identifier
  Range; and a refusal that says why the other seven tables are closed.
- The Client Mirror rule, in two sentences. It is the design decision a reviewer most needs.
- The client divergence, named and not solved, the same way the Package Spell is. One sentence, and
  a pointer to the Package Spell Range doc comment.
- The known ordering constraint between the `globals` graveyard link and the `dbc` graveyard.
- The follow-ups this chain does not do: `packages replay` for a second non-dump family, a Base
  Snapshot for the `dbc` family (which matters more here than for #311 or #312, because a claim key
  is a DBC record id and an author has no repo-local place to read those from), and the authoring
  library helper.
- The model and harness blurb at the end.

File follow-up issues for the three items above before opening the PR, and link them.

## Files owned

None in the source tree. This ticket runs, reports and files.

## Out of scope

- A `docs/*-verification.md`. This is a probe-style acceptance run; the commands belong on the PR.
- Any code change that the run reveals as desirable but not required by the acceptance criteria.
  File it.
