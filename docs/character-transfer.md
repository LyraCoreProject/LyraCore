# Character Transfer

The native runtime has one Shard, so nothing starts a Character Transfer. No Native Host or Module
code calls `begin_transfer` or the other Transfer reducers outside tests. They stay published, with
their tables, for schema compatibility.

`module/src/transfer/mod.rs` holds the Escrow ledger (`game_transfer_out`, `game_transfer_in`), the
planners, the Store traits and the Operator reducer shims. `transport.rs` holds the row manifest and
BSATN encoding. `tests.rs` and `harness.rs` run the protocol against two Fakes.

A Transfer freezes the source Character, imports it at the destination, and deletes the source copy
only after the import receipt exists. `finish_transfer` refuses without that receipt.

The rest of the Module still reads the ledger:

- `transfer::is_in_transit` hides a Character with an Escrow row from by-guid lookups
  (`helpers::character_by_guid`, `character_by_name`), refuses `world::player_login`, and keeps
  the rest and quest passes from touching it.
- Every Character-keyed table needs a `character_owned!` delete sweep and transfer arm. The build
  derives `CHARACTER_OWNED_TABLES` from them, and the manifest tests in `transfer/tests.rs` reject a
  missing arm. See `docs/module-build.md`.
- Package Teardown refuses while a Package Character has an Escrow row or a claimed Transfer Intent.
- The Architecture Test in `module/src/tripwires.rs` enforces the documented raw-lookup exclusions.
