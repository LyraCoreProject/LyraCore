# T4 — Move quest-log slots and world-entry replay through the seam

Parent: issue #212. Depends on T1. Runs in parallel with T2, T3, T5.
Model: Sonnet. Estimated size: ~160k tokens.

## Problem

The client's quest log addresses quests by slot, not by quest id. Two places read that slot
ordering: `handle_quest` resolves an abandon slot to a quest id, and `handlers/char.rs`'s
`send_quest_log` renders the descriptor block at world entry and on the quest-log relay. They both
call `WorldStore::player_quest_log` independently, so the meaning of a slot can drift between the
window the player sees and the quest an abandon click removes.

## Delivery

Move this branch out of `handle_quest` into `dispatch_quest_action` (added by T1):

- `CMSG_QUESTLOG_REMOVE_QUEST` → resolve the log slot, then request the durable abandon.

Add `player_quest_log` and `abandon_quest` to `QuestActionStore`.

Give the quest module the quest-log read model and make both callers use it:

- `pub(crate) fn quest_log_slots<St: QuestActionStore + ?Sized>(store, player_guid)
  -> Result<Vec<codec::update_mask::QuestLogSlot>>` — the one place slots are read.
- `pub(crate) fn quest_log_update<St: QuestActionStore + ?Sized>(store, player_guid)
  -> Result<Vec<Outbound>>` — the raw descriptor update, or an empty batch when there is nothing
  to show.

Rewrite `handlers/char.rs`'s `send_quest_log` to call `quest_log_update` and send the batch. Keep
the existing `crate::config::quest_log_fields_enabled()` gate and the empty-log no-op exactly where
they are today; do not change when the update is sent, only where it is built. `send_quest_log`
serves both login and the quest-log relay, so both keep working unchanged.

`handlers/char.rs` holds a `WorldStore`; pass it as the `QuestActionStore` the same way the other
callers do.

## Acceptance criteria

Covers stories 1, 3, 24, 25, 26, 35, 36, 37, 40, 41, 42, 44.

1. An abandon request resolves the client slot against the current quest-log ordering and requests
   the durable abandon for the quest id in that slot, with account and player.
2. An abandon request for a slot that is not in the quest log issues **no** durable request and
   returns an empty batch. Prove that a stale slot cannot abandon an arbitrary quest.
3. An abandon request against an empty quest log issues no durable request.
4. A refused abandon returns an empty batch, logs at debug, and is not session-fatal.
   A transport loss propagates as `Err`.
5. `quest_log_update` returns an empty batch for an empty quest log, so world entry gains no
   message.
6. `quest_log_update` returns the current raw descriptor update, unchanged, for a non-empty log.
   Call the existing mask and codec functions; do not re-derive the layout.
7. Abandon-slot resolution and the world-entry descriptor block read slots through the same
   function.

## Tests

- Focused seam tests in `handlers/quest.rs`, one per acceptance criterion above, through
  `dispatch_quest_action` and `quest_log_update` with `InMemoryQuestActions`.
- Delete the socket tests in `world/tests.rs` that these supersede:
  `quest_abandon_resolves_log_slot_to_quest_id`, `quest_abandon_unknown_slot_is_a_noop`.
- Keep the existing login-sequence socket coverage that asserts the quest-log descriptor update
  appears after the CREATE packet. That test proves login integration, which the seam cannot.

## File ownership

Yours: the quest-log region of `handlers/quest.rs`, all of `handlers/char.rs`, your tests in
`world/tests.rs`. Append your trait methods, match arm and tests after T3's region so sequential
integration merges cleanly.

## Non-goals

- Do not touch details, definitions, accept, complete, reward, gossip, party sharing or
  item-started quests.
- Do not remove anything from `WorldStore`. T6 owns that.
- Do not change when the quest-log update is sent, or the config gate that guards it.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway` clean.
Rebased onto the integration branch that carries T1.
