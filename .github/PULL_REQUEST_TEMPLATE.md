<!--
Delete any section that does not apply. The two a reviewer actually needs are "What you verified"
and "What you did NOT verify" — an honest gap is cheaper than a discovered one.
Traps that will break a realm: docs/danger-zones.md.
-->

## What and why

<!-- What changed, and the problem it solves. Link the issue: `Closes #N`, or `Refs #N` if this
     only moves it forward. -->

Closes #

## What you verified

<!-- Paste real output, not intentions. Each `cargo test` prints several `test result:` lines (unit
     target, integration target, doctests); read them all. docs/testing.md explains each tier. -->

The commands CI runs, in `.github/workflows/rust.yml`:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test -p lyracore-importer`
- [ ] `cargo test -p lyracore-module --lib --features=debug_reducers`
- [ ] `cargo test -p lyracore-gateway`
- [ ] `cargo test -p lyracore-shared`
- [ ] `cargo test -p lyracore-test-support`
- [ ] `cargo check --target wasm32-unknown-unknown -p lyracore-module --features=debug_reducers`

In `.github/workflows/module-durable.yml`, when the change reaches the Module or the schema:

- [ ] `cargo test -p lyracore-module --test <target> -- --ignored` for each durable target the change touches (CI runs them all)
- [ ] `scripts/check-gateway-bindings.py`

In `.github/workflows/runtime-scripts.yml`, when the change reaches `datascripts/`:

- [ ] `bun test tests/runtime-scripts.test.ts` in `datascripts/`

Other checks:

- [ ] Module or schema changes: a plain `spacetime publish` against your local stack migrates cleanly (**never** `publish -c`)
- [ ] `shellcheck --severity=warning` on any shell script you changed
- [ ] Markdown links you added resolve

Higher rungs, if the change reaches them — no rung substitutes for another
(*unit-test green is not suite green is not played green is not measured green*):

- [ ] Server state checked with `spacetime sql` — the transaction really did it
- [ ] Wire-level behavior verified against your local realm with a 1.12.1 client
- [ ] Watched on a real build-5875 client

## What you did NOT verify

<!-- Say it plainly. "Not run on macOS", "no live stack, so the wire path is untested",
     "needs a human eyeball on the client". The last one means this PR does not close
     the issue. -->

## Risk

<!-- Blast radius: which tier (module / gateway / importer / CLI / docs), and what breaks if this is
     wrong. -->

- [ ] **Schema change** — a new column is END-appended with `#[default(...)]` (which cannot default
      a `String`); a new table has regenerated gateway bindings. Every shard needs republishing, and
      a partial publish fails quietly. See `docs/danger-zones.md` §1.2.
- [ ] **Protocol change** — the 5875 wire format is fixed; note what a real client does with this.
- [ ] Touches `packages/` — remember a Tier-1 package is trusted module code, reviewed like a patch
      to `module/`.
- [ ] Documentation only.

## Notes for the reviewer

<!-- Where to look first, anything you are unsure about, and anything you deliberately left out of
     scope. If the change was substantially machine-generated, say so here — it tells the reviewer
     where to look. -->
