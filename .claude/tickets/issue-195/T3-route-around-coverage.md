# T3 — nav: route around active vmap-derived coverage

Parent: issue #195. **After T1, T2.**
Model: opus. Estimated size: ~100k tokens.

## Problem

Route planning still ignores the stored coverage: `nav_step` returns straight paths that the exact collision gate then truncates, so creatures stop at walls instead of routing around them.

## Delivery

- Merge coverage in `module/src/nav.rs::fetcher` — the single `NavCellData` construction site — behind a new `game_config` gate defaulting off. One extra indexed lookup per crossed cell, memoized via the shared `Cache` (copy `vmap.rs::fetcher`'s active-generation lookup shape).
- Every caller above `fetcher` untouched. `LEG_MAX_EXPANSIONS` stays 4096.

## Acceptance criteria

- [ ] With the gate on and coverage active, `nav_step` routes around a covered obstacle through the chase, return, wander, flee, and pet-follow paths.
- [ ] Gate off, no active generation, or incomplete coverage → planning byte-identical to today.
- [ ] The exact vmap ray remains the final gate: a committed step into geometry still stops short by existing clearance.
- [ ] Coverage from a non-active generation never affects planning.
- [ ] `debug_find_leg` records the A* expansion outcome (waypoints, expansions, complete) for a covered obstacle before/after coverage — the input to T5's budget decision.
