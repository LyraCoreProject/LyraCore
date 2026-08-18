# T4 — Integrate and verify the quest-33 acceptance path

Parent: issue #255. **Runs last after T1–T3 are integrated.**
Model: GPT-5.6 Sol. Estimated size: ~160k tokens.

## Problem

Reducer correctness, refusal presentation and asynchronous item visibility can each pass alone while
their union still grants the wrong displayed choice, sends success too early, or leaves a retry
unsafe. This ticket verifies the assembled behavior and removes accidental duplication.

## Delivery

Read the integrated paths end to end: importer ordering, offer encoding, choose-reward dispatch,
durable transaction, item relay, and completion/refusal presentation. Reconcile duplicate helpers or
inconsistent ordering. Add the narrowest highest-seam acceptance coverage still missing, using quest
33's authentic objective and two reward definitions when repository fixtures allow it.

## Acceptance criteria

1. Both quest-33 choices map from displayed position to the exact durable item and quantity.
2. Success produces immediate inventory visibility and only then truthful completion presentation.
3. Full inventory preserves the active quest/objectives and reopens the reward offer for retry.
4. Invalid and duplicate packets grant nothing.
5. No schema change or quest-specific production branch appears in the final diff.
6. Ticket docs and nearby code comments describe the shipped behavior accurately.

## File ownership

Runs serially with cross-cutting ownership after T1–T3. Keep reconciliation minimal and preserve
unrelated work in the branch.

## Definition of done

Run `cargo fmt`, affected-crate clippy, module tests, gateway tests, and importer tests. Report any
acceptance check that still requires a live stock 1.12 client.

