# Dispatch boilerplate

Lines to carry in every implementer dispatch prompt. The orchestrator pastes what applies; agents treat them as binding.

## Worktree and branch

- Shared feature branches may be checked out in another agent's worktree. Work on a local tracking branch: `git checkout -b <local> origin/<branch>`, push with `git push origin <local>:<branch>`. Parallel slices push their own branch (`<epic>-tN`) and leave merging to the integrate step.
- On ENOSPC from `/tmp`, set `TMPDIR` to a scratch dir inside your own worktree.

## Verification

- Before excepting any failing test, confirm it fails identically on the unmodified base commit; report it as pre-existing with that evidence.
- Format touched files individually (`rustfmt <file>` and inspect the diff). Repo-wide formatting rewrites ~75 files of pre-existing drift.
- Zero new clippy warnings in touched files; wasm check for module changes.

## Scope discipline

- Stay inside the file ownership named in your brief; report gaps that belong to another slice instead of fixing them.
- No PR, no GitHub comments, no merge — unless your ticket assigns it.
- Return: what you built, test numbers, and handoff notes for the next slice.
