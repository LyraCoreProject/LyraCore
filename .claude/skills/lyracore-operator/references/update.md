# Production update

Run this branch only after the user authorizes an update to a named host. The gateway and all
databases are live state; complete each gate before starting the next.

## 1. Capture the pre-update envelope

On the target, establish the non-interactive tool environment, then verify both executables:

```bash
. "$HOME/.cargo/env"
export PATH="$HOME/.local/bin:$PATH"
command -v cargo
command -v spacetime
spacetime --version
```

Record the checkout's branch, commit, remote, and tracked changes. Untracked runtime files are not a
reason to clean the checkout. Let `./lyracore update` enforce its own tracked-edit refusal.

Capture the current gateway PID, start time, process manager, filtered topology variables, listeners,
and log source. Read only these process variables:

```text
LYRACORE_DATABASE
LYRACORE_AOI
LYRACORE_COORDINATOR_TOKEN
LYRACORE_SPACETIMEDB_URL
LYRACORE_SHARD_MAP
LYRACORE_SHARD_MAP_FILE
LYRACORE_REALM_CORE
LYRACORE_LOGON_BIND
LYRACORE_WORLD_BIND
LYRACORE_REALM_ADDRESS
LYRACORE_METRICS_DB_IDS
LYRACORE_MAX_SESSIONS
LYRACORE_MAX_BLOCKING_THREADS
LYRACORE_ADMIT_CONCURRENCY
MALLOC_ARENA_MAX
RUST_LOG
```

Report `LYRACORE_COORDINATOR_TOKEN=[redacted]` by presence only. If no managed gateway is running,
require an operator-supplied production configuration source before restart.

Derive expected databases from `LYRACORE_DATABASE` (default `lyracore`), every database named by the
shard map, and realm-core. Compare that set with `spacetime list -s local`. Verify that the shard
map routes map `36:*` to the expected instance database and `1:*` to the expected world database.
The fixture name `lyracore-kalimdor` is not evidence that production's `lyracore-world-1` is
missing.

Completion criterion: target, tools, current manager, log source, and all three topology views are
known. A topology mismatch is recorded before proceeding.

## 2. Update the checkout

Run `./lyracore update` from the target checkout. Record its exit status and old/new commits. Its
printed `dev down && dev up` suggestion describes the contributor fixture; keep following this
production branch.

Completion criterion: the checkout is at the intended `origin/main` commit, or the tracked-edit/tool
failure is a blocker and the run stops.

## 3. Run the complete deploy gate

Run each command separately and wait for its process to exit before starting the next:

```bash
cargo test -p lyracore-importer
cargo test -p lyracore-module --lib --features=debug_reducers
cargo test -p lyracore-gateway
cargo test -p lyracore-shared
cargo build
./lyracore preflight
```

Capture every `test result:` line; do not infer a crate's total from its final line. Preflight output
must show the pinned Rust and SpacetimeDB versions, module build, offline schema/default extraction,
and RLS identifier validation. Any critical `SKIP` is a failed deploy gate.

When remote execution yields without a final status, inspect the original remote PID and continue
waiting for that process. Starting a duplicate gate can block on Cargo's workspace lock and destroys
the evidence trail.

Completion criterion: every command exits zero and every critical preflight phase ran.

## 4. Publish the complete topology

Re-check the expected database set immediately before publish. Run one guarded command:

```bash
./lyracore publish <default-db> <world-shards...> <instance-pool> <realm-core>
```

The wrapper publishes sequentially and stops at the first failure. Preserve its output so a partial
publish is named precisely. After success, repeat `spacetime list -s local` and account for every
expected database.

Completion criterion: the wrapper reports success for every database and inventory still matches.
Any partial publish blocks restart and is the first item in Blockers.

## 5. Rebuild and restart the managed gateway

Run `cargo build -p lyracore-gateway` even when the workspace build was warm.

Prefer the gateway's existing service manager. Restart a systemd unit only after its unit name and
effective environment have been positively identified. For the documented manual deployment, match
the exact executable and preserve the captured production environment. Linux truncates the process
comm to `lyracore-gatewa`; confirm the old PID exits and the new PID differs. Keep token extraction
and process launch in the same remote shell so the token never crosses the operator boundary.

Before restart, require the exact production configuration: `LYRACORE_AOI=1`, a present coordinator
token, a map `36:*` instance route, a map `1:*` world route, a distinct realm-core, allocator limit,
and `RUST_LOG`. Stop when the manager is unknown, the old process cannot be identified, those
requirements are absent, the credential source is unavailable, or inventory and configured topology
disagree.

Completion criterion: exactly one replacement gateway is running with the expected sanitized
configuration and a known log source.

## 6. Verify the replacement

Run the repository-pinned CLI against the replacement's log and the explicit production topology:

```bash
./lyracore production status \
  --gateway-log <log> \
  --realm-core <realm-core> \
  <default-db> <world-shards...> <instance-pool> <realm-core>
```

This is the canonical parser for latest-start gateway evidence. Do not replace it with a connection
count or an ad-hoc grep: configured databases and connected databases are separate facts. Also
verify process identity and the actual listening sockets with `ss -ltnp` or the host's equivalent.

Require:

- one distinct coordinator connection per expected database;
- `realm-core active` naming the expected realm-core;
- `shard map active` naming the configured set;
- logon and world listeners;
- zero startup errors or panics.

Report these warnings with impact:

- realm advertises loopback while the world listener is remote: clients return to realm select;
- `LYRACORE_METRICS_DB_IDS` absent: writer occupancy is unmeasured;
- login queue disabled or blocking-pool capacity unresolved: admission capacity is unbounded or
  unknown;
- cross-shard catalogue check unavailable: replicated content parity remains unproved.

Completion criterion: the new gateway stays running through a second observation, every connection
marker is present, both listeners remain bound, and all warnings have impact plus remedy.
