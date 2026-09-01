# T1: One event catalogue, no third copy

Parent: issue #320. Blocks T2. Runs in parallel with T0 and T5.
Model: sonnet. Repo: engine (`crates/lyracore-package-delta/`).

## Problem

The Module's hook catalogue is already written twice on purpose:

- `module/build.rs`, `HOOK_EVENTS`, which generates the dispatch.
- `crates/lyracore-package-delta/src/script.rs`, `HOOK_EVENT_NAMES`, because a pure crate cannot
  read the Module's build script. The Module asserts the two are identical.

The typed authoring surface T2 builds needs the same list as a TypeScript union, so a Package author
binding to a misspelled event fails at `tsc` rather than at apply. Hand-writing it would make a
third copy, in a language neither existing cross-check can read.

Emit it instead.

## Delivery

Give `lyracore-delta-check` a way to print the catalogue, and let the build write the TypeScript.

- A `--print-events` mode on the existing `lyracore-delta-check` binary
  (`crates/lyracore-package-delta/src/bin/`) that writes `HOOK_EVENT_NAMES`, one per line, in
  catalogue order, and exits. Nothing else: no file paths, no artifacts, no clock.
- Prefer this over a new binary. The CLI already invokes `lyracore-delta-check` in `packages build`
  step 6, so this adds a mode to a tool the build already has, not a second tool to pin.
- Read the repo's CLI convention first: `.claude/../CLAUDE.md` points at verbs over mode flags for
  the `lyracore` CLI. `lyracore-delta-check` is a single-purpose validator, not that CLI, so a flag
  is right here. If the implementing agent disagrees after reading, a `print-events` subcommand is
  an acceptable substitute; say which was chosen.

T2 consumes this into a git-ignored generated file. T3 wires the invocation. Neither is this
ticket's job.

## Files owned

- `crates/lyracore-package-delta/src/bin/` (the delta-check binary)
- `crates/lyracore-package-delta/src/script.rs`, only if the catalogue needs a public accessor it
  does not already have. Do not touch `HOOK_EVENT_NAMES`'s contents, `ScriptArtifact`, `Script`,
  `ScriptName`, `EventBinding`, or any refusal.

## Out of scope

- Adding, removing or reordering any event. The catalogue's contents are #317's and #318's.
- The TypeScript file itself, its location, and its shape. T2 owns those.
- Any CLI-repo change.

## Acceptance tests

- The printed list equals `HOOK_EVENT_NAMES` exactly, in order, one name per line.
- Printing takes no artifact path and touches no file.
- The existing cross-check between `HOOK_EVENT_NAMES` and the Module's `GAME_HOOK_EVENT_NAMES` still
  passes and is untouched.
- The binary's existing validation behaviour is unchanged for every existing invocation shape.
