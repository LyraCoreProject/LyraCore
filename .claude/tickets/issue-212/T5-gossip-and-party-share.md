# T5 — Move gossip quest contributions and party sharing through the seam

Parent: issue #212. Depends on T1. Runs in parallel with T2, T3, T4.
Model: Sonnet. Estimated size: ~140k tokens.

## Problem

A gossip-flagged questgiver delivers its quests through the gossip menu, not through
`CMSG_QUESTGIVER_HELLO`. `handlers/query.rs` therefore reads quest evaluations itself to build the
quest section, and reads quest taken/rewarded state itself to filter quest-gated gossip options. Two
separate quest readers mean gossip icons and direct quest interaction can disagree. Party quest
sharing is a third small quest operation still reading the wide `WorldStore`.

## Delivery

Move this branch out of `handle_quest` into `dispatch_quest_action` (added by T1):

- `CMSG_PUSHQUESTTOPARTY` → the durable share request.

Add `push_quest` and `quest_status` to `QuestActionStore`. `quest_giver_evals` already exists from
T1 — reuse it.

Give the quest module the two reads `handlers/query.rs` performs today and make gossip call them:

- `pub(crate) fn gossip_quest_items<St: QuestActionStore + ?Sized>(store, npc, self_guid)
  -> Result<Vec<..>>` — the quest section of a combined gossip menu, built from the same evaluation
  and the same `codec::quest_menu_items` the giver menu uses.
- `pub(crate) fn quest_gate_state<St: QuestActionStore + ?Sized>(store, player_guid, quest_id)
  -> (bool, bool)` — the taken/rewarded pair that condition-filters quest-gated gossip options.

Rewrite the quest reads in `handlers/query.rs` to call these. The gossip handler keeps owning
everything else: option filtering, title text, the message build and the CharSelect fallback that
shows no quests. Do not move gossip decisions into the quest module.

## Acceptance criteria

Covers stories 1, 3, 32, 33, 34, 38, 40, 41, 42, 44.

1. Party sharing requests the durable share with account, player and quest, and produces **no**
   outbound message on success. Per-member group-event relays remain the only feedback path.
2. A refused share returns an empty batch, logs at debug, and is not session-fatal.
   A transport loss propagates as `Err`.
3. The gossip quest section for an NPC is built from the same evaluation and the same menu-item
   derivation as the giver menu. Prove it with a test whose fixture produces identical quest items
   from `gossip_quest_items` and from the giver-menu path.
4. A plain gossip NPC with no quests contributes an empty quest section, as today.
5. A quest-gated gossip option stays hidden until the gating quest is taken, and appears once it
   is. Existing behavior, now read through the quest module.
6. `handlers/query.rs` no longer calls `quest_giver_evals` or `quest_status` directly.

## Tests

- Focused seam tests in `handlers/quest.rs` for party sharing and for `gossip_quest_items` /
  `quest_gate_state`, with `InMemoryQuestActions`.
- Delete the socket tests in `world/tests.rs` that these supersede:
  `push_quest_to_party_dispatches_the_quest_id`,
  `push_quest_to_party_rejection_is_logged_and_ignored_not_session_fatal`.
- Keep the gossip socket tests `gossip_hello_hides_a_quest_gated_option_until_the_quest_is_taken`
  and `gossip_hello_shows_a_quest_gated_option_once_the_quest_is_taken`. They prove the assembled gossip message, which is the gossip handler's job,
  not the seam's.

## File ownership

Yours: the gossip/share region of `handlers/quest.rs`, all of `handlers/query.rs`, your tests in
`world/tests.rs`. Append your trait methods, match arm and tests after T4's region so sequential
integration merges cleanly.

## Non-goals

- Do not touch details, definitions, accept, complete, reward, abandon, world entry or
  item-started quests.
- Do not remove anything from `WorldStore`. T6 owns that.
- Do not move gossip option filtering, title text or message assembly into the quest module.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway` clean.
Rebased onto the integration branch that carries T1.
