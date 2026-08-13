# Vendor-interaction seam — shared context for all tickets

Source: GitHub issue #205 (do NOT modify or close it). This is the second vertical slice of the
WorldStore deepening work; it must reuse the item-action seam pattern from #204/#209, not invent
a competing one. Durable game rules (prices, stock, copper, ownership, range checks, repair
costs) stay in the SpacetimeDB module — this work changes gateway architecture only and must
preserve player-visible behavior byte-for-byte.

## The pattern to copy: the item-action seam

`gateway/src/world/handlers/item.rs` is the template. It contains, in one file:

- `pub(crate) trait ItemActionStore: Send + Sync` — six narrow methods, seam-owned vocabulary.
- `ItemActionPlayer { account_id: u64, self_guid: Option<u64> }` (`Copy`) as input context.
- `enum ItemActionOutcome { Handled { outbound: Vec<Outbound> }, PassThrough(ClientOpcodeMessage) }`
  — the seam RETURNS packets instead of writing to `tx`; that is what makes it unit-testable.
- `pub(crate) fn dispatch_item_action<St: ItemActionStore + ?Sized>(store, player, msg) -> Result<ItemActionOutcome>`.
- Error classification: `enum ItemActionErrorClass { GameplayRefusal, Fatal }`;
  `classify_item_action_error` walks `error.chain()` for `"reducer transport disconnected"` →
  `Fatal` (propagate `Err`, session dies); everything else → `GameplayRefusal` (client feedback).
- Production adapter `impl ItemActionStore for crate::stdb::Coordinator` lives IN the seam file,
  forwarding to inherent Coordinator methods — NOT in `gateway/src/stdb/world_store.rs`.
- Broad trait requires the seam: `pub trait WorldStore: ItemActionStore + Send + Sync`
  (`gateway/src/world/store.rs`).
- Seam-local `#[cfg(test)]` in-memory adapter (`InMemoryItemActions`): `Mutex<Vec<…>>` request
  logs + per-method `Option<String>` error injection, `#[derive(Default)]`.
- Dispatcher call site: `gateway/src/world/mod.rs` `fn dispatch` — `Handled` loops
  `send(tx, message)?` then returns; `PassThrough` rebinds `msg` and falls through.
- Socket-test store also implements the seam trait: `impl ItemActionStore for InMemoryStore`
  in `gateway/src/world/tests.rs`.

## Current vendor code (what migrates)

- `gateway/src/world/handlers/vendor.rs` — `handle_vendor` (old `Result<Option<ClientOpcodeMessage>>`
  convention, writes to `tx`): CMSG_LIST_INVENTORY (npc_refuses_interaction silent gate →
  vendor_items → build_list_inventory_raw → Outbound::Raw), CMSG_BUY_ITEM (Err → SMSG_BUY_FAILED
  via build_buy_failed), CMSG_SELL_ITEM (guid→slot via `player_items(...).find(...)`; Ok →
  push_buyback_view; Err/unknown guid → debug-log only), CMSG_BUYBACK_ITEM
  (`slot.as_int().saturating_sub(69) as u8`; Ok → push_buyback_view; Err → debug-log),
  CMSG_REPAIR_ITEM (guid 0 = repair-all → slot `u8::MAX`; else guid→slot; Err → system chat via
  build_gm_system_message as SMSG_MESSAGECHAT).
- Gossip branch in `gateway/src/world/handlers/query.rs`: is-vendor = `!vendor_items(npc)?.is_empty()`
  in CMSG_GOSSIP_HELLO; `codec::gossip_menu_options(imported, is_vendor, is_innkeeper)` in
  `gateway/src/codec/npc.rs` synthesizes the "I'd like to browse your goods." VENDOR option
  (row_id u32::MAX) unless an imported row already carries the action; CMSG_GOSSIP_SELECT_OPTION
  VENDOR arm duplicates the vendor_items + build_list_inventory_raw + Outbound::Raw triple.
- World entry: `gateway/src/world/handlers/char.rs` `enter_world` calls
  `push_buyback_view(tx, store, character_guid, true)` (skip_if_empty = true).
- Shared builder: `push_buyback_view` in `gateway/src/world/handlers/mod.rs` — reads
  `buyback_ring(self_guid)`; for i in 0..12: fabricated guid `0x4090_0000_0000_0000 | i`,
  `ItemInstanceView { slot: 69 + i }` → build_item_create_object → SMSG_UPDATE_OBJECT; one
  UpdateMaskValues with `set_u64(486 + (69+i)*2, fab_guid)`, `set_u32(1226 + i, price)`,
  `set_u32(1238 + i, 0)` → build_values_update_raw → Outbound::Raw. Writes straight to tx —
  must become returned outbound inside the seam.

## WorldStore operations involved (declared in gateway/src/world/store.rs)

vendor-only candidates for eventual removal from the broad trait: `vendor_items`, `buy_item`,
`sell_item`, `buyback_item`, `repair_item`, `buyback_ring`.
Shared with other families (keep on WorldStore, seam may re-declare its own narrow copies):
`player_items`, `npc_refuses_interaction`, `item_slot_by_guid` (combat enchant path uses it;
note vendor sell/repair currently hand-roll guid→slot instead of using it).
Adapters: production in `gateway/src/stdb/world_store.rs` + inherent methods in
`gateway/src/stdb/reducers.rs` / `gateway/src/stdb/reads/`; shared test store `InMemoryStore`
in `gateway/src/world/tests.rs` (NOTE: its `buyback_ring` is hardwired empty today — no test
exercises ring rendering).

## Known behavior quirks to preserve

- Refusing NPC → total silence on direct vendor open (vanilla client behavior).
- Empty stock still sends the (empty) vendor window on direct open.
- Buy failure → SMSG_BUY_FAILED; sell/buyback failure → silent (debug log) non-fatal;
  repair failure → private system chat message. Per-arm policies differ TODAY; preserve each
  arm's client-visible behavior while unifying fatal/transport classification underneath
  ("reducer transport disconnected" anywhere in the chain must kill the session, like item.rs).
- Unknown item-instance guid on sell/repair → harmless no-op (no reducer call).
- Wire buyback slot ↔ zero-based ring slot: base 69 (BuybackSlot::Slot1) … 81 (Slot13); the
  render direction (69 + i) and the parse direction (−69) should share one constant in the seam.
- Successful purchase relies on item/purse subscriptions for row-change delivery — do not
  duplicate delivery.

## Existing tests (gateway/src/world/tests.rs unless noted)

Socket tests: buy_item_err_sends_smsg_buy_failed; buyback_maps_the_wire_slot_enum_to_zero_based_ring_slots;
sell_item_resolves_the_instance_guid_to_its_slot_before_dispatch; sell_item_for_an_unknown_guid_does_not_dispatch;
repair_item_resolves_the_instance_guid_to_its_slot_before_dispatch;
repair_item_guid_zero_is_repair_all_and_dispatches_the_whole_body_slot;
repair_all_err_is_relayed_as_a_system_chat_line_not_silently_swallowed;
gossip_select_on_a_vendor_opens_the_inventory_window (+2 imported-menu variants).
Codec tests in `gateway/src/codec/tests.rs`. Coverage gaps: no bare CMSG_LIST_INVENTORY test,
no vendor refusal-gate test, no buyback-ring rendering test, no login-replay test.

## Commands

Fast loop: `cargo test -p lyracore-gateway`. Full check before finishing a ticket:
`cargo test --workspace` equivalents per docs/danger-zones.md:
`cargo test -p lyracore-importer && cargo test -p lyracore-module --lib --features=debug_reducers && cargo test -p lyracore-gateway && cargo test -p lyracore-shared && cargo build`.

## Repo conventions

- Comments: one or two lines stating a constraint the code cannot show. Never tracker numbers.
  Legacy essay comments exist — do not imitate their density.
- The seam file should follow item.rs's layout: adapter, types, dispatch fn, classification,
  `#[cfg(test)]` module with seam-level unit tests.
