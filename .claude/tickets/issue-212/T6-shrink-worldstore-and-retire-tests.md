# T6 — Shrink `WorldStore` and retire the shallow quest tests

Parent: issue #212. **Runs last, after T1–T5 are integrated on one branch.**
Model: Opus. Estimated size: ~150k tokens.

## Problem

The point of the slice is a smaller interface and smaller test setup, not file movement. After
T1–T5, the quest operations on `WorldStore` have no callers left, `handle_quest` has no branches
left, and `world/tests.rs` still carries an `InMemoryStore` that implements every quest method. The
prior slice's post-mortem warned that independent slices can each pass while their union leaves a
dead handler, stale documentation, or incomplete error behavior. This ticket closes that.

## Delivery

**1. Verify the union first.** Before deleting anything, read the integrated `handlers/quest.rs`
end to end and confirm: one trait, one player context, one outcome type, one dispatch, one error
classifier. If T2–T5 introduced parallel helpers that duplicate each other, or an inconsistent
refusal-versus-fatal split, fix that before proceeding. Report anything you merge or reconcile.

**2. Retire `handle_quest`.** Every branch is migrated. Delete the function, its `mod` export in
`handlers/mod.rs`, and its call site in `world/mod.rs`. Do not leave a pass-through-only handler
behind; the prior slice's post-mortem calls that a secondary seam that weakens the boundary.

**3. Shrink `WorldStore`.** Delete each quest method that now has no caller outside the quest
module: `quest_giver_evals`, `quest_detail`, `accept_quest`, `turn_in_quest`, `abandon_quest`,
`push_quest`, `player_quest_log`, `quest_status`. Delete their `InMemoryStore` implementations in
`world/tests.rs` and any fixtures left write-only by the deletions.

`npc_refuses_interaction` **stays** on `WorldStore` — vendor, trainer and gossip still call it.
Confirm each deletion with a search across the whole crate before removing it, and state the
before/after operation count in the PR description.

**4. Retire the shallow quest tests.** Remove the remaining `world/tests.rs` quest tests that only
re-prove decisions the seam tests now cover. Keep a small representative encrypted set, each with a
one-line statement of the socket-level contract it proves. Target roughly five, matching the
convention commit `58d5aff` set for the item slice:

- questgiver hello, for dispatch plus a raw details body over the cipher
- quest query, for the raw definition body
- choose reward, for the typed completion reply
- the login sequence, for the quest-log descriptor block after the CREATE packet
- gossip with a quest-gated option, for the assembled gossip message

**5. Refresh documentation.** Update the module docs on `handlers/quest.rs`, `handlers/mod.rs` and
any `docs/` page that describes quest dispatch or the `WorldStore` surface. Comments must match the
code that ships. No issue numbers in comments.

## Acceptance criteria

Covers stories 4, 5, 43, 47, 48, 49, 50.

1. `handle_quest` no longer exists anywhere.
2. Every quest operation listed above is gone from `WorldStore` and from `InMemoryStore`, and the
   crate builds.
3. `npc_refuses_interaction` is still on `WorldStore` and still used by its non-quest callers.
4. The encrypted quest suite is the small representative set above, each test naming its contract.
5. No test asserts the same quest decision at both the seam and the socket.
6. No schema, reducer or protocol change appears in the diff.
7. Module docs describe the shipped architecture.

## Non-goals

- Do not add quest behavior. This ticket only deletes, verifies and documents.
- Do not touch the loot-window, vendor, melee or cast paths. Those are issues #213, #205, #206
  and #210.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway` clean.
The PR description states the `WorldStore` operation count before and after, and lists anything
reconciled in step 1.
