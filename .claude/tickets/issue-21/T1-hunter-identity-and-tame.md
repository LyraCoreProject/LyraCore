# T1 — Establish durable Hunter identity through a completed tame

Parent: issue #21. **Tracer bullet. Runs alone and blocks T2–T5.**
Model: strongest. Estimated size: ~190k tokens.

## Problem

The existing pet is only a summoned live creature. Hunter work needs a durable identity and one
atomic transition that proves imported spell data can convert an eligible wild beast into that
identity plus the existing live pet representation.

## Delivery

- Add a private durable Hunter-pet record with a stable pet id and one-current-pet owner index. It
  must carry owner, creature entry, family, name, rename eligibility, level, pet XP, loyalty level
  and progress, happiness, and enough timestamps for later deterministic care updates.
- Add an explicit live pet kind so code can distinguish Hunter pets from summoned pets without
  guessing from entry or class. Preserve existing summoned-pet behavior and old rows through safe
  defaults.
- Map raw spell effect 55 to a generic tame effect in importer and module taxonomy. Route it through
  normal channel completion and effect dispatch.
- On completion, validate Hunter class, alive owned caster, alive non-player target, target not
  already owned/charmed, target level no higher than owner, no active pet, creature family present,
  and `pet_talent_type >= 0`. Revalidate everything at completion.
- Atomically create default identity, convert the target into the owner's live Hunter pet, preserve
  entry/family/level/display and remaining-health proportion, and remove it from the wild spawn
  lifecycle without creating a duplicate.
- Supply small Hunter/Tame Beast/boar fixtures using the existing fixture style. Do not hardcode a
  production spell id in runtime behavior.

## Acceptance criteria

1. A valid completed tame yields exactly one durable Hunter identity and one live Hunter pet owned
   by the caster; the old wild entity is gone.
2. Family, entry, display, level and remaining-health proportion survive conversion.
3. Non-Hunters, targets above owner level, non-tameable or missing family rows, owned targets,
   players, dead targets and owners with an active pet are rejected without partial writes.
4. An interrupted channel creates no identity and leaves the wild creature intact.
5. Revalidation catches a target or owner that becomes invalid during the channel.
6. Summon Imp still creates a summoned pet, never a Hunter identity.
7. Existing pet cycle tests and gateway pet-bar tests remain green; a successful tame naturally
   reaches the existing create relay and bar path.

## Tests

- Importer unit tests pin raw effect 55 and its target/parameter mapping.
- Spell-completion scenario tests cover success, interruption and every gate above using observable
  entity/identity results.
- One regression test proves Summon Pet remains a separate pet kind.

## Non-goals

No feeding, decay, damage multiplier, pet XP, loyalty ticking, name-query packet, stable, Call Pet,
Revive Pet, or logout restoration.

## Definition of done

`cargo fmt`, importer tests, module tests and module clippy are clean. Commit the tracer on its own
branch for integration before parallel tickets start.

