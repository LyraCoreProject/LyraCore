# T3: `packages build` compiles Runtime Scripts

Parent: issue #320. Runs after T2. Blocks T6.
Model: opus. Repo: **lyracore-cli** (`/home/t3agent/projects/LyraCoreProject/lyracore-cli`).

Read `docs/agents/cross-repo-cli.md` in the engine repo before starting. The CLI source is the
sibling `lyracore-cli` repository, never `.lyracore/cli/`'s installed cache. Local `main` there is
stale; branch from `origin/main`.

The engine repo pins the CLI at `.lyracore-cli-rev` = `e67044d3`, which already carries
`packages build`, `packages check` and the Build Identity. **Do not bump the pin here.** T6 carries
one consolidated bump at the chain head, per the repository's stacked-CLI-PR convention.

## Problem

`packages build` runs Datascripts and emits Package Deltas. It knows nothing about Lua, `game_script`
or Script Artifacts. `packages/review.rs` says so out loud today:

> `runtime scripts    none detected (nothing in this checkout reads them yet)`

T2 gives the engine a `runtimescripts/` project that emits a Script Artifact under `bun run`.
Nothing drives it, nothing records what it was built from, and nothing catches it going stale.

## Delivery

### The build stage

Extend `src/cmd/packages/build.rs` with a Runtime Script stage, mirroring the Datascript stage's
shape exactly: the same `CommandSpec` helpers, the same `run_streaming` so `tsc` diagnostics reach
the terminal with their own file and line, the same deterministic ordering (Package folders sorted
by name, then files sorted by name), the same first-failure-stops-everything rule.

Reuse what is already there. In particular:

- The existing `bun --version` gate against `doctor::REQUIRED_BUN` covers this stage too.
- Skip the whole stage when no enabled Package has a `runtimescripts/src/<package>/` folder, the way
  the Base Snapshot gate already skips.
- Extend the step 6 `lyracore-delta-check` call to cover the new `script.json` artifacts. It already
  routes on `artifact_kind()`, so a Script Artifact and a Package Delta can go in the same
  invocation. Confirm that against the crate before assuming it.

New work specific to this stage:

1. Write the event-name union into `runtimescripts/generated/` from T1's `lyracore-delta-check`
   catalogue mode, before the typecheck. Git-ignored, regenerated every run, for the same reason
   `datascripts/generated/` is: the Rust catalogue is the authority and a committed copy would be a
   second one.
2. `bun install --frozen-lockfile` with cwd `runtimescripts/`.
3. `bun ./node_modules/typescript/bin/tsc --noEmit` with cwd `runtimescripts/`. Spell the path, do
   not use `bun x tsc`; the existing code comments say why, and the reason holds here.
4. `bun run <manifest.ts>` per Package, passing `LYRACORE_PACKAGES_ROOT`. No `LYRACORE_BASE_SNAPSHOT`
   is needed: a Runtime Script reads no base data, which is why the Base Snapshot gate does not
   apply to this stage at all.

There is no Base Snapshot dependency anywhere in this stage. That is the point worth protecting: a
checkout with no client-derived data can still build Runtime Scripts.

### Build Identity for Script Artifacts

Extend `src/cmd/packages/identity.rs`. The sidecar for a Script Artifact is `script.identity`,
beside `script.json`, matching how `spell.identity` sits beside `spell.json` and staying off the
`*.json` glob `artifact::read_enabled` sweeps.

Recorded inputs, by the same `sha256-tree-v1:` / `sha256-v1:` scheme:

| Input | Covers |
|---|---|
| source tree | `runtimescripts/src/<package>/`, holding manifest, handlers, hand-written `.lua` |
| lib tree | `runtimescripts/lib/` |
| generated tree | `runtimescripts/generated/`, the event catalogue |
| bun version | `doctor::REQUIRED_BUN`, the pin |
| tsconfig | `runtimescripts/tsconfig.json`, the TSTL configuration and Lua target |
| package json | `runtimescripts/package.json`, the TSTL and TypeScript pins |
| bun lock | `runtimescripts/bun.lock` |
| artifact | the artifact's own hash, carried verbatim |

No Base Snapshot field. Do not fold identity into the artifact itself; the existing comment explains
why, and the reason is unchanged.

Prefer generalising the existing `Identity` over copying it, if that comes out simpler than two
structs. Judge it against the code; the Datascript identity has a `snapshot_hash` this one does not,
and forcing them together for its own sake is not worth an `Option`.

### Prebuilt is not stale

`packages check` treats a missing sidecar as stale: *"it predates identity tracking"*. That reading
is wrong for a Package installed by `packages add` that ships a prebuilt artifact and carries no
sources in this checkout at all. The issue's acceptance criterion **"operators do not need Bun to
publish prebuilt Runtime Scripts"** turns that from a wart into a defect for this family.

For a Script Artifact: a Package with no `runtimescripts/src/<package>/` folder is **prebuilt**.
Report it as prebuilt, with its artifact hash, and do not fail the check. A Package that *does* have
sources and no sidecar stays stale.

The same wart exists for Package Deltas and a prebuilt data-only Package. It is not fixed here: it
changes behaviour for an existing family that the maintainer should weigh separately. Say so in one
line on the PR.

### Report them

`packages review.rs` prints `runtime scripts    none detected`. Make it real: report each Package's
Runtime Scripts by name, event and whether the source was compiled or hand-written.

## Files owned

Everything in `lyracore-cli`:
- `src/cmd/packages/build.rs`, `identity.rs`, `check.rs`, `review.rs`, `mod.rs`
- `src/cmd/preflight.rs`, only if check 5's wording needs to name Script Artifacts too
- the CLI's `README.md` / `USAGE_ALL` help text in `src/cmd/mod.rs`

## Out of scope

- **The engine repo entirely.** No file there is touched by this ticket, including
  `.lyracore-cli-rev`. T6 bumps the pin once.
- `runtimescripts/**` content. T2 owns it; this only drives it.
- `packages replay`, `publish`, `client sync`. The script family's apply shipped with #318 and needs
  no CLI change.
- The Package Delta prebuilt-versus-stale wart.

## Acceptance tests

- `packages build` on a checkout with a Runtime Script Package emits `script.json` and
  `script.identity`, and `lyracore-delta-check` validates the artifact.
- The stage is skipped entirely when no enabled Package has a `runtimescripts/src/<package>/` folder.
- The stage runs with no Base Snapshot present.
- The build never publishes and never touches a database. Extend the existing
  `the_build_never_publishes_and_never_touches_a_database` test to cover the new stage.
- Two builds from the same checkout produce byte-identical `script.json` and byte-identical
  `script.identity`.
- Moving the checkout to another path produces the same sidecar. Extend
  `moving_the_checkout_produces_the_same_sidecar`.
- Touching each recorded input in turn makes `packages check` name that input and only that input.
- A Package with a `script.json` and no source folder is reported prebuilt, and `packages check`
  succeeds. `preflight` passes on that checkout with no Bun installed.
- A Package with sources and no sidecar is still reported stale.
- A `tsc` error stops the build before any artifact is written, with the file and line reaching the
  terminal.
