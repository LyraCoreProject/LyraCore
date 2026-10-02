# T3: Stone matching and party-queue rules

Parent: issue #532, meeting stones. Depends on T1. **Runs in parallel with T2.**
Model: Opus. Estimated size: ~200k tokens.

## Problem

After T1, Seekers wait forever. Nobody is grouped, a queued party never fills, and a queued party
that loses or kicks a member keeps stale Seeker rows. Vanilla groups Seekers by class role,
announces each stone add, completes the party at five, and reminds a waiting party every five
minutes. It also reacts to every membership change of a queued party. All of it is gameplay, so it
belongs in the Module on Realm-core, and every party it forms or changes must go through the same
group cores a player's invite uses.

## Delivery

All in the Module. No Gateway file. Keep T1's table shapes and reducer signatures.

### Roles, pure and unit-tested, in `module/src/meeting_stone.rs`

- Class roles (`cm:LFG/LFGMgr.cpp:91-106`): Druid and Paladin tank, healer, damage. Priest and
  Shaman healer, damage. Warrior tank, damage. Hunter, Mage, Rogue, Warlock damage. Class 0 fills
  nothing.
- Class priority per role (`cm:LFG/LFGMgr.cpp:108-158`).
- A party's **Open Roles** from its members' classes in join order: one tank, one healer, three
  damage. Each member takes the first of tank, healer, damage its class fills and that is still
  open, unless another not-yet-placed member has a higher priority for it
  (`cm:Groups/Group.cpp:1589-1685`). Worked example: paladin then warrior leaves three damage open.
- The next stone add: for tank, then healer, then damage, if open, the longest-waiting Seeker whose
  class fills it; ties by guid. This is what `cm:LFG/LFGQueue.cpp:292-390` does once its `bool`
  priority compare is read literally.

### The bucket pass, `match_bucket(ctx, area_id, team)`

Run it at the end of every transaction that can create a match: T1's JOIN, and the hooks below that
free an Open Role or queue a Character. Loop until nothing changes:

1. Each queued party in the bucket, oldest `queued_at` first, takes stone adds while it has fewer
   than five members. Before an add, push `MEMBER_ADDED(guid)` to the current members, then
   delete the solo Seeker, add the Character through the shared join core, and write its party
   Seeker row. At five members push `COMPLETE` then `QUEUE(0, NONE)` to every member and delete
   the party and its Seeker rows (`cm:LFG/LFGQueue.cpp:372-383,420-454`).
2. If the bucket still holds five or more solo Seekers, form a party: the longest wait leads, the
   next joins. Push `MEMBER_ADDED(member)` to the leader, create the party through the join core,
   queue it with `QUEUE(area, JOINED)` to both (`cm:LFG/LFGQueue.cpp:173-221`), then go to 1.

A solo Seeker whose Account Claim is closed or past `expires_micros` is deleted silently when the
pass meets it and is never added. T1 drops most of them when the claim ends; this check covers the
up to 15 s between expiry and the lease reaper. Read the bucket through the `(area_id, team)`
index; never scan the whole Seeker table.

### Group cores, `module/src/group.rs`

- Extract the part of `accept_invite_on` (`group.rs:1400-1517`) that creates a party for a leader
  when needed, inserts the member and pushes the roster into one join core. ACCEPT and the bucket
  pass both call it. There is no second way to add a member. The core keeps everything the accept
  does today: the `has_room` and `RaidSlot` rules, Vanilla's GROUP loot at Uncommon for a new
  party, folding the inviter's other pending invites into a new party, `push_list_to_all`, and
  `instance::reconcile_instance_removal` for the joiner and a new party's leader.
- Member identity in the join core (README decision 27): the local Character row's
  `owner_identity` when this database holds the row, else zero. Use the existing Character lookup
  chokepoint; `character_fence_tripwire` forbids a raw `game_character` lookup.
- Every transaction that changes a party roster advances its Roster Revision through
  `advance_group_revision`, the helper `realm_group_op` uses. That includes every party the bucket
  pass forms or fills during a `realm_meeting_stone_op` JOIN, and a party a kick re-queue fills.
  Advancing a party twice in one transaction is fine. T2's relay pushes whatever this advances.
- `realm_group_op` passes ACCEPT's `arg_a` class and `arg_b` race into the accept core. On the
  single-database plane (`gw_group_accept`, `Plane::Shard`) read the acceptor's class from its local
  `game_character` row.
- Hooks for queued parties, per the README fact table. Keep `remove_member` the one removal core;
  tell kick from leave with a cause argument or with work in `uninvite_on` before it calls
  `remove_member`. Deletion already arrives as `leave_group_on(.., CHARACTER_DELETED)` on
  Realm-core and as the `sweep_delete_game_group_member` marker on a single database; both end in
  `remove_member`.
  - Accept: a solo Seeker who joins any party loses its solo row, and gets `QUEUE(0, LEAVE_QUEUE)`
    unless the party is queued for the same area (`cm:Groups/Group.cpp:360-374`). A queued party
    gains the joiner's Seeker row with its class; five members completes it. A playerbot invited
    into a queued party gets a Seeker row the same way.
  - Leave or Character deletion, leader unchanged: leaver `QUEUE(0, NONE)`, the rest
    `QUEUE(area, PARTY_MEMBER_LEFT_LFG)`, the leaver's Seeker row goes, the party stays queued, then
    the bucket pass (`cm:Groups/Group.cpp:437-447,478-479`).
  - Leave, leader changed: leaver `QUEUE(0, NONE)`, the rest `QUEUE(0, LEAVE_QUEUE)`, the party
    leaves the queue (`cm:Groups/Group.cpp:464-476`).
  - Kick from a party that survives: the rest get `QUEUE(0, PARTY_MEMBER_REMOVED_PARTY_REMOVED)` then
    `QUEUE(0, LEAVE_QUEUE)`, the party leaves the queue. The kicked Character gets
    `QUEUE(area, LOOKING_FOR_NEW_PARTY_IN_QUEUE)`, becomes a solo Seeker with its stored class and
    team, gets `QUEUE(area, JOINED)`, then the bucket pass (`cm:Groups/Group.cpp:413-435`).
  - Disband, including a kick or leave from two members: every former member `QUEUE(0, NONE)`, the
    party leaves the queue, nobody is re-queued (`cm:Groups/Group.cpp:550-562`).
  - Raid convert (README decision 22): `raid_convert_on` calls
    `meeting_stone::dequeue_party(ctx, group_id, LEAVE_QUEUE)` when the party is queued. Every
    member gets `QUEUE(0, LEAVE_QUEUE)` before the raid list.
  - Set leader: nothing (README decision 23).

### Reminders

Add `game_meeting_stone_reminder_schedule`, a 5 s scheduled reducer with the scheduler-only gate.
Seed it in `seed.rs::seed_scheduler_arming` and re-arm it in
`debug/repair.rs::debug_repair_after_publish`, the weather precedent (`rearm_weather_schedule`).
Every party row with `next_reminder_at <= now` pushes `IN_PROGRESS` to each member and moves
`next_reminder_at` on by five minutes (`cm:LFG/LFGMgr.cpp:55`, `cm:LFG/LFGQueue.cpp:149-163`).
The party table is small, private and not spatial.

### Event order

Insert each recipient's events in wire order inside one transaction, for example `MEMBER_ADDED`
before the roster LIST it precedes in cmangos. T4 proves the relay keeps that order.

## Acceptance criteria

1. Five solo Seekers of one area and team form one party. With a tank, a healer and three damage
   classes it completes at once: every member ends with `COMPLETE` then `QUEUE(0, NONE)`, and no
   Seeker or party row remains.
2. Five mage Seekers form a party of leader, first member and one more damage add. It stays queued
   with tank and healer open, and two mages keep waiting.
3. A queued warrior and mage party takes the longest-waiting healer-capable Seeker as healer. A
   second priest waits.
4. Existing members get `MEMBER_ADDED(guid)` for each stone add. The added Character does not.
5. Horde and Alliance Seekers of one area never meet. Different areas never meet.
6. A Seeker without a live Account Claim is never added and its row is gone after the pass.
7. Each leave, kick, disband, accept and raid-convert case in the hooks list produces exactly the
   listed events and queue rows, on both planes.
8. A kicked Character re-queued alone joins a second queued party in the same transaction when a
   role fits.
9. Every party the Module forms or changes has a higher Roster Revision after the transaction.
10. A party row whose reminder is due sends `IN_PROGRESS` to every member once and schedules the
    next five minutes later.
11. A stone-formed party has the same `game_group` defaults as an invited one, group loot at
    Uncommon, because both go through the one join core.
12. A Character who joins a stone party inside a dungeon instance its new party owns has its
    Instance Removal cancelled, as an invited one does.

## Verification rungs

- Unit: the role table and priorities against the cmangos rows cited above; Open Roles for the
  paladin-then-warrior case, a two-warrior party and a full party; the stone-add pick order with ties;
  the formation threshold at four and five Seekers.
- Durable Module: `module/tests/meeting_stone_matching.rs` with `support::Standalone`, staged
  through T1's debug fixtures. One test per criterion 1-3 and 5-12, asserting `game_group`,
  `game_group_member`, `game_group_roster_revision`, the Seeker and party tables and the ordered
  `game_group_event` rows per recipient. Criterion 7 runs through `realm_group_op` for the
  Realm-core plane and through the `gw_group_*` reducers for the single-database plane. The
  existing `raid_convert.rs`, `instance_removal.rs` and `loot_tag.rs` durable tests must stay green
  after the join core extraction.
- The Module wasm check and `cargo test -p lyracore-module --lib` for the tripwires.

## CONTEXT.md terms

In the `### Meeting stones` section T1 created:

- **Open Role**: one of a queued Party's one tank, one healer and three damage roles that no
  member's class fills yet. _Avoid_: vacancy, role slot.
- **Stone Add**: the Module adding a Seeker to a queued Party, or forming a Party from five waiting
  Seekers, through the party authority's join core. _Avoid_: matchmaking, auto-invite.

## File ownership

- `module/src/meeting_stone.rs`: everything except T1's table shapes and reducer signatures.
- `module/src/group.rs`: the join core, the accept, leave, uninvite, raid convert and
  `remove_member` cores, `realm_group_op`'s ACCEPT arm and revision advances.
- `module/src/seed.rs` and `module/src/debug/repair.rs`: the reminder schedule's seed and re-arm.
- `module/src/debug/meeting_stone.rs`: may add fixtures. A new table or reducer means regenerating
  bindings and saying so in the handoff. T2 touches no bindings.
- `module/tests/meeting_stone_matching.rs` (new).
- `CONTEXT.md`: the two entries above.

No Gateway files except regenerated bindings. No importer.

## Non-goals

- No talent roles, no summoning, no playerbot queueing.
- No mirror push. T2 owns it.
- No change to T1's Gateway flow.

## Definition of done

Touched files formatted. Module wasm check, `cargo clippy -p lyracore-module`,
`cargo test -p lyracore-module` and the new durable test clean. The reminder schedule table means
bindings are regenerated; the binding check and `cargo test -p lyracore-gateway` are clean too.
Commit on your worktree branch and report the exact commands, results and any schema or fixture
change T4 must know.
