# Read-only native Realm diagnosis

Use the [production contract](production-contract.md) to establish the exact target. Follow
[the native operations guide](../../../../docs/native-operations.md) for configuration and health
requirements. Diagnosis permits process metadata, service status, sockets, bounded logs, schema
reads and narrow `SELECT` queries. A publish, reducer, import, SQL write, restart or crash test
requires authorization for that action.

Record the UTC window, checkout commit, Native Host PID and start time, service manager, executable,
data directory, log source, explicit HTTP URI and Shard. Do not assume contributor ports describe
a managed deployment. `lyracore dev status` describes the local fixture, not another service.

Check these layers in order:

1. Pinned tools and the approved configuration.
2. Native Host process, restart count, data path and actual HTTP listener.
3. The configured Shard on the explicit node, using its private publisher configuration.
4. Native logon/world listener ownership and the advertised `game_realm.address`.
5. Bounded Host and Module logs around the failure, including native transport, admission,
   reducer and packet-delivery failures.
6. Native connection counts, resource use and the workload's latency and Transport Losses.

A fresh world challenge proves the listener reaches the Module. Authenticated world entry requires
a dedicated test Account and authorization to use it, because login can replace an existing World
Session. Keep Account verifiers, publisher tokens, Session keys and full environment dumps out of
reports.

Use narrow SQL only for a named unresolved question. Examples are the Realm address, one
Character's committed position, or counts of native connections. Use the private configuration and
explicit node for each query. SpacetimeDB SQL has no `ORDER BY`, `IN` or subqueries.

Distinguish a missing process, wrong Shard, failed bind, unreachable advertised address, authentication
Refusal and delivery failure. Do not treat a downstream symptom as the cause when an earlier layer
failed. A missing load measurement is a warning; it does not prove the Realm is unhealthy.

Finish with the skill's result contract, bounded evidence for each finding and the smallest next
step. Diagnosis itself does not perform the proposed repair.
