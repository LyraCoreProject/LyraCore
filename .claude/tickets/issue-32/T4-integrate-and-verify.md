# T4 — Reconcile and verify the complete capacity lever

Parent: issue #32. **Runs last after T1–T3 are integrated.**
Model: strongest. Estimated size: ~150k tokens.

## Problem

The gateway seam, module behavior tests and measurement slice can each pass alone while their union
duplicates counters, measures attempted rather than submitted calls, leaves stale comments, or misses
the unchanged socket anti-lag contract. This ticket validates the system as one capacity lever.

## Delivery

Read the integrated movement path from socket coalescing through gateway draining, reducer application
and peer motion publication. Reconcile naming and counters so each concept has one owner. Remove any
test-only interface or duplicate helper orphaned by parallel integration. Verify all issue stories and
record a concise acceptance runbook using the actual commands and metric names that landed.

Run the complete gateway and module test suites plus script tests. Do not fabricate a 500-mover result
or a real-client verdict. If the environment cannot run them, leave exact commands and clearly marked
evidence slots. The automated definition of done is a deterministic acceptance harness and all
behavioral contracts green, not a claim about an unperformed external run.

## Acceptance criteria

1. Only steady coalesced heartbeats enter the shared per-shard batch.
2. State changes and non-movement actions still flush pending heartbeats immediately and in order.
3. Batches are empty-suppressed, 40 ms cadence, bounded at 128 entries and independently submitted.
4. One bad module entry cannot roll back valid neighbors.
5. Per-player stale ordering is proven across immediate and batched paths.
6. Batch and single paths share movement rules and relay accepted raw movement equivalently.
7. Metrics count the same submissions the production adapter makes and report failures unambiguously.
8. The acceptance tool cannot silently target production and cannot pass below 500 active movers.
9. Existing socket anti-lag tests remain unchanged and green.
10. Comments and docs describe the code as it exists, without issue-number comments or obsolete
    historical claims.

## File ownership

Own union cleanup, acceptance documentation and cross-cutting test fixes after T2 and T3 are merged.
Do not broaden scope into adaptive cadence, OCC, sharding or deployment.

## Definition of done

`cargo fmt`, gateway and module clippy, gateway and module tests, and acceptance-script tests are clean.
Report any external load/client checks still awaiting an authorized environment.
