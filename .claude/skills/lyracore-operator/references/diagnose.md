# Read-only realm diagnosis

Gather the cheapest evidence first. Use the approved target and expected topology established by the
production contract. Keep the run read-only: status, inventory, process metadata, listeners, logs,
schema description, and `SELECT` queries. Ask for separate authorization before any publish, reducer,
import, SQL write, service restart, or node-death test.

## 1. Fix the target and time window

Record host, checkout, commit, node URI, current UTC time, gateway PID/start time, and whether the
gateway and SpacetimeDB are systemd-managed or manual. Identify log sources:

- production standalone: its systemd journal and `/var/log/lyracore/spacetimedb-standalone.log`;
- documented manual production gateway: `/tmp/gw.log`;
- contributor fixture: `.lyracore/logs/{spacetime,gateway}.log`.

Treat `lyracore dev status` as fixture evidence only. It owns fixture database names and cannot
validate a production topology.

Completion criterion: every observation has a target process and bounded time window.

## 2. Walk the failure layers

Collect and classify in this order:

1. **Prerequisites:** `cargo`, `spacetime`, pinned versions, and the node URI.
2. **Node:** standalone service state, PID/restarts, loopback listener on 3000, and bounded error-log
   tail.
3. **Inventory:** `spacetime list -s <node>` and expected database names from the approved source.
4. **Gateway identity:** PID, start time, manager, executable, and only the named variables from the
   production contract. Render the coordinator token as present/absent.
5. **Gateway connectivity:** run the production contract's latest-start health probe. It isolates
   the latest-start segment and distinguishes configured shards from successful coordinator
   connections.
6. **Client path:** logon/world listeners, advertised realm address, and recent accept/login errors.
7. **Capacity:** queue settings, blocking-pool setting, file-descriptor startup line, `QUEUESTAT`, and
   `SHARDLOAD` warnings.

Completion criterion: the first failing layer and every downstream symptom are separated. Do not
name a downstream symptom as the root cause when an upstream layer already failed.

## 3. Use narrow SQL only when logs leave a question

SpacetimeDB SQL lacks `ORDER BY`, `IN`, and subqueries. Query one database and one fact at a time.
State the question before running the query.

**Advertised address on each database**

```bash
spacetime sql -s <node> <database> "SELECT id, name, address FROM game_realm"
```

Expected: every database advertises the same externally reachable `host:port`. Loopback is valid
only for a loopback client.

**Realm-core load samples for one shard**

```bash
spacetime sql -s <node> <realm-core> "SELECT shard, sessions, updated_at_micros FROM game_shard_load_total WHERE shard = '<database>'"
```

Expected: one recently updated row per connected shard. Absence supports a sampling/connectivity
finding; it does not prove the database is down.

**Feature configuration on one shard**

```bash
spacetime sql -s <node> <database> "SELECT vmap_enabled FROM game_config WHERE id = 0"
```

Use only when the symptom concerns collision/VMAP rollout parity. Repeat separately for every world
and instance shard.

Private authentication tables are intentionally not a generic SQL diagnostic surface. Diagnose
coordinator access through `coordinator subscriptions applied`, explicit access errors, and gateway
behavior without dumping accounts, sessions, keys, or operator identities.

Completion criterion: every SQL result answers a named unresolved question and contains no private
authentication data.

## 4. Map evidence to findings

- Inventory missing a configured database: **topology blocker**; do not restart into it.
- `shard map active` lists four but fewer than four distinct connection markers: **configured but
  disconnected shard**.
- Realm-core marker absent or wrong: **authentication outage risk**; the listener can still look
  healthy.
- Coordinator subscriptions absent after connection: **credential/private-table access failure**.
- Public world listener plus loopback `game_realm.address`: **client-breaking advertisement**;
  request separate authorization to run `set_realm_address` on every database. Diagnosis does not
  invoke the reducer.
- Metrics IDs absent or `occupancy=unmeasured`: **observability gap**; record a separate `WARN`,
  never `FAIL` or a blocker by itself. Preserve `PASS` for gameplay and connectivity when their
  evidence passes.
- Listener absent with a live process: **startup/bind failure**; inspect the first error after the
  latest gateway start.
- Repeated `QUEUESTAT` depth with no admission: **capacity/admission problem**; preserve settings and
  timestamps before proposing changes.

Completion criterion: each finding cites a bounded command, SQL result, or log marker and states
operator impact plus the smallest safe next action.
