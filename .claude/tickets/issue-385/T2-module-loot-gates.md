# T2: Module Loot Gates and reducer seam

Parent: issue #385. **Runs after T1. Blocks T3 through T5.**
Model: mid. Estimated size: ~180k tokens.

## Problem

Corpse item-taking, money-taking, skinning, and opening already apply liveness, range, and
loot-row reservation Gates, but not Loot Tag eligibility. Empty eligibility can fall back to the
actor as if a corpse were solo loot. The Gateway also has no durable request for opening a corpse,
so it can show a window before the Module authorizes it.

## Delivery

Use T1's canonical eligibility API as the only corpse authorization rule.

1. Add one narrow Module Gate for corpse access. It must distinguish creature corpses from
   GameObjects, require a `game_corpse_loot_eligible` row for every tagged corpse action, log a
   denial with actor and corpse guids, and return a stable gameplay Refusal containing both guids.
2. Apply the Gate before opening a creature corpse, taking an item, taking money, and skinning.
   GameObject and chest behavior stays byte-identical.
3. Add an actor-facing Gateway reducer for opening a creature corpse. Keep protocol concepts out
   of the Module. The successful result only authorizes the read that follows.
4. Make money sharing use the eligible set. One eligible character is solo, not a group split. More
   than one eligible character follows the existing party split rules.
5. Keep loot-row reservation as an additional restriction after tag eligibility. Being in the tag
   party does not override round-robin, master-loot, roll, or quest-need reservation.
6. Make quest-only item clones draw recipients from corpse eligibility, so later joiners never gain
   a reserved quest item through a fresh current-group lookup.
7. Make skinning require both tag eligibility and the existing looted-or-empty condition. A foreign
   actor cannot bypass the Loot Tag by skinning.

Use the same stable Refusal classifier for all four actions. Do not depend on exact transport error
text in the Module.

## Acceptance criteria

1. An eligible solo actor may open, take an item, take money, and skin when the remaining action
   rules allow it.
2. An entitled party member may perform each action even when another party member tagged.
3. A foreign actor receives the same class of Refusal for open, item, money, and skin. Every log
   record names the actor and corpse.
4. A corpse with no eligibility rows is usable by nobody.
5. A refused item or money action does not mutate loot rows, inventory, money, roll state, or open
   state.
6. A one-recipient corpse does not emit or apply a party money share.
7. Existing GameObject loot tests remain unchanged and green.
8. The new reducer binding surface is clear enough for T4 to generate or add the Gateway binding
   without guessing names or arguments.

## Tests

Add reducer-level tests for all four Gates, including no-eligibility, foreign actor, solo actor,
party member, and GameObject non-regression. Extend existing take-allowed, money, and skinning
tests instead of building a parallel harness.

## File ownership

- `module/src/items/ops.rs`
- the action and money regions of `module/src/loot/mod.rs`
- `module/src/professions.rs`
- `module/src/gw.rs`
- the matching actor wrapper file under `module/src/actors/`
- focused Module tests for these actions

Do not edit Gateway bindings, codecs, handlers, subscriptions, or dynamic-flag projection. T3 and
T4 own them.

## Definition of done

Touched Rust files are individually formatted. Focused tests and the Module wasm check are clean.
Push the result to the shared feature branch and report the reducer name, arguments, and stable
Refusal classifier to T3 and T4.
