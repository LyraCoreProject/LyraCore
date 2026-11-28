# Danger zones

This page is the authority for the traps below and for schema migration rules. Each entry cost
real debugging time at least once. A change that crosses a line in §1 needs a human review before
it ships, whoever wrote it.

To deploy this revision, follow [the native operations guide](native-operations.md). It covers
backup, publication, migration calls and verification for a Realm with one Native Host and one
Shard. An existing multi-Shard Realm needs a reviewed data migration first; see
[Multi-Shard Realms](migrating-to-native-host.md#multi-shard-realms).

## 1. Do-not-touch traps

1. **`spacetime publish -c` is destructive.** It wipes the database and breaks login until
   `claim_operator` runs again. Never run it. Plain `spacetime publish` auto-migrates, or aborts
   without touching the data. `lyracore publish` refuses to forward `-c` or any other flag-shaped
   argument, so deploy through it.
2. **Migrations:**
   - A new column goes at the END of the struct with a `#[default(...)]`. That is an additive
     auto-migrate. A column anywhere else makes the publish abort.
   - SpacetimeDB SQL INSERT needs every column, and it matches values to columns by position, not
     by the names it lists. It never checks those names. The importer and `Standalone::assert_sql`
     read the published columns with `spacetime describe` and pass each statement through
     `lyracore_shared::import_sql::in_table_order`. That writes every published column in published
     order. It takes each value from the statement or else from `import_sql_defaults.txt`, and
     refuses a name the table lacks or a column with neither, before any statement of the batch
     runs. The Native Host's `host.sql` reorders the same way through `native-host/sql_defaults.py`.
     `spacetime describe` and `SELECT *` report the published snake_case names (`value1` as
     `value_1`), SQL accepts either name, and both writers convert the statement's names before they
     compare. Both `canonical` ports follow SpacetimeDB 2.14's convert_case 0.6 naming; recheck them
     when SpacetimeDB is upgraded. Any other SQL writer must name columns in table order. Durable
     tests write fixture rows with `Standalone::insert_row`, which reads the published columns and
     their declared defaults, so a new column needs nothing there. When an existing importer or
     native check INSERT can omit the new column, add its default to
     `crates/lyracore-shared/src/import_sql_defaults.txt`. The importer, the native check and the
     `conformance/support` worlds fill it from there.
   - Change native reducer arguments and their Host calls together. The Host and Module also share
     the BSATN layouts of `game_native_cipher` and `game_native_packet`. Update
     `native-host/src/feed.rs` with `module/src/native/mod.rs`, then run the native protocol check.
     Both sides' unit tests pin these layouts, and `game_native_motion_permit`, to the rows in
     `native-host/src/delivery_rows.rs`. Publish a Module with `game_native_motion_permit` before
     you start a Host that subscribes to it; that Host's output subscription fails without it.
   - `./lyracore dev watch` permits only an unchanged Module definition. Its development Host
     compares all reducer arguments and private table layouts before it replaces the Module.
     SpacetimeDB's normal migration check does not compare reducer arguments. Use a planned
     update for schema changes; see [development reload](quickstart.md#development-reload).

   **A defaulted column's default can be a valid value, not a sentinel (#456).** `cell` was
   END-appended to the AOI-scoped tables as `#[default(0i64)]`, and 0 is the packed id of grid cell
   (0, 0). Every existing row claims that cell the moment the publish lands, and AOI looks `cell` up
   by equality. Moving entities fix themselves on their next heartbeat. Static `game_gameobject`
   rows never restamp and stay invisible until backfilled. After a publish of this shape, call
   `debug_backfill_cell_ids` as the Operator before you open the native listeners.
   When you END-append a defaulted column that something indexes or filters on, ask what the
   default means as a value. The migration will apply cleanly and still be wrong.
3. **The 5875 partial-VALUES crash.** Any partial `UNIT_FIELD_*` update must go through the
   `dirty_reset` path so it never carries `OBJECT_FIELD_TYPE`. Sending TYPE again crashes the
   client (null+0x110). Copy `build_health_values` or `build_resistance_values` in
   `crates/lyracore-protocol/src/codec/values.rs`; each has a wire-pinning test.
4. **`SMSG_SET_FACTION_STANDING` sends the reputation index** (`Faction.dbc` `ReputationListID`),
   not the `faction_id`. The id indexes past the client's 64-slot array and crashes the client
   with ERROR #132. It and `SMSG_INITIALIZE_FACTIONS` send the stored standing less the
   Character's base standing; the client adds the base from its own Faction.dbc.
5. **Don't gate features on client-display readback.** The server is authoritative, so verify
   effects server-side. Flag client visuals for a human to look at.
6. **`#[default(...)]` cannot default a `String` column.** The SpacetimeDB macro type-checks every
   default inside a `const` block, and dropping a `String` in const-evaluated Rust is
   `error[E0493]` on stable. This blocks `#[default(String::new())]` and `#[default("")]` alike.
   Numeric and bool defaults are fine. Put new `String` data in a new table instead: it has no
   existing rows, so no column needs a default. `game_creature_waypoint` and `game_creature_spell`
   use the same one-row-plus-child-rows shape. See `NpcTextSlot` in `module/src/gossip.rs`.
7. **Never `.iter()` or `.count()` a spatial table** (`game_world_entity`, `game_corpse`,
   `game_creature_spawn`, `game_gameobject`, `game_dynamic_object`). That includes the usual
   `let entities = ctx.db.game_world_entity();` handle, which is the same whole-world read one line
   later. Use `helpers::entities_near(ctx, map_id, instance_id, x, y, radius)` for a radius search
   and `helpers::in_same_partition(&e, map_id, instance_id)` for a membership test. A whole-table
   scan reads every map and every Instance, so it costs O(world) where the helpers cost
   O(neighbourhood), and it ignores the map and Instance partition that gameplay depends on.
   `partition_discipline_tripwire` in `module/src/tripwires.rs` enforces this with a per-file
   `WHITELIST` that only shrinks; a new scan fails `cargo test -p lyracore-module --lib`. Raise a
   budget only for a realm-wide or partition-managing read, with a one-line reason. A budget
   larger than the real count fails its own test.

### Changing the World Source

A classic-db to vmangos re-import replaces world content and changes spawn guids. Rebuild Package
Spatial Claims against the new Import Revision and recreate Instances. Classic-only quest, item
and credit ids can disappear while Character records still refer to them. Follow
[the source migration procedure](native-operations.md#move-an-existing-realm-from-classic-db-to-vmangos),
including a restored-copy rehearsal and the order of world import before Package apply.

### After every publish

Call `debug_repair_after_publish` as the Operator on the Shard. `init` does not run again on an
auto-migrating publish, so this reducer arms the interval schedules `init` arms
(`arm_interval_schedules` in `module/src/seed.rs`). It deletes and re-creates the instance reaper,
the Gateway lease reaper, the weather schedule, the Game Event pass, the Meeting Stone reminder and
the catch-all creature tick, which discards a cadence set with `debug_rearm_creature_tick`. It creates the event reaper,
melee, pet care, aura, breath, duel, ground area and motion publish schedules only when missing, and
a Transport crossing for each Transport without one. It also re-freezes Proc profiles, restores
missing Auction expiries, re-tags legacy auction mail as Auction Mail before the Mail Timer pass
(so a legacy "Auction won" is deleted, not returned), arms a Mail Timer for each Mail that has
none, moves each legacy Character GM level onto its Account, stamps the current time as the
respawn time of each opened or despawned world chest that has none and a template `respawn_secs`
above 0, and re-seeds the curated content and fixtures `init` seeds. That chest step reaches only a
Package or curated chest whose template already has a respawn time. An imported chest has none until
the `gameobjects` Import Family is reimported with the updated importer, which fills `respawn_secs`
from the smallest positive vmangos `spawntimesecsmin` of the entry and gives each spawn its own
range; that reimport also resets every chest to closed and clears every picked lock. `packages
apply` calls it for each Shard it publishes. A skipped call looks like a mid-session hang, not an
error.

Imported content wins over that re-seed. A seed that writes real ids skips once the importer has
stamped its family in `game_import_meta`: `spell` (Weakened Soul, Frost Armor, Chilled, Drain
Soul, Mana Burn, Stealth, Demon Skin, the stacking probe spells, the synthetic Warrior talents),
`items` (Soul Shard), `globals` (the starting spell kits, the Hunter Tame Beast kit row, the
Warrior crit rates) and `dbc` (the Boar creature family, the Riding skill line). Fixtures on
Fixture-Reserved Identifiers and the `SPELL_GROUPS` rules are written on every Shard; the
importer does not import spell groups. `seed_curated_content` in `module/src/seed.rs` holds the list.

The first run on a Shard with Mail from before the Mail Timer applies the vanilla 30-day rule
(3 days with a cash on delivery price) to that Mail. Every Mail already past its life returns to
its sender or is deleted with its copper, at 100 Mail a second. That is a maintainer decision, not
a fault.

### Vendor stock migration

`game_npc_vendor` end-appends `restock_seconds` and `stock_flags` with typed zero defaults.
The private `game_vendor_stock` and `game_vendor_stock_runtime` tables start empty. Publish the
Module before starting the updated Native Host: its first connection announces a process identity
through `native_vendor_stock_runtime`. A new process identity clears stock; reconnects and Module
publication within one Host process retain it. An older Module lacks that reducer and the updated
Host refuses the connection. No native delivery row or existing reducer argument changes. Upgrade both Module and Host for
restart resets; an older Host does not announce a new process.

Reimport the `items` Import Family once, with the updated importer, to gain stock and restock on
an existing Realm. This is one cold import of the source item catalogue, creature loot and vendor
offers; it clears current stock counters. Item templates at or above the Operator Template Range
floor survive, including Operator-created items. Loot and offers that refer to those item entries
or creature entries survive too. Imported rows must not collide with their retained row IDs.
Before its first write, the importer refuses a plan that would leave any owned Item or retained
loot/offer without an item template. A source entry removed from the new catalogue can therefore
block the import; resolve it through a reviewed content migration before retrying. These checks
assume a cold import with no concurrent content edits or Item grants.
Package claims replay only when `--packages` is supplied; replay does not restore Operator edits.
With that flag, the same preflight also refuses removal of an owned or referenced Package Item
template that the enabled Package claims would not restore.
The import does not replace Characters or their owned Items. Supply the same client Data directory
(`--dbc`) required by the items import's Random Property validation; no separate DBC catalogue
reimport is needed for vendor stock.
Without that reimport, the zero restock interval preserves today's purchases: the list shows the
configured maximum and purchases do not consume stock. Seeded and authored offers with a zero
interval keep that behavior. Direct SQL INSERTs must supply both appended columns; shared import
SQL defaults fill older inserts. Rust Packages that construct `NpcVendor` must supply both fields.

Stock belongs to a vendor spawn and item, independently of other spawns with the same entry or
shared offers. Ordinary death, corpse removal and respawn retain stock and elapsed time. Removing
the spawn or its Instance discards stock. These lifetimes follow the in-memory creature object in
vmangos; LyraCore does not implement vmangos grid unloading. Server restart restores full stock.
The counter is lazy: each elapsed interval restores `buy_count` items, capped at the maximum.
A partial refill observed by a list read or buy check resets its timestamp to that request,
discarding the remainder. A refused buy can still observe restock, including this timestamp reset.
It never pays for, stores or consumes the refused purchase. A failure during storage commit rolls
back the payment, stock debit and queued acknowledgement together.
A successful purchase resets the timestamp and selects the next interval. Offer flag 1 selects
80 through 120 percent of the interval; flag 2 scales it above 2500 active, unqueued World Sessions.
Integer truncation occurs after each scaling operation. A computed zero interval is floored to one
second to avoid the Reference Server's division by zero for such source inputs, as agreed by the
maintainer.

These rules come from `Creature::GetVendorItemCurrentCount`, `UpdateVendorItemCurrentCount`,
`VendorItemCount` and the creature destructor at
[vmangos d9173eca](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Creature.cpp).
List zero counts remain visible, as in
[`SendListInventory`](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Handlers/ItemHandler.cpp).

### Absorb school migration

No schema changes. A school absorb Aura (aura 69) now takes damage only when its `eff_p0` school
mask covers the damage's school, and a Mana Shield (aura 97) is an `A_ABSORB` row marked by
`p0_kind` `P_MANA_SHIELD_MASK`, with its school mask in `p0` and its mana cost per absorbed point
in `p1`; see [Absorb auras](schema.md#absorb-auras). School absorbs run first, then Mana Shields,
which spend that mana and absorb no more than the unit's mana pays. Reimport the `spell` family
straight after the publish, then call `debug_repair_after_publish`. Until the reimport a Mana
Shield row has mask 0 and no marker, so it absorbs nothing, and a live Mana Shield Aura keeps that
until the repair copies the new row onto it. Every other absorb row already carries its mask.

### Base reputation migration

`game_faction` gains `base_standing_2` to `base_standing_4`, `base_race_mask_1` to
`base_race_mask_4` and `base_class_mask_1` to `base_class_mask_4`, end-appended with default 0.
`base_standing` keeps slot 1. A Character starts each faction at the first slot whose race and
class masks admit it, a zero mask admitting all, and at 0 when none does. Until a reimport every
mask is 0, so slot 1 admits every Character. Interaction refusal now reads that base for a faction
with no stored row instead of the faction templates, so until the reimport a Character counts as
Friendly to the other team's capital cities (their slot 1 holds 3100) and their vendors, taxis and
quest givers serve it. Reimport the `dbc` family straight after the publish. Shared import SQL
defaults fill older inserts.

`SMSG_INITIALIZE_FACTIONS` and `SMSG_SET_FACTION_STANDING` now send a stored standing less the
Character's base (trap 4). A reputation pane for a faction with a non-zero base drops to the correct
value: the client used to add its base to the full standing.

Every `game_player_reputation` row written before the reimport holds slot 1's base. Order: publish,
reimport the `dbc` family straight away, and the `dbc` stamp from `stamp_import_meta` runs a
one-shot repair. It sets each row to `standing - base_standing + the Character's base`, clamped,
and records the `repair_base_standing` family in `game_import_meta` so it never runs again. It
skips, and records nothing, until some reputation faction has a non-zero mask, and while any
Character with a reputation row is in Transfer; a later `dbc` stamp then runs it. Do not write rows
on new masks before the stamp: the repair would shift them too. Move no Character between Shards
until both Shards have run the repair, because a Transfer carries its rows unchanged.

### Buyback migration

`game_buyback_item` and `game_buyback_state` are new private tables. Existing table layouts and the
Native Host delivery layouts do not change. Use the normal planned publish and
`debug_repair_after_publish`; no Shard clear or content reimport is needed for buyback.

The legacy `game_character_buyback` table keeps its schema for additive migration. Its snapshots
cannot restore Item identity and are discarded by the repair reducer or the Character's next login.
The repair reducer ends connected World Sessions that have no `game_buyback_state` row. Their
clients discard legacy buyback links, prices and synthetic objects; the next login starts a fresh
ring and timestamp origin. World Sessions that already have buyback state stay connected. Run the
repair reducer before play resumes after the upgrade, even when the legacy table is empty.
A Handoff or World Port within one World Session preserves buyback, including its login origin
and paused Temporary Enchantments. Logout and fresh login discard it, including login after a
crash or a sessionless Transfer. Carried and banked Items retain their existing rows and GUIDs.

The Character Transfer table list now carries buyback without changing table layouts or the blob
format. The existing compatibility path fills both tables empty for older manifests that omit
them, so sessionless import and fresh login still work. A native World Port requires the original
buyback session row and refuses import atomically if it is absent; an older destination refuses
the new manifest tables. Update both Shards before moving active World Sessions. Retain the source
copy after a refusal and do not attest an import that failed. The older Item row codecs remain
explicit in `module/src/transfer/legacy_item_rows.rs`.

### Call for assistance migration

`game_faction_template` gains `flags`, end-appended with default 0. The private one-shot
`game_creature_assistance_call` schedule is new and starts empty. Reimport the `dbc` family
straight after the publish. Until then no faction template carries the respond-to-call-for-help
flag, so no creature answers a Call for Assistance, including the pack mates that joined a
proximity pull before this revision. Direct SQL INSERTs must supply `flags`; shared import SQL defaults fill
older inserts. No repair is needed.

### Chest despawn and spawn respawn range migration

`game_gameobject` gains `respawn_min_secs` and `respawn_max_secs`, end-appended with default 0, and
`despawned`, end-appended with default false. Both defaults keep a row written before the publish
working as before: 0 and 0 mean the spawn has no range of its own and takes its template's
`respawn_secs`, and false keeps the object shown. No backfill is needed. Reimport the `gameobjects`
Import Family so each source spawn carries its vmangos `spawntimesecsmin` to `spawntimesecsmax`
range; until then every spawn of a chest entry keeps the entry's smallest positive time.
`import_gameobjects` takes a packed row of 12 fields, as before, or 14 with the range.

A chest opened before the publish keeps its respawn time. When that time comes the chest despawns,
and it respawns one respawn delay later. A Package that builds a `GameObject` row must set the three
new fields, so its branch pins a Core revision with them.

### GameObject pool migration

`game_gameobject_pool_member` gains `rotation_0` to `rotation_3`, `respawn_min_secs` and
`respawn_max_secs`, end-appended with default 0. The private `game_gameobject_pool_link` and
`game_gameobject_pool_copy` tables are new and start empty. `game_gameobject_pool.in_place` keeps its
column but nothing reads it. No other layout changes, and no Package builds a pool row.

A Realm published without a reimport behaves as before: every imported chest, herb and ore
placement is a static spawn, and the pool seeded at `init` (map 0's copper and tin vein) keeps
rolling. One thing changes for that pool and for any pool written by hand: a used member's
replacement now waits hidden for its respawn delay. A pool member's respawn no longer rolls the
pool again.

Publish the Module before you reimport: the importer calls the new `arm_pools` reducer. Reimport
the `gameobjects` Import Family to import the pools. The reimport replaces every pool row,
including the seeded pool, and writes no static spawn for a pooled placement. Its first
`import_gameobjects` batch removes every old object, also when a slice holds only pooled
placements. The importer then rolls the open-world pools through `arm_pools`, in batches that bring
up at most 250 objects each, like the static spawn batches. Only the rolled members are up
afterwards: about 6,700 of the 32,550 pooled placements in the full world. A rerun writes the same
rows and rolls each pool once again, never twice over. `arm_all_pools` refuses a Realm with more
than 250 top-level pools, so `import-world.sh` no longer calls it. An Instance created after the
reimport rolls the pools of its map; a running Instance has lost its objects to the reimport, as
before, and gets them back when it is created again. Shared import SQL defaults are not needed: the
importer writes every member column.

The import floors in `importer/scripts/import-world.sh` count the GameObjects the import wrote,
static objects plus pool members, so a roll that shows only some members does not fail them.

### Game event migration

The private `game_event`, `game_event_link`, `game_event_pool`, `game_event_status` and
`game_event_override` tables and the `game_event_schedule` scheduled table are new and start empty.
No existing column or reducer argument changes, so no Package builds a changed row.

**A publish alone changes nothing a player sees.** With no link, every spawn shows as before, and a
creature population member's `event_ids` keep its positive-event members out of selections as
before: creatures tied to an event stay parked, creatures tied to its absence stay up, and event
GameObjects stay up. Call `debug_repair_after_publish`, which arms the Game Event pass; it has no
work until links exist.

Then import in one of these orders, with the updated importer:

1. The full `import-world.sh` run. Its links are written before the spawn batches, so every spawn
   and pool comes up in its event's state.
2. `--family creatures` or `--family gameobjects`. Each also writes the `events` family before its
   spawn batches, so links always name the spawns the import writes.
3. `--family events` alone. It clears `game_event_status`, and the minute pass then brings the
   existing world to each event's state, at most 500 changes a minute: the full world links about
   12,000 GameObjects and 3,000 creatures, so a Realm imported before this change settles within
   about half an hour. A population whose selection was made while `event_ids` kept its members out
   rolls again the first time its event shows it.

A rerun writes the same rows. Operator holds survive an import; `debug_event_clear` releases a hold
whose event an import removed. An importer from before this change re-importing `creatures` writes
`event_ids` but no links; if no `events` import ever ran, the Realm keeps the behavior before Game
Events, otherwise the existing links still name the same spawns.

### Game event condition migration

`game_creature_loot` and `game_fishing_loot` gain `event_entry`, `event_reversed` and
`equal_chance`, end-appended with defaults 0, false and false; shared import SQL defaults fill older
inserts. Rust Packages that construct `CreatureLoot` must supply all three fields. A Package Delta
fishing loot claim cannot name them, so a Package fishing row has no event and no equal chance. The
stored EventAI `EventPredicate` gains `GameEventRunning` and `GameEventNotRunning` after its
existing variants, so stored rules keep their discriminants; SpacetimeDB classifies the appended
variants as a client-breaking change, which a normal publish acknowledges without deleting data (see
[command lists in EventAI](data-ingestion.md#command-lists-in-eventai)). `game_gossip_condition`
keeps its columns and gains kind 12 as data. No reducer argument changes.

**A publish alone changes nothing a player sees.** Every loot row still has no event, so event
loot such as the Hallow's End Pumpkin Bag and the winter fish keeps dropping all year; every
imported type-12 gossip condition is still stored as the unavailable sentinel, so its options stay
hidden; and the EventAI import left the rules on an event out.

With the updated importer, reimport `items` for creature loot (a cold import of the item catalogue,
creature loot and vendor offers; see [vendor stock migration](#vendor-stock-migration)), `loot`
for fishing loot, `gossip` for gossip conditions and `creature-ai` for EventAI rules. Each family
takes effect alone. A rerun writes the same rows. A corpse rolled before the `items` reimport keeps
the loot it rolled.

### Game event creature data migration

The private `game_event_creature_data` table is new and starts empty. The private
`game_event_status` gains `next_creature_data_id`, end-appended with default `u64::MAX`, so a status
written before the publish has no creature data work left. No reducer argument changes and no
Package builds or reads either table. A publish alone changes nothing a player sees: with no rows
every creature keeps its own template and equipment.

With the updated importer, run `--family events` again; a `creatures` or `gameobjects` import also
writes it. The import writes the table and clears `game_event_status`, so the next minute pass
brings each living creature with a row to its running events' data: about 570 rows in the full
world, within two passes. A rerun writes the same rows and walks the running events' creatures
again, so it casts each start spell again whose aura the creature no longer has. A corpse takes the
data when it respawns.

### Game event mail migration

The private `game_event_mail` and `game_event_started` tables are new and start empty. No existing
column or reducer argument changes, so no Package builds a changed row. The first Game Event pass
after the publish records each running event in `game_event_started` without mail, because
`game_event_mail` is still empty.

With the updated importer, run `--family events` again. A `creatures` or `gameobjects` import also
writes the `events` family. The pinned World Source has no `game_event_mail` rows, so the import
writes none. An imported row for an event that is already running sends nothing until the event's
next start, as vmangos sends nothing for an event that resumes. A rerun writes the same rows and
sends nothing.

### Game event mailing migration

The private `game_event_mailing` table is new and starts empty. No existing column or reducer
argument changes, so no Package builds a changed row. An event already running at the publish has
no mailing, and one that starts afterwards queues its mail. A publish or restart keeps a queued
mailing, which carries on at its cursor on the next minute pass. A mailing copies the values of its
`game_event_mail` row, so an import that replaces the mail rows leaves a queued mailing as it was,
and the next start queues the new rows.

### Game event quest and vendor migration

The private `game_event_quest` and `game_event_vendor_item` tables are new and start empty. No
existing column or reducer argument changes, so no Package builds a changed row. A publish alone
changes nothing a player sees: with no rows every quest is offered and every vendor item sold, as
before.

With the updated importer, run `--family events` again. A `creatures` or `gameobjects` import also
writes the `events` family. The import writes both tables, so event quests and event vendor items
follow their event at once. A rerun writes the same rows.

### Chest money and item loot migration

`game_item_template` gains `lock_id`, `money_min` and `money_max`, and `game_gameobject_template`
gains `money_min` and `money_max`, all end-appended with default 0. The public `game_item_loot`
table and the private `game_loot_money` table are new and start empty. Shared import SQL defaults
fill older inserts.

Between the publish and the reimport, `item_flags` already carries `ITEM_FLAG_LOOTABLE`, but
`game_item_loot` is empty and every `lock_id` is 0. Every clam, lockbox and coin pouch then opens to
an empty Loot Window, with no Lockpicking check, and release destroys it. Reimport the `items` and
`loot` families before players can reach the Realm. Reimport `gameobjects` for chest money; until
then chests give no money.

`game_loot_money` is a Character-owned Transfer table. A blob from an older source lacks it and
imports with no purse rows; an older Shard has opened no item. An older destination refuses the new
manifest, so update both Shards before moving active World Sessions.

### Control aura source migration

An `A_CONTROL` effect's `p1` now keeps its source aura id, and the Module reads it to release a
Character's Loot Window on a Polymorph, Gouge, charm or possess. No schema changes. Reimport
`spell`, then call `debug_repair_after_publish` to copy the new `p1` to live control Auras. Until
the reimport, every control effect keeps `p1` 0: a stun or fear still releases the window, and a
confuse, charm, possess or Gouge leaves it open, as before.

### Corpse AOI migration

The native runtime indexes `game_corpse` by map, Instance and `cell`. The end-appended `cell`
column defaults to zero, which is also a valid cell. After you publish this schema to an existing
Shard, call `debug_backfill_cell_ids` as the Operator before you open the native listeners. It
restamps corpse positions as well as the other spatial tables. New corpses stamp their cell on
insertion; corpse decay and Instance teardown use their time and Instance indexes.

### Creature call for help migration

`game_creature_template` gains the END-appended column `call_for_help_range` with default 5.0,
vmangos's in-code default, so every existing template keeps its Call for Assistance. A range of 0
turns the Call for Assistance and the Call for Help off for that template. From the publish on, a
creature that fights more than 10 yards from its respawn point calls friendly creatures within 5
yards for help, until a reimport of the `creatures` family fills each template's own range from the
World Source. A live creature reads its template's range on each call and keeps no copy, so it
needs no repair, respawn or `debug_repair_after_publish` step. Package Deltas may claim the column;
one that omits it keeps the default. The Operator Content API does not carry it, so a created
template has range 5.0 and an update keeps the template's range. Direct SQL INSERTs may omit it;
shared import SQL defaults fill older inserts.

### Creature school immunity migration

`game_creature_template` gains the END-appended column `school_immune_mask` with default 0, which
means no template immunity, so every existing template is unchanged until a reimport. Reimport the
`creatures` family to fill it from the World Source; then a creature whose vmangos template names a
school takes no white swing or damage shield damage of that school, and a creature immune to
physical damage is a low-priority victim. A live creature reads its template's mask on each check
and keeps no copy, so it needs no repair, respawn or `debug_repair_after_publish` step. Package
Deltas may claim the column; one that omits it keeps the default. The Operator Content API does not
carry it, so a created template has no school immunity and an update keeps the template's mask.
Direct SQL INSERTs may omit it; shared import SQL defaults fill older inserts.

### Creature template equipment migration

The public `game_creature_equipment` table is new and starts empty. The public
`game_encounter_equip` gains the END-appended columns `main_hand_item`, `off_hand_item` and
`ranged_item` with default 0; a row written before the publish keeps its displays and sends no
item class, subclass, inventory type or sheath. No reducer argument changes. Packages read
`game_encounter_equip` displays, which keep their meaning, and neither build nor claim the new
table. A publish alone changes nothing a player sees: with no equipment rows every creature shows
what it showed before.

Reimport the `creatures` family to fill the table: 5,182 choices for 5,140 templates in the full
world of the pinned World Source. The Source Profile now also approves `creature_equip_template` and
`game_event_creature_data`, which the `events` family already read. A creature takes its template
equipment as it enters the world; the reimport despawns every creature and the respawn pass brings
each back armed. Nothing arms a living creature in place. A creature's CREATE now carries its
virtual items, so a viewer no longer receives them as a separate values update after the CREATE.

### Drunk value migration

`game_character` gains the END-appended column `drunk_value` with default 0, so every Character
starts sober. The private `game_drunk_sobering` table is new and starts empty. Spell effect 100 now
imports as `E_INEBRIATE` (code 0x2C); older spell rows keep it as `E_SCRIPTED`, so alcohol makes
no one drunk until a reimport of the `spell` family. No repair is needed.

`game_character` crosses in the Transfer blob. A Character row from an older source ends before
`drunk_value` and arrives sober. An older destination refuses the longer row, so update both Shards
before moving active World Sessions. `game_drunk_sobering` is a Character-owned table that is not
transported. A blob from an older source lacks it and imports with no rows; login on the
destination starts a new timer.

### Duel end migration

`game_duel.countdown_deadline_micros` now keeps the countdown's end once the Duel is active, as the
Duel's start; it used to reset to 0. No schema change, repair or reimport is needed. A Duel active
at the publish still holds 0, so it counts every negative Aura the opponent cast on a duelist as cast
after the start, and ending it removes them all. A Duel that ends before it starts removes none. A
duelist who teleports to another map or Instance after accepting now loses the Duel instead of
interrupting it.

### Dungeon run holder migration

`game_instance` gains the END-appended column `holder_guid` with default 0. 0 means that no
handover gave the run a holder, so every existing lease keeps its routing: a Group's run belongs
to its Group, and a solo run routes the Characters bound to it. A run gets a holder only when a
Group disbands around its last member or a leader change gives the old leader the Group's run. No
repair, respawn or `debug_repair_after_publish` step is needed. `game_instance` does not cross in
the Transfer blob. Code that inserts a `GameInstance`, such as a Package fixture, must set the
field; Packages pin a Core revision that has it.

### Enemy chain migration

No schema changes. The importer now writes vmangos target 6 with an `EffectChainTarget` above 1 as
`T_CHAIN_ENEMY` (6), so Chain Lightning, Multi-Shot and the other source chains jump to more
enemies. The melee chains Druid Swipe, Holy Cleave 18819, Strong Cleave 8255 and Sundering Cleave
25174 are cast Spells, not queued strikes, so they also jump. Until you reimport `spell`, they keep
`T_TARGET_ENEMY` and hit the selected unit alone. Reimport `spell` from this revision before play
resumes. A queued strike, such as Cleave 845 and most creature Cleaves, still imports as
`T_TARGET_ENEMY` with its chain count, which it has had since earlier imports, so its swing reaches
its chain units at publish without the reimport. A Package Delta emitted from an older Base Snapshot
writes these effects with target 1; emit it again from a Base Snapshot of this revision.

These changes apply at publish, without the reimport. A cast's SMSG_SPELL_GO lists every unit its
effects reached, each as hit or with its miss, instead of the selected unit alone. Each unit other
than the selected one gets its own damage and heal log, so an instant area spell now logs the damage
it deals every unit. A queued strike's GO lists its victim and every chain unit, and each chain
unit gets its own hit roll, damage log, threat and procs.

### Equipment display migration

`game_character` gains the END-appended column `hide_equipment_flags` with default 0, and
`game_corpse` gains `guild_id` (default 0), `hide_equipment_flags` (default 0) and `worn_items`
(default none). Every Character starts with its helm and cloak shown. A Corpse made before the
publish keeps no guild, no hidden gear and no worn items in its CREATE; a Character that releases
after the publish gets them. A Character Transfer row from a Shard before the publish arrives with
everything shown, and an older destination refuses the longer row, so update both Shards before you
move a Character. No repair is needed. Package Deltas do not read these columns. The reducer
`migrate_character` takes a new last argument, `hide_equipment_flags`, so use the Character Migration
tool from the same revision as the Module; a plan file written before it still reads.

### Group join reset migration

The private `game_group_join_reset` table is new and starts empty. It is a Character-owned table
whose Transfer arm carries one row: the membership revision the source applied to the Character. A
blob from an older source lacks the table; the destination keeps any marker it holds and resets
nothing on that import. An older destination refuses a blob that names the table, so update every
World Shard before moving active World Sessions. A dungeon admission request gains the source's
Group at queue time. An older Instance Shard ignores it, and a request from an older source decodes
without it and keeps the carried binding. No repair or `debug_repair_after_publish` step is needed.

### Health leech migration

A `SPELL_AURA_PERIODIC_LEECH` effect's `p1` now keeps its `EffectMultipleValue` in hundredths, and the
Module reads it to heal the caster on each tick. No schema changes. Reimport `spell`, then call
`debug_repair_after_publish` to copy the new `p1` to live leech Auras (`spell/repair.rs` copies
`effect.p1` onto them). Until the reimport, every leech effect keeps `p1` 0 and ticks as plain periodic
damage that heals nothing, as before.

### Hunter pet presence migration

`game_hunter_pet` gains the END-appended columns `presence`, `display_id`, `saved_health` and
`saved_power`, each with default 0. Every existing row reads as dismissed with no saved model or
vitals. A pet live at the publish stays live; after its owner logs out it does not return at the
next login, and Call Pet brings it back. Its first build stores the model it gets.

Spell effect 109 (Revive Pet) now imports as `E_REVIVE_PET` (code 0x2E). Reimport the `spell`
family straight after the publish. Until then Revive Pet stays a scripted no-op, so a tame replaces
a dead pet instead of being refused; it is refused again once the reimport writes `E_REVIVE_PET`
for spell 982.

`game_hunter_pet` crossed in the Transfer blob under the `game_hunter_pet@presence-1` tag until
the [Hunter pet stable migration](#hunter-pet-stable-migration). An untagged row from an older
source arrives dismissed with no saved model or vitals. An older destination refuses the tag before
it applies any row, so update both Shards before moving Hunters.

### Hunter pet stable migration

`game_hunter_pet` gains the END-appended column `stable_slot`, default 0. Every existing row is a
Current Pet, which is what 0 means, so no repair is needed. The private Character-owned
`game_character_stable` table is new and starts empty, so no Character has bought a stable slot.
Publish through a planned update.

A new tame now takes its `pet_id` from the GUID Range instead of the owner's guid, so a stabled pet
and the next tame never share a number. Existing pets keep the owner's guid.

`game_hunter_pet` now crosses in the Transfer blob under the `game_hunter_pet@stable-slot-1` tag. A
`game_hunter_pet@presence-1` or untagged row from an older source arrives as the Current Pet, and a
blob without `game_character_stable` arrives with no bought slots. An older destination refuses the
new tag and manifest before it applies any row, so update both Shards before moving Hunters.

### DynamicObject AOI migration

The native runtime adds a map, Instance and `cell` index to `game_dynamic_object`. Its new `cell`
column defaults to zero, which is a valid cell. After you publish this schema to an existing Shard,
call `debug_backfill_cell_ids` as the Operator before you open the native listeners. It restamps
DynamicObjects and reconciles their AOI Grants. New ground effects stamp their cell on insertion;
expiry removes the DynamicObject and its Grants in the same transaction.

### Item spell charge migration

`game_item_template` gains `spellcharges_1` to `spellcharges_5`, end-appended with default 0. The
private `game_item_spell_charges` table is new and starts empty, and `CastRefusalReason` gains the
append-only code 32. Shared import SQL defaults fill older inserts.

An item cast now uses up its item only when an expendable spell, one with negative charges, spends
its last charge, and only after the cast's `SMSG_SPELL_GO` and effects. Between the publish and the
reimport every template has zero charges, so no item cast uses up its item: potions, food,
bandages, scrolls and soulstones last for ever. Reimport the `items` family before players can
reach the Realm. A Shard that keeps the fixture items its `init` seeded has Tough Jerky at zero
charges until it is recreated. The Operator Content API and Package Deltas do not carry the
columns, so a created item has no charges, is never used up and an update keeps them.

Trade and buyback keep an item's spent charges. Mail and the Auction House refuse an item with
spent charges, as they refuse an opened item, until their rows can carry the charges.

`game_item_spell_charges` is a Character-owned Transfer table. A blob from an older source lacks it
and imports with no rows; an older Shard has spent no charges. An older destination refuses the new
manifest, so update both Shards before moving active World Sessions.

### Item use requirement migration

`game_item_template` gains `required_spell` and `required_honor_rank`, end-appended with default 0,
and `ItemRefusal` gains `CantEquipRank` and `NoChargesRemain`. Shared import SQL defaults fill older
inserts. Between the publish and the reimport every template has both at zero, so no item asks for
a spell or an honor rank. Reimport the `items` family before players can reach the Realm. The
Module keeps no honor, so every Character holds rank 0 and an item with a required honor rank above
0 is refused until one does. The Operator Content API and Package Deltas do not carry the columns,
so a created item needs neither and an update keeps them.

An item's client charges now come from the template: the item query carries the `spellcharges` of
each slot that has a spell, and an item's create carries the first `ITEM_FIELD_SPELL_CHARGES` word
with the charges it has left. A later word follows in a VALUES update only when it is not zero, and
spending a charge updates the words it changes. An older client session sees the new words after
its next login. The importer sets a `required_spell` that names no imported spell to 0, as vmangos
does.

### Party area Aura migration

A plain Module publish adds the private `game_area_aura_source` and `game_area_aura_copy` tables,
which start empty. Publish through a planned update; development reload requires an unchanged
schema. Then run `debug_repair_after_publish`: it records every party and pet area Aura a unit holds
of its own as a source, so its copies reach the Subgroup at the next Aura pass. That pass drops a
source whose caster is not in the world, and the caster's world entry records it again. Until the
repair, such an Aura stays on its caster alone.

Reimport `spell` to set `CAST_FLAG_PET_AREA_AURA` (0x0100_0000) on the effect 119 spells, such as
Spirit Bond, Soul Link and Master Demonologist. Until that reimport they carry only
`CAST_FLAG_AREA_AURA` and spread to the owner's Subgroup like a party Aura, instead of reaching the
owner alone. [Area Auras](schema.md#area-auras) gives the rule.

### Pet learn migration

Spell effect 36 aimed at the caster's pet (implicit target 5: grimoires and the pet spells of Dash,
Dive and Prowl) imports as `E_SCRIPTED` with target `T_CASTER_PET` (8), a value only the importer
writes. Like vmangos, trainers and the `--trainer` import offer the wrapper and resolve its level,
known state and previous rank from its trigger spell. Trainers read an `E_SCRIPTED` effect with a
trigger spell as a learn effect only when its target is `T_SCRIPTED` (rows imported before
`E_LEARN_SPELL`) or `T_CASTER_PET`; any other `E_SCRIPTED` effect is no longer a learn effect. A
spell row imported before `E_LEARN_SPELL` whose effect 36 had implicit target 1 stored `T_SELF` and
stops teaching until the `spell` reimport. No publish or repair is needed.

No vmangos trainer offers a pet-learn wrapper. A pet trainer offers a wrapper such as 15147 (Growl
Rank 3) whose effect 36 aims at the buyer, so the Character learns 14923, the Beast Training spell
that later teaches the pet; that buy is unchanged. A trainer row that offers a pet-learn wrapper
directly, such as one a Package adds, is refused with `TrainerRefusal::Unavailable` and takes no
money: vmangos casts a wrapper whose SpellVisual is not 222 from the trainer NPC, which has no pet,
so the cast fails. The refusal needs the `spell` reimport: a row imported before #1222 stores the
wrapper as `E_SCRIPTED` with `T_SCRIPTED`, which looks like an ordinary learn row.

### PvP Flag persistence migration

`game_character` gains the END-appended columns `pvp_flagged` and `pvp_desired`, default false. A
Character online at the publish keeps its live PvP Flag and `PVP_DESIRED`, and its next save records
them. A Character offline at the publish logs in unflagged, as before. A Character Transfer row from
a Shard before the publish arrives unflagged, and an older destination refuses the longer row, so
update both Shards before you move a Character. No repair or reimport is needed. Package Deltas do
not read these columns. The `migrate_character` argument gains the fields `pvp_flagged` and
`pvp_desired`, so use the Character Migration tool from the same revision as the Module; a plan file
written before them still reads, as unflagged.

### Queued strike report migration

`game_spell_cast_event` and `game_spell_cast_event_live` gain `blocked_amount` and
`strike_swing_report`, end-appended with default 0. Only a queued strike's GO row writes them; every
other row keeps 0, which means no partial block and no swing report. Nothing reads stored rows, so
no repair or reimport is needed. The Native Host's delivery layouts do not change. An SDK client
that subscribes to `game_spell_cast_event_live` needs regenerated bindings to decode the new
columns.

### Ranged impact report migration

`game_ranged_impact_schedule` gains `resisted`, `blocked` and `absorbed`, end-appended with default
0. A shot launched before the update lands with all three at 0, which means no partial block,
resist report or absorb proc.
The table is transient and nothing stores it, so no repair or reimport is needed. Only a Spell row
whose `damage_class` is 3 (ranged) rolls a partial block for an Auto Shot or a cast ranged Weapon
Strike, and not one with `SPELL_ATTR_NO_ACTIVE_DEFENSE` or `SPELL_ATTR_EX3_COMPLETELY_BLOCKED`, so a
ranged shot with no imported Spell row never blocks.

### Spell attributes migration

`game_spell.attributes_ex` holds the Spell.dbc AttributesEx. It is END-appended with default 0, so
no repair is needed. Publish through a planned update; development reload requires an unchanged
schema.

Until you reimport `spell`, every row has 0 and no immunity purges what it covers ([immunity
purge](schema.md#immunity-purge)): Will of the Forsaken leaves a fear on. Reimport `spell` from this
revision before play resumes. A Package Delta written before this revision has no `attributes_ex`, so
its new spells carry no AttributesEx bits. Emit the Package again from a Base Snapshot of this
revision, or set the field, before you apply it.

### Spell hit table migration

No schema changes. The importer sets the new `CAST_FLAG_LEVEL_SKILL` (0x0040_0000) on a melee or
ranged damage-class Spell that is neither combat range nor a weapon spell, such as Hammer of Wrath
or Blind. Such a Spell now rolls the Attack Table, and the flag makes it roll with the maximum skill
for the caster's level. Until you reimport the `spell` family, older rows have no flag, so these
Spells roll with the caster's weapon skill: a Paladin with no ranged weapon misses Hammer of Wrath
60% of the time. No repair is needed.

### Spell impact block migration

`game_pending_spell_impact`, `game_spell_impact_event` and `game_spell_impact_event_live` gain
`blocked`, end-appended with default 0. A travelling spell launched before the update lands with 0,
which means no partial block. The pending table is transient and nothing reads stored event rows,
so no repair or reimport is needed. Only a Spell row whose `damage_class` is 2 (melee) or 3
(ranged) rolls the partial block of an `E_DAMAGE` or `E_FINISHER_DAMAGE` effect, and not one with
`SPELL_ATTR_NO_ACTIVE_DEFENSE` or `SPELL_ATTR_EX3_COMPLETELY_BLOCKED`. The Native Host's delivery
layouts do not change. An SDK client that subscribes to `game_spell_impact_event_live` needs
regenerated bindings to decode the new column.

### Spell impact effects migration

`game_pending_spell_impact` gains `carries_damage` (default true), `effect_mask`, `level`,
`cast_target_guid` and `effect_0_points` to `effect_2_points` (default 0), END-appended. A travelling
spell launched before the update lands its damage alone, as before; its other effects already
applied in its cast. The table is private and transient, so no repair or reimport is needed, and the
Native Host's delivery layouts do not change. Publish through a planned update; development reload
requires an unchanged schema.

From this revision every effect of a travelling spell other than a Weapon Strike, Judgement or
finisher strike lands at impact ([spell travel](schema.md#spell-travel)), so Frostbolt's slow
arrives with its damage.

### Spell modifier charge migration

`game_pending_cast` gains `began_at_micros`, END-appended with default 0. A cast in flight at the
update completes with 0, so it spends no charged cast-time modifier, such as Nature's Grace, and
keeps it for the next cast. The table is private and transient, so no repair or reimport is needed,
and the Native Host's delivery layouts do not change. Publish through a planned update; development
reload requires an unchanged schema.

From this revision a cast pays its cost after its `SPELLMOD_COST` modifiers, so cost talents such as
Improved Heroic Strike and Frost Channeling lower what the server charges, as the client already
shows. A charged spell modifier, such as Clearcasting or Presence of Mind, is spent by the next
spell it changes.

### Spell power refund migration

No schema changes. The importer sets the new `CAST_FLAG_DISCOUNT_POWER_ON_MISS` (0x0080_0000) on a
Spell with `SPELL_ATTR_EX_DISCOUNT_POWER_ON_MISS`, such as Sinister Strike, Kick, Heroic Strike or
Mortal Strike. Such a Spell gives back 82% of its energy or rage cost when its target avoids it.
Until you reimport the `spell` family, older rows have no flag and keep the whole cost. No repair is
needed. A Package Delta emitted before this revision carries its new spells without the flag; emit
it again with this importer before you apply it.

The full block of a Spell with `SPELL_ATTR_EX3_COMPLETELY_BLOCKED` reads the stored
`attributes_ex_3`, so it needs no reimport.

### Spell travel migration

`game_spell.speed_yps` holds the source `speed`. It is END-appended with default 0, which means no
travel, so no repair is needed. Publish through a planned update; development reload requires an
unchanged schema.

Travel time now comes only from this column ([spell travel](schema.md#spell-travel)). At publish,
a melee ability with a range, such as Bloodthirst or Execute, stops travelling and lands in its
cast. Until you reimport `spell`, every row has speed 0: a travelling damage spell such as Fireball
also lands in its cast, and an Auto Shot, wand Shoot or cast ranged Weapon Strike such as Throw
lands in the next scheduler batch with no flight. Reimport `spell` from this revision before play
resumes.

A Package Delta written before this revision has no `speed_yps`, so its new spells do not travel.
Emit the Package again from a Base Snapshot of this revision, or set the field, before you apply it.

### Summoning ritual migration

The imported `game_gameobject_ritual` table and the private `game_summoning_ritual`,
`game_summoning_ritual_participant`, `game_summoning_ritual_schedule` and `game_summon_offer` tables
are new and start empty. `CastRefusalReason` gains the append-only code 33. Spell.dbc effects 50
and 85 import as the new `E_SUMMON_OBJECT` and `E_SUMMON_PLAYER` kinds; older spell rows keep them
as `E_SCRIPTED`. Ritual of Summoning places no portal until you reimport both the `spell` family and
the `gameobjects` family, which fills `game_gameobject_ritual`. No repair is needed.

### Combo finisher migration

`game_spell.max_duration_ms` and `game_spell_effect.points_per_combo` default to zero, and older
spell rows mark only Slice and Dice with `CAST_FLAG_COMBO_FINISHER`. Until spells are reimported,
the other finishers skip the combo-point Gate and keep their points, no finisher scales with combo
points, and Slice and Dice lasts its 6-second base. After you publish this schema to an existing
Shard, reimport spells with `--dbc` before play resumes.

### Self-cast refusal migration

Older spell rows lack `CAST_FLAG_REFUSES_SELF_CAST`. Until spells are reimported, a Character can
start a negative spell on itself, such as Fireball with itself selected, where vmangos refuses it with
`BAD_TARGETS`. After you publish this revision to an existing Shard, reimport the `spell` family before
play resumes.

### Chance on Hit migration

`game_item_template.spellppmrate_1` through `spellppmrate_5` default to zero, and older spell rows
lack `CAST_FLAG_WEAPON_PROCS`. Until both are reimported, every Chance on Hit rolls its spell's
proc chance, which is one proc per minute for most weapons, and only white swings roll it. After you
publish this schema to an existing Shard, reimport the `items` and `spell` families before play
resumes.

Enchantment Chance on Hit needs the combat-spell Stat Kind on `game_item_enchantment` rows and the
`game_enchant_proc_rate` table. Crusader, Fiery Weapon and the other combat-spell enchantments
never roll until you reimport the `items` family with `--dbc`. Fiery Weapon, Lifestealing and Icy
Chill roll at the wrong rate until you also reimport the `spellmeta` family.

Extra attacks need effect 19 imported as `E_ADD_EXTRA_ATTACKS`. Older spell rows keep it as
`E_SCRIPTED`, so Hand of Justice, Ironfoe and Sword Specialization grant nothing until you reimport
the `spell` family. The new `game_extra_attack` table starts empty and needs no repair.

### Passive creature combat migration

A plain Module publish adds the private `game_creature_combat_grid` table. It starts empty; the
next melee tick arms a grid check for existing threat with no creature victim. No SQL repair is
needed. Publish through a planned update; development reload requires an unchanged schema.

Passive creatures retain threat and combat without a victim. Their ordinary melee leash does not
expire this threat. The grid check uses the pinned Reference's default geometry, visibility distances
(100 yards outdoors, 170 in dungeons, 533 in battlegrounds) and 30-second activity interval. A grid
with no nearby Character or active creature enters evade and clears that combat. LyraCore keeps the
creature object loaded; this does not implement grid unloading or change vendor stock lifetime.

### Equip spell migration

The seven END-appended source fields on `game_spell` and five category overrides on
`game_item_template` default to zero. Reimport `spell` and `items` before play resumes so equip
effects retain their forms, visibility, duration and category cooldown rules.

Reimport `spell` after the passive visibility update to retain party and pet area-aura origins in
`game_spell.cast_flags`. The translated target alone cannot recover that distinction. After the
import, `debug_repair_after_publish` applies the current visibility rule to stored cast auras;
ordinary passive rows move to hidden slot 255 without losing their effects.

`game_spell_effect` END-appends `equip_supported` with a false default. Until the `spell`
reimport, equipment effects remain inert. That reimport also separates melee and ranged AP,
healing and spell crit, and marks source percentages for conversion to basis points after their
amount is rolled. Healing retains its school mask in `A_MOD_HEALING` for the existing flat
healing calculation. Flat school damage and damage shields no longer import as damage percentages
or absorbs. The complete admitted/inert type list and Item counts are in
[the equip aura support audit](schema.md#equip-aura-support).

The skill update needs a `spell` reimport to translate source auras 30 and 98 to `A_MOD_SKILL`.
After import, publish repair rebuilds stale equipment Auras and repairs existing cast and passive
Auras from their current definitions. It also repairs level-dependent skill caps. No skill table
columns change, and no earned current value is reset. A stored value of 5 cannot distinguish old
initialization from earned progress, so existing Characters keep it. A newly learned skill
starts at 1. Login and repair also add the rows of combat skills an existing Character knows
through a spell but never stored, such as starting Daggers, Bows or Wands, or learns from a
starting spell at login. Those rows start at the level cap, the value a missing row used to give.
Login applies the same rules and reconstructs both client bonus halves.

Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` and `items` with the updated importer.
3. Call `debug_repair_after_publish` after the imports.

A repair before reimport is safe; call it again after the imports. Each derived equip aura retains
its source definition locally. Repair keeps unchanged effects and rebuilds stale definitions,
including changed units, visibility and eligibility. It does not restart equip cooldowns or retain
duplicate effects. Changed finite equip effects restart their source duration during this explicit
repair. Unchanged aura identity and deadlines are preserved. Connected Characters and ghosts
receive the rebuilt state.

Repair also compares every ordinary cast Aura's frozen effect kind, parameters, units and cadence
with its imported definition, including offline and permanent rows. It removes missing definitions,
refreshes obsolete metadata, converts percentage units once and clears obsolete absorb capacity.
Aura writes notify connected clients and refresh their sheets. Existing cast magnitudes, stacks,
application times and remaining durations stay frozen, apart from the unit conversion. A legacy
Seal of Righteousness judgement marker restores a missing damage effect at its saved cast level
and deadline. School-modifier group repair keeps the source's stronger elixir or the newest
exclusive spell and removes conflicting saved holders without refreshing their deadlines. Buff and
debuff slots follow the current definition. Missing learned passives are restored without stacking
existing effects. No manual Aura cleanup or recast is required. Repeating repair is idempotent.
Unsupported cast modifiers retain ordinary Aura holders with inert numeric effects; this includes
the source types listed as inert in the equip audit. Flat school damage,
incoming school damage, incoming healing and school crit are active after the spell-power update.
Reimport `spell` to fill the six END-appended source equipment, attribute and damage-class fields
and derive missing coefficients. Equipment class defaults to -1 (unrestricted); the other new
columns default to zero. Reimport `items` to populate the zero-default END column
`game_item_template.damage_school` from `dmg_type1`. The zero-default END column
`game_ranged_impact_schedule.school` freezes the damage school at launch. Run publish repair
after reimport to refresh both cast and equip auras. The zero-default END column
`game_ground_area.effect_id` identifies each new ground effect's incoming coefficient. Older Package Deltas default to inert equip
effects; an author must supply a supported claim.

`game_world_entity` END-appends `sheet_ap_modifiers` and `sheet_ranged_ap_modifiers` with
zero defaults. These derived projections pack the positive signed-16 modifier in the low half
and the negative signed-16 modifier in the high half. Ranged base AP no longer includes these
modifiers. The final publish repair recomputes both projections for connected Characters; world
entry recomputes them for returning Characters. Neither column is Character-owned Transfer state.
Two further zero-default END columns, `sheet_ranged_dmg_min_fraction` and
`sheet_ranged_dmg_max_fraction`, retain fractional ranged damage for the wire. They leave the
existing integer columns intact. Publish repair recomputes them for connected Characters; world
entry recomputes them on login. Both fields are derived and do not cross Character Transfer.
The zero-default END field `sheet_ranged_crit_bp` keeps the ranged weapon's skill contribution
separate from melee crit. Publish repair and world entry refresh it; weapon skill writes refresh
the affected melee or ranged projection. It is also derived and does not cross Character Transfer.
The zero-default END fields `sheet_ap_multiplier` and `sheet_ranged_ap_multiplier` cache
UNIT_FIELD_ATTACK_POWER_MULTIPLIER and its ranged twin, the percent Auras' product minus 1. Publish
repair and world entry refresh them; they are derived and do not cross Character Transfer.
The spell importer now maps auras 166 and 167 (attack power percent, melee and ranged) to
`A_MOD_COMBAT` on the new `COMBAT_ATTACK_POWER_PCT` and `COMBAT_RANGED_ATTACK_POWER_PCT` fields.
Run `spell --dbc` again after publish so imported Spells carry them.
`sheet_ap_base` and `sheet_ranged_ap` keep their `u32` type and hold the bits of the signed base,
so SQL shows a negative base as a large number and the wire reads it as `i32`. Rows saved before
this change hold non-negative bases and need no migration.

`game_item_aura` and its finite-expiry schedule `game_item_aura_expiry` start empty.
`debug_repair_after_publish` reconciles equipment and refreshes the sheet of every online Character
without starting an equip cooldown, including passive effects on ghosts. Fresh login, reconnect,
resurrection and form changes also reconcile equipment. World entry restores passive effects
for a ghost. World exit removes derived equip Auras;
Transfer excludes them and world entry rebuilds them once. Ordinary Auras keep their published
row layout and Transfer codec.

Transfer now carries `game_spell_cd`, including an active equip cooldown. Old blobs that lack
this table import with no spell cooldown rows. An older destination refuses the new manifest.
Update both Shards before an active World Session crosses them. Keep a refused source Transfer
until both ends can import it. The Native Host delivery layouts are unchanged.

The category and category deadline on existing spell cooldown rows default to zero; their spell
deadlines stay unchanged. A pending cast's new category override defaults to `None`, which uses
the spell's category. New Item casts retain their Item category through completion. World entry
sends remaining spell and category cooldowns in `SMSG_INITIAL_SPELLS`.

### Damage shield migration

Reimport `spell`, then call `debug_repair_after_publish` before play resumes. Both cast and equip
Auras use the corrected shield kind. Repair clears obsolete absorb capacity and retains cast
amounts and application times. Repeated repair preserves unchanged equip effects.

### Regeneration migration

Reimport `spell`, then call `debug_repair_after_publish` before play resumes. Both cast and equip
Auras use the corrected MP5 and HP5 kinds. Repair stops obsolete periodic energize and heal ticks,
repairs Evocation's former 15% magnitude to the source 1500% spirit multiplier, and moves Anger
Management to 3/5 of its rage every three seconds. The remaining cast amounts and application times
stay frozen. Repeated repair preserves unchanged equip effects and does not add a second
regeneration contribution.

`game_character_regeneration` is a new empty private table. It needs no backfill. Transfer carries
its fractional health remainder and next tick deadline; older Transfer blobs start with neither.
Existing Characters use the two-second consumer without a recast or manual Aura cleanup. Rage decay
and energy keep their existing consumers.

### Temporary enchantment migration

Poisons, sharpening stones, weapon oils and shaman weapon spells need effect 54 imported as
`E_ENCHANT_ITEM_TEMPORARY`. Older spell rows keep it as `E_SCRIPTED`, so these items and spells
put nothing on the weapon until you reimport the `spell` family. Poisons have no charges until you
also reimport the `spellmeta` family, and a poison proc needs the combat-spell enchantment rows from
an `items` reimport with `--dbc`. The new `game_temporary_enchantment` table starts empty and needs
no repair.

### White melee catalogue migration

`game_crit_per_agility` and `game_creature_defense` start empty after a publish. Reimport the
`globals` and `creatures` families before play resumes; `debug_repair_after_publish` seeds warrior
levels 1 and 60 only on a Shard that never imported `globals`. Other class/level combinations need imported crit rates;
creatures need their no-parry, no-block and no-defense flags. Missing crit rates use the previous
crit calculation in combat and on the sheet, with one warning per Shard. This fallback does not
replace the globals reimport. Existing Character sheet values are refreshed on world entry,
equipment changes and weapon skill changes.

### Starting stack migration

`game_start_item.count` defaults to one. Until you reimport the `globals` family, a new Character
starts with one of each item: one throwing knife, one arrow or bullet, one piece of food. After you
publish this schema to an existing Shard, reimport `globals` before new Characters are created.

### Warrior stance migration

Warrior stances are now Forms: `A_SHAPESHIFT` Auras built from the new public `game_form` and
`game_form_spell` tables. The new public `game_spell_form` table holds the Forms each spell requires
or excludes. All three start empty. No `game_aura` column, Transfer codec or Native Host layout
changes. Stance and passive Auras travel with the other Auras; an older destination Module keeps an
`A_SHAPESHIFT` Aura as an inert Aura.

While `game_form` is empty, warriors behave as before Forms. A warrior enters the world with the
Battle Stance form byte and switches stances through the old `E_SET_STANCE` spell rows. No stance
passive applies, so Defensive Stance has no damage or threat change, and logout returns a warrior
to Battle Stance. Until `debug_repair_after_publish` runs, `game_spell_form` is empty too, so no
spell needs a stance.

Before play resumes:

1. Publish the Module through a planned additive update.
2. Call `debug_repair_after_publish`. It fills `game_spell_form` from each spell's `form_mask` and
   `form_exclude` where the spell has no rows.
3. Reimport `spell` with `--dbc`. It writes Forms 17 to 19 from SpellShapeshiftForm.dbc, the
   spells' Form Sets, the three stances as `A_SHAPESHIFT`, Tactical Mastery as
   `A_FORM_POWER_RETAINED`, `CAST_FLAG_NO_AURA_CANCEL` on spells the client shows no aura icon
   for, and `CAST_FLAG_LOCKS_FORM` on Nefarian's Berserk.
4. Call `debug_repair_after_publish` again. Each online warrior with no stance Aura gets the stance
   its client form byte shows, or Battle Stance, with that stance's passives. A druid form byte is
   left alone.

Offline warriors enter the world in Battle Stance unless they hold a stance Aura.

### Druid form migration

Bear, Dire Bear and Cat Form are now Forms 5, 8 and 1. `game_form` gains four END-appended columns
with defaults: `natural_attack_ms`, `attack_power_from_agility`, `clears_snares` and
`entry_chance_spell`. A Package Delta written before this revision omits them, so its Forms keep the
weapon combat profile, clear no snares and cast nothing on entry. The new Effect Kind
`A_FORM_ENTRY_CHANCE` (`0xD7`) carries Furor. `WorldEntity.stance` stays a column, but nothing reads
it, and the Module ignores the reserved `E_SET_STANCE`.

Until you reimport `spell` with `--dbc`, the druid form spells keep their old `E_SET_STANCE` rows,
so casting them enters nothing: no druid can enter a feral form, and a druid shown in one fights
with its weapon. The cast still spends its mana and starts the global cooldown. Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` with `--dbc`. It writes Forms 1, 5 and 8 with their combat profiles, snare
   clearing, Furor spells and passives (Cat 3025, Bear 1178 and 21178, Dire Bear 9635 and 21178),
   the three form spells as `A_SHAPESHIFT`, and Furor as `A_FORM_ENTRY_CHANCE` with the Form Set
   Cat, Bear and Dire Bear. Forms 17 to 19 get the new columns at their defaults.
3. Call `debug_repair_after_publish`. Each online druid whose client form byte shows 1, 5 or 8 and
   who holds no shapeshift Aura gets that Form's Aura and passives. Its mana goes into the Reserve
   Pool and the Form's pool starts empty. A dead or ghost druid gets no Form. The same repair's
   `repair_cast_auras` rewrites every held Furor Aura, online or not, from its old kind to
   `A_FORM_ENTRY_CHANCE`.

Druid talent passives whose Form Set names the feral Forms, such as Sharpened Claws and Feral
Instinct, now come and go with those Forms. Before, they held in every form. Players see the stat
change on the character sheet when they leave a feral Form.

A druid whose Auras change between the reimport and the repair is projected first and drops to
caster form, as does a druid in a form after a crash. Offline druids enter the world in caster form,
as they did before: an `E_SET_STANCE` form never survived logout.

### Remaining form migration

Travel Form, Aquatic Form, Ghost Wolf, Shadowform, Stealth and Moonkin Form are now Forms 3, 4, 16,
28, 30 and 31. `game_form` gains the END-appended column `blocks_mounting` with default false, and
`power_type` 255 (`lyracore_shared::form::OWN_POOL`) shows the unit's own pool. A Package Delta
written before this revision omits the column, so its Forms block mounting and taxis only when they
act as shapeshifted, as before.

From the publish on, breaking stealth ends every Aura of the stealth spell, not only its
`A_STEALTH` Aura. Until you reimport `spell` with `--dbc`, the six form spells keep their old effect
rows: no unit enters these Forms, Travel Form gives no speed, and Ambush, Garrote, Cheap Shot, Sap,
Pick Pocket and Premeditation stay refused, as they have been since Forms 17 to 19, because their
Form Set requires Form 30. Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` with `--dbc`. It writes Forms 3, 4, 16, 28, 30 and 31 with their passives
   (Travel 5419, Aquatic 5421, Moonkin 24905), the shapeshift effect of every spell naming those
   Forms as `A_SHAPESHIFT` (every Stealth rank, and creature spells such as Arantir's Deception and
   Spirit Shade Visual), and `blocks_mounting` on Forms 1, 3, 4, 5, 8, 16 and 31.
3. Call `debug_repair_after_publish`. First it ends Stealth remnants, online or offline: the Auras
   of a stealth spell whose unit no longer holds that spell's `A_STEALTH` Aura. An older break, on
   logout, a cast or a swing, shed only the `A_STEALTH` Aura, so such a rogue still carries
   Stealth's slow. Without this step the reimported shapeshift row would put that rogue in Form 30
   without stealth. Then `repair_cast_auras` rewrites the shapeshift row of every held Aura of
   these spells, online or not, to `A_SHAPESHIFT`. Each online Character is then projected: a
   stealthed rogue shows form byte 30 and keeps its energy, and a druid in Travel Form gets 5419. A
   druid who held one of these Auras beside a feral Form keeps the newer one. Creatures are not
   projected: a creature already holding Stealth 1785 (Defias Blackguard, Shadowy Assassin, Shadow
   Panther) keeps form byte 0 until it stealths again or respawns. Only its look is affected.

Offline units holding one of these Auras enter the world in its Form. Players see these changes:
Holy spells are refused in Shadowform and healing spells in Moonkin Form, as their Form Sets say;
Travel, Aquatic and Moonkin Form and Ghost Wolf refuse mounts and taxis; the Stealth openers work.

### Reputation flag migration

`game_faction` gains `reputation_flags_1` to `reputation_flags_4`, Faction.dbc `ReputationFlags`,
END-appended with default 0. A Character's default flags come from the same race and class slot as
its base standing. Until a reimport every default is 0: the login faction list marks no faction
visible, hidden or peace forced unless the Character has a row for it, and any faction can be set at
war. Reimport the `dbc` family straight after the publish. Shared import SQL defaults fill older
inserts.

`game_player_reputation` keeps its layout, so Transfer is unchanged and no repair runs. Login sends
a stored row as visible unless its faction is hidden or invisible forced, and at war when its row
says so. A faction at Hostile or Hated, stored or not, is at war, as vmangos `LoadFromDB` does,
unless it is peace forced and the earned standing is above Hated. A row written before the publish
of a faction that defaults to at war, at a rank above Hostile, reads as not at war.

From the publish on, war alone no longer makes an NPC refuse service. Vendors, trainers, taxis,
quest givers, bankers and auctioneers of a reputation faction refuse at Unfriendly or below, and
otherwise serve the Character, at war or not, as vmangos `CanInteractWithNPC` does. A quest giver
hides its mark only at Hostile or below.

### Resistance aura layer migration

Reimport `spell`, then call `debug_repair_after_publish` before play resumes. `A_MOD_RESISTANCE`
effects gain a layer in `p1`: 0 for flat resistance as before, 1 for source aura 83, 2 for
source aura 142 and 3 for source aura 101. Until the reimport every layer is 0: Bear, Dire Bear and
Moonkin Form, Thick Hide, Toughness and Lay on Hands add flat armor instead of multiplying the armor
from items, and Inspiration, Recklessness and Resurrection Sickness add or remove a flat amount
instead of a percent.
Repair copies the layer to stored Auras. No table schema changes.

### Global cooldown migration

`game_spell.gcd_ms` now comes from the source `startRecoveryTime`. An older import set 1500 for
every spell without the passive (0x40) or on-next-swing (0x4) attribute bit. Until you reimport
`spell` with `--dbc`, the old values stay:
- Battle, Defensive and Berserker Stance, and every other spell vmangos gives no global cooldown,
  start one for 1.5 seconds.
- Spells vmangos gives 1 second, such as the rogue Combo Point builders, keep 1.5 seconds.
- The few spells vmangos gives one but the old rule did not, such as Levitate, Water Breathing and
  Detect Traps, start none.

The change is in the importer only, so no Module publish, schema change or repair is needed. Reimport
`spell` from this revision before play resumes.

### Global cooldown category migration

A caster now has one global cooldown per `game_spell.gcd_category`, the source
`startRecoveryCategory`. The column is END-appended with default 0, and the public
`game_global_cooldown` table replaces `game_spell_cooldown`. The old table moves to `retired.rs`;
its rows end within seconds and nothing reads them. The new table starts empty, and no repair is
needed. Publish through a planned update; development reload requires an unchanged schema.

Until you reimport `spell` with `--dbc`, every existing spell is in category 0, so they all share
one global cooldown as before. Stances, Heroic Strike and the other spells without one are still
refused while a global cooldown runs. Reimport `spell` from this revision before play resumes.

A Package Delta written before this revision has no `gcd_category`, so its new spells are in
category 0: their global cooldown neither blocks nor waits for a category 133 spell. Emit the
Package again from a Base Snapshot of this revision, or set `gcd_category`, before you apply it.

### Interrupt flags migration

`game_spell.interrupt_flags`, `channel_interrupt_flags` and `prevention_type` hold the source
`InterruptFlags`, `ChannelInterruptFlags` and `PreventionType`. They are END-appended with defaults
2, 4 and 1, and the private `game_channel_delay` table starts empty, so no repair is needed. Publish
through a planned update; development reload requires an unchanged schema.

Until you reimport `spell`, every existing spell keeps the interrupt behaviour from before the
columns: damage pushes back a cast and leaves a channel running, and an interrupt effect stops every
cast and channel. Damage delays or cancels a channel, and Kick or Counterspell spares a spell vmangos
does not interrupt, only after the reimport. Reimport `spell` from this revision before play
resumes.

Two changes apply at publish, without the reimport. A creature's cast is no longer pushed back. A
ground-area tick and a hit from a spell with SPELL_ATTR_EX3_TREAT_AS_PERIODIC no longer push back a
cast, and the `on_damage_taken` hook reports them with `periodic` set to true.

A Package Delta written before this revision has no interrupt fields, so its new spells take the
defaults. Emit the Package again from a Base Snapshot of this revision, or set the fields, before
you apply it.

### Damage reaction migration

`game_pending_cast` gains the END-appended `cast_time_ms` (default 0), the full cast time that caps a
pushback. A cast in flight at the publish keeps 0, which caps nothing, so a hit can push it back past
its full cast time before it completes. No repair is needed.

Reimport `spell` from this revision straight after the publish, then call
`debug_repair_after_publish`. The reimport makes an `A_FLAG` effect keep its source aura id in `p1`,
and the repair writes that `p1` into each live Aura of the effect. After both steps, live
Concentration Aura, Barkskin and the other `SPELL_AURA_RESIST_PUSHBACK` Auras give their chance to
resist a pushback; until then they give none.

Four changes apply at publish. A cast pushback follows the vmangos steps instead of 500 ms twice. A
hit absorbed whole breaks the Auras that damage breaks and cancels a Hearthstone or mount cast.
Falling, drowning and fatigue damage break those Auras, cancel or push back a cast and end a Duel at
1 health; a Character immune to physical damage takes none of it. The `on_damage_taken` hook now also fires for a hit absorbed whole, for the hit that ends
a Duel, and for environmental damage, which reports attacker 0.

### Area target cap migration

`game_spell.max_affected_targets` holds the source `MaxAffectedTargets`. It is END-appended with
default 0, which means no cap, so no repair is needed. Publish through a planned update; development
reload requires an unchanged schema.

These changes apply at publish, without the reimport. No area spell stops at 20 units: an instant
area effect and a ground area tick reach every unit they select. An instant or chain area spell that
harms picks units by the ground area's rule: it hits a neutral creature without reputation, spares
a reputation faction the Character is not at war with, and an unflagged Character's area spares
another Character outside a Duel. Every harmful area spell, instant or a ground area tick, now
passes over a totem and a unit whose flags make it not attackable or not selectable. A friendly area
(`T_AREA_ALLY`) helps only units friendly to its caster, no longer neutral ones. Target 30 and
helpful targets 7 and 8 import as `T_AREA_NOT_HOSTILE` (11), which keeps helping every unit not
hostile to the caster; until the reimport those spells keep `T_AREA_ALLY` and help only friendly
units.

The importer now writes vmangos party targets 20, 33 and 34 as `T_PARTY_AREA` (9) and raid target
56 as `T_RAID_AREA` (10), which reach only the caster's Group. Until you reimport `spell`, these
spells keep `T_AREA_ALLY` and help every friendly unit in their radius, and a spell that vmangos
limits with MaxAffectedTargets hits every unit in its area. Reimport `spell` from this revision
before play resumes. An Operator Content edit now accepts effect targets up to 11.

A Package Delta written before this revision has no `max_affected_targets`, so its new spells have no
cap. Emit the Package again from a Base Snapshot of this revision, or set the field, before you
apply it.

### Area target from the selected unit migration

No schema changes. The importer now writes vmangos target 37 as `T_TARGET_SUBGROUP` (12), target 45
as `T_CHAIN_ALLY` (13) and target 61 as `T_TARGET_GROUP_CLASS` (14). Until you reimport `spell`,
Prayer of Fortitude, Gift of the Wild, Arcane Brilliance, the Greater Blessings, Chain Heal and
Healing Wave keep `T_TARGET_ALLY` and reach only the selected unit. Reimport `spell` from this
revision before play resumes. An Operator Content edit now accepts effect targets up to 14. A
Package Delta emitted from an older Base Snapshot writes these effects with target 2; emit it again
from a Base Snapshot of this revision.

### Melee reach migration

The new `game_display_combat_reach` table starts empty after a publish, so every unit keeps the
default 1.5 yd combat reach and a 5 yd Melee Reach. Reimport the `creatures` family with `--dbc`
before play resumes; large creatures and Tauren then reach as far as vmangos allows. Without
client data the import stores a display scale of 1, as it does for template scales.

### Reserve Pool migration

`game_world_entity` gains three END-appended columns: `reserve_power_type` (default 255),
`reserve_power` and `reserve_max_power` (default 0). The default 255 is the "no Reserve Pool"
value, not a power type, so every existing row holds none, which is true: before this change no
Form showed a pool other than the unit's own. No repair or reimport is needed, and the Transfer
codec and Native Host layouts are unchanged. A Package Form that shows a pool other than a unit's
own, such as rage for a druid, reserves that unit's own pool on its next entry or world entry.
A Package Form whose `power_type` is not a power type (above 4) is now refused at apply; an
already applied one keeps the unit's pool and logs a warning on entry.

### Aura charge migration

Every Aura now freezes its spell's proc profile and `proc_charges`, not only `A_PROC_TRIGGER` and
`A_PROC_DAMAGE` rows, and adds the caster's `SPELLMOD_CHARGES` modifiers. No schema change and no
reimport are needed: the `spell` family already imports `proc_flags`, `proc_chance` and
`proc_charges` for every spell, and `spellmeta` already imports every `spell_proc_event` row; the
Module now gives a first rank's row to its higher ranks.

An Aura applied before this publish carries no profile, so it shows no charge count and spends
none. `debug_repair_after_publish` re-freezes each Aura that has neither proc flags nor charges
with its spell's full charges. Without the repair, such an Aura stays uncharged until it is
reapplied. The repair refills a live mixed holder once: Holy Shield's block row gets 4 charges,
and the holder spends from the larger count of its rows, so a Holy Shield with charges already
spent goes back to full.

### Creature mechanic immunity migration

`game_creature_template` gains the END-appended column `mechanic_immune_mask` with default 0, which
means no template immunity, so every existing template is unchanged until a reimport. Reimport the
`creatures` family to fill it from the World Source; then bosses and elites refuse the mechanics
their vmangos template names. A live creature reads its template's mask on each check and keeps no
copy, so it needs no repair, respawn or `debug_repair_after_publish` step.
Package Deltas may claim the column; one that omits it keeps the default. The Operator Content API
does not carry it, so an Operator update keeps the template's mask.

### Follow-up Cast migration

`game_spell_follow_up` is a new table and starts empty, so no spell makes a Follow-up Cast after the
publish. Reimport the `spell` family to fill it; then Divine Shield, Divine Protection and Blessing
of Protection apply Forbearance and a second bubble on that unit is refused. Forbearance (25771) is
already imported. No existing row changes, so the publish needs no repair, and a unit that is
already shielded gets no Forbearance until its next bubble.

### Follow-up claim migration

No table changes; `game_spell_follow_up` exists since the Follow-up Cast migration. Reimport the
`spell` family straight after the publish. Until then:

- Power Word: Shield applies no Weakened Soul. The Module no longer reads the old `A_ABSORB` `p1`
  link, and the follow-up rows that replace it come with the import. So rank 1, which the link
  covered, can be recast on any unit that has no live Weakened Soul.
- Bandages still apply Recently Bandaged through the old synthetic trigger effect, and Recently
  Bandaged still refuses the next bandage by its immunity. The item check that refused a bandage
  by aura id is gone.

The reimport drops the synthetic bandage trigger and the `p1` link, and writes follow-up rows for
the 19 First Aid spells and all 11 Power Word: Shield ranks; before, only rank 1 linked Weakened
Soul. Live Recently Bandaged and Weakened Soul Auras need no repair: the imported spells were
already aura 77 immunities with bit 0x100. A Shard without a spell import keeps the bandage rows
its `init` seeded, whose Recently Bandaged no longer refuses a bandage; recreate it to get the new
seed. `debug_repair_after_publish` updates the shield fixture and Weakened Soul seeds.

### Form model migration

`game_form` gains the END-appended columns `alliance_display_id` and `horde_display_id` (default 0)
and `display_scale` (default 1). A Form with model 0 for a unit's team keeps the unit's own model.
A Package Delta written before this revision omits them, so its Forms show no model, as before.

Until you reimport `spell` with `--dbc`, every imported Form keeps model 0, so no unit changes model.
Before play resumes:

1. Publish the Module through a planned additive update.
2. Reimport `spell` with `--dbc`. It writes the models and scales of Cat (892 or 8571, 0.8), Bear and
   Dire Bear (2281 or 2289, 1), Travel (632, 0.8), Aquatic (2428, 0.8), Moonkin (15374 or 15375, 1)
   and Ghost Wolf (4613, 0.8). Stances, Shadowform and Stealth stay at model 0.
3. Call `debug_repair_after_publish`. It projects each online Character, so a druid already in a
   Form takes the Form's model and scale. Offline Characters take it on world entry. Creatures are
   not projected.

From the publish on, `form::project_form` recomputes the unit's model: a transform Aura wins over
the Form's model, the Form's model wins over the unit's own, and a Character on the Horde side
takes the Horde model while a creature takes the Alliance one. Players see a Form's model and scale
change for every viewer, with one VALUES update for the display id and one for the scale.

### Cast item migration

`game_pending_cast` gains the END-appended `cast_item_guid` (default 0), and an Item cast now uses
up its Item when the cast goes off, not when it starts. Zero means no cast item. A timed Item cast
in flight at the publish already used up its Item at start, so zero is right for it: it completes
without using up another, and its cooldown category still marks it as an Item cast. No repair or reimport is needed, and the Native Host layouts are unchanged.

### Taxi team mount migration

`game_taxi_node` gains `own_mount_horde` and `own_mount_alliance`, end-appended with default true.
Reimport the `dbc` family straight after the publish. Until then every node passes the node-status
query's team filter for both teams, including a node with no mount at all, which the old resolved
display check skipped; so a nearer mountless node can be reported as not known. The reimport writes
both flags from each team's own mount entry. No repair is needed.

### Taxi network migration

`game_taxi_node` gains `in_taxi_network`, end-appended with default true. Reimport the `dbc` family
straight after the publish. Until then every node counts as in the taxi network, so a flight
master's lookup can land on a node that only spell paths leave. The taxi menu, the learn step and
the node-status query now share one lookup with no 10-yard limit. A Character who opened a menu
before the change may have learned a node that lookup now skips; no repair is needed, and the
node stays known. The reimport writes the flag from `spell_template` effect 123 and `TaxiPath`.

### Trainer skill requirement migration

`game_trainer_spell` gains `required_skill` and `required_skill_value`, end-appended with default 0.
Reimport the `trainers` family straight after the publish. Until then no offer carries a skill
requirement, so a Character can buy any rank or recipe that its level allows. The primary
profession limit applies at once: it counts the Character's own `game_player_skill` rows, so a
Character who already holds more than two primary professions keeps them and cannot learn another
first rank. No repair is needed.

### Quest availability migration

The private `game_quest_availability` and `game_quest_prerequisite` tables are new. Reimport the
`quests` family straight after the publish. Until then no quest has an exclusive group, a maximum
level or a predecessor beyond its `prev_quest_id`, so each of those quests is offered and accepted
as before. The reimport also clears a `prev_quest_id` that names a breadcrumb or a missing quest,
as vmangos does. A Character already on two quests of one exclusive group keeps both; no repair is
needed.

The next-quest-in-chain check needs no reimport: it reads `next_quest_id`, which existing quests
already hold. From the publish on, a Character who has taken or finished a quest's next quest in
its chain can no longer take that quest, and its giver no longer marks it.

### Quest chain index migration

`game_quest_template` gains a `by_next_quest` index on `next_quest_id`. SpacetimeDB builds it in the
publish; no reimport or repair is needed. From the publish on, a Character on a quest, not failed
or rewarded, whose `next_quest_id` names another quest can no longer take that other quest, and its
giver no longer marks it. A giver also shows the blue repeat mark over a repeatable quest the
Character was rewarded for, unless the quest has flag 0x100.

### Quest item bonding migration

An item whose template has quest bonding (`bonding` 4, `BIND_QUEST_ITEM`) is now soulbound from the
moment a grant stores it, as `Player::StoreItem` does. Bonding 5 binds nothing. No schema change
and no reimport is needed. An item stored before this publish keeps `soulbound = false`, so its
owner can still trade, mail and auction it, and its recipient's copy then binds. A later grant that
merges into its stack binds it. To bind the rest, list the entries with
`SELECT entry FROM game_item_template WHERE bonding = 4` and run
`UPDATE game_item_instance SET soulbound = true WHERE entry = <entry>` for each. Run it only when
the Operator wants those items bound at once; the Module needs no repair to work. The SQL bypasses
the World Session observer, so an online client can show a stale soulbound tooltip until an
inventory change or a relog.

### Quest requirement migration

`game_quest_template` gains `required_skill`, `required_skill_value`, `required_min_rep_faction`,
`required_min_rep_value`, `required_max_rep_faction` and `required_max_rep_value`, end-appended
with default 0. Reimport the `quests` family straight after the publish. Until then no quest has a
skill or reputation requirement, so each such quest is offered and accepted as before. A Character
who already holds a quest it no longer meets keeps it; no repair is needed. Package Deltas written
before the change omit the columns and still apply, with no requirement.

### Quest retake migration

The private `game_character_quest_retake` table is new and starts empty. It needs no backfill: a
rewarded quest row already holds its reward record. A Character who retook a repeatable quest
before the publish lost that record, so its follow-up stays closed until the next turn-in, as
before. From the publish on, a retake keeps the record, and abandoning the retake returns the row
to rewarded.

`game_character_quest_retake` is a Character-owned Transfer table. A blob from an older source lacks
it and imports with no rows. An older destination refuses the new manifest, so update both Shards
before moving active World Sessions.

### Quest share migration

The private `game_quest_share` table is new and starts empty, so no Quest Share survives the
publish. A Character takes a quest from a Group member only through that member's Quest Share, and
a push needs the source shareable bit in `game_quest_flags`. #977 added that table on 2026-11-04,
and the world importer fills it. Sharing works only on a Realm whose quests were imported on
or after that date; a quest with no flags row, such as an authored quest, is not shareable. A
Package quest is shareable when its Package Delta claims a `game_quest_flags` row with the shareable
bit. The claim adds a table to the quest Package Delta family and changes no row layout; a Package
Delta written before it applies unchanged, and its quests stay unshareable. The importer clears and
refills `game_quest_flags` with the quest import, and the Package replay that follows writes the
claims back, so a quests reimport restores the base flags of a quest whose Package claim was
removed.

### Quest reward migration

`game_quest_template` gains `required_money` and `rew_rep_faction_3`, `rew_rep_value_3` through
`rew_rep_faction_5`, `rew_rep_value_5`, end-appended with default 0. Reimport the `quests` family
straight after the publish. Until then no quest has a money cost or a third to fifth reputation
reward, so each such quest turns in as before. A turn-in now grants `reward_xp` scaled by the
Character's level above the quest, so a quest whose `reward_xp` is 0 grants none, where it used to
grant a level-based amount. At level 60 it pays `reward_money_max_level` and no XP, where it used to
pay six copper per XP. A Package Delta that relied on the XP fallback must set `reward_xp`. A quest
with `required_money` is incomplete until the Character holds the money. A quest that sets both
`reward_money` and `required_money` needs the cost first, then pays the cost and the reward. Package
Deltas written before the change omit the new columns and still apply.

### Quest reward spell migration

`game_quest_template` gains `reward_spell` and `reward_spell_cast`, end-appended with default 0, and
`game_quest_reward_spell` moves to `retired.rs`. Spell effect 36 now imports as `E_LEARN_SPELL`
(code 0x2D); older spell rows keep it as `E_SCRIPTED`, which trainers still read as a learn effect.
Reimport the `spell` and `quests` families straight after the publish. Until then no quest shows or
casts a reward spell, so a quest that taught a spell teaches nothing. After the `spell` reimport, a
recipe, tome or manual teaches its spell when it is used. Item use checks the item's class, race,
required skill and required reputation from the publish on. No repair is needed. Package Deltas
written before the change omit the columns and still apply, with no reward spell.

### Repair cost migration

`game_durability_cost` and `game_durability_quality` are new, private tables from
`DurabilityCosts.dbc` and `DurabilityQuality.dbc`. They need no data migration. Reimport the `dbc`
family straight after the publish. Until then both tables are empty, so a repair request repairs
nothing and a damaged item cannot be sold. vmangos skips an item with no cost row, and so does the
Module.

### Taxi multi-hop migration

`game_active_taxi_flight` gains `current_leg_index`, end-appended with default 0. The private
`game_taxi_flight_leg` table is new. A flight already in the air has no leg rows, so it is a
one-leg route on leg 0 and flies as before. No repair is needed.

### PvP Flag migration

`game_pvp_flag_timer` is a new private one-shot schedule, and `CastRefusalReason` gains the
append-only codes 30 and 31. No repair or reimport is needed. A Character online at the publish
carries no PvP Flag until it toggles PvP, moves in a PvP-enforced zone or joins PvP combat. From
then on another Character can harm it only while the flag is set or in a started Duel.

### Spell threat migration

The private `game_spell_threat` table is new and starts empty. Until you reimport the `spellmeta`
family, no spell adds flat table threat and every multiplier is 1, so Sunder Armor adds no threat
and Heroic Strike only its damage. Harmful Auras no longer add `level * 5` threat either way, and a
Paladin's direct heal now draws a quarter threat a point. No repair is needed.

### Native Relay events

Combat, spell cast, spell impact, Speech and emote notifications use SpacetimeDB event tables. Each
adds `_live` to its original table name, such as `game_combat_event_live`. The original
`game_combat_event`, `game_spell_cast_event`, `game_spell_impact_event`, `game_chat_event` and
`game_emote_event` stay ordinary tables with their published schemas, and share each row type with
the event table. The Module writes new notifications only to the event table, and nothing reads
or reaps the original tables.

SpacetimeDB refuses a change to an existing table's event flag. Keep the old declarations and
their indexes for additive schema compatibility. Do not clear a Shard to make this migration apply.

Event tables reach connected subscribers without initial replay, cached rows, update callbacks or
delete callbacks. The Module emits client packets in the transaction that changes gameplay state. A
reconnect restores durable state and receives later notifications. Events emitted during a Transport
Loss are not replayed. Event inserts still enter the commit log, so they do not replace history
retention. See the [event table contract](https://spacetimedb.com/docs/tables/event-tables/).

`game_group_event` and `game_system_message_event` keep their published storage. Group
notifications go directly from the Module; login also reads pending system messages.

Durable tests observe notifications through subscriptions opened before the action. A SQL read
after the action sees an empty event table.

### Loot stack range migration

`game_creature_loot`, `game_pickpocket_loot`, `game_gameobject_loot`, `game_skinning_loot` and
`game_fishing_loot` gain `min_count`, end-appended with default 0. Zero keeps a row's stack fixed
at `count`, so rows written before the change drop as before. Reimport the `items` and `loot`
families after the publish; until then every drop still gives its largest stack. The Operator
Content API loot rows and the Package Delta loot tables carry `min_count` too; a Package Delta
that omits it inserts zero.

### World Session end migration

A split Realm adds the `idc_end_world_session` reducer, the `game_world_session_end` outbox and the
`game_world_session_end_retry` schedule. Both tables start empty. Publish the World Shards before
the Realm Shard. A World Shard on the old Module refuses the new call. A newer connection for an
Account with a World Session on that Shard then waits the full 90-second bound before the Realm
Shard admits it, and the older World Session continues until its next request fails or it closes.
A Realm Shard on the old Module never sends the call, so the World Shards may go first.

The owed end adds the private scheduled `game_world_session_owed_end` table to the Realm Shard. It
starts empty; no reducer argument changes. A Realm Shard on this Module sends an owed end to a World
Shard on an older Module again every 30 seconds until that World Shard is published or the World
Session closes, so publish the World Shards first here too. A `game_world_session_end_retry` row
that the previous Module scheduled at a resend time waits on to its deadline, and the waiting ticket
owes the end again on its next Login Queue pass.

Arrival stamps (#1286) END-append `arrival` to `game_native_away` and `arrival` and `sent` to
`game_world_session_owed_end`, all defaulting to 0, and add the private `game_native_handed_off`
table, which starts empty. No reducer argument changes, so the idc bindings stay as they are. Rows
from before the publish carry arrival 0 and match each other. An owed end from the previous Module
has `sent` 0, so no answer counts for it, and it goes out once more 30 seconds later.

Either publish order is safe. Publish the World Shards first, as for the rest of this section. Until
the second publish, each order leaves one hazard:

- World Shards first: the Realm Shard on the previous Module still ends an owed end on any Ok answer
  from its World Shard, late or not. A World Shard on this Module no longer reports a connection it
  holds nothing of, so a stale away record that names it makes a waiting ticket wait the full
  90-second bound.
- Realm Shard first: World Shards on the previous Module still report a connection they hold
  nothing of, and still report one they handed on by Transfer when the Native Host detaches it. A
  late report can end the owed end and the away record of a World Session that came back to that
  World Shard until it is published.

### Movement height check migration

`game_config` gains `movement_height_checks_enabled`, end-appended with default false. The Flight
and Under-world movement violations stay off until an Operator turns the Gate on after a live
measurement ([vmap rollout](operations/vmap-rollout.md#movement-height-checks)). No repair is
needed.

## 2. Tooling gotchas

- **`spacetime sql` (2.5):** no `ORDER BY`, no `IN` or subqueries, no `Timestamp` or `Identity`
  literal. A range filter on a single column can wrongly return 0 rows; use a histogram or
  `COUNT`. To load a `Timestamp` column, go through a reducer that stamps `ctx.timestamp`.
- **`spacetime sql` INSERT fills columns by table position.** The values go to the table's columns
  in declaration order, whatever order the named list gives, and nothing reports the mismatch. Name
  columns in table order. The Native Host's `host.sql`, the importer and `Standalone::assert_sql`
  put a named INSERT in the published order; see §1 item 2.
- **`spacetime sql` prints whole floats without a decimal point.** A regex that reads a coordinate
  back needs `-?[0-9]+(\.[0-9]+)?`, or a Character parked at exactly `x=-8890.0` fails every
  "did it arrive" poll.
- **`spacetime call` u64 arguments:** shell-quote the decimal argument to keep it intact. Inside
  structured arguments, keep u64 fields as JSON numbers, for example
  `'{"guid":9007199254740993,"ownership":null}'`. JSON strings in those fields are refused. CLI
  2.7.1 forwards numeric argument text unchanged; avoid float-based intermediates when you build it.
  See the pinned [argument
  assembly](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/cli/src/subcommands/call.rs#L111-L130)
  and [HTTP
  body](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/cli/src/api.rs#L69-L76).
- **A bare `spacetime publish` drops the debug tables.** `lyracore publish` builds with
  `--features=debug_reducers`, passes `--yes`, runs preflight first and refuses `-c`. A bare
  `spacetime publish` omits the feature, so it tries to drop the feature-gated tables and aborts
  with "Removed table: game_debug_readout".
- **`#[default(0)]` on a u64 breaks only at deploy.** It encodes four bytes where eight are
  expected ("data too short for u64: Expected 8, given 4"), and no unit test sees it. Write
  `#[default(0u64)]`. `./lyracore preflight` checks every `#[default(...)]` encoding offline, with
  the pinned SpacetimeDB version gate, schema extraction and RLS-filter validation. Run it before
  every publish.
- **`game_package_import` is private.** Read it with `spacetime sql` only, with an equality filter
  on `family`. `lyracore packages apply` is the supported way to change it; never write it by hand.
  `apply` can publish the Module; see [Applying packages](../packages/README.md#applying-packages)
  for when. Its confirmation covers the Realm changes; `--check` prepares locally without Realm
  writes. An empty Package Delta payload is not "unknown". It means "no Package claims this family"
  and clears the Package spell range.
- **`auto_inc` sequences sit behind explicitly numbered imported rows.** A content import writes
  quest, vendor, trainer and creature-loot rows with explicit ids and does not advance the
  sequence. A later `id: 0` insert from a reducer, or SQL `VALUES (0, …)`, then allocates a
  colliding id. Reducers panic with errno-12 and roll back the whole transaction (a seed that
  "does nothing" is usually this), and SQL inserts fail silently. Fixture and scenario rows use
  fixed reserved ids in the decimal 5,090,000–5,099,999 range (`509_0000`–`509_9999` in Rust)
  with a delete first. `SpellEffect.id` is the same class: it is the deterministic key
  `(spell_id << 2) | index`, so never plain-insert one the importer may also write. Use the
  fixtures' `upsert_effect`.
- **In an imported world, low ids are not yours.** Deleting and reinserting faction-template rows
  14 and 1 breaks the imported Monster and Player templates until the next import. Item entries 50
  and 52 are real imported items. Reputation on a real faction imported without a reputation bar
  does nothing. All fixture data lives at reserved ids (509xxxx items and factions, 50900 quests,
  5090x faction templates), and staging helpers do nothing when the real rows exist. Teardown
  assertions compare against a count taken before the test, never against zero.
- **Probe line of sight before you park a test fixture.** Melee swings gate on `has_los`, which
  uses the exact vmap ray (`vmap::los_ray`, WMO only) when `game_config.vmap_enabled` is on, and
  the rasterized nav obstacle grid when it is off. A spot that looks open can sit inside geometry,
  and then the fight stalls with an armed row, `last_swing_ms=0` and no error. Probe the ray that
  will gate it: `spacetime call lyracore -- debug_vmap_ray <map> <x0> <y0> <z0> <x1> <y1> <z1>` on
  a vmap-enabled map (require `los=None`), or `spacetime call lyracore -- debug_nav_leg <map> <x> <y> <x2> <y2>`
  on a grid-only one (require `has_los=true`). A stationary automated client only
  reaches the 5-yard standstill melee range, so put fight fixtures at 4 yards or less unless the
  client walks in. A level-1 Character in an imported world is a valid aggro target for nearby
  spawns, so disposable test Characters default to level 5.
- **gtker vanilla has known wire gaps:** the update-mask wall, the loot-item codec and the
  faction-standing width. If a packet decodes correctly but the real client misbehaves, suspect a
  gap between gtker and the real client before your own logic.
- **A headless protocol client shares gtker's reader.** It confirms that the server sent the right
  packet and nothing more. It cannot reproduce a client crash or lock-up. That class of bug needs
  a real 5875 client and a person watching it.

## 3. Operating SpacetimeDB

Deployment, backup, restart and verification steps are in
[the native operations guide](native-operations.md). This section covers the node underneath.

### Supervise the node

`deploy/systemd/spacetimedb-standalone.service` is the committed unit for a supervised node. It
waits thirty seconds between restart attempts and allows five starts in five minutes. It gives
every start `524288` file descriptors and appends stderr to
`/var/log/lyracore/spacetimedb-standalone.log`. The unit starts the binary at
`/opt/lyracore/spacetimedb/spacetimedb-standalone` and sets no `LYRACORE_NATIVE_*` variables, so it
opens no WoW listeners. A Native Host service needs the Native Host executable and those
variables; use this unit's restart, descriptor and log settings as its base. After you repair a
start-limit failure, run `systemctl reset-failed` on the unit before you start it again.

The service account `lyracore` owns the data and log directories, so the data survives an
executable upgrade and a crash:

```bash
sudo useradd --system --home-dir /var/lib/lyracore --shell /usr/sbin/nologin lyracore  # once
sudo install -d -o lyracore -g lyracore /opt/lyracore/spacetimedb /var/lib/lyracore/spacetimedb /var/log/lyracore
sudo install -o lyracore -g lyracore -m 0755 /path/to/spacetimedb-standalone /opt/lyracore/spacetimedb/spacetimedb-standalone
sudo install -o root -g root -m 0644 deploy/systemd/spacetimedb-standalone.service /etc/systemd/system/spacetimedb-standalone.service
sudo systemctl daemon-reload
sudo systemctl enable --now spacetimedb-standalone
```

`lyracore-ops service reconcile`, from the `lyracore-cli` repository, runs the last three commands,
restarts the unit and reads back `ActiveState`, `LimitNOFILE` and `StandardError` against the
tracked unit. It never creates the service account, the directories or the binary. Its
[Host operations](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/operations.md)
page has the full plan.

Inspect the service and its stderr log with:

```bash
sudo systemctl status spacetimedb-standalone
sudo journalctl -u spacetimedb-standalone --no-pager -n 100
sudo tail -n 100 /var/log/lyracore/spacetimedb-standalone.log
```

After you replace the binary, restart the unit and repeat those checks. Never delete
`/var/lib/lyracore/spacetimedb` during an upgrade: it is the node's database state. The unit binds
to loopback. Do not change its listen address to expose it.

### SpacetimeDB never deletes its own history

SpacetimeDB 2.7.1 keeps every commit-log segment, snapshot and daily module log until someone
deletes them. On a busy Shard this fills the disk: on 2026-09-28 the `lyracore` database held
73 GB of history, and the next deploy had no room to build. The Native Host's own cleanup runs only
during Development reload.

Never delete files under `replicas/` by hand. A restart needs the newest valid snapshot and every
segment after it, and a gap between segments stops the database from starting. Deleting a segment
in the middle, or the newest snapshot, makes the database unrecoverable. Install the pruner, the
disk monitor and the journald cap from the [disk policy](./operations/disk-policy.md) before
another sustained bot run.
