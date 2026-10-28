# Operate a native Realm

This runbook updates a deployed Realm. A Realm runs one Native Host and one Shard. The Host owns
the HTTP, logon and world listeners. The Module owns Accounts, gameplay, AOI and packet
construction.

These steps apply to an existing native deployment. A multi-Shard Realm needs a separate reviewed
data migration first; see [Multi-Shard Realms](migrating-to-native-host.md#multi-shard-realms). Read
[danger zones](danger-zones.md) before you change a schema or import content. Local
`./lyracore dev up` creates an isolated native data directory; it is not a production service
manager.

## Establish the target

Use a reviewed service unit and its configuration, or an Operator-supplied launch configuration.
Record these facts before a write:

- Host, checkout, current commit and intended commit.
- Native Host executable, service manager, data directory, key paths and log source.
- Explicit loopback HTTP URI and the one Shard name.
- `LYRACORE_NATIVE_DATABASE`, `LYRACORE_NATIVE_LOGON_ADDR` and `LYRACORE_NATIVE_WORLD_ADDR`.
- Published `game_realm.address`, the private publisher configuration path and file permissions.
- Any other Realm on the machine, so its processes, ports and data stay outside the operation.

The process environment is observed state, not the source of the intended configuration. Do not
print the publisher token, private key, Account verifiers or Session keys. Keep credentials on the
target machine. `.lyracore/native-cli.toml` must contain the identity that owns this Shard and have
mode 0600. The CLI stores it during native startup. A managed deployment must retain that same
identity; an ambient `spacetime login` is not a substitute.

Check the unit, executable, process start time and actual listening sockets. Query the explicit
node with the private configuration:

```bash
spacetime --config-path .lyracore/native-cli.toml list -s <http-uri>
spacetime --config-path .lyracore/native-cli.toml sql -s <http-uri> <shard> \
  "SELECT id, name, address FROM game_realm"
```

A loopback world address works only for a local client or an SSH tunnel. A remote client needs a
reachable advertised world address as well as reachable listeners. Stop if the configured Shard,
process or node differs from the intended target.

## Prepare the update

Preserve tracked changes. Fetch the reviewed commit and fast-forward the deployment checkout, or
use a separate clean checkout when the existing branch cannot fast-forward. Record the complete
commit SHA and installed Package revisions. Do not reset a dirty checkout.

Run the repository checks for the changed paths, followed by `./lyracore preflight`. For native
request, delivery or Host changes, build the Module and run the native protocol checks on disposable
local instances:

```bash
cargo test -p lyracore-protocol
cargo test -p lyracore-module --lib --features=debug_reducers
python3 native-host/prepare-host.py --checkout .lyracore/native-host/SpacetimeDB --build
cargo build --locked --release --target wasm32-unknown-unknown \
  -p lyracore-module --features=debug_reducers
python3 native-host/check.py \
  --host .lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone
./lyracore preflight
```

The native check also has the `admission-wait`, `recovery`, `slow-readers` and `accept-recovery`
checks. Name them after `--host` when the change affects those paths. They create their own
instances and must not be adapted to clear or seed a live Shard. Importer changes also require
`cargo test -p lyracore-importer`.

Preflight must complete the pinned toolchain checks, Module build, offline schema extraction and
RLS checks. A missing compiler or failed test blocks publication. Retain logs and exit statuses;
continue an existing build process rather than starting another copy.

## Prove recovery before publication

Take a consistent backup using the deployment's approved snapshot mechanism. For a filesystem
copy, stop the exact Native Host service, confirm that its process has exited, then copy its data
directory. Copy the matching executable, Module artifact, configuration and keys into a private
backup directory. Record artifact hashes and a bounded census of content and durable test state.
Restart the original service after the copy.

Restore into a different data directory and boot it on unused loopback ports. Verify its Shard,
content census and a fresh native world challenge. Stop that temporary instance afterwards.
Never open the same data directory from two Host processes. A backup that has not booted is not a
proven recovery path.

Restoring a backup can discard writes made after it. A production restore requires the Operator's
explicit decision about those writes. Publishing an older Module is not proof of schema rollback.

## Publish and restart

Deploy through the pinned wrapper with the exact loopback node and Shard:

```bash
./lyracore publish --server <http-uri> <shard>
```

The native wrapper uses `.lyracore/native-cli.toml`. Port 3000 also requires the live Native Host
recorded by this checkout. A managed deployment on another port supplies its explicit approved
URI. The wrapper accepts no clear-publish path. Keep the successful publication log and the
published Module hash.

Read the change's migration requirements before reopening clients. For the AOI index migration,
call `debug_backfill_cell_ids` with the same publisher and node. Call `debug_repair_after_publish`
with that identity; [After every publish](danger-zones.md#after-every-publish) says what it
repairs. Do not call `claim_operator` to replace an owner on an existing Realm, install another
GUID range, or rerun world imports as an update shortcut.

If Host code changed, install the tested Native Host executable and restart only its identified
service. Preserve the data directory, keys, publisher identity, Shard and listener configuration.
Host and Module reducer arguments and delivery layouts must match. A change to that interface needs
a maintenance interval: keep native client listeners closed until the matching Module is published.
Do not run old and new Host binaries against the same data directory.

## Move an existing Realm from classic-db to vmangos

Treat this as a world replacement during a maintenance interval. Publishing the Module preserves
Character state, but `lyracore import` replaces imported world content. Source spawn guids change.
A numeric spawn id can name a different creature or GameObject placement. Existing Package Spatial
Claims are stale and must be rebuilt against the new Import Revision, checking entry, map and
position. Package and Operator identifier ranges do not change.

Classic-only quest, item and credit ids can disappear. Before the update, compare the source census
with active Character quests, inventory and recorded credit on a restored copy. Decide how to
handle missing ids before reopening clients. This import supplies no automatic conversion for
those Character records. The unsupported behavior ledger lists the accepted quest and script gaps.

1. Follow the backup and restored-copy verification above. Record the old `game_import_meta`
   rows and Package revisions. Keep native client listeners closed during the replacement.
2. Move Characters out of Instances and let the existing Instances be removed through the
   [Instance lifecycle](instance-lifecycle.md). Confirm no old `game_instance` leases remain.
   Instances copy spawn rows and routes, so they must be recreated from the new import. A Module
   publish or spell-only Package apply does not rebuild them.
3. Publish the reviewed Module and call `debug_repair_after_publish` as described above.
4. Run `./lyracore import` with the intended profile and the Operator's client data. Complete all
   world families and `./lyracore import check` before applying Packages. Do not use
   `lyracore packages apply` as the source migration: it can reimport spells without replacing
   the world. Spell apply and its `--check` mode refuse world provenance that differs from the
   selected vmangos snapshot.
5. Rebuild the Package artifacts and Spatial Claims against the new Import Revision, then run
   `./lyracore packages apply --check` followed by `./lyracore packages apply`. Verify Package
   placement and behavior on the restored copy before repeating the procedure on the target.
6. Recreate Instances through fresh entry and verify Character reconnect, quests, inventory,
   credits and the imported content census. Reopen clients only after those checks pass.

## Verify the result

Record the new PID, executable hash, start time, restart count and listener ownership. HTTP health
alone is insufficient. Check a fresh native world challenge, authenticated logon, Character
selection, world entry and a subsequent request. Use dedicated test Accounts. For user-visible
changes, record Benilla and inspect both the packets and committed state.

Compare the content census and durable Character, money and item state with the pre-update
record. During recovery tests, verify that fresh logins work, original items remain unique, and
World Session and AOI rows for lost connections are removed. Include any relevant migration calls
and their outcomes in the report.

Capacity evidence must state client count, activity, duration, world content, AOI density, latency
percentiles, resource use and unexpected Transport Losses. A movement throughput result alone is
not a concurrent-Character capacity claim. Report load generation limits separately from Realm
limits, including the per-address logon admission limit.

Leave the intended service running. Remove only the temporary processes and test state created by
the verification. Keep the backup, logs, recordings and exact revision information.
