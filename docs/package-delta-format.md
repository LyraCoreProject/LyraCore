# Package Delta format

The **Package Delta** artifact and the claim conflict tracer.

A Package Delta is the durable record of what one Package changes in the spell tables. It names
the Package, the source it was generated from, and one claim per row: the table, the typed
primary key, the operation, and the columns the Package sets there. A base import replaces whole
data families, so a Package's row edits have to be a replayable artifact rather than a one-shot
edit that the next import silently reverts.

This library is pure and deterministic. It reads no files, keeps no state, reads no clock, and
writes nothing. It parses, canonicalizes, and traces; applying a delta to a shard is a separate
concern. The crate also ships `lyracore-delta-check`, a command that reads named artifact files
and prints the plan they form. File access lives in the command.

# The artifact

```json
{
  "version": 1,
  "package": "example.sharper-fireballs",
  "source_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "claims": [
    {
      "table": "game_spell",
      "key": { "spell_id": 133 },
      "operation": "update",
      "fields": {
        "cooldown_ms": { "type": "u32", "value": 1500 },
        "family_flags": { "type": "u64", "value": "18446744073709551615" }
      }
    },
    {
      "table": "game_spell_effect",
      "key": { "spell_id": 133, "effect_index": 0 },
      "operation": "update",
      "fields": { "base_points": { "type": "i32", "value": 120 } }
    }
  ]
}
```

Every value states its type. An unsigned 64-bit value is a decimal **string**, because a JSON
number loses precision above 2^53 and would come back silently wrong. A `game_spell_effect` key
names the spell and the effect index; the packed durable key
(`(spell_id << 2) | effect_index`) is derived, never authored.

# What it refuses

Unknown versions, tables, columns, type tags and malformed keys fail at `PackageDelta::parse`,
before an applier sees the artifact. So does row deletion: uninstalling a Package replays the
affected families without its claims, and there is no pre-image a delete could be undone from.
An `update` names only the columns it changes; an `insert` carries the whole row.

# What it traces

`trace` merges the claims of several Packages. Different columns of one row merge. The same
column, or two inserts at one primary key, is a `ClaimConflict` naming both Packages and the
exact claim. There are no priority numbers: the tracer reports that a human has to choose.

# Import Families

Every claimable table belongs to one Import Family, the unit a base import clears and reloads
and the unit an apply runs for. `Table` is one closed enum with its variants grouped by
family, and `Table::family` names the owner, so an applier called for one family can tell that
a claim belongs to the import it is running. This build's catalogue is the spell, item, quest,
loot, cast, trainer, gossip, globals, spellmeta, creatures, gameobjects and creature-ai
families. `game_creature_quest`/`game_gameobject_quest` (quest givers) and
`game_creature_loot`/`game_npc_vendor` (creature-scoped loot and vendor stock) are out of it.
So is `game_start_item`, which two importer passes own between them, `game_creature_waypoint`,
which carries no map to route by, and the two gameobject pool tables, which no base import
writes. So are the `EventAI` definition tables, whose rules are a nested payload no scalar column
shape can state: a Package Delta claims typed rows, never a script blob. See
`Table::columns`, `LOOT_FAMILY`, `GLOBALS_FAMILY`, `CREATURE_FAMILY`,
`GAMEOBJECT_FAMILY` and `CREATURE_AI_FAMILY` for why.

# Spatial claims and routing

Two tables are SPATIAL: `game_creature_spawn` and `game_gameobject`. Their claims carry the map
in the primary key rather than in a column, and `PrimaryKey::map_id` reads it. A spatial claim
reaches only the Shards whose World Import Scope owns that map; the importer applies that filter
with the scope it already builds for the base import, so there is one routing concept and this
crate only has to state which map a row sits on. Every other table is a global catalogue every
Shard loads whole.

A spawn's durable guid is derived from the key, never authored: `game_creature_spawn` packs its
creature template and spawn identifier into one guid, `game_gameobject` its spawn identifier
alone. The map is not part of either guid, which is why it is identity for routing and not a
claimable column: a Package cannot move a placed row onto a map another Shard owns.

Most tables permit insert and partial update. Six are update-only, because their primary key
names something no Package may invent: a creature template (`game_creature_cast`,
`game_gossip_menu`), a race, class or level the client fixes (`game_class_level_stats`,
`game_level_stats`, `game_start_position`), or an `AreaTrigger.dbc` trigger id
(`game_areatrigger_teleport`). `DeltaError::InsertNotSupported` refuses every insert on those
at parse time, regardless of identifier.

# Identifier policy

An inserted row must sit in its family's Package identifier band, which no client and no import
can reach: for spells, `PACKAGE_SPELL_ID_FLOOR`..=`PACKAGE_SPELL_ID_CEIL`; for items,
`PACKAGE_ITEM_ID_FLOOR`..=`PACKAGE_ITEM_ID_CEIL`; for quests,
`PACKAGE_QUEST_ID_FLOOR`..=`PACKAGE_QUEST_ID_CEIL`; for loot rows,
`PACKAGE_LOOT_ID_FLOOR`..=`PACKAGE_LOOT_ID_CEIL`; for `game_creature_spell` rows,
`PACKAGE_CAST_ID_FLOOR`..=`PACKAGE_CAST_ID_CEIL`; for `game_trainer_spell` rows,
`PACKAGE_TRAINER_ID_FLOOR`..=`PACKAGE_TRAINER_ID_CEIL`; for the insertable gossip rows,
`PACKAGE_GOSSIP_ID_FLOOR`..=`PACKAGE_GOSSIP_ID_CEIL`; for the insertable globals rows,
`PACKAGE_GLOBALS_ID_FLOOR`..=`PACKAGE_GLOBALS_ID_CEIL`; for `game_spell_learn` rows,
`PACKAGE_SPELLMETA_ID_FLOOR`..=`PACKAGE_SPELLMETA_ID_CEIL`; for the creature template and
spawn rows, `PACKAGE_CREATURE_ID_FLOOR`..=`PACKAGE_CREATURE_ID_CEIL`; for the gameobject
template, trap and spawn rows,
`PACKAGE_GAMEOBJECT_ID_FLOOR`..=`PACKAGE_GAMEOBJECT_ID_CEIL`; for the broadcast text,
summon placement and quest event requirement rows,
`PACKAGE_CREATURE_AI_ID_FLOOR`..=`PACKAGE_CREATURE_AI_ID_CEIL`. `game_spell_chain` and
`game_spell_proc_event` key on a spell identifier rather than one of their own, so an insert
there takes the Package spell band. Updates can tune imported rows, but every operation refuses
fixture-reserved identifiers. See `ids` for the bands and their formulas.

# The other artifact

`script` holds the **Script Artifact**: the whole Runtime Scripts one Package ships. It is a
second kind rather than more tables in the claim schema because a Runtime Script has no base
import behind it, so a Package owns the whole row and two Packages meeting on one is a collision
rather than a merge.

Both kinds live in one Package's generated directory, so a reader that walks `*.json` meets
both. `artifact_kind` reads the root `kind` member alone and says which is which; each
parser refuses the other kind by name rather than complaining about its members.
