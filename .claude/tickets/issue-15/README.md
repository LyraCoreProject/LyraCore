# Issue #15 — guild system, broken into tickets

Source: `gh issue view 15` — "Guild system (charter/create, roster, ranks, guild chat, MOTD)".
Labels: `enhancement`, `P3`. The issue is a parity-audit stub whose only acceptance note is
"(slice on pickup)". This folder is that slicing.

Design decisions are in `docs/guild-system.md` on branch `t3code/d105e0a4`. **That file is
uncommitted at the time of writing — commit it before fanning out, or every agent starts blind.**
Everything load-bearing is inlined below anyway.

## State of the world

Nothing guild-related exists. The gap shows up as four silent holes, all of which some ticket here
closes:

| Site | Today |
| --- | --- |
| `gateway/src/codec/char.rs:124` | `SMSG_CHAR_ENUM` sends `guild_id: 0` for everyone |
| `gateway/src/codec/social.rs:37` | `SMSG_WHO` sends an empty guild name |
| `gateway/src/world/handlers/query.rs:361` | `ChatType::Guild` falls through, dropped, no reply |
| every `CMSG_GUILD_*` opcode | never dispatched |

**The issue-number trap.** `module/src/group.rs` cites "#22", "#54", "#19", "#20", "#30". Those are
**private-tracker** numbers carried over in the migration. They are NOT this repo's issues (public
#22 is mounts, #54 is zone-shard topology). Do not chase them on GitHub. The work they describe has
shipped — read the code, not the tracker.

## The two patterns to copy

Guild sits at the intersection of two existing systems. Both already agree with themselves. Follow
them and do not invent a third style.

### 1. Realm-core routing — copy `gateway/src/world/party.rs`

Guild state is authoritative on **realm-core**, like party, whisper and mail. The routing half is
generic over `WorldStore` so decisions execute under test against the in-memory multi-database
topology the transfer-transport harness provides:

```rust
pub(crate) fn run<St: WorldStore + ?Sized>(store: &St, /* … */) -> Result<()>
```

`WorldStore::realm_store()` answers `None` on a single-database gateway, which takes the direct
path: the player's own connection, the player-facing reducer, the shard's own tables. Byte-identical
to the sharded path from the client's side. `party.rs` pins this with
`an_unsharded_gateway_runs_every_party_op_on_the_players_own_shard`; every guild ticket that adds an
op adds the equivalent assertion.

Realm-core holds **no character rows**. Names, levels, classes, areas and online flags are filled at
render time from the shards. `world::party::render_list` is the precedent. See T4.

Cross-shard helpers that already exist and must be reused, not rewritten:
`party::resolve_all_by_name` (realm-wide name union), `party::live_anywhere`, `party::presence`.

### 2. The gateway action seam — copy `handlers/melee.rs` or `handlers/vendor.rs`

Issues #204, #205, #206, #210 and #212 converged the gateway on one shape. Guild is greenfield, so
it starts there rather than being retrofitted later:

```rust
pub(crate) trait GuildActionStore: Send + Sync { /* narrow, guild-only durable ops + reads */ }
impl GuildActionStore for crate::stdb::Coordinator { /* thin forwarding */ }

pub(crate) struct GuildActionPlayer { account_id: u64, self_guid: Option<u64> }

pub(crate) enum GuildActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

pub(crate) fn dispatch_guild_action<St: GuildActionStore + ?Sized>(
    store: &St, player: GuildActionPlayer, msg: ClientOpcodeMessage,
) -> Result<GuildActionOutcome>
```

Plus `#[cfg(test)] mod tests` in the same file with an `InMemoryGuildActions` adapter that records
durable requests and returns canned reads. One focused test per dispatch branch.

This is what makes the issue's "all wire-verified headlessly" criterion cheap: the branches are
tested at the seam, and only one encrypted-socket test per opcode family proves dispatch over the
cipher.

Error classification, already written in `handlers/item.rs` and copied verbatim: a cause containing
`"reducer transport disconnected"` is `Fatal` and propagates as `Err`. Everything else is a
`GameplayRefusal` — logged at debug, never session-fatal.

## Decisions that are settled — do not relitigate inside a ticket

- **D1: no world-shard mirror.** Party needed `sync_group_mirror` because ~50 in-world reads resolve
  party membership on the hot path. Guild has **zero** hot-path in-world reads. The character's own
  guild id and rank ride two scalar columns on `game_character`, pushed at world entry and on
  membership change. Do not build a roster mirror.
- **D2: charter-less creation.** `CMSG_GUILD_CREATE` creates the guild for the caller. Petitions,
  signatures and the charter item are a separate future issue. Do not enforce a signature count.
- **D3: ranks are data, not permissions.** The wire forces ranks: `SMSG_GUILD_QUERY_RESPONSE`
  carries a fixed `[String; 10]`, `GuildMember.rank` is a required `u32`. So tables and packets
  carry real values from T1. **Enforcement is deferred**: the only permission check in this whole
  folder is "are you the guild master". The `rights` bitmask ships as the vanilla per-rank defaults
  and is never consulted server-side. Writing enforcement is out of scope and will be reverted.
- **D4: guild chat reuses the group-event relay shape**, not a new transport.

## Schema (T1 owns all of it)

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

Invariants:

- `Guild.name` unique realm-wide. One database on realm-core, so a unique index suffices.
- `GuildMember.character_guid` unique across the table. A character is in at most one guild.
- **`GuildRank` is always exactly ten rows per guild.** Seeded at creation with the vanilla names
  (Guild Master, Officer, Veteran, Member, Initiate, then five empty). Treat the count as an
  invariant, not as data: `SMSG_GUILD_QUERY_RESPONSE.rank_names` is `[String; 10]` and a short guild
  panics or truncates at packet build.

Two columns are END-appended to the existing `game_character`: `guild_id` (`u64`) and `guild_rank`
(`u32`). `docs/danger-zones.md` §1.2 applies. **The `u64` default must be written `#[default(0u64)]`,
not `#[default(0)]`** — a bare `0` encodes as 4 bytes and `publish` rejects the migration with
"data too short for u64". Nothing in `cargo test` or `cargo check` catches this; only a real publish
does. `group.rs` records the same trap at `GroupEvent.recipient_guid`.

Sweeps, following `group.rs`:

- `character_owned!(delete, ...)` on `GuildMember` — a deleted character leaves through the same
  path a voluntary leave uses, never a bare row delete, so master succession and last-member
  disband both run.
- `character_owned!(delete, ...)` on `GuildInvite` — rows where the character is target (indexed)
  and where it is inviter (scan; the table only holds pending invites).
- `character_owned!(not_transported, ...)` on `GuildMember`, `GuildInvite`, `GuildEvent` —
  membership is realm-core state and must not ride the transfer blob, or it races the authority.
- `GuildEvent` is reaped by the shared event GC in `gc.rs`. `GuildInvite` gets the two-minute TTL
  `GroupInvite` has.

## Shared contract (T1 creates it, later tickets append)

`crates/lyracore-shared/src/guild.rs`, sibling to `group.rs`: event kinds, the roster payload
grammar, realm-op tags, classified error strings. One definition both crates import, so a renumber
or delimiter change is a cross-crate compile error, never runtime drift. The roster encode/decode
pair mirrors `encode_roster` / `decode_roster`.

## Wire surface

All types exist in `wow_world_messages` 0.3 (vanilla feature) and `wow_world_base` 0.3. No
hand-rolled packets. Opcodes below are verified against the crate, not from memory — note that
LEAVE / REMOVE / DISBAND are 0x08D / 0x08E / 0x08F, which is easy to transpose.

| Direction | Message | Opcode | Ticket |
| --- | --- | --- | --- |
| C→S | `CMSG_GUILD_QUERY` | 0x054 | T1 |
| S→C | `SMSG_GUILD_QUERY_RESPONSE` | 0x055 | T1 |
| C→S | `CMSG_GUILD_CREATE` | 0x081 | T1 |
| C→S | `CMSG_GUILD_INFO` | 0x087 | T1 |
| S→C | `SMSG_GUILD_INFO` | 0x088 | T1 |
| S→C | `SMSG_GUILD_COMMAND_RESULT` | 0x093 | T1 |
| C→S | `CMSG_GUILD_INVITE` | 0x082 | T2 |
| S→C | `SMSG_GUILD_INVITE` | 0x083 | T2 |
| C→S | `CMSG_GUILD_ACCEPT` | 0x084 | T2 |
| C→S | `CMSG_GUILD_DECLINE` | 0x085 | T2 |
| S→C | `SMSG_GUILD_EVENT` | 0x092 | T2 |
| C→S | `CMSG_GUILD_LEAVE` | 0x08D | T3 |
| C→S | `CMSG_GUILD_REMOVE` | 0x08E | T3 |
| C→S | `CMSG_GUILD_DISBAND` | 0x08F | T3 |
| C→S | `CMSG_GUILD_LEADER` | 0x090 | T3 |
| C→S | `CMSG_GUILD_ROSTER` | 0x089 | T4 |
| S→C | `SMSG_GUILD_ROSTER` | 0x08A | T4 |
| C→S | `CMSG_MESSAGECHAT` (`ChatType::Guild`) | 0x095 | T5 |
| S→C | `SMSG_MESSAGECHAT` (`ChatType::Guild`) | 0x096 | T5 |
| C→S | `CMSG_GUILD_MOTD` | 0x091 | T6 |
| C→S | `CMSG_GUILD_INFO_TEXT` | 0x2FC | T6 |
| C→S | `CMSG_GUILD_SET_PUBLIC_NOTE` | 0x234 | T6 |
| C→S | `CMSG_GUILD_SET_OFFICER_NOTE` | 0x235 | T6 |

`SMSG_GUILD_COMMAND_RESULT` is the **only** error channel. Never invent a system chat message.
`command` is `GuildCommand` (Create, Invite, Quit, Founder); `result` is `GuildCommandResult`
(`AlreadyInGuild`, `GuildNameExistsS`, `GuildNameInvalid`, `GuildPlayerNotFoundS`,
`GuildPermissionsOrLeader`, `GuildPlayerNotInGuildS`, `GuildPlayerNotInGuild`, …).

`SMSG_GUILD_EVENT` carries a `GuildEvent` enum (`Promotion`, `Demotion`, `Motd`, `Joined`, `Left`,
`Removed`, `LeaderIs`, `LeaderChanged`, `Disbanded`, `RosterUpdate`, `SignedOn`, `SignedOff`) plus a
`Vec<String>` of descriptions. It is the broadcast-to-the-guild channel.

`PLAYER_GUILDID` (191) and `PLAYER_GUILDRANK` (192) go in `gateway/src/codec/update_mask.rs`'s `idx`
module, between the existing `PLAYER_FLAGS` (190) and `PLAYER_BYTES_2` (194). T6 owns them.

## Execution order

```
T1  (serial, blocks everything)
 ├── T2 ─┐
 ├── T3 ─┤  parallel worktrees
 ├── T4 ─┤
 ├── T5 ─┤
 └── T6 ─┘
        └── T7 (serial, runs after T2..T6 are integrated)
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|--------|-------|-------------|----------------|
| T1 | Establish the guild seam with create and query | Opus | ~200k | `module/src/guild.rs`, `module/src/lib.rs`, `crates/lyracore-shared/src/guild.rs`, `gateway/src/world/handlers/guild.rs`, `gateway/src/world/guild.rs`, `gateway/src/world/mod.rs`, `gateway/src/codec/char.rs` |
| T2 | Invite, accept and decline | Opus | ~180k | invite region of `guild.rs` (module + both gateway files), `world/tests.rs` |
| T3 | Leave, kick, disband and leadership transfer | Opus | ~170k | teardown region of `guild.rs` (module + both gateway files), `world/tests.rs` |
| T4 | Roster, rendered from the shards | Opus | ~180k | roster region of `guild.rs` (module + both gateway files), `world/tests.rs` |
| T5 | Guild chat | Sonnet | ~150k | chat region of `guild.rs`, `module/src/chat.rs`, `handlers/query.rs`, `world/tests.rs` |
| T6 | MOTD, notes, unit fields and `/who` | Sonnet | ~150k | motd region of `guild.rs`, `codec/update_mask.rs`, `codec/social.rs`, `world/tests.rs` |
| T7 | Integrate, wire-verify, document | Opus | ~150k | all of the above, `docs/guild-system.md`, `CONTEXT.md` |

T2..T6 can all start the moment T1 is integrated. T3 does not truly need T2's invite flow: its tests
seed multi-member guilds through `InMemoryGuildActions` canned reads. Same for T4.

## Shared rules for every ticket

- **Append, do not interleave.** T2..T6 all touch `module/src/guild.rs`,
  `gateway/src/world/handlers/guild.rs`, `gateway/src/world/guild.rs` and
  `crates/lyracore-shared/src/guild.rs`. Append inside each region in ticket order (tables → trait
  block → `Coordinator` impl → `dispatch_guild_action` match → test adapter → tests) so the
  sequential integration merge stays mechanical.
- Durable rules stay in the SpacetimeDB module. The gateway owns protocol and screen selection.
- Every ticket lands demoable and green: `cargo fmt`, `cargo clippy -p lyracore-gateway`,
  `cargo test -p lyracore-gateway`, and `cargo test -p lyracore-module` where the module changed.
- Reducer refusal → `SMSG_GUILD_COMMAND_RESULT` or a silent drop, never session death. Transport
  loss → propagate `Err`.
- Every op added must also add the single-database assertion: an unsharded gateway runs it on the
  player's own shard, byte-identically.
- Match surrounding naming and idiom. Do not copy the legacy essay-comment density from `group.rs`.
  No issue numbers in comments.
- Nothing is published to GitHub. Do not touch issue #15.
