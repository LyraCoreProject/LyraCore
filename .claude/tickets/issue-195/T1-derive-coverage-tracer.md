# T1 — nav: derive per-cell blocker coverage from vmap triangles (tracer)

Parent: issue #195. **Runs alone.**
Model: opus. Estimated size: ~150k tokens.

## Problem

Nav-grid model coverage today is produced only by `importer --nav`, from a separate box selection, with no link to the vmap generation gameplay rays use. The derivation logic must become pure, shared, and provably conservative before any schema or reducer exists.

## Delivery

- `crates/lyracore-shared/src/nav.rs`: pure per-cell derivation from `&[VmapTri]` producing the existing `walk`/`obs`/`base_z` blob shape, plus a pure merge of a terrain cell with a coverage cell (`walk` AND, `obs` per-column max).
- Refactor `importer/src/nav.rs` to rasterize its collision triangles through the shared function. Move, don't fork; margins verbatim (README).
- No schema, no reducer, no gameplay change.

## Acceptance criteria

- [ ] Existing importer nav tests pass unchanged.
- [ ] Deterministic fixture with a WMO wall: `find_leg` detours over merged coverage, goes straight without it, and `cast_ray` still hits on the straight segment.
- [ ] M2 doodad fixture blocks walkability but leaves the obstruction grid untouched (matches shipped LoS rule).
- [ ] Policy pinned by test: thin near-vertical wall, triangle spanning a cell boundary, geometry below step-up height, geometry above head height.
- [ ] Standing band references terrain ground only.
