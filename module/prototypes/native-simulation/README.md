# Native simulation prototype

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
It is a parity baseline, not a capacity measurement or the final compact simulation layout. No
throughput, memory saving, or supported Character count has been established.

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

An authority change needs an instance ownership epoch checked by the Module on every durable
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
