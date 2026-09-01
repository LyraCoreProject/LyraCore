# Issue #320, Runtime Scripts: compile TypeScript authoring output to Lua

Compile a Package author's TypeScript into the Lua the Runtime Script Host already runs, ship it in
the Script Artifact #318 already applies, and let hand-written Lua in through the same door.

The Lua-side contract is **stable and closed to this work**. `module/src/runtime_script.rs` (piccolo
0.3.3, the Fuel Budget, the allowlisted surface, the Host Operations, Entity Handles) and
`crates/lyracore-package-delta/src/script.rs` (the Script Artifact) are what this compiles TO. The
one exception is T0, which is a correctness defect in the Host, not a contract change.

## What the spike left, and what measurement added

`spikes/tstl-runtime/` proved TSTL 1.37.1 output runs on piccolo 0.3.3 under 64-fuel slices with a
`table.concat` shim. Measuring the interpreter again for this spec found the recorded defect is
narrower than the truth, and found two more limits. All three are load-bearing; see T0 and T5.

## Slices

| | Ticket | Repo | Model | Runs after |
|---|---|---|---|---|
| T0 | Restore Lua argument semantics in the Host | engine | opus | nothing (blocks T2) |
| T1 | One event catalogue, no third copy | engine | sonnet | nothing |
| T2 | The Runtime Script authoring project | engine | opus | T0, T1 |
| T3 | `packages build` compiles Runtime Scripts | lyracore-cli | opus | T2 |
| T4 | Fuel tests over generated Lua | engine | sonnet | T2 |
| T5 | Script Diagnostics locate the generated Lua | engine | sonnet | nothing |
| T6 | Docs, worked example, CLI pin bump | engine | sonnet | T3, T4, T5 |

T0 and T1 run in parallel. T4 and T5 run in parallel with T3. T6 is the chain head and carries the
single `.lyracore-cli-rev` bump for the whole T3 CLI chain.

## Two decisions the maintainer owns

Both are stated in full at the top of their tickets, recommendation first.

1. **T0**: how to answer the piccolo argument defect. Recommendation: fix it once in the Host.
2. **T2**: where the authoring project lives. Recommendation: a sibling `runtimescripts/` project,
   not a second tree inside `datascripts/`.

## What "CI" means here

This repository has **no GitHub Actions workflows** (`gh api .../actions/workflows` returns
`total_count: 0`). The gate the issue calls CI is `lyracore preflight` plus `cargo test`.
"CI typechecks TypeScript" is `packages build` step 3; "CI rejects stale generated Lua" is
`packages check`, which preflight check 5 folds in. No workflow file is written by this work.
