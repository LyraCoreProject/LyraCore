# T5: verification and PR prep

Parent: issue #311. **Depends on T4. Last slice. Run alone.**
Model: sonnet.

## Goal

Prove the five families work together on a real Shard, close out issue #311's acceptance criteria
one by one, and prepare the PR text. No new family and no new table.

## Wire-level cross-reference test

`module/tests/package_delta_references.rs` covers quests and loot against a live standalone Shard.
Extend it, or add a sibling file in the same shape, so each of the five new families is exercised
end to end at the wire:

- One refused claim per family, naming a reference the Shard does not hold, asserting the refusal
  text names the offending column.
- One accepted claim per family, followed by a `SELECT` proving the durable row holds the claimed
  value.

Both tests carry `#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]`. Run them
explicitly with `--ignored`; report whether the toolchain was available in the environment, and say
so plainly if it was not.

Reuse `module/tests/support` and the seeded scenario fixtures
(`debug_seed_scenario_fixtures`). If a family has no fixture row to claim against, add the minimum
needed to the existing fixture seeder rather than inventing a new seeding path.

## Conflict coverage

Issue #311 asks for "a conflict in each supported Import Family group". Add one Claim Conflict case
per new family: two Packages claiming the same column of one row, refused with both Package
identities named. The pure tracer is the right rung for these, so put them in
`crates/lyracore-package-delta/tests/conflict_tracer.rs` alongside the existing spell cases, not at
the wire.

## Acceptance criteria walkthrough

Work through the issue's own list and record where each is proven. Do not tick a box without a test
or a code site to point at.

1. Every supported table maps to exactly one Import Family. `Table::family` is total and
   `tests/families.rs` walks it.
2. Table-specific primary-key and reserved-row rules are enforced. The per-family identifier tests,
   plus the packed-key derivation tests.
3. Reloading one Import Family reapplies only its Package claims. `check_claims_belong_to`, and one
   scoping test per family.
4. Non-spatial catalogue claims reach every Shard that owns that catalogue. Point at how the
   importer fans the stage out; if nothing proves it beyond the spell path, say so and name it as
   the gap the follow-up replay issue covers.
5. Unsupported tables and operations fail during preflight. The unknown-table refusal, and the
   `InsertNotSupported` refusal T1 added.
6. Tests cover cross-table references and a conflict in each supported Import Family group. The two
   sections above.

## Full verification run

- `cargo test --workspace --no-fail-fast`. One pre-existing failure is expected and unrelated:
  `writer_trace_dump_writes_a_file_with_the_traced_frames` (gateway, a `/tmp` ownership problem in
  this sandbox). Confirm it fails identically on `main` before reporting it.
- `cargo test -p lyracore-package-delta`
- `cargo test -p lyracore-module --lib`
- `cargo test -p lyracore-importer --bin lyracore-importer`
- `cargo build --release --target wasm32-unknown-unknown -p lyracore-module`
- `cargo clippy` on every file touched across T1 to T4, zero new warnings. The delta crate is clean
  under `clippy::pedantic`; keep it that way.
- `cargo fmt --check` on every touched file.
- `cargo test -p lyracore-module --test package_delta_references -- --ignored` if the SpacetimeDB
  CLI and Wasm toolchain are present.

If `/tmp` runs out of space, set `TMPDIR` to a scratch directory inside the worktree.

## Follow-up issues to file

Two, both named in the README as out of scope:

1. **Generalize `packages replay` past the spell family.** The verb in the sibling `lyracore-cli`
   repository (`src/cmd/packages/replay.rs`) is hardcoded to `spell`. Ten families now carry Package
   artifacts and only one can be replayed realm-wide from the CLI. Note the cross-repo pin
   (`docs/agents/cross-repo-cli.md`, `.lyracore-cli-rev`).
2. **Extend the authoring library past `.spell(id)`.** `datascripts/lib/authoring.ts` exposes one
   family. A Datascript author cannot reach items, quests, loot or any of the five added here
   without hand-writing artifact JSON. Include the "insert at a real identifier" question in this
   issue or a third one: several tables here are update-only purely because the band model has no
   reconciliation story for a row inserted at a real client identifier.

Write both with the `unslop` skill applied. Do not open them until a maintainer asks.

## PR

Use the `file-pr` skill. Rebase onto latest `main` first. Suggested title, following the repo's
conventional-commit form:

    feat(package-delta): add the gossip, trainers, casts, globals and spellmeta Import Families

The description opens with the problem in plain terms (five dump-backed catalogues a Package cannot
reach), then what each family covers, then the per-table v1 policy as a table, then the identifier
bands and why they continue the decade convention, then the named gaps. Simplified Technical English,
`CONTEXT.md` vocabulary, no em dashes. Close with the model and harness blurb.

If T1 to T4 landed as separate PRs, this one is the integration head: state the stack order and
which PR carries which family.

## Files owned

`module/tests/package_delta_references.rs` (or its sibling),
`crates/lyracore-package-delta/tests/conflict_tracer.rs`, `module/src/package_import/fixtures.rs`
and the scenario fixture seeder if a family needs a row to claim against, `CHANGELOG.md`.

## Out of scope

- Any change to a family's schema, band or policy. If verification finds one wrong, report it and
  stop; do not fix it in this slice.
- Filing the follow-up issues. Draft them, do not open them.

## Definition of done

Every issue #311 acceptance criterion has a named proof or a named gap, the full suite is green
except the one documented pre-existing gateway failure, the wasm release build succeeds, clippy and
rustfmt are clean on every file the plan touched, and the PR text is ready.
