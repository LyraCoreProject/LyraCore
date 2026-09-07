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

1. Prevent new gameplay and Operator requests from creating Loot Rolls, including bot combat. Keep the old Gateways running while existing rolls resolve so their relays can deliver winner settlement to the World Shards.
2. On every World Shard, Instance Pool and Realm-core, require zero rows in `game_loot_roll` and `game_loot_roll_vote`, and zero rows in `game_corpse_loot WHERE withheld = true`. An empty active-roll table alone does not prove that the winner received the item. Check the state rather than assuming the 60-second roll deadline completed settlement. Diagnose any remaining rows before continuing.
3. Once settlement has drained, stop every old Gateway and other Operator client that can promote or settle a roll. Keep gameplay producers quiescent. Recheck every intended destination before the first publish.
4. Publish the new Module to every destination with the ordinary non-destructive publish procedure. Do not reset tables or their auto-increment sequences.
5. Start only the Gateway build that sends the new promotion identity arguments. Old Gateway processes and their retained requests must remain stopped.

The empty-roll checkpoint prevents an old Realm-core roll from resolving without a corresponding receipt while an old staging copy is still eligible for promotion. The withheld-loot check also prevents publishing between resolution and winner settlement. A source-column backfill alone would not establish the missing Realm-core receipt.

A controlled check published baseline `3455aa2` to two disposable Shards, created a source staging roll and a Realm-core roll, then cast the winning votes. Both roll tables became empty while the source still held withheld loot. Delivering the old `settle_loot_roll` request granted exactly one item and cleared the final pending row. Publishing this change then preserved that item and the source sequence, refused the old START arguments and produced a nonzero identity on new staging rows. This checks the Module transition; the publish command's separate all-destination check enforces the checkpoint.

No production upgrade was performed while developing this change. The regression suite uses owned disposable Standalone fixtures.
