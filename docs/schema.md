# Schema — the SpacetimeDB data model

**Status:** current — verified against the tree on 2026-09-03. This document is a *map* of the data
model, not a copy of it. **The code is authoritative**: every table is a Rust `#[table]` struct in
`module/src/**` — or in an extension package compiled into the same module — and where a snippet here
and the code disagree, the code wins.

Start at [`architecture.md`](./architecture.md) for how these tables fit into the system.
[`danger-zones.md`](./danger-zones.md) §1.2 is authoritative for migrations.

> **Historical note.** Until this rewrite, this document described the ten tables of the 2026-06
> vertical slice, and included two tables — `game_opcode_route` and `game_event_smsg_map` — that
> were designed but **never built**. The gateway's opcode routing is a `match` in
> `gateway/src/world/mod.rs`, not a config table. Those two entries have been removed rather than
> corrected.

---

## 1. Conventions

- **Naming.** Core tables take the `game_` prefix; package tables take `pkg_<name>_`. External gtker
  crates keep their `wow_` names and are never renamed.
- **`accessor`** is the Rust identifier the macro exposes as `ctx.db.<accessor>()`. SpacetimeDB 2.x
  syntax: `#[table(accessor = …)]` (1.x's `name =` was renamed), with all indexes declared inside the
  single `#[table(…)]` attribute.
- **Visibility.** `public` = subscribable by a client connection. Omitting it makes the table
  **private** — only the database owner (the gateway's coordinator connection) can read it.
- **RLS.** The module declares no `#[client_visibility_filter]` today; §4 says why, and what a new
  one would still have to satisfy. `module/Cargo.toml` keeps SpacetimeDB's `unstable` feature
  pinned, because the attribute needs it.
- **Migration discipline.** Every table is designed to grow by **END-appending** a column with an
  explicitly typed `#[default(...)]`. Ordering-sensitive data uses explicit sequence or timestamp
  columns, never auto-inc ordering.

### The three migration rules that have actually bitten

1. **A default must be END-appended and explicitly typed.** A bare `#[default(0)]` on a `u64` column
   encodes as a 4-byte i32 and the publish migration rejects it (`data too short for u64`). Write
   `#[default(0u64)]`. `module/src/character.rs` carries several worked examples in its comments.
2. **`#[default(...)]` cannot default a `String`.** The macro type-checks the expression in a const
   block, and dropping a `String` in const-evaluated Rust is `error[E0493]` on stable. This is a Rust
   limitation, not a repo convention. Put new string data in a **new table** instead — the same
   one-row-plus-child-rows shape the codebase already uses (`game_creature_waypoint`,
   `game_npc_text_slot`).
3. **A new table requires regenerating the gateway bindings**, and the generator snake-cases digit
   suffixes, so a handful of columns must be hand-patched back. The exact patch list is in
   `danger-zones.md` §1.2. `gateway/tests/schema_parity.rs` structurally checks every subscribed
   table's binding against the module schema.

And the one that is not about a single table: **a schema change must reach every shard.** A partial
publish presents as an unrelated mid-session hang, not a loud "no such table".

---

## 2. Inventory

**238 tables**, all of them in `module/src/**`: 122 public, 116 private. No table comes from a
package in this tree; `packages/example` is the only in-tree package and it declares none. Recount
rather than trust the numbers below, which drift on every schema change:

```bash
grep -rn '^#\[table(' module/src --include='*.rs' | wc -l   # 238 on 2026-09-03
```

| Domain | Tables | Public | Where |
|---|---:|---:|---|
| Auth / session / identity | 6 | 0 | `auth.rs` |
| Character and per-character progression | 23 | 19 | `character.rs`, `skill.rs`, `reputation.rs`, `talent.rs`, `spell/spellbook.rs`, `action_bar.rs`, `combo.rs`, `rest.rs`, `corpse.rs`, `xp.rs`, `exploration.rs`, `breath.rs`, `breath_relay.rs`, `graveyard.rs` |
| World entity and movement | 8 | 3 | `world.rs`, `motion.rs` |
| Terrain / nav / exact vmap | 9 | 5 | `terrain.rs`, `nav.rs`, `vmap.rs` |
| Chat / social / addon bridge | 9 | 9 | `chat.rs`, `bridge.rs` |
| Combat / threat / duel | 10 | 4 | `combat/engage.rs`, `combat/death.rs`, `threat.rs`, `duel.rs` |
| Spell / aura | 20 | 10 | `spell/tables.rs`, `spell/stacking.rs` |
| Quest | 12 | 8 | `quest.rs` |
| Item / vendor / trade / mail | 11 | 5 | `items/tables.rs`, `trade.rs`, `mail.rs`, `mail_escrow.rs` |
| Auction house | 7 | 2 | `auction.rs` |
| Creature (template, spawn, AI, pet, trainer) | 42 | 17 | `creatures/*`, `trainer.rs` |
| GameObject | 9 | 6 | `gameobject.rs`, `go_model.rs` |
| Loot | 12 | 6 | `loot/*` |
| Group / party | 5 | 3 | `group.rs` |
| Instance / encounter | 7 | 1 | `instance.rs`, `encounter.rs` |
| Sharding: region, transfer, load | 9 | 0 | `region.rs`, `transfer/mod.rs`, `load.rs` |
| Realm-core | 2 | 0 | `realm_core.rs` |
| Gateway leases | 3 | 0 | `gw.rs` |
| Taxi | 5 | 1 | `taxi.rs` |
| Weather | 3 | 1 | `weather.rs` |
| Packages | 3 | 1 | `package_config.rs`, `package_import.rs`, `script_binding.rs` |
| Event GC | 1 | 0 | `gc.rs` |
| Config / static data / diagnostics | 22 | 21 | `config.rs`, `gm.rs`, `faction.rs`, `skilldata.rs`, `stats.rs`, `import_meta.rs`, `debug/*` |

Two shapes recur and are worth naming:

- **`[static]` / catalogue tables** — `game_item_template` (17,720 rows), `game_spell`,
  `game_spell_effect`, `game_creature_template`, `game_quest_*`, `game_faction*`, the three
  `game_taxi_*` tables, and other DBC-derived reference data. Written only by the importer,
  replicated identically to every shard, and checked for skew by the maintainers' cross-shard
  catalogue-parity check. Per-player connections are gone
  (#483) — there is now exactly one connection per database, so a catalogue is read into exactly one
  cache copy by construction. See §5 for why that used to need enforcing.
- **`[event]` tables** — transient outbound signals that cannot be derived from a row diff (a
  movement relay carries the *animation opcode*, which field sync cannot). Inserted inside the
  transaction that caused them and reaped ~1 s later. Delivery is the insert delta; the row itself is
  not a mailbox.

---

## 3. The load-bearing tables

Read the structs; this section gives you the shape and the reason.

### `game_taxi_node` / `game_taxi_path` / `game_taxi_path_node` (`module/src/config.rs`)

Public static catalogue rows imported from the operator's `TaxiNodes.dbc`, `TaxiPath.dbc`, and
`TaxiPathNode.dbc`. A node's `id` is its storage key and `client_node_id` is its unique wire id in the
vanilla client's fixed eight-word (256-bit) taxi mask. Imported nodes use their DBC id for both;
reserved fixtures keep 509xxxx storage ids but use bounded synthetic wire ids. Nodes also retain map
position, name, and the Horde/Alliance mount-display pair. Paths are directed source-to-destination
edges with their fare in copper; the reverse direction is present only when the DBC contains a second
row. Path nodes keep their stable DBC id and explicit `(path_id, node_index)` order, plus map position,
flags, and delay. `flags` remains the DBC's signed `int32` container so every raw bit survives; delay
keeps the source type but the importer rejects negative time. The importer validates references,
wire-id bounds/collisions, and duplicate ordinals before writing, then replaces the family
point-first/path-second/node-last so a successful rerun removes stale geometry.

### Taxi progression, schedule, spline, and service tables (`module/src/taxi.rs`)

`game_character_taxi_node` is a private module table containing durable, character-scoped taxi
progression. Each row records a known catalogue node by its server storage id. Opening a nearby,
living, selectable, friendly flight master discovers its source node idempotently; a status query
only reads this state. The table participates in character deletion and shard transfer, and remints
its local surrogate id at the destination. The gateway does not subscribe or read it.

`game_active_taxi_flight` is private and has one row at most per character because
`character_guid` is its primary key. A successful direct-route activation creates the row in the
same serialized transaction that deducts the imported fare. It records the storage ids for the path
and endpoints, the selected mount display, the paid fare, the starting point cursor, and the start
time. The gateway queues the successful activation reply before an idempotent arm reducer sets the
live mount/flight flag and starts the schedule. Duplicate activation therefore sees
the existing row before it can charge again. Character deletion removes the row. It is explicitly
not transported because supported baseline flights are confined to one open-world shard.

`game_taxi_flight_schedule` drives the 250 ms authoritative route clock. The public
`game_taxi_passenger_spline` row carries the current remaining route to the owner and visibility-
checked AOI observers; it is refreshed whenever the passenger changes AOI cell.

`game_taxi_service_reply` is a private transient request/reply seam between the gateway and the
module. The owner-token coordinator subscribes it; ordinary clients cannot. The gateway sends a
status, open, or activation request with a unique request id, and the module writes an independently
keyed reply, so overlapping requests for one character cannot overwrite each other before
observation. Each observed row is acknowledged and deleted by an operator-gated reducer. Writes
also reap crash
leftovers older than 60 seconds, while never deleting a young unobserved row regardless of overlap.
The module owns NPC, range, reaction, discovery, topology, and direct-route policy. Replies expose
client node ids for the fixed 256-bit wire mask while catalogue paths and discoveries keep storage
ids. Activation replies also carry a stable numeric result which the gateway maps directly to the
closest vanilla `ActivateTaxiReply`; refusal prose remains diagnostic. The mailbox is deleted with
the character but is not transported, so a stale request can never become progression on the
destination shard.

### `game_world_entity` — the live in-world row (`module/src/world.rs:20`)

Public. One row per player *currently in the world* and per spawned creature. Created at login,
deleted at disconnect or on transfer-out. It is both the visibility source and the descriptor source
for `SMSG_UPDATE_OBJECT`.

```rust
#[table(
    accessor = game_world_entity,
    public,
    index(accessor = by_map,   btree(columns = [map_id])),
    index(accessor = by_grid,  btree(columns = [map_id, instance_id, grid_x, grid_y])),
    index(accessor = by_owner, btree(columns = [owner_identity]))
)]
pub struct WorldEntity { /* ~50 columns; see the source */ }
```

Hunter pets keep a separate durable identity in `game_hunter_pet`. A materialized pet is an ordinary
owned `game_world_entity`, classified by `game_live_pet_kind`; summoned demons use the same live
creature behavior but have no Hunter identity or care state. `game_hunter_pet_protocol` is the
bounded owner-facing gateway projection. Taming retires the current wild entity and arms its
authored `game_creature_spawn` for a later respawn, so the spawn point is not transferred into or
deleted with the pet identity.

Column groups: identity/control (`guid`, `owner_identity`, `account_id`), spatial (`map_id`,
`instance_id`, `x/y/z/orientation`, `grid_x`, `grid_y`, `last_move_ms`), the object block
(`type_mask`, `entry`, `scale_x`), the unit block (health/power/level/faction/display/mount/flags/attack
time/dynamic flags), the player block (appearance bytes, flags, xp, money, the five base stats,
armor), the current `target_guid`, and the creature-movement cursor.

`mount_display_id` carries `UNIT_FIELD_MOUNTDISPLAYID` for **both** mounted mechanics, and the module
decides which one owns it:

- **Taxi flight** owns the field for the whole flight. Activation writes the faction mount display and
  sets `UNIT_FLAG_TAXI_FLIGHT` in the existing `unit_flags` word; landing clears both. The gateway
  relays that pair in one OBJECT_FIELD_TYPE-free partial VALUES mask, so self and observers see one
  coherent flight presentation. Route progress is never stored in `game_creature_spline`; it belongs
  to the separate active-flight row.
- **Land mount** owns the field the rest of the time. Here the field is a *projection*, not a state
  machine: the `A_MOUNTED` aura row is the mounted state, and `mount::recompute_mount` re-derives both
  `mount_display_id` and `run_speed_mult_bp` from the target's current aura set. Every aura removal
  path converges on that one recompute, so cancel, expiry, dispel, unlearn and each explicit dismount
  trigger land on the same end state. A land mount never sets `UNIT_FLAG_TAXI_FLIGHT`, and the gateway
  relays it through a standalone single-field VALUES builder. Both mount entry points refuse a player
  in flight, so land-mount cleanup can never clear a taxi presentation.

`run_speed_mult_bp` is the same kind of projection while a land mount is active: it holds the exact
effective multiplier from the shared move-speed fold (16000 for a nominal 60% mount), and the existing
subscription diff turns a change into `SMSG_FORCE_RUN_SPEED_CHANGE`.

Three indexes, each earning its keep:
- `by_grid` is the AOI range scan. Note it is **four** columns — `instance_id` is in the key, because
  an instance is its own spatial partition.
- `by_owner` exists because `entity_by_owner` is the auth prologue of ~77 player reducers and was a
  full table scan per transaction before it.
- `by_map` is the coarse map filter.

**Never `.iter()` this table** (nor `game_creature_spawn`, `game_gameobject`,
`game_dynamic_object`). Use `helpers::entities_near` / `helpers::in_same_partition`. A whole-table
scan on a sharded realm silently returns a subset rather than erroring, and every feature built on
"I can see the whole world" quietly goes wrong. Enforced by
`module/src/tripwires.rs::partition_discipline_tripwire` against a whitelist that only ever shrinks.

Six sibling tables carry the **identical** `(map_id, instance_id, grid_x, grid_y)` key so they can
ride the same AOI box: `game_entity_motion`, `game_creature_spline`, `game_combat_event`,
`game_spell_cast_event`, `game_spell_impact_event`, `game_emote_event`, `game_roll_event`.
`game_gameobject`'s grid key is only three columns (no `instance_id`).

### `game_character` — the durable character (`module/src/character.rs:7`)

Public + RLS. Exists whether or not the player is online; distinct from `game_world_entity` so an
offline character costs no live-world machinery. Carries appearance, position, level/xp, money,
rested-xp pool, hearthstone home, played-time accounting, and the persisted health/power snapshot.
`owner_identity` is `Identity::ZERO` until bound at `establish_session`.

### `game_account` / `game_alpha_test_tools_enrollment` / `game_session` / `game_operator` (`module/src/auth.rs`)

All four are **private**. `game_account` holds SRP6 salt and verifier, never a password. It also
holds the Account's Alpha Test Tools authority. Existing Accounts receive that authority through an
end-appended default. New Accounts copy `game_alpha_test_tools_enrollment`, which starts enabled;
changing enrollment never changes an existing Account.
`game_session` holds the SRP6 session key **K**, which is what makes gateways stateless: the logon
flow writes it, and any world gateway can read it to complete the handshake. `game_operator` is a
singleton holding the one trusted operator identity, captured (not derived) by `claim_operator`.

On a realm-core deployment these live on realm-core, and the world shards' copies are downgraded to
a **write-through cache that is never read for auth**: a world shard's own `game_account`/
`game_session` rows are overwritten on every logon but never consulted for the SRP6 challenge, the
session write, or the handshake's K lookup — realm-core is the only handle those ever use. If
realm-core is configured and unreachable, the gateway fails closed (refuses logons) rather than
falling back to the cache.

The same boundary applies to Alpha Test Tools. For each dot-Say command, the Gateway reads the
current Account value from Realm-core, then conveys it to the Character's Home Shard in one Durable
Request. The Module makes the final Gate with that value and the Character's GM level. No
Character-side authority projection exists.

`auth.rs` also carries a `ensure_shadow_account` path: an instance or continent shard has no real
account row, so a transferred character gets an index-entry row with empty salt and verifier. It can
never satisfy an SRP proof; it exists only to carry the identity binding.

### `game_map_region` / `game_region_assignment` (`module/src/region.rs:44,:67`)

Both private, and **unused since #471** removed the region tier from the gateway (2026-08-08) —
nothing subscribes to or routes on them; they stay in the schema because dropping a table is a
destructive migration. A region is an **inclusive cell rectangle** keyed `(map_id << 32) | region_id`, and
region 0 is reserved for "the rest of the map". An assignment names a **database**, carries a
monotonic `epoch`, and is authoritative on realm-core only. `shard` is the one column in the schema
that a module file outside `region.rs`/`load.rs` may not touch — the build fails if it does.

### `game_transfer_out` / `game_transfer_in` (`module/src/transfer/mod.rs`)

Both private. The two halves of a cross-database character move. While either row exists the
character is **in transit**, and four chokepoints refuse to act on it. The escrow row on disk is the
recovery authority; the transfer id is the character guid, so recovery needs nothing from gateway
RAM.

### Auction listing state (`module/src/auction.rs`)

`game_auction_house` is the public `AuctionHouse.dbc` catalogue used to resolve an auctioneer's
house and economic policy through its imported faction. `game_auction` is the public active market;
its item columns are the complete item-instance snapshot while no inventory row exists, and its
house and rate columns preserve the listing-time policy. Private `game_auction_hold` is the source-shard value fence;
private `game_auction_operation_receipt` makes listing retries idempotent after that Hold is deleted.
When realm-core refuses a held listing, it records a payload-matching `auction_id == 0` receipt and
mails the held item and deposit back to the seller in one transaction. Only then does
`gw_auction_release_listing_hold` delete the source Hold; a Hold with an active receipt is never
released.
Private `game_auction_bid_hold` fences a bidder's complete offer and retains the terminal source
outcome, normalized accepted price, and any purse-overflow refund awaiting relay; private
`game_auction_bid_decision` is realm-core's serialized, replay-safe decision and exact-once
settlement/refund-mail receipt.
`game_auction_expiry` is a private one-shot schedule at the listing's original deadline. These
callbacks return an unbid item or settle a winning bid with exact item and proceeds mail, then no-op
when replayed. These tables are additive and are deliberately excluded from character transfer
manifests; deletion is refused while a character owns Auction value.

### Riding data (`module/src/skill.rs`, `module/src/skilldata.rs`, `module/src/trainer.rs`)

Riding is an ordinary skill line, not a new concept. `game_skill_line` carries line **762** (`Riding`),
and a character's learned rank is a normal `game_player_skill` row on that line. `game_skill_ability`
joins a mount spell to line 762 with the `min_skill` threshold that holds vanilla's 75 (Apprentice) and
150 (Journeyman) tiers, plus the race and class masks that keep one race's riding tradition from
satisfying another's mount. The cast gate walks the character's own learned lines and probes
`by_skill_line`, so it never scans the full ability catalogue. It fails closed: a mount whose skill
data was never imported is uncastable rather than free.

`game_trainer_spell.learn_skill_line` now names three kinds of offering rather than two: a profession,
a weapon line, or riding. `learn_skill_cap` carries the tier the purchase grants. A riding offering's
`spell_id` is a marker with no `Spell.dbc` row, so the gateway confirms the purchase without echoing it
as a learned spell. The trainer NPC declares itself with `trainer_type::MOUNTS`.

### Proc data (`module/src/spell/tables.rs`)

A **Proc** is an aura that fires off a combat event. Its rules live in three places, and the split is
the point: two of them are static data the importer writes, the third is a per-aura snapshot.

`game_spell` carries what `Spell.dbc` knows, verbatim: `proc_flags` (the vanilla `procFlags`
combat-event bitmask — the engine names only the bits it fires and there is no translation table),
`proc_chance` (a whole percent) and `proc_charges` (0 = unlimited).

`game_spell_proc_event` is the classic-db overlay for everything `Spell.dbc` has no column for:
`ppm_rate`, `icd_ms`, `proc_ex` (the normal-hit / critical-hit rule), the `school_mask` /
`family_name` / `family_flags` filter on which spells may trigger the Proc, plus a `proc_flags`
override and a `custom_chance`. Keyed by spell id, module-private, and **absent for most spells** —
an absent row means "the header's values". Loaded by the `spellmeta` dump family, whose clear stops
at the reserved synthetic id floor so the seeded Proc fixtures keep their own overlay rows.

`game_aura` carries the FROZEN profile: `proc_flags`, `proc_chance`, `proc_ppm`, `proc_ex`,
`proc_school_mask`, `proc_family_name`, `proc_family_flags`, `proc_charges`, `proc_icd_ms`,
`proc_ready_micros`. `spell::proc::freeze_profile` folds the header and the overlay together once,
at apply, so the proc pass reads one row and re-joins nothing on the hot path. The overlay wins where
both carry a value; charges only ever come from the header. The last two columns are the mutable
pair — a fire spends a charge and stamps the next cooldown deadline — and everything before them is
re-frozen on a refresh, which also refills charges and deliberately leaves a running cooldown alone.

The overlay is the reason `debug_repair_after_publish` re-freezes proc profiles: an aura that was
already on a unit when the columns landed reads as "never procs", and a permanent self-buff never
refreshes on its own.

### `game_vmap_indoor_cell` (`module/src/vmap.rs`)

A module-private, derived per-cell marker keyed `(generation_id, cell_key)`: this cell of this vmap
generation holds at least one indoor WMO triangle. Written once inside `verify_vmap_generation`, which
already decodes every staged chunk, and deleted with the geometry it came from. It has **no gateway
binding** and is never subscribed; it exists only so the movement heartbeat can ask "am I indoors?"
with one indexed find and pay a ray cast only in a cell that could answer yes.

**A missing row means outdoors.** That is the fail-open contract the whole indoor rule rests on: vmap
disabled, no active generation, no marker row, or no containing WMO group all read as outdoors. A
generation verified before this table existed carries no rows, so indoor behavior stays inactive there
until the generation is verified again or the vmap data is imported again.

---

## 4. Row-level security

**The module declares no `#[client_visibility_filter]` filters.** Recount at any time:

```bash
grep -rn 'client_visibility_filter' module/src --include='*.rs'
```

Only two comment lines match. This document used to list sixteen owner-scoped filters, all of the
shape `SELECT * FROM <table> WHERE <identity column> = :sender`. Commit `7fda35a` (2026-08-11)
deleted every one of them, in the same change that deleted per-player client connections. An RLS
filter only ever applied to a client-identity subscription, and no client-identity subscription is
left: the coordinator connection authenticates as the owner and bypasses RLS by design.

Visibility is therefore entirely gateway-side. The gateway reads every table through the owner token
and decides what each player sees with its own recipient-keyed lookups, audience predicates and
area-of-interest tracker. See [`architecture.md`](./architecture.md) §5.2, §5.3 and §5.4.

Two constraints survive the removal and bind any filter added later.

1. **RLS cannot express spatial scoping.** `:sender` is a static identity, so a filter cannot join on
   the caller's *current* position. Interest management stays gateway-managed.
2. **Offline validation covers identifiers, not shapes.** Step 3 of `lyracore preflight` parses every
   `Filter::Sql` and checks its tables and columns against the generated bindings, and it fails the
   preflight on an unknown identifier. It cannot check the query *shape*: SpacetimeDB keeps this SQL
   as raw text during extraction, so a filter the node's RLS engine rejects fails `subscribe()` on
   the connection that carries it. A new filter shape is a live-verification item; a renamed column
   is caught offline.

---

## 5. Two subscription rules the schema exists to serve

- **No static catalogue duplicated per connection — moot since #483.** `game_item_template` alone
  is 17,720 rows × 32 columns; measured pre-#483, 20 per-player connections went 283 MB → 1,236 MB
  with it and 283 MB → 503 MB without. `gateway/src/stdb/subscriptions.rs:3362` (pre-#483) carried
  the ban, the numbers, and a test that failed if one was re-added. The concern this rule guarded
  against — one cache copy per player connection — no longer exists: per-player connections are
  deleted, so every catalogue read already goes through the single coordinator cache, once per
  database, with nothing left to duplicate it.
- **The sharded-only tables are subscribed conditionally.** A subscription to a table the deployed
  module does not have **fails to apply**, which fails the whole gateway — so a gateway restarted
  before its module was republished must not ask for `game_map_region`, `game_region_assignment`,
  `game_character_shard`, or the realm-core group/whisper/loot tables. `connection.rs:207–216`.

---

## 6. Scheduled tables

**23 scheduled tables** drive every periodic and deferred effect in the game. Nothing on a gateway
timer decides gameplay. Recount and re-list them with:

```bash
grep -rn 'scheduled(' module/src --include='*.rs'   # 23 tables plus 4 comment lines, 2026-09-03
```

| Scheduled table | Reducer | Cadence | Where |
|---|---|---|---|
| `game_motion_publish_schedule` | `publish_motion` | 50 ms (20 Hz) | `motion.rs:102` |
| `game_melee_schedule` | `tick_melee` | 100 ms | `combat/engage.rs:462` |
| `game_duel_schedule` | `tick_duels` | 250 ms | `duel.rs:81` |
| `game_taxi_flight_schedule` | `advance_taxi_flight` | 250 ms while a passenger is active | `taxi.rs:56` |
| `game_creature_move_schedule` | `tick_creatures` | 500 ms (sensing every 8th pass, so ~4 s) | `creatures/tick/mod.rs:178` |
| `game_ground_area_schedule` | `tick_ground_areas` | 500 ms | `spell/tables.rs:632` |
| `game_aura_schedule` | `tick_auras` | 1 s | `spell/tables.rs:576` |
| `game_breath_schedule` | `tick_breath` | 1 s | `breath.rs:32` |
| `game_event_reaper_schedule` | `reap_movement_events` | 1 s (`EVENT_TTL_MICROS`) | `gc.rs:24` |
| `game_transfer_reaper_schedule` | `reap_transfers` | 5 s, armed lazily by `begin_transfer` | `transfer/mod.rs:250` |
| `game_mail_escrow_reaper_schedule` | `reap_mail_escrows` | 5 s, armed lazily | `mail_escrow.rs:68` |
| `game_pet_care_schedule` | `tick_pet_care` | 7.5 s | `creatures/pet_care.rs:18` |
| `game_gateway_lease_reaper_schedule` | `reap_gateway_leases` | 15 s | `gw.rs:71` |
| `game_instance_reaper_schedule` | `reap_instances` | 60 s | `instance.rs:312` |
| `game_weather_schedule` | `tick_weather` | 10 min | `weather.rs:153` |
| `game_pending_cast` | `fire_pending_cast` | one-shot at cast completion | `spell/tables.rs:643` |
| `game_pending_spell_impact` | `fire_spell_impact` | one-shot at projectile landing | `spell/tables.rs:689` |
| `game_ranged_impact_schedule` | `ranged_impact` | one-shot at shot landing | `combat/engage.rs:474` |
| `game_auction_expiry` | `expire_auction` | one-shot at listing expiry | `auction.rs:162` |
| `game_creature_ai_summon_expiry` | `expire_eventai_summon` | one-shot at summon lifetime end | `creatures/eventai/mobility.rs:17` |
| `game_creature_ai_forced_despawn` | `fire_eventai_forced_despawn` | one-shot at the authored despawn time | `creatures/eventai/mobility.rs:51` |
| `game_creature_ai_relay_continuation` | `resume_relay_run` | one-shot at the authored relay delay | `creatures/eventai/relay.rs:295` |
| `game_creature_ai_relay_arrival` | `resume_relay_arrival` | one-shot on movement completion | `creatures/eventai/relay.rs:309` |

The interval rows are inserted by `init` (`seed_scheduler_arming`, `module/src/seed.rs`), except the
transfer and mail-escrow reapers, which their own entry points arm lazily and idempotently.
Scheduled reducers self-gate on `ctx.sender() == ctx.database_identity()` so they cannot be driven
externally.

⚠ Re-arming after a schema change is a real operational step: a republish can leave a schedule row
stale, because `init` does not re-run on an auto-migrating publish. `debug_repair_after_publish`
re-arms the motion, creature-tick, aura, ground-area, weather, gateway-lease and instance-reaper
schedules, and re-seeds every fixture family `init` seeds. It does not repair every scheduled table:
the event reaper and melee schedules are outside this reducer. **Nothing runs it for you.** The
operator calls it by hand on every shard after every publish:

```bash
spacetime call -s <server> <database> debug_repair_after_publish
```

Folding this into `lyracore publish` is tracked as issue 41 in the `lyracore-cli` repository. Until
it lands, a skipped call presents as a mid-session hang rather than a loud error.

Packages get a periodic hook without a table of their own: `game_tick_pass!` runs at the end of
every `tick_creatures` pass, after all core passes, and is expected to self-quantize for a slower
cadence.

---

## 7. Descriptor field indices

The generic UpdateMask encoder needs, per client-visible column, its `uint32` field index for build
5875.

> **Correction.** This document previously claimed the index map was *generated* into a shared-crate
> file. It is not, and never was. The indices are **hand-authored constants** in
> `gateway/src/codec/update_mask.rs`, module `idx` — each one the literal `set_int(N, ..)` /
> `set_bytes(N, ..)` argument from a `wow_world_messages` setter, with byte-equivalence tests
> cross-checking our serializer output against gtker's for every index gtker exposes. The handful
> gtker has no reference for (`PLAYER_QUEST_LOG_1_1 = 198` and friends) carry their derivation in a
> comment, agree byte-for-byte across mangos-zero / cmangos / vMaNGOS, and were verified on a live
> 5875 client. That is the discipline the "never typed from memory" rule was really asking for.

The encoder itself is hand-rolled because gtker 0.3 keeps its mask serializer crate-private and
exposes only slot 0 of each indexed descriptor array — auras (48 slots), quest-log counters (20
slots × 3), and multi-field item descriptors all hit that wall. One shared encoder owns every field
index; it is wired into the outbound path through `gateway/src/codec/values.rs`.

Two hard-won rules live with it and are restated in `danger-zones.md` §1:

- A partial `UNIT_FIELD_*` VALUES update must route through the `dirty_reset` path so it **never**
  carries `OBJECT_FIELD_TYPE`. Re-sending TYPE crashes the 5875 client at null+0x110.
- `SMSG_SET_FACTION_STANDING` carries the Faction.dbc **reputation index**, not the faction id.
  Sending the id indexes past the client's 64-slot array → ERROR #132.

---

## 8. The seeded fixture

`init` (`module/src/seed.rs:23`) seeds the realm row, start positions, race/class base info, the
config rows, the scheduled rows, and the static fixtures the test harness depends on, **all in one
transaction** — a partial seed never persists.

Three things to know about it:

- `game_realm.address` is seeded to `127.0.0.1:8085`. An external client will log in and then fail to
  reach the world server. There is no config knob for it today; change it in `seed.rs` and republish,
  or `UPDATE game_realm SET address=...` before launch. This is a known hardening gap for any
  non-localhost deployment — fix it before exposing the gateway beyond loopback.
- **auto_inc sequences sit behind ETL-imported explicit ids.** The world import writes rows with
  explicit ids without advancing the sequence, so a later `id: 0` insert allocates a colliding id —
  reducers panic and roll back the whole transaction, SQL inserts fail silently. Fixture rows must
  use fixed reserved ids (509xxxx) with delete-first. `danger-zones.md` §2 has the full list of
  reserved ranges.
- The taxi harness reserves the 5090000+ storage namespace for two nodes, one directed route, three
  ordered points, and a nearby `GOSSIP|TAXI` flight master. Its wire ids 255 and 256 fit the vanilla
  taxi mask and are unique-indexed independently of storage. They support headless protocol tests;
  an unmodified client has no matching synthetic map entries, so visual real-client flights must use
  imported nodes. The DBC pass always restores the catalogue rows; the map-0 world import restores
  and verifies the NPC after replacing spatial content.

Real world content — 2,200+ spawns, 420 quests, the item/spell/faction catalogues — is **not**
seeded. It comes from the importer; see [`data-ingestion.md`](./data-ingestion.md).
