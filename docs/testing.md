# Testing

CI runs four workflows on every pull request, forks included. This page lists their commands in
the order to run them locally. A green unit tier does not prove the durable tier, and neither proves
what a real client shows.

## Prerequisites

- Rust through rustup. `rust-toolchain.toml` pins 1.93.0 with rustfmt, clippy and the
  `wasm32-unknown-unknown` target, and rustup installs them on the first `cargo` call in the
  checkout.
- On Linux, a C toolchain, `pkg-config` and the OpenSSL headers (`build-essential pkg-config
  libssl-dev` on Ubuntu). macOS needs only the Xcode Command Line Tools.
- For the durable tier and schema checks: the SpacetimeDB CLI 2.7.1 on `PATH`
  as `spacetime`, with the matching standalone. [`quickstart.md`](./quickstart.md) gives the
  install. Durable test fixtures also accept `SPACETIME_BIN` to locate the CLI.
- For the Runtime Script check: Bun 1.4.3.

None of the tiers needs your own client or a world database. Tests that do need one are ignored by
default (see [Client data](#client-data)).

## Unit tier

From `.github/workflows/rust.yml`. No SpacetimeDB node runs.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p lyracore-conformance -- --nocapture
cargo test -p lyracore-protocol --example benilla_inventory
cargo test -p lyracore-importer
cargo test -p lyracore-module --lib --features=debug_reducers
cargo test -p lyracore-module --test build_script
cargo test -p lyracore-module --test native_debug_gate
cargo test -p lyracore-module --test notification_tables
cargo test -p lyracore-module --test package_api_lint
cargo test -p lyracore-package-delta
cargo test -p lyracore-protocol
cargo test -p lyracore-shared
cargo test -p lyracore-test-support
cargo check --target wasm32-unknown-unknown -p lyracore-module --features=debug_reducers
```

The wasm check builds the feature set `lyracore publish` builds. A default-feature check would skip
the published debug reducers.

CI also builds the Module with every Official Package installed, so a core change that breaks one
fails before the Package's next publish. To run the same check, link each Package from a checkout of
`LyraCoreProject/packages` into `packages/`, run `cargo test -p lyracore-module --test package_api_lint`,
and repeat the wasm check. The named test checks all installed Package source files; the Wasm
compile checks their types and generated registrations.

## Durable tier

From `.github/workflows/module-durable.yml`. Durable tests in `module/tests/` are `#[ignore]`d because
each starts its own SpacetimeDB standalone on a free loopback port, publishes the Module with
`--features=debug_reducers,package_test_fixture`, calls reducers and reads the result. The
`lyracore-test-support` crate owns that fixture. Standalone logs go to `lyracore-standalone-logs`
under the system temp directory.

Build the Module first, so a build failure shows before any fixture starts. Then run one target:

```bash
cargo build --release --target wasm32-unknown-unknown -p lyracore-module --features=debug_reducers,package_test_fixture
cargo test -p lyracore-module --test weather -- --ignored
```

CI runs every target in three parallel jobs with `--test-threads=4 --skip deadmines --skip
lethal_floor`. The Deadmines targets need the separately installed dungeons Package, and the
`lethal_floor` ranged phase is flaky.

CI runs `populated_old_module_upgrades_without_clearing` separately. It builds the Module at
`44b04878f2050936faa61d6f087f3aa1a7a78a46`, sets `LYRACORE_TEST_BASELINE_WASM` to that Wasm,
and publishes the current Module over the populated fixture without clearing it. Other durable
runs must skip that test unless the baseline Wasm is available.

`scripts/durable-test-targets.py` assigns targets by the durations in
`.github/durable-test-seconds.json`. It includes every current target once and estimates 20 seconds
for a new target. `JOB_SECONDS` reserves extra time for the populated-upgrade baseline.
To refresh the estimates, use a successful main run: record each Module target's reported test
duration, rounded up to whole seconds, in the JSON file. Set each job's `JOB_SECONDS` entry to its
total duration minus those target durations. Keep the run reference in the script current.
Run `python3 -m unittest discover -s scripts/tests` to check the assignment.

The Session expiry test checks the five-minute production interval before it shortens the schedule
on its private fixture. It then checks two real scheduled cleanups without a five-minute wait.
The Operator-only `debug_accelerate_session_reaper` control is for disposable test instances;
its shorter interval persists if used on an existing Shard.

## Native protocol tier

`.github/workflows/native-host.yml` builds the pinned SpacetimeDB Host extension with Rust 1.97.1,
then runs the real Module through WoW TCP connections. These checks own temporary standalone
processes and data directories. They do not use an existing Realm.

```bash
python3 native-host/prepare-host.py --checkout .lyracore/native-host/SpacetimeDB --build
(cd .lyracore/native-host/SpacetimeDB && CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_LTO=false cargo +1.97.1 test --release --locked -p spacetimedb-standalone --lib native_world::feed::tests)
cargo build --locked --release --target wasm32-unknown-unknown -p lyracore-module --features=debug_reducers
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone recovery reload
```

CI builds the Host once and reuses the binary until `prepare-host.py`, the Host toolchain, a
patch or a file in `native-host/src` changes. A parallel job runs the patched Host's
`compile_cache`, `retention` and delivery `feed` unit tests in the SpacetimeDB checkout with the
Host toolchain.

`check.py` runs named checks. Without names it runs every shared check on one seeded Disposable
Host. Give names to run only those checks, and use `--list` to print them. Checks run in the order
of the registry, not the order of the names. A shared check covers authentication, Character
lifecycle or one gameplay path listed in [the architecture](./architecture.md#native-protocol-coverage).
After each shared check, the runner ends its World Sessions, deletes the Characters it created and
writes back the rows the check recorded with `host.restore()`. The shared checks pass in the
registry order, in the reverse order and one at a time. Creatures that a check spawns stay in the
world. The runner prints `PASS` or `FAIL` with the check name, and for a failure the Host log lines
written during that check.

These checks get their own Host. CI runs the shared checks and these checks in three parallel jobs:

| Check | Covers |
|---|---|
| `reload` | Development reload with connected clients |
| `retention` | History cleanup, crash recovery and snapshot fallback during development reload |
| `admission-wait` | Login Queue waits longer than 90 seconds |
| `recovery` | A killed Host restarts with durable Character state, connection cleanup and fresh AOI |
| `slow-readers` | A blocked output connection closes while another client stays responsive |
| `accept-recovery` | The listeners recover after descriptor exhaustion |

To add a check, write a function `check_<name>(host)` in `native-host/check.py` and add it to
`SHARED`. `host.enter_world(name)` creates an Account and a Character and returns its World Session.
`native_and_peer(host)` gives the common pair of Characters, Native and Peer, and `spawn()` returns
the guid of the creature it spawns. Before a check changes a seeded row, such as a creature template
or a spell, call `host.restore(table, condition, columns...)`; the runner writes the row back after
the check. Call it the same way before you insert a row that another check could meet, such as a
faction template; the runner then deletes that row. `native-host/host.py`
holds the Disposable Host; `native-host/headless.py` holds the Headless Client and packet decoders.
Native builds use no generated Gateway bindings.

The reload check changes the WASM program hash with a custom section and replaces the Module
20 times under the development Host's definition check and confirms each installed program hash.
Two Headless Clients exchange movement and pings during each replacement and retain their World
Sessions, movement Relays, Speech and items. It also checks malformed WASM and a private-table
rename, which must fail with the development compatibility error in the running Host.
This check preserves gameplay semantics; it does not measure source rebuild time. The patched
Host's `spacetimedb-schema` tests named `development_reload` cover reducer argument changes and
private table additions that ordinary automatic migration permits.

For a Linux movement load measurement, run
`python3 native-host/load.py --host <binary> --clients 1000 --seconds 30 --group-size 16`. The
report includes Host and driver CPU, sampled RSS, latencies, closed connections and final observer
positions. This fixture has no imported world or
Package AI, so it does not establish populated-world capacity.

The independent Benilla codec checks and opcode inventory are described in
[`benilla-compatibility.md`](./benilla-compatibility.md).

## Conformance

The Milestone 1 native reducer check starts its own local Shard and bypasses WoW transport and
SRP through a debug-only reducer. It proves world entry and a ping/pong transaction, with a gtker
decoder used directly by the test. It has no Golden Trace yet. See
[ADR 0005](adr/0005-1-12-conformance.md) for the planned tiers and their ownership.

```bash
export CARGO_TARGET_DIR="$HOME/.cache/lyracore-scratch/conformance/target"
export TMPDIR="$HOME/.cache/lyracore-scratch/conformance/tmp"
mkdir -p "$TMPDIR"
cargo test -p lyracore-module --test native_debug_gate
cargo test -p lyracore-module --test native_conformance -- --ignored
```

The [Feature Ledger](../conformance/README.md) has its own Architecture Test:

```bash
cargo test -p lyracore-conformance -- --nocapture
cargo test -p lyracore-protocol --example benilla_inventory
```

The first command checks every Behavior file and its dependencies. Scenario ids are cross-checked
when the pinned harness supplies a catalogue. Milestone 1's pin has none, so the test prints that
skip. The second command checks retained Rust evidence declarations in the inventory. Neither
command establishes that a Behavior passes its required verification rungs.

The existing durable CI target discovery includes `native_conformance` automatically.
`native_debug_gate` checks that the debug module remains behind the `debug_reducers` feature and
that the check detects a missing or ineffective feature gate.

## Runtime Scripts

From `.github/workflows/runtime-scripts.yml`:

```bash
cd datascripts
bun install --frozen-lockfile
bun test
```

## Client data

These ignored tests read data you own. Set the variable, then run the test with `--ignored`.

| Variable | Holds | Tests |
| --- | --- | --- |
| `LYRACORE_CLIENT_DATA` | your 1.12.1 client's `Data/` directory | the terrain slice self-checks in `importer/src/terrain.rs` |
| `LYRACORE_TEST_DBC` | the DBC files of a build 5875 client | the spell catalogue tests in `importer/src/spell.rs` |
| `LYRACORE_CLASSIC_DB_SQL` | the pinned classic-db dump, decompressed or gzip | the dump tests in `importer/src/world_import.rs` |

For example:

```bash
LYRACORE_CLIENT_DATA=/games/WoW-1.12.1/Data cargo test -p lyracore-importer terrain -- --ignored
```

The complete creature import test loads the pinned dump and client catalogues into a private local
SpacetimeDB node, checks every source position, waits for the shared-world population, and reimports
the populated roster:

```bash
LYRACORE_CLASSIC_DB_SQL=/path/to/classic.sql LYRACORE_TEST_DBC=/path/to/client/Data \
  cargo test -p lyracore-importer full_world_source_loads_every_definition_and_spawn_locally -- --ignored
```

Run the terrain check before an `--apply` after you change a profile anchor.

A Package test runner sets `LYRACORE_TEST_CORE` to the Core checkout it builds against.

## Verification ladder

Each check establishes a different property, and a lower rung does not prove a higher one. A codec
test does not establish native TCP behaviour, and a protocol check does not establish real-client
presentation or world capacity.

| Check | Command or source | Establishes |
| --- | --- | --- |
| Module and protocol tests | `cargo test -p lyracore-module --lib --features=debug_reducers`, `cargo test -p lyracore-protocol` | Gameplay calculations, packet layouts and Architecture Tests |
| Durable Module tests | `module/tests/` with `--ignored` | Transactions in disposable Shards |
| Build and schema | `./lyracore preflight` | The publish feature set and offline schema extraction |
| Native protocol | `python3 native-host/check.py --host <binary>` | TCP authentication, requests, committed state and emitted packets |
| Login Queue | Add `admission-wait` | Admission Gates and a waiting client kept connected past 90 seconds |
| Crash recovery | Add `recovery` | Restart with durable Character and inventory state, connection cleanup and fresh AOI |
| Slow readers | Add `slow-readers` | A blocked output connection closes while another client stays responsive |
| Movement load | `python3 native-host/load.py --host <binary> --clients 1000 --seconds 30 --group-size 16` | Throughput, latency and final observer positions for the stated fixture |
| Real client | WoW build 5875 | Presentation and client interaction |

The native checks create temporary keys, ports and data directories. The crash check kills its own
Host with a Character online; restart must clear stale connections, channel membership and AOI,
keep committed position and inventory, and permit world entry with the saved Session key.

The load check reads Linux `/proc` for Host CPU and sampled RSS. It uses the full Module with seed
fixtures, pre-established world-session keys and synthetic Characters, and it imports no world and
runs no Package AI. Heartbeats can coalesce before the 50 ms movement publication tick, so the
final stop check verifies that every expected observer receives the final position. Driver CPU and
loop delay are reported so a saturated driver is not mistaken for the Host's limit. A movement
throughput result is not a concurrent-Character capacity claim.
[`full-module-1000.json`](../native-host/results/full-module-1000.json) is a recorded run with the
Module and Host binary hashes.

A live stack, a real client and a load run each catch what the tiers above cannot. The
`verify-lyracore` skill drives those checks, and
[its checklist](../.claude/skills/verify-lyracore/checks.md) says what a person must see on a real
client.

`dev smoke` fetches the wire harness that `.wire-harness-rev` pins into the ignored
`.lyracore/wire-harness/<sha>/`. A harness change is a harness release and a pin bump, never a
patch to that cache from a LyraCore pull request.
