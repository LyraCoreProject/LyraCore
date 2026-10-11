# Testing

CI runs three workflows on every pull request, forks included. This page lists their commands in
the order to run them locally. A green unit tier does not prove the durable tier, and neither proves
what a real client shows.

## Prerequisites

- Rust through rustup. `rust-toolchain.toml` pins 1.93.0 with rustfmt, clippy and the
  `wasm32-unknown-unknown` target, and rustup installs them on the first `cargo` call in the
  checkout.
- On Linux, a C toolchain, `pkg-config` and the OpenSSL headers (`build-essential pkg-config
  libssl-dev` on Ubuntu). macOS needs only the Xcode Command Line Tools.
- For the durable tier: the SpacetimeDB CLI 2.7.1 on `PATH` as `spacetime`, or named by
  `SPACETIME_BIN`. [`quickstart.md`](./quickstart.md) gives the install.
- For the Runtime Script check: Bun 1.3.7.

None of the tiers needs your own client or a world database. Tests that do need one are ignored by
default (see [Client data](#client-data)).

## Unit tier

From `.github/workflows/rust.yml`. No SpacetimeDB node runs.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p lyracore-importer
cargo test -p lyracore-module --lib --features=debug_reducers
cargo test -p lyracore-module --test package_api_lint
cargo test -p lyracore-package-delta
cargo test -p lyracore-gateway
cargo test -p lyracore-shared
cargo test -p lyracore-test-support
cargo check --target wasm32-unknown-unknown -p lyracore-module --features=debug_reducers
```

The wasm check builds the feature set `lyracore publish` builds. A default-feature check would skip
the published debug reducers.

CI also builds the Module with every Official Package installed, so a core change that breaks one
fails before the Package's next publish. To run the same check, link each Package from a checkout of
`LyraCoreProject/packages` into `packages/` and repeat the wasm check.

## Durable tier

From `.github/workflows/module-durable.yml`. Each test in `module/tests/` is `#[ignore]`d because it
starts its own SpacetimeDB standalone on a free loopback port, publishes the Module with
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

`scripts/durable-test-targets.py` assigns targets by the durations in
`.github/durable-test-seconds.json`. It includes every current target once and estimates 20 seconds
for a new target. `JOB_SECONDS` reserves time for each job's builds, setup and Gateway checks.
To refresh the estimates, use a successful main run: record each Module target's reported test
duration, rounded up to whole seconds, in the JSON file. Set each job's `JOB_SECONDS` entry to its
total duration minus those target durations. Keep the run reference in the script current.
Run `python3 -m unittest discover -s scripts/tests` to check the assignment.

The Session expiry test checks the five-minute production interval before it shortens the schedule
on its private fixture. It then checks two real scheduled cleanups without a five-minute wait.
The Operator-only `debug_accelerate_session_reaper` control is for disposable test instances;
its shorter interval persists if used on an existing Shard.

CI also runs Gateway tests that need a node. The workflow names individual tests with
`--ignored --exact` and runs the party command test group with:

```bash
cargo test -p lyracore-gateway --bin lyracore-gateway stdb::subscriptions::party_command_durable_tests -- --ignored --test-threads=1
```

## Gateway bindings

Also in `module-durable.yml`. The check regenerates the full private schema into a temporary
directory and compares it with `gateway/src/stdb/bindings`. It needs the SpacetimeDB 2.7.1 CLI and
publishes nothing.

```bash
python3 -m unittest discover -s scripts/tests
scripts/check-gateway-bindings.py
```

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

Run the terrain check before an `--apply` after you change a profile anchor.

A Package test runner sets `LYRACORE_TEST_CORE` to the Core checkout it builds against.

## Higher rungs

A live stack, a real client and a load run each catch what the tiers above cannot.
[`architecture.md`](./architecture.md#8-verification) describes the ladder, and
[`verification/`](./verification/README.md) holds the written checks.
