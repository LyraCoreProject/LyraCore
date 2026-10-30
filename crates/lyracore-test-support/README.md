# Private integration fixtures

`Standalone` owns an isolated SpacetimeDB process, CLI configuration and database storage. Tests
can publish the selected Core Module, call reducers, read durable state and retain failure logs.
`module_bytes()` builds the tested Module once per test process.
The test Module enables `debug_reducers`, `package_test_fixture` and `conformance_test_fixture`.
The Package fixture registers an inert Package named `test_fixture` for Core's ownership and
teardown tests; normal publishes omit it.

Core tests use this crate's workspace. Package test runners set `LYRACORE_TEST_CORE` to the explicit
Core checkout used for both builds and evidence. Package-specific setup and assertions belong in
the Package repository.

`Standalone::connect_reducers` opens one Operator WebSocket and installs its subscriptions before
returning. `ReducerConnection::call` waits for that connection's reducer result and returns only
its transaction's subscribed inserts, as positional JSON rows. `drain_inserts(table)` takes buffered
inserts across received transactions; `drain_packets()` takes only `game_native_packet` inserts.
These methods do not read pending socket messages. A subsequent reducer call acts as a barrier;
`wait_for_packets` reads until packets arrive or its deadline expires.

This uses SpacetimeDB 2.7.1's `v1.json.spacetimedb` protocol. The CLI subscription process cannot
submit reducer calls on its existing connection. The SDK needs generated Module bindings; this
small synchronous client keeps the test's arguments and subscribed rows explicit without those
bindings. See [ADR 0005](../../docs/adr/0005-1-12-conformance.md) for the conformance boundary.
