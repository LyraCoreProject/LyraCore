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
dependency remains unchanged. The default mode selects `debug_native_committed_step`, the
movement reference `debug_native_simulation_reference` and rollback fixture
`debug_native_database_fault`. The manifest can select all reducers for the next parity checks.
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
work. Callback count and elapsed-time checks can mark a transaction for rollback. They cannot
interrupt native execution, including code that ignores failed callbacks. These checks are not
general execution or memory isolation.

The selected boundary keeps Wasm as the default. An Operator may opt into native execution for
trusted Core and compiled Package code. Native activation checks for finite Linux cgroup v2 CPU,
memory and swap limits, including limits inherited from a parent group. Wasm execution does
not require that opt-in configuration. A native abort or exhausted process budget can affect every
Shard in the process; it does not have Wasm's per-reducer isolation.

`database/run_native.py` creates a user scope with explicit CPU and memory limits, disables swap
and enables group termination on an out-of-memory kill. Pass `--manifest`, `--cpu-percent` and
`--memory-mib`, then the standalone command after `--`. A CPU quota of 100 percent allows one CPU's
execution time. It throttles a runaway reducer; it cannot make that reducer return. The launcher
requires systemd 253 or later for the [scope OOM policy](https://github.com/systemd/systemd/blob/v259/man/systemd.scope.xml#L97).

The `native_database_lifecycle --process-failures` check changes Character money and a fixture row
before aborting or entering a busy loop. Native abort ends the process with signal 6. The loop check
kills the owned process with signal 9 after its ready marker arrives. Both native cases preserve all
previously committed application rows and the sibling Shard's money after restart. Native execution
then resumes. Wasm abort traps within the process; its loop check uses the same forced termination.
The check does not wait for Wasm fuel exhaustion.

Native activation is refused separately when CPU, memory or swap limits are missing. The Wasm
reference runs without those opt-in limits. The launcher check reads the actual cgroup settings,
including swap disabled and group termination on memory exhaustion. These checks establish the
selected process boundary; they do not restore per-reducer native isolation. Results and artifact
hashes are in [native-process-recovery-1.json](native-process-recovery-1.json).

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

Build the Wasm reference with `native_simulation_database` and the benchmark example with
`native_simulation_host`, using the unmodified SDK. Then build the native Module library with
`native_simulation_database` and Cargo's `patch.crates-io.spacetimedb.path` set to the private SDK
copy. Both Module artifacts must have the same feature set and Module description. Keep a copy of
each artifact. Use one Cargo job and one release code-generation unit for both
Module builds. The SDK patch adds native-only `libm` 0.2.16 and changes that checkout's lockfile;
it must not enter the normal build.

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

An isolated library probe reproduced the lifetime issue. A library stayed mapped after `dlclose`
while its calling thread lived. It unloaded after the thread exited and the loader collected it.
The SDK's buffer pool uses thread-local storage. [Rust's destructor registration](https://github.com/rust-lang/rust/blob/1.93.0/library/std/src/sys/thread_local/destructors/linux_like.rs)
and [glibc's unload check](https://github.com/bminor/glibc/blob/master/elf/dl-close.c) account for
that lifetime.

The recovery policy now permits one native library load per Module host. If an instance traps, its
replacement uses the published Wasm Module and the same database. The failed transaction still
rolls back. Subsequent requests retain the existing gameplay behavior. This bounds retained native
library memory to one library per Module host; it does not force Rust thread-local destructors to
run while their thread is active. A process restart or new Module host can enable native execution
again. A Refusal does not replace the instance or disable native execution.

The repeated recovery check passed with this policy. Native resident memory was 244,032, 244,264
and 244,260 KiB after zero, eight and sixteen further traps. Its two library mappings remained
unchanged from before the first trap. Initial publication creates a temporary validation host as
well as the persistent host, so the check compares against the initial count. The Wasm reference
remained near 159,000 KiB. Rollback, the fallback marker, subsequent requests and restart all
passed. The raw five-sample, 100-creature check is in
[native-database-recovery-1.jsonl](native-database-recovery-1.jsonl). This run also records
uncommitted changes. The clean, twenty-sample run at `3d62194fb9cf59b38fe9ffd364a53b22d5ad8ac5`
then confirmed the gain across all nine combinations of workload and size. Its results are in
[README.md](README.md#native-execution-in-the-database-engine).

## Expanding beyond movement

The expanded parity gate reuses the original eight-scenario, 60-frame movement trace, then the existing
durable Module and Gateway tests. Preserve their expected outcomes. Run each selected test against
both the Wasm reference and native execution on disposable loopback instances. Build both Module
artifacts from the same source and feature set, including the inert test Package where required.

The artifact manifest must opt into all reducers separately from the movement experiment. This
includes initialization, connection lifecycle and scheduled reducers. Each first native reducer
invocation must leave a named marker in the owned process log. A suite pass without native execution
evidence does not count. Unexpected fallback invalidates a native suite result, even if Wasm later
completes the test. The deliberate trap test remains a separate recovery check.

Use the existing tests for creature movement, combat, spell scheduling, Runtime Scripts and EventAI
first. Then cover inventory, rewards, Packages, lifecycle, Transfer and cross-Shard Gateway work.
Keep random and time-dependent gameplay assertions at their existing contract. Add fixed-clock
comparisons for the complete creature cycle before extending its benchmark. Native hash iteration
and Runtime Script entropy are separate from the timestamp-seeded gameplay random generator;
their effects must be checked rather than inferred from movement parity.

The database engine retains authority, transaction ordering, schedules, commits and publication.
The separate worker's ownership and commit-receipt protocol is therefore not part of this candidate.
Gateway request and subscription interfaces stay the same, but the durable Headless Client tests
must still establish packet behavior. Process limits and recovery remain separate completion gates.

For durable tests, build the normal `debug_reducers,package_test_fixture` feature set twice. Use the
unmodified SDK for Wasm and the private SDK copy for the native library. Restore the checkout's
normal lockfile before compiling the integration tests. The manifest contains `library`,
`native_blake3`, `wasm_blake3` and `"execution": "reducers"`. Keep its absolute artifact path local.
Set the CLI wrapper variables as above and run the same test command through `database/run_durable.py`:

```sh
python3 module/prototypes/native-simulation/database/run_durable.py \
  module /path/to/private-manifest.json /path/to/reference-result.json -- \
  cargo test --release --locked -p lyracore-module --test eventai_runtime_boundaries \
  -- --ignored --test-threads=1
python3 module/prototypes/native-simulation/database/run_durable.py \
  native_database /path/to/private-manifest.json /path/to/native-result.json -- \
  cargo test --release --locked -p lyracore-module --test eventai_runtime_boundaries \
  -- --ignored --test-threads=1
```

The runner records artifact hashes, test names and native reducer coverage. It keeps command output
beside the result, retains fixture logs on failure and removes successful scratch storage. It
requires passing Rust test results and native evidence from every owned fixture process. These
suite durations are verification timings, not gameplay performance measurements.

The first expanded check passed the original sixty-frame movement reference on both implementations,
then repeated the committed-frame, subscription and recovery checks. Its five-sample movement
comparison at 100 creatures remained positive. The run used the default movement selection and
uncommitted changes. Its raw results are in
[native-database-expansion-1.jsonl](native-database-expansion-1.jsonl).

The first all-reducer batch passed the same 26 existing durable tests in both implementations.
The tests cover aura death and healing, cast cooldowns and replacement, combat logs, movement
persistence, death stops, path renewal, EventAI revision and summon expiry, Operator Gates, Package
Runtime Scripts and scripted spell effects. All 26 native fixture processes recorded native
execution, with no fallback. This includes initialization, scheduled creature, melee and aura work,
pending casts and connection cleanup. The paired evidence is in
[native-database-durable-1.json](native-database-durable-1.json).

The remaining Core batch then passed 286 existing durable tests across 313 fixture processes in
each implementation, with native execution evidence and no fallback in every native fixture.
Its results are in [native-database-durable-2.json](native-database-durable-2.json). This batch uses
the clean Core checkpoint `bffd6b1b81ae5b2c3b61c6fec029d5d9e34bfd16`. Its Module artifact hashes
match the first batch, before the aura-ordering change; the recorded runtime binary hashes differ.
Together the batches cover 312 existing durable tests and 364 named native reducers. Installed
Official Packages, Gateway protocol checks and integrated load measurements still gate completion. These suite durations include
concurrent verification work and are not speed comparisons.

### Fixed-time gameplay comparisons

The `native_gameplay_parity` example compares full committed rows after each deterministic input.
It publishes matching `native_simulation_database` artifacts to two disposable persistent Shards.
The native manifest selects all reducers. Each fixture clears application tables, preserves the
Operator claim, reseeds Core at a fixed time and disarms background schedules in one transaction.
The schema supplies the table list, so adding a gameplay table also adds it to the comparison.
`game_operator` is the only excluded table because the two publishers have different identities.

Both implementations must have identical initial rows, identical confirmed subscription updates
after every step, and identical final durable rows. Row order is ignored. All field values,
including floats, must match exactly. A mismatch records both versions of each affected table and
stops the run. Native execution markers are required for initialization, fixture setup and steps;
fallback invalidates the result.

The first reward case kills sixteen creatures in the same aura tick. It exercises the existing
periodic damage, death, XP, money and loot rules. The complete creature-cycle case uses the real
table-backed cycle, active cells, global maintenance, sensing, movement, melee and auras. Separate
assertions require deaths and rewards in the first case, and awake creatures, combat events and
damage in the second. Calling a phase with no eligible Actors does not establish coverage of that
phase's behavior. Navigation obstacles, authored EventAI and installed Package callbacks need
additional fixtures and their existing durable tests.

The scheduled reducers retain their caller Gates and call the same rule functions as the fixture.
The fixture passes a synthetic context to those functions to control time and timestamp-seeded
random draws. It does not establish scheduled-caller authentication or wall-clock scheduling parity;
the unchanged durable tests cover those boundaries. Its per-step elapsed times are diagnostic,
not a performance comparison when other verification work shares the host.

The first simultaneous-death comparison found different per-corpse money, loot rows and XP event
IDs. The same comparison also failed between two Wasm instances with identical starting rows and
rule times. `tick_auras` processed its pending deaths in hash-map order. The same set of money
draws went to different corpses. The continuation orders pending aura outcomes by GUID in both
builds. This changes the previously unspecified order between victims; reward formulas and
aura-ID killing-blow credit stay the same. The comparator still checks each corpse and event
exactly. It does not accept a permutation of rewards as parity.

The ordered implementation passes both cases across all 296 application tables. It also passes
the simultaneous-death comparison between two Wasm instances. The reward assertion checks the
active world entity, which reaches level 2 with 400 XP remaining, and sixteen reward events of
50 XP each. The saved Character row is not the active XP projection. The existing aura test and
the new scheduled simultaneous-death ordering test pass in both implementations. The two comparator
tests and all 1,583 Module unit tests pass. The paired results and source archive hashes are in
[native-gameplay-parity-1.json](native-gameplay-parity-1.json).

### Module publication and mixed versions

The `native_database_lifecycle` example must preserve every application row through sixteen
publications of the same Module. It checks that each replacement enables native execution and
that the number of mapped native libraries stays bounded. It then publishes a Wasm artifact with
an added custom section. Its instructions and schema are unchanged, but its hash differs from the
manifest. This version must run as Wasm, preserve state, and coexist with a matching native Shard
through a process restart. Restoring the matching artifact must enable native execution again.
Use `--wasm` for the reference and `--versions-only` to isolate version selection.

The first publication run preserved application rows but retained two native library mappings per
update. Native process resident memory grew from 304,212 to 416,640 KiB across sixteen updates.
These are diagnostic observations under concurrent verification, not a steady-state memory
benchmark. The initial mapping counter also included one journal mapping; the revised counter
accepts only native library filenames ending in `.so`.

Loader tracing found a second retention cause beyond the SDK's thread-local buffers. The pinned
`tracy-client-sys` dependency exports a `dlclose` replacement that returns success without unloading
anything. This override is present even when no Tracy session is active. The experimental runtime
patch disables Tracy's compiled features so native Module replacement can unload libraries. Normal
LyraCore builds are unchanged. The experimental executable does not provide Tracy profiling.
Publication and recovery checks pass with this executable. Performance comparisons must use
this executable on both paths before it replaces the preceding candidate.

A native manifest selects one exact Wasm artifact. Other programs run as Wasm and emit
`LYRACORE_NATIVE_DATABASE_WASM`. A matching program still requires the native artifact hash and
complete Module description to match. The durable evidence collector refuses both unmatched
programs and unexpected trap fallback, so either condition cannot silently turn a native suite
into a Wasm suite. Mixed-version behavior is checked separately by the lifecycle example.

The revised runtime passes sixteen native updates with two mapped libraries throughout. All
application rows survive both artifact mismatch checks, an unmatched Wasm update, a mixed-version
process restart and restoration of the matching native program. The mismatch checks supply a wrong
native hash and then change a table name in the Wasm data section while keeping registration exports
intact. Both publications fail before changing application rows. The matching sibling Shard records
native execution after restart.

Recovery also passes with all reducers selected. Both implementations match the sixty-frame
movement reference and twenty-one committed frames and subscription updates. Refusals and native
panics preserve the complete pre-call frame. After the first native trap, zero, eight and sixteen
further traps leave one mapped native library. The count decreases from the two libraries observed
before the trap. The boundedness assertion permits unloading and forbids growth. Restart restores
native execution, and the timed movement replay requires fresh native execution evidence with no
fallback after its final restart. These checks and their artifact hashes are in
[native-database-lifecycle-1.json](native-database-lifecycle-1.json). Memory readings in that file
were collected during concurrent verification.

Execution checks use the runtime output for named native markers. Module log reads sometimes
omitted previously observed markers in these runs, while the runtime output retained every call.
Mixed-version checks additionally inspect the matching Shard's Module log.

### Complete-cycle measurements

`native_gameplay_parity` also accepts `--benchmark <creatures> <samples>` for `creature_cycle`.
It resets and reseeds each disposable Shard before every sample, adds three warmups and alternates
which implementation runs first. Every initial snapshot, step update and final snapshot must match.
The corrected native build passes all 96 paired steps from five samples and three warmups at each
of 100, 1,000 and 5,000 creatures. Its timings were collected during concurrent verification and
are diagnostic.

The fixture advances rule time by one second per step and calls the creature cycle, melee and
aura processing once each. It measures this combined workload, not the independent production
schedule cadences. Existing durable tests exercise those scheduled boundaries. Each timing includes
request encoding, acknowledgment and confirmed subscription output. CPU readings cover the runtime
and driver during that interval; they exclude the separate CLI subscriber. Setup and full-row
comparisons are outside the measured interval. No Character capacity claim follows from these
creature counts.

### Floating-point parity

The first 5,000-creature comparison found one different spline destination at step nine. Wasm
stored `dy = -126.01679`; native execution stored `-126.0168`. The native library called the
platform's math functions, which can round differently from the Wasm implementations.

The private SDK now binds the thirteen math imports observed in Core and the Official Packages
to `libm` 0.2.16. Hidden ELF symbols keep these bindings inside each native Module library. They
do not replace the host process's math functions. The normal Wasm build remains unchanged.

`database/check_math.py <new-scratch-directory>` builds Wasm, ordinary native and corrected native
libraries with the pinned Rust toolchain. It compares the bits of fifteen outputs from those
thirteen functions over one million deterministic input pairs, including all four angle quadrants.
Logarithms and powers use positive bases. Every corrected result matches Wasm; every operation
has mismatches in the ordinary native build. This samples the stated input range, not all possible
floating-point inputs. New math imports need the same check before native use.

The full Module then passes the 100-, 1,000- and 5,000-creature comparisons, simultaneous-death
rewards, committed movement, Refusal, panic rollback and restart checks. Artifact hashes, frame
digests and math comparisons are in [native-gameplay-parity-2.json](native-gameplay-parity-2.json).

### Gateway fixture timing

The first grouped Gateway check exhausted its process file descriptors. Separate test processes
removed that accumulation. Two Headless Client tests then exposed a setup race on the Wasm
reference: the Session request was acknowledged before the Coordinator received its projection.
An instrumented run observed an absent Session at that boundary. The fixtures now wait for the
expected Session key before opening their World Sessions. The competing-session fixture waits for
both Coordinators. Handshake, gameplay, ownership and reconnect assertions remain in place.

The cross-Shard Character cleanup fixture also built a second Module with fewer test features.
Its assertions passed, but the execution audit correctly refused to count its Wasm fallback as
native coverage. The fixture now publishes the same built artifact to every Shard and uses
persistent disposable storage for those named Shards. Its gameplay assertions remain unchanged.

The corrected fixtures pass all 30 selected behavior checks with native execution in all 31
fixture processes and no fallback. The reference passes the same checks across the main run and
the two focused fixture reruns. The ordinary Gateway suite passes 2,278 tests, and Clippy passes
for all Gateway targets with warnings denied. The combined evidence records the earlier reference
failure and its successful rerun in [native-gateway-parity-1.json](native-gateway-parity-1.json).
