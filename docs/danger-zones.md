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
   - SpacetimeDB SQL INSERT needs every column. When an existing INSERT can omit the new column,
     add its default to `crates/lyracore-shared/src/import_sql_defaults.txt`; the importer, the
     durable tests and the native check fill it from there.
   - Change native reducer arguments and their Host calls together. The Host and Module also share
     the BSATN layouts of `game_native_cipher` and `game_native_packet`. Update
     `native-host/src/feed.rs` with `module/src/native/mod.rs`, then run the native protocol check.
     Both sides' unit tests pin these layouts to the rows in `native-host/src/delivery_rows.rs`.
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
   with ERROR #132.
5. **Don't gate features on client-display readback.** The server is authoritative, so verify
   effects server-side. Flag client visuals for a human to look at.
6. **`#[default(...)]` cannot default a `String` column.** The SpacetimeDB macro type-checks every
   default inside a `const` block, and dropping a `String` in const-evaluated Rust is
   `error[E0493]` on stable. This blocks `#[default(String::new())]` and `#[default("")]` alike.
   Numeric and bool defaults are fine. Put new `String` data in a new table instead: it has no
   existing rows, so no column needs a default. `game_creature_waypoint` and `game_creature_spell`
   use the same one-row-plus-child-rows shape. See `NpcTextSlot` in `module/src/creatures/spawn.rs`.
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
auto-migrating publish, so this reducer re-arms the interval schedules, re-freezes Proc profiles,
restores missing Auction expiries, re-tags legacy auction mail as Auction Mail before the Mail Timer
pass (so a legacy "Auction won" is deleted, not returned), arms a Mail Timer for each Mail that has
none, moves each legacy Character GM level onto its Account and re-seeds the fixtures `init`
seeds. `packages apply` calls it for each Shard it publishes.
A skipped call looks like a mid-session hang, not an error.

The first run on a Shard with Mail from before the Mail Timer applies the vanilla 30-day rule
(3 days with a cash on delivery price) to that Mail. Every Mail already past its life returns to
its sender or is deleted with its copper, at 100 Mail a second. That is a maintainer decision, not
a fault.

### Corpse AOI migration

The native runtime indexes `game_corpse` by map, Instance and `cell`. The end-appended `cell`
column defaults to zero, which is also a valid cell. After you publish this schema to an existing
Shard, call `debug_backfill_cell_ids` as the Operator before you open the native listeners. It
restamps corpse positions as well as the other spatial tables. New corpses stamp their cell on
insertion; corpse decay and Instance teardown use their time and Instance indexes.

### DynamicObject AOI migration

The native runtime adds a map, Instance and `cell` index to `game_dynamic_object`. Its new `cell`
column defaults to zero, which is a valid cell. After you publish this schema to an existing Shard,
call `debug_backfill_cell_ids` as the Operator before you open the native listeners. It restamps
DynamicObjects and reconciles their AOI Grants. New ground effects stamp their cell on insertion;
expiry removes the DynamicObject and its Grants in the same transaction.

### Combo finisher migration

`game_spell.max_duration_ms` and `game_spell_effect.points_per_combo` default to zero, and older
spell rows mark only Slice and Dice with `SPELL_ATTR_COMBO_FINISHER`. Until spells are reimported,
the other finishers skip the combo-point Gate and keep their points, no finisher scales with combo
points, and Slice and Dice lasts its 6-second base. After you publish this schema to an existing
Shard, reimport spells with `--dbc` before play resumes.

### Chance on Hit migration

`game_item_template.spellppmrate_1` through `spellppmrate_5` default to zero, and older spell rows
lack `SPELL_ATTR_WEAPON_PROCS`. Until both are reimported, every Chance on Hit rolls its spell's
proc chance, which is one proc per minute for most weapons, and only white swings roll it. After you
publish this schema to an existing Shard, reimport the `items` and `spell` families before play
resumes.

Enchantment Chance on Hit needs the combat-spell Stat Kind on `game_item_enchantment` rows and the
`game_enchant_proc_rate` table. Crusader, Fiery Weapon and the other combat-spell enchantments
never roll until you reimport the `items` family with `--dbc`. Fiery Weapon, Lifestealing and Icy
Chill roll at the wrong rate until you also reimport the `spellmeta` family.

### Native Relay events

Combat, spell cast, spell impact, Speech and emote notifications use SpacetimeDB event tables. Each
adds `_live` to its original table name, such as `game_combat_event_live`. The original
`game_combat_event`, `game_spell_cast_event`, `game_spell_impact_event`, `game_chat_event` and
`game_emote_event` stay ordinary tables with their published schemas, and share each row type with
the event table. The Module writes new notifications only to the event table. The event reaper
expires rows left in the original tables after an upgrade.

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

## 2. Tooling gotchas

- **`spacetime sql` (2.5):** no `ORDER BY`, no `IN` or subqueries, no `Timestamp` or `Identity`
  literal. A range filter on a single column can wrongly return 0 rows; use a histogram or
  `COUNT`. To load a `Timestamp` column, go through a reducer that stamps `ctx.timestamp`.
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
