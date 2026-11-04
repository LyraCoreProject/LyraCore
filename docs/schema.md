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
