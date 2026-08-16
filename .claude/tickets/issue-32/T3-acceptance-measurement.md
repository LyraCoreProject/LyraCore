# T3 — Add reproducible batch-factor acceptance measurement

Parent: issue #32. **Depends on T1. Runs in parallel with T2.**
Model: mid. Estimated size: ~170k tokens.

## Problem

Issue #32 is done only when a 500-plus-mover run demonstrates the batch factor without action-latency
or client-visible regression. The runtime logs describe historical results, but operators lack one
reproducible output that pairs movement entry rate, reducer-call rate, batch-size distribution,
transaction duration and a representative action-latency signal over the same interval.

## Delivery

Extend the existing metrics/load-sample and approved synthetic-load tooling rather than creating a
second benchmark framework. Instrument movement batches at the gateway's explicit batch submission
boundary established by T1. Expose cumulative counters and a compact distribution sufficient to
calculate entry rate, call rate, batch factor, failures and chunk sizes. Keep hot-path accounting
lock-free or amortized and do not log per movement.

Add a reproducible, non-production acceptance command or script for at least 500 active movers. Its
summary must include environment/configuration, seated movers, duration, submitted entries, reducer
calls, achieved batch factor, failed entries/calls, transaction/tick duration signal, and action
latency signal. Document the manual 1.12.1 client visual check separately because automation cannot
claim it occurred.

Do not touch a live database. Tests may use deterministic samples and fake command output. A real load
run occurs only against an explicitly selected disposable realm.

## Acceptance criteria

1. Metrics distinguish movement entries from movement batch reducer calls.
2. Operators can derive the batch factor from two reported cumulative values over one interval.
3. Failed calls and their dropped entry counts are reported separately from accepted submissions.
4. Batch-size output shows whether the 128-entry cap is active without unbounded label cardinality.
5. An idle interval reports zero calls and no divide-by-zero or fabricated batch factor.
6. The acceptance tool refuses an accidental default/live target and requires an explicit disposable
   realm/load-driver selection.
7. A successful summary requires at least 500 active movers and identifies an insufficient run as not
   accepted.
8. The summary pairs batching results with tick/transaction duration and action latency from the same
   run; reduced transaction count alone cannot pass.
9. Documentation states the measurement tripwire and lever order, and leaves the real-client visual
   verdict as an explicit human field.

## Tests

Add focused counter/summary tests for empty, normal, capped, failed and insufficient-population runs.
If a shell script is extended, follow the repository's existing executable test-script pattern and
provide deterministic fixtures rather than launching 500 clients in CI.

## File ownership

Own movement batch metrics, load-driver/acceptance scripts and their documentation/tests. Do not edit
module reducers or the gateway batching component beyond consuming T1's observation seam.

## Definition of done

Relevant Rust formatting, clippy and tests are clean; script tests pass; no live service is touched.
