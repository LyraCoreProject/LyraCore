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

## Committed movement experiment

The next comparison uses the same movement workloads, with one tick per transaction in a
disposable persistent Shard. Setup stops the fixture's scheduled work before inserting rows.
The reference executes the existing movement rules in Wasm. The native path sends the complete
net row changes from those same rules to a feature-gated commit reducer. Both paths publish the
existing gameplay tables through SpacetimeDB subscriptions.

Define these checks before measuring:

- Each committed tick has the reference's complete entity, spline and attack state.
- Subscription updates contain the same inserted and deleted gameplay rows.
- A repeated commit has no second effect. Reusing its sequence with different bytes fails.
- An old run, skipped sequence or mismatched old row fails without changing any row.
- A net batch contains at most one old row and one new row for each table and primary key.
- A failure after applying part of a batch rolls back the batch and its progress row.
- Restart preserves the accepted sequence and rows. Retrying the last accepted batch after restart
  does not apply it again.

An update with the same primary key must compare its old BSATN row with the committed row before
using the indexed update operation. Inserts and deletions retain their row checks. Every operation
and the receipt remain in one transaction.

The experiment admits one outstanding tick and retains its last receipt. A new setup increments
the run number, so delayed commits cannot enter a later run. The commit reducer accepts only the
fixture's entity, spline, attack and movement-intent tables. This is not the complete runtime's
ownership protocol: other gameplay entry points are not routed through it, and only an empty,
disposable Shard with paused schedules is supported.

Use a persistent binary WebSocket connection for both paths. Include request encoding, native
execution where applicable, commit and the confirmed subscription notification in elapsed time.
Setup and parity queries stay outside that interval. Record separate tick samples and request bytes. Subscription
sizes are the CLI's decoded JSON output, not network bytes. Its decoding and printing time is part
of the publication measurement. This is a disposable subscriber, not the Gateway protocol path.
The first HTTP attempt exceeded the endpoint's request-size limit at 5,000 creatures. Both paths
therefore use the pinned SpacetimeDB v2 binary protocol. Request sizes count the BSATN message,
excluding WebSocket framing. A tick remains one transaction.

## Alternative to a separate commit pass

The committed movement experiment exposes a second cost: the native host changes its memory
tables, then SpacetimeDB applies those changes again. Faster native row decoding and indexed
durable updates reduce that cost, but the exploratory runs still lose to the Module for idle and
combat movement. Pursuit improves. That mixed result does not justify expanding this authority path.

The next candidate runs the native Module against SpacetimeDB's existing transaction engine.
Storage, commit, schedules and subscriptions would keep their current implementation. The pinned
runtime separates function execution from transaction handling through
[`WasmInstance` and `WasmModuleInstance`](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/core/src/host/wasm_common/module_host_actor.rs).
The first prototype may select only the movement reducer for native execution. Every other entry
point continues through Wasm until its native path passes the same checks.

Establish these conditions before extending that prototype:

- Pin the database source, SDK patch and both Module artifacts. Load native code only from an
  operator-selected local artifact that matches the published Module and its schema.
- Give each Module instance its own registration state and table identifier caches. A second Shard
  or replacement instance must not reuse another instance's mutable state.
- Preserve the actual caller, connection, logical time, structured Refusal and transaction result.
- Catch native panics inside the native library boundary. A trap must roll back its transaction;
  no Rust unwind may cross a C entry point.
- Compare committed state and subscription rows with the Wasm reference after every movement tick.
  Repeat stale-input, malformed-row, partial-update and restart checks where they apply.
- Measure request-to-acknowledgment and confirmed publication using the same fixture and resource
  limits. Include CPU and memory before making a capacity claim.

This candidate needs a database runtime patch and a native SDK entry point. Those are maintenance
costs, not completed work. Native execution also does not inherit Wasmtime's fuel limit. Bounded
execution, trap recovery and instance replacement need their own proof before this path can acquire
general authority. Keep the normal database binary and live Realms unchanged during the experiment.

The prototype files are in `database/`. They patch SpacetimeDB commit
`8ecd3c3bac427a49a9d9b13671bf83324afa59ba` and a private copy of the crates.io 2.7.1 SDK. The normal
dependency remains unchanged. The server selects native execution only for
`debug_native_committed_step` and the rollback fixture `debug_native_database_fault`.
Both implementations use the existing transaction, commit and subscription code. Native table
operations call `InstanceEnv` directly. They do not copy rows into a second authoritative store.

The loader checks both artifact hashes and the complete Module description. It loads a separate
library copy for each Module instance. The native SDK catches reducer and description panics inside
the library. Unsupported host operations trap the transaction, including immediate scheduling and
procedure calls. The prototype does not accept an arbitrary native Module as a replacement for Wasm.
Generated registration functions still use the SDK's existing C entry points.

The driver compares 21 complete committed frames and subscription updates before timing. It also
checks authorization, stale runs, skipped input, structured Refusals, panic rollback and persistent
restart. A Module log marker proves that the selected reducer used the native entry point. Timed
samples retain server and driver CPU ticks, server resident memory and its process peak. CPU counts
exclude the separate CLI subscriber. Wasmtime fuel and linear-memory metrics do not include native
work. A host-call count and elapsed-time limit bound callback work, but cannot interrupt native code
that makes no callbacks. These limits are not general execution or memory isolation.

Build the experiment only on the benchmark machine. Use a separate LyraCore checkout for the SDK
patch and a disposable directory for its server and artifacts:

```sh
prototype_dir="$PWD/module/prototypes/native-simulation/database"
prototype_scratch=/path/to/disposable-native-database
prototype_sdk=/path/to/registry/spacetimedb-2.7.1
git clone --depth 1 --branch v2.7.1 https://github.com/clockworklabs/SpacetimeDB.git "$prototype_scratch/server"
python3 "$prototype_dir/prepare.py" "$prototype_scratch/server" "$prototype_sdk" "$prototype_scratch/sdk"
CARGO_BUILD_JOBS=1 CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  cargo build --manifest-path "$prototype_scratch/server/Cargo.toml" --release --locked -p spacetimedb-standalone
```

Build the Wasm reference with `native_simulation_prototype` and the benchmark example with
`native_simulation_host`, using the unmodified SDK. Then build the native Module library with
`native_simulation_database` and Cargo's `patch.crates-io.spacetimedb.path` set to the private SDK
copy. Keep a copy of each artifact. Use one Cargo job and one release code-generation unit for both
Module builds. The SDK patch changes that checkout's lockfile; it must not enter the normal build.

For the comparison, copy `database/spacetime-wrapper.sh` to the disposable directory and make the
copy executable. Set `SPACETIME_BIN` to that wrapper, `LYRACORE_NATIVE_DATABASE_CLI` to the unmodified
2.7.1 CLI, `LYRACORE_NATIVE_DATABASE_SERVER` to the experimental server and
`LYRACORE_NATIVE_DATABASE_LIBRARY` to the native Module library. Run `native_committed_benchmark`
with the Wasm artifact, a new output path and creature counts as its arguments. Use a private
`TMPDIR` and the same CPU and memory limits as the preceding measurements. Both comparison servers
run the experimental binary; only one receives the native artifact manifest. The wrapper passes
other CLI operations to the original CLI. Each server uses its own disposable data directory.

The first exploratory run passed the 21-frame comparison, structured Refusal, panic rollback and
restart checks. All six timed scenario and size combinations favored native execution. At 5,000
creatures, mean time through confirmed CLI publication fell by 12.6% to 20.0%, and measured server
CPU time fell by 24.5% to 31.6%. The run used five measured samples after three warmups. All 336
paired ticks and 48 paired encounter results matched, including warmups. This source included
uncommitted prototype changes; its metadata records that fact. The raw samples and artifact hashes
are in [native-database-benchmark-1.jsonl](native-database-benchmark-1.jsonl).

The recovery probe found retained native library memory. After zero, eight and sixteen further
traps, native process resident memory was 247,708, 294,608 and 341,548 KiB. The Wasm reference stayed
at about 161,644 KiB. Fresh native library copies retain thread-local state until the executor
thread ends. Repeated native instance replacement therefore cannot remain the recovery policy.
The next check must bound this memory use before the native path expands. Both servers restarted
after the recovery probe and before timing. Timed native process resident memory was about 6 to
13 MiB higher than the reference, depending on the workload.
