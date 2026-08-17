# T2 — Re-present the reward offer on a gameplay refusal

Parent: issue #255. Depends on T1. Runs in parallel with T3.
Model: GPT-5.6 Terra. Estimated size: ~150k tokens.

## Problem

The gateway correctly withholds `SMSG_QUESTGIVER_QUEST_COMPLETE` when durable turn-in fails, but it
currently sends an empty batch. A full bag or invalid choice therefore leaves the stock client on a
dead-end screen instead of returning it to the reward offer for retry.

## Delivery

In the quest action seam, classify a durable gameplay refusal as handled and return the current
offer-reward screen when quest details are still readable. Preserve fatal transport propagation.
Add concise diagnostic context containing quest id and selected reward index without private player
data. Never build or send the completion popup on refusal.

## Acceptance criteria

1. A gameplay refusal returns exactly the offer-reward screen for the giver and quest.
2. No quest-complete packet is present on refusal.
3. Missing quest details on refusal produces an empty batch rather than a false success.
4. Reducer transport loss remains fatal.
5. The log identifies the quest and reward index without account or character-private data.

## File ownership

Own only `gateway/src/world/handlers/quest.rs` and its in-file focused tests. Do not edit module,
importer, subscriptions, or broad socket-test files.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, and the focused quest-handler tests are clean.

