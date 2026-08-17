# Issue #255 — choice-reward quest turn-in, broken into tickets

Source: `gh issue view 255` — "selected choice reward is not granted on turn-in".

## Execution order

```text
T1 (serial tracer)
 ├── T2 (gateway refusal and reward re-offer) ─┐
 └── T3 (live inventory relay ordering) ──────┤ parallel worktrees
                                               └── T4 (serial integration)
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|--------|-------|-------------|----------------|
| T1 | Prove the durable choice-reward transaction — **DONE** (`69ecddd`) | GPT-5.6 Sol | ~170k | `module/src/quest.rs`, `module/tests/quest_choice_reward.rs` |
| T2 | Re-present the reward offer on a gameplay refusal — **DONE** (`496a0ce`) | GPT-5.6 Terra | ~150k | `gateway/src/world/handlers/quest.rs` and its focused tests only |
| T3 | Make live item insertion relays atomic and ordered — **DONE** (`0abc5cd`) | GPT-5.6 Sol | ~170k | `gateway/src/stdb/subscriptions.rs` and relay-focused tests only |
| T4 | Fence completion behind inventory visibility — **DONE** (`30168ad`) | GPT-5.6 Sol | ~160k | coordinator/reducer wiring and encrypted socket acceptance |

## Outcome

The tracer proved the authentic quest-33 import rows and durable transaction were already correct.
The player-visible loss was an ordering bug downstream: the completion reply could overtake the
asynchronous reward-item relay, whose own feedback also preceded the item CREATE. The integrated
fix batches CREATE, slot pointer and feedback in client-safe order, and routes the turn-in reducer
through the subscribed coordinator so those relays enqueue before completion is authorized. A
gameplay refusal now reopens the reward offer for retry.

## Shared rules

- Keep the vanilla contract: the client reward value is a zero-based index into the displayed,
  ordered choice array.
- Use the generic choice-reward path. Do not special-case quest 33 or either reward item.
- Keep turn-in atomic. A failed reward grant must preserve objectives and active quest state.
- Do not change schemas unless the tracer proves the existing schema cannot express the behavior.
- Reuse the shared item grant and existing quest/item packet builders.
- Tests assert durable/player-visible state, not helper call order or generated ids.
- Each ticket runs focused tests plus `cargo fmt`; T4 runs the complete affected-crate suite.
