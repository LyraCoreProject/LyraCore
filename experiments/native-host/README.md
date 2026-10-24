# Native SpacetimeDB world protocol experiment

This directory records the initial movement and AOI experiment, before the full Module migration.
Its fixture used a modified SpacetimeDB 2.13.0 host and left the game Module and Gateway unchanged.
The current implementation is in [`native-host/`](../../native-host), with its boundary and
verification described in [the architecture document](../../docs/architecture.md#native-host-migration).
The details and measurements below describe the earlier experiment.

The working design is `shared-edge`. The Module maintains private recipient grants. One internal connection subscribes to movement and grants, and native host code sends packets to the named recipients. The host does not calculate distance, cells, hidden state, or ghost eligibility in this mode. It retains a movement cache and reverse grant index for packet delivery.

Raw TCP and header encryption run in the native host. They do not run in WASM. This therefore requires a maintained SpacetimeDB host fork, not only a different Module.

Read [RESULTS.md](RESULTS.md) for measured results and remaining limits.

## Designs tested

| Variant | Module responsibility | Native host delivery |
| --- | --- | --- |
| `cell` | Cell grants and a query-backed view per viewer | One view subscription per connection |
| `edge` | Entity grants and a query-backed view per viewer | One view subscription per connection |
| `direct-cell` | Cell grants, hidden state, ghost state | One private SQL join per viewer |
| `direct-edge` | Exact entity grants | One private SQL join per viewer |
| `shared-edge` | Exact entity grants | Two shared table subscriptions and recipient routing |

The direct queries use indexes on viewer plus cell or viewer plus guid. The trusted native endpoint supplies fixed SQL and uses owner read authority for private tables. Clients cannot supply SQL. Movement still runs with the authenticated Character identity.

An important correction to the initial research: query-backed views in this SpacetimeDB release are not incremental materialized views. The host records table scans for their read sets and reruns them when those tables change. The source explicitly describes this in [`run_query_for_view`](https://github.com/clockworklabs/SpacetimeDB/blob/08e1aba133671f9c631c6e70133a82faa0e7b283/crates/core/src/host/wasm_common/module_host_actor.rs#L230). Ordinary subscription queries do use incremental evaluation, but distinct per-viewer joins still cost enough to limit these tests.

## Scope and measurement

Tests run on one local Intel i7-7820HQ, four cores and eight hardware threads, with 15 GiB RAM. The host and Headless Client driver share that machine. The host uses a release build, a 1 GiB page pool, disk durability, and confirmed reads. Each run creates a fresh database, data directory, JWT key pair, and loopback endpoints. The runner stops its processes afterward. It never targets an existing Realm.

A spread scene has groups of up to 32 Characters, separated by 400 yards. A dense scene puts everyone in one cell. AOI covers a 5 by 5 set of 50-yard cells. A separate space identifier tests isolation between equal coordinates in different spaces. It does not implement a cross-shard Transfer.

Each Character moves at a scheduled 10 Hz. In-cell movement changes height without changing the cell. Batch ingress submits at most 128 Characters per durable request. Native ingress sends one encrypted WoW movement packet per Character per tick. Native ingress is the relevant test for the proposed world endpoint; batch ingress isolates much of the delivery work.

The timestamp in each movement is its scheduled issue time. Latency includes lateness in the driver, durable processing, delivery, and client decoding. It is not just socket latency. The driver counts every recipient update and checks create-before-movement ordering. Expected delivery is the sum of each group's size squared per tick, including the mover's own update.

Runs request ten seconds of traffic. Batch runs stop early after falling three seconds behind schedule. Native runs allow up to eleven seconds of drain after submission. An incomplete count at that deadline means backlog or loss has not been resolved; it is not proof of permanent packet loss. `kept_up` in the final runner requires all scheduled ticks, all expected updates, and p99 below 100 ms. CPU figures include the drain interval. RSS is a sample, not a peak.

Crossing tests move an entire dense group 160 yards back and forth at 10 Hz in one transaction per tick. They deliberately stress grant churn. Separate correctness checks move one Character across the boundary through the native protocol.

## Correctness checked

The shared-feed path passes world proof refusal, foreign-identity refusal, duplicate World Session refusal, space isolation, hidden and ghost-only entities, viewer ghost changes, cell exit and entry, native movement, and reconnect snapshots. The driver checks that movement never precedes creation and that observed movement timestamps do not go backward.

Three codec tests parse creation, movement, and destruction through the independent `wow_world_messages` vanilla decoder. World proof verification and header encryption use [`wow_srp`](https://github.com/gtker/wow_srp). [`benilla-re`](https://github.com/samwhosung/benilla-re) supplied protocol research context.

The world authentication fixture uses a public, fixed session key. It represents an already established World Session. Full logon SRP, Account storage, realm lists, addon data, Character selection, and a real graphical client login are not implemented. Entity creation uses a minimal unit object, not a complete Character update mask.

This does not test combat bursts, full stealth detection, parties, inventories, instances, cross-shard traffic, host failover, production session security, or a sustained soak. No controlled comparison against the current Gateway has run, so these results do not establish a speedup over LyraCore today.

## Reproduce

Run from this directory. The experiment is a separate Cargo workspace.

```sh
rustup toolchain install 1.97.1 --profile minimal --component rustfmt --component clippy --target wasm32-unknown-unknown
cargo build --release -p native-host-experiment-module --target wasm32-unknown-unknown
cargo build --release -p native-host-experiment-driver
cargo test -p native-host-experiment-wire
python3 prepare-host.py --checkout /path/to/disposable-spacetimedb --build
python3 run.py --host-checkout /path/to/disposable-spacetimedb --variant shared-edge --count 32 --layout dense --seconds 10 --check
python3 run.py --host-checkout /path/to/disposable-spacetimedb --variant shared-edge --count 1000 --layout spread --seconds 10 --ingress native
```

`prepare-host.py` pins upstream commit `08e1aba133671f9c631c6e70133a82faa0e7b283`, adds the native endpoint, and builds the standalone executable. Its first build also builds upstream dependencies. The experiment requires Python 3, OpenSSL, Git, Rust, and the upstream host build dependencies.

Each `runs/<id>/` directory contains result JSON, latency histograms, per-batch timings, raw metrics, and logs. It also contains disposable database files and signing keys, so the directory is ignored. Curated results and a raw evidence archive are kept under `results/` without keys or database files.

The shared feed uses bounded per-connection queues and a two-second socket write deadline. It gives connection writers a scheduling opportunity after each batch of up to 32 internal messages. A full queue ends that World Session. These bounds are experimental settings, not a proven production overload policy.


## Files and review

`module/src/lib.rs` owns movement and AOI grants. `native_world.rs` is copied into the disposable host checkout. `wire/src/lib.rs` contains the world protocol fixture and independent decoder tests. `driver/src/main.rs` is the Headless Client load driver. `run.py` owns isolated startup, checks, timing, and cleanup.

The experiment workspace passes formatting, Clippy with warnings denied, the three codec tests, and Python compilation. The modified native host builds in release mode. This does not run the upstream host test suite or LyraCore's existing full game tests.

Comment review covers the new Rust, Python, TOML, and host integration templates. It passed. The retained interpreter directives and generated lockfile notices are tool-consumed. No explanatory source comments were added.
