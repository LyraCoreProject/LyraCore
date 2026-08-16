# T2 — Prove independent reducer behavior and stale ordering

Parent: issue #32. **Depends on T1. Runs in parallel with T3.**
Model: strongest. Estimated size: ~170k tokens.

## Problem

The batch reducer visibly loops over entries and calls the shared movement core, but the current tests
do not prove the transaction-level promise: bad entries are isolated, valid neighbors commit, stale
heartbeats cannot overtake newer movement, and accepted batch motion is published with the same opcode
and raw body as the single-update path.

## Delivery

Use the project's highest existing module/reducer test seam to execute `gw_movement_batch` against
real module state. Add only the minimum harness affordances needed to submit a batch and read durable
world entity and peer motion state. Prefer existing module test helpers over a parallel fake database.

Cover valid entries surrounding failures, monotonic per-player ordering, taxi suppression, and the
equivalence of batched versus single movement application. Do not change movement rules merely to make
tests convenient. If the current reducer cannot be invoked through the available harness, establish
one narrow core seam shared by the reducer and tests without weakening operator authorization at the
public reducer boundary.

## Acceptance criteria

1. Two valid actors around a missing actor both commit their movement in one batch.
2. Invalid coordinates or malformed movement for one actor do not reject a later valid actor.
3. A non-zero timestamp equal to or older than the actor's stored timestamp changes neither durable
   position nor published peer motion.
4. A newer immediate state change followed by an older batched heartbeat cannot be overtaken.
5. Taxi-suppressed movement is ignored while later valid batch entries still commit.
6. Equivalent accepted input through single and batch paths produces equivalent durable movement
   state and peer relay opcode/raw movement body.
7. Accepted staged motion is published before the batch call completes.
8. Operator authorization remains at both public movement reducer boundaries.

## Tests

Favor reducer integration tests that observe tables and relay rows after completion. Pure helper tests
are acceptable only for setup or exact comparison logic; source scanning is not sufficient evidence
for the behavioral requirements above.

## File ownership

Own module gateway reducer tests and the smallest module test harness/core seam needed by them. Do not
edit gateway coordinator code, load sampling, benchmark scripts or docs.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-module`, and `cargo test -p lyracore-module` are clean.

