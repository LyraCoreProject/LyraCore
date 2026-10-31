# Character Transfer

A native cross-map teleport starts a Transfer when the destination map belongs to a Peer World
Shard. The source sends `SMSG_TRANSFER_PENDING` and `SMSG_NEW_WORLD` first. It starts the Transfer
at `MSG_MOVE_WORLDPORT_ACK`, while the client is behind its loading screen. A Shard without a role,
or a World Shard without map assignments, keeps the local world-port path.

`module/src/native/transfer.rs` drives the protocol through durable IDC outboxes and their
`on_result` reducers. `module/src/transfer/mod.rs` holds the Escrow ledger (`game_transfer_out`,
`game_transfer_in`), planners, Store traits and retained Operator reducers. `transport.rs` holds the
row manifest and BSATN encoding. `tests.rs` and `harness.rs` exercise the protocol against Fakes.

## Ordering and recovery

Each numbered step commits in one transaction. IDC delivers each call exactly once and durably
returns its result to the source. A process crash rolls back an incomplete transaction; committed
outboxes and results resume on restart. A timeout alone never releases cross-Shard Escrow.

1. The teleport saves its origin in the World Session and persists its destination in the Character.
   The live entity has already left the world. Connection loss before acknowledgement restores the
   origin for a remote destination and marks the Character offline on its source.
2. Acknowledgement leaves the World Session's world presence, cancels a pending cast, freezes the
   offline Character in source Escrow and queues one IDC batch. That batch contains all manifest
   rows, Account and Session copies, connection proof, Character guid and teleport counter. Its
   Transfer id comes from the source's GUID allocator, whose ranges are distinct across Shards.
   Before this transaction commits, the source remains the sole copy. After it commits, only the
   queued call's result may settle Escrow. Gameplay packets wait while this connection is pending.
3. The target requires a Peer World Shard that names its Realm Shard, checks map ownership and
   Account ownership, imports through the existing idempotent import code, and writes an Escrow-in
   receipt and a WorldPort Handoff. All of these commit together. A refusal rolls back everything.
   A committed target is fenced and cannot enter the world yet; the source remains fenced too.
4. On IDC success, the source records the import attestation and runs the existing finish cascade.
   Deleting its Character and queuing the Realm Shard update commit together. On IDC refusal, it
   releases only unconfirmed source Escrow, restores the origin and closes the World Session.
   The target then has no copy. Source failure never assumes that a missing local receipt proves
   that the target failed.
5. The Realm Shard accepts an owner change only from the current directory owner, with a newer
   revision and the same Account. It updates the directory and the matching away World Session's
   `world` together. Old and duplicate revisions do nothing. Each World Shard observes the arriving
   revision and advances its directory clock, so a round trip remains ordered even across different
   wall clocks. A closed or replaced World Session is not restored by a late update.
6. After the Realm Shard accepts, the source queues a target release. The target checks the exact
   Transfer, source and connection against its Handoff before clearing the arrival fence. The source
   writes the Handoff marker only after this release succeeds. Directory and release refusals retain
   their work in new outboxes for retry; they cannot roll back a committed import. There is now one
   resident copy on the target even if the connection disappeared before adoption. If the source
   connection has closed, it asks the target to report World Session closure to the Realm Shard.
   The notice comes from the new owner and matches the old connection, so it cannot close a newer
   World Session.
7. The Native Host attaches the same connection to the target. `native_world_adopt` enters with
   `WorldEntry::WorldPort` and the saved teleport counter. Self CREATE finishes the loading screen.
   If the connection is lost, the next login routes to the target through the updated directory.
   A crash after a marker commits relies on the Native Host's existing ordered Handoff delivery.

The Escrow reaper holds an unattested cross-Shard source. The normal success result confirms and
finishes it in one transaction, so the reaper cannot delete a native source without also retaining
its queued directory update. The target release works without adoption; an abandoned connection
cannot leave the Character permanently fenced. A release for a closed source connection also
removes the unused target Handoff.

## State carried and state dropped

The Transfer uses the existing Character-owned BSATN manifest. Items, skills, spells, quests, Auras
and other transported Character-owned rows travel in one batch. Auras keep their absolute expiry.
AOI Grants, the NativePlayer row, Transport attachment and other explicitly omitted transient rows
are rebuilt or dropped. A pending cast is cancelled before export. Melee, threat and other combat
state do not survive. Leaving the world applies the existing logout cleanup, including pet despawn
and buyback cleanup. Connection request limits start with the adopted connection state.

Groups, guilds, whispers, who, friends, mail and auctions across World Shards remain unsupported.
The manifest does not make Shard-owned social or economic state shared. Transport attachment does
not survive a Transfer between World Shards. Sessionless Character Relocation does not use this
native World Session path.

## Manifest discipline

- `transfer::is_in_transit` hides a Character with an Escrow row from by-guid lookups and refuses
  world entry. The rest and quest passes also skip it.
- Every Character-keyed table needs a `character_owned!` delete sweep and Transfer arm. The build
  derives `CHARACTER_OWNED_TABLES` from them, and the manifest tests refuse a missing arm. See
  [Module build](module-build.md).
- Package Teardown refuses while a Package Character has Escrow or a claimed Transfer Intent.
- The Architecture Tests in `module/src/tripwires.rs` enforce the raw-lookup exclusions.
