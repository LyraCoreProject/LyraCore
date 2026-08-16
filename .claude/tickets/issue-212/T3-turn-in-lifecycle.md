# T3 — Move the quest turn-in lifecycle through the seam

Parent: issue #212. Depends on T1. Runs in parallel with T2, T4, T5.
Model: Opus. Estimated size: ~150k tokens.

## Problem

The turn-in round trip is the one quest path where ordering is player-visible. Opening a turn-in
picks between the offer-reward and request-items screens from a durable evaluation. Choosing a
reward must call the durable turn-in before the gateway claims completion, or a refused turn-in
shows a false "Quest Complete" popup. Today that ordering lives inline in `handle_quest`, above the
wide `WorldStore`, and can only be checked through an encrypted socket.

## Delivery

Move these branches out of `handle_quest` into `dispatch_quest_action` (added by T1):

- `CMSG_QUESTGIVER_COMPLETE_QUEST` → offer-reward or request-items.
- `CMSG_QUESTGIVER_CHOOSE_REWARD` → durable turn-in, then the quest-complete popup.

Add `turn_in_quest` to `QuestActionStore`. `quest_giver_evals` and `quest_detail` already exist
from T1 — reuse them.

Keep the completion decision on the giver's current evaluation: a quest is complete when the
evaluation for that giver carries the end role, the requested quest id, and the complete flag.
The gateway never guesses objective completion.

## Acceptance criteria

Covers stories 3, 27, 28, 29, 30, 31, 40, 41, 42, 44.

1. A completion request for a quest the current evaluation reports complete returns exactly one
   `SMSG_QUESTGIVER_OFFER_REWARD` for that giver.
2. A completion request for an incomplete quest returns exactly one
   `SMSG_QUESTGIVER_REQUEST_ITEMS`.
3. A completion request for an unknown quest returns `Handled` with an empty batch.
4. A completion request issues no durable request. It only picks a screen.
5. Reward choice requests the durable turn-in with account, player, giver, quest and reward slot
   **before** any outbound message is produced.
6. A successful reward choice returns exactly one `SMSG_QUESTGIVER_QUEST_COMPLETE`, built from the
   current quest details, so displayed XP, money and items match the definition.
7. A refused reward choice returns an empty batch, logs at debug, and is not session-fatal. Assert
   explicitly that no quest-complete message is produced.
8. A transport loss on turn-in propagates as `Err`.
9. A successful turn-in whose quest details cannot be read returns an empty batch. The durable
   turn-in still happened and is not retried.

## Tests

- Focused seam tests in `handlers/quest.rs`, one per branch and per acceptance criterion above,
  driven through `dispatch_quest_action` with `InMemoryQuestActions`. The adapter records turn-in
  requests and can be configured to refuse.
- Delete the socket tests in `world/tests.rs` that these supersede:
  `quest_choose_reward_turns_in_and_replies_complete`,
  `quest_complete_picks_offer_reward_vs_request_items_by_completion`.
  Keep **one** encrypted socket test for `CMSG_QUESTGIVER_CHOOSE_REWARD`, stating that it proves
  dispatch and the typed completion reply over the cipher, not screen selection.

## File ownership

Yours: the turn-in region of `handlers/quest.rs` and your tests in `world/tests.rs`. Nothing else.
Append your trait method, match arms and tests after T2's region so sequential integration merges
cleanly.

## Non-goals

- Do not touch details, definitions, accept, abandon, world entry, gossip, party sharing or
  item-started quests.
- Do not remove anything from `WorldStore`. T6 owns that.
- Do not change any codec function or raw encoding.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway` clean.
Rebased onto the integration branch that carries T1.
