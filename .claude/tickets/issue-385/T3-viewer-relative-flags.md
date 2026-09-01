# T3: Viewer-relative tapped and lootable flags

Parent: issue #385. **Runs after T2. Parallel with T4. Blocks T5.**
Model: mid. Estimated size: ~160k tokens.

## Problem

`WorldEntity.dynamic_flags` is global, but tapped-by-player and corpse lootability are
viewer-relative. Sending the stored flags unchanged makes every player see the same name colour,
sparkle, and loot cursor. The Gateway does not subscribe to the private tag or corpse-eligibility
tables needed to project the Module result.

## Delivery

Project entity flags per viewer at the existing create and values-update seams.

1. Add the existing `game_creature_quest_tap`, `game_creature_quest_tap_member`, and
   `game_corpse_loot_eligible` tables to the Gateway's base subscription. Include any additive T1
   tag metadata table if T1 proved it necessary.
2. Add a focused pure helper that takes the viewer, entity row, and cached durable rows and returns
   viewer-relative dynamic flags.
3. For a live tagged creature, retain stored `TAPPED`. Add `TAPPED_BY_PLAYER` only when the viewer
   remains entitled under T1's tag-membership rule. Later joiners never receive it and leavers lose
   it.
4. For a dead creature, retain `LOOTABLE` only when the viewer has a corpse-eligibility row. Mask it
   for every other viewer, including the empty-eligible case.
5. Apply the projection to both entity CREATE construction and dynamic-flags VALUES relays. Do not
   add a new Module relay.
6. If a group-roster change can alter a live viewer's `TAPPED_BY_PLAYER` without changing the
   entity row, reuse the Gateway's existing group event path to refresh affected visible entities.
   Keep this narrowly scoped and test the transition.

Do not mutate cached Module rows or store viewer-relative bits back into SpacetimeDB.

## Acceptance criteria

1. The tagging character and entitled snapshot party members see `TAPPED_BY_PLAYER` on a live
   tagged creature.
2. A stranger sees `TAPPED` without `TAPPED_BY_PLAYER`.
3. A later joiner sees the stranger flags. A leaver stops seeing `TAPPED_BY_PLAYER`.
4. An eligible viewer sees `LOOTABLE` on a corpse; a foreign viewer and an empty-eligible viewer do
   not.
5. Both CREATE and VALUES update packets use the same projection helper.
6. An untagged entity's flags are byte-identical to the stored flags.
7. The base subscription includes every table read by the helper.
8. No new relay, schema change, or durable ownership rule appears in Gateway code.

## Tests

Add pure projection tests for owner, party member, stranger, later joiner, leaver, eligible corpse,
foreign corpse, empty eligibility, and untagged entity. Keep one dispatch-level test proving the
helper is used for both CREATE and update construction if the existing harness supports it.

## File ownership

- `gateway/src/stdb/connection.rs`
- `gateway/src/stdb/subscriptions.rs`
- `gateway/src/stdb/world_view.rs` only if the projection belongs there
- focused Gateway tests for entity create/update and subscription coverage

Do not edit loot codecs, loot handlers, reducer adapters, or Module files. T4 owns the Gateway loot
request path.

## Definition of done

Touched Rust files are individually formatted. `cargo test -p lyracore-gateway` and
`cargo clippy -p lyracore-gateway` are clean. Push to the dedicated T3 branch and report the commit
for T5 to integrate.
