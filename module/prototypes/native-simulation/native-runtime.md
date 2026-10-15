# Native runtime parity contract

The movement prototype established a reason to continue. It did not establish that the complete
game is faster. The next execution candidate must run the existing Module rules, including their
table reads and Package callbacks. The complete creature cycle must remain one transaction.
Temporary positions are visible to later phases before `settle_advances` decides which changes
to publish.

This contract precedes the native runtime implementation. The reference is the pinned SpacetimeDB
2.7.1 Module on this branch. Each comparison records the source commit and artifact hashes.

## First proof

Compile the same Module source for native execution. Provide the table operations it currently
gets from the SpacetimeDB host. Run the existing movement reference against that implementation,
with the same logical time and inputs, before expanding the scenarios. This removes the original
prototype's assumptions that auras, navigation, combat and Package state are absent.

The first proof passes only when all original frames match and scaled workloads have matching
state after every tick. Index lookups must use the declared column types, including signed numeric
ordering and composite prefixes. Refused transactions must restore every changed row. Iterators
must retain their query result while the transaction changes the underlying table.

The native host is experimental until the gates below pass. A successfully loaded native library
does not establish gameplay, persistence or failure parity.

The first implementation is a static native build behind `native_simulation_host`. It shares the
reference's request handling and calls the original `CtxWorld` movement and pursuit phases. Its
storage checks cover typed composite indexes, unique constraints, iterator lifetime, row rollback
and suppression of net-zero changes. Unsupported host operations fail the transaction. Scheduled
and event tables are refused during schema registration.

This proof uses the reference fixture's dummy caller context. It does not establish authorization,
generated-key behavior, random draw ordering or a complete creature cycle. Native and Wasm hash
iteration can differ, so random behavior needs its own reference before it can move. Table and index
identifiers must remain stable across native worlds because the SDK caches them in each process.
Table access and iterators are valid only inside the transaction closure.

## Runtime contract

One executor owns a Shard's ordered work. Instance-level ownership is premature while the existing
cycle and maintenance passes can read or change other instances. Gateway Stores keep their current
gameplay contract. The executor must retain the caller identity, connection identity, logical time,
random draws, Refusals and transaction boundaries of each request.

The runtime must discover all tables and entry points in the built Module, including installed
Packages. Unsupported host operations must stop the experiment explicitly. They must never return
empty state or pretend a change succeeded.

Before native execution can acquire durable authority, its commit protocol must atomically check
ownership and input revision, apply the complete result and record a receipt. Duplicate delivery
returns the original outcome; a reused request identity with different input cannot apply changes.
A delayed worker cannot commit after its ownership has been revoked. Existing scheduled reducers,
operator actions and Package writes must participate in the same ordering contract.

Publication follows durable commit. Recovery restores timers, pending commands, Package state and
the last accepted sequence. A crash after commit but before acknowledgment cannot duplicate loot,
currency, experience or quest credit. Native failure must not turn an unknown outcome into a new
gameplay request.

## Coverage required for completion

| Area | Required reference behavior |
| --- | --- |
| Storage | All declared column types, indexes, unique constraints, generated keys, inserts, updates, deletions, rollback, query membership and iterator lifetime |
| Context | Caller and connection identity, authorization, logical time, random draws, Refusals, traps and ordered transaction results |
| Creature cycle | Active cells, patrol, aggro, assist, pets, EventAI, casts, threat, pursuit, combat entry and exit, idle movement, regeneration, rout, fear and maintenance |
| Movement | Terrain, navigation, doors, roots, stuns, fear, knockback, teleport, transports, spline renewal, facing, arrival, cell changes and suppressed intermediate positions |
| Combat and spells | Melee, ranged and spell impacts, casts, interrupts, auras, periodic effects, ground areas, cooldowns, threat, death and respawn |
| Packages | Existing compiled Package callbacks, callback order, custom tables, Runtime Scripts, EventAI continuations, encounters and content revisions |
| Durable consequences | Loot, inventory, currency, experience, reputation, quests, cooldowns and refusal atomicity |
| Lifecycle | Login, logout, reconnect, session expiry, instance entry and removal, pets, Transfer and Escrow |
| Protocol | Committed table changes and ordered Gateway packets, AOI entry and exit, movement, combat, duplicate suppression and reconnect |
| Failure | Worker, Module and Gateway crashes, stale ownership, duplicate and reordered commands, dropped acknowledgments, checkpoint recovery and rollback |

Reuse existing behavior tests and disposable integration tests. Extend the reference traces where
the existing tests do not expose the state or output affected by the replacement. Expected traces
come from the reference implementation. Compare discrete fields exactly and coordinates with the
existing declared tolerance. A state comparison cannot substitute for subscription or protocol
verification.

## Measurement gates

Run heavy builds, reference execution and benchmarks on the authorized benchmark machine in a
disposable environment. Bound CPU and memory use because that machine also hosts the test Realm.
Keep hostnames, access details and local account paths out of published results.

Keep raw calculation timing separate from input-to-commit and input-to-publication timing. Start
committed-state measurements from committed rows, execute one tick per transaction and include a
real subscription. Then exercise the Gateway with the same content, Packages, command stream, AOI
density and persistence settings in each implementation.

Record individual-tick distributions, missed cadence, process CPU, memory, changed rows,
subscription bytes and queues. Repeat in fresh processes and alternate execution order. Add
sustained combat, spells, navigation, Package encounters, crowded AOIs and Transfer before making
capacity claims. Seven-tick averages remain labeled as replay averages.

Completion requires the integrated runtime and its recovery path to pass these parity gates and
show a useful measured improvement. Deployment to a live Realm is a separate operation.
