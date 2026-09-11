# Vmap production rollout acceptance

Use this checklist only after the shard-aware vmap workflow is deployed. It records live acceptance
for exact collision on both populated World Shards and for the bounded Deadmines entry and exit
route on the Instance Pool.

This is a maintenance operation. Keep `game_config.vmap_enabled` false while importing and while
checking generations. A successful import is data-plane evidence, not permission to change gameplay.

## Scope and prerequisites

- Use the approved production client data. The importer resolves `alliance-eastern` and
  `alliance-kalimdor` through its canonical profile catalogue; do not copy rectangles here. Set
  `CLIENT_DATA` to its `Data` directory for the direct Instance Pool commands below.
- Set `SPACETIME_SERVER` to the approved production SpacetimeDB endpoint before using any commands
  below; each command supplies it explicitly rather than relying on the CLI's ambient server.
- The production destinations are `lyracore` with `alliance-eastern`, `lyracore-world-1` with
  `alliance-kalimdor`, and the `lyracore-instances` Instance Pool with `instances`.
  `lyracore-kalimdor` is the contributor-fixture name, not the production Kalimdor World Shard.
- The `instances` profile owns only the `deadmines-entry-exit` Instance Vmap Slice. Its selection
  must be Map 36 cells x 511 through 513 and y 522 through 524. It supplies no terrain, no
  Navigation Coverage and no collision for the rest of Deadmines. Its collision geometry comes
  from the two ADT tiles crossed by that cell collar.
- Record the deployed module/importer commit, the client-data identity, the approved maintenance
  window, the operator, and the exact importer output for each target.
- Run the deployed importer's matching profile apply/resume flow against each named production
  World Shard, then run the `instances` profile directly against the Instance Pool. The contributor
  CLI's fixed fixture plan is not a production topology selector. Leave the vmap Gate disabled. Do
  not substitute the old
  clear-first `import_vmap_chunks` reducers for this workflow.

## Import and generation evidence

1. Run the matching profile dry runs and save their completion manifests. Run the apply/resume flow
   once for each World Shard. A retry must resume the same generation rather than creating duplicate
   chunk rows. For the Instance Pool, first retain the dry-run source identities, calibration and
   every route sample. Apply the exact same source and profile only when the static geometry is
   suitable: both authored endpoints match their floors, every sample has walkable support and
   headroom, every sampled height change is within step height, and the short and direct collision
   probes are clear. This is preparation for a private ordinary-Core route check. It does not model
   the caller-specific committed movement legs.

   ```bash
   lyracore-importer --vmap "$CLIENT_DATA" --world-profile instances \
     --server "$SPACETIME_SERVER" --db lyracore-instances
   lyracore-importer --vmap "$CLIENT_DATA" --world-profile instances --apply \
     --server "$SPACETIME_SERVER" --db lyracore-instances
   ```

   The dry run must record the exact Map.dbc, WDT, two selected ADTs, WMO roots and WMO groups. It
   stops if a selected WMO group references an active doodad. Resolve that format support against
   the same client archive before applying. Do not omit the doodad or substitute a flat floor. If
   any static geometry finding remains, stop. The importer enforces the same Gate before staging.
   After staging, retain a private ordinary-Core route result before the attended client check.
   Map 36 terrain and Navigation Coverage remain unavailable until a representation can preserve
   the route's multiple floors.
2. Query the active generation on each World Shard and the exact Map 36 generation on the Instance
   Pool. Record each profile, generation id, source and selection identity, chunk count, byte count,
   and digest. The Map 36 selection identity must name `deadmines-entry-exit` and the exact 3 by 3
   cell collar.

   ```bash
   lyracore-importer --vmap-status --map 0 --server "$SPACETIME_SERVER" --db lyracore
   lyracore-importer --vmap-status --map 1 --server "$SPACETIME_SERVER" --db lyracore-world-1
   lyracore-importer --vmap-status --map 36 --server "$SPACETIME_SERVER" --db lyracore-instances
   ```
3. Confirm the gate remains off on every shard before probe testing:

   ```bash
   spacetime sql --server "$SPACETIME_SERVER" lyracore "SELECT vmap_enabled FROM game_config WHERE id = 0"
   spacetime sql --server "$SPACETIME_SERVER" lyracore-world-1 "SELECT vmap_enabled FROM game_config WHERE id = 0"
   spacetime sql --server "$SPACETIME_SERVER" lyracore-instances "SELECT vmap_enabled FROM game_config WHERE id = 0"
   ```

   Each result must be `false`. If a configuration row is absent, treat it as false and record that
   fact rather than inserting one during this checklist.

## Geometry-probe readiness

Issue #184 requires ray and floor evidence before exact-vmap consumption is enabled. The current
`debug_vmap_ray` and `debug_floor_probe` reducers read active static geometry and registered doors.
They do not require `vmap_enabled`; use them while the gameplay Gate remains off. Do not use a temporary
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
same class of probes against `lyracore-world-1` at approved map-1 locations.

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

World Shard preflight: lyracore / lyracore-world-1:
Instance Pool profile and ownership Gate: lyracore-instances / instances:
lyracore profile, active generation id, chunks, bytes, digest:
lyracore-world-1 profile, active generation id, chunks, bytes, digest:
Instance Pool Map 36 generation id, selection, chunks, bytes, digest:
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

## Instance doors

DOOR and BUTTON models also participate in sight and collision rays when `vmap_enabled` is on.
They use `game_go_collider` rows indexed by map and instance. The bounded Map 36 static generation
supplies the entry and exit route's WMO floor and walls. Doors remain instance-specific rows layered
over that geometry. Static geometry retains the active-generation requirement; the navigation grid
still supplies the fallback on maps without one. Area and indoor queries stay static.

GameObject imports, model imports, pool activation, debug spawns and instance creation register eligible doors.
Gameobjects Package Delta apply and replay reconcile after all spawn and template changes finish.
Model imports rebuild the registry so either import order works. `rebuild_go_colliders` is an
Operator reconciliation for existing rows after a publish. Open doors keep their registration;
rays check their current state. Despawn and instance reap remove registration.

Collision scale and triangles come from `game_go_model`. A Package that changes a template's
size or display requires an explicitly matching model artifact. This path does not regenerate
meshes from Package Claims; a routine model import does not establish that those Claims match.

Before imported Deadmines acceptance, import approved DOOR/BUTTON models on the named development
Shard and run `rebuild_go_colliders`. Use `debug_vmap_ray_instance` with the instance id as its
final argument. The original `debug_vmap_ray` command still probes instance 0. Record the Factory
Door hit while closed, the clear ray after `debug_encounter_open_door`, and a closed sibling instance.
These probes also calibrate the
imported model axes and spawn quaternion. Synthetic tests do not establish imported-client parity.

Retain `debug_bench_collision_gate` and `debug_bench_los` evidence on the imported canonical box
before enabling gameplay consumption. Those performance and real-client checks remain Operator
acceptance; the private durable fixture proves geometry and lifecycle behavior.
