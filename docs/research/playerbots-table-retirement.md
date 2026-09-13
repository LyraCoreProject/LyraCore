# PB012 physical retired-goal table plan

Status: offline plan only. It authorizes no Realm read, data cleanup, schema removal, publish, or
deployment. The accepted v19 imported observation does not replace attended-client acceptance.

## Prepared source after imported acceptance

- Held Core `ec578c40e1b4ca6136dee78624136cca5e5de747`, tree
  `e0fb880a207c9328791038b59e342f709abfef9c`.
- Held Package Collection `5ae9170775415e05e43d41269f42d473d4f5c3fe`, tree
  `b8c91b6a6b4a801393f63bddfd2d9a21ed1204b6`.

These preparations preserve the accepted Recovery readiness behavior and callers. The Core base
has the same tree as merge `35495170`; the Collection base is merge `63f82468`. All four Core
commits and seven of eight Collection commits replay identically. The remaining Collection
commit changes only four CI pins to the prepared Core. The inventory below still applies.
Core preparation review SHA-256
`a5ec9d4f8d10551997b0ef018cc9e10eefca8491e34ecd3d70cb52460b9fec75`;
Collection review `3c8127ec7089682fc36311fd84eb45db46c5ad85deba2a6b971e92157cf00c90`.
These exact preparations have not been built or run. Their final checks and delivery follow
attended acceptance. Earlier native checks retain their original source identities.

## Original source inventory

- Core `24f53d3168e813a94ffcfef785e875d1276e24c0`, tree
  `8e20dd29af2dd8618eb4bf3f7bea75d8a194fd5b`.
- Package Collection `621b9f47ba019efe2dcc1556dcab6de9620b9826`, tree
  `c6c7aafdd65714482024e91152d94db40590b566`.
- `docs/danger-zones.md` at Core `24f53d31`, blob
  `f49d890f1705cf51841a80302976d931ceb4bb25`.
- Retirement Architecture Test, blob
  `ce14158e324faa93473bf6f94d365139658bcda6`.
- Final retirement inventory, SHA-256
  `a64e4e385f808d550ddf2704e6394db17f37ce003a316e4210f06ec6f69c2178`.
- Final-source addendum, SHA-256
  `74689f85315dfa157aa023ebd8d78a10a4c62ce78e1c61adcce2a83742ae793c`.
- The last source before goal-policy deletion used for the historical `kind` map is Package
  `75fb4711f77e907af2cb4d676dd6435eb80da0c2`, tree
  `40862e4b7a3b4545fd700ebe577b4ab4e9714a22`.

This inventory came from Git objects and retained local reports. No Realm was queried.

## Retained serialized state

Two stored structures remain compatible. They must not be treated as one migration.

### `pkg_playerbots_bot.controller`

`Controller` is a SpacetimeDB sum type in this exact declaration order: `Legacy`, `RecordOnly`,
`Cohort`, `Frozen`. The end-appended roster field remains
`#[default(Controller::Legacy)]`, so rows created before the field existed retain their original
meaning when decoded. New bot constructors write `Controller::Cohort` explicitly. The held
retirement source parks any remaining Legacy row and refuses new Legacy selection.

Removing or reordering `Controller::Legacy`, or changing the field default, is outside the old-table
operation. It needs a separate serialized-roster migration and review even after the old table is
gone.

### `pkg_playerbots_goal`

This remains a public table with an auto-increment `u64 id` primary key and a non-unique btree index
named `by_character` on `character_guid`. One row per Character is an application invariant, not a
database uniqueness constraint.

| Column | Type and retained meaning |
| --- | --- |
| `id` | `u64`; Shard-local surrogate key. |
| `character_guid` | `u64`; Character whose retired Legacy state this was. |
| `kind` | `u8`; retired Legacy goal code. Only value `5` has current operational meaning. |
| `since_micros` | `i64`; wall-clock microseconds when the Legacy executor took the goal. |
| `stalled_since_micros` | `i64`, default `0`; retained Quest-stall start, with `0` meaning no recorded stall. |
| `hub_known` | `bool`, default `false`; whether the retained Quest hub coordinates were known. |
| `hub_x`, `hub_y`, `hub_z` | `f32`, each default `0.0`; retained Quest hub coordinates. |
| `stall_warned` | `bool`, default `false`; whether the Legacy stall warning had been emitted. |
| `quest_credit` | `Option<u64>`, default `None`; last Legacy objective total, with `None` meaning no baseline. |

The historical `kind` encoding is fixed: `0` Follow, `1` Fight, `2` Flee, `3` Wander, `4`
Stranded, `5` In Transit, `6` Quest Travel, `7` Quest Hunt, `8` Grind, and `9` Resurrecting. In the
held retirement source, values other than `5` are inert historical data. Value `5` still fences a
Legacy controller migration because it means an unsettled predecessor Transfer.

`pkg_playerbots_personality` is current Cohort row policy. It is not an old table and must not be
included in this cleanup. Removal of Runtime Script ids `100100` and `100101` is also a distinct
artifact reconciliation step, not a table migration.

## Allowed access in the held retirement source

Production access is deliberately narrow:

- `playerbots/src/mod.rs::PlayerbotsGoal` declares the schema.
- `sweep_delete_pkg_playerbots_goal` deletes rows through the Character deletion lifecycle.
- `sweep_transfer_pkg_playerbots_goal` declares that rows do not cross a Shard boundary.
- `runner::predecessor_transfer_pending` performs an indexed read by Character and returns true only
  when the retained row has `kind == 5`.
- `playerbots_migrate_legacy_controllers` is the only production caller of that read. It does not
  change the old goal row. It moves an eligible Legacy roster row to Cohort only after Transfer
  Intent, Runner checkpoint, Character presence, and predecessor In Transit checks pass.

The `debug_reducers`-only controller-transition fixture may insert or update a `kind == 5` row so the
predecessor boundary can be tested. Core integration tests may read the table for evidence. Neither
is a gameplay writer.

There is no current Operator operation that deletes arbitrary retired goal rows in bounded batches.
Normal tick, movement, combat, Quest, Recovery, Companion Order, and Transfer planning do not read or
write this table. The Architecture Test enforces these owners and rejects renamed access, writes from
the migration reader, and any return of Legacy gameplay.

## Cutover fence

Logical retirement and physical removal use different fences.

Before logical Legacy retirement is activated on a declared Realm, the accepted inventory requires
all applicable Shards to show:

1. zero `Controller::Legacy` roster rows after replay-safe, sorted migration batches of at most 16
   Character GUIDs;
2. zero active `game_bot_transfer_intent` rows for bots;
3. zero source `game_transfer_out` Escrows for bots;
4. zero non-null `pkg_playerbots_runner.transfer_checkpoint` values;
5. zero pending `pkg_playerbots_goal.kind == 5` compatibility rows; and
6. completed imported-world and attended-client acceptance on the transitioned source and cutover
   state.

The count is Realm-wide. One Package reducer sees one Shard and cannot prove it. A skipped migration
row is work to settle and retry, not permission to delete its state.

Physical removal adds stricter predicates:

- the logically retired Package has run on every Shard that can contain Package rows;
- source review confirms the compatibility declaration, indexed predecessor read, Character-delete
  sweep, and not-transported marker are still the only production references;
- every `pkg_playerbots_goal` row, including inert kinds `0` through `4` and `6` through `9`, has been
  inventoried and removed by an approved bounded cleanup;
- the table row count is zero on every applicable Shard; and
- any value outside the historical `0..=9` range stops cleanup for review.

Zero In Transit rows is enough for controller cutover. It is not enough to drop the table because
ordinary inactive rows intentionally survive that cutover.

## Future migration and review procedure

1. Freeze exact Core and Package commits. Review the then-current schema, all table references, the
   Architecture Test allowlist, and `docs/danger-zones.md` again. Record file and tree identities.
2. Complete the logical cutover and both acceptance rungs. Name the Realm and obtain explicit human
   approval before any Realm operation.
3. Run read-only counts on every applicable Shard for each cutover predicate, total old-table rows,
   and each historical `kind`. Preserve before-state evidence. Stop on an unknown value or any live
   Transfer fence.
4. Add a separate Operator-gated cleanup operation. It must accept a small, strictly increasing
   Character or row-id batch, use the indexed table access, be replay-safe, and refuse while that
   Character is Legacy or owns a Transfer Intent, source Escrow, or Runner checkpoint. It must never
   delete `kind == 5` while the predecessor Transfer remains unsettled. Test restart, replay, skipped
   rows, and mixed safe/unsafe batches through the durable interface.
5. Publish that cleanup release through the normal reviewed procedure. Run bounded batches on every
   applicable Shard, capture before and after counts, and require a stable zero count after a restart
   and another read-only pass.
6. Prepare a second source change that removes `PlayerbotsGoal`, `pkg_playerbots_goal`,
   `predecessor_transfer_pending`, its migration dependency, the delete sweep, and the
   not-transported marker. Update the Architecture Test and documentation in the same change. Keep
   `Controller::Legacy` and its roster default unless their own migration has been approved.
7. Treat that change as a schema change. Regenerate the complete Gateway bindings with
   `--include-private`; do not hand-edit them. Run schema parity, the retirement Architecture Test,
   controller transition coverage that remains applicable, the required Package/Core suites, and
   `./lyracore preflight` at the exact composed source.
8. Before deployment, document a recovery path that does not assume an old Package can run after its
   table has disappeared. Review the exact SpacetimeDB migration behavior in use at that time. A
   plain publish may auto-migrate or abort safely; an abort is a stop. Never substitute
   `spacetime publish -c`, which wipes the Shard.
9. With separate human approval, publish the reviewed schema to every production Shard in one
   coordinated operation. Partial schema publication is a failure because stale Shards can degrade
   silently. Run the required post-publish repair and cross-Shard checks from the current
   `docs/danger-zones.md`.
10. Verify that the old table and bindings are absent everywhere, Bot Controller counts remain at
    their approved values, no Transfer ownership was disturbed, and ordinary Runner behavior is
    unchanged. Preserve the operation and verification evidence as a new receipt.

## Current-release decision

Keep `pkg_playerbots_goal`, `PlayerbotsGoal`, the `kind == 5` migration fence, both Character
lifecycle registrations, `Controller::Legacy`, and its default in the current release. Ship no
physical drop with PB012 acceptance or Legacy executor retirement.

That is the safe choice because source retirement is reversible while the stored rows and Controller values stay
readable. Dropping the table destroys retained state, removes the fence an old Transfer may still
need, changes the published schema and Gateway bindings, and makes rollback to a table-reading
Package unsafe. Attended acceptance proves client behavior. It does not approve destructive schema
work.
