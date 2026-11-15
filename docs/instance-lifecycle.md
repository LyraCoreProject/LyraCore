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

Creature copies set bit 23 (`INSTANCE_POP_LOW_BAND` in `module/src/creatures/population.rs`) in
the low field. This separates their identities from imported spawns and encounter wave allocation. A copy skips any guid that a spawn
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
Every Instance Shard needs the dungeon World Source and `hosts_instances` enabled. Continent
Shards need the entrance Area Triggers. The fixed map owner remains the default for legacy bindings;
additional hosts use the allocation configuration below.
Existing multi-Shard data needs the reviewed migration described in [danger zones](danger-zones.md).

For a remote dungeon, `native::instance_shard::enter` queues admission before teleport. The Realm
checks the source World Session and supplies its current Group snapshot. The destination applies
that snapshot, checks party capacity, resolves the party lease or personal binding, and creates
population there when needed. A sixty-second admission reservation keeps the lease occupied until
Transfer import. Admission refusal leaves the Character on its source Shard. A reply must match the
pending request and connection, and the Character must still be inside the entrance Area Trigger.

Package relocation uses the same allocator and admission reservations without a World Session. The
Realm checks Package authority and directory ownership; the source checks its saved consent revision
and entrance before departure. Import names the exact admission request as well as the Character and
Instance. A Package body is restored only after Transfer release. The `package-transfer` Native Host
check covers Package Group entry, binding reuse, exits and restart settlement.

The source starts its existing teleport and Transfer after admission. The destination requires a
live reservation for that source, Character, connection, map and Instance before accepting import.
Import consumes the reservation, binds the arriving Character and restores its Group projection.
The Transfer fence protects the Instance until release. Admission also clears its empty stamp.
An expired admission cannot create an unfenced arrival: import refuses and the existing Transfer
abort restores the source position.

Dungeon exits use map ownership to Transfer back to the continent. Personal bindings, including
their hosting Shard, travel in the Character manifest. A binding whose Instance was reaped is ignored on the next admission. Logging
back into a reaped Instance resolves the existing entrance or home fallback and Transfers to that
map's World Shard before sending the login sequence. The Character is never entered at Instance
zero on a dungeon map as an intermediate step.

The `instance-shard` Native Host check covers party entry, separate solo runs, party chat across
the portal, exits, binding reuse, reconnect, reaping and admission refusal on a disposable Realm.

## Allocation across Instance Shards

`operator_set_instance_host(map_id, world, accepting)` registers a dungeon host. Repeat the same
configuration on the Realm and every World, and list all participating Shards as peers of one another.
Keep the original `operator_set_map_owner` record: it identifies the host of old bindings whose
`world` column is null. Without registered hosts, fixed-map routing still applies.

The Realm creates an Instance Placement for each Group or solo Character and dungeon map. New
placements take turns across accepting hosts. Concurrent Group arrivals resolve the same durable
placement. An existing personal binding selects its host when there is no Group placement; a Group
placement takes precedence over a member's separate solo run. The Instance Shard still creates the
local lease and checks capacity. Instance ids can coincide on different Shards: a binding's `world`
field, map and Instance id identify its destination together.

Set `accepting` to false to stop new placements on a host. Existing placements and personal bindings
continue to enter it, including fresh runs after a reset. This is a drain of new placements, not an
automatic evacuation. An unavailable assigned host refuses entry; it never creates a second copy
of the run elsewhere. Adding hosts does not move existing placements. Group disband removes its
placements; Character deletion removes that Character's solo placements. To retire a host, first resolve
its remaining Characters, bindings and placements with gameplay closed.

Admission, Group snapshots, world-port Transfer, reconnect, reaped-instance return and reset replies
all recognize registered hosts. Entry to another dungeon on the same host stays local. Group,
guild, profile and Realm-query fan-out includes every registered Instance Shard. The admission
reservation protects a local world port until arrival, just as it protects a remote Transfer.

### Upgrade an existing Realm

Close gameplay, drain Transfers and reset requests and replies, and publish every Shard. The new nullable `world`
column is end-appended to `game_instance_binding`; old rows use the unchanged default map owner.
Existing admission payloads remain decodable, and IDC signatures and Host delivery layouts do not
change. `game_instance_host` and `game_instance_placement` start empty.

Before enabling additional hosts, capture every live dungeon lease and its Group on each existing
Instance Shard. For each grouped run, call `operator_adopt_instance_placement(group_id, map_id, world)`
on the Realm. It refuses to replace an existing placement with another host. Conflicting live runs
for the same Group and map require reconciliation before reopening gameplay. Solo bindings need no
placement import. Register hosts on every Shard, then call `operator_report_character_profiles`
and `operator_report_realm_guilds` on the Realm. Verify Group and guild projections with a
disposable member on each new host before reopening gameplay. Group admission carries the Realm
snapshot. Preserve captures of leases, bindings, population identities, Characters and inventory
through this update.

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

The `game_world_instance_reset` and `game_instance_reset_result` outboxes carry reset work. Reset
bindings include their hosting Shard; drain these outboxes before upgrading their internal payload.
IDC signatures and Host delivery layouts are unchanged. Publish the Realm and every
World before reopening gameplay. Raid lockouts, scheduled raid resets and dungeon difficulty
selection remain outside this path.
