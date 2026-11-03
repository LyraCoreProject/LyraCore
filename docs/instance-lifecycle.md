# Instance lifecycle

`module/src/instance.rs` owns dungeon leases, Character bindings, population, and removal timers.
Instance zero is the open world. SpacetimeDB allocates `game_instance.instance_id` from one.

`resolve_or_create_instance` handles dungeon Area Trigger entry. It resolves the Group's live
Instance first, then the Character's live binding, then creates an Instance. A solo Instance can be
adopted by a Group its holder joins, so the other members enter the same run. Character bindings
survive Group disband and expire when the Instance is reaped.

`create_instance` builds creature entities from the map's authored spawn rows and makes copies of
doors, buttons, chests and goobers. Templates and authored spawn rows are shared. With
`game_config.hosts_instances` off, it creates only a lease. The flag is on by default.

The Operator reducers `ensure_instance` and `evict_instance_population` create a lease with a given
id and remove an Instance's population while keeping its lease. Outside tests, only the debug
Package fixture `admit_to_instance` calls `ensure_instance`.

The global creature tick covers every Instance without a dedicated tick row. Operators may arm one
for a different cadence.

`reap_instances` observes player occupancy once per minute. It stamps an empty Instance and reaps it
after 30 minutes empty, or on the next empty observation when `reset_requested` is set. Re-entry
clears the stamp. Teardown removes population, encounter state, the dedicated tick row, Character
bindings, and finally the lease.

Each Instance copies its selected source spawn rows and waypoint routes. The copies own their death and
respawn timers, movement home, and life sequence. Reaping an Instance deletes those copies,
including dead creatures awaiting respawn. Imported dungeon and battleground source rows stay
dormant at instance zero. Battleground creation is unimplemented.

Dungeon exits and graveyard release use imported Area Trigger, area, and graveyard metadata.
Missing exit metadata sends a reaped-instance login to the Character's home. The small fixture keeps
its Deadmines bootstrap fallback. Raid admission uses map capacity.

Creature copies use `encounter::wave_guid` with bit 23 set in the low field. This separates their
identities from imported spawns and encounter wave allocation. A copy skips any guid that a spawn
row or entity already holds, because dormant instance-zero spawns in the Operator Spawn Range and
the Package Creature Range carry bit 23 too. GameObject copies use `0xF110 | bit46 | seq`, below the
pool band and above static and debug identifiers.

A Character standing in a Group's dungeon Instance without membership gets a 60-second removal
timer. `reconcile_instance_removal` runs when membership or location changes, including login and
Group projection. `expire_instance_removal` checks the rule again before moving the Character home.

All tables are private.

## Dedicated Instance Shard

A multi-Shard Realm assigns its dungeon maps to one World Shard with
`operator_set_map_owner`, using the same ownership records on the Realm and all World Shards.
That Instance Shard needs the dungeon World Source and `hosts_instances` enabled. Continent
Shards need the entrance Area Triggers. There is no Instance allocator across multiple Shards.
Existing multi-Shard data needs the reviewed migration described in [danger zones](danger-zones.md).

For a remote dungeon, `native::instance_shard::enter` queues admission before teleport. The Realm
checks the source World Session and supplies its current Group snapshot. The destination applies
that snapshot, checks party capacity, resolves the party lease or personal binding, and creates
population there when needed. A sixty-second admission reservation keeps the lease occupied until
Transfer import. Admission refusal leaves the Character on its source Shard. A reply must match the
pending request and connection, and the Character must still be inside the entrance Area Trigger.

The source starts its existing teleport and Transfer after admission. The destination requires a
live reservation for that source, Character, connection, map and Instance before accepting import.
Import consumes the reservation, binds the arriving Character and restores its Group projection.
The Transfer fence protects the Instance until release. Admission also clears its empty stamp.
An expired admission cannot create an unfenced arrival: import refuses and the existing Transfer
abort restores the source position.

Dungeon exits use map ownership to Transfer back to the continent. Personal bindings travel in the
Character manifest. A binding whose Instance was reaped is ignored on the next admission. Logging
back into a reaped Instance resolves the existing entrance or home fallback and Transfers to that
map's World Shard before sending the login sequence. The Character is never entered at Instance
zero on a dungeon map as an intermediate step.

The `instance-shard` Native Host check covers party entry, separate solo runs, party chat across
the portal, exits, binding reuse, reconnect, reaping and admission refusal on a disposable Realm.

## Manual resets

`CMSG_RESET_INSTANCES` resets eligible normal dungeons. A solo Character selects its bindings;
a Group leader selects the Group's leases, including a run the leader has not entered. Other
members cannot reset the Group. Raid maps are excluded. A Group member who is offline prevents
the reset. These rules follow the pinned [request handler](https://github.com/vmangos/core/blob/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Handlers/MiscHandler.cpp)
and [Group reset behavior](https://github.com/vmangos/core/blob/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Group/Group.cpp).

On a split Realm, the source snapshots personal bindings into the ordered Group command stream.
The Realm checks the current World Session, leadership and member presence, then sends its Group
snapshot to the Shards that own the dungeon maps. The Instance Shard applies the versioned snapshot
and refuses a reset if a newer projection has changed the caller's Group or leadership. It checks
occupancy in the same transaction that sets `reset_requested`: players, admission reservations and
Transfer fences all protect a run. The single-Shard path uses the same reset rules locally.

Accepted leases immediately stop admitting Characters. The existing reaper removes their population
and bindings on its next empty observation. A stale binding carried by a Character on another World
is ignored at the next admission; that entry creates or joins a fresh run. Resetting one run does
not reset other solo runs on the same map.

Each selected lease returns `SMSG_INSTANCE_RESET` on success or `SMSG_INSTANCE_RESET_FAILED` for
occupancy or an offline member. Delivery failures return an unavailable notification. Replies are
restricted to the requesting Character and connection on the source World. Requests and replies
expire after sixty seconds, and unavailable requests are not retried automatically. A Transfer or
reconnect can therefore lose a notification without undoing an accepted reset.

The new `game_world_instance_reset` and `game_instance_reset_result` outboxes start empty. Existing
stored rows, IDC signatures and Host delivery layouts are unchanged. Publish the Realm and every
World before reopening gameplay. Raid lockouts, scheduled raid resets and dungeon difficulty
selection remain outside this path.
