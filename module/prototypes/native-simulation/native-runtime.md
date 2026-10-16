# Native runtime parity contract

The reference is the pinned SpacetimeDB 2.7.1 Wasm Module built from the same Core source, Packages
and features as the native library. The parity contract preceded implementation. Earlier experiment
plans and measurements remain in history; this document describes the retained execution path.

## Authority and execution boundary

The native Module calls SpacetimeDB's existing `InstanceEnv`. It has no separate authoritative row
store, worker queue or commit protocol. The database engine retains transaction ordering, commit,
persistence, schedules and subscription publication. Gateway Stores keep their current contract.
The entire creature cycle stays one transaction, including temporary positions that later phases
read before `settle_advances` decides which positions to publish.

Wasm is the default. An Operator may select a matching native artifact for trusted Core and
compiled Package code. Native activation requires finite cgroup v2 CPU, memory and swap limits,
including inherited limits. Native execution shares the process's memory and failure boundary.
A process crash can interrupt every Shard in that process. CPU quotas bound CPU consumption;
they do not interrupt a non-returning reducer. The [build and launch guide](README.md) describes
activation and the limited launcher.

Each Module instance loads its own library copy, so its registration state, mutable globals and
SDK table identifiers stay separate. The loader verifies the published Wasm hash, native library
hash and complete Module description. It retains the real caller, connection, timestamp, reducer
arguments, Refusal and transaction outcome. Native host operations use the existing index and
storage implementations. An unsupported host callback traps rather than returning invented state.

The bridge permits two million host callbacks per transaction and checks elapsed time every 1,024
callbacks. Passing ten seconds marks that transaction for rollback when native code returns.
These checks do not interrupt execution or provide per-reducer isolation.

Reducer panics are caught inside the native library's C entry point. The database rolls back the
transaction and replaces the failed instance with Wasm. Only one native library load is allowed
per Module host, because the SDK retains thread-local state until the host thread exits. This
prevents repeated traps from accumulating native libraries. A process restart or a new Module host
can enable native execution again. A gameplay Refusal does not disable native execution.

An abort, native memory fault or exhausted process memory budget can end the process. Restart
uses the existing persistent database recovery. A request whose acknowledgment was lost remains
an unknown outcome; the Gateway must not replay it as a fresh gameplay request.

## Required behavior

| Area | Reference behavior |
| --- | --- |
| Storage | Column types, indexes, uniqueness, generated keys, inserts, updates, deletions, rollback and iterator behavior |
| Context | Caller and connection identity, authorization, logical time, random draws, Refusals, traps and ordered transaction results |
| Creature cycle | Active cells, patrol, aggro, assist, pets, EventAI, casts, threat, pursuit, combat, movement, regeneration, fear and maintenance |
| Combat and spells | Impacts, interrupts, auras, periodic effects, ground areas, cooldowns, death and respawn |
| Packages | Compiled callbacks and their order, custom tables, Runtime Scripts, EventAI continuations, encounters and content revisions |
| Durable consequences | Loot, inventory, currency, experience, reputation, quests, cooldowns and refusal atomicity |
| Lifecycle | Login, logout, reconnect, session expiry, instances, pets, Transfer and Escrow |
| Protocol | Committed rows and Gateway packets, AOI changes, movement, combat and duplicate suppression |
| Failure | Rollback, lost acknowledgments, process restart, Module replacement and mixed program versions |

Run the existing durable and Headless Client tests against both implementations without weakening
assertions. A native result needs named native reducer evidence in every owned fixture process.
Unexpected fallback invalidates the native result even when the test later passes under Wasm.
Deliberate trap recovery is checked separately.

The movement comparison retains the original eight-scenario, 60-frame Wasm trace. It checks
complete committed state and subscription updates for each of 21 scaled movement ticks before
benchmarking. Its historical trace allows an absolute float difference of 0.00001. The complete
fixed-time gameplay comparison is stricter: every field, including floats, must match exactly.

The complete-cycle fixture discovers its tables from the Module description, clears disposable
application state, seeds Core at a fixed time and disarms background schedules in one transaction.
It compares initial rows, every confirmed subscription update and final rows across all application
tables except `game_operator`, whose publishing identities differ. Row order is ignored. A mismatch
names the affected tables and records both values. Separate assertions require actual combat,
damage and rewards; an empty phase is not coverage of that phase.

The fixture calls the same creature, melee and aura rules as the scheduled reducers. Its synthetic
context controls rule time and timestamp-seeded draws. Existing durable tests cover scheduled-caller
Gates and wall-clock scheduling. Installed Package tests cover callbacks, navigation and encounters
that the Core-only fixture does not contain.

## Determinism

The simultaneous-death case exposed nondeterministic aura hash-map iteration even between two
Wasm instances. Pending aura outcomes now run in GUID order in both builds. Reward formulas and
aura-ID killing-blow credit remain unchanged. Per-corpse rewards and event rows must match exactly;
the comparison does not accept a permutation of loot or money as parity.

The first 5,000-creature comparison found a spline destination differing in its last stored digit.
Native platform math rounded differently from Wasm. The private SDK binds the thirteen math
imports observed in Core and the Official Packages to `libm` 0.2.16 with hidden ELF symbols.
These bindings stay inside each native library and do not replace the host process's math.

The math check compares fifteen outputs from those functions over one million deterministic input
pairs, including all four angle quadrants. Logarithms and powers use positive bases. Every corrected
output matched Wasm. This samples that range, not every floating-point input. New math imports need
the same check before native use. The normal Wasm build remains unchanged.

## Recovery checks

Repeated publication must preserve every application row and keep native library mappings bounded.
A Wasm artifact with a different hash must run as Wasm while coexisting with a matching native
Shard. Both must recover through a process restart. Restoring the matching artifact must restore
native execution. Bad native hashes and Module descriptions must fail before changing durable state.

The process-failure fixture changes Character money and inserts a fixture row before aborting or
entering a busy loop. The busy-loop check terminates the owned process after its ready marker.
Restart must discard both uncommitted changes, preserve prior application rows and the sibling
Shard's money, and resume native execution. Wasm abort must trap without ending its process; forced
termination of the Wasm reference must recover the same committed state.

Native activation must fail independently when CPU, memory or swap limits are missing. Wasm must
still work without the native opt-in limits. The launcher check reads actual kernel cgroup settings,
including disabled swap and group termination on memory exhaustion. A forced-termination recovery
check is not evidence of per-reducer native isolation.

## Measurement rules

Run heavy builds and benchmarks on authorized benchmark hardware, in disposable storage with CPU
and memory limits. Keep hostnames, account paths and access details out of published artifacts.
Pause other owned test work between tests during clean performance measurements.

Compare the same artifacts, content, persistence settings and inputs. Alternate execution order,
include warmups and use fresh processes. Exact parity and native execution evidence gate each run.
Keep raw calculation timings separate from committed and published outcomes.

The complete-cycle benchmark advances rule time by one second and calls creature, melee and aura
processing once each. It measures that combined transaction, not the independent production
schedule cadences. Request encoding, acknowledgment and confirmed CLI subscription output count
in elapsed time. Setup and full-row comparisons do not. Runtime and driver CPU counts exclude the
separate CLI subscriber. Subscription sizes are decoded CLI JSON bytes, not network bytes.

Package load measurements must keep scheduled work active and report the tested navigation and
callback workload. Synthetic creatures or automated Characters are not connected-player capacity.
A capacity claim also needs sustained Gateway traffic, crowded AOIs and cross-Shard work at the
same content and persistence settings. Deployment to a live Realm remains a separate operation.

Recorded results and their source revisions are linked from the [verification index](README.md#recorded-evidence).
