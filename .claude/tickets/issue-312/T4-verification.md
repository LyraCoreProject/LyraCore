# T4: verification and PR prep

Parent: issue #312. **Depends on T3. Last slice. Run alone.**
Model: sonnet.

**Rebase onto the #311 chain head and then onto latest `main`** before starting. See the README.

## Goal

Prove the two spatial families and the routing work on a real Shard, close out issue #312's
acceptance criteria one by one, and prepare the PR text. No new family, no new table, no new mode.

## Wire-level test

`module/tests/package_delta_references.rs` covers the quest and loot families against a live
standalone Shard. Extend it, or add a sibling file in the same shape, for `creatures` and
`gameobjects`:

- One refused claim per family, naming a reference the Shard does not hold, asserting the refusal
  text names the offending column.
- One accepted claim per family, followed by a `SELECT` proving the durable row holds the claimed
  value.
- **One spawn that becomes a creature.** Insert a Package creature template and a Package spawn of
  it, then prove a `game_world_entity` row appears for the derived guid after the respawn pass. This
  is the behaviour the whole family exists for, and it is the one thing a native test cannot reach:
  a spawn row that never materializes fails silently everywhere else.
- **One disabled-Package replay.** Apply a plan with two Packages, then reapply with one, and prove
  the removed Package's invented spawn and template are gone while the other Package's rows and every
  base row survive. That is the issue's fifth acceptance criterion, and the Package range clear is
  the only thing that makes it true.

Both files carry `#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]`. Run them
explicitly with `--ignored`; report whether the toolchain was available, and say so plainly if it was
not.

Reuse `module/tests/support` and the seeded scenario fixtures (`debug_seed_scenario_fixtures`). If a
family needs a row to claim against, add the minimum to the existing fixture seeder rather than
inventing a new seeding path.

## Conflict coverage

One Claim Conflict case per new family, in
`crates/lyracore-package-delta/tests/conflict_tracer.rs` alongside the existing spell cases: two
Packages claiming the same column of one row, refused with both Package identities named. Include the
spatial-specific one: two Packages stating a different `map_id` for the same spawn. That case is why
`map_id` is a claimed column rather than a key member, and the test is what proves the tracer reports
it as an ordinary conflict.

## Acceptance criteria walkthrough

Work through the issue's own list and record where each is proven. Do not tick a box without a test
or a code site to point at.

1. **Every supported spatial table maps to one base Import Family.** `Table::family` is total and
   `tests/families.rs` walks it. Name the tables that are deliberately absent and why: the README's
   named gaps.
2. **Template and spawn claims reach only the Shards that own them.** T1's and T2's scope-filter
   tests, per profile.
3. **Forced creature dependencies and whole-map ownership follow the base import rules.** T1's
   forced-entry test and the map-36 `instances` test, plus the fact that the filter calls the base
   import's own predicate rather than a copy of it.
4. **Cross-Shard preflight catches invalid map ownership and references before writes.** T3's tests.
   Name what it does not check without a dump, so the gap is stated rather than implied.
5. **Base-family replay removes a disabled Package's spatial claims and retains the others.** The
   wire test above.

## Full verification run

- `cargo test --workspace --no-fail-fast`. One pre-existing failure is expected and unrelated:
  `writer_trace_dump_writes_a_file_with_the_traced_frames` (gateway, a `/tmp` ownership problem in
  this sandbox). Confirm it fails identically on `main` before reporting it.
- `cargo test -p lyracore-package-delta`
- `cargo test -p lyracore-module --lib`: including `partition_discipline_tripwire`, which T1 and T2
  each added a whitelist entry to.
- `cargo test -p lyracore-importer --bin lyracore-importer`
- `cargo build --release --target wasm32-unknown-unknown -p lyracore-module`
- `cargo clippy` on every file touched across T1 to T3, zero new warnings. The delta crate is clean
  under `clippy::pedantic`; keep it that way.
- `cargo fmt --check` on every touched file.
- `cargo test -p lyracore-module --test package_delta_references -- --ignored` if the SpacetimeDB CLI
  and Wasm toolchain are present.

If `/tmp` runs out of space, set `TMPDIR` to a scratch directory inside the worktree.

## Follow-up issues to draft

Four. The first two are already drafted by #311's T5; if that chain filed them, add the spatial
families to their text rather than filing duplicates.

1. **Generalize `packages replay` past the spell family.** Now with the spatial hazard the README
   records: a realm-wide replay of the `creatures` family must not delete a standing wave add whose
   guid low landed inside the Package Creature Spawn Range. Note the cross-repo pin
   (`docs/agents/cross-repo-cli.md`, `.lyracore-cli-rev`).
2. **Extend the authoring library past `.spell(id)`, and the Base Snapshots with it.** A spatial
   claim needs the Base Snapshot for the `creatures` and `gameobjects` families to be authorable at
   all, and those snapshots would be dump-derived rather than DBC-derived, which the current
   `--spell-snapshot` mode is not.
3. **Package pools.** `game_gameobject_pool` and `game_gameobject_pool_member` exist but no dump ETL
   writes them, and creature pools have no durable representation at all. Giving either a Package
   surface means giving it a base import first. State both halves.
4. **`game_creature_waypoint`.** A Package spawn cannot be given a movement path in v1. The blocker
   is the surrogate key the importer assigns in dump order plus the missing map on the row; both are
   fixable, neither is fixable inside this issue.

Write all of them with the `unslop` skill applied. Do not open them until a maintainer asks.

## PR

Use the `file-pr` skill. Rebase onto latest `main` first. Suggested title:

    feat(package-delta): add the creatures and gameobjects Import Families with World Import Scope routing

The description opens with the problem in plain terms (a Package cannot reach creatures, game
objects or their spawns, and those rows do not live on every Shard), then the two families and their
tables, then the per-table v1 policy as a table, then the three identifier ranges and why two of
their ceilings come from guid layout bits rather than the decade convention, then the routing in
three sentences, then the named gaps. Simplified Technical English, `CONTEXT.md` vocabulary, no em
dashes. Close with the model and harness blurb.

State the stack order: this chain sits on #311's, which sits on `main`.

## Files owned

`module/tests/package_delta_references.rs` (or its sibling),
`crates/lyracore-package-delta/tests/conflict_tracer.rs`, `module/src/package_import/fixtures.rs` and
the scenario fixture seeder if a family needs a row to claim against, `CHANGELOG.md`.

## Out of scope

- Any change to a family's schema, range or policy. If verification finds one wrong, report it and
  stop; do not fix it in this slice.
- Filing the follow-up issues. Draft them, do not open them.

## Definition of done

Every issue #312 acceptance criterion has a named proof or a named gap, a Package spawn is proven to
become a live creature at the wire, a disabled Package's spatial rows are proven to leave on replay,
the full suite is green except the one documented pre-existing gateway failure, the wasm release
build succeeds, clippy and rustfmt are clean, and the PR text is ready.
