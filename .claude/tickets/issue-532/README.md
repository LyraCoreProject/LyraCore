# Issue #532 meeting stones: the 1.12.1 dungeon queue on Realm-core, split into tickets

Issue #532 schedules this work. The maintainer deferred it from the issue #3 push on 2026-09-23 and
filed #532 to carry it.

Closed spec #2 set the partition rule: state not coupled to space has one authoritative copy on
Realm-core, and party membership is authoritative there.

## Settled maintainer decisions (#532)

1. **Meeting stones only.** 1.12.1 has no LFG/LFM browse window, so none is built. No Package
   addon window either.
2. **Humans only.** A Seeker needs a World Session and a live Account Claim. Playerbots do not
   queue; a leader invites them into a stone party by ordinary invite, and the party counts their
   class like any member's.

## Sources and citation keys

| Key | Source | Pin |
|---|---|---|
| `cm:` | cmangos mangos-classic `src/game/` | `ca0775fe352fb67a0e82dc6051f0563187d569a5` |
| `vm:` | vmangos core `src/game/` | `4b350a09fca8b5797975e343ae6300fbb5f9937b` |
| `fx:` | stock 1.12 FrameXML (`## Interface: 11200`), `AtheneGenesis/Vanilla_enUS_FrameXML` | `64ba0a1fa7cf42679284b608562bde094bbee75f` |
| `wwm:` | `wow_world_messages` 0.3.0 and `wow_world_base` 0.3.0 | cargo registry |

cmangos LFG code is `LFG/{LFGHandler,LFGMgr,LFGQueue}.cpp` and `LFG/LFGDefines.h`. vmangos has the
same four files. The two queue implementations are the same algorithm line for line; vmangos only
swaps packet builders and sleeps 1 s instead of 500 ms (`vm:LFG/LFGQueue.cpp:66-224`,
`cm:LFG/LFGQueue.cpp:63-225`).

## What 1.12.1 ships

**No LFG/LFM window.** The 1.12 FrameXML has no LFG frame. The only LFG widget, a "I would like to
join a group" checkbox on the Who tab that calls `SetLookingForGroup`, sits inside an XML comment
(`fx:FriendsFrame.xml:1212-1301`, string `LFG_LABEL` at `fx:GlobalStrings.lua:2581`). Neither core
handles `CMSG_SET_LOOKING_FOR_GROUP` 0x200 (`cm:Server/Opcodes.cpp:540` `STATUS_NEVER`,
`vm:Server/Protocol/Opcodes.cpp:603` unhandled), and `wwm:` has no vanilla parser for it. The
LFG/LFM tool with dungeon, zone and quest entries, comments and browse results is the TBC tool:
`CMSG_SET_LFG_COMMENT` 0x366 and `SMSG_LFG_DISABLED` exist in `wwm:` only as `_tbc_wrath` types.

`MSG_LOOKING_FOR_GROUP` 0x1FF exists. The client message is empty, the server message is one `u32`.
vmangos answers it with `0` (`vm:Handlers/MiscHandler.cpp:277-282`); cmangos ignores it
(`cm:Server/Opcodes.cpp:539`). The stock UI never reads the answer.

**Meeting stones with a server queue.** This is the whole 1.12 LFG feature.

| Fact | Source |
|---|---|
| A meeting stone is GameObject type 23. `data0` min level, `data1` max level, `data2` AreaTable id of the dungeon | `cm:Entities/GameObject.h:266-272`, `vm:Objects/GameObjectDefines.h:437-443` |
| The client sends `CMSG_MEETINGSTONE_JOIN(go guid)` on use, not `CMSG_GAMEOBJ_USE`. vmangos says so and returns at once from `Use` for type 23. T4's client check confirms it | `vm:Objects/GameObject.cpp:1836-1841` |
| Join gates: interactable GO of type 23, else silent. In a party, in this order: not leader → `JOINFAILED(1)`, raid → `JOINFAILED(3)`, full → `JOINFAILED(2)`. No level check on the server | `cm:LFG/LFGHandler.cpp:34-84`, `vm:LFG/LFGHandler.cpp:31-75` |
| Queue key is dungeon area and team. A leader queues the whole party; anyone else queues alone. Re-joining overwrites the area and the wait time | `cm:LFG/LFGMgr.cpp:37-89`, `cm:LFG/LFGQueue.cpp:252-277` |
| Status bytes: 0 LEAVE_QUEUE, 1 JOINED_QUEUE, 2 PARTY_MEMBER_LEFT_LFG, 3 PARTY_MEMBER_REMOVED_PARTY_REMOVED, 4 LOOKING_FOR_NEW_PARTY_IN_QUEUE, 5 NONE. Failure bytes 1 PARTYLEADER, 2 FULL_GROUP, 3 RAID_GROUP | `cm:LFG/LFGDefines.h:50-67`, `wwm:` `MeetingStoneStatus`, `MeetingStoneFailure` |
| `SMSG_MEETINGSTONE_SETQUEUE` is `u32 area, u8 status`. `JOINFAILED` is `u8`. `MEMBER_ADDED` is a guid. `COMPLETE` and `IN_PROGRESS` are empty | `cm:LFG/LFGHandler.cpp:139-152`, `cm:LFG/LFGMgr.cpp:288-309` |
| Leave: alone → `SETQUEUE(0, LEAVE_QUEUE)`. Leader of a queued party → the party leaves, every member gets `SETQUEUE(0, LEAVE_QUEUE)`. Anyone else in a party → `SETQUEUE(0, NONE)` to self | `cm:LFG/LFGHandler.cpp:86-110`, `cm:LFG/LFGQueue.cpp:392-454` |
| `CMSG_MEETINGSTONE_INFO` arrives at world entry. Queued party → `SETQUEUE(area, JOINED)`, else `SETQUEUE(0, NONE)` | `cm:LFG/LFGHandler.cpp:112-137`, `wwm:` `CMSG_MEETINGSTONE_INFO` doc |
| Roles by class: Druid and Paladin T/H/D, Priest and Shaman H/D, Warrior T/D, Hunter, Mage, Rogue, Warlock D. A party has 1 tank, 1 healer, 3 damage roles | `cm:LFG/LFGMgr.cpp:91-106`, `cm:LFG/LFGMgr.h:51` |
| A queued party's Open Roles come from its members' classes, with a class priority table breaking ties | `cm:Groups/Group.cpp:1589-1685`, `cm:LFG/LFGMgr.cpp:108-158` |
| Fill: a solo Seeker of the same area and team takes an Open Role. The priority checks compare `bool`s, so in practice the longest-waiting capable Seeker wins each role | `cm:LFG/LFGQueue.cpp:99-171,292-390` (the `bool` at 300 and 316) |
| Formation: with 5 or more solo Seekers, the first becomes leader and one more joins; the fill pass adds the rest | `cm:LFG/LFGQueue.cpp:173-221` |
| Stone adds: existing members get `MEMBER_ADDED(guid)` before the add. At 5 members every member gets `COMPLETE` then `SETQUEUE(0, NONE)` | `cm:LFG/LFGQueue.cpp:372-383,420-454` |
| A queued party gets `IN_PROGRESS` every 5 minutes | `cm:LFG/LFGMgr.cpp:55`, `cm:LFG/LFGQueue.cpp:149-163` |
| Member leaves a queued party: leaver `SETQUEUE(0, NONE)`; if the leader stayed, the rest get `SETQUEUE(area, PARTY_MEMBER_LEFT_LFG)` and the party stays queued; if the leader left, the party leaves the queue with `LEAVE_QUEUE` | `cm:Groups/Group.cpp:437-447,464-479`, `vm:Group/Group.cpp:477-518` |
| Kick from a queued party: the rest get `SETQUEUE(0, PARTY_MEMBER_REMOVED_PARTY_REMOVED)` and the party leaves the queue. The kicked Character gets `SETQUEUE(area, LOOKING_FOR_NEW_PARTY_IN_QUEUE)` and is queued alone | `cm:Groups/Group.cpp:413-435`, `vm:Group/Group.cpp:449-474` |
| Disband: every member `SETQUEUE(0, NONE)`, party leaves the queue | `cm:Groups/Group.cpp:550-562`, `vm:Group/Group.cpp:586-602` |
| A manual invite into a queued party recomputes Open Roles; reaching 5 completes it | `cm:Groups/Group.cpp:376-382`, `cm:LFG/LFGQueue.cpp:237-250` |
| Converting to a raid and passing the lead touch no LFG state in either core | `cm:Groups/Group.cpp:208-222`, `vm:Group/Group.cpp:236-257` |
| Logout drops a solo Seeker silently | `cm:Server/WorldSession.cpp:662-666`, `vm:Server/WorldSession.cpp:721-725` |
| Client UI: a minimap button while queued, click → confirm → `CancelMeetingStoneRequest()` → `CMSG_MEETINGSTONE_LEAVE`. The client renders every status as its own `ERR_MEETING_STONE_*` line | `fx:Minimap.xml:175-235`, `fx:StaticPopup.lua:170-179`, `fx:GlobalStrings.lua:847,1698-1710,2678-2679,4354` |
| The queue exists from patch 1.3 | `vm:World.cpp:1882-1885` |

**Not 1.12.** cmangos `GameObject::Use` casts Meeting Stone Summon 23598 for type 23
(`cm:Entities/GameObject.cpp:1925-1954`). That is the 2.0 summoning stone, shared code in the
classic core. vmangos returns (`vm:Objects/GameObject.cpp:1836-1841`) and the 1.12 FrameXML has no
summon UI. The talent-based roles behind `LFG.Matchmaking` default off
(`cm:World/World.cpp:740-741`) and are a cmangos addition.

**No coupling to the LookingForGroup channel.** It is a Chat Channel on Realm-core now
(`lyracore_shared::channel`, prefix `LookingForGroup`), with the 1.12.1 dbc flags 0x0. No LFG code
in either core joins, leaves or restricts it: `Channel.RestrictedLfg` is read by nothing
(`cm:World/World.cpp:678`), and `Player::LeaveLFGChannel` has no caller
(`cm:Entities/Player.cpp:4796-4806`). Nothing here touches channels.

### Where the cores disagree or are buggy

| Behavior | cmangos | vmangos | We do |
|---|---|---|---|
| `MSG_LOOKING_FOR_GROUP` | ignored | replies `u32 0` | vmangos. It targets 1.12 and the reply is harmless |
| Solo Seeker accepts a manual invite | dropped from the queue, `LEAVE_QUEUE` unless the party is queued for the same area, playerbots builds only (`cm:Groups/Group.cpp:360-374`) | stays in the solo queue while grouped | cmangos. A grouped Character in the solo queue can be matched into a second party |
| `INFO` for a solo Seeker | `NONE`, because `m_offlinePlayers` is never written | same | the real state. Both cores read a map nothing fills |
| Formation trigger | 5 solos across all areas, then 4 more in the first Seeker's area; head-of-line blocks | same | 5 solos in one area and team |
| Leader of a new party | lowest guid | same | longest wait |
| A queued party converts to a raid | stays queued and keeps filling | same | leaves the queue with `LEAVE_QUEUE`. JOIN refuses raids, so a queued raid is the state the join gate exists to prevent |

## State of the world (verified on `main` at `da11dcfc`, 2026-10-02)

- Nothing meeting-stone related exists. `grep -rliE 'lfg|meetingstone|meeting_stone'` over
  `gateway/ module/ importer/ crates/` finds a chat string in
  `gateway/src/stdb/world_view/relay_bench.rs`, the LookingForGroup channel flags in
  `lyracore_shared::channel`, and the reservation comments in `lyracore_shared::group`.
- `lyracore_shared::constants::go_type` has `QUESTGIVER = 2` and `CHEST = 3` only
  (`constants.rs:105-113`).
- The importer imports every spawned GO type. Type 23 falls into the `other` arm of
  `go_template_row` (`importer/src/main.rs:703-706`), which writes `data0 = data1 = 0`. The stone's
  level range and area are lost. `got` names `DATA0`, `DATA1`, `DATA3` and `DATA5`, not `DATA2`
  (`main.rs:874-894`). The per-type side table precedent is `game_gameobject_trap`
  (`module/src/gameobject.rs:117-127`): the importer captures its fields into `GoMeta`
  (`main.rs:4940-4964`), emits rows in `gameobject_template_rows` (`main.rs:5083-5088`), clears
  and reloads them with the `gameobjects` family (`main.rs:5549-5559`), and counts them in that
  family's provenance stamp (`main.rs:5765-5770`). The trap table also rides the Package Delta
  `gameobjects` Import Family (`crates/lyracore-package-delta/src/schema.rs`,
  `module/src/package_import/gameobjects.rs`).
- `use_gameobject` treats unknown types as a benign no-op (`module/src/gameobject.rs:980-983`), so
  a type-23 `CMSG_GAMEOBJ_USE` does nothing today. The shared GO Gate is the private `usable_go`
  (`gameobject.rs:672-715`): GO present, live Character, same map and instance,
  `USE_RANGE_SQ = LOOT_RANGE_SQ = 100` (10 yd), template present. Its `ActionRefusalKind`s are
  `MissingTarget`, `MissingActor`, `OtherPartition` and `OutOfRange`. `taxi::is_in_flight`
  (`module/src/taxi.rs:789`) answers the flight check.
- `SMSG_GAMEOBJECT_QUERY_RESPONSE` sends `raw_data: [data0, data1, 0, 0, 0, 0]`
  (`gateway/src/codec/gameobject.rs:130-151`), read through `GameObjectTemplateView`
  (`codec/gameobject.rs:44-50`) and `stdb/reads/templates.rs:34-52`.
- Raids landed. `Group.group_type` holds a `GroupKind` byte (`module/src/group.rs:94-96`);
  `raid_convert_on` (`group.rs:1673-1683`) flips it and has no queue hook. `has_room` follows the
  kind (`group.rs:610-612`), and `accept_invite_on` (`group.rs:1400-1517`) assigns a `RaidSlot` and
  reconciles Instance Removal for the acceptor and a new party's leader. `remove_member`
  (`group.rs:1845-1918`) revokes Loot Tag membership, resolves rolls on disband and reconciles
  Instance Removal. `leave_group_on(.., cause)` and `uninvite_on` both end in `remove_member`.
  `SET_LEADER` exists as `set_leader_on`.
- Party authority: `realm_group_op(op, request_actor: SessionActor, target_guid, arg_a, arg_b,
  arg_c)` (`group.rs:1993-2062`) runs the group cores with `Plane::RealmCore` and advances
  `game_group_roster_revision` through `advance_group_revision` (`group.rs:2102-2116`) for every
  group `realm_op_groups` names before and after the op. Realm ops 0-15 are taken
  (`lyracore_shared::group::realm_op`). ACCEPT's `arg_a`/`arg_b` are documented as kept free for
  meeting stones (`crates/lyracore-shared/src/group.rs:239-240`).
- Plane: a member row's `owner_identity` is the local Character's identity on `Plane::Shard` and
  zero on `Plane::RealmCore` (`group.rs:1490-1502`). On a single-database realm the Gateway calls
  `gw_group_*` reducers (`Plane::Shard`) for membership ops.
- Gateway party flow: `party::run` (`gateway/src/world/party.rs:912-1009`) calls Realm-core through
  `run_on_authority_visible`, then pushes mirrors. Accept, leave and kick use
  `sync_membership_mirrors` (`party.rs:1016-1039`), which goes through
  `sync_group_mirrors_required` (`party.rs:1380-1428`, three attempts within a 5 s window per
  shard). Other ops use the best-effort `sync_mirrors` (`party.rs:1435-1495`). Both push only the
  actor's before and after groups, to every World Shard. `Op::realm_args` (`party.rs:403`) packs
  ACCEPT as `(ACCEPT, 0, 0, 0, 0)`. A playerbot's automatic accept is `answer_for_session_less`
  (`party.rs:873-905`).
- **No Roster Revision Relay exists.** Nothing watches `game_group_roster_revision`. Mirrors move
  only through `party::run`, world entry (`on_world_entry`, `party.rs:1505`), deleted-Character
  cleanup, and `reconcile_deleted_character_parties` (`party.rs:1327-1371`), which pushes every
  party known to Realm-core or any shard. That pass already runs at startup and on every Realm-core
  reconnect (`spawn_character_gone_relay`, `gateway/src/stdb/subscriptions.rs:4223-4252`). The
  Coordinator caches `game_group_roster_revision` on every connection of a sharded realm
  (`stdb/connection.rs`, `sharded_tables` block), but `reads/party.rs:207-216`
  `group_roster_revision` answers 1 for a missing row, so it cannot tell "no mirror" from
  revision 1.
- `game_group_event` is the per-recipient relay. The Gateway routes a row by `recipient_guid`
  (`gateway/src/stdb/world_view.rs:2728-2747` `group_event_appeared`, wired at `world_view.rs:1098`
  for a shard and `:1275` for Realm-core) and decodes it in `group_event_outbound`
  (`subscriptions.rs:2193`). Kinds: 0-3 membership, 4-8 loot rolls, 9 retired (party chat is a
  Realm Chat Line now), 10-11 quest sharing, 12-19 unassigned, 20 leader announcement, 21-26 Group
  Broadcasts, **27-30 reserved for meeting stones** (`crates/lyracore-shared/src/group.rs:173-182`).
  No test covers every kind at once; `quest.rs:247` covers 0-11.
- Account Claims live on Realm-core, one per Account (`module/src/account_ownership.rs:27-35`),
  60 s without renewal (`CLAIM_MICROS`). A claim ends in three places, and each one already calls
  `channel::leave_all` for its Character: `claim_account` replacing a dead generation
  (`account_ownership.rs:186-190`), `release_account_claim` (`:224-239`), and
  `reap_account_claims` (`:549-566`), which the Gateway lease reaper runs every 15 s
  (`module/src/gw.rs:118-127`, `LEASE_REAP_MICROS`). `require_actor` (`:475-524`) accepts a
  tokenless actor whose Character has no live claim, so JOIN must refuse a tokenless actor itself.
- Chat Channel membership is the precedent for Realm-core state tied to a claim: its tables sit in
  the `NOT_CHARACTER_OWNED` list of `module/src/tripwires.rs` ("membership ends with the Account
  Claim that admitted it"), not behind `character_owned!` markers.
- The Gateway's newer protocol families use a dispatch seam in `gateway/src/world/handlers/`:
  `dispatch_channel_action` with a narrow `ChannelActionStore` trait implemented for `Coordinator`
  and a Fake, wired into the chain in `gateway/src/world/mod.rs:1559`. The Coordinator picks the
  database; handlers never do. Party ops still use `world/party.rs` and `world/social.rs`.
- Realm-core tables the Gateway reads sit in the base list of `coordinator_queries`
  (`stdb/connection.rs:1011`), for example the Chat Channel tables at `connection.rs:1209-1212`.
- An opcode `wwm:` cannot parse ends the World Session (`gateway/src/world/framing_tests.rs:199`).
  The three meeting-stone client messages and `MSG_LOOKING_FOR_GROUP` all parse.
- `lyracore_shared::faction::team_for_race` maps race to team (469 Alliance, 67 Horde).
- Fixture ids: `docs/danger-zones.md` reserves `509_0000`-`509_9999`. The auction, mail and
  creature-cycle fixtures use `509_00xx`-`509_05xx` and `509_36xx`. `509_6000`-`509_6099` is free.
- Bindings: `docs/danger-zones.md` §1 item 2 holds the regeneration command
  (`spacetime generate --lang rust --out-dir gateway/src/stdb/bindings --module-path module
  --include-private --build-options='--features=debug_reducers' -y`), its hand-patches, and
  `scripts/check-gateway-bindings.py`.
- CI: `.github/workflows/module-durable.yml` discovers every `module/tests/*.rs` target by itself,
  but runs Gateway durable tests by exact name. A new Gateway durable test needs a workflow edit,
  and the agents' token cannot push `.github/workflows/*`.

## Design

### Placement

| Question | Answer | Why |
|---|---|---|
| Is the Character at a stone, in range, in its level range? | Module on the Home Shard, `gw_admit_meeting_stone` | Only that shard holds the GO and the Character |
| Queue state and matching | Module on Realm-core, `realm_meeting_stone_op` | Seekers on different shards meet in one queue. Matching changes party membership, which lives there |
| Party formation | the group cores in `module/src/group.rs`, on Realm-core | One group path |
| Class and race of Seekers | Gateway reads them from the shard caches and passes them in | Realm-core has no Character rows. Same shape as `invite_gate` and the channel `SpeakerFacts` |
| Client packets | Gateway, `world/handlers/meeting_stone.rs`, a `dispatch_meeting_stone_action` seam | The house shape for a protocol family on Realm-core |
| Notifications | `game_group_event` kinds 27-30, existing relay | One recipient, a kind byte, a small payload |
| Solo Seeker lifetime | the three Account Claim end points | The claim already ends Chat Channel membership there |
| Mirrors of stone-formed parties | Gateway Roster Revision Relay | The matcher changes parties the acting Character is not in |

On a single-database realm every call goes to the one database, as `run_bot_invite` already does.

### Durable state, all in `module/src/meeting_stone.rs`

- `game_meeting_stone` [static, private]: `entry` (template) PK, `min_level`, `max_level`,
  `area_id`. Imported with the `gameobjects` family. `max_level = 0` means no upper bound. The
  Gateway reads it with the owner token, like `game_mail_escrow`.
- `game_meeting_stone_seeker` [entity, private]: one row per queued Character. `character_guid`
  PK, `area_id`, `team`, `class`, `group_id` (0 = alone), `queued_at`. Index on
  `(area_id, team)` and on `group_id`. The Gateway subscribes it to answer `INFO`.
- `game_meeting_stone_party` [entity, private]: one row per queued party. `group_id` PK,
  `area_id`, `team`, `queued_at`, `next_reminder_at`. Module only.
- `game_meeting_stone_reminder_schedule` [scheduled], every 5 s, added by T3: sends `IN_PROGRESS`
  reminders. Nothing else needs a tick.

Invariant: a party row exists if and only if the party is queued, and its Seeker rows are exactly
its current members. The group cores keep it in the same transaction that changes membership.

### Flows

```text
CMSG_MEETINGSTONE_JOIN(go)
  Gateway: Home Shard gw_admit_meeting_stone(actor, go)   refused → silent
  Gateway: stone area from the Home Shard cache; area not in wwm Area → silent
  Gateway: facts = actor, or every party member: (guid, race, class)
  Gateway: authority realm_meeting_stone_op(JOIN, actor, area, facts)
     refused NotLeader/RaidGroup/PartyFull → SMSG_MEETINGSTONE_JOINFAILED(1/3/2)
  Realm-core, one transaction: enqueue, SETQUEUE(area, JOINED) events, match the bucket,
     form or fill parties through the group cores, advance Roster Revisions
  Gateway: relay events; Roster Revision Relay pushes changed parties to World Shards

CMSG_MEETINGSTONE_LEAVE → realm_meeting_stone_op(LEAVE) → events
CMSG_MEETINGSTONE_INFO  → Durable Read of the Seeker row → SETQUEUE(area, JOINED) or (0, NONE)
MSG_LOOKING_FOR_GROUP   → MSG_LOOKING_FOR_GROUP(0)
CMSG_GROUP_ACCEPT       → realm_group_op(ACCEPT, arg_a = class, arg_b = race)
Account Claim ends      → the claim's Character loses its solo Seeker row, no packet
```

| `lyracore_shared::meeting_stone::event_kind` | Value | SMSG | Payload |
|---|---:|---|---|
| `QUEUE` | 27 | `SETQUEUE` | `lyracore_shared::meeting_stone` grammar for area and status |
| `MEMBER_ADDED` | 28 | `MEMBER_ADDED` | `other_guid` = added Character |
| `IN_PROGRESS` | 29 | `IN_PROGRESS` | none |
| `COMPLETE` | 30 | `COMPLETE` | none |

## Decisions

1. Ship meeting stones only. 1.12.1 has no LFG/LFM window (`fx:FriendsFrame.xml:1212-1301`).
   Maintainer decision in #532.
2. Answer `MSG_LOOKING_FOR_GROUP` with `0`, as vmangos does. Leave 0x200 unparsed; the stock UI
   never sends it.
3. No summoning. 23598 on use is 2.0 behavior.
4. Queue on Realm-core, one Seeker row per Character plus one row per queued party. INFO is then
   one indexed read.
5. Match inside the transaction that changes a bucket, not on a 1 s tick. The core's priority
   checks reduce to longest wait per role, so a tick adds latency and no behavior.
6. Class roles and 1/1/3 roles from cmangos. No talent roles; `LFG.Matchmaking` defaults off.
7. Form a party when one area and team holds 5 solo Seekers. Leader is the longest wait. This
   removes the cores' head-of-line bug.
8. Joining any party drops the solo Seeker, with cmangos's notice rule.
9. `INFO` reports the real state. The cores' restore map is dead code.
10. A solo Seeker lives as long as its Account Claim. The three places that end a claim and call
    `channel::leave_all` also drop the solo Seeker, through one shared call. A Transfer keeps the
    claim, so the Seeker survives crossing shards. No claim sweep tick: the lease reaper already
    closes an expired claim within 15 s.
11. Humans only. JOIN refuses a tokenless actor. Maintainer decision in #532.
12. The Module enforces the stone's level range for the using Character, silently. Neither core
    does, the client pre-checks with `ERR_MEETING_STONE_INVALID_LEVEL`, and 1.12 has no failure
    code for it.
13. Reuse `usable_go` for range and partition. One GO interaction Gate, 10 yd, not the cores' 5 yd.
14. Stone facts in a new `game_meeting_stone` table, the trap precedent. A new template column
    would ripple through the Package Delta schema, every template literal and the Datascript
    typings.
15. The query response for type 23 fills `raw_data[0..3]` from the stone row so the client tooltip
    shows the level range.
16. Notifications reuse `game_group_event`, kinds 27-30. Verified free on `da11dcfc`;
    `lyracore_shared::group` reserves them. `JOINFAILED` goes back synchronously from the Refusal.
17. Realm-core learns class and race from the Gateway: JOIN facts, and ACCEPT's reserved
    `arg_a`/`arg_b`. An unknown class holds a seat and no role.
18. A Roster Revision Relay pushes any Realm-core roster change to stale World Shard mirrors. The
    matcher changes parties the actor is not in, so `party::run`'s actor-scoped push misses them.
    Main has no such relay, so T2 stays. It shrinks: it reuses `sync_group_mirrors_required`'s
    retries, and the existing Realm-core reconnect pass already covers changes missed while
    disconnected.
19. Only the Gateway can say whether a stone's area encodes. If `wwm:` `Area::try_from` refuses it,
    the Gateway drops the JOIN before any state changes.
20. `IN_PROGRESS` every 5 minutes from a 5 s Realm-core reminder tick (T3).
21. Raids exist, so T1 refuses a raid at JOIN with `JOINFAILED(3)` in the core's order: not
    leader, raid, full. No marker is left for later.
22. A queued party that converts to a raid leaves the queue with `SETQUEUE(0, LEAVE_QUEUE)` to every
    member. Both cores leave it queued, but they refuse a raid at JOIN, and a queued raid would
    keep taking stone adds past the 1/1/3 role model. T3 calls `dequeue_party` from
    `raid_convert_on`.
23. Passing the lead keeps the party queued with no event. Both cores do this, and the party row
    is keyed by `group_id`, so nothing changes.
24. The Seeker table goes in `tripwires.rs`'s `NOT_CHARACTER_OWNED` list, the Chat Channel
    precedent. A solo Seeker ends with its claim. A party Seeker ends with its membership, and a
    deleted Character leaves its party through `remove_member` on both planes, where T3's hook
    drops the row. The party table has no Character guid column, so the census test rejects an
    entry for it as stale.
25. Packages cannot author meeting stones in this work. `game_meeting_stone` does not join the
    Package Delta `gameobjects` Import Family. A Package type-23 template has no stone row and
    is refused as `NotAMeetingStone`. No Package ships a stone today; adding the table to the
    family later is additive.
26. The Gateway side is a `dispatch_meeting_stone_action` seam in `world/handlers/`, with a narrow
    `MeetingStoneActionStore` trait for the Coordinator and a Fake. Party code stays where it is.
27. A stone add writes the member's `owner_identity` the way `push_event` resolves a recipient:
    the local Character row's identity when the database holds one (a single-database realm),
    else zero (Realm-core, where each mirror re-derives it).
28. Debug fixtures use ids `509_6000`-`509_6099`.

## Maintainer judgment

1. **CI for the two new Gateway durable tests (T2, T4).** `module-durable.yml` names each Gateway
   durable test, and the agents' token cannot push a workflow change.
   a. T4 puts the two exact `cargo test ... --ignored --exact` lines in the PR description, and the
      maintainer pushes the workflow edit to the feature branch over SSH before merge.
      *Recommended.* The cross-shard proof is the only test of the realm-wide claim, so it should
      run in CI.
   b. Leave both tests local-only and run them by hand before each release.
   c. Grant the agents' token the `workflow` scope so T4 can push the edit itself.

## Coordination

This section describes merged reality on `da11dcfc`. No sibling ticket set is open against these
files.

- **Raids are on main** (#543 convert, #550 leadership, #551 Subgroup moves, #552 Group
  Broadcasts, #557 quest credit and loot scale, #558 raid chat, #566 Instance Removal timer, #574
  integration). They use realm ops 6-15 and event kinds 20-26, and left ACCEPT's `arg_a`/`arg_b`
  and kinds 27-30 alone. Their accept and removal cores (`RaidSlot` assignment, Instance Removal
  reconciliation, Loot Tag revocation) are what T3's join core extraction must keep intact.
- **Realm Chat is on main** (#539, #542, #546, #555, #558, #565, #568, #575, #576). Party and raid
  chat are Realm Chat Lines on `game_realm_chat_event`; kind 9 is retired and 12-19 are unassigned.
  Chat Channels live on Realm-core and end membership with the Account Claim, which is the hook
  T1 reuses. Nothing here touches channels or chat.
- **Guilds, mail and auctions are on main.** They add Realm-core tables, `realm_*` and `gw_*`
  reducers and Gateway dispatch seams, and share no file region with this work beyond the
  regenerated bindings and the base subscription list.
- **Playerbots tests moved to the Packages repository** (#595). The party-command durable tests and
  the playerbots Headless Client acceptance harness are no longer in Core. The sharded Gateway
  durable shape left in Core is `gateway/src/stdb/subscriptions_character_gone_durable_tests.rs`.
- **Perf work** (#578 playerbots capacity, #587 creature movement writes) does not touch the party,
  group event or GameObject paths used here.

## Execution order

```text
T1  meeting stone queue tracer (alone)
 ├── T2  Roster Revision Relay (Gateway) ──┐
 └── T3  stone matching (Module) ──────────┤  parallel worktrees
                                           └── T4  integrate, verify, PR
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|---|---|---:|---|
| T1 | Queue and leave at a meeting stone, alone or as a party | Opus | ~200k | everything it touches; see ticket |
| T2 | Roster Revision Relay | Opus | ~110k | Gateway only: `world/party_mirror.rs` (new), `sync_group_mirrors_required`'s signature in `world/party.rs`, the mirrored-revision read, the relay region of `stdb/subscriptions.rs`, `main.rs`, a new durable test file |
| T3 | Stone matching and party-queue rules | Opus | ~200k | Module only: `module/src/meeting_stone.rs`, `module/src/group.rs` cores, `module/src/debug/meeting_stone.rs`, `module/tests/meeting_stone_matching.rs` |
| T4 | Integrate, verify cross-shard, docs, PR | Opus | ~150k | union, docs, verification doc, PR |

T2 touches no Module file and no bindings. T3 touches no Gateway file except regenerated bindings.

## Shared rules

- Read `AGENTS.md`, `CODING_STANDARDS.md`, `CONTEXT.md`, `docs/architecture.md` §2.3 and §5.3,
  `docs/danger-zones.md` §1, and this file before editing.
- Apply `.claude/skills/unslop/SKILL.md` to prose. No issue numbers in code comments. No em dashes.
- Do not touch a production or development realm. Live wire runs are operator-gated.
- Prefix every cargo command with `nice -n 10 env CARGO_BUILD_JOBS=4`. One cargo command at a time.
- Schema: new tables only, END-append with typed `#[default(...)]` if a column is ever added. Never
  drop a table. A new table or reducer means regenerating bindings with the `danger-zones.md` §1
  item 2 command and its hand-patches, then `scripts/check-gateway-bindings.py`. Never hand-merge
  generated bindings; regenerate from the merged module.
- A new reducer argument changes the module reducer, the Gateway call and every durable test that
  calls it, in one commit.
- Wire values that cross crates live in `lyracore-shared` and are pinned by tests.
- A Refusal never ends a session. Transport loss and untagged errors stay `Err`.
- Stay inside your file ownership. Report a cross-ticket gap instead of editing a sibling's region.
- Format touched files individually. Run `cargo clippy` and `cargo test` for each touched crate, the
  wasm check for Module changes, and `lyracore preflight` after schema changes. Before excusing a
  failure, prove it fails on the unmodified base.
- Commit unsigned (`git -c commit.gpgsign=false commit`) on your worktree branch with a
  conventional-commit message. Do not push, open a PR or comment on GitHub unless the ticket
  assigns it. Return the branch, exact test commands and results, and handoff notes.
