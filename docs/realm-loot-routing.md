# Realm loot routing

`gateway/src/world/loot.rs` routes loot-roll votes and promotes locally created rolls to Realm-core.
It uses the existing `WorldStore` seam and the same in-memory topology as party routing tests.

`game_loot_roll` and `game_loot_roll_vote` are authoritative on Realm-core with Group membership.
A participant who crosses Shards during a roll can still vote through their current Gateway.

Creature death creates the roll in its own transaction on the World Shard. The Module cannot write
another database. That local roll is a staging copy. `relay_tick` promotes it to Realm-core and
clears the staging copy so only one database votes and resolves it. The initial `ROLL_START`
message reaches the nearby participants through their existing local relay.

`run_vote` flushes pending promotions before issuing the vote on Realm-core. `party::run` also
flushes them before leave and uninvite requests. Those requests can disband a Group, whose removal
transaction must see every existing roll to resolve it immediately. A periodic poll alone leaves
a window between creature death and promotion where disband could miss the roll.

The corpse's loot rows and item grant remain on the World Shard. After observing `ROLL_WON`, the
Gateway calls `settle_loot_roll` on connected World Shards. The Module's withheld-item check makes
the request harmless on a Shard that does not hold the corpse and prevents duplicate grants.

The 200ms poll reads each Coordinator's current connection and carries the database routing context
needed for promotion and settlement. It runs without a player session and bounds ordinary relay
latency. Synchronous promotion before votes and membership requests supplies the ordering guarantee.
