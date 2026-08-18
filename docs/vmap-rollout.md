# Vmap production rollout acceptance

Use this checklist only after the shard-aware vmap workflow is deployed. It records the live
acceptance for [#184](https://github.com/LyraCoreProject/LyraCore/issues/184): map 0 has a complete,
matching active generation on both eligible shards, the ineligible shard stays empty, and exact
collision is accepted deliberately.

This is a maintenance operation. Keep `game_config.vmap_enabled` false while importing and while
checking generations. A successful import is data-plane evidence, not permission to change gameplay.

## Scope and prerequisites

- Use the production client data at `/srv/wowclient/Data` and the canonical map-0 box
  `-11400,-8000,-3100,2000`.
- Set `SPACETIME_SERVER` to the approved production SpacetimeDB endpoint before using any commands
  below; each command supplies it explicitly rather than relying on the CLI's ambient server.
- The current production ownership for this rollout is `lyracore` and `lyracore-instances` for map
  0. `lyracore-world-1` owns map 1 and must receive no map-0 vmap generation.
- Record the deployed module/importer commit, the client-data identity, the approved maintenance
  window, the operator, and the exact importer output for each target.
- Run the importer workflow introduced by #183. Its preflight must accept the two eligible targets,
  reject `lyracore-world-1` for map 0, and leave the vmap gate disabled. Do not substitute the old
  clear-first `import_vmap_chunks` reducers for this workflow.

## Import and generation evidence

1. Run the canonical map-0 dry run and save its completion manifest. Run the matching apply/resume
   workflow once for `lyracore` and once for `lyracore-instances`; save both complete outputs.
   A retry must resume the same generation rather than creating duplicate chunk rows.
2. Query the active map-0 generation on both eligible shards using the workflow's status command.
   Record its generation id, source/selection identity, chunk count, byte count, and digest. The two
   manifests must match exactly. Record that no map-0 active or staging generation exists on
   `lyracore-world-1`.

   ```bash
   lyracore-importer --vmap-status --map 0 --server "$SPACETIME_SERVER" --db lyracore
   lyracore-importer --vmap-status --map 0 --server "$SPACETIME_SERVER" --db lyracore-instances
   lyracore-importer --vmap-status --map 0 --server "$SPACETIME_SERVER" --db lyracore-world-1
   ```
3. Confirm the gate remains off on every shard before probe testing:

   ```bash
   spacetime sql --server "$SPACETIME_SERVER" lyracore "SELECT vmap_enabled FROM game_config WHERE id = 0"
   spacetime sql --server "$SPACETIME_SERVER" lyracore-instances "SELECT vmap_enabled FROM game_config WHERE id = 0"
   spacetime sql --server "$SPACETIME_SERVER" lyracore-world-1 "SELECT vmap_enabled FROM game_config WHERE id = 0"
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
same probes directly against `lyracore-instances`' active map-0 generation.

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
Canonical map/box: 0 / -11400,-8000,-3100,2000

Eligible-shard preflight: lyracore / lyracore-instances:
Ineligible-shard rejection: lyracore-world-1:
lyracore active generation id, chunks, bytes, digest:
lyracore-instances active generation id, chunks, bytes, digest:
Manifest comparison: MATCH / MISMATCH
world-1 map-0 generation check:
Gate state before probes on all three shards:

WMO ray coordinates, expected hit, and log excerpt:
Elevated-floor coordinates, expected floor, and log excerpt:
Instance-shard probe evidence (if applicable):
Read-only probe surface / acceptance revision reference:

Human-review gate-enable approval, command, time, and target:
1.12.1 client build/account/character:
Client collision observation:
Client elevated-floor observation:
Nearby movement and line-of-sight observation:

Verdict: PASS / FAIL / INCONCLUSIVE
Links to raw importer output and server/client evidence:
```
