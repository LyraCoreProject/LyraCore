# T1 — Make the per-shard batch boundary deterministic

Parent: issue #32. **Tracer bullet. Runs alone and blocks T2–T4.**
Model: strongest. Estimated size: ~170k tokens.

## Problem

The gateway's real batching behavior is embedded in coordinator construction and a forever-running
Tokio task. The important rules exist, but tests cannot drive one drain explicitly or observe reducer
submissions without timing sleeps and a live coordinator. Empty windows, the 128-entry bound, FIFO
entry order and failure accounting therefore lack a stable behavioral seam.

## Delivery

Extract the smallest internal batching component that accepts complete `GwMove` values and performs
one explicit drain into bounded calls. Keep coordinator ownership per world shard and keep the 40 ms
task as a thin scheduler around that component. Give the drain a narrow submission callback/trait so a
test can observe calls and inject a submission failure without constructing SDK connections.

The production adapter must continue choosing a call pipe per chunk, invoking `gw_movement_batch`
fire-and-forget, logging the exact dropped count on failure, and discarding the failed chunk. Do not
change the public `WorldStore` API, cadence, chunk size, retry policy, or socket coalescer.

## Acceptance criteria

1. Draining an empty per-shard buffer performs zero submissions.
2. One to 128 queued entries produce one call containing the entries in queue order.
3. 129 entries produce calls of 128 and 1; a larger exact multiple has no empty trailing call.
4. Entries queued after one drain remain for the next drain and are never duplicated.
5. A failed chunk does not prevent later chunks from being submitted and exposes the failed chunk's
   entry count to the production logging boundary.
6. The coordinator still owns one independent buffer per connected world shard.
7. The scheduler still uses a 40 ms delayed missed-tick policy and performs no work for an empty tick.
8. The non-blocking heartbeat path still reports queueing completion without waiting for reducer
   completion.
9. Existing socket-level anti-lag and movement ordering tests pass unchanged.

## Tests

Add focused deterministic tests beside the extracted component. Use plain `GwMove` fixtures and a
recording submitter, not sleeps, sockets or a live SpacetimeDB. Retain existing encrypted socket tests
as the higher seam proving coalescer wiring.

## File ownership

Own the gateway coordinator connection/batching implementation and its local tests. Do not edit module
reducers, load sampling, benchmark scripts or docs.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, and `cargo test -p lyracore-gateway` are clean.
