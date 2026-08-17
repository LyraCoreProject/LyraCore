# T3 — Prove immediate item visibility and completion ordering

Parent: issue #255. Depends on T1. Runs in parallel with T2.
Model: GPT-5.6 Sol. Estimated size: ~170k tokens.

## Problem

The durable transaction may insert the reward while the stock client does not see the item until a
relog. Item row callbacks currently emit gain feedback, object CREATE and slot-pointer updates on a
separate relay path from the synchronous quest-complete reply. Their observable order has not been
tested for a quest reward.

## Delivery

Add a focused relay seam/test that applies a newly granted item row and observes the outbound batch:
item object CREATE precedes its inventory/container slot pointer, and both are made observable before
or consistently with quest completion presentation. Reuse the current item relay and packet builders.
If an ordering fence is required, keep it generic for every durable item grant rather than detecting
quest ids or item entries.

## Acceptance criteria

1. A new backpack reward emits item CREATE and then the player inventory-slot pointer.
2. A reward in an equipped bag emits item CREATE and then the container-slot pointer.
3. Gain feedback does not replace either required object update.
4. Initial-login item rows remain suppressed from gain feedback and are not duplicated.
5. The stock-client-visible inventory update cannot be overtaken by a quest-complete presentation.
6. Existing vendor, loot and item-move relay behavior stays unchanged.

## File ownership

Own `gateway/src/stdb/subscriptions.rs` and relay-local tests only. Do not edit the quest handler,
module, importer, or broad world socket tests.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, and focused subscription/relay tests are clean.

