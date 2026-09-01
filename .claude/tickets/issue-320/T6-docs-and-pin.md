# T6: Docs, worked example, and the CLI pin bump

Parent: issue #320. The chain head. Runs after T3, T4 and T5.
Model: sonnet. Repo: engine.

## Problem

After T2 through T5 there is a Runtime Script authoring path nobody has written down, a CLI that can
drive it but a pin that predates it, and a glossary that has no word for a compiled Runtime Script.

## Delivery

### The pin

Bump `.lyracore-cli-rev` to the merged head of T3's CLI chain. **This is the only pin bump in issue
#320.** T3 deliberately does not touch it, per the repository's stacked-CLI-PR convention: one
consolidated bump at the chain head.

Verify before committing the bump: `./lyracore packages build` on this checkout drives the whole
Runtime Script stage end to end, and `./lyracore packages check` and `./lyracore preflight` both
pass. A pin bump that has not been run is a guess.

### `docs/development-cli.md`

The Datascript sections are the template; match their voice and depth, do not restate them.

- Extend `## packages build` with the Runtime Script stage, in the numbered-step style already
  there, saying which steps are shared with the Datascript stage and which are not.
- Add a `### The Runtime Script project` block mirroring `### The Datascript project`: the tree, the
  pins, and which directories are committed.
- **State why the TypeScript pins differ.** `datascripts/` pins TypeScript 7.0.2; `runtimescripts/`
  pins 6.0.2 because `typescript-to-lua@1.37.1` peer-depends on it exactly. Without that sentence
  the next agent will "fix" the inconsistency and break the build.
- Extend `### Bun is author-side only`. The claim still holds and now covers more: an Operator
  applying a prebuilt Package with Runtime Scripts needs no Bun and no Node either.
- Extend `## packages check` with the prebuilt-versus-stale rule T3 adds for Script Artifacts, and
  say plainly that it is narrower than the Package Delta family's rule and why.

### `packages/README.md`

Its data-half paragraph names `data/.generated/*.json` as "the Package Deltas a Datascript
generates". A Package folder can now also hold a Script Artifact. Say so, name the worked example,
and point at `runtimescripts/src/<package>/` with the same reason already given for Datascripts:
only artifacts belong inside a Package folder.

### `CONTEXT.md`

`AGENTS.md` requires the glossary to move with the terms. The Runtime Scripts section has **Runtime
Script**, **Runtime Script Host**, **Invocation**, **Fuel Budget**, **Entity Handle**, **Host
Operation**, **Staged Effect** and **Script Diagnostic**. Missing after this work:

- **Script Artifact**: shipped by #318 and still absent from the glossary. Add it.
- **Event Binding**: likewise.
- A term for a compiled Runtime Script source, if T2's implementation needed one. Add it only if the
  code uses it; do not invent a word nothing says.

Give each an `_Avoid_` line. Existing `_Avoid_` lists rule out `sandbox`, `VM`, `plugin`, `gas` and
`addon`; keep the new entries consistent with them.

### The note the issue text needs

The issue says sources live at `scripts/*.ts` inside a Package. They do not: they live at
`runtimescripts/src/<package>/`, following the rule `packages/README.md` already states. Note the
departure and the reason on the PR, so the difference reads as a decision rather than a miss.

### `CHANGELOG.md`

One entry, in the file's existing style.

## Files owned

- `.lyracore-cli-rev`
- `docs/development-cli.md`
- `packages/README.md`
- `CONTEXT.md`
- `CHANGELOG.md`
- `packages/<example>/README.md`, only if T2 left it thin

## Out of scope

- Any code. If documenting reveals a defect, name it in one line on the PR and leave it, unless it
  is in a file this ticket already changes.
- The CLI repo. T3's chain is merged by the time this runs.
- `spikes/tstl-runtime/`. It is a closed spike and stays as its record.

## Acceptance tests

- A reader following `docs/development-cli.md` alone can author, build and check a Runtime Script.
- `./lyracore packages build`, `./lyracore packages check` and `./lyracore preflight` all pass on
  this checkout at the new pin, and the commands in the docs are the commands that were run.
- Every term the new prose uses is in `CONTEXT.md`, and no `_Avoid_` word appears in it.
- `cargo test` passes.
