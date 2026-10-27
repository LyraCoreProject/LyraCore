# Schema

This page is a map of the data model, not a copy of it. Every table is a Rust `#[table]` struct in
`module/src/**`, or in a Package compiled into the same Module. Where this page and the code
disagree, the code wins. Read a struct for its columns; this page records what the struct cannot
say: invariants, why a column exists and ordering rules.

[`architecture.md`](./architecture.md) explains how the tables fit into the runtime.
[`danger-zones.md`](./danger-zones.md) §1.2 is the authority for migrations.

## 1. Conventions

- **Naming.** Core tables take the `game_` prefix; Package tables take `pkg_<name>_`. External
  gtker crates keep their `wow_` names.
- **`accessor`** is the Rust identifier the macro exposes as `ctx.db.<accessor>()`. SpacetimeDB 2.x
  syntax is `#[table(accessor = …)]`, with every index declared inside that one attribute.
- **Visibility.** `public` means any connection can subscribe to the table. Without it the table
  is private, and only the Shard's owner can read it. The Native Host connects in process as that
  owner.
- **RLS.** The Module declares no `#[client_visibility_filter]`; §4 says why. `module/Cargo.toml`
  keeps SpacetimeDB's `unstable` feature, because the attribute needs it.
- **Migration discipline.** Every table grows by END-appending a column with an explicitly typed
  `#[default(...)]`. Ordering-sensitive data uses explicit sequence or timestamp columns, never
  auto-inc ordering.

`game_native_player.gossip_root_menu_id` is an end-appended column with default 0, the creature
entry menu. Submenu selections keep this root so a Runtime Script menu replacement invalidates
the open menu. SpacetimeDB disconnects clients for this schema update; reconnect starts at the
entry menu. The Host does not decode this table, and its delivery layouts do not change.

`game_creature_level_stats` holds final health, damage and armor by template and level.
`game_creature_display` holds weighted display choices by template. Both are new tables, so an
existing Realm migrates without changing stored template rows. A template stat change takes
precedence over level rows when its endpoint values no longer match the imported endpoints.
The presence of level rows for an entry preserves the imported stat convention even after an edit
changes the template endpoints; edited values never acquire classic rank multipliers. An authored
display or scale change likewise takes precedence over the imported display choices.
Classic imports clear the level rows and restore classic rank scaling.

EventAI keeps the published rule's policy enums fieldless. The new private
`game_creature_ai_script_policy` table holds command-list boundaries and event admission policies,
keyed by definition revision and source rule id. Imports replace rules and policies together.
`game_creature_ai_relay_run.failure_policy` is an end-appended optional enum with a typed `None`
default, so old relay runs keep their failure behavior. See [command lists](data-ingestion.md#command-lists-in-eventai)
for execution and failure rules.

### The three migration rules that have bitten

1. **A default must be END-appended and explicitly typed.** A bare `#[default(0)]` on a `u64`
   column encodes as a 4-byte i32, and the publish migration rejects it (`data too short for
   u64`). Write `#[default(0u64)]`. See the typed field defaults in `module/src/character.rs`.
2. **`#[default(...)]` cannot default a `String`.** The macro type-checks the expression in a
   const block, and dropping a `String` in const-evaluated Rust is `error[E0493]` on stable. Put
   new string data in a new table, the one-row-plus-child-rows shape that `game_creature_waypoint`
   and `game_npc_text_slot` use.
3. **Native reducer arguments are a binary contract.** The Native Host passes them as BSATN with
   no field names, and shares the `game_native_cipher` and `game_native_packet` layouts with the
   Module. Change both sides together and run the native protocol check. `./lyracore dev watch`
   refuses any change to reducer arguments or private tables.

## 2. Inventory

Recount rather than trust a number here. The pattern matches both `#[table(...)]` and the fully
qualified `#[spacetimedb::table(...)]`; missing the second form undercounts.

```bash
grep -rEn '^#\[(spacetimedb::)?table\(' module/src --include='*.rs' | wc -l
grep -rEc '^#\[(spacetimedb::)?table\(' module/src --include='*.rs' | grep -v ':0$'  # per file
```

| Domain | Where |
|---|---|
| Auth, Sessions, guid allocation | `auth.rs`, `account_ownership.rs`, `sessionless.rs` |
| Native connections, admission and AOI Grants | `native/mod.rs`, `native/logon.rs`, `native/admission.rs`, `native/member_stats.rs`, `aoi.rs` |
| Character and per-Character progression | `character.rs`, `skill.rs`, `reputation.rs`, `talent.rs`, `spell/spellbook.rs`, `action_bar.rs`, `combo.rs`, `rest.rs`, `corpse.rs`, `xp.rs`, `exploration.rs`, `breath.rs`, `breath_relay.rs`, `fatigue.rs`, `graveyard.rs` |
| World entity and movement | `world.rs`, `motion.rs` |
| Terrain, nav and exact vmap | `terrain.rs`, `nav.rs`, `vmap.rs` |
| Chat, social and addon bridge | `chat.rs`, `channel.rs`, `realm_chat.rs`, `away.rs`, `bridge.rs` |
| Combat, threat and duel | `combat/engage.rs`, `combat/death.rs`, `combat/energy.rs`, `threat.rs`, `duel.rs` |
| Spell and aura | `spell/tables.rs`, `spell/stacking.rs`, `spell/cast/resolve.rs` |
| Quest | `quest.rs` |
| Item, trade and mail | `items/tables.rs`, `items/properties.rs`, `trade.rs`, `mail.rs`, `mail_catalogue.rs`, `mail_escrow.rs`, `mail_timer.rs`, `mail_text.rs` |
| Auction house | `auction.rs` |
| Creature (template, spawn, AI, pet, trainer) | `creatures/*`, `trainer.rs`, `gossip.rs` |
| GameObject | `gameobject.rs`, `go_model.rs`, `go_collider.rs` |
| Loot | `loot/*` |
| Group, Meeting Stones and Guild | `group.rs`, `meeting_stone/mod.rs`, `guild/*` |
| Instance and encounter | `instance.rs`, `encounter.rs` |
| Taxi, Transport and weather | `taxi.rs`, `transport/mod.rs`, `weather.rs` |
| Packages | `package_account.rs`, `package_config.rs`, `package_import.rs`, `package_teardown.rs`, `script_binding.rs` |
| Event GC | `gc.rs` |
| Config, static data and diagnostics | `config.rs`, `gm.rs`, `faction.rs`, `skilldata.rs`, `stats.rs`, `import_meta.rs`, `debug/*` |
| Guid ranges and retired coordination | `realm_core.rs`, `gw.rs`, `region.rs`, `load.rs`, `transfer/mod.rs` |

`claim_guid_range` in `realm_core.rs` installs the Realm's guid range. The other tables in that
last row held multi-Shard routing, Gateway leases, load samples and cross-Shard Transfers. They
stay because dropping a table is a destructive migration. Native requests do not route through
them.

Two shapes recur:

- **Catalogue tables**: `game_item_template`, `game_spell`, `game_spell_effect`,
  `game_creature_template`, `game_quest_*`, `game_faction*`, the three `game_taxi_*` tables and
  other DBC-derived reference data. Only the importer writes them.
- **Event tables**: transient outbound signals that a row diff cannot carry, such as a movement
  Relay's animation opcode. The Module inserts them inside the transaction that caused them. The
  `*_live` tables are SpacetimeDB event tables and keep no rows; older event tables are reaped by
  `gc.rs` about one second later. See [Native Relay events](./danger-zones.md#native-relay-events).

## 3. Tables with rules the struct cannot show

### Map catalogue (`module/src/config.rs`)

`game_map` stores selected vmangos `map_template` rows. `internal_name` alone comes from the
client's `Map.dbc`, joined by map id, and names the WDT/ADT archive directory. Missing client
maps refuse the import. The gameplay fields are `map_type`, `linked_zone`, `parent`, `name`,
`player_limit`, `reset_delay`, and `ghost_entrance_map/x/y`. A ghost map of -1 means
no entrance. Instance admission, reset schedules and ghost routing do not yet consume these fields.
This is a new table, so publishing it adds an empty catalogue without changing existing rows.

### Taxi catalogue (`module/src/config.rs`)

`game_taxi_node`, `game_taxi_path` and `game_taxi_path_node` are public rows imported from the
Operator's `TaxiNodes.dbc`, `TaxiPath.dbc` and `TaxiPathNode.dbc`. A node's `id` is its storage key
and `client_node_id` is its unique wire id in the client's fixed 256-bit taxi mask. Imported nodes
use their DBC id for both; reserved fixtures keep 509xxxx storage ids with bounded synthetic wire
ids. Paths are directed edges with their fare in copper; the reverse direction exists only when the
DBC has a second row. Path nodes keep explicit `(path_id, node_index)` order. `flags` stays the DBC's
signed `int32` container so every raw bit survives, and `delay_ms` holds the DBC's seconds in
milliseconds. The importer validates references, wire-id bounds and collisions, and duplicate
ordinals before it writes. It replaces nodes first, paths second and path nodes last, so a
successful rerun removes stale geometry.

### Transports (`module/src/transport/mod.rs`)

`game_transport` is a public catalogue with one row per imported route. A Transport's position is
a function of the Realm Clock and its timetable, so no row stores it. `period_ms` is the client's
cycle from the world database, and `map_id` is the map of the route's first node. `game_transport_crossing` is private and holds the current map of each route
that crosses maps. Its one-shot schedule fires at the next map change and arms the one after it.
`game_transport_passenger` is private and holds one row per Passenger with its offset on the
Transport. Leaving the Transport, a teleport other than its Transport's map crossing, or leaving
the world deletes the row.
`game_transport_carry_schedule` has one 500 ms schedule per Transport with Passengers; it deletes
itself when none remain.

### Taxi flights (`module/src/taxi.rs`)

`game_character_taxi_node` is private, durable taxi progression: one row per known node, by storage
id. Opening a nearby friendly flight master discovers its node idempotently; a status query only
reads.

`game_active_taxi_flight` has at most one row per Character, because `character_guid` is its
primary key. A successful activation creates the row in the same transaction that deducts the
fare, so a duplicate activation sees the row before it can charge again. The Module queues the
activation reply before it arms the flight schedule. `game_taxi_flight_schedule` drives the 250 ms
route clock. The public `game_taxi_passenger_spline` row carries the remaining route to the owner
and AOI observers, and is refreshed when the passenger changes AOI cell.

### `game_world_entity` (`module/src/world.rs`)

Public. One row per Character in the world and per spawned creature. Login creates it; disconnect
deletes it. It is the visibility source and the descriptor source for `SMSG_UPDATE_OBJECT`.

- `by_grid` has four columns, `(map_id, instance_id, grid_x, grid_y)`. `instance_id` is in the key
  because an Instance is its own spatial partition.
- `by_cell` serves AOI. SpacetimeDB 2.7.1 serves that subscription only with three
  equality-indexed columns, `(map_id, instance_id, cell)`.
- `by_owner` turns the owner lookup in `helpers::entity_by_owner` into an index probe.

Never `.iter()` this table or the other spatial tables; see [danger zones](./danger-zones.md) §1.7.
`game_entity_motion` and `game_creature_spline` carry the same grid and cell keys.
`game_gameobject`'s grid key has three columns, without `instance_id`.

`mount_display_id` carries `UNIT_FIELD_MOUNTDISPLAYID` for both mounted mechanics, and the Module
decides which one owns it:

- **Taxi flight** owns the field for the whole flight. Activation writes the faction mount display
  and sets `UNIT_FLAG_TAXI_FLIGHT` in `unit_flags`; landing clears both. The pair goes out in one
  partial VALUES mask without `OBJECT_FIELD_TYPE`. Route progress belongs to the active-flight row,
  never to `game_creature_spline`.
- **Land mount** owns it the rest of the time, as a projection. The `A_MOUNTED` aura row is the
  mounted state, and `mount::recompute_mount` derives `mount_display_id` and `run_speed_mult_bp`
  from the current aura set. Every aura removal path ends in that one recompute. A land mount never
  sets `UNIT_FLAG_TAXI_FLIGHT`, and both mount entry points refuse a Character in flight.

`run_speed_mult_bp` holds the effective multiplier from the shared move-speed fold while a land
mount is active (16000 for a nominal 60% mount). A change becomes `SMSG_FORCE_RUN_SPEED_CHANGE`.

Hunter pets keep a durable identity in `game_hunter_pet`. A materialized pet is an ordinary owned
`game_world_entity`, classified by `game_live_pet_kind`; summoned demons have no Hunter identity or
care state. Taming retires the wild entity and arms its authored `game_creature_spawn` for a later
respawn, so the spawn point never moves into the pet identity.

### `game_character` (`module/src/character.rs`)

Public. It exists whether or not the Character is online, so an offline Character costs no live
world machinery. `owner_identity` is `Identity::ZERO` until `establish_session` binds it.

### Accounts and Sessions (`module/src/auth.rs`, `module/src/account_ownership.rs`)

All private. `game_account` holds the SRP6 salt and verifier, never a password, and the Account's
Alpha Test Tools authority and GM level. `game_character.gm_level` is legacy:
`debug_repair_after_publish` moves a nonzero value onto the Account and zeroes it. New Accounts copy
`game_alpha_test_tools_enrollment`, which starts enabled; changing enrollment never changes an
existing Account. `game_session` holds the SRP6
session key K. A row is valid for one hour from its logon (`expires_at`): world authentication
refuses an expired row like an absent one, and `reap_sessions` deletes expired rows every five
minutes. `game_operator` is a singleton holding the one Operator identity, captured by
`claim_operator`.

`game_account_character_owner` is keyed by Character guid and stores the exact Account id and name.
Its rows remain across logout and Character deletion. Character guids are never reused, so retained
rows are ownership tombstones. Each cleanup reads at most 4,097 rows and refuses when an Account
already has more than 4,096; it never truncates. `game_account_fence` is a retired
Gateway table, and nothing writes it. The playerbots test suite still opens Account Claims in
`game_account_claim`. Operator reducers that act for a Character take a `SessionActor`; a null
ownership refuses a Character with an active Account Claim.

### Chat (`module/src/chat.rs`, `module/src/realm_chat.rs`, `module/src/channel.rs`)

`game_chat_event_live` is the event row for one say, yell, `/e` or creature line. The chat handler
selects listeners in range. Only `apply_send_chat` (a Character) and `apply_send_chat_to` (EventAI)
write it, and a Character's line passes the language Gate first.

`game_realm_chat_event` (private event) is one Realm Chat Line: party, raid, raid leader, raid
warning, guild, officer, channel or whisper, with its whole recipient list, Chat Kind, language,
chat tag and an `ignorable` flag. A whisper writes several rows: the line, the sender's echo, and
an Auto-Reply or "is ignoring you" line when one applies. `realm_chat::emit` is its only writer. An
addon-language line keeps its payload untrimmed.

`game_character_away` (`module/src/away.rs`) holds the Auto-Reply of a Character with an Away
Status. The `PLAYER_FLAGS` bit on the live entity is what observers see; both change in one
transaction, and login clears them.

`game_chat_channel` is one Chat Channel, keyed by team and lowercase name, with the creator's
spelling, owner, password and flags. `game_chat_channel_member` is one Channel Membership; its `id`
is join order. `game_chat_channel_ban` holds bans, and `game_chat_channel_notice_event` is one
Channel Notice with its recipients. `realm_channel_op` runs every channel op from an op byte. The
last member to leave deletes the channel with its password and bans.

`game_character_contact` (public) holds friend and ignore rows.

`game_channel_member`, `game_channel_event` and `game_whisper_event` in `module/src/chat.rs` are
retired. They stay because dropping a table is a destructive migration. Nothing writes them.

### Mail (`module/src/mail.rs`, `mail_timer.rs`, `mail_text.rs`)

`mail::insert_letter` is the only writer of `game_mail`, and a source scan in `mail.rs` fails on
any other insert. The END-appended header columns carry what the 1.12 inbox reads: `sender_kind`
(the vanilla `MailMessageType`: 0 Character, 2 Auction, 3 Creature, 4 Gameobject), `sender_entry`
(the auction house id, or the creature or gameobject entry), `check_flags` (the `MailCheckMask`
bits except READ, which stays `was_read`), `mail_template_id` and `deliver_micros` (when the
recipient can first see the Mail). Every default is a real value: a legacy row is a Character mail
with no stored flags, visible since creation.

`check_flags` also holds server-only bits above the five vanilla ones
(`lyracore_shared::mail::CLIENT_CHECK_MASK`), and the codec never sends them. 0x20 is
`CHECK_FLAG_LETTER_GRANTED`, so the next free server-only bit is 0x40. `item_text_id` on
`game_mail` is the attached Plain Letter's text id; its default 0 means an attachment with no text.
Expiry is not a column: `lyracore_shared::mail::expires_at_secs` derives it from creation, delivery
and the cash on delivery price.

`game_mail_timer` holds each Mail's one Mail Timer, unique by `mail_id`. `insert_letter` arms it at
the delivery instant of an undelivered Mail and at the expiry of every other Mail. At expiry it
returns a Character's Mail that still carries an item, in place, and deletes every other Mail with
its copper. `game_mail_arrival` is the Mail Arrival event, reaped by the event GC.

`game_item_text` is a Letter Copy's durable text, keyed by its mail id narrowed to u32.
`mail_text::apply_copy_text` sets the COPIED bit and files the row in one reducer, so a Mail and
its item text always agree. The row outlives the Mail. Its END-appended `letters` counts the Plain
Letters that can carry it: each copy adds one, and `mail_text::drop_letter_text` takes one away
when a Mail is deleted with the letter attached or when a Character is deleted. The row goes at 0.
The default `u32::MAX` marks a row filed before the count existed, and such a row is never
deleted. The count can be too high, never too low. A letter destroyed or sold from the bags drops
nothing. `game_item_instance.item_text_id` is 0 for every item that is not a copied letter, and
every item snapshot (mail, trade, auction) keeps it.

`game_mail_escrow` and `game_mail_delivery` hold legacy cross-Shard Mail value. World Session mail
uses no Escrow. A Reward Letter for a Character without a native connection still files one
through `mail_escrow::file_reward` (`module/src/mail_reward.rs`).

### Auctions (`module/src/auction.rs`)

`game_auction_house` is the public `AuctionHouse.dbc` catalogue. It resolves an auctioneer's house
and economic policy through its imported faction. In `game_auction`, the item columns are the
complete item-instance snapshot while no inventory row exists, and the house and rate columns keep
the policy from listing time. `game_auction_operation_receipt` makes a repeated sell request
idempotent and keeps the snapshot for expiry. `game_auction_expiry` is a private one-shot schedule
at the listing's original deadline; it returns an unbid item or settles a winning bid, then does
nothing when replayed. Native requests create no `game_auction_hold`, `game_auction_bid_hold` or
`game_auction_bid_decision` rows; those are legacy Gateway rows. Deletion is refused while a
Character owns Auction value.

Every mail a bid, buyout, expiry, Cancellation or refused listing sends is an Auction Mail:
`MailSender::AuctionHouse(house)`, `checked = COPIED`, and a machine subject
(`{item_entry}:{random_property_id}:{action}`) that the client turns into its own text. Private
`game_auction_notice` is the matching live packet: one row per outbid, won, sold, expired, new-bid
or removed notice, inserted in the same transaction as its mail and reaped by the event GC.

### Guilds (`module/src/guild/`)

`game_guild` is one Guild: its name, a unique lower-case `name_key`, the leader, the team fixed at
founding, the MOTD, the info text and the emblem. `game_guild_rank` holds its five to ten Guild
Ranks with their Rank Rights. `game_guild_member` is keyed by Character guid, so a Character is in
at most one Guild; each row keeps a name snapshot. `game_guild_event` is an event table where
`recipient_guid == 0` goes to every online member and a nonzero value addresses one Character.
`game_guild_invite` is one pending offer per target Character, reaped on `INVITE_TTL_MICROS`.
`game_guild_petition` is one open Petition, at most one per owner, tied to its Guild Charter item
guid. `game_guild_petition_signature` is keyed by `petition_signature_key(petition_id, slot)` for
slots 0 to 8. Turn-in deletes both and founds the Guild in the same transaction. The typed
`GuildOp` gets each new variant at the end, because BSATN encodes a variant by its position.

PLAYER_GUILDID and PLAYER_GUILDRANK are not stored. The Module projects them from
`game_guild_member`. `game_guild_fee_hold` and
`game_guild_fee_decision` are retired Gateway tables, and nothing writes them.

### Riding (`module/src/skill.rs`, `module/src/skilldata.rs`, `module/src/trainer.rs`)

Riding is an ordinary skill line, 762. A Character's learned rank is a normal `game_player_skill`
row on it. `game_skill_ability` joins a mount spell to line 762 with the `min_skill` threshold for
vanilla's 75 (Apprentice) and 150 (Journeyman) tiers, plus race and class masks. The cast Gate
walks the Character's learned lines and probes `by_skill_line`, so it never scans the whole ability
catalogue. It fails closed: a mount whose skill data was never imported cannot be cast.

`game_trainer_spell.learn_skill_line` names a profession, a weapon line or riding, and
`learn_skill_cap` carries the tier the purchase grants. A riding offering's `spell_id` is a marker
with no `Spell.dbc` row, so the purchase confirmation does not echo it as a learned spell. The
trainer NPC declares itself with `trainer_type::MOUNTS`.

### Proc data (`module/src/spell/tables.rs`)

A Proc is an aura that fires off a combat event. Its rules live in three places: two are static
data the importer writes, the third is a per-aura snapshot.

`game_spell` carries what `Spell.dbc` knows: `proc_flags` (the vanilla `procFlags` bitmask, with
no translation table), `proc_chance` (a whole percent) and `proc_charges` (0 means unlimited).

`game_spell_proc_event` is the classic-db overlay for what `Spell.dbc` has no column for: `ppm_rate`,
`icd_ms`, `proc_ex` (the normal-hit and critical-hit rule), the `school_mask`, `family_name` and
`family_flags` filter on triggering spells, a `proc_flags` override and a `custom_chance`. It is
keyed by spell id, private, and absent for most spells; an absent row means "use the header's
values". The `spellmeta` dump family loads it, and its clear stops at the reserved synthetic id
floor so the seeded Proc fixtures keep their overlay rows.

`game_aura` carries the frozen profile: `proc_flags`, `proc_chance`, `proc_ppm`, `proc_ex`,
`proc_school_mask`, `proc_family_name`, `proc_family_flags`, `proc_charges`, `proc_icd_ms`,
`proc_ready_micros`. `spell::proc::freeze_profile` folds header and overlay together once, at
apply, so the proc pass reads one row. The overlay wins where both carry a value; charges come only
from the header. `proc_charges` and `proc_ready_micros` are mutable: a fire spends a charge and
stamps the next cooldown deadline. A refresh freezes the other columns again and refills charges,
and leaves a running cooldown alone. That is why `debug_repair_after_publish` freezes Proc profiles again: an
aura applied before the columns existed reads as "never procs", and a permanent self-buff never
refreshes on its own.

### `game_item_enchantment` (`module/src/items/properties.rs`)

One row per enchantment effect. `kind` is the Stat Kind: an append-only code for the effect's
contribution, independent of the client encoding. The importer keeps an unmapped effect as kind 0,
which contributes nothing. Amounts are flat points, except crit, hit, dodge, parry and block, which
use basis points, and regeneration, which uses points per five seconds. Spell power keeps a school
mask.

| Code | Contribution |
| --- | --- |
| 0 | Unknown |
| 1 | Strength |
| 2 | Agility |
| 3 | Stamina |
| 4 | Intellect |
| 5 | Spirit |
| 6 | Health |
| 7 | Mana |
| 8 | Holy resistance |
| 9 | Fire resistance |
| 10 | Nature resistance |
| 11 | Frost resistance |
| 12 | Shadow resistance |
| 13 | Arcane resistance |
| 14 | Armor |
| 15 | Weapon damage |
| 16 | Spell power |
| 17 | Healing power |
| 18 | Mana per five seconds |
| 19 | Health per five seconds |
| 20 | Crit |
| 21 | Hit |
| 22 | Defense |
| 23 | Dodge |
| 24 | Parry |
| 25 | Block |

Each enchantment effect has key `(u64(enchant_id) << 8) | effect_index`. Indices reserve 64
positions per client enchantment slot, and a referenced spell's three subeffects each reserve eight
positions for expanded stats or schools. The index is `slot * 64 + subeffect * 8 + expansion`, from
0 through 191. Direct effects use subeffect zero and expansion zero. `spell_id` keeps the source
spell reference even when the Stat Kind is unknown.

Enchantment IDs 7745 and 7748 are authored compatibility entries, +3 Strength and +3 Stamina. They
keep existing stored item meanings and are not rows from the build 5875 catalogue. The importer
refuses a client catalogue that collides with either ID. The client receives 7745 as 823 and 7748
as 724, with the same Stat Kind and amount (`lyracore_shared::item_property`); the Module keeps the
stored ID.

### `game_vmap_indoor_cell` (`module/src/vmap.rs`)

A private, derived per-cell marker keyed `(generation_id, cell_key)`: this cell of this vmap
generation holds at least one indoor WMO triangle. `verify_vmap_generation` writes it once, since
it already decodes every staged chunk, and it is deleted with the geometry it came from. It exists
so the movement heartbeat can ask "am I indoors?" with one indexed find, and pay for a ray cast only
in a cell that could answer yes.

**A missing row means outdoors.** The indoor rule rests on that fail-open contract: vmap disabled,
no active generation, no marker row or no containing WMO group all read as outdoors. A generation
verified before this table existed has no rows, so indoor behaviour stays off there until the
generation is verified or imported again.

## 4. Row-level security

The Module declares no `#[client_visibility_filter]`. Recount at any time:

```bash
rg -n '#\[client_visibility_filter' module/src --glob '*.rs'
```

An RLS filter applies only to a subscription made under a client identity. The Native Host
connects in process as the Shard owner, which bypasses RLS. The Module decides what each client
sees through AOI Grants and recipient selection; see [`architecture.md`](./architecture.md).

Two constraints bind any filter added later:

1. **RLS cannot express spatial scoping.** `:sender` is a static identity, so a filter cannot join
   on the caller's current position.
2. **Offline validation covers identifiers, not shapes.** `lyracore preflight` parses every
   `Filter::Sql` and fails on an unknown table or column. It cannot check the query shape:
   SpacetimeDB keeps this SQL as raw text during extraction, so a filter the node's RLS engine
   rejects fails `subscribe()` on its connection. A new filter shape needs live verification.

## 5. Scheduled tables

Scheduled reducers drive every periodic and deferred effect in the game. List them with:

```bash
rg -n 'scheduled\(' module/src --glob '*.rs'
```

| Scheduled table | Reducer | Cadence | Where |
|---|---|---|---|
| `game_motion_publish_schedule` | `publish_motion` | 50 ms (20 Hz) | `motion.rs` |
| `game_melee_schedule` | `tick_melee` | 100 ms | `combat/engage.rs` |
| `game_duel_schedule` | `tick_duels` | 250 ms | `duel.rs` |
| `game_taxi_flight_schedule` | `advance_taxi_flight` | 250 ms while a passenger is active | `taxi.rs` |
| `game_transport_carry_schedule` | `carry_transport_passengers` | 500 ms per Transport while it has Passengers, armed by boarding | `transport/mod.rs` |
| `game_transport_crossing` | `cross_transport_map` | one-shot at a route's next map change, armed by world entry | `transport/mod.rs` |
| `game_creature_move_schedule` | `tick_creatures` | 500 ms (sensing every 8th pass, about 4 s) | `creatures/tick/mod.rs` |
| `game_ground_area_schedule` | `tick_ground_areas` | 500 ms | `spell/tables.rs` |
| `game_native_admission_tick` | `native_admission_tick` | 1 s while the Login Queue holds waiters | `native/admission.rs` |
| `game_aura_schedule` | `tick_auras` | 1 s | `spell/tables.rs` |
| `game_breath_schedule` | `tick_breath` | 1 s | `breath.rs` |
| `game_event_reaper_schedule` | `reap_movement_events` | 1 s (`EVENT_TTL_MICROS`) | `gc.rs` |
| `game_transfer_reaper_schedule` | `reap_transfers` | 5 s, armed lazily | `transfer/mod.rs` |
| `game_mail_escrow_reaper_schedule` | `reap_mail_escrows` | not armed; a leftover row deletes itself | `mail_escrow.rs` |
| `game_meeting_stone_reminder_schedule` | `remind_queued_parties` | 5 s; a queued Party hears `IN_PROGRESS` every 5 min | `meeting_stone/mod.rs` |
| `game_native_member_stats` | `tick_native_member_stats` | 5 s per grouped native connection | `native/member_stats.rs` |
| `game_pet_care_schedule` | `tick_pet_care` | 7.5 s | `creatures/pet_care.rs` |
| `game_gateway_lease_reaper_schedule` | `reap_gateway_leases` | 15 s; closes expired Account Claims | `gw.rs` |
| `game_native_logon_reaper` | `native_reap_logon_addresses` | 60 s, armed by a logon connection | `native/logon.rs` |
| `game_instance_reaper_schedule` | `reap_instances` | 60 s | `instance.rs` |
| `game_session_reaper_schedule` | `reap_sessions` | 5 min, armed by `establish_session` | `auth.rs` |
| `game_weather_schedule` | `tick_weather` | 10 min | `weather.rs` |
| `game_pending_cast` | `fire_pending_cast` | one-shot at cast completion | `spell/tables.rs` |
| `game_pending_spell_impact` | `fire_spell_impact` | one-shot at projectile landing | `spell/tables.rs` |
| `game_ranged_impact_schedule` | `ranged_impact` | one-shot at shot landing | `combat/engage.rs` |
| `game_auction_expiry` | `expire_auction` | one-shot at listing expiry | `auction.rs` |
| `game_instance_removal` | `expire_instance_removal` | one-shot 60 s after an Instance Removal starts | `instance.rs` |
| `game_mail_timer` | `fire_mail_timer` | one-shot at a Mail's delivery, then at its expiry | `mail_timer.rs` |
| `game_creature_ai_summon_expiry` | `expire_eventai_summon` | one-shot at summon lifetime end | `creatures/eventai/mobility.rs` |
| `game_creature_ai_forced_despawn` | `fire_eventai_forced_despawn` | one-shot at the authored despawn time | `creatures/eventai/mobility.rs` |
| `game_creature_ai_relay_continuation` | `resume_relay_run` | one-shot at the authored relay delay | `creatures/eventai/relay.rs` |
| `game_creature_ai_relay_arrival` | `resume_relay_arrival` | one-shot on movement completion | `creatures/eventai/relay.rs` |

`init` inserts the interval rows (`seed_scheduler_arming` in `module/src/seed.rs`). The transfer,
mail-escrow, session and native logon reapers, the native admission and member-stat ticks and the
Transport carry schedules are armed by their own entry points, lazily and idempotently. Scheduled
reducers check `ctx.sender() == ctx.database_identity()`, so nothing outside can drive them.

`init` does not run again on an auto-migrating publish, so a schedule row can go stale.
`debug_repair_after_publish` re-arms the interval schedules except the event reaper and melee
schedules. Call it after every publish; see
[After every publish](./danger-zones.md#after-every-publish).

Packages get a periodic hook without a table of their own: `game_tick_pass!` runs at the end of
every `tick_creatures` pass, after all core passes, and must quantize itself for a slower cadence.

## 6. Descriptor field indices

The UpdateMask encoder needs each client-visible column's `uint32` field index for build 5875.
The indices are hand-authored constants in `crates/lyracore-protocol/src/codec/update_mask.rs`,
module `idx`. Each one is the literal `set_int(N, ..)` or `set_bytes(N, ..)` argument from a
`wow_world_messages` setter, and byte-equivalence tests compare our output with gtker's for every
index gtker exposes. The few gtker has no reference for, such as `PLAYER_QUEST_LOG_1_1 = 198`,
carry their derivation in a comment. They agree across mangos-zero, cmangos and vMaNGOS and were
verified on a live 5875 client.

The encoder is hand-written because gtker 0.3 keeps its mask serializer crate-private and exposes
only slot 0 of each indexed descriptor array: auras (48 slots), quest-log counters (20 slots × 3)
and multi-field item descriptors all hit that wall. One shared encoder owns every field index, and
`crates/lyracore-protocol/src/codec/values.rs` builds the outbound VALUES blocks with it. The two
rules that crash the client, partial VALUES without `OBJECT_FIELD_TYPE` and the faction-standing
reputation index, are in [danger zones](./danger-zones.md) §1.

## 7. The seeded fixture

`init` (`module/src/seed.rs`) seeds the realm row, start positions, race and class base info, the
config rows, the scheduled rows and the static fixtures the tests depend on, all in one
transaction, so a partial seed never persists.

- `game_realm.address` is seeded to `127.0.0.1:8085`. `./lyracore dev up` sets it for the local
  Realm, and `--lan` sets the private address. A deployment records and sets the advertised
  address itself; see [the native operations guide](./native-operations.md).
- auto_inc sequences sit behind imported explicit ids, so fixture rows use fixed reserved ids
  (509xxxx) with a delete first. [Danger zones](./danger-zones.md) §2 lists the reserved ranges.
- The taxi fixture reserves the 5090000+ storage namespace for two nodes, one directed route, three
  ordered points and a nearby `GOSSIP|TAXI` flight master. Its wire ids 255 and 256 fit the vanilla
  taxi mask and have their own unique index. They support headless protocol tests. An unmodified
  client has no map entries for them, so visual flights use imported nodes. The DBC pass always
  restores the catalogue rows; the map-0 world import restores and verifies the NPC after it
  replaces spatial content.

Real world content is not seeded. It comes from the importer; see
[`data-ingestion.md`](./data-ingestion.md).
