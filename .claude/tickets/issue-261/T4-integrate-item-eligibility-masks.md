# T4 — Reconcile and verify the completed item path

Parent: issue #261. **Blocks on T2 and T3; runs alone.**
Model: gpt-5.6-sol. Estimated size: ~150k tokens.

## Problem

The two parallel vertical slices share T1's item eligibility definition.  Their union must remain
one coherent import → Module → Gateway → client path, without duplicate helpers or stale tests that
only check packet defaults.

## Delivery

- Integrate T2 and T3 on top of T1, resolving overlapping fixture changes in favor of one canonical
  item definition and one eligibility predicate per layer.
- Run the targeted importer, Module, Gateway codec, and encrypted world-session checks, then the
  relevant workspace checks.  Diagnose and fix only regressions within issue #261.
- Verify defaults, source normalization, exact packet masks, Module gates, quest/vendor reuse, and
  negative cases together.  Remove tests or compatibility code orphaned by the slices.
- Inspect the final diff for accidental schema/data destructive behavior and document a concise
  manual stock-client verification for the Brotherhood of Thieves/Godric Rothgar Warrior scenario.

## Acceptance criteria

- All issue #261 acceptance paths are represented by focused, non-duplicative tests.
- The effective schema migration is safe for preexisting rows before reimport.
- Formatter, relevant clippy checks, and targeted tests are green.
- The final handoff identifies the manual stock-client check and any intentionally deferred content
  coverage.
