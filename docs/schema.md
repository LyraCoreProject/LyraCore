# Schema

This page is a map of the data model, not a copy of it. Every table is a Rust `#[table]` struct in
`module/src/**`, or in a Package compiled into the same Module. Where this page and the code
disagree, the code wins. Read a struct for its columns; this page records what the struct cannot
say: invariants, why a column exists and ordering rules.

[`architecture.md`](./architecture.md) explains how the tables fit into the runtime.
[`danger-zones.md`](./danger-zones.md) §1.2 is the authority for migrations.

`game_crit_per_agility` stores imported class/level rates; `game_creature_defense` stores imported
creature defense flags. Both are new catalogues. See the [white melee migration](danger-zones.md#white-melee-catalogue-migration)
before publishing to an existing Realm. The private `game_missing_crit_catalogue` singleton records
the first missing crit rate and suppresses repeat warnings while the previous formula is in use.

`game_npc_vendor.restock_seconds` and `stock_flags` default to zero, preserving legacy purchases
until the items reimport. The private `game_vendor_stock` key combines spawn GUID and item entry;
an open-world entry choice uses its source spawn GUID, while Instance copies keep their own GUID.
Counts and whole-second timestamps are shared by all Characters buying from that spawn.
`game_vendor_stock_runtime` records the Host process identity that owns those counters.
See [vendor stock migration](danger-zones.md#vendor-stock-migration) for lifetime and import rules.

`game_active_taxi_flight.first_destination_node_index` is end-appended with default 1. It names
the first DBC vertex after the stored launch origin, so existing flights retain their route.
A reconnect sends the unmounted login CREATE, then restarts the remaining spline from the saved
position. `game_combat_check_schedule` is a new private one-shot schedule: suspending hostility for
taxi resumption keeps the combat flag until the 1,200 ms check, including when the flight lands first.

## 1. Conventions

- **Naming.** Core tables take the `game_` prefix; Package tables take `pkg_<name>_`. External
  gtker crates keep their `wow_` names.
- **`accessor`** is the Rust identifier the macro exposes as `ctx.db.<accessor>()`. SpacetimeDB 2.x
  syntax is `#[table(accessor = …)]`, with every index declared inside that one attribute.
- **Catalog.** `module/build.rs` re-exports each Core table under `crate::tables` and the crate
  root. A table Packages must not rely on goes in `crate::outside_table_catalog!(..)` in its
  declaring file; [`module-build.md`](./module-build.md) has the rules.
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

`game_native_player.teleport_started_micros` records when a native teleport began. Its typed zero
default expires a near teleport already pending at migration on the next tick. World ports do not
use this timeout.

Native world-port Transfer adds private IDC outboxes and a pending connection table. The
`game_directory_clock` row carries the greatest observed directory revision across World Shards.
`game_native_player.teleport_origin` is an end-appended optional position with default `None`.
`game_native_handoff` appends `world_port` (false), `teleport_counter` (0), `transfer_id` (0) and
`transfer_source` (`None`). Old Handoffs remain ordinary login or selection arrivals. The Native
Host's cipher and packet layouts do not change. See [Character Transfer](character-transfer.md)
for the ledger and Handoff ordering.

`game_character_profile` is a new private directory projection. Its revision orders name and faction
facts and deletion tombstones independently of delivery order. `game_character_profiles` and
`game_character_profiles_retry` carry and retry these projections. Existing contact and presence
row layouts are unchanged. Populate profiles on an existing Realm before opening gameplay; see
[contacts across World Shards](architecture.md#contacts-across-world-shards).

`game_creature_level_stats` holds final health, damage and armor by template and level.
`game_creature_display` holds weighted display choices by template. Both are new tables, so an
existing Realm migrates without changing stored template rows. A template stat change takes
precedence over level rows when its endpoint values no longer match the imported endpoints.
The presence of level rows for an entry preserves the imported stat convention even after an edit
changes the template endpoints; edited values never acquire classic rank multipliers. An authored
display or scale change likewise takes precedence over the imported display choices.
vmangos imports replace the level rows with selected source class-level stats.
`game_display_combat_reach` is a new table of combat reach by display id, filled by the creatures
import; until then every unit uses the default reach ([migration](danger-zones.md#melee-reach-migration)).

EventAI keeps the published rule's policy enums fieldless. The new private
`game_creature_ai_script_policy` table holds command-list boundaries and event admission policies,
keyed by definition revision and source rule id. Imports replace rules and policies together.
`game_creature_ai_relay_run.failure_policy` is an end-appended optional enum with a typed `None`
default, so old relay runs keep their failure behavior. See [command lists](data-ingestion.md#command-lists-in-eventai)
for execution and failure rules.

`game_character_quest_log` is a new private companion table with 20 quest entries per Character;
zero denotes an empty slot. An absent row projects legacy quests in quest-id order. The next quest
mutation saves these positions before changing the log. New quests use the first free slot;
swaps move positions and abandon leaves a hole. Reconnect and Character Transfer retain the row.
The public `game_character_quest` layout stays unchanged for Package compatibility.
A row's `rewarded` describes the current attempt. While a Character is on a rewarded repeatable
quest again, the private `game_character_quest_retake` row keeps its reward record for
prerequisites ([migration](danger-zones.md#quest-retake-migration)).

`game_quest_flags` is a new private companion table for the source `QuestFlags` bits returned in
quest queries. Absence means zero; reimport quests to populate an existing Realm. The public quest
template layout stays unchanged for Packages. Existing authored templates default to zero flags.
These stored bits do not implement the gameplay
rules for every flag; the import Compatibility Manifest continues to report that gap.

`game_quest_share` is a new private table holding each Character's Quest Share: the sharer and
quest pushed to it. A row ends when the receiver accepts any quest, fails to accept one, or
declines, and when either Character logs out, transfers or is removed from the Group; it stops
counting once the two Characters are not in one Group. A refused accept commits the end of the row
and the sharer's result, and leaves the quest log and bags unchanged. Only a quest whose
`game_quest_flags` row has the shareable bit can be pushed
([migration](danger-zones.md#quest-share-migration)).

`game_quest_emote` is a new private companion table for the source quest emotes: the emote the
giver plays while the quest is incomplete or complete, and up to four offer-reward emotes with
their delays. Absence means no emotes; reimport quests to populate an existing Realm. The public
quest template layout stays unchanged for Packages.

`game_quest_availability` and `game_quest_prerequisite` are new private companion tables for the
source quest availability rules. An availability row holds a quest's signed `ExclusiveGroup` and
its `MaxLevel`; absence means neither. Prerequisite rows hold the predecessors vmangos adds beyond
a positive `PrevQuestId`: a negative `PrevQuestId` and each quest whose `NextQuestId` names the
quest, with `must_be_active` for a negative link. `prev_quest_id` keeps the positive `PrevQuestId`,
so Packages can still set one. Reimport quests to populate an existing Realm.

`game_quest_template.reward_spell` is the source `RewSpell`, which the quest details, offer-reward and
quest-query packets show. A turn-in casts `reward_spell_cast`, or `reward_spell` when that is 0, as
a triggered cast on the Character after its other rewards and the turn-in hook, as vmangos
`Player::RewardQuest` does. A creature ender casts a spell with an `E_LEARN_SPELL` or
`E_CREATE_ITEM` effect or an effect whose imported target is `T_TARGET_ALLY`; otherwise the
Character casts it. vmangos checks the raw implicit target for 21 or 25, so for a few spells the
caster differs (the `reward_spell_caster`
[deviation](../conformance/features/quests/quest_reward.toml)). The retired
`game_quest_reward_spell` held the taught spell of a single-effect learn reward.

An `E_LEARN_SPELL` effect teaches a Character target its `trigger_spell` and that spell's dependent
spells, and teaches any other target nothing, as vmangos `Spell::EffectLearnSpell` does. A
grimoire's learn effect aims at the caster's pet, so it imports as `E_SCRIPTED` and teaches
nothing. An `E_CREATE_ITEM` effect that a creature casts on a Character fills that Character's
bags. Every item use first checks the item's class and race masks, its required skill and rank,
its required level and its required reputation rank, as vmangos `Player::CanUseItem` does. The
item's required spell and required honor rank are not imported.

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

`game_operator_spell_source` is a new private table keyed by spell id. It retains the editable
source fields committed with an Operator spell edit. See [spell edits](operator-content-api.md#spell-edits)
for replacement and import rules. Existing spell table layouts stay unchanged.

`game_world_instance_reset` and `game_instance_reset_result` are new private outboxes for manual
Instance resets. They start empty; no existing columns or IDC signatures change. Reset state uses
the existing `game_instance.reset_requested` field. See [Instance lifecycle](instance-lifecycle.md#manual-resets).

`game_instance_binding.world` is end-appended with `None::<Identity>`. A legacy `None` uses the
unchanged default map owner; new bindings record their hosting Shard explicitly. Transfer carries
this column with the binding. `game_instance_host` and `game_instance_placement` are new private
tables for host configuration and Realm placement. See the [allocation upgrade](instance-lifecycle.md#upgrade-an-existing-realm)
before enabling multiple hosts for existing leases.

## 2. Inventory

`game_item_aura` associates a runtime Aura with the Item that applied it. Aura deletion deletes
the association. Equipment changes reconcile these rows through `ItemInstance`; world entry,
resurrection and form changes use the same reconciliation. World entry also restores passive
effects for a ghost. Hidden passive effects use slot 255
and do not consume a client Aura slot. The association and its derived Auras do not travel in
Character Transfer. Finite equip Auras use `game_item_aura_expiry` to expire at their deadline,
independent of the general Aura sweep. World entry rebuilds them from equipment. Spell cooldowns
travel separately in `game_spell_cd` and keep their absolute deadlines.

`game_spell.form_mask`, `form_exclude`, `attributes_ex_2`, `visual_id`, `duration_index`, `category`
and `category_cooldown_ms` retain source fields needed by equip effects. The five
`game_item_template.spellcategory_*` fields override category admission for an Item cast.
`game_spell_cd.category` identifies the shared category owner; `category_ready_micros` keeps its
absolute deadline. `game_pending_cast.cooldown_category` preserves an Item's override, including
zero, through cast completion. `game_pending_cast.cast_item_guid` names the Item that started a
cast; zero means no cast item, and a row in flight at upgrade already used up its Item at start. A
channel whose Item is destroyed, sold, traded, mailed or auctioned sets it, and an
`item_target_guid` naming the same Item, to zero and runs on, as vmangos `ClearCastItem` does. Their
typed defaults preserve old rows during an additive publish. The five
`game_item_template.spellcharges_*` fields hold vmangos `SpellCharges`: zero is unlimited, and a
negative count is expendable. After the cast's `SMSG_SPELL_GO` and effects, the cast spends a
charge, and an expendable spell's last charge uses up one Item. See [equip spell
migration](danger-zones.md#equip-spell-migration) before play resumes.

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
| Combat, threat, duel and PvP | `combat/engage.rs`, `combat/damage.rs`, `combat/energy.rs`, `threat.rs`, `duel.rs`, `pvp.rs` |
| Spell and aura | `spell/tables.rs`, `spell/control.rs`, `spell/stacking.rs`, `spell/cast/resolve.rs` |
| Quest | `quest.rs` |
| Item, trade and mail | `items/tables.rs`, `items/buyback.rs`, `items/properties.rs`, `trade.rs`, `mail.rs`, `mail_reward.rs`, `mail_escrow.rs`, `mail_timer.rs`, `mail_text.rs` |
| Auction house | `auction.rs` |
| Creature (template, spawn, AI, pet, trainer) | `creatures/*`, `trainer.rs`, `gossip.rs` |
| GameObject and Summoning Ritual | `gameobject.rs`, `go_model.rs`, `go_collider.rs`, `summoning_ritual.rs` |
| Loot | `loot/*` |
| Group, Meeting Stones and Guild | `group.rs`, `meeting_stone/mod.rs`, `guild/*` |
| Instance and encounter | `instance.rs`, `encounter.rs` |
| Taxi, Transport and weather | `taxi.rs`, `transport/mod.rs`, `weather.rs` |
| Packages | `package_account.rs`, `package_config.rs`, `package_import.rs`, `package_teardown.rs`, `script_binding.rs` |
| Event GC | `gc.rs` |
| Config, static data and diagnostics | `config.rs`, `gm.rs`, `faction.rs`, `skilldata.rs`, `stats.rs`, `import_meta.rs`, `debug/*` |
| Gateway residue and Character Transfer | `realm_core.rs`, `gw.rs`, `transfer/mod.rs` |
| Retired tables | `retired.rs` |

`install_guid_range` in `auth.rs` installs the Shard's guid range. `realm_core.rs` keeps the
Gateway-era Realm locator, and `gw.rs` the Gateway Verbs that durable tests still call.
`transfer/mod.rs` keeps the Transfer ledger: native Transfer between World Shards runs through its
`apply_*` functions. Nothing writes the tables in `retired.rs`; they stay because dropping a table
is a destructive migration.

Two shapes recur:

- **Catalogue tables**: `game_item_template`, `game_spell`, `game_spell_effect`,
  `game_creature_template`, `game_quest_*`, `game_faction*`, the three `game_taxi_*` tables and
  other DBC-derived reference data. Only the importer writes them.
- **Event tables**: transient outbound signals that a row diff cannot carry, such as a movement
  Relay's animation opcode. The Module inserts them inside the transaction that caused them. The
  `*_live` tables are SpacetimeDB event tables and keep no rows; other event tables are reaped
  about one second later by their owning module's `sweep_expired`, which `gc.rs` runs. See [Native Relay events](./danger-zones.md#native-relay-events).

## 3. Tables with rules the struct cannot show

### Character selection routing (`module/src/native/directory.rs`)

`game_map_owner` has one World Shard per map. On the Realm Shard it routes creation; on each World
Shard it bounds creation and world entry. `game_character_start_map` holds only the Realm Shard's
race/class start-map routing facts.

`game_character_directory` contains live guid, Account and World Shard entries.
`game_character_directory_version` retains each guid's latest source revision after deletion.
Both belong to the Realm Shard and do not follow a Character-owned delete sweep. Revisions come
from the source World Shard's reducer timestamp; a source cannot replace a different owner.

`game_pending_selection` holds one request per connection with its deadline and expected World
Shards. It is removed on completion, failure or connection close. The same row holds a Character
login until its Handoff, and the 30-second deadline of a Handoff row that waits for adoption.
`game_selection_result` carries correlated replies and directory changes. The previous
`game_selection_reply` layout remains for additive migration. `game_selection_request.selection_id`
and `game_native_away.world` are typed, END-appended defaults; a legacy away row uses the default
World Shard.

`game_world_session_end` is a new private Realm outbox to `idc_end_world_session` on a World Shard.
`game_world_session_end_retry` is a new private scheduled Realm table with at most one row per
Account. It holds the end that a newer Login Queue ticket waits for, its 90-second deadline and the
resend time of a refused end. Both start empty. See
[the admission rules](architecture.md#host-listeners-and-admission) and
[the deploy order](danger-zones.md#world-session-end-migration).

`game_character_name` is a new private Realm table keyed by the lowercased name. A pending row
has guid zero. Its selection id is nonzero for native creation and zero for Package creation;
a completed row has its Character guid and selection id zero. The Account and World identify the creator. Confirmed deletion releases the row. It stays
on the Realm through Transfer and selection expiry. `game_character_names_ready` records World
Shards whose complete name report committed. `game_character_name_report` retains the current
report revision, expected batch count and accepted batch indexes until that World becomes ready.
All three tables start empty. See the
[upgrade procedure](architecture.md#upgrade-character-name-reservations).

### Package Character creation (`module/src/native/package_creation.rs`)

`game_package_character_request` retains each World's Package request and its Pending, Created or
Refused result until the Package consumes it. It has at most 64 rows per Package per Shard.
`game_package_creation_delivery` carries request, grant, created-profile and completion messages.
`game_package_creation_retry` retains refused result deliveries for retry while the target is a
Peer Shard; a refused Grant's Refusal retries regardless. Transport failures stay in the outbox.
All three tables are private and new. Request ids are local to the requesting World.
A grant failure rolls back World creation before its Realm callback releases the pending name.
Package Account allocation on the Realm includes both directory entries and pending reservations.
See [the Package contract](package-api.md#asynchronous-package-character-creation) for result
consumption and upgrade obligations.

### Package relocation (`module/src/native/package_relocation.rs`)

`game_package_relocation` retains the source request, entrance and authority snapshot, and result.
It is Shard-owned: source Character cleanup must not discard the completion that the Package polls.
`game_package_arrival` holds the exact Transfer and source that may release an imported body. It is
deleted with successful release. `game_package_relocation_delivery` and its retry table retain work
after source removal. These four private tables are new; existing table layouts and Host delivery
layouts do not change. See [Package relocation ordering](character-transfer.md#package-relocation).

### Package Groups (`module/src/native/realm_party/package.rs`)

`game_package_party_authority`, declared in `native/package_actor.rs`, is a private table on the
World and Realm. It retains the Character guid, World, authority revision and active state. Body and consent changes advance the
revision on the owning World; the Realm keeps it after presence becomes inactive to fence delayed
commands. Existing Group table layouts and IDC arguments are unchanged. Opcode zero in
`game_realm_party_command` carries a Package command and its authority. A zero connection alone
does not grant Group or guild authority. The Realm also requires Package Account ownership and
the current Character directory owner. See [the Package contract](package-api.md#session-less-action-consent)
for upgrade and completion obligations.

`game_package_party_refresh` is a new private scheduled table with at most one row per World. It
refreshes statistics for active Package Group members every five seconds, keeping their authority
revisions unchanged. Outstanding Package commands delay a refresh batch. The timer removes itself
when no active Package Group members remain. After upgrading an existing World, the Package Group
presence report starts the timer for existing membership.

### Package guilds (`module/src/native/package_guild.rs`)

`game_package_guild_delivery` carries World requests and Realm invitation projections through
`idc_package_guild`. `game_package_guild_retry` retains refused projection deliveries for retry;
transport failures remain in the outbox. `game_package_guild_invitation` retains one current
invitation or cleared revision per Character on its World. Cleared rows survive Character removal
to prevent a delayed delivery from restoring an old invitation. All three tables are new and
private. The existing `game_package_party_authority` layout is unchanged and fences both Group and
guild actions. See [the Package contract](package-api.md#sessionless-guild-membership) for explicit
acceptance, asynchronous completion and the presence report required after an upgrade.

### Realm communication (`module/src/native/realm_communication.rs`)

`game_native_away.character_guid` is END-appended with default zero. New world entries record the
Character guid. Existing away rows with zero cannot use Realm whispers or `/who` until logout and
world entry, or reconnect. This prevents replies reaching a different Character on the same
connection. A planned update closes existing World Sessions; development reload cannot add this
column.

`game_realm_communication_request`, `game_world_query`, `game_world_query_answer` and
`game_world_delivery` are IDC outboxes. `game_realm_query` joins replies from the configured World
Shards and expires after 15 seconds. `game_realm_delivery` holds a reply batch for one Account,
Character and connection, with a 15-second expiry; `game_realm_delivery_retry` schedules another
attempt only after a refused IDC. Successful delivery removes the pending batch. These tables
belong to the Realm communication flow and do not follow Character Transfer or deletion sweeps.
All new tables start empty on migration. See [Realm routing](architecture.md#durable-state-and-extension-points) for the
snapshot and delivery limits.

### Map catalogue (`module/src/config.rs`)

`game_map` stores selected vmangos `map_template` rows. `internal_name` alone comes from the
client's `Map.dbc`, joined by map id, and names the WDT/ADT archive directory. Missing client
maps refuse the import. The gameplay fields are `map_type`, `linked_zone`, `parent`, `name`,
`player_limit`, `reset_delay`, and `ghost_entrance_map/x/y`. A ghost map of -1 means
no entrance. Instance admission, reset schedules and ghost routing do not yet consume these fields.
This is a new table, so publishing it adds an empty catalogue without changing existing rows.

### Tavern AreaTriggers (`module/src/config.rs`)

`game_tavern_area_trigger` holds the selected vmangos `areatrigger_tavern` ids. Each id resolves
through `game_area_trigger` for its map and three-dimensional volume. The table is new, so an
existing Realm gains an empty catalogue until the world import runs again. Inn rest applies only
inside a selected trigger; a missing geometry row cannot grant rest.

### Taxi catalogue (`module/src/taxi/catalogue.rs`)

`game_taxi_node` comes from vmangos `taxi_nodes`. `game_taxi_path` and `game_taxi_path_node`
come from the Operator's `TaxiPath.dbc` and `TaxiPathNode.dbc`. These rows are public.
A node's `id` is its storage key
and `client_node_id` is its unique wire id in the client's fixed 256-bit taxi mask. Imported nodes
use their source id for both; reserved fixtures keep 509xxxx storage ids with bounded synthetic wire
ids. Paths are directed edges with their fare in copper; the reverse direction exists only when the
DBC has a second row. Path nodes keep explicit `(path_id, node_index)` order. `flags` stays the DBC's
signed `int32` container so every raw bit survives, and `delay_ms` holds the DBC's seconds in
milliseconds. The importer validates references, wire-id bounds and collisions, and duplicate
ordinals before it writes. It replaces nodes first, paths second and path nodes last, so a
successful rerun removes stale geometry. `own_mount_horde` and `own_mount_alliance` are end-appended
with default true and record whether the source node names a mount for that team itself. The mount
displays fill an empty team slot from the other team for flight, so only these flags tell the teams
apart. `in_taxi_network` is end-appended with default true; it is false only for a node that has
paths and every one is a spell path (`SPELL_EFFECT_SEND_TAXI`). The node-status query, the taxi menu and the
learn step share one lookup, as vmangos `GetNearestTaxiNode` does: the nearest node on the
creature's map that is in the network and has the Character's own team flag, however far.
Reimport the `dbc` family before play resumes; see the
[taxi team mount migration](danger-zones.md#taxi-team-mount-migration) and the
[taxi network migration](danger-zones.md#taxi-network-migration).

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
id. A node query, or a gossip menu with the taxi option, teaches the nearest qualifying node and
announces it; the taxi menu opens only on a known node. A status query only reads.

`game_active_taxi_flight` has at most one row per Character, because `character_guid` is its
primary key. A successful activation creates the row in the same transaction that deducts the
fare, so a duplicate activation sees the row before it can charge again. The Module queues the
activation reply before it arms the flight schedule. `game_taxi_flight_schedule` drives the 100 ms
route clock. The public `game_taxi_passenger_spline` row carries the remaining route to the owner
and AOI observers, and is refreshed when the passenger changes AOI cell or route point. The active
flight also records its takeoff position, so a new viewer's CREATE carries the original spline path
with elapsed time even after the passenger has crossed a cell boundary.

A multi-hop flight keeps its first leg in `path_id` and each later leg in a row of the private
`game_taxi_flight_leg` table, ordered by `leg_index`. `destination_node_id` is the final node.
Activation debits only the first leg's fare. The route clock pays each later leg as the flyer
enters it. `current_leg_index` is the leg the flyer is on; that leg and every earlier one are
paid. A login resume rewrites the row to its current leg and deletes the leg rows, because vmangos
saves only that leg.

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
  never to `game_creature_spline`. That row, not the flag, decides whether a Character is in flight.
- **Land mount** owns it the rest of the time, as a projection. The `A_MOUNTED` aura row is the
  mounted state, and `mount::recompute_mount` derives `mount_display_id` and `run_speed_mult_bp`
  from the current aura set. Every aura removal path ends in that one recompute. A land mount never
  sets `UNIT_FLAG_TAXI_FLIGHT`, and both mount entry points refuse a Character in flight.

`run_speed_mult_bp` holds the effective multiplier from the shared move-speed fold while a land
mount is active (16000 for a nominal 60% mount). A change becomes `SMSG_FORCE_RUN_SPEED_CHANGE`.

`power` and `max_power` are the shown pool, whose power type is `unit_bytes_0` byte 3. While a Form
shows a pool of another power type, `reserve_power_type`, `reserve_power` and `reserve_max_power`
hold the unit's own pool, the Reserve Pool; `reserve_power_type` 255 means the unit holds none.
`world::power_pool` keeps the pair consistent. A change to either pool, or to the power type, goes
out in one partial VALUES mask to the owner and peers, as vmangos marks every `UNIT_FIELD_POWERn`
and `MAXPOWERn` public. A creature is the exception: its CREATE writes no power fields and its
current power is never sent, so only a later change to its max power reaches a viewer
(`character_visibility.toml` records the gap). A Character's CREATE carries both pools. Logout
saves the own pool to `game_character.power`; a Form's pool is not saved and starts empty when
world entry shows it again. A pool that appears sends its current value, so the client
never keeps the full energy that Character CREATE writes. A spell cost, an energize and an
energize tick reach the pool of their power type, shown or reserved; an energize skips a pool whose
max is 0. A cost in a power type the unit holds in neither pool is refused with `NoPower`, as
vmangos `Spell::CheckPower` does. A creature is never refused for a cost and never pays one. A
cost in health (power type 254, vmangos `POWER_HEALTH`) is paid from health and refused with
`CasterAuraState` when health is at or below it. Only the up-front cost is modelled, so Health
Funnel's per-second health cost is a known gap, as is any cost a creature or pet pays. A cast an
item starts pays no cost and is never refused for one. Resurrection, corpse reclaim and the GM
heal set every pool the unit holds by its power type; a resurrect request sets mana by its
percentage, energy full and rage to 0.

Hunter pets keep a durable identity in `game_hunter_pet`. A materialized pet is an ordinary owned
`game_world_entity`, classified by `game_live_pet_kind`; summoned demons have no Hunter identity or
care state. Taming retires the wild entity and arms its authored `game_creature_spawn` for a later
respawn, so the spawn point never moves into the pet identity. Tame, Call Pet, Revive Pet and world
entry build the live pet from the durable row through one builder: Revive Pet at its effect's
percent of maximum health, the others with the saved health and power, or at full health when none
are saved.

`game_hunter_pet` end-appends `presence`, `display_id`, `saved_health` and `saved_power`, each with
default 0. `presence` is 1 while the pet is out, including across logout and Transfer, so world
entry rebuilds it; 0 after a dismiss, a spirit release or the owner's death, so only Call Pet brings
it back; 2 once it dies, so only Revive Pet does. Abandon, and a summon with a creature entry over a
live Hunter pet, delete the row. `display_id` is the pet's model; 0 until the first build, which
stores the model it chose. `saved_health` and `saved_power` hold the vitals of a living pet that
left the world; 0 health means none are saved, and each build clears them. A Transfer carries all
four columns under the `game_hunter_pet@presence-1` tag; see the
[Hunter pet presence migration](danger-zones.md#hunter-pet-presence-migration).

### Forms (`module/src/form/`)

A Form is data composed from building blocks the Module defines in Rust. `game_form` holds one row
per Form, keyed by its own `form_id`, and `game_form_spell` holds the passive spells the Form grants.
The `spell` import writes them for the 1.12 player Forms: the warrior stances, Cat, Bear, Dire
Bear, Travel, Aquatic and Moonkin Form, Ghost Wolf, Shadowform and Stealth.
`debug_seed_warrior_forms` and `debug_seed_druid_forms` write the same warrior stances and feral
druid Forms for tests.
A new Form needs rows, not Rust: a Package adds one through its Package Delta, at a `form_id` in the
Package Form Range (`docs/package-api.md`).

Imported Forms keep the ids of the 1.12 client's `SpellShapeshiftForm.dbc`, and the `spell` import
replaces only the Forms it writes. A Package Form sits in a band no import reaches, so a reimport
leaves it, its passives and its Form Set rows in place.

- **Power rule.** `power_type` names the pool the Form shows and `power_on_entry` is a
  `lyracore_shared::form::PowerOnEntry` code: keep, zero, or keep up to the unit's
  `A_FORM_POWER_RETAINED` amount (Tactical Mastery for the warrior stances). A Form whose pool is
  not the unit's own holds the own pool as the Reserve Pool and shows its pool from empty, as
  vmangos `Unit::SetPowerType` does; the rule then runs on the shown pool. Leaving the Form ends its
  pool and shows the own pool again. `power_type` `lyracore_shared::form::OWN_POOL` (255) shows the
  unit's own pool, whatever its power type, as vmangos `Player::InitDataForForm` does in every Form
  but Cat and the bears: a rogue keeps energy in Stealth, and a druid shows mana in Travel, Aquatic
  and Moonkin Form. A nonzero
  `entry_chance_spell` is cast on entry at the percent chance of the unit's best
  `A_FORM_ENTRY_CHANCE` Aura whose spell's Form Set requires the Form, as Furor gives Cat Form
  energy and the bear Forms rage. Such a spell must carry
  `SPELL_ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED`: otherwise its passive is bound to those Forms, ends
  when the unit leaves one and is not yet back when the next entry reads the chance. A Package plan
  that breaks this rule is refused. Such a passive is held, and castable, in any Form.
- **Combat profile.** `natural_attack_ms` zero keeps the weapon profile. Otherwise the unit sets its
  weapons aside: both hands swing every `natural_attack_ms` for level-based damage, weapon skill is
  the level cap, a druid has no ranged attack power and weapon-bound spell bonuses stop. With
  `attack_power_from_agility`, a druid's melee attack power adds Agility, as in Cat Form; the
  druid's Predatory Strikes adds to it with natural attacks.
- **Lifecycle.** `default_for_class` puts a unit of that class with no Form into the Form at world
  entry, through the Aura of `spell_id`. An Aura survives death by its spell's attributes, the
  same rule as every other Aura, so the druid Forms end at death. `clears_snares` ends the unit's
  roots and movement slows when it enters or leaves the Form, as vmangos Shapeshift Form Effect 9033
  does: a root with any mechanic, and a slow whose mechanics a shapeshift does not keep (stun, daze,
  fear and the other crowd control in vmangos `MECHANIC_NOT_REMOVED_BY_SHAPESHIFT`).
- **Spell rules.** `acts_as_shapeshifted` decides whether spells treat the unit as shifted. The
  1.12 form spells are `SPELL_ATTR_NOT_SHAPESHIFT`, so a druid in one feral Form cannot cast
  another: the client sends `CMSG_CANCEL_AURA` for the current Form first, and the same request is
  how it leaves a Form. A spell's Form Set can exclude a Form whatever `acts_as_shapeshifted` says,
  as the Holy spells exclude Shadowform, and require one, as Ambush and the other openers require
  Stealth.
- **Mounting.** A Form that acts as shapeshifted, or sets `blocks_mounting`, refuses mount spells
  and taxis, as vmangos `Unit::IsInDisallowedMountForm` does. Moonkin Form needs the column: spells
  treat it as unshifted, yet it blocks mounting.
- **1.12 client presentation.** `client_form` is the form byte the 1.12 client reads from
  `UNIT_FIELD_BYTES_1`. It is a projection, never the Form's identity, so a Form a client does not
  know can name a byte it does. A Package Form's byte is its fallback on the 1.12 client, and the
  Module never reads a byte back as that Form: a byte names only the Form whose id it is.

`game_spell_form` holds the Forms a spell requires, or excludes when `excluded`. Its rows are the
spell's Form Set, so a Form id above the 1.12 client's 32-bit `Stances` mask can gate a spell. The
importer, Package spell rows and Operator spell edits write rows for Forms 1 to 32 from the DBC
masks, which `game_spell.form_mask` and `form_exclude` keep verbatim; rows for higher Form ids stay.
The one exception is Furor, whose `form_mask` the importer extends with Cat, Bear and Dire Bear to
scope its entry chance.
A Package names its own Forms in a spell's Form Set with `game_spell_form` rows.
`game_spell.stances` is no longer read.

A unit's only Form state is its one `A_SHAPESHIFT` Aura, whose `eff_p0` is the `form_id`. The
Aura's events drive the Form:

- **Apply** (`form::entered`): the unit leaves any other Form, the Form's pool is shown and the
  power rule runs.
- **Refresh** (`form::refreshed`): as vmangos removes and reapplies the modifier, the unit's
  Form-bound Auras end, then the Form is entered again, power rule included.
- **Removal** (`form::exited`): the unit's own Auras that need a Form end, as in vmangos
  `HandleShapeshiftBoosts`. A unit left in no Form gets the shapeshift effect's `trigger_spell`;
  Nefarian's Berserk 23397 ends into Berserker Stance this way. Breaking stealth ends every Aura of
  the stealth spell, so Stealth's Form and slow end with its `A_STEALTH` Aura.

The Aura change then projects once: `form::project_form` re-derives the client form byte, the shown
pool, the passives the Form brings and the equipment that depends on the Form. The passives are the
Form's `game_form_spell` rows and every known passive whose Form Set requires a Form with a
`game_form` row, such as Defiance or Sharpened Claws. A passive whose Form Set names only Forms
without a row, such as Tree Form, keeps the Aura its learning applied. The projection
removes only Auras the unit cast itself; equipment Auras stay with the equipment reconcile. A spell
with `CAST_FLAG_LOCKS_FORM`, Berserk among them, refuses every shapeshift cast while its Aura is
held.

Every Character body that enters the world runs the same projection, and one with no shapeshift Aura
enters its class's default Form, whether it came from login, a Package or a sessionless action.
Publish repair projects too, and puts a unit with no shapeshift Aura into the Form its client form
byte shows, or else its class default; a dead or ghost unit enters no Form its death would end.
`WorldEntity.stance` is a retired column: nothing reads it.

### `game_character` (`module/src/character.rs`)

Public. It exists whether or not the Character is online, so an offline Character costs no live
world machinery. `owner_identity` is `Identity::ZERO` until `establish_session` binds it.
`rest_state` records the client rest-state byte. Login seeds it from normal and rested XP.
WorldPort keeps the existing state, so hysteresis lasts through map changes in one World Session.

#### Drunk value

A Character's live Drunk Value is bits 1 to 15 of `player_bytes_3` on its `game_world_entity` row;
bit 0 is its gender, so the value is even and at most 65534, as vmangos saves it. A change reaches
every viewer as a `PLAYER_BYTES_3` VALUES update, and the 1.12 client draws the drunk tier, the
screen blur and the "You feel tipsy" lines from it.
`game_character.drunk_value` keeps the value while the Character is offline.

An `E_INEBRIATE` effect adds 256 per effect point, up to 65534, as vmangos `Spell::EffectInebriate`
does. It affects only Characters. `game_drunk_sobering` holds one timer for each drunk Character in
the world. The breath tick reads it every second and takes 256 every 10 seconds, as vmangos
`Player::HandleSobering` does. A cross-map WorldPort keeps the timer while the entity is away; a
logout or login drops it. Another drink keeps the running timer. Death sets the value to zero.
A login, but not a WorldPort, scales the stored value down in a straight line to zero over the 15
minutes since logout, as vmangos `Player::LoadFromDB` does; with no logout time it keeps the value.

The 1.12 client slurs a drunk Character's own chat, so the server sends say and yell text unchanged.
vmangos has no `SMSG_CROSSED_INEBRIATION_THRESHOLD`; that packet arrived after 1.12. vmangos also
lets a drunk Character see units with invisibility type 6; LyraCore has no invisibility yet.

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

`game_account_fence` and `game_account_character_owner` are retired Gateway tables in
`module/src/retired.rs`, and nothing writes them. Module tests and the native check still open Account Claims in
`game_account_claim`. Operator reducers that act for a Character take a `SessionActor`; a null
ownership refuses a Character with an active Account Claim.

### Character Migration (`module/src/character_migration.rs`)

Private. `game_character_migration` is keyed by `<Migration Source label>/<source guid>` and names
the Character guid that the migration created. It is classified as not Character-owned: deleting the
Character keeps the row, so a rerun of the migration does not recreate the Character. See
[Character Migration](./character-migration.md).

`game_mail_migration` is private, keyed by `<label>/<source mail id>/<source item guid>`, and names
the Mail it created. Deleting the Mail keeps the row.
`game_contact_migration` is private and keyed by the owner's `<label>/<source guid>`. It marks a
contact list as applied, so removed contacts stay removed after a rerun.
`game_guild_migration` is private, keyed by `<label>/<source guild id>`, and names the Guild it
founded.

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

`game_channel_member`, `game_channel_event` and `game_whisper_event` in `module/src/retired.rs`
are retired. They stay because dropping a table is a destructive migration. Nothing writes them.

### Buyback (`module/src/items/buyback.rs`)

`game_buyback_item` holds a sold Item's complete instance and any paused Temporary Enchantment.
It is private. The Item has no `game_item_instance` row while it is in buyback, so slot lookup,
equipment, storage, uniqueness, quests, item use, trade, Mail, Auctions, bank access, durability
and repair cannot treat it as inventory. GUID allocation and Transfer import check both tables
for collisions. Letter Copy grant retries also check buyback to avoid a duplicate copy. A full
sale keeps the GUID; a partial sale creates a new stack with the reference
clone rules. Restoration keeps that GUID unless the stack merges completely into existing stacks.
Merging keeps the destination stack's property and binding. Buyback sends no item receipt.
Sale and restoration write ordinary Item rows through named `ItemInstance` methods with their own
delivery. A sale removes equipped armor and resistance bonuses from the client's fields. It pauses
Temporary Enchantments instead of forgetting them. It does not close a Guild Charter's Petition:
the Reference Server removes the Item from inventory without destroying it.

`game_buyback_state` stores the login time and current slot. Slots 69 through 80 retain their
positions when another slot is bought back. A full ring selects the oldest timestamp, with ties
going to the lowest slot. The timestamp field is 108000 plus whole seconds since login. It is a
client field and does not drive an expiry timer, as in the pinned Reference Server. Eviction
destroys the Item and releases its Item Text. Temporary Enchantments retain their remaining duration and
charges until restoration.

Logout and Transport Loss discard buyback. Every fresh login also clears it before building the
client view, including after a crash with no previous live body or World Session. Character deletion
and Package Teardown clear it, including teardown of a Character whose body is already absent.
Both buyback tables travel in the Character Transfer manifest. A Handoff or World Port preserves
the same World Session: ring positions, prices, timestamps, `login_at`, `current_slot`, complete
Items and paused Temporary Enchantments stay intact and replay on arrival. Buyback Items still
have no ordinary inventory row. Import checks GUID collisions across both tables and the whole
arriving payload. Nested Item ownership is restamped at entry. Item Text travels with its
references and receives local ids; the source deletion sweep releases each buyback reference once
and sends no destroy delivery for transported Items. Fresh login discards transported buyback,
including after a sessionless Transfer; Package relocation clears it while making its source dormant.

Both buyback tables are additive. The old `game_character_buyback` declaration and row layout stay
for migration compatibility; no gameplay reads its snapshots. `debug_repair_after_publish` discards
those rows and ends connected World Sessions without buyback state, so the client discards legacy
objects and fields and a fresh login establishes the timestamp origin. Sessions with buyback state
remain connected, including Sessions adopted by Handoff. Native World Port import refuses a
missing buyback session row before adoption. Login also clears the Character's legacy rows. See
[the buyback migration](danger-zones.md#buyback-migration) before updating an existing Realm.

### Mail (`module/src/mail.rs`, `mail_timer.rs`, `mail_text.rs`)

`mail::store_letter` allocates Mail ids for new and arriving letters. It also reserves ids for
imported Item Text, so text and future letters cannot collide. The END-appended header columns carry what the 1.12 inbox reads: `sender_kind`
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
deleted. The count can be too high, never too low. Destroying a Letter Copy or discarding it from
buyback releases its reference. A sale retains the reference until the Item is restored or discarded.
`game_item_instance.item_text_id` is 0 for every item that is not a copied letter, and
every item snapshot (mail, trade, auction) keeps it.

`game_mail_forward` carries a letter and its attachment text through IDC; `game_mail_forward_retry`
retains refused deliveries and resolves the recipient's current World again on the Realm. Both are
private and survive Character logout, deletion and Transfer. Resident inboxes use `game_mail`.

`game_mail_escrow` and `game_mail_delivery` hold legacy cross-Shard Mail value. World Session mail
uses the forward outbox, and a Reward Letter inserts or forwards its Mail directly
(`module/src/mail_reward.rs`).

### Repair cost (`module/src/items/tables.rs`)

`game_durability_cost` holds `DurabilityCosts.dbc` as one row per item level and multiplier column:
column 0..=20 is a weapon subclass and 21..=28 is an armor subclass. The key `id` is
`item_level * 32 + multiplier_id`.
`game_durability_quality` holds `DurabilityQuality.dbc`; an item quality reads row
`(quality + 1) * 2`. Only the importer writes both. See
[repair cost migration](danger-zones.md#repair-cost-migration).

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


In a sharded Realm, `game_auction` and its authoritative receipts and expiries live on the Realm.
World receipts retain accepted sell identities for repeated packets; they do not own a Listing.
`game_auction_transaction` is private World state keyed by operation id and indexed by Character.
It retains the request, reserved copper and eventual outcome until settlement completes.
`game_realm_auction_request` and `game_world_auction_answer` are private IDC outboxes;
`game_auction_retry` schedules refused answer deliveries and refunds that still need purse space.
`game_auction_deletion` fences late Auction requests until a pending Character deletion finishes.
These five tables are additive. Existing row layouts remain unchanged. The migration and failure
contracts are in [Auctions across World Shards](architecture.md#auctions-across-world-shards).

### Guilds (`module/src/guild/`)

`game_guild` is one Guild: its name, a unique lower-case `name_key`, the leader, the team fixed at
founding, the MOTD, the info text and the emblem. `game_guild_rank` holds its five to ten Guild
Ranks with their Rank Rights. `game_guild_member` is keyed by Character guid, so a Character is in
at most one Guild; each row keeps a name snapshot. `game_guild_event` is an event table where
`recipient_guid == 0` goes to every online member and a nonzero value addresses one Character.
`game_guild_invite` is one pending offer per target Character, reaped on `INVITE_TTL_MICROS`.
`game_guild_petition` is one open Petition, at most one per owner, tied to its Guild Charter item
guid. `game_guild_petition_signature` is keyed by `petition_signature_key(petition_id, slot)` for
slots 0 to 8. Turn-in deletes both and founds the Guild in the same transaction. These rows are
Realm-owned in a split topology. `game_guild_petition_sequence` keeps its highest allocated or
imported petition id, including after a petition closes. The typed `GuildOp` gets each new variant
at the end, because BSATN encodes a variant by its position.

PLAYER_GUILDID and PLAYER_GUILDRANK are not stored. The Module projects them from
`game_guild_member`. With a Realm Shard, guilds, ranks, memberships and invitations are
Realm-owned; World Shards hold guild, rank and membership projections. `game_guild_sequence`
keeps the highest allocated or imported Realm guild id, including after disband.

`game_guild_character` holds current connection facts, roster fields and private ignore snapshots
on the Realm. Presence reports include stored roster fields for offline members; the Operator
backfills them before guild import. `game_guild_projection` retains guild revisions and disband records.
`game_guild_membership_projection` retains each membership's own revision and optional member row;
a missing member is a deletion record, not permission to restore an older roster. These records
survive Transfer. `game_realm_guild_command`, `game_realm_guild_presence` and
`game_world_guild_update` are IDC outboxes; `game_guild_projection_retry` retains refused snapshot
deliveries. See [the guild migration procedure](architecture.md#guilds-across-world-shards) before
using these tables on an existing Realm.

`game_guild_transaction` records one unsettled World request per Character, its outbox call id,
operation, debited copper and optional result. `game_guild_transaction_retry` schedules closure,
reconciliation or remaining-refund retries. Transfer and deletion refuse while this state exists;
it never travels in a Character export. Successful settlement deletes it. The existing guild
command IDC carries these typed operations under internal opcode one, leaving its published
arguments unchanged.

`game_guild_fee_hold` and
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

### Trainer offering gates (`module/src/trainer.rs`)

`game_trainer_spell.required_skill` and `required_skill_value` carry the vmangos `reqskill` and
`reqskillvalue`. An offer with a `required_skill` is red, and its buy refused, while the Character's
base skill in that line is below the value. The base skill is the stored value plus the permanent
bonus from `A_MOD_SKILL` Auras, as vmangos `GetSkillValueBase` counts it; a temporary bonus does not
count. A Character holds at most two primary professions
(`lyracore_shared::trainer::PRIMARY_PROFESSION_LINES`). The first rank of a primary profession is
the offer with `learn_skill_cap` 75. The list sends it green with no learn button, and the buy is
refused, once two primary skill rows exist. The count is derived from `game_player_skill`;
`PLAYER_CHARACTER_POINTS2` carries the free count to the client. Every refusal of these gates is
sent as `TrainingFailureReason::NotEnoughSkill`, as vmangos does.

### Proc data (`module/src/spell/tables.rs`)

A Proc is an aura that fires off a combat event. Its rules live in three places: two are static
data the importer writes, the third is a per-aura snapshot.

`game_spell` carries the selected vmangos `spell_template` fields: `proc_flags`, the vanilla
`procFlags` bitmask with no translation table, `proc_chance` (a whole percent) and `proc_charges` (0 means unlimited).

`game_spell.cast_flags` retains party and pet area-aura origins as `CAST_FLAG_AREA_AURA`.
Their translated target can be self, so slot visibility must use this flag instead of inferring
the source effect from the target. Ordinary passives use slot 255; visual passives, recipient
area auras and totem auras can use visible slots. Repair applies the same rule to stored cast auras.

`game_spell_proc_event` carries the source proc-event overlay: `ppm_rate`,
`icd_ms`, `proc_ex` (the hit result that fires it, or `PROC_EX_TRIGGER_ALWAYS`), the `school_mask`, `family_name` and
`family_flags` filter on triggering spells, a `proc_flags` override and a `custom_chance`. It is
keyed by spell id, private, and absent for most spells; an absent row means "use the header's
values". A higher rank without its own row takes its first rank's row through `game_spell_chain`. The `spellmeta` dump family loads it, and its clear stops at the reserved synthetic id
floor so the seeded Proc fixtures keep their overlay rows.

`game_enchant_proc_rate` (`module/src/spell/chance_on_hit.rs`) carries the vmangos
`spell_proc_item_enchant` rate: the procs-per-minute of an enchantment's Chance on Hit spell, keyed
by its first rank. Higher ranks take the first rank's rate through `game_spell_chain`. It is private. The `spellmeta` dump family loads it, and packages cannot claim it.

`game_spell_enchant_charges` (`module/src/items/temporary_enchantment.rs`) carries the vmangos
`spell_enchant_charges` count: the charges a temporary enchantment spell puts on its item. An absent
row means no charge limit. It is private. The `spellmeta` dump family loads it, and packages cannot
claim it.

`game_spell_threat` (`module/src/spell/tables.rs`) carries the vmangos `spell_threat` row: a spell's flat
`threat`, the `multiplier` on its damage and heal threat, and the `inverse_effect_mask` of effect
indexes whose targets skip the flat threat. A rank without its own row takes its first rank's row
through `game_spell_chain`; a later rank's own row with no flat threat is ignored, as in vmangos. A
harmful spell adds the flat threat to each creature it lands on. A helpful spell splits it among the
creatures fighting each target, as heal threat is split. A spell with both kinds of effect adds
none. It is private. The `spellmeta` dump family loads it, and packages cannot claim it.

`game_temporary_enchantment` (`module/src/items/temporary_enchantment.rs`) holds one Temporary
Enchantment per item: `item_guid`, `enchant_id`, `expires_at` and `charges` (0 means no limit). It is
a private scheduled table. Its schedule fires at `expires_at` and deletes the row; replacing,
spending the last charge or deleting the item deletes it earlier.

`game_item_spell_charges` (`module/src/items/spell_charges.rs`) holds the item spell charges one
Item has spent, by `item_guid` with its `owner_guid`. No row means the Item holds its template
charges. Only an Item that cannot stack keeps a row, as vmangos stores charges only on such an
Item. It is private and Character-owned; deleting the Item deletes it.

`game_gameobject_ritual` (`module/src/gameobject.rs`) holds a type-18 template's vmangos
summoning-ritual data: the participants it needs, the spell its owner then casts, the channel spell
of the other participants, and whether it persists and needs grouped participants. The
`gameobjects` dump family loads it. `module/src/summoning_ritual.rs` keeps the live state in private
tables: `game_summoning_ritual` (one placed object, its owner, the owner's selected summon target
and its phase), `game_summoning_ritual_participant` (one channel per Character),
`game_summoning_ritual_schedule` and `game_summon_offer` (one pending Summon Offer per target).

`game_aura` carries the frozen profile: `proc_flags`, `proc_chance`, `proc_ppm`, `proc_ex`,
`proc_school_mask`, `proc_family_name`, `proc_family_flags`, `proc_charges`, `proc_icd_ms`,
`proc_ready_micros`. `spell::proc::freeze_profile` folds header and overlay together once, at
apply, for every Aura, so the proc pass reads one row. The overlay wins where both carry a value;
charges come from the header, plus the caster's `SPELLMOD_CHARGES` modifiers. `proc_charges` and `proc_ready_micros` are mutable. The proc pass
treats the effect rows of one spell from one caster and one item as one Aura Holder: one roll, and one charge spent
for all its rows, whether a Proc fired or an Aura such as Inner Fire only counted the event. A
refresh freezes the other columns again and refills charges, and leaves a running cooldown alone.
That is why `debug_repair_after_publish` freezes profiles again: an Aura applied before its profile
was frozen reads as "never procs, no charges", and a permanent self-buff never refreshes on its
own.

### Global cooldowns (`module/src/spell/tables.rs`)

`game_spell.gcd_ms` and `gcd_category` are the source `StartRecoveryTime` and
`StartRecoveryCategory`. A caster has one global cooldown per category, as vmangos `AddGCD` keeps
one. A cast starts one in its spell's category when `gcd_ms` is non-zero, and the Gate refuses a
cast only while its own category's cooldown runs. Most spells with a global cooldown are in
category 133 and most spells without one in category 0, so the latter can be cast during a
category 133 cooldown. Cancelling a timed cast clears only its category.

`game_global_cooldown` holds one row per caster and category, keyed by `by_caster_category`. It is
public and transient: a row lingers after `ready_at` and is overwritten by the next cooldown in its
category. It replaces `game_spell_cooldown`, which kept one row per caster and is retired. Packages
claim `gcd_category`; a Package Delta without it gives a new spell category 0.

### Interrupt flags

`game_spell.interrupt_flags`, `channel_interrupt_flags` and `prevention_type` are the source
`InterruptFlags`, `ChannelInterruptFlags` and `PreventionType`, kept verbatim. The Module reads these
bits, as vmangos `Unit::DealDamage` and `Spell::EffectInterruptCast` do:

- A direct hit on a Character cancels its cast in progress when `interrupt_flags` holds 0x10, and
  otherwise pushes it back when it holds 0x02. A creature's cast is never pushed back.
- A direct hit on a Character from another unit shortens its channel when `channel_interrupt_flags`
  holds 0x4000: by 1000 ms, then 200 ms less for each later hit, down to 200 ms. The channel's
  Auras and ground areas end that much sooner, their next tick moves so the last one falls on the
  new end, and the channel ends when no time remains. Without 0x4000, a hit on any unit cancels its
  channel when the flags hold 0x02.
- An interrupt effect stops only a spell whose `prevention_type` is 1 (silence): a cast whose
  `interrupt_flags` holds 0x02, or a channel whose `channel_interrupt_flags` holds 0x04.

Periodic damage, including a ground-area tick, neither pushes back nor shortens anything.

The defaults, 2, 4 and 1, keep the behaviour from before the columns existed: damage pushes back a
cast and leaves a channel running, and an interrupt effect stops every cast and channel. A row
written before the reimport, a test fixture and a Package Delta that omit the fields keep that
behaviour, so none becomes uninterruptible. Packages claim all three columns.

`game_channel_delay` holds one private row per caster: the channel's spell, its end after the last
delay, and the number of delays. Breaking or ending the channel deletes the row. A row left by a
channel that ran out is harmless: a channel that no longer ends at that time starts again from no
delays, and the next delay overwrites the row.

### Combo finishers (`module/src/combo.rs`)

A finisher is a Rogue or Druid spell whose `Spell.dbc` AttributesEx carries 0x0010_0000 or
0x0040_0000. The importer marks its `game_spell.cast_flags` with `CAST_FLAG_COMBO_FINISHER`.
A Character cannot cast one without combo points, on the target for an enemy-targeted finisher and
anywhere for Slice and Dice. A cast spends the points unless it misses its target.

`game_spell_effect.points_per_combo` is the DBC `EffectPointsPerComboPoint`. Each effect adds it
once per point the caster holds on the effect's target. `game_spell.max_duration_ms` is the DBC
maximum duration. A finisher's aura grows from `duration_ms` toward it by a fifth per point.
Physical finisher damage imports as `E_FINISHER_DAMAGE`. Eviscerate, Ferocious Bite, Rupture and Rip also add the 1.12
attack-power share that vmangos gives them. The Package API does not claim either new column.

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
| 26 | Combat spell: a Chance on Hit, no stat. `spell_id` is the spell. `amount` is its percent chance when `game_enchant_proc_rate` has no rate; zero means one proc per minute |

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
| `game_taxi_flight_schedule` | `advance_taxi_flight` | 100 ms while a passenger is active | `taxi.rs` |
| `game_transport_carry_schedule` | `carry_transport_passengers` | 500 ms per Transport while it has Passengers, armed by boarding | `transport/mod.rs` |
| `game_transport_crossing` | `cross_transport_map` | one-shot at a route's next map change, armed by `init`, `debug_repair_after_publish` and each firing | `transport/mod.rs` |
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
| `game_pvp_flag_timer` | `expire_pvp_flag` | one-shot 5 min after a Character's PvP Flag was last refreshed; a held flag re-arms it | `pvp.rs` |
| `game_auction_expiry` | `expire_auction` | one-shot at listing expiry | `auction.rs` |
| `game_instance_removal` | `expire_instance_removal` | one-shot 60 s after an Instance Removal starts | `instance.rs` |
| `game_mail_timer` | `fire_mail_timer` | one-shot at a Mail's delivery, then at its expiry | `mail_timer.rs` |
| `game_summoning_ritual_schedule` | `advance_summoning_ritual` | one-shot at a Summoning Ritual's spell completion, else at its channel's end | `summoning_ritual.rs` |
| `game_creature_ai_summon_expiry` | `expire_eventai_summon` | one-shot at summon lifetime end | `creatures/eventai/mobility.rs` |
| `game_creature_ai_forced_despawn` | `fire_eventai_forced_despawn` | one-shot at the authored despawn time | `creatures/eventai/mobility.rs` |
| `game_creature_ai_relay_continuation` | `resume_relay_run` | one-shot at the authored relay delay | `creatures/eventai/relay.rs` |
| `game_creature_ai_relay_arrival` | `resume_relay_arrival` | one-shot on movement completion | `creatures/eventai/relay.rs` |

`init` inserts the interval rows (`arm_interval_schedules` in `module/src/seed.rs`). The transfer,
mail-escrow, session and native logon reapers, the native admission and member-stat ticks and the
Transport carry schedules are armed by their own entry points, lazily and idempotently. Scheduled
reducers check `ctx.sender() == ctx.database_identity()`, so nothing outside can drive them.

In a conformance build the virtual clock drives every scheduled table that is not marked
`virtual_clock_exempt!`. The build generates that registry; see
[Module build](./module-build.md).

`init` does not run again on an auto-migrating publish, so a schedule row can go stale.
`debug_repair_after_publish` arms the same interval schedules as `init`, from one list in
`module/src/seed.rs`. Call it after every publish; see
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

### Equip aura support

`game_world_entity.sheet_ap_modifiers` and `sheet_ranged_ap_modifiers` cache the wire AP
breakdown. The low signed-16 half is the positive modifier and the high signed-16 half is the
negative modifier. They are END-appended with zero defaults and refreshed by publish repair.
The ranged base AP stays separate and exists for every Character, with or without a launcher.
Damage uses the signed total scaled by `1 + multiplier`, where `sheet_ap_multiplier` and
`sheet_ranged_ap_multiplier` hold the multiplier the client receives; both CREATE and VALUES
retain the individual modifier halves. `sheet_ap_base` and `sheet_ranged_ap` store the bits of a
signed base. Priests, mages and warlocks ignore ranged AP Auras, flat and percent.

`game_item_aura` retains the exact BSATN source definition for each active derived aura. Repair
compares it with the current header and effect, so unchanged auras keep their identity and timing.
The source bytes stay local to the Shard and are excluded from Character records and Transfer.

`game_spell_effect.equip_supported` defaults to false. The spell importer enables only source
shapes whose units, selection and consumer have been audited. Existing and authored rows stay
inert on equip until reimport or an explicit supported claim. This does not enable instant effects.
At patch 10 / build 5875, 2,645 Item entries name 640 equip spells. Of these, 2,643 entries
name 638 spells with the 52 aura types below; two entries name spells without an aura type. Counts
are distinct Item entries per type and overlap. One weapon-dependent crit Item stays inert; the
other 383 use the shared melee/ranged crit bonus. One area stat Item, one area spell-crit Item and the area damage aura on Atiesh
also stay inert. Only ordinary self-targeted effect-6 shapes are enabled. Healing retains its
school mask in `A_MOD_HEALING`. This is a source-to-consumer audit, not a claim that every
spell or baseline combat calculation is complete.

Unsupported cast effects import as `A_INERT` (0xB9). They keep their Aura holder, icon, duration,
refresh, stack limit and dispel behavior; no numeric consumer reads this kind. This does not enable
those effects on equipment. Arcane Elixir, Greater Arcane Elixir and Ephemeral Power now add magic damage;
Curse of Weakness reduces physical weapon damage. Dampen and Amplify Magic modify incoming
spell damage and healing by school. They replace each other. The spell plan imports source
stack groups for school modifiers, including Arcane versus Greater Arcane Elixir. The zero-default
END column `game_spell_group.power_rank` preserves each source group's stronger-spell order.
Groups with zero power ranks retain their existing magnitude comparisons. A cast checks every
source group before it removes conflicting auras; a refusal preserves the active spell.
Aspect of the Hawk contributes only its ranged AP to the separate ranged field. Unrestricted crit
checks only the source equipment class of -1; masks are ignored in that case, including Recklessness
and Elixir of the Mongoose. Ranged AP uses the class formula, and ranged damage retains its fraction
on the wire. The sheet sends both melee and ranged crit percentages, using each attack slot's weapon skill.

`game_spell` END-appends source equipment class, subclass and inventory masks, attribute words
3 and 4, and damage class. The importer computes missing damage and healing coefficients from
the source cast time, channel duration, direct/periodic split, area targets, extra effects and
spell level. Explicit coefficients already include level penalties. The pinned Reference uses
no caster-level downranking factor. Intellect contributes spell crit, not spell damage or healing.
Damage-only bonuses do not increase healing. Physical nonweapon spells such as Eviscerate
use their source coefficients too. Area and pet-area modifier shapes remain inert until their
propagation is supported. Seal of Righteousness uses its source weapon-speed
base and 0.10 one-hand / 0.125 two-hand coefficient; the judgement Dummy remains a holder.
Physical flat damage enters each weapon slot once;
weapon-restricted magic bonuses do not become general spell power. Flat incoming magic damage
and healing reductions cannot remove more than half the spell's pre-mitigation amount.
Ground damage snapshots outgoing spell power and applies incoming modifiers per target on each tick.
`game_ground_area.effect_id` END-appends a zero-default source effect key for that coefficient.
`game_ground_area.by_caster` finds the caster's channelled ground areas when its channel ends.
The native CREATE and aura VALUES send all seven signed positive/negative damage fields and
float percent multipliers. `game_item_template.damage_school` END-appends the source
`dmg_type1` index (zero defaults to physical). Wand attacks use that school for spell crit,
ignore flat spell power, and retain the school on a projectile already in flight.
Damage display fields are derived from active auras and need no new durable cache.

`game_world_entity.sheet_ranged_dmg_min_fraction` and `sheet_ranged_dmg_max_fraction` are
zero-default END-appended caches for the fractional parts. The existing integer damage columns and
Character Transfer layout stay unchanged. The zero-default END cache `sheet_ranged_crit_bp`
keeps ranged crit separate from melee crit and refreshes when the ranged weapon skill changes.
Publish repair refreshes these projections; see
[the migration order](danger-zones.md#equip-spell-migration).

`game_player_skill.current` stores earned progress without bonuses; `max_rank` stores the level
or profession cap. A new weapon, ranged, unarmed or defense skill starts at 1 with a cap of five
per level. Learning a spell grants its combat skill line when `game_skill_ability.acquire_method`
is 2, as a weapon master's wrapper spell does. Login and publish repair also add a known combat
skill that an existing Character never stored, including one from a starting spell it first learns
at that login; that row starts at the level cap, the value a missing row used to give. Existing
progress survives login and publish repair. Ranged skills also follow the level cap.
A landed weapon hit trains the skill of the weapon that made it: the ranged weapon for ranged
attacks and wand shots, the off-hand weapon for off-hand swings, otherwise the main hand or
unarmed. Neither weapon nor defense skill rises against a player-controlled opponent.
An empty or broken ranged weapon slot contributes zero attack skill; an empty main hand uses
unarmed. Feral forms use the level cap for melee skill.

`A_MOD_SKILL` retains signed skill points, the skill line in `p0`, and temporary (0) or permanent
(1) selection in `p1`. Source aura 30 supplies temporary bonuses; aura 98 supplies talent and
racial bonuses, including Human Sword and Mace Specialization. Active Auras supply both bonuses
without changing earned progress or granting unknown skills. Combat and skill delivery use the
same values. `PLAYER_SKILL_INFO` packs the signed temporary bonus in the low half and permanent
bonus in the high half, in both CREATE and VALUES. Equip removal, Aura expiry and repair update
these fields and weapon crit. Defense bonuses also reach white melee hit, avoidance, crit and
glancing calculations, including maximum defense against another Character. Skill requirements
for equipment, skinning, gathering and locks also include bonuses. Fishing retains its existing
loot selection without a skill-dependent success roll; its 11 ModSkill equipment items deliver
the correct bonus fields but do not change that missing roll.
Flattened defense enchantments still use the separate item-stat fold. Their bonus fields are
absent, and their positive bonus is added after a negative skill Aura has clamped at zero. The
feature ledger records this remaining item-enchantment representation and delivery gap.

| Source aura type | Items | Consumer or missing behavior | Equip result | Example spell |
| --- | ---: | --- | --- | ---: |
| 3 PeriodicDamage | 1 | periodic damage / leech shape not certified | Inert | 19307 |
| 4 Dummy | 12 | A_FLAG placeholder | Inert | 16372 |
| 8 PeriodicHeal | 8 | periodic healing bonus/cadence not certified | Inert | 5707 |
| 13 ModDamageDone | 928 | signed school damage; weapon-slot restrictions; spell coefficients | Correct for 927 Items; area aura 28143 on Atiesh stays inert | 7685 |
| 14 ModDamageTaken | 2 | flat incoming damage by school; spell coefficients and half-damage floor | Correct | 5102 |
| 15 DamageShield | 10 | qualifying melee hits reflect school damage; SMSG_SPELLDAMAGESHIELD | Correct | 15438 |
| 17 ModStealthDetect | 8 | detection placeholder | Inert | 12418 |
| 19 ModInvisibilityDetection | 2 | detection placeholder | Inert | 17623 |
| 22 ModResistance | 74 | resistance_bonus; flat school mask | Correct | 12956 |
| 23 PeriodicTriggerSpell | 9 | periodic trigger needs mechanics | Inert | 758 |
| 29 ModStat | 1 | effective_stat supports self auras; selected effect 35 needs area mechanics | Inert for the selected Item | 7363 |
| 30 ModSkill | 206 | signed skill points; combat, skill requirements and client bonuses | Correct bonuses; fishing success roll remains absent | 7515 |
| 31 ModIncreaseSpeed | 62 | run_speed lacks faithful mounted and slow composition | Inert | 17625 |
| 42 ProcTriggerSpell | 52 | item proc restrictions not certified | Inert | 4070 |
| 43 ProcTriggerDamage | 1 | item proc restrictions not certified | Inert | 16550 |
| 45 TrackResources | 2 | tracking placeholder | Inert | 4152 |
| 47 ModParryPercent | 28 | parry modifier not implemented | Inert | 13665 |
| 49 ModDodgePercent | 89 | roll uses basis points; full dodge sheet projection is absent | Inert | 13669 |
| 51 ModBlockPercent | 24 | block modifier not implemented | Inert | 13674 |
| 52 ModCritPercent | 384 | effective_crit_bp / effective_ranged_crit_bp and both wire fields; percent to basis points | Correct when unrestricted; weapon-restricted 9132 inert | 7597 |
| 54 ModHitChance | 192 | effective_miss_bp; percent to basis points | Correct | 15464 |
| 55 ModSpellHitChance | 76 | spell hit needs its own consumer | Inert | 23727 |
| 56 Transform | 7 | transform requirements not certified | Inert | 8299 |
| 57 ModSpellCritChance | 2 | spell_crit_bp; separate spell basis points | Correct for 1 Item; 1 area Item inert | 23434 |
| 58 ModIncreaseSwimSpeed | 1 | swim stacking not certified | Inert | 8747 |
| 64 PeriodicManaLeech | 1 | mana leech needs drain and gain | Inert | 19307 |
| 71 ModSpellCritChanceSchool | 215 | spell crit by school; source percent units | Correct | 18382 |
| 77 MechanicImmunity | 4 | mechanic immunity representation not certified | Inert | 7219 |
| 82 WaterBreathing | 3 | water breathing placeholder | Inert | 11789 |
| 85 ModPowerRegen | 345 | flat MP5 on two-second ticks, including during the five-second rule | Correct for mana | 18378 |
| 89 PeriodicDamagePercent | 1 | percent damage must scale with health | Inert | 28347 |
| 99 ModAttackPower | 291 | melee_attack_power; signed flat AP | Correct | 9138 |
| 102 ModMeleeAttackPowerVersus | 40 | melee AP needs creature-type selection | Inert | 14097 |
| 107 AddFlatModifier | 50 | SpellMod operation/mask semantics not certified | Inert | 18388 |
| 108 AddPctModifier | 16 | SpellMod operation/mask semantics not certified | Inert | 23043 |
| 109 AddTargetTrigger | 2 | target trigger needs mechanics | Inert | 17768 |
| 112 OverrideClassScripts | 6 | class scripts need mechanics | Inert | 27853 |
| 117 ModMechanicResistance | 5 | mechanic resistance needs mechanics | Inert | 18985 |
| 123 ModTargetResistance | 30 | target resistance needs mechanics | Inert | 25717 |
| 124 ModRangedAttackPower | 305 | class-specific ranged_sheet_values and ranged attacks; flat AP | Correct | 9138 |
| 130 ModMountedSpeedAlways | 1 | mounted-speed stacking not certified | Inert | 13587 |
| 131 ModRangedAttackPowerVersus | 39 | ranged AP needs creature-type selection | Inert | 14097 |
| 135 ModHealingDone | 1017 | healing_power; flat healing by school mask | Correct | 7678 |
| 138 ModMeleeHaste | 3 | melee haste must stay separate from ranged haste | Inert | 18065 |
| 139 ForceReaction | 1 | faction reaction needs mechanics | Inert | 28282 |
| 140 ModRangedHaste | 1 | ranged haste must stay separate from melee haste | Inert | 23674 |
| 141 ModRangedAmmoHaste | 21 | ammo haste needs its own consumer | Inert | 14824 |
| 144 SafeFall | 2 | safe fall placeholder | Inert | 23409 |
| 154 ModStealthLevel | 5 | stealth level needs mechanics | Inert | 17746 |
| 158 ModShieldBlockvalue | 21 | block value needs mechanics | Inert | 22852 |
| 161 ModHealthRegenInCombat | 38 | flat HP5 in and out of combat, with health fraction carry | Correct | 12956 |
| 180 ModFlatSpellDamageVersus | 6 | flat damage needs creature-type selection | Inert | 22849 |

The source handlers are in [SpellAuras.cpp](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellAuras.cpp).
School damage, melee and ranged AP, healing and crit distinctions also follow
[Unit.cpp](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Unit.cpp).
Cast Aura polarity also follows
[SpellEntry::IsPositiveEffect](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellEntry.cpp).
The class AP formulas and fractional damage follow
[StatSystem.cpp](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/StatSystem.cpp).
Mana regeneration follows the two-second `Player::Regenerate` cadence in
[Player.cpp](https://raw.githubusercontent.com/vmangos/core/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Objects/Player.cpp).

### Damage shields

`A_DAMAGE_SHIELD` (0xC0) retains the source amount. Main-hand and off-hand swings trigger
reflection when they deal damage or block. Weapon-proc spells trigger it when they deal damage.
Ranged attacks and other spells do not trigger reflection. The bearer receives damage and threat
credit. A reflected hit uses its own school and the shield packet, bypasses absorbs and partial
resistance, cannot crit, and cannot trigger another shield. The shield amount is not consumed.

For immunity Auras, `p1` retains the source aura id in its low byte and bit 0x100 when the source
immunity also blocks friendly effects. Reflection uses this distinction to choose a spell miss or
`SMSG_SPELLORDAMAGE_IMMUNE` before applying damage. A damage immunity always blocks the reflected
damage; a school immunity blocks it only when its source is helpful or carries bit 0x100. A creature
template's `school_immune_mask` blocks it too. White swings and queued strikes use the same damage
test for physical damage, and a ranged swing for its spell's school and its weapon's school: an
immune target takes no attack table roll and no damage, and the swing reports victim state 7 or the
spell miss Immune. A Spell takes the test with its school before its hit roll, as vmangos
`SpellHitResult` does, and the cast reports `SPELL_MISS_IMMUNE`; a refused harmful Spell still draws
its target, creature or Character, into combat. Only a harmful Spell on another unit meets the
template mask and damage immunity. A school immunity refuses a helpful Spell only when its source is
harmful or carries bit 0x100, and the cast refuses such a Spell before it charges its cost. A
periodic damage tick and a ground area tick take the test on every tick. A Spell with
`SPELL_ATTR_NO_IMMUNITIES` or the ignore-restrictions attribute skips the test, and one with
`SPELL_ATTR_EX2_NO_SCHOOL_IMMUNITIES` skips school immunity Auras. Shield damage still lands when
the reflected swing ended a Duel.

### Mechanic numbering

A Spell's `mechanic` and an effect's `effect_mechanic` (zero names none) carry the Spell.dbc
mechanic, as vmangos `SharedDefines.h` numbers it: 5 is fear, 7 root, 12 stun, 17 polymorph. So does
an `A_IMMUNITY` row with `p0_kind` `P_MECHANIC`: aura 77 keeps its misc value, and aura 147 keeps
its mask, with the source aura id in the low byte of `p1`. An `A_CONTROL` row's `p0` is not a
Spell.dbc mechanic but a control class (`M_STUN`, `M_ROOT`, `M_FEAR`, `M_POLY`), which several
mechanics share: it picks the unit's control state and its diminishing-returns category, and no
immunity reads it.

A target is immune to a whole cast when it holds an immunity Aura that names the Spell's `mechanic`,
before the hit roll and before any effect runs, as vmangos `IsImmuneToSpell` decides. It is immune
to one effect of any kind when an Aura names the effect's `effect_mechanic`, whatever the hit roll,
as `IsImmuneToSpellEffect` decides when vmangos builds the target's effect mask: that effect is
dropped and the others still run. So Frostbolt's
damage lands on a unit immune to snare without its slow, and Kick's damage lands on one immune to
interrupt without its interrupt. The Aura's source Spell must have the other polarity, or its `p1`
must carry bit 0x100. A Spell is harmful when any effect aims at an enemy, and an effect is harmful
when it does, unless `SPELL_CUSTOM_POSITIVE` says otherwise. A Spell with `SPELL_ATTR_NO_IMMUNITIES`
(Attributes 0x20000000) skips the header mechanic, and one with the ignore-restrictions bit
(AttributesEx3 0x10000000) skips both. A unit is immune to its own cast too, as vmangos ignores `castOnSelf`.

A creature is also immune to a mechanic its template's `mechanic_immune_mask` names, with bit
`mechanic - 1` set, as vmangos `Creature::IsImmuneToSpell` decides for the header mechanic and
`IsImmuneToSpellEffect` for an effect's mechanic. The mask holds whatever the polarity,
and the same attribute bits skip it. It is the one immunity LyraCore implements that spares a
creature's own cast, as vmangos `castOnSelf` does. A live creature reads its template's mask, so a
changed template applies at once. An interrupt does not lock the schools of a creature whose
template names silence (9), as vmangos `Creature::LockOutSpells` does; it still stops the cast
unless the creature is immune to interrupt (26). An Aura that names silence does not skip the
lockout.

A helpful spell aimed at a target that is immune to its header mechanic is refused when the cast
starts, with `SPELL_FAILED_TARGET_AURASTATE`, so no item, power or cooldown is spent, as vmangos
`Spell::CheckCast` does. A harmful one is refused after it starts.

A cast that was refused whole on a target reports it as immune (`SpellMissInfo` Immune, for the
primary target): Immune when the header mechanic refused it, and Immune2 when only effect
mechanics masked every effect, even when the hit roll missed, as vmangos `WriteSpellGoTargets` does. It also runs the proc pass for that target with `PROC_EX_IMMUNE`,
which is how Fear Ward is used up: it fires on any spell whose header mechanic is fear, whatever
the result. A cast an aura started runs no such pass, a harmful item cast runs it, and a helpful spell raises no
proc event. A finisher refused whole keeps its combo points.

### Follow-up casts

`game_spell_follow_up` names, for a spell, the spell its caster casts as a Follow-up Cast at each
unit the spell hits, as a vmangos spell script's `OnHit` or `OnAfterHit` does. LyraCore casts it
when the spell resolves, after its `SMSG_SPELL_GO`. vmangos casts it when the spell reaches the
unit, so a follow-up of a spell with travel time comes early here; no follow-up spell has travel
time. A unit the hit roll missed, or that refused every effect, gets none. A queued next-swing spell
makes none when it is queued.

A Follow-up Cast is a Triggered Cast. While one runs, no cast it leads to makes a Follow-up Cast,
whether through a trigger effect, a Proc or a Chance on Hit, so a pair such as A follows up with B
while B triggers A ends after one round. The guard holds only while the first Follow-up Cast and
the casts it leads to run, and is released when they return. A later cast, in the same Durable
Request or a later one such as a periodic tick of an Aura the follow-up applied, may follow up
again.

The spell import writes the rows from vmangos `spell_template.script_name`, by the script names in
`SCRIPT_FOLLOW_UPS` (`importer/src/spell_overrides.rs`); such a spell is no `spells.script`
finding. Each follow-up spell is an aura 77 immunity to its spell's header mechanic whose `p1`
carries bit 0x100, so the next such spell on that unit is refused at the cast-start gate with
`SPELL_FAILED_TARGET_AURASTATE`, and an item that casts it is kept:

| Script | Spells | Follow-up | Mechanic |
| --- | --- | --- | --- |
| `spell_paladin_bubble` | Divine Shield, Divine Protection, Blessing of Protection (7) | Forbearance (25771) | 25, invulnerability |
| `spell_first_aid` | First Aid (19, the bandages) | Recently Bandaged (11196) | 16, bandage |
| `spell_priest_power_word_shield` | Power Word: Shield (11 ranks) | Weakened Soul (6788) | 19, shield |

Lay on Hands has no mechanic and no script at build 5875, so it neither applies Forbearance nor is
refused by it.

A Package may claim the table; [the Package API](./package-api.md) gives the rule. The spell
import deletes and rewrites every row keyed by an imported spell, including a Package's row on
one, and the Package reapply restores the Package's rows. The import is a fixed SQL plan whose
statements commit one by one, so it cannot skip a row by what the Shard holds, while a Package
apply is one transaction: a Package row that collides with an import row is refused there and
nothing of that apply is written.

### Regeneration

`A_MOD_POWER_REGEN` (0xC1) stores mana per five seconds. Each Character keeps a two-second
timer, independent of energy. Mana adds MP5 to class-specific spirit regeneration and
truncates each tick. The five-second rule suppresses only spirit regeneration; aura 134 permits a
percentage of spirit while that rule is active. Aura 110 multiplies spirit, without multiplying
MP5. Evocation therefore uses its source spirit multiplier and casting allowance. Aura 85 on rage
(Anger Management) stays a periodic energize: vmangos adds 3/5 of the five-second amount every
3 s. Other non-mana aura 85 shapes import as inert, because vmangos gives them no effect. Energy
and rage decay keep their separate calculations.

`A_MOD_HEALTH_REGEN` (0xC2) stores HP5 that applies in and out of combat. `A_MOD_REGEN` (0xC3)
stores out-of-combat regeneration, with the source period in `p1` (zero defaults to 5,000 ms).
`A_MOD_HEALTH_REGEN_PCT` (0xC4) multiplies out-of-combat spirit regeneration. Existing
`A_COMBAT_HEALTH_REGEN_PCT` remains the percentage of spirit permitted in combat.
`A_MANA_REGEN_INTERRUPT` (0xC5) stores the casting allowance. Sitting multiplies spirit health
regeneration by 1.5; polymorph uses 10% maximum health per tick before HP5.

`game_character_regeneration` is a private Character-keyed row holding the fractional health gain
and next tick deadline. Full pools do not accumulate credit before a new deficit.
It starts empty, travels with the Character across Transfer, and resets on a fresh login.
Character deletion removes the row.
Regeneration selects Character deficits through the composite type/pending index and reads indexed Auras;
it adds no whole-table scan. The source mana rates have no dedicated 5875 update fields. Health
and power changes use the ordinary unit field updates.
