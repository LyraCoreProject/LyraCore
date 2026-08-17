# T4 — vmap: drive nav-coverage preparation from the operator workflow

Parent: issue #195. **After T3 (serial chain). Integrates the branch and files the PR.**
Model: sonnet. Estimated size: ~120k tokens.

## Problem

The T2 reducers need a driver: enumerate a generation's cells, prepare them in bounded batches, resume after interruption, report status — and never on a shard that does not own the map.

## Delivery

- Importer/CLI-side workflow (beside the existing vmap import workflow in `importer/src/vmap.rs`): prepare coverage for a named generation, resumable, with manifest reporting. Reuse `preflight_map_ownership` fail-closed.
- No gameplay gate is touched by the workflow.
- **Integration duties:** rebase `feat/issue-195-derived-coverage` onto latest `origin/main`, run the full importer + module + shared test suites, reconcile anything the chain orphaned, then file ONE PR per `.claude/skills/file-pr/SKILL.md`. Title like `feat(nav): derive path-grid coverage from vmap geometry`. Description notes T5 (live rollout) stays open as operator work. Do not merge; do not close the issue.

## Acceptance criteria

- [ ] Workflow prepares coverage for a named generation on an eligible shard and reports the manifest.
- [ ] An interrupted run resumes without re-preparing accepted cells; same final digest.
- [ ] Ownership preflight rejects an ineligible target before any coverage work.
- [ ] Fixture larger than one batch: bounded per-call payloads, deterministic coverage identity.
- [ ] Full test suites green; PR filed and linked to #195.
