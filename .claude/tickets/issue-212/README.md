# Issue #212 — quest-interaction slice, broken into tickets

Source: `gh issue view 212` — "Deepen gateway quest interactions behind one test seam".

## State of the prior slices

Issue #212 says it "follows item actions in #204, vendor interactions in #205, melee attacks in
#206, and cast lifecycle in #210". As of 2026-08-14, **#204, #205 and #206 have shipped** — items
(PRs #207/#208/#209/#215/#216), vendors (PR #219) and melee (PR #220). Only the cast lifecycle
(#210) is still open.

Three sibling seams therefore exist to copy: `handlers/item.rs`, `handlers/vendor.rs` and
`handlers/melee.rs`. They agree on the shape below. Follow it and do not invent a fourth style.

Important consequence: `WorldStore` is now declared as

```rust
pub trait WorldStore:
    ItemActionStore + MeleeActionStore + QuestActionStore + VendorActionStore + Send + Sync
```

so any caller holding a `WorldStore` already satisfies `QuestActionStore`. A handler that needs
quest reads just widens its own generic bound. It never needs a second store parameter.

## The pattern to copy (`handlers/item.rs`)

```rust
pub(crate) trait QuestActionStore: Send + Sync { /* narrow, quest-only durable ops + reads */ }
impl QuestActionStore for crate::stdb::Coordinator { /* thin forwarding */ }

pub(crate) struct QuestActionPlayer { account_id: u64, self_guid: Option<u64> }

pub(crate) enum QuestActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

pub(crate) fn dispatch_quest_action<St: QuestActionStore + ?Sized>(
    store: &St, player: QuestActionPlayer, msg: ClientOpcodeMessage,
) -> Result<QuestActionOutcome>
```

Plus, in the same file, `#[cfg(test)] mod tests` with an `InMemoryQuestActions` adapter that
records durable requests and returns canned reads. One focused test per dispatch branch
(commit `58d5aff` set that convention).

Error classification is already written in `item.rs`: a cause containing
`"reducer transport disconnected"` is `Fatal` and propagates as `Err`; everything else is a
`GameplayRefusal` — logged at debug, never session-fatal.

## Execution order

```
T1  (serial, blocks everything)
 ├── T2 ─┐
 ├── T3 ─┤  parallel worktrees
 ├── T4 ─┤
 └── T5 ─┘
        └── T6 (serial, runs after T2..T5 are integrated)
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|--------|-------|-------------|----------------|
| T1 | Establish the quest-action seam with status and the giver menu — **DONE**, integrated on `t3code/quest-action-seam` | Opus | ~200k | `handlers/quest.rs`, `handlers/mod.rs`, `handlers/loot.rs`, `world/mod.rs`, `world/tests.rs` |
| T2 | Move quest details, definitions and accept through the seam | Opus | ~180k | `handlers/quest.rs` (details/accept region), `handlers/item.rs`, `world/tests.rs` |
| T3 | Move the quest turn-in lifecycle through the seam | Opus | ~150k | `handlers/quest.rs` (turn-in region), `world/tests.rs` |
| T4 | Move quest-log slots and world-entry replay through the seam | Sonnet | ~160k | `handlers/quest.rs` (log region), `handlers/char.rs`, `world/tests.rs` |
| T5 | Move gossip quest contributions and party sharing through the seam | Sonnet | ~140k | `handlers/quest.rs` (gossip/share region), `handlers/query.rs`, `world/tests.rs` |
| T6 | Shrink `WorldStore` and retire the shallow quest tests | Opus | ~150k | `world/store.rs`, `world/tests.rs`, `handlers/quest.rs`, docs |

## Shared rules for every ticket

- No schema, reducer or protocol changes. Player-visible behavior is byte-identical.
- Durable rules stay in the SpacetimeDB module. The gateway owns protocol and screen selection.
- Every ticket lands demoable and green: `cargo fmt`, `cargo clippy -p lyracore-gateway`,
  `cargo test -p lyracore-gateway`.
- Reducer refusal → client feedback or silent drop, never session death.
  Transport loss → propagate `Err`.
- Match surrounding naming and idiom. Do not copy the legacy essay-comment density. No issue
  numbers in comments.
- T2..T5 all append to `handlers/quest.rs`. Append in ticket order inside each region
  (trait block, `Coordinator` impl, `dispatch_quest_action` match, test adapter, tests) so the
  sequential integration merge stays mechanical.
