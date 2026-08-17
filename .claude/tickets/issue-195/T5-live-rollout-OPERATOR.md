# T5 — nav: roll out vmap-derived coverage on map 0 (OPERATOR-GATED)

Parent: issue #195. **After T1–T4 merge. Not agent work — needs a live realm, a real 1.12.1 client, and human approval.** Use `$lyracore-operator`; safety boundary is `docs/danger-zones.md`.

## Checklist

- [ ] Prepare coverage for the active map-0 generation (`2010376918105624374` at spec time) on both eligible shards; manifests match; `lyracore-world-1` holds none.
- [ ] Record `debug_find_leg` and `debug_bench_collision_gate` evidence before and after coverage; decide the `LEG_MAX_EXPANSIONS` question from it (default: keep 4096).
- [ ] A named WMO wall and a named M2 obstacle each produce an observed detour, with server movement logs.
- [ ] Real 1.12.1 client observation: a chasing NPC routes around the wall, no clipping.
- [ ] Commit the rollout checklist + evidence (pattern: `docs/vmap-rollout.md`); enable the `game_config` gate only after human approval.
