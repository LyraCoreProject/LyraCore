# Vmap production rollout acceptance

Use this checklist only after the shard-aware vmap workflow is deployed. It records live acceptance
for exact collision on both populated World Shards and confirms that the Instance Pool receives no
open-world vmap generation.

This is a maintenance operation. Keep `game_config.vmap_enabled` false while importing and while
checking generations. A successful import is data-plane evidence, not permission to change gameplay.

## Scope and prerequisites

- Use the approved production client data. The importer resolves `alliance-eastern` and
  `alliance-kalimdor` through its canonical profile catalogue; do not copy rectangles here.
- Set `SPACETIME_SERVER` to the approved production SpacetimeDB endpoint before using any commands
  below; each command supplies it explicitly rather than relying on the CLI's ambient server.
- The World Shard destinations are the default Shard with `alliance-eastern` and the Kalimdor World
  Shard with `alliance-kalimdor`. The Instance Pool must receive no bounded-profile vmap generation.
- Record the deployed module/importer commit, the client-data identity, the approved maintenance
  window, the operator, and the exact importer output for each target.
- Run `lyracore import vmaps` with the deployed topology. It must address both World Shards, skip the
  Instance Pool, and leave the vmap gate disabled. Do not substitute the old clear-first
  `import_vmap_chunks` reducers for this workflow.

## Import and generation evidence

1. Run the matching profile dry runs and save their completion manifests. Run the apply/resume flow
   once for each World Shard. A retry must resume the same generation rather than creating duplicate
   chunk rows.
2. Query the active generation on each World Shard. Record its profile, generation id,
   source/selection identity, chunk count, byte count, and digest. Record that no bounded-profile
   active or staging generation exists on the Instance Pool.

   ```bash
   lyracore-importer --vmap-status --map 0 --server "$SPACETIME_SERVER" --db lyracore
   lyracore-importer --vmap-status --map 1 --server "$SPACETIME_SERVER" --db lyracore-kalimdor
   lyracore-importer --vmap-status --map 36 --server "$SPACETIME_SERVER" --db lyracore-instances
   ```
3. Confirm the gate remains off on every shard before probe testing:

   ```bash
   spacetime sql --server "$SPACETIME_SERVER" lyracore "SELECT vmap_enabled FROM game_config WHERE id = 0"
   spacetime sql --server "$SPACETIME_SERVER" lyracore-kalimdor "SELECT vmap_enabled FROM game_config WHERE id = 0"
   spacetime sql --server "$SPACETIME_SERVER" lyracore-instances "SELECT vmap_enabled FROM game_config WHERE id = 0"
   ```

   Each result must be `false`. If a configuration row is absent, treat it as false and record that
   fact rather than inserting one during this checklist.

## Geometry-probe readiness

Issue #184 requires ray and floor evidence before exact-vmap consumption is enabled. The current
`debug_vmap_ray` and `debug_floor_probe` reducers read the active generation directly and do not
require `vmap_enabled`; use them while the gameplay gate remains off. Do not use a temporary
`debug_set_vmap_enabled true` change as a substitute: it already enables the behavior this checklist
is meant to accept.

After that prerequisite is met, select coordinates from the imported canonical slice: at least one
ray that crosses a WMO wall and one elevated model floor whose terrain height differs. Keep exact
coordinates and server log excerpts as rollout evidence. The intended command shape is:

```bash
spacetime call --server "$SPACETIME_SERVER" lyracore -- debug_vmap_ray 0 <x0> <y0> <z0> <x1> <y1> <z1>
spacetime call --server "$SPACETIME_SERVER" lyracore -- debug_floor_probe 0 <x> <y> <probe_z>
```

The ray output must identify an expected WMO hit (`los` and `collision`), and the floor probe must
report a model `floor_z` selected over the terrain ground where those heights differ. Repeat the
same class of probes against `lyracore-kalimdor` at approved map-1 locations.

Also probe the indoor answer, which gameplay rules such as the land-mount refusal and the indoor
dismount read:

```bash
spacetime call --server "$SPACETIME_SERVER" lyracore -- debug_vmap_area_info 0 <x> <y> <z>
```

Pick one point inside a WMO interior and one in the open world nearby. The interior must report a
group id with `indoor` true; the open-world point must report no group.

**A generation verified before the indoor-presence table existed carries no marker rows.**
`game_vmap_indoor_cell` is written only inside `verify_vmap_generation`, and a missing row means
outdoors, so an older generation answers `indoor` false everywhere and every indoor rule stays
inactive on it. Run `verify_vmap_generation` on the generation again, or import the vmap data again,
before recording indoor evidence.

## Client acceptance and gate decision

1. A human reviewer must approve the completed generation/probe evidence and an approved
   maintenance window before the operator changes `vmap_enabled`. Capture the approval, command,
   time, target shard, and prior value.
2. With a disposable 1.12.1 client character, validate the intended collision behavior at the
   selected wall and elevated-floor locations. Record character, map, coordinates, client build,
   expected result, observed result, and server log window. Re-test normal movement and line of
   sight nearby so the acceptance covers both exact collision and gameplay reachability.
3. If any client or probe result is ambiguous, turn the gate back off, preserve the active
   generation for diagnosis, and mark the rollout inconclusive. Do not delete the prior generation
   as part of incident handling.

## Evidence template

```text
UTC date/time and approved maintenance window:
Operator and approval reference:
Deployed module/importer commit:
Client Data path and source identity:
Canonical profiles: alliance-eastern / alliance-kalimdor

World Shard preflight: lyracore / lyracore-kalimdor:
Instance Pool skip: lyracore-instances:
lyracore profile, active generation id, chunks, bytes, digest:
lyracore-kalimdor profile, active generation id, chunks, bytes, digest:
Instance Pool bounded-generation check:
Gate state before probes on all three shards:

WMO ray coordinates, expected hit, and log excerpt:
Elevated-floor coordinates, expected floor, and log excerpt:
Kalimdor World Shard probe evidence:
Read-only probe surface / acceptance revision reference:

Human-review gate-enable approval, command, time, and target:
1.12.1 client build/account/character:
Client collision observation:
Client elevated-floor observation:
Nearby movement and line-of-sight observation:

Verdict: PASS / FAIL / INCONCLUSIVE
Links to raw importer output and server/client evidence:
```
