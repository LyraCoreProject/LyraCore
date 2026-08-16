# T2 — Authorize Duel combat and stop at 1 health

Parent: issue #13. **Parallel with T3 after T1.**
Model: mid tier. Estimated size: ~175k tokens.

## Problem

The tracer can make a Duel active, but normal faction gates still reject same-faction attacks and
normal lethal damage kills a player. The exception must be exact: two active participants may harm
one another, nobody else gains permission, and the loser never reaches the death path.

## Delivery

Add one shared `may harm` decision that existing melee and hostile spell target gates consume. An
active Duel makes only the two participants mutually hostile for combat. Pending/countdown state does
nothing. Extend the common damage application seam so opponent Duel damage that would be lethal
applies only to 1 health, completes the Duel as won, tears down both directions of combat, and emits
the terminal result needed for `SMSG_DUEL_COMPLETE` and `SMSG_DUEL_WINNER`. Complete active cancel as
a surrender/fled result. Preserve unrelated combat where the existing engagement model requires it.

## Acceptance criteria

- Same-faction melee and hostile spells are refused before countdown expiry and after completion.
- During active state, each participant can harm the other and no third same-faction player becomes
  attackable or a hostile area-target candidate.
- Existing ordinary hostile-faction behavior is unchanged.
- Representative weapon and spell damage use the same no-kill decision.
- A would-be lethal opponent hit leaves the loser at exactly 1 health, never marks them dead, never
  creates a corpse, and never invokes death-side rewards or durability behavior.
- Completion removes the Duel, stops both participants' Duel combat, clears combat flags when they
  have no other reason to remain in combat, and emits exactly one won result.
- Active cancel produces a fled winner result and the same teardown.
- Gateway relays `SMSG_DUEL_COMPLETE` and `SMSG_DUEL_WINNER` with correct reason and names.
- Tests cover pending/active/finished faction behavior, third-party isolation, weapon and spell
  lethal floors, no death state, surrender, idempotence, and exact terminal packet shape.
- Existing combat and spell tests pass. The work is committed on the ticket branch.
