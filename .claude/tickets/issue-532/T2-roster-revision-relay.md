# T2: Roster Revision Relay

Parent: issue #532, meeting stones. Depends on T1. **Runs in parallel with T3.**
Model: Opus. Estimated size: ~110k tokens.

## Problem

World Shard party mirrors change only when this Gateway's own `party::run` pushes the acting
Character's before and after groups (`gateway/src/world/party.rs:1003-1007`), at world entry, on
deleted-Character cleanup, and in `reconcile_deleted_character_parties` (`party.rs:1327-1371`). A
roster change the Module makes on Realm-core for a party the actor is not in never reaches the
mirrors. Main has no relay for it; verified on `da11dcfc`.

T3 makes that normal. A Seeker joining a queue can form a party from five other Seekers. A kick can
put the kicked Character into a second party. Those parties' mirrors stay stale until someone's next
op or world entry, so `/p`, the XP split, loot rules and the dungeon binding read the wrong roster
on every shard.

Realm-core already advances `game_group_roster_revision` for every roster change, and each Gateway
connection of a sharded realm caches that table. The mirror can follow the revision.

## What already exists, and is reused

- `sync_group_mirrors_required` (`party.rs:1380-1428`) reads the Realm-core roster or its disband
  tombstone, certifies partitions, and pushes to every World Shard with three attempts inside a
  5 s retry window. The relay calls it; it writes no retry logic of its own.
- A Realm-core reconnect already starts `reconcile_deleted_character_parties`, which pushes every
  known party (`gateway/src/stdb/subscriptions.rs:4237-4248`). Changes missed while disconnected
  need no new pass.
- `spawn_bot_invite_relay` and `arm_bot_invite_relay` (`subscriptions.rs:4084-4140`) are the shape
  for a callback relay that re-arms on reconnect and runs its work off the SDK callback thread.

## Delivery

**Roster Revision Relay**: when a Realm-core `game_group_roster_revision` row is inserted or
updated, push that party's authoritative roster to every World Shard whose mirrored revision is
older or missing.

- `gateway/src/world/party_mirror.rs` (new):
  - A pure selection of stale shards from the Realm-core revision and each shard's mirrored
    revision. A shard with no row for the party is stale.
  - A coalescing set of dirty party ids with one worker thread per Gateway process. The worker
    drains the set, selects stale shards from the caches, and calls `sync_group_mirrors_required`
    for those shards only. Several revisions of one party before the worker runs cause one push.
  - A push that still fails logs a warning naming the party and shard. The party then waits for
    its next revision or the reconnect pass. Never panic the process.
- `gateway/src/world/party.rs`: give `sync_group_mirrors_required` the target shards as an
  argument; its three existing callers pass `store.world_stores()`. Change nothing else in
  `party.rs`.
- Mirrored revision read: `stdb/reads/party.rs:207-216` `group_roster_revision` answers 1 for a
  missing row. Add a read that returns `Option<u64>` (the shard's row, or `None`), a matching
  `WorldStore` method in `world/store.rs` with a default, its `stdb/world_store.rs` impl, and the
  `InMemoryStore` Fake in `world/tests.rs`. Leave `group_roster_revision` as it is.
- `gateway/src/stdb/subscriptions.rs`: `spawn_roster_revision_relay` and `arm_roster_revision_relay`
  after `arm_bot_invite_relay`. Register `on_insert` and `on_update` for `game_group_roster_revision`
  on the Realm-core connection only. Callbacks enqueue a party id and return; they never call a
  reducer. Re-arm through Realm-core's `on_reconnect`. The initial subscription apply may enqueue
  every row; the stale check finds them current and pushes nothing, so it needs no special case.
- `gateway/src/main.rs`: call `coordinator.spawn_roster_revision_relay()` beside the other relays,
  and add an Architecture Test beside the bot relay ones proving `main` calls it and the re-arm is
  installed.
- Arm nothing when Realm-core is not a distinct database.

Keep `party::run`'s inline push. It gives the acting session its own party on its next packet. The
relay then finds those shards current and skips them.

## Acceptance criteria

1. A roster change committed by calling `realm_group_op` directly on Realm-core, with no
   `party::run`, reaches every World Shard mirror. Each shard's `game_group_roster_revision` then
   equals Realm-core's.
2. A disband reaches every shard as the tombstone and the mirror forgets the party.
3. A shard already holding the revision gets no `sync_group_mirror` call.
4. Five revisions of one party before the worker runs cause one push per stale shard.
5. No reducer call runs on an SDK callback thread. A slow or unavailable shard delays only the
   worker.
6. After a Realm-core reconnect the relay is armed again.
7. A single-database Gateway spawns no relay.
8. A failed push is logged with party and shard after `sync_group_mirrors_required`'s attempts.

## Verification rungs

- Unit: stale-shard selection, including a missing row and an equal revision; coalescing.
- Gateway Store Fake: tests in `gateway/src/world/party_mirror.rs` using
  `party_tests::party_topology`. Change the `FakeParty` roster directly, as the matcher would, run
  the worker once, assert both shard mirrors. Cover the tombstone and the skip.
- Durable Gateway: `gateway/src/stdb/subscriptions_roster_relay_durable_tests.rs`, the
  `subscriptions_character_gone_durable_tests.rs` shape: a real Realm-core plus two World Shards, a
  Coordinator with the relay spawned, `realm_group_op` INVITE and ACCEPT sent straight to Realm-core
  with the private CLI, then poll both shards' `game_group_member` and
  `game_group_roster_revision`. Run it locally; CI wiring is a README maintainer question.
- Architecture Test in `main.rs`.

## CONTEXT.md terms

Under "Sharding and transfer", after **Roster Revision**:

- **Roster Revision Relay**: the Gateway's push of a party's Realm-core roster to each World Shard
  whose mirror holds an older Roster Revision or none, whatever changed it on Realm-core. _Avoid_:
  mirror sync thread, roster watcher.

## File ownership

- `gateway/src/world/party_mirror.rs` (new) and its tests.
- `gateway/src/world/mod.rs`: one module declaration line.
- `gateway/src/world/party.rs`: `sync_group_mirrors_required`'s signature and its three call sites.
- `gateway/src/stdb/reads/party.rs`, `gateway/src/world/store.rs`, `gateway/src/stdb/world_store.rs`,
  `gateway/src/world/tests.rs`: the mirrored-revision read and its Fake, nothing else.
- `gateway/src/stdb/subscriptions.rs`: the new spawn and arm functions, placed after
  `arm_bot_invite_relay`. Do not touch `group_event_outbound`.
- `gateway/src/main.rs`: the call and its Architecture Test.
- `gateway/src/stdb/subscriptions_roster_relay_durable_tests.rs` (new) and its module line.
- `CONTEXT.md`: the one entry above.

No Module files, no bindings. T3 owns the Module and may regenerate bindings.

## Non-goals

- No meeting-stone logic. The relay does not know why a revision moved.
- No change to `sync_group_mirror`, `party::run`, or the reconciliation passes.
- No new retry policy and no new full reconciliation pass.

## Definition of done

Touched files formatted. `cargo clippy -p lyracore-gateway` and `cargo test -p lyracore-gateway`
clean, including the new durable test where the standalone toolchain exists. Commit on your
worktree branch and report the exact commands and results.
