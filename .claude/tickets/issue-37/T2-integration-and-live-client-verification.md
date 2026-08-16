# T2 — Integrate verification and perform the live-client acceptance check

Parent: issue #37. **Runs after T1.**
Model: gpt-5.6-terra. Estimated size: ~40k tokens.

## Problem

Automated protocol tests cannot prove that the 1.12.1 client accepts the supersede packet or
renders the relogged spellbook correctly.

## Delivery

Review T1 against the issue and the shared rank-stacking rule, run the relevant gateway tests, and
perform the live-client acceptance check if a realm and 5875 client are available. Use a
non-mana/passive trainer chain whose old rank is already known. Confirm after purchase that the old
rank is absent from the spellbook UI, then relog and confirm it remains absent. Record the exact
result on issue #37. If the client is unavailable, report that precise blocker and leave the issue
open.

## Acceptance criteria

- The focused trainer socket test and relevant gateway test suite pass.
- No production behavior is changed unless a concrete defect is found.
- A successful live client check is recorded on issue #37 before closure.
- Without the live client, the issue stays open with the blocker recorded.
