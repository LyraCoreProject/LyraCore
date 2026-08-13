# 01 — Vendor-action seam + tracer bullet: direct vendor inventory open

**What to build:** A player who right-clicks a vendor NPC sees the vendor window with the
vendor's stock — served through a new deep vendor-interaction module, not the old handler.
This ticket lands the seam itself (trait, player context, outcome enum, error classification,
production adapter, dispatcher wiring) with CMSG_LIST_INVENTORY as the one representative
action migrated end-to-end, copying the item-action seam pattern exactly.

The seam: `vendor request + player context → client-visible outcome`. Define
`VendorActionStore` (narrow methods the family needs: stock read, interaction-refusal test,
plus whatever the tracer action uses), `VendorActionPlayer`, `VendorActionOutcome
{ Handled { outbound }, PassThrough }`, `dispatch_vendor_action`, and
`classify_vendor_action_error` (gameplay refusal vs fatal transport loss — "reducer transport
disconnected" in the chain ends the session). Production adapter for the Coordinator lives in
the seam file; `WorldStore` gains the seam as a supertrait; the session dispatcher routes
CMSG_LIST_INVENTORY through the seam while the remaining vendor opcodes keep flowing through
the legacy handler untouched.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Direct vendor open on an NPC with stock returns the same raw vendor-window bytes as before.
- [ ] An NPC that refuses interaction produces total silence (no reply of any kind).
- [ ] Empty vendor stock still produces the (empty) vendor window — the client never waits forever.
- [ ] A "reducer transport disconnected" failure from any seam store call is session-fatal.
- [ ] Seam-level unit tests (no socket) cover the three behaviors above via a seam-local
      in-memory adapter with error injection, following item.rs's test layout.
- [ ] The shared socket-test `InMemoryStore` implements the new seam trait; existing gateway
      tests stay green; other vendor opcodes are behaviorally untouched.
