# Character Transfer

`module/src/transfer/mod.rs` owns the Escrow ledger, planners, Store traits, and reducer shims.
`transport.rs` owns the row manifest and BSATN encoding. `tests.rs` checks planner outcomes and
manifest coverage. `harness.rs` executes the protocol against two Fakes.

A Transfer freezes the source Character before importing it at the destination. The source durable
copy survives until the destination copy is committed. Import is idempotent by Transfer id.
`finish_transfer` refuses without an import receipt, so a retry cannot delete the only durable copy.

```text
Source                          Destination
begin_transfer
    export blob -------------> import_character_blob
    committed <--------------- import receipt
confirm_import
finish_transfer
    source gone -------------> release_transfer
```

`confirm_import` records the destination's commitment on the source. `release_transfer` clears the
destination fence only after the source copy is gone. A same-database Transfer uses the same ledger
and planners, but changes the Character's partition without making a second durable copy.

An out-row or in-row makes the Character in transit. Four boundaries enforce the fence:

- `helpers::entity_by_owner` refuses player actions.
- `world::player_login` refuses to materialize another live entity.
- `begin_transfer` deletes the source live entity, which removes it from target lookup and relays.
- `helpers::character_by_guid` and `character_by_name` hide the durable Character from by-guid callers.

A by-guid operation needs a deliberate outcome. Most operations refuse through the fenced lookup.
`loot::credit_purse` defers a third party's copper into the Transfer blob. `auth::establish_session`
regenerates connection identity at the destination. Group membership is authoritative on Realm-core;
the Gateway projects it at world entry, and it is excluded from the blob. The Architecture Test in
`module/src/tripwires.rs` enforces the documented raw-lookup exclusions.

Every transported table has a `character_owned!(transfer, ..)` marker beside its declaration.
Generated registries name each arm and whether it transports rows. The manifest tests reject a
missing arm. `ExportBlob` carries the Character row and one opaque `TableRows` payload per table.
Before applying rows, import refuses unknown tables, malformed payloads, and missing registry
entries. Deliberately untransported tables still require an empty payload entry.

Recovery never guesses whether a destination import committed. A same-database reaper reads the
local receipt and may roll back an unimported stale Escrow. Across databases, a missing local
receipt means the destination is unconsulted. The reaper holds the Escrow, and the Gateway resumes
forward recovery at the next world entry. A frozen Character is recoverable; deleting or duplicating
one is not.

The Fakes execute the shared planners and protocol bodies. Durable tests exercise reducer shims,
real table arms, and SpacetimeDB transaction rollback. These checks cover different boundaries.
