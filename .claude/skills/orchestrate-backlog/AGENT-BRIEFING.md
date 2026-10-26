# Dispatch boilerplate

Lines to carry in every implementer dispatch prompt. The orchestrator pastes what applies; agents treat them as binding.

## Worktree and branch

- Shared feature branches may be checked out in another agent's worktree. Work on a local tracking branch: `git checkout -b <local> origin/<branch>`, push with `git push origin <local>:<branch>`. Parallel slices push their own branch (`<epic>-tN`) and leave merging to the integrate step.
- Keep cargo target dirs, worktrees, clones and other large scratch output out of `/tmp` and `/dev/shm`. A throwaway, baseline or review build uses `CARGO_TARGET_DIR="$HOME/.cache/lyracore-scratch/<task>/target"` and puts its checkout under `$HOME/.cache/lyracore-scratch/<task>/`; delete it when done. Your own worktree keeps the default `./target`.

## Verification

- Read `AGENTS.md` in your checkout. Complete the
  [comment review](../../../CODING_STANDARDS.md#comment-review) and include its scope and result in
  your handoff.
- Before excepting any failing test, confirm it fails identically on the unmodified base commit; report it as pre-existing with that evidence.
- Format the files you change and run the formatting check in `docs/testing.md`.
- Zero new clippy warnings in touched files; wasm check for module changes.

## Scope discipline

- Stay inside the file ownership named in your brief; report gaps that belong to another slice instead of fixing them.
- No PR, no GitHub comments, no merge — unless your ticket assigns it.
- Return: what you built, test numbers, and handoff notes for the next slice.
