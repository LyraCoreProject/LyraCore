# Native Module execution

The native path compiles the same Core and Package rules into a shared library and calls them
inside SpacetimeDB's existing transactions. Storage, indexes, schedules, commit, subscriptions and
Gateway requests keep their existing implementation. Wasm remains the default.

An Operator can opt into a matching native artifact for trusted code. The runtime requires finite
Linux cgroup v2 CPU, memory and swap limits before loading it. Native code shares the process's
memory and failure boundary. It does not retain Wasm's per-reducer CPU or memory isolation.
The [parity contract](native-runtime.md) defines the behavior checks and measurement limits.

The earlier in-memory executor and separate commit-batch path have been removed. Their source and
results remain in history at `dc52dde84c9fc870b4b79a2d685ac5a562abe272`. The saved movement reference,
committed replay, complete-cycle comparison and recovery checks remain executable.

## Build matching artifacts

Use Linux benchmark hardware and a separate checkout. Heavy Module builds and benchmarks must not
run on a machine with known CPU instability. The commands below do not publish a Module or update
a running Realm.

Pin Rust to the checkout's toolchain and SpacetimeDB to commit
`8ecd3c3bac427a49a9d9b13671bf83324afa59ba`, version 2.7.1. `database/prepare.py` checks the runtime
revision and the original SDK source before applying either patch. Its SDK argument names the
unmodified crates.io `spacetimedb-2.7.1` directory.

```sh
native_tools="$PWD/module/prototypes/native-simulation/database"
native_scratch=/path/to/disposable-native-runtime
native_sdk=/path/to/registry/spacetimedb-2.7.1
git clone --depth 1 --branch v2.7.1 https://github.com/clockworklabs/SpacetimeDB.git "$native_scratch/server"
python3 "$native_tools/prepare.py" "$native_scratch/server" "$native_sdk" "$native_scratch/sdk"
(
  cd "$native_scratch/server"
  CARGO_BUILD_JOBS=1 CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    cargo +1.93.0 build --release --locked -p spacetimedb-standalone
)
```

Build the Module twice from the same checkout, installed Packages and features. Use the normal SDK
for Wasm. Copy its artifact before building the native library with Cargo's
`patch.crates-io.spacetimedb.path` set to the private SDK copy. Use one Cargo job and one release
code-generation unit for both Module builds. The native build adds `libm` 0.2.16 to the private
SDK and changes that checkout's lockfile. Restore the normal lockfile before other builds.

Ordinary Core and Package execution needs no native feature flag. Use
`debug_reducers,package_test_fixture` for the existing durable integration suites, or
`native_simulation_database` for the deterministic replay and failure fixtures. Never publish those
fixture artifacts to a live Realm. The native and Wasm artifacts must have the same Module
description. Keep the source revision, features and both artifact hashes with each result.

Create a private JSON manifest with an absolute `library` path and the BLAKE3 hashes of both files:

```json
{
  "library": "/path/to/artifacts/liblyracore_module.so",
  "native_blake3": "BLAKE3_OF_THE_NATIVE_LIBRARY",
  "wasm_blake3": "BLAKE3_OF_THE_PUBLISHED_WASM"
}
```

The loader checks the Wasm hash, native hash and complete Module description. A published Wasm
artifact without a matching manifest entry runs as Wasm. A matching entry with a bad native hash
or description fails activation. The native library is trusted executable code; a hash match does
not make an untrusted library safe.

## Run with process limits

`database/run_native.py` supplies the manifest and starts the command in a limited user scope:

```sh
python3 "$native_tools/run_native.py" \
  --manifest /path/to/private-manifest.json --cpu-percent 100 --memory-mib 5120 -- \
  "$native_scratch/server/target/release/spacetimedb-standalone" start \
  --listen-addr 127.0.0.1:3100 --data-dir "$native_scratch/state" --non-interactive
```

The launcher disables swap and enables group termination after an out-of-memory kill. It requires
systemd 253 or later for the [scope OOM policy](https://github.com/systemd/systemd/blob/v259/man/systemd.scope.xml#L97).
A CPU quota of 100 percent allows one CPU's execution time. It throttles a busy loop; it cannot
force that reducer to return. Persistent storage recovers after the process restarts.

Without `LYRACORE_NATIVE_DATABASE_MANIFEST`, the patched runtime executes Wasm. Removing that
variable and restarting the process restores the default. Publication, backup and production
rollout still follow the existing operational procedure.

## Run parity and benchmarks

Build the three release examples with `native_simulation_database` and the normal SDK:

```sh
CARGO_BUILD_JOBS=1 CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  cargo build --release --locked -p lyracore-module --features native_simulation_database \
  --example native_committed_benchmark --example native_gameplay_parity \
  --example native_database_lifecycle
```

Set `SPACETIME_BIN` to an executable copy of `database/spacetime-wrapper.sh`,
`LYRACORE_NATIVE_DATABASE_CLI` to the normal 2.7.1 CLI and `LYRACORE_NATIVE_DATABASE_SERVER` to the
patched standalone binary. Use a private `TMPDIR` and an outer cgroup with identical limits for
both implementations. Do not set the native manifest globally for paired comparisons; their drivers
set it only for the native process.

- `native_committed_benchmark <wasm> <output.jsonl> [creatures ...]` checks the saved 60-frame
  movement reference, 21 complete committed frames, subscriptions, Refusals, panic rollback and
  restart before measuring. Set `LYRACORE_NATIVE_DATABASE_LIBRARY` to the matching native library.
- `native_gameplay_parity <wasm> <library> creature_cycle <output.jsonl> --benchmark <creatures> <samples>`
  compares every application table after each complete creature, melee and aura step. It accepts
  2 to 5,000 creatures and 5 to 100 samples, adds three warmups and alternates execution order.
  Use `periodic_deaths` without benchmark arguments to check simultaneous-death rewards.
- `native_database_lifecycle <wasm> <library> <output.jsonl>` checks repeated publication, artifact
  refusal and mixed native/Wasm versions through restart. Add `--process-failures` to check abort
  and forced termination after uncommitted row changes. Add `--wasm` for the reference.
- `database/run_durable.py <module|native_database> <manifest> <output.json> -- <test command>` runs
  the existing durable or Gateway tests and audits every fixture's execution evidence. Unexpected
  Wasm fallback makes a native suite fail. Successful scratch storage is removed; failed logs remain.
- `database/check_math.py <new-scratch-directory>` compares Wasm and native math output bits over
  one million deterministic input pairs. It needs the pinned Rust toolchain and Node.js.

These commands create disposable loopback Shards. They do not use the live Realm.

## Recorded evidence

| Check | Record |
| --- | --- |
| Movement performance gate, 20 samples at each size | [native-database-benchmark-2.jsonl](native-database-benchmark-2.jsonl) |
| 312 existing Core durable tests | [first batch](native-database-durable-1.json), [remaining batch](native-database-durable-2.json) |
| Exact complete-cycle and math parity | [native-gameplay-parity-2.json](native-gameplay-parity-2.json) |
| Complete-cycle benchmark before prototype removal | [native-full-cycle-benchmark-1.json](native-full-cycle-benchmark-1.json) |
| Final complete-cycle benchmark, 20 samples at each size | [native-full-cycle-benchmark-2.json](native-full-cycle-benchmark-2.json) |
| Scheduled Package travel load, three samples per size and format | [native-package-load-benchmark-1.json](native-package-load-benchmark-1.json) |
| Repeated publication and mixed versions | [native-database-lifecycle-1.json](native-database-lifecycle-1.json) |
| Gateway behavior and ordinary tests | [native-gateway-parity-1.json](native-gateway-parity-1.json) |
| 303 native Package cases and two mixed-version cases | [native-package-parity-1.json](native-package-parity-1.json) |
| Six Package Gateway and two Headless Client cases | [native-package-client-parity-1.json](native-package-client-parity-1.json) |
| Process limits, abort and forced termination | [native-process-recovery-1.json](native-process-recovery-1.json) |
| Parity and recovery after prototype removal | [native-cleanup-parity-1.json](native-cleanup-parity-1.json) |

The final clean complete-cycle comparison at `04dde129` used three warmups and twenty measured
samples per size. Every one of its 828 paired steps matched across all 296 application tables.

| Creatures | Wasm mean, ms | Native mean, ms | Mean time reduction | Runtime CPU reduction |
| --- | ---: | ---: | ---: | ---: |
| 100 | 14.57 | 11.07 | 24.0% | 30.1% |
| 1,000 | 126.29 | 91.27 | 27.7% | 33.5% |
| 5,000 | 638.51 | 476.56 | 25.4% | 31.1% |

At 5,000 creatures, per-step p95 fell from 828.06 to 616.96 ms. Process peak memory was about
310 MiB for Wasm and 328 MiB for native execution. Native resident memory was not uniformly lower.
These are Core gameplay fixtures and CLI subscription measurements, not Character capacity.
Each record identifies its source and artifacts; results from different checkpoints must not be
presented as one run.

The scheduled Package comparison uses Core `0f6d2560`, the final runtime patch and the same pinned
Collection as the Package verification below. It runs the existing `playerbots_capacity_profile_travel`
test with `PLAYERBOTS_CAPACITY_WORLDS=1` and `PLAYERBOTS_CAPACITY_COUNT` set to each size. Each of
three fresh-process samples runs scheduled navigation and Package decisions for 45 seconds in one
in-memory Shard. Both formats share a one-CPU quota, a 5 GiB memory limit and disabled swap.
Setup is excluded from runtime CPU; the measured window includes the final evidence queries and
has no warmup exclusion. The fixture has no Gateway or connected clients.

| Automated Characters | Wasm CPU, % of one CPU | Native CPU, % of one CPU | Wasm passes | Native passes |
| --- | ---: | ---: | ---: | ---: |
| 100 | 13.61 | 10.30 | 3/3 | 3/3 |
| 500 | 47.36 | 32.46 | 3/3 | 3/3 |
| 1,000 | 53.78 | 36.05 | 0/3 | 0/3 |

Runtime CPU fell by 24.3% at 100 Characters and 31.5% at 500. Decision-interval p95 stayed near
one second in both formats at those passing sizes. At 1,000, every sample failed the existing
travel-progress assertion, and decision-interval p95 exceeded two seconds. The Package retains
its limits of 256 decisions and 32 path searches per pass. The highest tested passing load is
500 automated Characters in this fixture; these results do not establish connected-player capacity.
The record retains all six failed samples and their assertion failures.

The broad Package run covers 305 existing tests from Official Package Collection commit
`5248c22835053bf88e97d0d19a31526fdd387140`. All passed on Wasm. Native execution passed 303 cases.
The other two deliberately publish a different Wasm hash, exercise the default Wasm path after
replacement and preserve their gameplay assertions. The strict execution audit reports those two
as mixed-version checks. The record retains interrupted batches, failed attempts and successful
serial rechecks, and lists the cases that need additional fixtures or earlier Module artifacts.

The final runtime patch also passed six Package Gateway cases and two Headless Client cases under
both execution formats. These cover companion commands, combat, death, resurrection, instance
entry and exit, Transfer, process restart and lost acknowledgments. Together, the Package records
cover 313 passing cases: 311 fully native and two intentional mixed-version checks. All 313 passed
on Wasm. Another 21 cases were not run because they need earlier Module artifacts, composed
evidence manifests or imported navigation and client data; the client record lists each one.
