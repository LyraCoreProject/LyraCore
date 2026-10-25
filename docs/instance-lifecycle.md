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
