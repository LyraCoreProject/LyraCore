# Operator Content API

The Operator Content API edits creature and gameobject spawns, item and creature templates,
vendor offers, creature displays, loot, spells and Character positions on a running Realm.
A world editor such as Meld calls it with the Owner Token. Spawn edits despawn the old live entity and let the respawn pass
make the new one. Item edits refresh connected wearers and send new item query answers to clients.

The source is `module/src/operator_content.rs` and `module/src/operator_content/items.rs`. The
durable tests are in `module/tests/operator_content.rs` and `module/tests/operator_items.rs`.
`module/src/operator_content/creatures.rs` and `module/tests/operator_creatures.rs` cover creature edits.
`native-host/operator_creatures.py` checks their packets and respawns.
`native-host/operator_items.py` checks item edits with the Headless Client.
`module/src/operator_content/characters.rs`, `module/tests/operator_characters.rs` and
`native-host/operator_characters.py` cover Character teleport and its packets.

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
| `operator_apply_spawn_edits` | `edits: Vec<SpawnEdit>` | Applies a list of spawn edits, in order, in one transaction. |
| `operator_apply_item_edits` | `edits: Vec<ItemEdit>` | Creates or updates item templates and replaces vendor offers, in order, in one transaction. |

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

## Character teleport

```text
operator_teleport_character(name: String, map_id: u32, x: f32, y: f32, z: f32, o: f32)
    -> Result<(), String>
```

The Operator calls this with the Owner Token. Meld's Game › Play Here and Launch-at-camera
can use the same call. Play Here moves the Character in the world. Launch must close the game
and wait for logout before the call, then start the game after success. The Meld caller ships
in a separate PR.

Names match without ASCII case distinctions, so `Tester`, `tester` and `TESTER` match the same
Character. Whitespace is significant; non-ASCII letters must match exactly. The Module's
unique name index is case-sensitive, so older or directly created rows can differ only in case.
More than one matching Character is a Refusal, even when one spelling matches exactly.
Characters in Transfer are hidden by the existing Character read Gate.

A Character with a `game_world_entity` uses `world::teleport_player`. Same-map moves send
`MSG_MOVE_TELEPORT_ACK`; map changes send `SMSG_TRANSFER_PENDING` and `SMSG_NEW_WORLD` and
wait for `MSG_MOVE_WORLDPORT_ACK` before rebuilding a connected Character's entity.
A live Character without a World Session moves immediately. The normal teleport path
updates the saved position, drops queued motion and resolves the destination zone. Its usual
mount, trade, duel and Instance cleanup still applies. A pending native teleport refuses a
second call, including the far-teleport interval with no live entity.

Without a live entity, the reducer writes `map_id`, `x`, `y`, `z`, `orientation` and `zone_id`
to `game_character`, and resets `pending_instance_id` to zero. The zone comes from the terrain
chunk's area and its parent zone. If area data is unavailable, the offline write sets zone zero
instead of keeping a zone from the previous position. It emits no teleport packet. The next
login reads this saved position. An `online` Character without an entity is refused, unless an
already-pending teleport has supplied the more specific Refusal first.

The destination must have a `game_map` row with `map_type = 0`, and must not be one of the
Module's known Instance maps. Dungeons, raids and battlegrounds are out of scope. The Realm
must also have instance-zero spawn content for the map or terrain or navigation at the point,
using the same loaded-map check as spawn edits. A catalogue row alone does not admit a map.
All four floats must be finite. Both horizontal coordinates must lie inside the terrain grid,
and the absolute height must not exceed `MAP_COORD_MAX`, about 17,066.67 yards.

The reducer preserves `z` exactly. It does not call `ground_z` or `snap_z`: terrain height can
lie above a cave floor or below a bridge, and collision data may differ from Meld's edited
client files. Meld must supply its ground point, with a small upward offset if needed. The call
does not check walkability, collision or height above ground. Orientation is in radians and
is stored without normalization.

This authoring operation refuses a dead Character, a ghost, combat, a taxi flight or a Transport
Passenger. It does not resurrect, end combat or cancel travel. Ghost status is read from the
live entity, or `pending_ghost` when offline. Offline health zero means an uninitialized vitals
snapshot, not death. Logout completes in one transaction in the current Native Host path;
there is no pending-logout timer to cancel. The call either runs before logout under the online
rules or after logout under the offline rules.

### Refusals

A Refusal returns `Err` and changes no rows. These are the messages; braces denote substituted
values. `{name:?}` is Rust debug string formatting, including quotes and escaping.

| Message | Cause |
| --- | --- |
| `operator only` | The caller is not the claimed Operator. |
| `operator not claimed` | No Operator has been claimed. |
| `the position and orientation must be finite` | At least one float is NaN or infinite. |
| `({x}, {y}, {z}) is outside the map bounds` | Coordinates are outside the grid or height bound. |
| `map {map_id} is not in the Realm's map catalogue` | No map row exists. |
| `map {map_id} requires an Instance` | The map type is nonzero or the Module identifies an Instance map. |
| `map {map_id} is not loaded at ({x}, {y})` | No loaded-map evidence exists. |
| `character {name:?} does not exist or is in Transfer` | No visible name matches. |
| `character name {name:?} is ambiguous ignoring ASCII case` | Multiple visible names match. |
| `character is already teleporting` | A native teleport awaits acknowledgement. |
| `character is on a taxi flight` | A taxi flight row exists. |
| `character is aboard a transport` | A Transport Passenger row exists. |
| `character is a ghost` | The live ghost flag or offline `pending_ghost` is set. |
| `character is dead` | The live entity is dead. |
| `character is in combat` | Combat is active or its grace period has not expired. |
| `character is online without a world entity; retry after logout` | An online Character has no entity. |

One `info` line after success records the caller, Character guid, escaped requested name,
online or offline path, and destination. A Refusal emits no successful teleport log.

### Call example

Use the CLI identity for the Realm's Owner Token and select the intended Realm server:

```bash
spacetime call --server <server-url> <db> operator_teleport_character -- \
  '"Tester"' 0 -8949.95 -132.493 83.5312 0.0
```

The positional JSON argument array, also used by the HTTP reducer endpoint, is:

```json
["Tester",0,-8949.95,-132.493,83.5312,0.0]
```

The CLI takes each array element as a separate argument. It does not take the whole array as
one argument. Saved Character positions survive content imports.

## Item arguments

```text
operator_apply_item_edits(edits: Vec<ItemEdit>) -> Result<(), String>
ItemEdit = createItem      { fields: ItemFields }
         | updateItem      { entry: u32, fields: ItemFields }
         | setVendorOffers { creature_entry: u32, offers: Vec<VendorOffer> }
VendorOffer { item_entry: u32, max_count: u32 }
ItemFields {
    class: u8,
    subclass: u8,
    name: String,
    display_id: u32,
    quality: u8,
    inventory_type: u8,
    item_level: u8,
    required_level: u8,
    max_durability: u32,
    buy_price: u32,
    sell_price: u32,
    max_stack: u32,
    damage_min: f32,
    damage_max: f32,
    delay_ms: u32,
    stat_strength: i32,
    stat_agility: i32,
    stat_stamina: i32,
    stat_intellect: i32,
    stat_spirit: i32,
    stat_crit: i32,
    stat_hit: i32,
    stat_armor: i32,
    block_value: i32,
    restores_power: bool,
    spellid_1: u32,
    spelltrigger_1: u8,
    spellid_2: u32,
    spelltrigger_2: u8,
    container_slots: u8,
    sheath: u8,
    bonding: u8,
    holy_res: i32,
    fire_res: i32,
    nature_res: i32,
    frost_res: i32,
    shadow_res: i32,
    arcane_res: i32,
    spellid_3: u32,
    spelltrigger_3: u8,
    spellid_4: u32,
    spelltrigger_4: u8,
    spellid_5: u32,
    spelltrigger_5: u8,
    required_skill: u32,
    required_skill_rank: u32,
    required_reputation_faction: u32,
    required_reputation_rank: u32,
    max_count: u32,
    item_flags: u32,
    page_text: u32,
    start_quest: u32,
    bag_family: u32,
    buy_count: u32,
    food_type: u8,
    allowed_class: u32,
    allowed_race: u32,
    random_property: u32,
}
```

Every `ItemFields` member is required. An update replaces every column except `entry`; it is not a
partial update. Start with the current `game_item_template` row, remove `entry`, and change the
fields you want. The exhaustive constructor in the Module keeps this type in step with the table.
There is no item delete. Instances, loot, quests and vendors can refer to an item.

This is a complete create edit. Pass an array of edits as the reducer's sole argument. The HTTP
call body wraps that argument once more, as `[[edit, ...]]`.

```json
{
  "createItem": {
    "fields": {
      "class": 2,
      "subclass": 7,
      "name": "Meld blade",
      "display_id": 0,
      "quality": 1,
      "inventory_type": 21,
      "item_level": 0,
      "required_level": 0,
      "max_durability": 0,
      "buy_price": 0,
      "sell_price": 0,
      "max_stack": 1,
      "damage_min": 2.0,
      "damage_max": 4.0,
      "delay_ms": 2000,
      "stat_strength": 0,
      "stat_agility": 0,
      "stat_stamina": 0,
      "stat_intellect": 0,
      "stat_spirit": 0,
      "stat_crit": 0,
      "stat_hit": 0,
      "stat_armor": 0,
      "block_value": 0,
      "restores_power": false,
      "spellid_1": 0,
      "spelltrigger_1": 0,
      "spellid_2": 0,
      "spelltrigger_2": 0,
      "container_slots": 0,
      "sheath": 0,
      "bonding": 0,
      "holy_res": 0,
      "fire_res": 0,
      "nature_res": 0,
      "frost_res": 0,
      "shadow_res": 0,
      "arcane_res": 0,
      "spellid_3": 0,
      "spelltrigger_3": 0,
      "spellid_4": 0,
      "spelltrigger_4": 0,
      "spellid_5": 0,
      "spelltrigger_5": 0,
      "required_skill": 0,
      "required_skill_rank": 0,
      "required_reputation_faction": 0,
      "required_reputation_rank": 0,
      "max_count": 0,
      "item_flags": 0,
      "page_text": 0,
      "start_quest": 0,
      "bag_family": 0,
      "buy_count": 1,
      "food_type": 0,
      "allowed_class": 1503,
      "allowed_race": 255,
      "random_property": 0
    }
  }
}
```

The other variants have these shapes, with the same complete fields object for an update:

```text
{"updateItem": {"entry": 5200500, "fields": ItemFields}}
{"setVendorOffers": {"creature_entry": 620, "offers": [
  {"item_entry": 5200500, "max_count": 0},
  {"item_entry": 25, "max_count": 3}
]}}
```

An empty `offers` list clears the vendor. `slot` is the zero-based list index. Vendor row IDs are
explicit `max + 1` values, allocated before deleting the old rows. Imported explicit IDs do not
advance SpacetimeDB's `auto_inc` sequence. `max_count = 0` means unlimited stock; a positive count
keeps the current vendor stock behavior. This API does not add stock depletion or restock timers.

## New template entries

A create allocates from the Operator Template Range, **5,200,500 to 5,999,999**. It takes the highest
item entry in that range plus one, or the floor when no item uses the range. One batch scans the
non-spatial item table at most once for this high-water mark. Vendor allocation scans the
non-spatial vendor table at most once. These reads do not change the spatial scan whitelist.

The range starts after the curated range, which ends at 5,200,499. Trainer overrides use
5,200,000 through 5,200,499. Fixture-Reserved items use 5,090,000 through 5,099,999, and the next
reserved band is the Package Spell Range at 6,000,000. The local vmangos snapshot at source revision `4641790`
had 20,034 item rows, a highest entry of 25,818, and no entries in this range. The ceiling also fits
the 24-bit creature-entry limit, 16,777,215. Creature, gameobject and quest template allocation can
use this range later; this reducer allocates only item entries.

Imports must not write template entries in this range. The items importer refuses such a source
row before it emits SQL. An items import still removes Operator-created items when it clears the
item table. After that import, a later create can reuse their entries.

The reducer returns no entry. Read new rows in the range after the call, or use the create log line
to distinguish concurrent Operator calls. Later edits in the same batch can refer to an entry that
earlier edits created, but an editor must not predict the allocated entry across concurrent calls.

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

**Item create or update.** Gameplay reads the new template on its next use. After all edits apply,
the Module finds connected Characters through `game_native_player` and their items through
`by_owner_guid`. For each Character wearing an updated item, it calls `recompute_vitals` before
`recompute_sheet`, once for the batch. It does not scan all item instances. Offline Characters
recompute at login. Existing instances keep their durability, soulbound flag and Random Property;
new grants use the current template. Edits do not move equipped items or recheck their eligibility.

For each distinct created or updated entry, the Module queues a fresh
`SMSG_ITEM_QUERY_SINGLE_RESPONSE` to each admitted World Session, including Character selection.
It sends the final row once even if the batch edits that item more than once. The Headless Client
check confirms arrival without a preceding `CMSG_ITEM_QUERY_SINGLE`. **Whether an unmodified 1.12
client replaces its cache after this unsolicited response is not verified.** This push does not
prove that an open tooltip or vendor window redraws.

**Vendor offers.** List and purchase requests read `game_npc_vendor` fresh, so no cache refresh is
needed. An open list changes when the client requests it again. Interaction uses the live creature
entity's `npc_flags`. Adding offers to a non-vendor does not give it the VENDOR bit. Editing creature
templates and refreshing those flags is outside this slice.

## Refusals

A Refusal changes nothing. In a batch, one Refusal rolls back every edit in the batch, and the
message names the zero-based index of the failed edit, such as `spawn edit 2: ...` or
`item edit 2: ...`. Item stat refreshes and queued packets roll back too.

Spawn reducers refuse:

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

## Spawn limits

`operator_apply_spawn_edits` takes at most 256 edits and at most 4,096 waypoints in total. One patrol
holds at most 256 waypoints. These limits keep one call from stalling the realm tick.

A create scans the spawn table once per transaction to find the highest identifier in the range.
Send many creates as one batch.

## Spawn log

Each spawn edit writes one `info` line to the Module log with the caller identity, the guid, and the old
and new values. A batch writes its lines only after every edit applied, so a refused batch writes
none. The log is the only record of the previous values. Use it to find and undo a wrong
edit on a live realm:

```bash
spacetime logs <db> | grep 'operator .* spawn'
```

## Spawn durability

The edits change the live database only. They are not a Package and they are not in the import
data:

- `lyracore import` deletes every spawn and loads the import again, so it removes these edits.
- `packages apply` clears and writes only the Package bands, so it does not touch these edits, and
  these reducers refuse to edit the rows it writes.

To keep an edit across a new import, put it in a Package. Exporting live edits to a Package is
separate work.

## Item Refusals and limits

Item edits refuse a missing update target, a Package Item Range entry, or a Fixture-Reserved item.
Vendor replacement refuses a missing creature template, a missing item, or duplicate item entries.
Vendor rows are not Package-claimable; offers may refer to Package or Fixture-Reserved items.
A missing nonzero spell in any of the five spell slots is a Refusal. Range exhaustion is a Refusal.

An update checks only the values it changes. A value the row already holds passes, for example an
imported spell id the Realm does not have, so an edit of one column is not refused for another. A
create checks every value.

The item Gates use the same vanilla enum types as the item-query encoder:

| Field | Accepted values |
| --- | --- |
| `class`, `subclass` | A pair in `ItemClassAndSubClass`, classes 0 through 15 with that class's subclasses. |
| `quality` | 0 through 6. |
| `inventory_type` | 0 through 28. |
| `bonding` | 0 through 5. |
| `sheath` | 0 through 7. |
| `spelltrigger_1` through `spelltrigger_5` | 0 on use, 1 on equip, 2 chance on hit. |
| `food_type` | 0 through 8. |
| `required_reputation_rank` | 0 through 7. |
| `max_stack`, `buy_count` | At least 1. |
| `damage_min`, `damage_max` | Finite, with `0 <= damage_min <= damage_max`. |
| `name` | Nonblank, no NUL, at most 255 UTF-8 bytes. |

The accepted class/subclass pairs are:

| Class | Subclass |
| --- | --- |
| 0 | 0 through 2 |
| 1, 6 | 0 through 4 |
| 2 | 0 through 20 |
| 3, 5, 8, 10, 12, 14, 15 | 0 |
| 4, 9 | 0 through 9 |
| 7, 11 | 0 through 3 |
| 13 | 0 through 1 |

Other fields retain their table types and current gameplay semantics. This API does not promise
support for every client feature that an item field describes.

A call accepts at most **64 item edits**, **128 offers per vendor edit**, and **512 offers across
the batch**. The name length bounds text and log output. The edit and offer caps bound catalogue
writes and the number of item responses sent per connection. Stat refresh cost depends on the
number of connected Characters and their owned items; packet cost depends on connected clients.
Split larger saves into separate transactions. Each transaction is atomic; the whole save is not.

The Module writes one `info` line per item edit only after the entire batch, stat refreshes and
packet construction succeed. Each line includes the caller identity and item or creature entry.
Item updates list changed columns as old -> new values; creates list all fields with an absent old
value. Vendor edits log the old and new ordered offer lists. String values escape line breaks.

## Item edit durability

These rows survive a restart and a plain Module publish. `lyracore import` with the `items` Import
Family clears and reloads `game_item_template` and `game_npc_vendor`, so it wipes these edits,
including Operator-created items. Save authored content outside the Realm before that import.
Item instances can remain after their template disappears. This API adds no import reconciliation
and no item deletion or reference repair.

## Creature templates

`operator_apply_creature_edits(edits: Vec<CreatureEdit>) -> Result<(), String>` edits NPC
creature templates and their display, loot and spell lists. It calls `require_operator` first.
Edits apply in order in one transaction. A Refusal rolls back the entire batch, including rows,
despawns and queued packets. Edit Refusals start with `creature edit N:`, where N is zero-based.
An authority Refusal has no edit index.

| Variant | Payload | Effect |
| --- | --- | --- |
| `createCreature` | `fields: CreatureFields` | Allocates a creature entry in the Operator Template Range. |
| `updateCreature` | `entry: u32, fields: CreatureFields` | Replaces every template column except entry. |
| `setCreatureDisplays` | `entry: u32, displays: Vec<CreatureDisplay>` | Replaces displays and copies the first display's id and scale to the template. |
| `setCreatureLoot` | `creature_entry: u32, rows: Vec<LootRow>` | Replaces death loot for the entry. |
| `setPickpocketLoot` | `creature_entry: u32, rows: Vec<LootRow>` | Replaces pickpocket loot for the entry. |
| `setSkinningLoot` | `skin_loot_id: u32, rows: Vec<SkinRow>` | Replaces the shared skinning list. |
| `setCreatureSpells` | `creature_entry: u32, spells: Vec<CreatureSpell>` | Replaces the AI spell list. |
| `setCreatureCast` | `creature_entry: u32, spell_id: Option<u32>` | Sets the fallback cast; `{"some":133}` sets a spell, `null` removes it. |

The JSON variant names use camelCase; payload fields use snake_case. Send the array as the
reducer's sole argument. All fields are required, including zero values. The examples below are
complete JSON payloads. Use a faction template, items and spells present in the target Realm.

### Types and examples

`CreatureFields` contains every `game_creature_template` column except `entry`.

| Type | Fields |
| --- | --- |
| `String` | `name`, `subname` |
| `f32` | `scale` |
| `u8` | `creature_type`, `creature_family`, `rank`, `trainer_type`, `trainer_class` |
| `u32` | `display_id`, `level`, `health`, `faction_template`, `npc_flags`, `unit_flags`, `type_flags`, `base_attack_time_ms`, `money_min`, `money_max`, `max_level`, `max_level_health`, `aggro_range`, `damage_min`, `damage_max`, `armor`, `pickpocket_loot_id`, `skin_loot_id` |

Create a template:

```json
[{"createCreature":{"fields":{"name":"Meld scout","subname":"Watchkeeper","display_id":100,"level":10,"health":250,"faction_template":35,"npc_flags":1,"unit_flags":0,"creature_type":7,"creature_family":0,"type_flags":0,"rank":0,"scale":1.0,"base_attack_time_ms":2000,"money_min":0,"money_max":10,"max_level":10,"max_level_health":250,"aggro_range":0,"damage_min":5,"damage_max":10,"armor":50,"pickpocket_loot_id":0,"skin_loot_id":0,"trainer_type":0,"trainer_class":0}}}]
```

The first empty-range allocation is 5,200,500. Later allocations use the highest existing creature
entry in 5,200,500 through 5,999,999 plus one. Item entries have their own high-water mark. Read
`game_creature_template` to get the created entry; reducers do not return it.

Update that template:

```json
[{"updateCreature":{"entry":5200500,"fields":{"name":"Meld captain","subname":"Watchkeeper","display_id":200,"level":10,"health":300,"faction_template":35,"npc_flags":129,"unit_flags":0,"creature_type":7,"creature_family":0,"type_flags":0,"rank":0,"scale":1.0,"base_attack_time_ms":2000,"money_min":0,"money_max":10,"max_level":10,"max_level_health":300,"aggro_range":0,"damage_min":5,"damage_max":10,"armor":50,"pickpocket_loot_id":0,"skin_loot_id":0,"trainer_type":0,"trainer_class":0}}}]
```

The published input types are `OperatorCreatureDisplay` and `OperatorCreatureSpell`, distinct
from the table row types. In the reducer source they are `CreatureDisplay` and `CreatureSpell`.

`CreatureDisplay` has `display_id: u32`, `scale: f32`, `weight: u32`, `gender: u8`, and
`bounding_radius: f32`. Weights are relative selection weights. Zero disables a row; a nonempty
list needs total weight from 1 through 4,294,967,295 to fit the selector's 32-bit roll. The first row may have zero weight. Its display and
scale still become the template's fallback, because the catalogue uses the list only when its
first row agrees with those template fields. An empty list removes the alternatives and keeps
the current template fallback. Later template display or scale edits can disable the list until
it is set again. No client display catalogue is required in the Realm; display ids must be
nonzero, but the Operator must choose models that the client has. Gender and bounding radius
lookups use the display id across entries, so keep metadata for the same model consistent.

```json
[{"setCreatureDisplays":{"entry":5200500,"displays":[{"display_id":100,"scale":1.0,"weight":1,"gender":0,"bounding_radius":0.389},{"display_id":200,"scale":1.5,"weight":3,"gender":1,"bounding_radius":0.5}]}}]
```

`LootRow` has `item_entry: u32`, `chance_bp: u32`, `count: u32`, `group_id: u32`, and
`quest_only: bool`. `SkinRow` has the same fields except `quest_only`.
For `group_id = 0`, each row gets an independent roll in 0 through 9,999; 10,000 always drops
and zero never drops. A nonzero group gets one roll over cumulative weights on that same
10,000-point scale. At most one row wins. Group weights are not normalized: a total of 7,500
leaves a 25% chance of no drop. A total above 10,000 is refused so later rows remain reachable.
Counts must be positive. Empty lists remove all rows.

```json
[{"setCreatureLoot":{"creature_entry":5200500,"rows":[{"item_entry":25,"chance_bp":7500,"count":1,"group_id":1,"quest_only":false}]}}]
```

Pickpocket lists are keyed by creature entry. `pickpocket_loot_id` records the import source's
identifier; the importer resolves it before storing rows. Editing that template field does not
select a different pickpocket list.

```json
[{"setPickpocketLoot":{"creature_entry":5200500,"rows":[{"item_entry":25,"chance_bp":10000,"count":1,"group_id":0,"quest_only":false}]}}]
```

Skinning lists use a nonzero `skin_loot_id`, shared by every template that names it. This variant
can create a list before a template refers to it. Set the template's `skin_loot_id` separately. When a skinning roll produces no winner, the
existing profession logic grants one Light Leather, item 2318. Removing the list preserves that
fallback.

```json
[{"setSkinningLoot":{"skin_loot_id":42,"rows":[{"item_entry":25,"chance_bp":10000,"count":1,"group_id":0}]}}]
```

`CreatureSpell` has `spell_id: u32`, `priority: u8`, `condition: u8`, and `condition_value: u8`.
Condition 0 is always, 1 is self health below the given percent from 0 through 100, 2 is target
missing this spell's aura, and 3 is self missing this spell's aura. Higher priorities run first. Only condition 1 reads
`condition_value`. The AI reads both spell tables fresh when choosing a cast. Authored Casting
can suppress these lists. Existing casts and cooldowns remain.

```json
[{"setCreatureSpells":{"creature_entry":5200500,"spells":[{"spell_id":133,"priority":7,"condition":1,"condition_value":25}]}}]
```

```json
[{"setCreatureCast":{"creature_entry":5200500,"spell_id":{"some":133}}}]
```

To remove the fallback cast:

```json
[{"setCreatureCast":{"creature_entry":5200500,"spell_id":null}}]
```

### World effects and limits

After template or display edits, the reducer finds entities through `game_world_entity.by_entry`.
It despawns creatures with an instance-zero Authored Spawn and rearms each spawn for the current
time. The normal respawn pass creates the new life with the edited display, scale, stats and NPC
flags. Each affected entry is handled once per batch. This resets combat and life state; normal
despawn cleanup applies. Running Instance copies, population copies, Package spawns, pets,
guardians and summons stay in place. Their copied fields change at their next creation.
Instance-map source rows remain dormant. Loot and spell edits do not cause a respawn.

A batch may respawn at most **2,000 creatures**, summed across its affected entries. The
Refusal names the edit index, the entry that crossed the limit and the cap. A batch over the cap
is refused in full before any despawn. Repeating an entry in the batch counts its creatures once.
`pass_respawn` still rebuilds at most 256 creatures per 500 ms creature tick, so a full batch
needs at least eight ticks, about four seconds. Other due spawns may delay the new lives.
[Respawn measurements](#respawn-measurements) explain the cap and its limits.

Creature level-stat rows stay unchanged. The catalogue uses stored stats only while their
endpoints agree with the template; otherwise it synthesizes stats from the edited template.
The presence of imported level-stat rows continues to select the imported stat convention.
Death, pickpocket and skinning rolls read their lists fresh. Already-rolled `game_corpse_loot`
rows stay unchanged by list replacement.

Create, update and display replacement send one fresh `SMSG_CREATURE_QUERY_RESPONSE` per affected
entry to each authenticated native World Session, including Character selection. Multiple edits
to an entry send its final state once. Respawned entities reach clients through normal entity
updates. Headless Client coverage checks packet delivery and destroy/create updates. Real build
5875 client cache replacement and visual presentation remain unverified.

Limits are 64 edits, 128 rows per replacement list, and 512 replacement rows across a batch.
Names and subnames have a 255 UTF-8 byte limit and cannot contain NUL; names must contain text.
Creation checks every field. Updates check only changed fields; a cross-field rule runs if any
of its participating fields changes. Unchanged imported values pass even if they would fail on
creation. Other Gates require:

- Creature entries in 1 through 16,777,215, outside the Package Creature Range and Fixture-Reserved
  creature bands. The target must exist for every entry-based variant except creation.
- A present faction template; nonzero display id; finite positive scale; levels satisfying
  `1 <= level <= max_level <= 255`; positive health, max-level health and attack time;
  ordered damage and money bounds.
- Creature type 1 through 10, a protocol-supported creature family, rank 0 through 4,
  trainer type 0 through 3, and trainer class zero or a protocol-supported Character class.
- Display gender 0 through 2 and finite nonnegative bounding radius.
- Existing item and spell references, valid loot chances and groups, and AI conditions as above.

Every list replacement allocates explicit nonzero ids before deleting old rows. It starts above
the highest existing id outside that table's protected bands, then advances a transaction-local
cursor. Allocation skips the general fixture band 5,090,000 through 5,099,999. Pickpocket and
skinning ids also skip the Package Loot Range 9,000,000 through 9,999,999. Spell row ids skip the
Package Cast Range 10,000,000 through 10,999,999. Creature-loot rows are not Package-claimable;
displays use their entry's ownership. Exhausted ids refuse the batch. Replacement refuses a list
that contains protected rows, so it cannot delete someone else's rows to make space. Fallback
cast keys in the Package Cast Range are also refused. Imported explicit ids never require an
`auto_inc` reset.

One `info` line per edit records the caller and old -> new values after the entire batch succeeds.
Template lines list only changed columns. Text is escaped so names cannot add log lines. A refused
batch writes no successful edit log.

### Durability

Plain publish preserves these rows. Operator edits do not survive a `lyracore import` of the
owning Import Family:

| Import Family | Replaced rows |
| --- | --- |
| `creatures` | Templates, level stats and displays |
| `items` | Creature death loot |
| `loot` | Pickpocket and skinning loot |
| `casts` | Creature casts and spell lists |

The creature importer refuses source templates in the Operator Template Range. That reservation
prevents source allocation conflicts; it does not preserve Operator rows across an import.

### Respawn measurements

Measured on 2026-10-28 with an Intel Core i7-7820HQ, Linux x86_64, the release Wasm Module and
SpacetimeDB Standalone 2.7.1. Each case had one idle template with N live instance-zero Authored
Spawns, no imported world, combat, Package AI or clients. Five updates changed template health
and waited for every creature to return with the new health. Other processes were running on
this machine; these are observed durations, not a capacity guarantee.

| Live spawns | Empty batch median ms | Edit median / max ms | Recovery median s | Respawn ticks | Tick mean / upper bound ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 256 | 0.015 | 8.544 / 9.506 | 0.466 | 1 | 7.571 / <=10 |
| 1,000 | 0.013 | 34.514 / 36.250 | 1.920 | 4 | 7.881 / <=25 |
| 2,000 | 0.017 | 73.290 / 86.675 | 3.885 | 8 | 9.669 / <=50 |
| 4,000 | 0.019 | 204.285 / 226.194 | 7.738 | 16 | 12.775 / <=50 |

The final durable-suite run repeated the accepted sizes with the shipped cap. Other jobs
continued running; the larger peaks show why the initial curve needs headroom.

| Live spawns | Empty batch median ms | Edit median / max ms | Recovery median s | Respawn ticks | Tick mean / upper bound ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 256 | 0.023 | 12.050 / 14.960 | 0.429 | 1 | 13.049 / <=25 |
| 1,000 | 0.025 | 58.133 / 63.079 | 1.912 | 4 | 13.366 / <=25 |
| 1,129 | 0.020 | 41.779 / 65.225 | 2.414 | 5 | 10.282 / <=25 |
| 2,000 | 0.018 | 89.699 / 160.897 | 3.858 | 8 | 13.149 / <=50 |

The 4,000 case used a temporary measurement cap of 4,000; the shipped cap refuses it. Across
both runs, the 2,000 case's maximum used 32.2% of the 500 ms creature interval, leaving 67.8%
headroom. The interval is over three times that observed peak. Even the initial 4,000 case used
45.3%. The cap is a count limit, not an elapsed-time deadline.
The code and danger-zone guidance define no general reducer-duration target. The 256-per-tick
respawn limit bounds reconstruction work after a world import and remains unchanged. Movement
publication runs every 50 ms and melee every 100 ms, so an edit can delay those shorter schedules.
This measurement does not promise uninterrupted movement during an Operator edit.

Edit and empty-batch times come directly from the HTTP `spacetime-execution-duration-micros`
header, so no wall-clock baseline subtraction is needed. Recovery starts after the edit response
and polls at 50 ms intervals. Tick durations come from differences in the
`spacetime_reducer_plus_query_duration_sec` histogram at `/v1/metrics`, including subscription
query work. The upper bound is a histogram bucket boundary, not an exact maximum. Tick means
include every creature tick during recovery. The benchmark has no elapsed-time assertions;
it checks successful edits and complete reconstruction.

Run the durable benchmark on its own, with Python 3 and the prerequisites in
[Testing](testing.md#durable-tier):

```bash
mkdir -p target/measure-tmp
TMPDIR="$PWD/target/measure-tmp" cargo test -p lyracore-module --test operator_creatures \
  benchmark_creature_template_respawns -- --ignored --nocapture
```

`LYRACORE_BENCH_SIZES` defaults to `256,1000,1129,2000`; `LYRACORE_BENCH_REPEATS` defaults to 5.
Use increasing sizes within the cap. The test owns a Standalone on a free loopback port.
The same driver can use a Disposable Native Host and one Headless Client. By default, 32
creatures are in the client's AOI and the rest are distant. It checks a destroy/create pair for
each visible creature and one template response per edit, and reports wire bytes.

```bash
TMPDIR="$PWD/target/measure-tmp" LYRACORE_BENCH_SIZES=1129,2000 \
  python3 module/tests/operator_creature_benchmark.py --native-host /path/to/spacetimedb-standalone
```

The pinned Native Host, SpacetimeDB 2.13.0 with LyraCore extensions, gave these additional
five-update results on the same machine:

| Live / visible spawns | Edit median / max ms | Recovery median s | Ticks | Tick mean / upper bound ms | Wire bytes per edit |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,129 / 32 | 44.946 / 53.924 | 2.358 | 5 | 13.383 / <=25 | 6,130 |
| 2,000 / 32 | 82.676 / 83.913 | 3.837 | 8 | 15.058 / <=25 | 6,130 |
| 2,000 / 2,000 | 100.403 / 105.121 | 3.796 | 8 | 58.802 / <=100 | 380,042 |

Every edit delivered exactly the expected destroy/create pairs and one template response.
Wire totals include four-byte packet headers. The dense case's 105.121 ms maximum leaves 79%
of the 500 ms creature interval. Its reconstruction ticks all stayed within the 100 ms histogram
bucket. The durable-suite peak above remains the limiting measurement. The 2,000 cap is about
1.77 times the largest source entry; raising it to 4,000 spends headroom without helping that snapshot's entries.
The Host binary SHA-256 was
`2d8a48c830cc6b7e4c17efe5272cf6b1b6eacb8eafe9334ce55373790936adce`.

`LYRACORE_BENCH_VISIBLE=2000` puts every creature in this run in the client's AOI. Native delivery
cost scales with observer grants: each visible life produces one destroy and one create per
observer. Template responses scale with authenticated World Sessions, once per edited entry.
The ordinary cleanup path uses indexed entity, spawn, threat, loot and AOI lookups. One broader
read remains: `combo::forget` scans all combo-point rows for each despawn. Relay cancellation can
also scan active Relay definitions when it releases references. Removing these costs requires
changes outside the template editor; neither dominates the empty-state measurement above.

The cached vmangos SQLite snapshot had 1,141 migrations, highest id `20260913120518`, matching
`importer/scripts/vmangos.lock`. Its SHA-256 was
`a8ce8ecc8e9c9d00dfe23321d0763cf8aee19677c836f1e1b9acc15fb1fbd5c9`.
A read-only SQL census applied patch 10 and the importer's `spawn_flags & 2 = 0` and nonzero-entry
filters. All retained source rows import with instance zero; Instance-map rows stay dormant.
The largest entry was **16356**, Scourge Invasion Minion, finder, with **1,129 positions**, all on
maps 0 and 1. These belong to events 90 through 95, so this is a source-count upper bound,
not a claim that all 1,129 are normally live together. Rabbit, entry 721, had 456 source positions,
of which 452 were on open-world maps. The 2,000 cap covers the largest source count with 871
positions to spare. No full world import was run for this census.

This query reproduces the source-count upper bound on `mangos.sqlite`:

```sql
SELECT id AS entry, COUNT(*) AS source_positions
FROM creature
WHERE patch_min <= 10 AND patch_max >= 10
  AND (spawn_flags & 2) = 0 AND id <> 0
GROUP BY id
ORDER BY source_positions DESC
LIMIT 10;
```

The boundary test verifies that 2,001 live creatures refuse the whole batch without changing
lives or templates, then removes one spawn and verifies that two updates to the same entry
rebuild exactly 2,000 creatures once. Populated-world combat, many simultaneous observers,
production hardware and real build 5875 presentation remain unmeasured.
