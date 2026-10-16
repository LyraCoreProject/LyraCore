# Native simulation prototype

The continuation uses the [native runtime parity contract](native-runtime.md). It adds native
execution of the existing Module movement rules through indexed memory tables. Run that comparison
on the benchmark machine with `bash module/prototypes/native-simulation/run.sh host-benchmark`.
This path remains experimental and does not own durable state or Gateway publication.

The next experiment commits each tick to a disposable persistent Shard and compares confirmed
subscription updates. Its contract and limits are in [native-runtime.md](native-runtime.md).
Run it on suitable benchmark hardware with:

```sh
bash module/prototypes/native-simulation/run.sh commit-benchmark target/committed-movement.jsonl
python3 module/prototypes/native-simulation/summarize_commits.py target/committed-movement.jsonl
```

Both implementations use a persistent binary WebSocket connection. The native timing includes
execution, row-change encoding and durable commit. Both include the confirmed subscription's CLI
decoding and output. The script checks committed frames, subscription rows, duplicate delivery,
refusal atomicity and restart behavior before measuring. These are still short movement replays;
they do not cover the complete creature cycle or Gateway packet delivery.

This throwaway prototype asks whether creature movement can run in native Rust with in-memory
state while preserving the current Module's observable results. The reference uses movement rules
from Core commit
`44b04878f2050936faa61d6f087f3aa1a7a78a46`. Normal builds retain the current authority model.

Run the native demo from the repo root. It prints the input and complete state after each step.

```sh
bash module/prototypes/native-simulation/run.sh demo
bash module/prototypes/native-simulation/run.sh demo --list
bash module/prototypes/native-simulation/run.sh check
```

The demo defaults to a moving target that turns and stops. Pass any listed scenario to replay it.
`check` compares against the saved Module trace and never starts SpacetimeDB. The script limits Cargo
and code generation to one job, and restricts itself and its children to one CPU when `taskset` is
available. The demo and checks use unoptimized builds.

## Parity contract, before implementation

Give both implementations the same initial rows and the same ordered commands at explicit times.
Compare after every command, including commands that do not change state. Compare all fields of
`WorldEntity`, `CreatureSpline`, and `MeleeAttack`, with rows sorted by their primary keys. Compare
integers, identifiers, flags, timestamps, row presence, and strings exactly. Allow at most
`0.00001` absolute difference in floating-point fields. A mismatch must name the scenario, command,
row, and field. The comparator must have tests that deliberately change expected results.

Use the existing movement and pursuit rules in both implementations. The native implementation
owns its rows in memory. The reference uses the actual table-backed `CtxWorld`, including spline
emission and `settle_advances`. Comparing two copies of an in-memory implementation is insufficient.
The reference uses a disposable local Shard with no imported world content. Its clock is controlled,
so scheduling delays cannot change results. Never generate expected traces with the candidate.

The first slice covers straight splines, stopped and moving pursuit targets, turns, facing,
arrival, immobilization, missing movers, grid crossings, and the four-yard position persistence
threshold. A fixture has one instance, flat terrain, no navigation obstacles, no auras, no random
choices, and no Package hooks. The only authored movement input is the immobilization switch.
Target movement and removal are fixture inputs, not replacements for the Character movement or
creature death paths. The prototype refuses unsupported inputs before changing state.

Observe both stored rows and the spline needed to reconstruct client-visible movement. The current
Module advances a creature for its calculations, then may restore its stored position within the
same transaction. Persisting every intermediate position would change subscription traffic and
must fail the comparison. Each reference scenario runs in one transaction and records table state
after each step, before fixture cleanup. These snapshots check the rows a Relay would consume.
They do not verify committed subscription events, Gateway packet ordering, or client behavior.

## Result and limits

The real table-backed Module matched all 60 snapshots across eight scenarios and 52 commands.
The native checks also cover duplicate and out-of-order sequences, clock reversal, unsupported
movement, refusal atomicity, independent simulation state, and deliberate comparator mismatches.
These command checks belong to the prototype interface; they do not establish durable retry handling.

The answer to the first question is yes for this movement slice. The native process runs the
existing advancement and pursuit functions against indexed in-memory slots. It preserves the
Module's row projection, including suppressed intermediate position updates. It does not run
`run_cycle` as a whole. Combat flags, damage, rewards, AI senses, Package execution and other cycle
phases remain outside this slice.

The prototype retains complete Module rows. The demo serializes diagnostic snapshots on every command;
the benchmark applies the same commands without snapshots in its timed section.
Elapsed movement-phase timings are recorded below. Memory savings and supported Character counts
remain unmeasured. This prototype does not implement the final compact simulation layout.

Everything is behind the non-default `native_simulation_prototype` feature. Normal builds and the
Gateway keep their current behavior. The extra reference reducer exists only in prototype Wasm
builds. Do not publish such an artifact to a Realm.

The captured output is [reference.jsonl](reference.jsonl). Its baseline, artifact hash and frame
counts are in [reference-provenance.json](reference-provenance.json). To recapture it on suitable
hardware with the pinned SpacetimeDB CLI and Wasm toolchain:

```sh
bash module/prototypes/native-simulation/run.sh reference
```

This command builds prototype Wasm, starts one disposable loopback-only Shard, and runs the eight
scenarios serially with at most two fixture entities. It compares the result before writing
`target/native-simulation-reference.jsonl`. It does not overwrite the checked-in reference. Review
behavior changes before replacing that file or its provenance. One reference run took about
17 seconds here; no sustained workload or capacity test ran.

## Gates before expanding authority

| Area | Required comparison |
| --- | --- |
| Movement and AOI | Paths, terrain, doors, roots, stuns, fear, knockback, teleport, transports, cell crossings, create/update/destroy packets |
| AI and combat | Aggro, assist, threat, taunt, leash, evade, pets, timers, melee, spells, auras, interrupts, death and respawn, with recorded random draws |
| Durable outcomes | Identical inventory, money, loot, quest credit, experience, reputation, and cooldowns after accepted and refused requests |
| Packages | Existing Package tests, Runtime Script hooks, EventAI, encounter state and callback order |
| Sessions and Transfer | Login, logout, reconnect, instance entry/exit, transfer failure and recovery |
| Failure recovery | Crash before/after durable commit, duplicate and reordered commands, stale authority, replay from a checkpoint, exactly one reward |
| Protocol | Headless Client traces through the unchanged Gateway and targeted real-client checks |
| Capacity | Same content and activity, compare tick latency, CPU, memory, table changes and bytes per World Session after correctness passes |

An authority change needs a Shard ownership epoch checked by the Module on every durable
outcome. Durable commands need stable identifiers and stored receipts so a retry cannot duplicate
rewards. Recovery needs a defined checkpoint and command replay policy. Those mechanisms are design
requirements, not capabilities supplied by this movement prototype. Do not route real instances to it.

Expand one behavior at a time, add reference scenarios first, and fail on the first divergence.
Keep the existing Module authoritative until the full comparison and failure checks pass. A later
shadow run must consume copied inputs without publishing gameplay outcomes.

## Local workload limit

Build with one Cargo job. Keep checks deterministic, small, and serial. Do not run capacity tests,
populate a world, or start a sustained SpacetimeDB workload on the current machine. The native
checks and the opt-in table-backed comparison are separate commands.

## Movement benchmark

Run this mode only on hardware suitable for sustained compilation and SpacetimeDB work:

```sh
bash module/prototypes/native-simulation/run.sh benchmark
```

It builds both implementations in release mode with one code-generation unit, then runs all native
and table-backed parity checks against that artifact. The benchmark creates its own loopback-only
SpacetimeDB process with in-memory storage. It never connects to an existing Shard. On a shared host,
use a process scope with a memory limit, pin it to one spare CPU, and lower its scheduling priority.
The script inherits that affinity. Artifact builds also use one Cargo job.
The driver removes the initial map-zero demo creatures from its disposable Shard before seeding
the benchmark. Each sample requires empty movement tables.

Each workload replicates an existing scenario with independent creature identifiers. `idle_drift`
advances a straight spline with suppressed intermediate position persistence. `combat_advance`
persists each advance. `stopped_target` pursues a stationary target with one target per creature.
The default counts are 100, 1,000 and 5,000 creatures. These are short seven-tick encounters,
including their arrivals, not continuous movement or concurrent Character capacity tests.

Before timing, four copies of each workload must match every field after every tick. For every
timed sample, a BLAKE3 digest of the complete final state must also match exactly. A digest difference
stops the run even if it would fit the float tolerance used by the detailed comparator.

The native timer uses `Instant`; the Module uses SpacetimeDB's host-side `LogStopwatch`. Both bracket
the seven movement ticks. Fixture construction, JSON snapshots, hashing, logging, cleanup and CLI
calls are outside those sections. Each Module sample runs in one transaction, so this comparison
also excludes per-tick commit costs, subscriptions, durability and network work. It measures the
current movement implementation against the native prototype's complete in-memory rows. It does not
measure a compact layout or the whole creature cycle.

Fixture rows are inserted, exercised and removed in the same transaction. The Module therefore
operates on uncommitted fixture rows, while normal movement also reads previously committed rows.
The native adapter assumes omitted systems are absent; the Module still checks their empty tables.
The measured difference combines storage, runtime and adapter costs. Extending the native adapter
to cover the full game adds work. Confirm any architectural decision with committed state and the
missing gameplay systems before projecting these ratios onto a Realm.

Each workload gets three warmups and twenty measured samples. Native-first and Module-first samples
alternate. The output retains raw samples, state digests, artifact hashes, compiler versions and CPU
affinity. The summary reports median and p95 across the seven-tick averages. These are not individual
tick latency percentiles. Incomplete output cannot produce a summary.

To select counts or rerun the summary:

```sh
bash module/prototypes/native-simulation/run.sh benchmark target/movement-benchmark.jsonl 100 1000
python3 module/prototypes/native-simulation/summarize.py target/movement-benchmark.jsonl
```

The output path must not already exist. `LYRACORE_BENCHMARK_SAMPLES` changes the measured sample count,
with a range of 5 to 100. A complete run is still a movement cost comparison. A capacity claim needs
the gameplay, persistence, recovery and protocol checks listed above.

## Measurements on 2026-10-15

The measured source is commit `8d63cdfcc0b8e396ec1a61afb48c8897539404aa`, with Rust 1.93.0 and
SpacetimeDB 2.7.1. Both implementations used the same CPU on a shared 8-vCPU AMD EPYC-Rome KVM host.
The process scope had a one-CPU quota, a 5 GiB memory limit, no swap allowance and scheduling niceness
10. No existing Shard was accessed.

The first run covered 100, 1,000 and 5,000 creatures, with twenty samples after three warmups for
each scenario and count. At 5,000 creatures, median elapsed time per tick was:

| Scenario | Native, ms | Module, ms | Module / native |
| --- | ---: | ---: | ---: |
| Idle movement | 0.889 | 75.908 | 85.4x |
| Movement in combat | 0.879 | 68.901 | 78.4x |
| Pursuit of a stationary target | 1.926 | 178.442 | 92.7x |

These are medians of seven-tick averages. The pursuit workload has 5,000 creatures and 5,000 target
entities. The other workloads have 5,000 entities each. The complete first run is
[benchmark-run-1.jsonl](benchmark-run-1.jsonl); the summary command above also reports p95 values.

A fresh process repeated the three 5,000-creature workloads with the same artifacts and twenty
samples each. Module medians stayed within 2.5% of the first run; native medians were 4.4% to 9.2%
lower. The repeat measured 84.0x to 104.4x, compared with 78.4x to 92.7x in the first run. Its raw
output is [benchmark-run-2.jsonl](benchmark-run-2.jsonl).

All 276 final-state digests matched across both runs, including 240 measured samples and 36 warmups.
The scaled comparison also matched every field after each tick in both runs. All thirteen release
parity tests passed against the same Module artifact, including the original eight-scenario trace.
Shared host scheduling and the limited workload remain constraints on interpreting these timings.

The result supports continuing the movement prototype. It does not establish the gain from a full
authority change. The next comparison should use committed state and include movement publication
through the Gateway. Keep the existing Module authoritative while adding that comparison and the
missing gameplay and recovery behavior.

## Existing Module rules in the native host

Commit `106ec25c4330f8c8d5ee2db0c9161c9b7d3772de` adds a stronger comparison. The native host runs
the same `CtxWorld` movement and pursuit code as the Module, including its table operations.
It includes the memory transaction and collection of net row changes. Durable commits and
subscriptions remain outside this measurement.

The first host implementation lost most of the original prototype's advantage. Profiling found
redundant row serialization and updates to indexes whose fields had not changed. Removing those
operations improved the native host while preserving every checked frame.

The fresh comparison used the same CPU and resource limits described above. At 5,000 creatures:

| Scenario | Native host, ms | Module, ms | Time reduction |
| --- | ---: | ---: | ---: |
| Idle movement | 64.640 | 75.817 | 14.7% |
| Movement in combat | 57.210 | 69.906 | 18.2% |
| Pursuit of a stationary target | 120.797 | 179.099 | 32.6% |

These are medians of seven-tick averages, with twenty samples after three warmups. All nine
scenario and size combinations favored the native host. All 207 compared final-state digests
matched, including warmups. The sixty-frame Wasm reference, scaled per-tick comparison and native
storage checks passed before timing. Raw results are in
[native-host-benchmark-1.jsonl](native-host-benchmark-1.jsonl).

This supports the next committed-state experiment. The original prototype's much larger ratios
do not describe the native host or a complete gameplay rewrite. These fixtures still have no
authored encounters, spells, obstacles or active Package callbacks.

## Committed movement results

Commit `31af299eae76c7653680e2af5e250cd84d8ad329` includes faster native row decoding, indexed
durable updates and checks for stale input, retries and rollback after a partial update. The clean
comparison used persistent storage and one transaction per tick. Its timing includes request
encoding, native work, the acknowledgment and confirmed subscription output through the CLI.

The run used the same host limits described above, with twenty samples after three warmups for
each of the three 5,000-creature workloads. Results use individual ticks:

| Scenario | Module mean, ms | Native host mean, ms | Native change | Module p95, ms | Native host p95, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Idle movement | 211.203 | 220.025 | 4.2% slower | 342.773 | 396.081 |
| Movement in combat | 281.024 | 302.178 | 7.5% slower | 365.485 | 402.413 |
| Pursuit of a stationary target | 322.772 | 297.672 | 7.8% faster | 484.237 | 459.895 |

All 483 paired ticks had matching subscription rows, and all 69 paired encounter results had
matching committed state digests, including warmups. The five commit checks, thirteen movement
prototype checks and 1,583 Module unit tests passed at this source revision. Raw results are in
[native-committed-benchmark-1.jsonl](native-committed-benchmark-1.jsonl). Reproduce the summary with
`python3 module/prototypes/native-simulation/summarize_commits.py module/prototypes/native-simulation/native-committed-benchmark-1.jsonl`.

This result does not pass the performance gate for expanding the separate worker. The native host
updates its own tables, then asks SpacetimeDB to apply the changes again. This extra work erases the
raw execution gain in two workloads. The CLI subscriber also adds JSON decoding and printing;
these numbers are not Gateway latency or Character capacity. The next candidate and its required
checks are in [native-runtime.md](native-runtime.md#alternative-to-a-separate-commit-pass).

## Native execution in the database engine

Commit `3d62194fb9cf59b38fe9ffd364a53b22d5ad8ac5` executes the native Module against the existing
SpacetimeDB transaction engine. Commit, storage and subscription handling use the same code as the
Wasm reference. There is no second row store or commit-batch application.

The clean comparison used twenty samples after three warmups at 100, 1,000 and 5,000 creatures.
It retained the same resource limits, persistent storage and confirmed CLI subscription timing.
All nine combinations improved in both mean and p95 time. At 5,000 creatures:

| Scenario | Wasm mean, ms | Native mean, ms | Time reduction | Wasm p95, ms | Native p95, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Idle movement | 224.249 | 196.811 | 12.2% | 359.269 | 334.244 |
| Movement in combat | 288.635 | 264.553 | 8.3% | 367.940 | 341.113 |
| Pursuit of a stationary target | 337.325 | 275.066 | 18.5% | 499.647 | 426.785 |

Measured database process CPU time fell by 24.8%, 19.3% and 27.8%, respectively. These CPU counts
exclude the separate CLI subscriber. Maximum sampled resident memory was about 11 to 12 MiB higher
for native execution. All 1,449 paired ticks had matching subscription rows, and all 207 paired
encounters had matching committed state digests, including warmups.

Before timing, the run passed complete committed-frame comparisons, authorization, stale-input,
Refusal, panic rollback and restart checks. After a trap, replacement instances use the Wasm
Module. Sixteen further traps added no library mappings, and native resident memory remained
within 200 KiB of its first post-trap sample. Both processes restarted before timing.

Raw measurements and artifact hashes are in
[native-database-benchmark-2.jsonl](native-database-benchmark-2.jsonl). This passes the movement
performance gate for expanding native execution. It still measures movement fixtures and CLI
publication, not the complete creature cycle, Gateway latency or Character capacity.
