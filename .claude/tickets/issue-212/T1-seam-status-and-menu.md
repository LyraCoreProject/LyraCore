# T1 — Establish the quest-action seam with questgiver status and the giver menu

Parent: issue #212. **Tracer bullet. Blocks T2–T6. Run alone.**
Model: Opus. Estimated size: ~200k tokens.

## Problem

Quest interaction knowledge is spread across the gateway. `handlers/quest.rs` reads the wide
`WorldStore`, sends its own sockets, and shares the giver-menu decision with `handlers/loot.rs`
through a free function parked in `handlers/mod.rs`. A creature giver and a questgiver GameObject
reach the same window by two different routes, so they can drift. Tests must build a full
`InMemoryStore` and an encrypted socket to check which screen a giver opens.

## Delivery

Create the quest-action seam and move the first two entry points through it.

```
quest request + player context → durable quest request + ordered client-visible outcome
```

Add to `gateway/src/world/handlers/quest.rs`, copying `handlers/item.rs` exactly:

- `pub(crate) trait QuestActionStore: Send + Sync` with only what this ticket needs:
  `quest_giver_evals`, `quest_detail`, `npc_refuses_interaction`.
- `impl QuestActionStore for crate::stdb::Coordinator` — thin forwarding only.
- `pub(crate) struct QuestActionPlayer { account_id: u64, self_guid: Option<u64> }`.
- `pub(crate) enum QuestActionOutcome { Handled { outbound: Vec<Outbound> }, PassThrough(..) }`.
- `pub(crate) fn dispatch_quest_action<St: QuestActionStore + ?Sized>(..)`.
- The error classifier (`GameplayRefusal` vs `Fatal` on `"reducer transport disconnected"`),
  written the same way `item.rs` writes it.
- `pub(crate) fn quest_giver_menu<St: QuestActionStore + ?Sized>(store, giver, self_guid)
  -> Result<Vec<Outbound>>` — the single chokepoint that picks the screen. Exported so the
  GameObject path can call it without going through an opcode.

Migrate these branches out of `handle_quest`:

- `CMSG_QUESTGIVER_STATUS_QUERY` → `SMSG_QUESTGIVER_STATUS`.
- `CMSG_QUESTGIVER_HELLO` → the menu, with the Unfriendly-or-below silent refusal preserved.

Move `send_questgiver_menu` out of `handlers/mod.rs` into the quest module as
`quest_giver_menu`, returning `Vec<Outbound>` instead of writing to the `SessionTx`. Change
`handlers/loot.rs`'s `go_type::QUESTGIVER` branch of `CMSG_GAMEOBJ_USE` to call it and send the
returned batch, so the creature and GameObject givers share one implementation.

Wire `dispatch_quest_action` into `gateway/src/world/mod.rs` immediately before the existing
`handle_quest` call, in the same shape the `dispatch_item_action` block already uses. `handle_quest`
stays in place for the opcodes T2–T5 have not moved yet, and keeps using `WorldStore`.

## Acceptance criteria

Covers stories 1, 2, 3, 5, 6, 7, 8, 12, 13, 14, 15, 16, 17, 39, 40, 41, 42, 44, 46.

1. `dispatch_quest_action` returns `PassThrough` for every opcode it does not own, including the
   two opcodes above when the player is not in world.
2. A status query evaluates the giver against the player and returns exactly one
   `SMSG_QUESTGIVER_STATUS` for that giver guid.
3. A creature giver whose faction refuses interaction produces `Handled` with an empty outbound
   batch and no durable request.
4. A giver with several menu-worthy quests returns one `SMSG_QUESTGIVER_QUEST_LIST`.
5. A giver with a single new quest returns the raw `Outbound::Raw` quest-details screen.
6. A giver with a single complete turn-in returns `SMSG_QUESTGIVER_OFFER_REWARD`.
7. A giver with a single incomplete turn-in returns `SMSG_QUESTGIVER_REQUEST_ITEMS`.
8. A giver with no menu-worthy quests returns the same empty-list behavior as today. Do not invent
   a quest.
9. The menu reads `quest_giver_evals` **once** and reuses that evaluation for screen selection, so
   state cannot change between two reads inside one request.
10. A questgiver GameObject and a creature giver in the same state produce the identical outbound
    batch, proven by a test that drives both.
11. `send_questgiver_menu` no longer exists in `handlers/mod.rs`.

## Tests

- Focused seam tests in `handlers/quest.rs` under `#[cfg(test)] mod tests`, one per dispatch
  branch and one per menu shape, driven through `dispatch_quest_action` / `quest_giver_menu` with
  an `InMemoryQuestActions` adapter. The adapter supplies evals and details and records nothing
  durable yet. No socket, no `SessionTx`, no `InMemoryStore`.
- Keep **one** encrypted socket test in `world/tests.rs` for `CMSG_QUESTGIVER_HELLO`, stating the
  socket-level contract it proves (dispatch reaches the quest module and the raw details body
  survives the cipher). Delete the socket tests that now only re-prove screen selection:
  `quest_hello_replies_with_the_quest_list`,
  `quest_hello_with_one_menu_quest_opens_its_screen_directly_by_state`,
  `quest_hello_with_two_menu_quests_shows_the_list`.

## Non-goals

- Do not touch details, definitions, accept, abandon, complete, reward, sharing, gossip,
  world entry or item-started quests. Later tickets own those.
- Do not remove anything from the `WorldStore` trait. T6 owns that.
- Do not change codec functions or raw encodings.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway` and `cargo test -p lyracore-gateway` are clean.
Branch rebased onto latest `main`.
