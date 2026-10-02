# T1: Queue and leave at a meeting stone, alone or as a party

Parent: issue #532, meeting stones. **Tracer. Runs alone. Blocks T2, T3, T4.**
Model: Opus. Estimated size: ~200k tokens.
Base: `main` at `da11dcfc` or later, branch `feat/meeting-stones`.

## Problem

A 1.12.1 client can right-click a meeting stone, and nothing answers. The importer drops the stone's
level range and dungeon area (`importer/src/main.rs:703-706`). No table holds a queue, no reducer
accepts one, and the Gateway has no arm for `CMSG_MEETINGSTONE_JOIN`, `_LEAVE` or `_INFO`. The
queue must be realm-wide: a Character who queued in Westfall and then walked into the Deadmines
instance is on another shard and still waiting.

This ticket fixes every contract the other tickets build on: schema, reducer signatures, shared
bytes, event kinds and the Gateway flow. T2 and T3 then change no signature and share no file.

## Delivery

### Shared contract, `crates/lyracore-shared`

- `constants.rs` `go_type`: add `MEETINGSTONE = 23` with a pin test, like `QUESTGIVER`.
- New `meeting_stone.rs`, exported from `lib.rs`:
  - `realm_op`: `JOIN = 0`, `LEAVE = 1`. Its own byte space; `realm_meeting_stone_op` is a separate
    reducer from `realm_group_op`.
  - `queue_status`: 0 `LEAVE_QUEUE` .. 5 `NONE`, and `join_failure`: 1 `PARTY_LEADER`,
    2 `FULL_GROUP`, 3 `RAID_GROUP` (`cm:LFG/LFGDefines.h:50-67`).
  - `MeetingStoneRefusal` with stable `meeting_stone:*` tags and `ALL`, the `GroupRefusal` shape:
    `NotLeader`, `RaidGroup`, `PartyFull`, `ActorUnavailable`, `NotAMeetingStone`, `OutOfRange`,
    `OtherPartition`, `LevelOutOfRange`, `UnknownArea`.
  - `join_failure_for(refusal) -> Option<u8>`: `NotLeader` 1, `PartyFull` 2, `RaidGroup` 3. Every
    other Refusal is silent, as in both cores.
  - `event_kind`: `QUEUE = 27`, `MEMBER_ADDED = 28`, `IN_PROGRESS = 29`, `COMPLETE = 30`.
  - `encode_queue(area_id, status)` and `decode_queue`, which fails closed on a status above 5.
- `group.rs`: in the `event_kind` doc table, replace "27-30: reserved for meeting stones" with a
  pointer to `crate::meeting_stone::event_kind`. In the `realm_op` doc, ACCEPT reads `arg_a` as the
  acceptor's class and `arg_b` as its race, 0 for unknown.
- One unit test that every `game_group_event` kind is distinct: `group` 0-3 and 20-26, `loot_roll`
  4-8, `quest::share_event_kind` 10-11, `meeting_stone` 27-30, and that none is 9.

### Importer, `importer/src/main.rs`

- Name `got::DATA2` (dump column 10).
- Capture `data2` in `GoMeta` for raw type 23, beside the trap fields (`main.rs:4940-4964`).
- In `gameobject_template_rows`, emit a `game_meeting_stone` row `(entry, data0, data1, data2)` for
  every used template with raw type 23. Return it beside the trap rows, clear and reload it with the
  `gameobjects` family exactly as `game_gameobject_trap` is (`main.rs:5549-5559`), and add its
  count to that family's provenance stamp (`main.rs:5765-5770`).
- The type census prints `type 23 MEETINGSTONE <n> (meeting stone flow, not use-dispatched)`,
  the `QUESTGIVER` line's shape.
- The template row keeps its `other`-arm shape. The stone row is the only copy of the three values.
- The Package Delta `gameobjects` family does not change (README decision 25).

### Module, `module/src/meeting_stone.rs` (new)

Tables as in the README's durable-state list, with `SeekerFacts { character_guid, race, class }`
as a `SpacetimeType`. All three tables private. No schedule table in this ticket.

- `gw_admit_meeting_stone(request_actor: SessionActor, go_guid)` on the Home Shard.
  `require_operator`, then `require_actor`; a tokenless actor (`ownership: None`) is
  `ActorUnavailable`. `taxi::is_in_flight` is `ActorUnavailable`. Call `gameobject::usable_go` and
  map `MissingActor`, `OtherPartition`, `OutOfRange` and `MissingTarget` to `ActorUnavailable`,
  `OtherPartition`, `OutOfRange` and `NotAMeetingStone`. A template that is not type 23 or has no
  stone row is `NotAMeetingStone`. A level outside `min_level..=max_level` is `LevelOutOfRange`;
  `max_level = 0` has no upper bound. Writes nothing.
- `realm_meeting_stone_op(op, request_actor: SessionActor, area_id, seekers: Vec<SeekerFacts>)`,
  the authority op. `require_operator`, `require_actor`, tokenless is `ActorUnavailable`, area 0 is
  `UnknownArea`, more than `GROUP_MAX_MEMBERS` facts is an untagged error. Team comes from the
  leader's or actor's race through `lyracore_shared::faction::team_for_race`. A missing fact means
  class 0.
  - JOIN, not in a party: upsert the solo Seeker with `queued_at = now`. Push `QUEUE(area, JOINED)`
    to the actor. Re-joining replaces area and wait time.
  - JOIN, in a party, checked in the core's order (`cm:LFG/LFGHandler.cpp:58-77`): not its leader
    is `NotLeader`; `GroupKind::Raid` is `RaidGroup`; five members is `PartyFull`.
  - JOIN, leader of a Party with room: upsert the party row with `next_reminder_at = now + 5 min`,
    replace its Seeker rows with one per current member, push `QUEUE(area, JOINED)` to every member.
  - LEAVE: a solo Seeker is deleted and gets `QUEUE(0, LEAVE_QUEUE)`. A leader of a queued party
    deletes the party and its Seekers, and every member gets `QUEUE(0, LEAVE_QUEUE)`. Anyone else in
    a party gets `QUEUE(0, NONE)`. Nobody queued and no party: `Ok`, no event.
  - Membership reads go through `group::checked_group_membership`, `group_kind_of` and
    `members_of`. Events go through `group::push_event`. Widen `checked_group_membership`, its
    error type `GroupOpError`, and `gameobject::usable_go` to `pub(crate)`; change nothing else in
    `group.rs` or `gameobject.rs`.
- `meeting_stone::claim_ended(ctx, character_guid)` deletes that Character's solo Seeker row, no
  event. A party Seeker row stays. Call it at the three points in `account_ownership.rs` that call
  `channel::leave_all` (`claim_account` replacing a dead generation, `release_account_claim`,
  `reap_account_claims`). If the two calls then always travel together, fold them into one
  `account_ownership` helper so the next claim-scoped feature has one place to hook.
- `tripwires.rs`: add `game_meeting_stone_seeker` to `NOT_CHARACTER_OWNED` with the reason
  "Realm-core Meeting Stone Queue; a solo Seeker ends with its Account Claim, a party Seeker with
  its party membership". `game_meeting_stone_party` has no Character guid column, so it needs no
  entry, and the census test fails on one. No `character_owned!` markers.
- `debug/meeting_stone.rs`, behind `debug_reducers`, ids `509_6000`-`509_6099` (grep first): stage
  a stone template, stone row and spawned GO; stage Characters with chosen race, class and level
  beside it, each with a live Account Claim; stage a Party or Raid led by one of them; backdate a
  Seeker's `queued_at` and a party's `next_reminder_at`. T3's durable tests use these, so make them
  general enough for five Characters.

T1 does no matching. A queued party waits until T3.

### Gateway

- Regenerate bindings once, with the hand-patches, and run `scripts/check-gateway-bindings.py`.
- `stdb/connection.rs`: add `game_meeting_stone` and `game_meeting_stone_seeker` to the base list
  of `coordinator_queries`, beside the Chat Channel tables. Both are small, and a single-database
  realm needs them.
- `stdb/reads/templates.rs` and `codec/gameobject.rs`: add `data2` to `GameObjectTemplateView`.
  For a type-23 template fill `data0..data2` from its stone row; send
  `raw_data: [data0, data1, data2, 0, 0, 0]`. Other types keep `data2 = 0`.
- Reads: the stone area for a spawned GO guid on this shard, and the Seeker's area for a Character.
- `stdb/reducers.rs`: wrappers for both reducers returning `Ran` or `Refused(MeetingStoneRefusal)`.
  An untagged error stays `Err`.
- `world/handlers/meeting_stone.rs` (new), the `dispatch_channel_action` shape:
  `dispatch_meeting_stone_action` over a narrow `MeetingStoneActionStore` trait, implemented for
  `Coordinator` and for a Fake. The Coordinator picks the database: admission on the actor's Home
  Shard, the op on `realm_store()` or the one database. Wire it into the chain in
  `world/mod.rs` beside `dispatch_channel_action`. Opcodes:
  - `CMSG_MEETINGSTONE_JOIN`: admission; stone area; `Area::try_from` refuses → debug log, no
    further call; facts for the actor, or for every member of the actor's Realm-core roster via
    `presence::character_anywhere`; then the op. `join_failure_for` decides whether to answer
    `SMSG_MEETINGSTONE_JOINFAILED`.
  - `CMSG_MEETINGSTONE_LEAVE`: the op, nothing else.
  - `CMSG_MEETINGSTONE_INFO`: `SETQUEUE(area, JOINED)` when the Character has a Seeker row, else
    `SETQUEUE(0, NONE)`.
  - `MSG_LOOKING_FOR_GROUP`: answer `MSG_LOOKING_FOR_GROUP { unknown1: 0 }`.
  - All four drop silently outside the world.
- `world/party.rs`: ACCEPT carries the acceptor's class and race. Pack them in `Op::realm_args`
  only, so one place fills `arg_a`/`arg_b`. Both Realm-core accept paths supply them: the client
  accept through `run` and the playerbot accept in `answer_for_session_less`. Read them through
  `presence::character_anywhere`, 0 when unknown. The single-database `group_accept` path is
  unchanged; T3 reads the local Character row there.
- Codec builders for `SETQUEUE`, `JOINFAILED`, `MEMBER_ADDED`, `IN_PROGRESS`, `COMPLETE` using
  `wwm:` types.
- `stdb/subscriptions.rs` `group_event_outbound`: arms for kinds 27-30. A bad payload logs a warning
  and sends nothing.

## Acceptance criteria

1. An imported world has one `game_meeting_stone` row per spawned type-23 template, carrying the
   dump's `data0`, `data1`, `data2`.
2. `CMSG_GAMEOBJECT_QUERY` for a stone answers `raw_data[0..3] = [min, max, area]`. Other GO types
   answer byte-identically to today.
3. A lone Character in range and level range who joins gets exactly one `SETQUEUE(area, JOINED)`
   and one Seeker row on the authority. Joining again at another stone replaces the row.
4. Out of range, another map or instance, wrong GO type, outside the level range, taxi flight, or a
   tokenless actor: no durable change, no packet.
5. A party member who is not the leader gets `JOINFAILED(1)`. The leader of a Raid gets
   `JOINFAILED(3)`. The leader of a full Party gets `JOINFAILED(2)`. None changes durable state.
6. A Party leader's join queues the party: one party row, one Seeker row per member with the class
   the Gateway supplied, `SETQUEUE(area, JOINED)` to every member on any shard.
7. `LEAVE` sends `LEAVE_QUEUE` to a solo Seeker, `LEAVE_QUEUE` to every member when the leader of a
   queued party leaves, and `NONE` to a non-leader in a party, per the README table.
8. `INFO` answers `SETQUEUE(area, JOINED)` for any Character with a Seeker row, and `SETQUEUE(0, NONE)`
   otherwise, on world entry and after a Transfer.
9. Logout drops the solo Seeker with no packet. A crashed Gateway's solo Seekers are gone after one
   lease reaper pass past their claims' expiry. A new claim on the same Account drops the previous
   Character's solo Seeker. Party Seeker rows survive all three.
10. `MSG_LOOKING_FOR_GROUP` gets `MSG_LOOKING_FOR_GROUP(0)`.
11. On a sharded Gateway, `realm_meeting_stone_op` runs on Realm-core only; the admission runs on
    the Character's Home Shard only. On a single database both run on that database.
12. The Realm-core ACCEPT carries the acceptor's class and race, for a client and for a playerbot.
13. A Refusal never ends the World Session. Transport loss does.

## Verification rungs

- Unit: shared tags, `ALL`, kind distinctness, payload round trip and fail-closed decode; the level
  range rule with inclusive bounds and `max_level = 0`; the JOIN classification given membership
  and kind, in the core's order; importer stone rows from a fixture dump, including an unspawned
  stone that yields none.
- Codec: `SETQUEUE` bytes equal `u32 area, u8 status` (`cm:LFG/LFGHandler.cpp:146-152`); the
  status and failure bytes equal `wwm:` `MeetingStoneStatus::as_int` and
  `MeetingStoneFailure::as_int`; the stone query response.
- Gateway Store Fake: an in-file `mod tests` in `world/handlers/meeting_stone.rs` with a Fake
  `MeetingStoneActionStore`, the `channel.rs` shape. One test per routing branch: admission refused
  stops before Realm-core; each join failure byte; unknown `Area` stops before Realm-core; party
  facts come from every member across shards; leave; `INFO` both answers; `MSG_LOOKING_FOR_GROUP`;
  unsharded runs everything on the one store. ACCEPT class and race go in `party_tests.rs`. Relay
  arms for kinds 27-30 go beside the existing `group_event_outbound` tests.
- Durable Module: `module/tests/meeting_stone_queue.rs` with `support::Standalone`, the
  `auction_buyout.rs` shape. Stage through the debug fixtures and drive the real reducers. Cover
  criteria 3 to 7 and 9 against `game_meeting_stone_seeker`, `game_meeting_stone_party` and
  `game_group_event` rows. CI picks the new target up by itself.
- `lyracore preflight` after the schema change.

## CONTEXT.md terms

Add a `### Meeting stones` section:

- **Meeting Stone**: a type-23 GameObject at a dungeon entrance. Using it puts the Character, or
  the Party it leads, in the Meeting Stone Queue for the stone's dungeon area, inside the stone's
  level range. _Avoid_: summoning stone, LFG tool.
- **Meeting Stone Queue**: Realm-core's realm-wide list of Seekers and queued Parties, per dungeon
  area and team. _Avoid_: LFG queue, matchmaking queue.
- **Seeker**: one Character in the Meeting Stone Queue, alone or as a member of a queued Party. It
  carries the class and team the Gateway supplied. A solo Seeker lasts as long as its Account
  Claim. _Avoid_: queued player, LFG player.

## File ownership

Everything this ticket touches. T2 and T3 start from its merged branch.

## Non-goals

- No matching, no stone adds, no reminders, no group-core hooks, no raid-convert dequeue. T3 owns
  them.
- No mirror push for parties the actor is not in. T2 owns it.
- No summoning, no LFG window, no channel change, no playerbot queueing, no Package Delta change.

## Definition of done

Touched files formatted. `cargo clippy` and `cargo test` clean for `lyracore-shared`,
`lyracore-importer`, `lyracore-module` (plus the wasm check) and `lyracore-gateway`. Binding check
clean. Preflight clean. The durable test passes locally. Commit on the feature branch and report
the exact commands, the fixture reducer names and anything T3 must know.
