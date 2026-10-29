# World Editor

The World Editor is [Meld](https://github.com/LyraCoreProject/Meld/tree/lyracore), a 1.12.1 map editor. Its
`lyracore` branch reads spawns, items and creatures from a Realm and saves their edits through the
[Operator Content API](./operator-content-api.md), so a moved spawn, a renamed item or a creature's
new look shows in the world at once.

## Run it

```bash
./lyracore dev up        # or ./lyracore dev watch
./lyracore editor
```

`lyracore editor` builds the Meld commit pinned in `.lyracore-editor-rev` into
`.lyracore/editor/<rev>/` on first use. That takes a few minutes and needs Git, Go, Bun and Wails, and
on Linux the webkit2gtk 4.1 development package. Later runs start the installed build.

Meld starts connected to the local Realm: `http://127.0.0.1:3000`, the `lyracore` Shard, and the path
of the Owner Token in `.lyracore/native-owner-token`. Meld reads the token from that file. The client
folder is the parent of the configured `client-data` path (`./lyracore config set client-data PATH`).
These settings hold for that Meld session only and change nothing in Meld's own config.

Open a region (File › Open Region), edit its creatures and gameobjects, then press Save Spawns. The
status line says "live now" when the Realm took the edits. The Item Editor and the Creature Editor
save to the Realm the same way. While a save runs, the Saving bar reads "Updating the realm".

## With Development reload

`lyracore dev watch` publishes the Module again with `clear=false`, so the Operator claim and the
spawn rows stay. Meld keeps working across a reload. A reload changes Module code, not spawns, so
Meld does not need to read the region again.

## What a save does

Save Spawns sends the region's edits as one `operator_apply_spawn_edits` call, one transaction. A
Refusal rolls back the whole save, and Meld shows its message.

- A creature changed to another entry is deleted and created again, because its guid holds the entry.
- The Realm gives new spawns their guids, so Meld reads the region again after a save that added any.
  A new creature's patrol is saved by the next save, once it has a guid.
- Fields a LyraCore spawn has no column for (scale, equipment and pose, wander distance, formations,
  pools) are listed as not written.
- A save over the call's limits (256 edits, 4,096 waypoints) is refused. Save fewer spawns at a time.

## Item and creature saves

- An item save calls `operator_apply_item_edits` with the full template row, and the creature
  editor's Shop tab saves the vendor offers in the same call. Players who wear the item get its new
  stats, and connected clients receive the new item query response.
- A creature save calls `operator_apply_creature_edits` with the template, its display rows and its
  spell list. The Realm respawns the live creatures of that entry, so they come back with the edit.
- New items and creatures get entries in the Operator Template Range. Meld has no delete for either.
- Fields the Realm has no column for are listed as not written. Meld's
  [client notes](https://github.com/LyraCoreProject/Meld/blob/lyracore/docs/architecture/client.md#lyracore-items-and-vendors)
  list them.

[Item Refusals and limits](./operator-content-api.md#item-refusals-and-limits) and
[Creature templates](./operator-content-api.md#creature-templates) give the limits.

Saved edits change the live database only. `lyracore import` removes them. To keep an edit across an
import, put it in a Package ([durability](./operator-content-api.md#durability)).

## Known gaps

- On a Realm, Meld writes its terrain and client file edits to `patch-4.MPQ`, beside the
  `patch-3.MPQ` that `lyracore client sync` writes. The terrain import does not read patches above
  `patch-2.MPQ`, so terrain edits do not reach the Realm.
- The gameobject, quest, gossip, loot and spell editors do not write a Realm yet. On a Realm they say
  so and save nothing.

## Change the pinned build

Push the Meld commit to the `lyracore` branch of
[LyraCoreProject/Meld](https://github.com/LyraCoreProject/Meld) and write its full SHA to
`.lyracore-editor-rev`. The next `lyracore editor` builds it. Upstream's changes come in by merging
`Coldensjo/Meld`'s `main` into `lyracore`.
