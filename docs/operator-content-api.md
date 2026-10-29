# Operator Content API

The Operator Content API edits creature and gameobject spawns, item templates and vendor offers on
a running Realm. A world editor such as Meld calls it with the Owner Token. Spawn edits despawn the
old live entity and let the respawn pass make the new one. Item edits refresh connected wearers and
send new item query answers to clients.

The source is `module/src/operator_content.rs` and `module/src/operator_content/items.rs`. The
durable tests are in `module/tests/operator_content.rs` and `module/tests/operator_items.rs`.
`native-host/operator_items.py` checks item edits with the Headless Client.

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
