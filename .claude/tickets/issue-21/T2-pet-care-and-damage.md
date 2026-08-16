# T2 — Feed Hunter pets and make happiness observable in damage

Parent: issue #21. Depends on T1. **Parallel with T3 and T4.**
Model: strongest. Estimated size: ~180k tokens.

## Problem

A tamed pet has durable identity but no care loop. Feeding must cross imported item data, generic
spell effects, inventory atomicity, scheduled state and shared swing math while remaining completely
absent from summoned demons.

## Delivery

- Add item food type to the authoritative item template/import path and starter food fixture. Keep
  existing item field ordering and gateway views/builders synchronized.
- Map raw spell effect 101 to a generic feed-pet effect. Route the item target through the existing
  cast/use-item seam rather than naming Feed Pet.
- Validate owner, live Hunter pet, alive/out-of-combat state, family diet mask and food benefit by
  item level. Consume exactly one item only after all validation succeeds.
- Implement bounded happiness and three states. Use CMaNGOS-compatible bands, death loss of one full
  band, food benefits, and deterministic elapsed-time decay; combat may use the selected documented
  faster decay behavior. Run care updates from a module scheduled row, not a gateway timer.
- Apply 75%, 100%, 125% to Hunter-pet base physical damage at the shared combat-math seam. Summoned
  pets and spell damage remain unchanged.

## Acceptance criteria

1. Suitable food consumes one item and raises happiness, clamped at maximum.
2. Wrong diet, food too low-level, no pet, summoned pet, dead pet, foreign pet or in-combat pet
   consumes nothing and changes nothing.
3. Catch-up over elapsed time is deterministic, bounded and equivalent to regular schedule firings.
4. Pet death subtracts one happiness band once.
5. Identical swings produce the 75/100/125 Hunter-pet damage ratio for unhappy/content/happy.
6. An Imp's damage and state do not change with Hunter happiness logic.
7. Existing ordinary consumable behavior is unchanged.

## Tests

- Inventory/spell-effect tests assert consume-on-success only and all gates.
- Pure elapsed-time examples plus scheduled reducer tests cover each happiness boundary, catch-up,
  saturation and death.
- Shared combat tests assert resulting deterministic damage across all three bands and a summoned
  pet control case.
- Importer tests pin FoodType and raw effect 101.

## File ownership

Own a new pet-care module, item food metadata/import, feed mapping and the smallest possible shared
damage/death hooks. Do not implement XP/level progression or gateway pet queries.

## Definition of done

`cargo fmt`, importer/module tests and focused clippy are clean. Commit only this slice.

