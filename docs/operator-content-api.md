# Operator Content API

The Operator Content API is a set of reducers that create, update or delete one creature spawn or one
gameobject spawn on a running realm. A world editor or any other tool calls them with the Owner
Token. Each edit shows in the world at once: the Module removes the old live entity with its normal
despawn code and the respawn pass makes the new one.

The source is `module/src/operator_content.rs`. The durable tests are in
`module/tests/operator_content.rs`.

## Which database

Call the Realm's Shard, `lyracore` on a local Realm. Every map is in that one Shard.

## Who can call

Every reducer calls `require_operator` first. Only the Operator identity gets through. The first
caller after a new publish becomes the Operator through `claim_operator`, so these reducers are as
safe as that claim. Claim the Operator immediately after each publish.

## Reducers

| Reducer | Arguments | Effect |
| --- | --- | --- |
| `operator_create_creature_spawn` | `entry: u32`, `placement: CreatureSpawnPlacement` | Adds a creature spawn with a new guid from the Operator Spawn Range. |
| `operator_update_creature_spawn` | `guid: u64`, `placement: CreatureSpawnPlacement` | Moves or changes one creature spawn. The entry stays. |
| `operator_delete_creature_spawn` | `guid: u64` | Removes one creature spawn and everything that hangs from it. |
| `operator_set_creature_waypoints` | `guid: u64`, `points: Vec<WaypointPoint>` | Replaces the patrol of one creature spawn. An empty list removes the patrol. |
| `operator_create_gameobject` | `fields: GameObjectSpawnFields` | Adds a gameobject spawn with a new guid from the Operator Spawn Range. |
| `operator_update_gameobject` | `guid: u64`, `fields: GameObjectSpawnFields` | Moves or changes one gameobject spawn, including its template. |
| `operator_delete_gameobject` | `guid: u64` | Removes one gameobject spawn. |
| `operator_apply_spawn_edits` | `edits: Vec<SpawnEdit>` | Applies a list of the edits above, in order, in one transaction. |

The argument types:

```text
CreatureSpawnPlacement { map_id: u32, x: f32, y: f32, z: f32, orientation: f32,
                         movement_type: u8, respawn_secs: u32 }
WaypointPoint          { x: f32, y: f32, z: f32 }
GameObjectSpawnFields  { template_entry: u32, map_id: u32, x: f32, y: f32, z: f32,
                         orientation: f32, state: u8,
                         rotation_0: f32, rotation_1: f32, rotation_2: f32, rotation_3: f32 }
SpawnEdit = createCreature       { entry, placement }
          | updateCreature       { guid, placement }
          | deleteCreature       guid
          | setCreatureWaypoints { guid, points }
          | createGameObject     GameObjectSpawnFields
          | updateGameObject     { guid, fields }
          | deleteGameObject     guid
```

SpacetimeDB 2.7 names enum variants in camelCase in JSON. A call through the HTTP API or the CLI
looks like this:

```bash
spacetime call <db> operator_update_creature_spawn -- 17379390972424617985 \
  '{"map_id":0,"x":-8930.0,"y":-130.0,"z":82.5,"orientation":1.5,"movement_type":0,"respawn_secs":120}'

spacetime call <db> operator_apply_spawn_edits -- '[
  {"createCreature": {"entry": 620, "placement": {"map_id":0,"x":-8925.0,"y":-132.0,"z":82.0,
                                                   "orientation":0.0,"movement_type":1,"respawn_secs":0}}},
  {"deleteGameObject": 17370383762768003073}
]'
```

## Guids

A creature spawn guid is `0xF130 << 48 | entry << 24 | spawn_id`. The entry and the spawn identifier
have 24 bits each. Because the entry is part of the guid, an update cannot change it. To change the
creature at a place, delete the spawn and create a new one.

A gameobject spawn guid is `0xF110 << 48 | spawn_id`. An update can change the template.

`crates/lyracore-package-delta/src/ids.rs` holds `packed_creature_spawn_guid` and
`packed_gameobject_spawn_guid`.

## New spawn identifiers

A create takes the next free identifier in the Operator Spawn Range, 14,000,000 to 14,999,999, in
the same transaction. The tool does not choose it. No import and no Package writes a spawn in this
range, so a create cannot take a guid that an import or a Package apply writes later. A batch with
several creates gives them consecutive identifiers in order.

The next identifier is the highest one in use plus one. After the spawn with the highest identifier
is deleted, the next create takes its identifier again. A tool that keeps a deleted guid, for
example to undo, must not use it after a later create.

The reducers return no value. To find the guid of a new spawn, read the highest guid in the range
after the call. Another Operator call can commit in between, so the log line of the create is the
exact record. For creature entry 620 the range runs from `packed_creature_spawn_guid(620,
14000000)` to `packed_creature_spawn_guid(620, 14999999)`:

```sql
SELECT guid FROM game_creature_spawn
WHERE guid >= 17379390972438617984 AND guid <= 17379390972439617983
```

For gameobjects the range is `17370383762782003072` to `17370383762783003071`.

## What an edit does to the world

**Creature update.** The Module despawns the live entity with `despawn_creature_entity`. That clears
combat, threat, relay runs, gossip, loot tags, the spline and motion. It also removes the population
copies of the spawn at instance zero. Then it writes the new row and sets `respawn_at` to now, so the
next respawn pass makes the creature again from the new row in a new life (`life_seq` goes up by
one). A creature that was dead or waiting to respawn comes back at once.

**Creature delete.** The Module despawns the live entity, removes every copy of the spawn (the
population copies and the copies in running Instances), the waypoints, and the spawn row.

**Waypoints.** The Module replaces the `game_creature_waypoint` rows of the spawn and respawns the
creature like an update, so the new patrol starts from its first point.

**Creature create.** The Module writes the row with `respawn_at` set to now. The next respawn pass
makes the creature.

On a map with Instance populations (dungeons and battlegrounds), the spawn row at instance zero stays
dormant. Each new Instance copies it when the Instance is created. A running Instance keeps its old
copies until it resets, except after a delete. An Instance copies only doors, buttons, chests and
goobers among gameobjects, so another gameobject type created on such a map never shows in an
Instance.

**Gameobject update.** The Module removes the old object the way a Relay despawn does
(`despawn_from_relay`): its loot, unlock state, trap cooldown, collider and row. Then it writes the
new row, calculates `grid_x`, `grid_y` and `cell`, and registers the collider again. The guid stays.

**Gameobject delete.** The same removal, without a new row. Copies in running Instances stay until
the Instance resets.

**Gameobject create.** The Module writes the row and registers the collider.

The Module adds a `game_creature_spawn_map` row when a creature spawn goes onto a map that has none.

## Refusals

A Refusal changes nothing. In a batch, one Refusal rolls back every edit in the batch, and the
message names the index of the failed edit (`spawn edit 2: ...`).

The reducers refuse:

- A guid that is not an Authored Spawn. A creature guid must have the high part `0xF130` and a
  `game_creature_spawn` row with `instance_id = 0` and `source_guid = 0`. A gameobject guid must
  have the high part `0xF110`, no pool tag (bit 47), no Instance copy bit (bit 46), and a
  `game_gameobject` row with `instance_id = 0`. This stops a tool from
  despawning a player, a pet, a summon or an Instance creature.
- A spawn identifier in the Package Creature Range or the Package Gameobject Range. Those rows belong
  to a Package, and the next `packages apply` writes them again.
- A coordinate or rotation that is not finite, an `x` or `y` outside the terrain grid
  (`lyracore_shared::terrain::cell_index`), or a `z` beyond `MAP_COORD_MAX`.
- A map that the realm does not hold. A map is known when it has creature spawns or gameobject spawns
  at instance zero, or when a terrain or navigation chunk covers the position.
- A creature entry or gameobject template that does not exist, and a creature entry wider than 24
  bits.
- A `movement_type` other than 0 (idle), 1 (random) or 2 (waypoint). Imported spawns keep the
  source's value 2. Only 1 makes the creature wander; a patrol comes from the waypoints.
- A gameobject `state` other than 0 or 1.
- A delete of a creature spawn that is a member of a population group. The group's selection names
  its members, and the import owns the group. An update of a member is allowed.

`respawn_secs` takes any value. `u32::MAX` means the creature does not come back after it dies, the
same value an encounter wave uses.

## Limits

`operator_apply_spawn_edits` takes at most 256 edits and at most 4,096 waypoints in total. One patrol
holds at most 256 waypoints. These limits keep one call from stalling the realm tick.

A create scans the spawn table once per transaction to find the highest identifier in the range.
Send many creates as one batch.

## Log

Each edit writes one `info` line to the module log with the caller identity, the guid, and the old
and new values. A batch writes its lines only after every edit applied, so a refused batch writes
none. The log is the only record of the previous values. Use it to find and undo a wrong
edit on a live realm:

```bash
spacetime logs <db> | grep 'operator .* spawn'
```

## Durability

The edits change the live database only. They are not a Package and they are not in the import
data:

- `lyracore import` deletes every spawn and loads the import again, so it removes these edits.
- `packages apply` clears and writes only the Package bands, so it does not touch these edits, and
  these reducers refuse to edit the rows it writes.

To keep an edit across a new import, put it in a Package. Exporting live edits to a Package is
separate work.
