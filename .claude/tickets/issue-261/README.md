# Issue #261 — item eligibility masks, broken into tickets

Source: `gh issue view 261 --comments` — "fix(item): eligible quest and vendor items render red".

## State of the prior slices

As of 2026-08-17, the issue is open, has no comments, and no linked pull requests.  The working
tree is at `03d5874`; it contains the earlier item-template requirement plumbing (the work-item-213
columns) but no allowable class/race masks.  `module/src/items/tables.rs` has
`required_skill`, `required_skill_rank`, and reputation columns.  The importer reads and emits
those fields, `gateway/src/stdb/views.rs` copies them into `ItemTemplateView`, and
`gateway/src/codec/item.rs` places them in the vanilla query response.  The same response leaves
the packet library's `allowed_class` and `allowed_race` defaults empty.

`module/src/items/inventory.rs::apply_item_move` already gates equipment slot, level, and the
legacy derived armor/weapon proficiency rule.  It does **not** yet use the explicit template
class/race masks, the `game_player_skill` row required by `required_skill`, or the item reputation
columns.  The existing `reputation_rank` helper is the durable rank vocabulary to reuse.

## Pattern to establish

The tracer adds a single source-of-truth pair of unsigned masks to the item template, normalizes
the Classic unrestricted source sentinel at import time, and carries the values unchanged to the
item-query packet:

```rust
// module ItemTemplate: END-appended additive columns, defaults are all playable bits.
pub allowed_class: u32,
pub allowed_race: u32,

// importer: source unrestricted sentinel -> ALL_PLAYABLE_*; restrictive u32 stays verbatim.
// gateway: ItemTemplate -> ItemTemplateView -> SMSG_ITEM_QUERY_SINGLE_RESPONSE fields.
allowed_class: ClassMask::new(t.allowed_class),
allowed_race: RaceMask::new(t.allowed_race),
```

Do not derive masks from the gateway's local enum list.  Preserve imported high/unknown bits.
The only importer transformation is the documented unrestricted sentinel normalization.  Existing
durable rows must receive a non-empty all-playable default through the additive migration.

The equip ticket then consumes those very columns from `ItemTemplate`.  A mask of zero in a
durable row is malformed/restrictive and fails closed; it must never be interpreted as unrestricted
outside the importer boundary.

## Shared rules

- No quest, vendor stock, reward-choice, inventory relay, or client UI special case.  The cached
  item query is shared by every client screen.
- Keep class/race masks as `u32` end-to-end.  Do not widen real restrictions or discard unknown
  bits.
- Treat source-format ambiguity as a fact to establish from the dump/parser tests before changing
  a column index.  Add importer fixture coverage for both source unrestricted and restrictive
  values, asserting emitted durable rows.
- Item equip gating remains Module authority.  Client tint is only guidance.
- A refusal from an equip request must leave item placement unchanged.  Do not add a new gateway
  path for it.
- Use domain terms from `CONTEXT.md`: Module, Gateway, Durable Request, Gate, Refusal, Character.
- Every ticket finishes with focused tests plus the relevant crate checks.  The integration ticket
  runs `cargo fmt`, `cargo clippy -p lyracore-gateway`, and the appropriate module/importer tests.

## Execution order

```
T1  (serial tracer)
 ├── T2 (Module equip gates) ─┐
 └── T3 (Gateway/world path) ─┤
                             └── T4 (serial integration)
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|--------|-------|-------------|----------------|
| T1 | Add and import the canonical eligibility masks | gpt-5.6-sol | ~190k | `module/src/items/tables.rs`, `importer/src/main.rs`, item seed/fixture literals, `gateway/src/codec/item.rs`, `gateway/src/stdb/views.rs`, generated bindings, focused tests |
| T2 | Enforce item eligibility in Module equip operations | gpt-5.6-terra | ~160k | `module/src/items/inventory.rs`, `module/src/items/rules.rs`, `module/src/reputation.rs` only if a small reusable query is needed, module tests |
| T3 | Cover the real query route and eligibility fixtures | gpt-5.6-terra | ~150k | gateway item-query/session tests and test-only fixture helpers; no production files owned by T1/T2 |
| T4 | Reconcile and verify the completed item path | gpt-5.6-sol | ~150k | combined integration result, stale tests/docs only |
