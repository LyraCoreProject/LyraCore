# Production target and topology contract

Use this contract for both update and diagnosis.

## 1. Establish an independent authority

Require an approved production configuration source that exists independently of the running
gateway. It may be a reviewed service unit plus its referenced environment file, a versioned deploy
configuration, or an operator-supplied checked launch configuration. The running process environment
is observed state, never the authority from which expected topology is inferred.

The approved source must identify:

- host, checkout, and SpacetimeDB server selector or URI;
- default database, complete shard map, and realm-core database;
- gateway manager, executable, log source, and listener binds;
- AOI setting, coordinator credential source, allocator limit, and log level.

Stop with a blocker when the source is missing, ambiguous, or does not name the complete production
topology. Finish when every later command has one explicit host, checkout, node, and database set.

## 2. Derive the expected topology

Derive expected databases only from the approved source: the default database, every shard-map
target, and realm-core. Require unique names, a map `36:*` instance route, a map `1:*` world route,
and a realm-core distinct from the world databases.

Query the approved node explicitly:

```bash
spacetime list -s <node>
```

Diagnosis may use any explicit SpacetimeDB nickname, host, or URL. Update mode requires the approved
node identifier to be exactly `local`: `./lyracore publish` is hardwired to `-s local` and cannot
publish to another selector. A host or URL that happens to reach the same process is still a blocker
because the update would validate one selector and publish through another.

The contributor fixture is not production evidence. In particular, `lyracore-kalimdor` does not
substitute for a configured production world shard.

Finish when the expected set and discovered inventory agree. In update mode, also finish only when
the approved node identifier is exactly `local`. Either mismatch blocks mutation, publish, and
restart.

## 3. Compare sanitized live state

Record the live gateway PID, start time, manager, executable, listeners, log source, and only these
configuration keys:

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

Render `LYRACORE_COORDINATOR_TOKEN=[redacted]` by presence only. Keep every credential remote, pass
it from its remote source directly to the remote process, and never emit a full environment dump.

Compare three independent views in the report:

1. expected databases, node, shard map, and realm-core from the approved source;
2. discovered inventory from the approved node;
3. sanitized configuration observed on the live gateway.

Finish when all three agree or each mismatch is a named blocker. Missing live state is evidence in
diagnosis; it never changes what the approved source says should exist.

## 4. Prove latest-start health

Use the repository-pinned parser with the same explicit node and expected database set:

```bash
./lyracore production status \
  --server <node> \
  --gateway-log <log> \
  --realm-core <realm-core> \
  <database>...
```

Configured databases prove intent. A distinct `coordinator connected to shard <database>` marker
for every expected database proves connectivity. Listener log markers do not prove that the process
still owns its sockets, so inspect actual sockets separately.

Finish when status accounts for every expected database, realm-core, startup error, and listener.
