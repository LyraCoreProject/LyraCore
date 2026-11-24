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

`Standalone::insert_row` writes one fixture row from named fields, for example
`shard.insert_row("game_spell", &[("spell_id", &spell), ("name", &"Fixture")])`. It reads the
table's columns from the published Module's `describe` and caches them per database until the next
publish. An unnamed column takes its declared `#[default(..)]`, otherwise the zero of its type.
String values are quoted for you; other values are written as SQL literals. An unknown table or
column panics with the table's real columns. `insert_rows` writes several rows in one statement,
and `insert_row_database` writes to a named database. Durable tests in `module/tests` write rows
this way; an Architecture Test in `module/src/tripwires.rs` refuses a raw `INSERT INTO` there.

`Standalone::connect_reducers` opens one Operator WebSocket and installs its subscriptions before
returning. `ReducerConnection::call` waits for that connection's reducer result and returns only
its transaction's subscribed inserts, as positional JSON rows. `drain_inserts(table)` takes buffered
inserts across received transactions. `game_native_packet` rows arrive only decoded, as
`NativePacket`: `ReducerRows::packets()` for one call, `drain_packets()` across transactions.
These methods do not read pending socket messages. A subsequent reducer call acts as a barrier;
`wait_for_packets` reads until packets arrive or its deadline expires.

`WorldSessionObserver` answers which packets one World Session received. It routes `NativePacket`s
as the Native Host does: direct packets by connection, AOI packets by the AOI Grants it has seen,
which Revoke and Clear withdraw. The Host writes neither a Handoff marker nor any later packet from
this Shard to the client. The observer returns the marker as its last packet, so a test can see the
Handoff, and nothing after it. Feed it every packet from World Session entry onward. It judges
nothing; a test checks `close_after`, `not_before_micros` and `authentication_complete` itself. The Host's own router in `native-host/src/feed.rs` cannot be linked by tests, so the two
must change together.

This uses SpacetimeDB 2.7.1's `v1.json.spacetimedb` protocol. The CLI subscription process cannot
submit reducer calls on its existing connection. The SDK needs generated Module bindings; this
small synchronous client keeps the test's arguments and subscribed rows explicit without those
bindings. See [ADR 0005](../../docs/adr/0005-1-12-conformance.md) for the conformance boundary.
