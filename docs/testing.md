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
cargo test -p lyracore-importer
cargo test -p lyracore-module --lib --features=debug_reducers
cargo test -p lyracore-module --test build_script
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
cargo build --locked --release --target wasm32-unknown-unknown -p lyracore-module --features=debug_reducers
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone --check-admission
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone --check-recovery
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone --check-reload
python3 native-host/check.py --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone --check-slow-readers
```

The normal check covers authentication, Character lifecycle and the gameplay paths listed in
[the architecture](./architecture.md#native-host-migration). The recovery check kills its own Host
and checks committed state after restart. The slow-reader check fills one connection's output while
another remains responsive. Native builds use no generated Gateway bindings.

The reload check changes the WASM program hash with a custom section and replaces the Module
three times under the development Host's definition check. Two connected Headless Clients retain
their World Sessions, movement Relays, Speech and items. It also checks a malformed-WASM update.
This check preserves gameplay semantics; it does not measure source rebuild time. The patched
Host's `spacetimedb-schema` tests named `development_reload` cover reducer argument changes and
private table additions that ordinary automatic migration permits.

For a Linux movement load measurement, add `--load-clients 1000 --load-seconds 30
--load-group-size 16` to the check command. The report includes Host and driver CPU, sampled RSS,
latencies, closed connections and final observer positions. This fixture has no imported world or
Package AI, so it does not establish populated-world capacity.

The independent Benilla codec checks and opcode inventory are described in
[`benilla-compatibility.md`](./benilla-compatibility.md).

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
| `LYRACORE_CLASSIC_DB_SQL` | the pinned classic-db dump, decompressed or gzip | the dump tests in `importer/src/main.rs` |

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

## Higher rungs

A live stack, a real client and a load run each catch what the tiers above cannot.
[`architecture.md`](./architecture.md#verification) describes the ladder, and
[`verification/`](./verification/README.md) holds the written checks.
