# Vmap rollout

Run this checklist when a native Realm first gets exact collision geometry (vmap), or imports a new
generation. It is a maintenance operation with a human approval gate. Keep `game_config.vmap_enabled`
false while importing and checking. A successful import is data, not permission to change gameplay.

## Before you start

- The Realm has one Shard, `lyracore`. It owns terrain and nav chunks for maps 0 and 1 and has
  `game_config.hosts_instances` true (the default). The importer refuses a map the Shard does not
  own.
- Set `CLIENT_DATA` to the approved 1.12.1 client's `Data` directory, and `SPACETIME_SERVER` to the
  Realm's loopback HTTP URI. `importer/src/world_import_scope.rs` owns the canonical profiles, so do
  not copy their rectangles here.
- Use the Realm's World Import Profile. `alliance-single` covers both continent corridors and the
  `deadmines-entry-exit` Instance Vmap Slice (Map 36, cells x 511 to 513 and y 522 to 524). The slice
  supplies no terrain, Navigation Coverage or collision for the rest of Deadmines.
- Record the module and importer commit, the client-data identity, the maintenance window, the
  operator and the importer output. Never use the clear-first `import_vmap_chunks` reducers.

## Import

1. Dry run, then apply. A retry resumes the same generation and adds no duplicate chunk rows.

   ```bash
   lyracore-importer --vmap "$CLIENT_DATA" --world-profile alliance-single \
     --server "$SPACETIME_SERVER" --db lyracore
   lyracore-importer --vmap "$CLIENT_DATA" --world-profile alliance-single --apply \
     --server "$SPACETIME_SERVER" --db lyracore
   ```

   The Map 36 dry run records the Map.dbc, WDT, the two ADTs, WMO roots and WMO groups, and every
   route sample. It stops if a selected WMO group references an active doodad. Resolve that
   format support against the same archive before applying. Do not drop the doodad or substitute a
   flat floor. Any remaining static geometry finding blocks `--apply`.
2. Record each map's active generation: profile, id, source and selection identity, chunk count,
   bytes and digest. The Map 36 selection must name `deadmines-entry-exit` and the 3 by 3 collar.

   ```bash
   lyracore-importer --vmap-status --map 0 --server "$SPACETIME_SERVER" --db lyracore
   lyracore-importer --vmap-status --map 1 --server "$SPACETIME_SERVER" --db lyracore
   lyracore-importer --vmap-status --map 36 --server "$SPACETIME_SERVER" --db lyracore
   ```
3. Confirm the Gate is off. A missing config row counts as false. Record that and insert nothing.

   ```bash
   spacetime sql --server "$SPACETIME_SERVER" lyracore "SELECT vmap_enabled FROM game_config WHERE id = 0"
   ```

## Probe

`debug_vmap_ray`, `debug_floor_probe` and `debug_vmap_area_info` read active geometry and do not
need `vmap_enabled`. Do not use `debug_set_vmap_enabled true` as a shortcut. It enables the behavior
under acceptance.

- Pick a ray that crosses a WMO wall and a point on an elevated model floor above different
  terrain. Keep the coordinates and log excerpts. The ray must report a WMO hit (`los` and
  `collision`), and the floor probe must pick the model `floor_z` over terrain. Repeat on map 1.
- Probe the indoor answer that the land-mount refusal and indoor dismount read. An interior point
  must report a group id with `indoor` true, and a nearby open-world point must report none.

  ```bash
  spacetime call --server "$SPACETIME_SERVER" lyracore -- debug_vmap_ray 0 <x0> <y0> <z0> <x1> <y1> <z1>
  spacetime call --server "$SPACETIME_SERVER" lyracore -- debug_floor_probe 0 <x> <y> <probe_z>
  spacetime call --server "$SPACETIME_SERVER" lyracore -- debug_vmap_area_info 0 <x> <y> <z>
  ```

  A generation verified before `game_vmap_indoor_cell` existed has no marker rows and answers
  `indoor` false everywhere ([schema](../schema.md#game_vmap_indoor_cell-modulesrcvmaprs)). Run
  `verify_vmap_generation` on it again, or import again, before you record indoor evidence.

## Instance doors

DOOR and BUTTON models join sight and collision rays once `vmap_enabled` is on. They live in
`game_go_collider` rows per map and instance, layered over the static generation. Imports, pool
activation, debug spawns and instance creation register eligible doors. `rebuild_go_colliders`
reconciles existing rows after a publish. Collision scale and triangles come from `game_go_model`, so
a Package that changes a template's size or display needs a matching model artifact.

Before imported Deadmines acceptance, import the approved DOOR and BUTTON models, run
`rebuild_go_colliders`, then use `debug_vmap_ray_instance` with the instance id as its last argument
(`debug_vmap_ray` probes instance 0). Record the Factory Door hit while closed, the clear ray after
`debug_encounter_open_door`, and a closed sibling instance. Retain `debug_bench_collision_gate` and
`debug_bench_los` results on the imported canonical box.

## Enable

1. A human reviewer approves the generation and probe evidence and the maintenance window. Capture
   the approval, command, time and prior value of `vmap_enabled`.
2. With a disposable 1.12.1 client Character, check collision at the chosen wall and elevated
   floor, then normal movement and line of sight nearby. Record Character, map, coordinates, client
   build, expected and observed result, and the log window.
3. If any result is ambiguous, set the Gate off, keep the active generation for diagnosis and mark the
   rollout inconclusive. Never delete the prior generation as part of incident handling.
