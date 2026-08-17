# Issue #195 — nav: derive path-grid coverage from vmap geometry

Source: `gh issue view 195 --comments`. Spec drafted from the shipped vmap lifecycle and grilled with the maintainer 2026-08-17. Read this file, then your own ticket, before touching code.

## Decisions (maintainer-approved, do not relitigate)

- **Standing-ground reference: terrain ground only.** Exact parity with the shipped `--nav` rasterizer policy. Railings/walls on elevated model decks are out of scope.
- **Elevated model-floor heights: follow-up issue**, not here. Blockers only.
- **`LEG_MAX_EXPANSIONS` stays 4096.** Retuning is a measured decision in the operator rollout (T5), from `debug_find_leg` / `debug_bench_collision_gate` evidence.
- **Importer margins reused verbatim** (0.35 yd sliver inflation, 0.5 yd body inset, standing-band z-interval test). Consequence accepted: narrow WMO interiors rasterize closed; rollout targets outdoor map 0 first.
- **Gate: new `game_config` flag, defaults off**, separate from `vmap_enabled`. One-command rollback.
- **Coverage derived module-side from stored `game_vmap_generation_chunk` rows.** No re-import; the active map-0 generation can be retrofitted.
- **Activation is optional-coverage**: `activate_vmap_generation` succeeds without coverage and selects it only when a complete matching record exists.
- **Partial coverage never routes.** All-or-nothing per generation.

## Seams

- **Read seam (only runtime change):** `module/src/nav.rs::fetcher` — the single construction site of `NavCellData`. Merge coverage there: `walk` = bitwise AND, `obs` = per-column max, memoized via the shared `Cache` like `vmap.rs::fetcher`. Nothing above it changes.
- **Write seam:** the generation lifecycle in `module/src/vmap.rs`. New reducers sit beside `stage_/append_/verify_/activate_/discard_vmap_generation`.
- **Pure seam:** `crates/lyracore-shared/src/nav.rs` — `derive` (per-cell blobs from `&[VmapTri]`) and `merge` (terrain cell + coverage cell). The importer's rasterizer (`importer/src/nav.rs::rasterize_cell` + `WorldTri::z_at`) refactors to call the same code; it is today's only rasterization policy and moves, not forks.

## Schema conventions

- Reuse the `NavChunk` blob triple (`base_z`, 512-byte `walk`, 1024-byte `obs`) and `NavCellData`. No new codec; `walk_get`/`obs_top`/`find_leg`/`step_hit` stay byte-identical readers.
- New tables, never writes to `game_nav_chunk` (no generation column; cleared wholesale by a different pipeline): `VmapNavCoverage` (private per-cell blobs, keyed `(generation_id, cell_key)`, indexes `by_generation` / `by_generation_cell`) and `VmapNavCoverageManifest` (public, blob-free: cell count, digest, complete flag). Privacy split mirrors `VmapGenerationChunk` / `VmapGenerationReceipt`.
- New table rather than columns on `VmapGeneration`: `#[default]` cannot end-append a `Vec<u8>` (danger-zones §1.6).
- Coverage `base_z` = the cell's minimum terrain corner height when a terrain chunk exists, else min z of the cell's own triangles. Folds into the coverage digest.
- Preparation allowed for verified or active generations; refused for staging/discarded.
- Shared `cell_key` convention throughout; the importer already duplicates triangles into every cell their AABB touches, so per-cell derivation clamped to the cell is conservative by construction.
- Steep model surfaces (>60° from horizontal) inherit the existing full-z-span rule.

## Execution DAG

```
T1 (tracer, opus)  →  T2 (sonnet)  →  T3 (opus)  →  T4 (sonnet, integrates + files PR)
                                                     T5 (operator-gated, NOT agent work)
```

Serial chain, one branch `feat/issue-195-derived-coverage`, one PR at the end (no shippable intermediates). Each agent pulls the branch, works, pushes. Tests live where the logic is pure: fixture route tests in `lyracore-shared`, lifecycle tests in the module's existing in-memory harness style. No new database-backed harness. Terse comments, no issue numbers in code.
