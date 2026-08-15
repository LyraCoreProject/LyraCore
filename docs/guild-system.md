# Guild system

Spec for [issue #15](https://github.com/LyraCoreProject/LyraCore/issues/15). The issue is a parity-audit
stub with a "slice on pickup" note. This document is that slicing.

## The problem

There is no guild system. Today the gap shows up as four separate silent holes:

| Site | Current behaviour |
| --- | --- |
| `gateway/src/codec/char.rs:124` | `SMSG_CHAR_ENUM` sends `guild_id: 0` for every character |
| `gateway/src/codec/social.rs:37` | `SMSG_WHO` sends an empty guild name for every row |
| `gateway/src/world/handlers/query.rs:361` | `CMSG_MESSAGECHAT` with `ChatType::Guild` is dropped, no reply |
| every `CMSG_GUILD_*` opcode | never dispatched |

The client renders all four as "no guild", so nothing looks broken. It just is not there.

## Decisions

### D1: guild state is authoritative on realm-core, with no world-shard mirror

Guild rows are not coupled to space, so the partition rule from the sharding design puts them on
realm-core, next to `game_group` (#22), whisper (#54 era) and mail. Building them shard-local first
would reproduce the exact bug the group slice had to unwind: a member inside Deadmines could not be
invited by, or chat with, a member standing in Elwynn.

Guilds differ from party in one way that removes most of the cost. Party needed a write-through shard
mirror (`sync_group_mirror`) because roughly fifty in-world reads resolve party membership on the hot
path: kill-XP split, quest credit, loot rules, dungeon binding. **Guild membership has no hot-path
in-world read at all.** Every guild read is cold:

- create / invite / accept / leave / kick / roster / MOTD: request-response, once per player action.
- guild chat: a relay, same shape as whisper, which already runs entirely on realm-core.
- the character's own guild id and rank in the player unit fields: two scalars, not a roster.

So there is no mirror table. The two scalars ride a `guild_id` / `guild_rank` pair the gateway pushes
onto the shard's `game_character` row at world entry and on every membership change, which is the
`world::party::on_world_entry` push minus the roster. This is the single largest simplification in
the spec and it should be defended in review: if a later slice adds a hot-path guild read (guild
perks, guild bank withdrawals during combat), that slice pays for the mirror, not this one.

### D2: charter-less creation. No petitions.

Vanilla gates guild creation behind a charter: buy a petition item from a civilian vendor, collect
nine signatures, turn it in. That is its own subsystem (petition item class, `CMSG_PETITION_*`
opcodes, signature rows, refund-on-expiry) and it is orthogonal to every acceptance criterion in the
issue. The MVP handles `CMSG_GUILD_CREATE` directly and creates the guild for the caller. Petitions
are deferred, tracked as a follow-up issue, and the signature count is enforced nowhere.

### D3: ranks exist as data in slice 1, but grant no permissions

The wire cannot avoid ranks. `SMSG_GUILD_QUERY_RESPONSE` carries a fixed `[String; 10]` of rank
names, and `GuildMember.rank` is a required `u32`. So the tables carry rank rows and the packets
carry real values from day one. What defers is **enforcement**: in slices 1 to 3 the only permission
check is "are you the guild master", which every op that needs gating uses. The rights bitmask in
`SMSG_GUILD_ROSTER` ships as the vanilla defaults per rank index and is not consulted server-side.

Rationale: the issue explicitly permits deferring rank permissions, and a rights system that is
written but not enforced is worse than one that is visibly absent, because it reads as working.

### D4: guild chat reuses the group-event relay shape, not a new transport

Party chat (`/p`) is a `GroupEvent` row with `kind = PARTY_CHAT` and the message in `payload`,
RLS-scoped to the recipient. Guild chat is the same construction against a `GuildEvent` table, with
the sender's line echoed back to the sender exactly as party does. Guild chat delivers to **online
members only**, which is the issue's wording and also vanilla: there is no offline queue and no
replay on login.

`/o` (officer chat) is out of scope for slice 1. It is the same relay with a rank filter and lands
with rank enforcement.

## Language

Additions for `CONTEXT.md`, under a new `### Guilds` heading:

**Guild**: A realm-wide, persistent roster of characters with a name unique across the realm, a guild
master, ten ranks, and a message of the day. Authoritative on realm-core, never mirrored to a world
shard. _Avoid_: clan, order.

**Guild Master**: The single member at rank 0. Only the guild master may invite, kick, promote,
demote, set the MOTD, or disband. Leadership transfers explicitly, never automatically. _Avoid_: GM
(that is a game master), leader, owner.

**Guild Rank**: One of ten ordered slots per guild, index 0 (guild master) to 9. A rank has a name
and a rights bitmask. In the first slices the bitmask is transmitted and ignored. _Avoid_: role,
permission level.

**Guild Invite**: A pending offer from the guild master to one character, at most one per target, TTL
two minutes, consumed by accept or decline. Same lifecycle as a party invite. _Avoid_: application,
petition (a petition is the deferred charter item).

**Guild Chat**: Non-proximity chat scoped to a guild's currently online members. No offline delivery,
no history. _Avoid_: guild channel (a channel is the `game_channel_*` system).

## Schema

New tables in a new `module/src/guild.rs`. All are realm-core authoritative under D1.

```
Guild            guild_id (pk, auto_inc), name (unique), master_guid,
                 motd, info_text, created_at
GuildMember      id (pk, auto_inc), guild_id, character_guid (unique across table),
                 rank_index, public_note, officer_note, joined_at
                 index by_guild, by_character
GuildRank        id (pk, auto_inc), guild_id, rank_index, name, rights
                 index by_guild
GuildInvite      id (pk, auto_inc), target_guid, inviter_guid, guild_id, created_at
                 index by_target
GuildEvent       id (pk, auto_inc), recipient_identity, recipient_guid, kind,
                 other_guid, other_name, payload, created_at
                 public + RLS on recipient_identity, index by_recipient
```

Constraints that matter:

- `Guild.name` is unique realm-wide. On realm-core that is one database, so a unique index is enough.
- `GuildMember.character_guid` is unique across the table. A character is in at most one guild.
- `GuildRank` rows are created ten-at-a-time at guild creation with the vanilla default names
  (Guild Master, Officer, Veteran, Member, Initiate, then five empty slots). Never fewer than ten,
  because the query response packet is a fixed-size array.

Two columns are added to the existing `game_character`: `guild_id` (`u64`, `#[default(0u64)]`) and
`guild_rank` (`u32`, `#[default(0)]`). These are the D1 scalars. They are END-appended with typed
defaults. `docs/danger-zones.md` §1.2 applies, and the `u64` default must be written `0u64`, not `0`,
or `publish` rejects the migration (`group.rs` records the same trap).

Sweeps, following `group.rs`:

- `character_owned!(delete, ...)` on `GuildMember`: a deleted character leaves through the same path
  a voluntary leave uses, never a bare row delete, so guild-master succession and the last-member
  disband both run.
- `character_owned!(delete, ...)` on `GuildInvite`: rows where the character is target (indexed) and
  rows where it is inviter (scan; the table only holds pending invites).
- `character_owned!(not_transported, ...)` on `GuildMember`, `GuildInvite`, `GuildEvent`. Membership
  is realm-core state and must not ride the transfer blob, for the same reason party membership does
  not: the blob would race the authority.
- `GuildEvent` is reaped by the shared event GC in `gc.rs`. `GuildInvite` gets the same two-minute
  TTL `GroupInvite` has.

## Shared contract

A new `crates/lyracore-shared/src/guild.rs`, sibling to `group.rs`, holding the one definition both
crates import: event kinds, the roster payload grammar, the realm-op tags, and classified error
strings. A renumber or a delimiter change is then a cross-crate compile-visible edit, never a runtime
drift. The roster encode/decode pair mirrors `encode_roster` / `decode_roster`.

## Wire surface

All types exist in `wow_world_messages` 0.3 (vanilla feature) and `wow_world_base` 0.3. No hand-rolled
packets.

| Direction | Message | Opcode | Slice |
| --- | --- | --- | --- |
| C to S | `CMSG_GUILD_CREATE` | 0x081 | 1 |
| C to S | `CMSG_GUILD_INVITE` | 0x082 | 2 |
| C to S | `CMSG_GUILD_ACCEPT` | 0x084 | 2 |
| C to S | `CMSG_GUILD_DECLINE` | 0x085 | 2 |
| C to S | `CMSG_GUILD_INFO` | 0x087 | 1 |
| C to S | `CMSG_GUILD_ROSTER` | 0x089 | 2 |
| C to S | `CMSG_GUILD_LEAVE` | 0x08D | 2 |
| C to S | `CMSG_GUILD_REMOVE` | 0x08E | 2 |
| C to S | `CMSG_GUILD_DISBAND` | 0x08F | 2 |
| C to S | `CMSG_GUILD_LEADER` | 0x090 | 4 |
| C to S | `CMSG_GUILD_MOTD` | 0x091 | 4 |
| C to S | `CMSG_GUILD_INFO_TEXT` | 0x2FC | 4 |
| C to S | `CMSG_GUILD_SET_PUBLIC_NOTE` | 0x234 | 4 |
| C to S | `CMSG_GUILD_SET_OFFICER_NOTE` | 0x235 | 4 |
| C to S | `CMSG_GUILD_QUERY` | 0x054 | 1 |
| C to S | `CMSG_MESSAGECHAT` (`ChatType::Guild`) | 0x095 | 3 |
| S to C | `SMSG_GUILD_QUERY_RESPONSE` | 0x055 | 1 |
| S to C | `SMSG_GUILD_INFO` | 0x088 | 1 |
| S to C | `SMSG_GUILD_INVITE` | 0x083 | 2 |
| S to C | `SMSG_GUILD_ROSTER` | 0x08A | 2 |
| S to C | `SMSG_GUILD_COMMAND_RESULT` | 0x093 | 1 |
| S to C | `SMSG_GUILD_EVENT` | 0x092 | 2 |
| S to C | `SMSG_MESSAGECHAT` (`ChatType::Guild`) | 0x096 | 3 |

Notes on the payloads:

- `SMSG_GUILD_QUERY_RESPONSE.rank_names` is `[String; 10]`, fixed. See the schema note.
- `SMSG_GUILD_ROSTER` carries `motd`, `guild_info`, `rights: Vec<u32>` (one per rank), and
  `members: Vec<GuildMember>`. Each member carries `guid`, `status` (online/offline), `name`, `rank`,
  `level`, `class`, `area`, `public_note`, `officer_note`. Realm-core holds no character rows, so
  `name`, `level`, `class`, `area` and `status` are filled at render time from the shards, the same
  way `world::party::render_list` fills the party roster. This is the single most fiddly part of the
  slice and it is where a naive implementation will hard-code `Area::None`.
- `SMSG_GUILD_COMMAND_RESULT` is the error channel: `command` (`GuildCommand`: Create, Invite, Quit,
  Founder), `string` (the offending name), `result` (`GuildCommandResult`: `AlreadyInGuild`,
  `GuildNameExistsS`, `GuildNameInvalid`, `GuildPlayerNotFoundS`, `GuildPermissionsOrLeader`,
  `GuildPlayerNotInGuildS`, and the rest). Every rejection maps to one of these. Do not invent a
  system message.
- `SMSG_GUILD_EVENT` carries a `GuildEvent` enum (`Promotion`, `Demotion`, `Motd`, `Joined`, `Left`,
  `Removed`, `LeaderIs`, `LeaderChanged`, `Disbanded`, `RosterUpdate`, `SignedOn`, `SignedOff`) plus
  a `Vec<String>` of descriptions. It is the broadcast-to-the-guild channel.
- `PLAYER_GUILDID` (191) and `PLAYER_GUILDRANK` (192) must be added to
  `gateway/src/codec/update_mask.rs`'s `idx` module. They sit between the existing `PLAYER_FLAGS`
  (190) and `PLAYER_BYTES_2` (194), so the numbering cross-checks against what is already there.

## Slices

Each slice is independently shippable and independently verifiable. The order is chosen so that the
first slice proves the storage and the realm-core routing with the smallest possible wire surface.

### Slice 1: create and query

A guild exists, has a name, has ten ranks, and the client can look it up.

- `module/src/guild.rs` with all five tables, the sweeps, and a `create_guild` reducer.
- `realm_guild_op` on realm-core, operator-gated, mirroring `realm_group_op`.
- `gateway/src/world/guild.rs`, the routing half, generic over `WorldStore`, mirroring
  `world::party`. Single-database gateways take the direct path with no realm-core hop.
- `CMSG_GUILD_CREATE`, `CMSG_GUILD_QUERY`, `CMSG_GUILD_INFO` and their responses.
- Name validation: length, character set, realm-wide uniqueness. Rejections go out as
  `SMSG_GUILD_COMMAND_RESULT`.
- `game_character.guild_id` / `guild_rank` columns, written on create, pushed to the shard.
- `SMSG_CHAR_ENUM` sends the real `guild_id`.

**Done when**: a character runs `CMSG_GUILD_CREATE`, `spacetime sql` shows one `game_guild` row and
ten `game_guild_rank` rows on realm-core, `CMSG_GUILD_QUERY` returns the name and all ten rank names,
and a second create with the same name is rejected with `GuildNameExistsS`.

**Verification**: unit tests for name validation and the ten-rank seeding, `spacetime sql` for the
rows, one wire-harness assertion for the create-then-query round trip.

### Slice 2: invite, accept, leave, kick, disband, roster

The membership lifecycle. This is the largest slice and it is where the party precedent pays off:
the invite handshake, the per-recipient event relay, and the cross-shard name resolution are all
structurally the same code as `world::party`.

- `CMSG_GUILD_INVITE` / `SMSG_GUILD_INVITE` / `CMSG_GUILD_ACCEPT` / `CMSG_GUILD_DECLINE`.
- `CMSG_GUILD_LEAVE`, `CMSG_GUILD_REMOVE` (kick), `CMSG_GUILD_DISBAND`.
- `CMSG_GUILD_ROSTER` / `SMSG_GUILD_ROSTER`, with names, levels, classes, areas and online status
  filled at render time from the shards.
- `SMSG_GUILD_EVENT` broadcast on join, leave, removal, disband, sign-on and sign-off.
- Name resolution across shards via `world::party::resolve_all_by_name`.
- Guild-master succession: leaving as guild master with members remaining is **rejected** in this
  slice (`GuildPermissionsOrLeader`). Explicit transfer via `CMSG_GUILD_LEADER` lands with slice 4.
  Rejecting is correct here, not a gap: silently auto-promoting somebody is a guild-politics decision
  the server should not make, and vanilla does not make it either.
- Disband deletes the guild, all members, all ranks, all pending invites, and zeroes every member's
  `game_character.guild_id` / `guild_rank` on whichever shard holds them.

**Done when**: an invite crossing a shard boundary is accepted, both characters appear in each
other's `SMSG_GUILD_ROSTER` with correct names and online flags, a kick and a leave both remove the
row and fire `SMSG_GUILD_EVENT`, and disband leaves zero guild rows behind.

**Verification**: `world::guild` decision tests against the in-memory multi-database topology the
transfer-transport harness provides, including one test pinning that a single-database gateway runs
every op on the player's own shard. Wire-harness assertion for the invite-accept-roster round trip.

### Slice 3: guild chat

- `ChatType::Guild` stops falling through in `handlers/query.rs`.
- `guild_chat` reducer on realm-core, writing one RLS-scoped `GuildEvent` per online member plus an
  echo to the sender.
- Relay through realm-core's coordinator connection, self-filtered per session.
- Offline members receive nothing. No queue, no replay.

**Done when**: two guild members on **different shards** exchange `/g` lines, an offline member's
session receives nothing on login, and a non-member sending `/g` gets
`SMSG_GUILD_COMMAND_RESULT(GuildPlayerNotInGuild)`.

**Verification**: a pure-function test for the recipient set, mirroring
`party_chat_other_recipients`, plus one wire-harness assertion for cross-shard delivery.

### Slice 4: MOTD, notes, and leadership transfer

- `CMSG_GUILD_MOTD`, guild-master gated, broadcast as `SMSG_GUILD_EVENT(Motd)`.
- MOTD delivered on login to every member.
- `CMSG_GUILD_INFO_TEXT`, `CMSG_GUILD_SET_PUBLIC_NOTE`, `CMSG_GUILD_SET_OFFICER_NOTE`.
- `CMSG_GUILD_LEADER`, the explicit transfer slice 2 rejected.
- `SMSG_WHO` sends the real guild name.

**Done when**: the MOTD set by the guild master appears for a member logging in afterwards, and
`/who` shows guild names.

**Verification**: unit tests plus one wire-harness login assertion.

## Explicitly deferred

Each of these is a follow-up issue, not a hidden gap in the above.

- **Rank permissions.** `CMSG_GUILD_RANK`, `CMSG_GUILD_ADD_RANK`, `CMSG_GUILD_DEL_RANK`,
  `CMSG_GUILD_PROMOTE`, `CMSG_GUILD_DEMOTE`, and enforcement of the rights bitmask. Per D3.
- **Charters and petitions.** Per D2.
- **Tabards and emblems.** `MSG_SAVE_GUILD_EMBLEM`. The emblem columns in
  `SMSG_GUILD_QUERY_RESPONSE` ship as zeroes.
- **Officer chat** (`/o`). Lands with rank enforcement.
- **Guild bank.** Not a vanilla 1.12.1 feature. Never in scope.
- **Faction gating.** `GuildNotAllied` exists in the result enum, and cross-faction invites should be
  rejected once faction data is threaded through the realm-core name resolution. Not slice 1 to 4.

## Risks

1. **Realm-core holds no character rows.** Every roster render needs a shard fan-out for names,
   levels, classes, areas and online status. `world::party::render_list` is the precedent and the
   thing to read first. Getting this wrong produces a roster that renders but is full of level-1
   unknown-class members.
2. **The fixed ten-rank array.** A guild that somehow has fewer than ten `GuildRank` rows will panic
   or truncate at packet build. Seed ten at creation and treat the count as an invariant, not as data.
3. **Migration.** Two new `game_character` columns. `docs/danger-zones.md` §1.2, typed defaults, and
   the reminder that nothing in `cargo test` or `cargo check` validates default-value encoding. Only
   a real `publish` does.
4. **Scope creep into ranks.** D3 draws the line at "transmitted, not enforced". The pull toward
   implementing the rights bitmask properly during slice 2 will be strong, because the column is
   right there. It is a separate issue.
