# T1 — Prove and repair the durable choice-reward transaction

Parent: issue #255. **Tracer bullet. Blocks T2–T4. Run alone.**
Model: GPT-5.6 Sol. Estimated size: ~170k tokens.

## Problem

The isolated picker and socket routing tests pass, but no test invokes the durable turn-in with a
real two-choice collect quest and observes the resulting inventory and quest state. The defect can
still be in imported rows, objective consumption, the choice lookup, or shared item storage.

## Delivery

Create the highest practical reducer/integration seam for a quest-33-equivalent fixture. Seed two
ordered choice rows, both item templates, eight objective items, the collect objective, a live
player, a valid end giver, and completed quest state. Drive the same durable turn-in entry point the
gateway calls.

First make the test reproduce the missing reward. Record the exact proven fault before changing
production code. Repair the generic path only. If authentic import data is the fault, correct the
import and add a narrow parser/import regression; otherwise leave importer code untouched.

## Acceptance criteria

1. Choice 0 grants only the first displayed item and choice 1 grants only the second.
2. Counts match the quest rows and the eight objective items are consumed.
3. Guaranteed items, money, XP and reputation remain additive where present.
4. The quest is marked rewarded once; a duplicate turn-in grants nothing.
5. Invalid choice indices fail without consuming objectives or marking the quest rewarded.
6. A storage failure rolls back objective consumption, reward insertion and quest completion.
7. The fix contains no quest-id or item-id special case.

## File ownership

Own `module/src/quest.rs`, directly related module fixtures/tests, and `importer/src/main.rs` only if
the failing test proves an import defect. Do not edit gateway files.

## Definition of done

The reproduction is red before the fix, green after it, `cargo fmt` is clean, and the focused module
and importer tests pass.

