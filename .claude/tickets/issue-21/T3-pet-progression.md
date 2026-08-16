# T3 — Advance Hunter pet level and loyalty from shared kill credit

Parent: issue #21. Depends on T1. **Parallel with T2 and T4.**
Model: mid. Estimated size: ~170k tokens.

## Problem

The durable identity has progression fields but no source of advancement. Pet XP must join the
existing kill-credit transaction, respect owner caps and recalculate the live creature without
forking player XP or creature combat.

## Delivery

- Add a focused pet-progression module with a pet XP curve, multi-threshold leveling and derived
  live-stat refresh. A Hunter pet cannot level beyond its owner's current level.
- Hook the existing authoritative creature-kill award after kill credit is resolved. An eligible
  active Hunter pet receives pet XP from the kill using pet-relative kill XP; summoned pets do not.
- Update durable level/XP and the live entity atomically. Clear or retain capped XP according to the
  documented CMaNGOS behavior and prevent overflow.
- Implement loyalty level/progress transitions using the durable identity. Qualifying kills grant
  the Vanilla-style loyalty bonus. Expose a narrow care-update function for T2 happiness ticks to
  call: happy/content gain, unhappy loses, with bounded levels and explicit threshold behavior.
- Keep abandonment/rebellion side effects out of this slice; clamp at the lowest supported loyalty
  level rather than inventing lifecycle behavior.

## Acceptance criteria

1. A qualifying credited kill advances an active Hunter pet and no summoned pet.
2. A pet levels through one or several thresholds without passing its owner.
3. A capped pet cannot overflow XP or level.
4. A pet level change refreshes level-dependent live health and damage while preserving a sensible
   health proportion.
5. Kill loyalty bonuses and happiness-driven loyalty deltas cross thresholds deterministically and
   saturate at supported bounds.
6. Group/owner kill credit remains the authority; pet-only kills continue crediting the owner and
   do not duplicate the pet award.
7. Player XP events and level-up packets are unchanged.

## Tests

- Pure progression tests cover exact threshold, multi-level, cap and overflow cases.
- Kill-credit scenario tests cover owner kill, pet kill credited to owner, group/no-credit edge and
  summoned-pet exclusion.
- Loyalty tests cover all happiness states and both boundaries through observable identity state.

## File ownership

Own a new pet-progression module and the smallest kill-award/live-stat hooks. T2 owns schedules,
happiness and damage; T4 owns gateway wire fields.

## Definition of done

`cargo fmt`, module tests and focused clippy are clean. Commit only this slice.

