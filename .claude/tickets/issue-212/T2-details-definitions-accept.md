# T2 — Move quest details, definitions and accept through the seam

Parent: issue #212. Depends on T1. Runs in parallel with T3, T4, T5.
Model: Opus. Estimated size: ~180k tokens.

## Problem

Three routes render the same quest-details screen: clicking a quest in a giver's menu, the
client's cold-cache quest definition query, and using an item that starts a quest. The item route
lives in `handlers/item.rs` with its own `item_start_quest` / `item_quest_detail` store methods, so
the item and creature paths can render different screens for the same quest.

## Delivery

Move these branches out of `handle_quest` into `dispatch_quest_action` (added by T1):

- `CMSG_QUESTGIVER_QUERY_QUEST` → raw quest details for the giver.
- `CMSG_QUEST_QUERY` → the raw vanilla quest-definition reply.
- `CMSG_QUESTGIVER_ACCEPT_QUEST` → the durable accept request.

Add to `QuestActionStore`: `accept_quest` and `item_start_quest`. `quest_detail` already exists
from T1 — reuse it, do not add a second read.

Add `pub(crate) fn quest_details_screen<St: QuestActionStore + ?Sized>(store, giver, quest_id)
-> Result<Vec<Outbound>>` as the single chokepoint for the details screen, and route both
`CMSG_QUESTGIVER_QUERY_QUEST` and the item-started path through it.

Migrate the item-started quest out of `handlers/item.rs`:

- Delete `item_start_quest` and `item_quest_detail` from `ItemActionStore` and its `Coordinator`
  impl and its in-memory test adapter.
- In `dispatch_item_action`'s `CMSG_USE_ITEM` main-bag branch, ask the quest module first. Export a
  `pub(crate) fn item_started_quest<St: QuestActionStore + ?Sized>(store, player, slot)
  -> Result<Option<Vec<Outbound>>>` from the quest module: `Some(batch)` when the item starts a
  quest, `None` when it does not. `handlers/item.rs` falls through to the ordinary `use_item` path
  on `None`.
- The item route needs a `QuestActionStore`. Do **not** add a second store parameter and do **not**
  widen `ItemActionStore`. `WorldStore` is a supertrait of every action store, so simply widen the
  bound: `dispatch_item_action<St: ItemActionStore + QuestActionStore + ?Sized>`. The existing
  caller in `world/mod.rs` already satisfies it and needs no change.

## Acceptance criteria

Covers stories 3, 9, 10, 11, 18, 19, 20, 21, 22, 23, 40, 41, 42, 45.

1. A details request for a known quest returns the full raw details screen for the requesting
   giver guid.
2. A details request for an unknown quest returns `Handled` with an empty batch. No error, no
   session end.
3. A definition query for a known quest returns the existing raw vanilla encoding, unchanged.
   Do not re-derive the layout; call the existing codec function.
4. A definition query for an unknown quest returns `Handled` with an empty batch.
5. Accept requests the durable accept with account, player, giver and quest, in that order.
6. A refused accept is logged at debug, produces an empty batch, and is not session-fatal.
   A transport loss still propagates as `Err`.
7. Using an item that starts a quest returns the details screen with the **item instance guid** as
   the giver, and issues **no** `use_item` durable request, so the item is not consumed.
8. Using an item with no start quest issues the ordinary `use_item` durable request. The quest
   module does not consume the action.
9. An item-started quest with an unknown quest id returns an empty batch and still does not consume
   the item.
10. With no resolved player guid, the item-started query is not attempted.
11. The details screen produced by `CMSG_QUESTGIVER_QUERY_QUEST` and by the item route come from
    the same function.

## Tests

- Focused seam tests in `handlers/quest.rs`, one per branch above, through `dispatch_quest_action`
  and `item_started_quest` with `InMemoryQuestActions`.
- Update the item-seam tests in `handlers/item.rs` that covered quest-start routing
  (`use_item_with_start_quest_*`) to drive the new two-store `dispatch_item_action`. Keep proving
  that a quest-start item is not consumed.
- Delete the socket tests in `world/tests.rs` that now only re-prove screen or accept behavior:
  `quest_accept_dispatches_to_reducer_with_giver_and_quest`,
  `use_item_with_start_quest_opens_details_and_does_not_consume`,
  `use_item_without_start_quest_falls_through_to_the_ordinary_use_path`.
  Keep **one** encrypted socket test for `CMSG_QUEST_QUERY`, proving the raw definition body
  survives dispatch and the cipher.

## File ownership

Yours: the details/definitions/accept region of `handlers/quest.rs`, all of `handlers/item.rs`,
your tests in `world/tests.rs`. Append your trait methods, match arms and tests **after** T1's and
**before** wherever T3/T4/T5 land, so sequential integration merges cleanly.

## Non-goals

- Do not touch complete, reward, abandon, world entry, gossip or party sharing.
- Do not remove anything from `WorldStore`. T6 owns that.
- Do not change any codec function or raw encoding.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway` clean.
Rebased onto the integration branch that carries T1.
