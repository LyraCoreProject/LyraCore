# T5 — Integrate the Hunter pet loop and prove the union

Parent: issue #21. **Runs last after T1–T4 are integrated.**
Model: strongest. Estimated size: ~170k tokens.

## Problem

The slices cross schema, spell import, inventory, combat and wire translation. Each can pass alone
while the union has duplicated pet types, stale bindings, inconsistent defaults or no complete
Tame → bar → feed → damage path.

## Delivery

1. Read the integrated pet modules end to end. Reconcile duplicate types, constants, schedules,
   ownership checks, names and happiness/loyalty defaults. Keep one durable identity and one live pet
   kind discriminator.
2. Regenerate bindings using the repository workflow and remove handwritten/generated drift.
3. Add one headless vertical acceptance scenario: a Hunter channels Tame Beast on the starter boar,
   receives/derives the active bar state, sets Passive and proves no auto-aggro, issues Attack/Follow/
   Stay, feeds valid food, observes happiness rise, kills for pet XP, and observes the physical
   damage band. Add an Imp control proving shared bar/commands with no Hunter care row.
4. Run schema/tripwire checks and update architecture/schema or acceptance documentation where the
   durable/live boundary or manual client verification needs recording.
5. Produce a short live-client checklist but do not touch production. Mark any client-only verdict
   honestly rather than simulating it.

## Acceptance criteria

1. Every issue story inside the agreed scope is implemented or explicitly documented as requiring
   the live-client eyeball.
2. One test crosses the complete Hunter loop at the highest available headless seams.
3. Warlock summon, spell casting, commands and bar do not regress and create no Hunter care state.
4. No gateway timer or durable pet gameplay rule appears.
5. No stale generated binding, duplicate pet identity, dead helper or unused fixture remains.
6. Formatting, clippy and all relevant workspace tests pass.

## Definition of done

Run `cargo fmt --check`, workspace clippy/tests appropriate to the changed crates, importer tests and
repository tripwires. Commit integration fixes and report the exact checks plus any live-only gap.
