# 0008. Cross-Shard retries and revision fences stay per feature

Accepted, 2026-11-11.

`module/src/native/` holds twelve scheduled retry tables, each paired with a reducer that
re-inserts a refused cross-Shard call, and about a dozen hand-written revision comparisons. They
look like one mechanism copied many times. An architecture review on 2026-11-11 proposed one shared
retry module and one fence helper, and the proposal was rejected. Each feature keeps its own retry
table and reducer, and each stream chooses its own fence comparison.

## Why the retries stay separate

- A retry table stores its feature's typed outbox row, and each outbox table binds one remote
  reducer. A shared retry needs a new table that names its target outbox by kind and dispatches on
  it.
- Removing a table aborts publish ([danger zones](../danger-zones.md)). The twelve tables would
  move to `module/src/retired.rs` after a drain. They would not go away.
- `module/build.rs` finds scheduled tables by scanning source text for the virtual clock registry
  and the table catalogue. A macro that generated the table and reducer pair would be invisible to
  it.
- The pairs differ on purpose. Mail routes a retried parcel again and does not send it to its old
  target. The Package guild stream retries invitations only. Auction retries both settlements and
  deliveries. Transfer waits 30 seconds, for the reason its comment gives.

A retry policy fix is therefore a few lines at each site.

## Why the fences differ

A stream whose rows each carry their own revision refuses an equal revision: guild projections,
guild membership records and Character profiles. The Group stream accepts an equal revision,
because one update carries many rows at one revision, and both a roster and the absent list of that
update can write the same membership row. One comparison rule for both would drop valid Group
updates.

## What would reopen this

A SpacetimeDB release that lets one outbox table target several reducers, or that lets a Module
drop a table, removes the first two reasons.
