# T2 — vmap: store nav coverage as a generation-scoped record

Parent: issue #195. **After T1.**
Model: sonnet. Estimated size: ~120k tokens.

## Problem

Derived coverage must belong to exactly one vmap generation and must never half-exist. There is nowhere to put it: `game_nav_chunk` has no generation column and is cleared wholesale by a different import pipeline.

## Delivery

- `module/src/vmap.rs`: `VmapNavCoverage` + `VmapNavCoverageManifest` tables (schema conventions in README), and batched, resumable preparation reducers beside the existing lifecycle. Preparation decodes one cell's chunks at a time (memory bounded by batch, not map) and calls the T1 pure derivation.
- `activate_vmap_generation` gains one atomic selection step (optional-coverage semantics, README); `discard_vmap_generation` cleans coverage rows and manifest.

## Acceptance criteria

- [ ] Prepare in batches for a verified or active generation, finalize, and read a complete manifest with cell count and digest.
- [ ] Re-running an already-prepared batch is idempotent; manifest digest unchanged.
- [ ] Preparation refused for staging/discarded generations; discard removes coverage.
- [ ] Activation selects coverage only when complete and belonging to that exact generation; failed/partial preparation leaves prior active coverage in effect.
- [ ] In-memory lifecycle tests: prepare, retry, finalize, activate, discard, prior-coverage preservation — mirroring the existing `module/src/vmap.rs` harness.
