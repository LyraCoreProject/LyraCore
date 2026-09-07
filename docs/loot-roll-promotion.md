# Loot Roll promotion identity

A promotion names the source Module identity and the source LootRoll row id. The Module stamps `promotion_source` with `ctx.database_identity()` when it stages a local roll. This is the identity of the source Module, not the Operator identity shared by Gateway connections. The Gateway copies both values from the staging row on every retry.

Realm-core keeps one promotion receipt per source identity, corpse guid and slot. The receipt stores the greatest accepted source row id. A START with that id or an older id succeeds without creating rows. The Gateway can then clear exactly that staging row by its id. A larger id can start after the active roll on the corpse and slot has resolved. If that roll is still active, the request leaves all durable state unchanged and the Gateway keeps the staging row for retry.

The source table's auto-increment sequence orders generations within one Module identity. Ordinary publish and restart preserve the table and its sequence. Content imports must not replace this gameplay table or insert explicit LootRoll ids. Restoring an older copy under the same identity can reuse ids and is outside this guarantee. Such a restore needs a separate reviewed recovery procedure.

## Cleanup

Resolution and staging cleanup remove the active LootRoll and its votes. They do not remove promotion receipts. A later accepted generation updates the existing receipt, so it also continues to cover every earlier generation. Receipts for unrelated slots or source identities have independent order.

There is no time-based expiry. Storage grows with distinct source identities, corpse guids and slots, including dynamic creature guids. Retiring a source does not make its delayed requests invalid, so retirement alone does not permit receipt deletion. Deletion would need a durable protocol that refuses every request from the retired identity before removing its receipts. This implementation has no such operation.

## Upgrade review

This change adds a defaulted `Identity` column to LootRoll, a private receipt table, and two reducer arguments. The default identity is ZERO. START refuses ZERO and source row id zero, so an old staging row cannot silently enter a new identity namespace.

Human review under `docs/danger-zones.md` section 1 is required before shipping. The reviewer must approve the additive schema, generated bindings, retained receipt storage and the following upgrade boundary:

1. Stop every Gateway that can promote a Loot Roll. Keep the Modules running so their scheduled deadline sweeps can finish existing rolls. Stop other Operator clients that can send START.
2. On every World Shard, Instance Pool and Realm-core, check through SQL that both `game_loot_roll` and `game_loot_roll_vote` have zero rows. Normal roll deadlines are 60 seconds. Check the state rather than assuming that waiting 60 seconds drained it. If rows remain, diagnose their deadlines and the scheduled sweep before continuing.
3. Publish the new Module to every destination with the ordinary non-destructive publish procedure. Do not reset tables or their auto-increment sequences.
4. Start only the Gateway build that sends the new promotion identity arguments. Old Gateway processes and their retained requests must remain stopped.

The empty-roll checkpoint prevents an old Realm-core roll from resolving without a corresponding receipt while an old staging copy is still eligible for promotion. An idempotent source-column backfill alone would not establish that missing Realm-core receipt.

No production upgrade was performed while developing this change. The regression suite uses owned disposable Standalone fixtures.
