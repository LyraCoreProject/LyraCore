# T1 — Establish the guild seam with create and query

Parent: issue #15. **Runs alone. Blocks T2, T3, T4, T5, T6, T7.**
Model: Opus. Estimated size: ~200k tokens.

## Problem

There is no guild system at all: no tables, no reducers, no shared contract, no gateway seam, and no
`CMSG_GUILD_*` dispatch. Every later ticket needs the same four pieces of scaffolding, and if each
invents its own the parallel branches will not merge.

This ticket is the tracer. It establishes the storage, the realm-core routing decision, and the
gateway action seam by driving the smallest complete path through all of them: a player creates a
guild and the client queries it back.

## Delivery

**Module (`module/src/guild.rs`, new).** All five tables from the README, with their indexes and
uniqueness constraints, plus the sweeps (`character_owned!` delete on `GuildMember` and
`GuildInvite`, `not_transported` on `GuildMember`, `GuildInvite` and `GuildEvent`). Register the
module in `module/src/lib.rs`. Wire `GuildEvent` into the shared event GC in `gc.rs` and give
`GuildInvite` the two-minute TTL `GroupInvite` has.

Reducers:

- `create_guild(name)` — validates the name, creates the `Guild`, seeds **exactly ten** `GuildRank`
  rows with the vanilla names, inserts the caller as a `GuildMember` at `rank_index` 0, and sets
  `master_guid`.
- `realm_guild_op(...)` — the operator-gated realm-core entry point, mirroring `realm_group_op`.
  Carries an op tag from the shared contract. T1 wires the CREATE tag; later tickets append theirs.

Add `guild_id` (`u64`) and `guild_rank` (`u32`) to `game_character`, END-appended, with
**typed** defaults — `#[default(0u64)]` for the `u64`. Set both on create.

**Shared contract (`crates/lyracore-shared/src/guild.rs`, new).** Event kinds, realm-op tags,
classified error strings, and the roster payload grammar with its `encode_roster` / `decode_roster`
pair. Sibling to `group.rs`; register it in `lib.rs`.

**Gateway seam (`gateway/src/world/handlers/guild.rs`, new).** `GuildActionStore`,
`GuildActionPlayer`, `GuildActionOutcome`, `dispatch_guild_action`, the `Coordinator` impl, and the
`InMemoryGuildActions` test adapter. Export from `handlers/mod.rs`. Add `GuildActionStore` to the
`WorldStore` supertrait list in `world/store.rs`, so any caller holding a `WorldStore` already
satisfies it and no handler needs a second store parameter.

**Gateway routing (`gateway/src/world/guild.rs`, new).** The realm-core routing half, generic over
`WorldStore`, copying `world::party`. `realm_store()` returning `None` takes the direct path.

**Opcodes.** `CMSG_GUILD_CREATE`, `CMSG_GUILD_QUERY`, `CMSG_GUILD_INFO`, replying with
`SMSG_GUILD_QUERY_RESPONSE`, `SMSG_GUILD_INFO` and `SMSG_GUILD_COMMAND_RESULT`. Add the opcode
numbers to `crates/lyracore-shared/src/opcodes.rs`.

**`SMSG_CHAR_ENUM`** sends the real `guild_id` instead of the hard-coded `0` at
`gateway/src/codec/char.rs:124`.

Name validation: length bounds, character set, and realm-wide uniqueness. Rejections go out as
`SMSG_GUILD_COMMAND_RESULT`, never as a chat line.

## Acceptance criteria

1. `CMSG_GUILD_CREATE` with a valid unused name creates exactly one `Guild` row, exactly ten
   `GuildRank` rows, and exactly one `GuildMember` row at `rank_index` 0 whose guid is the caller.
2. `Guild.master_guid` is the caller, and the caller's `game_character.guild_id` / `guild_rank` are
   set to the new guild and 0.
3. A create with a name that already exists is rejected with
   `SMSG_GUILD_COMMAND_RESULT { command: Create, result: GuildNameExistsS }` and writes no rows.
4. A create with an invalid name is rejected with `GuildNameInvalid` and writes no rows.
5. A create by a character already in a guild is rejected with `AlreadyInGuild` and writes no rows.
6. `CMSG_GUILD_QUERY` for an existing guild returns one `SMSG_GUILD_QUERY_RESPONSE` carrying the
   guild id, the name, and all ten rank names in rank order.
7. `CMSG_GUILD_QUERY` for an unknown guild id returns `Handled` with an empty batch.
8. `CMSG_GUILD_INFO` returns one `SMSG_GUILD_INFO` with the name, the creation date split into day,
   month and year, and the member count.
9. `SMSG_CHAR_ENUM` reports the real `guild_id` for a character in a guild, and `0` for one not in
   a guild.
10. A refused reducer returns an empty batch or a command result, logs at debug, and is not
    session-fatal. A cause containing `"reducer transport disconnected"` propagates as `Err`.
11. An unsharded gateway (`realm_store()` is `None`) runs create and query on the player's own
    shard, byte-identically. Pin this the way
    `an_unsharded_gateway_runs_every_party_op_on_the_players_own_shard` does.

## Tests

- Seam tests in `handlers/guild.rs`, one per dispatch branch and per criterion above, driven through
  `dispatch_guild_action` with `InMemoryGuildActions`. The adapter records durable requests and can
  be configured to refuse.
- Module tests for name validation and the ten-rank seeding invariant.
- Routing tests in `world/guild.rs` against the in-memory multi-database topology, including the
  single-database assertion in criterion 11.
- **One** encrypted-socket test in `world/tests.rs` for `CMSG_GUILD_CREATE`, stating that it proves
  dispatch and the typed reply over the cipher, not validation logic.

## File ownership

Everything listed under Delivery. You are alone; nothing else runs concurrently.

## Non-goals

- No invite, accept, decline, leave, kick, disband, roster, chat, MOTD or notes. Later tickets.
- No rank permissions and no rights enforcement. D3 in the README. The `rights` column is written
  with vanilla defaults and never read.
- No charter or petition. D2.
- No world-shard roster mirror. D1.
- Do not touch `codec/social.rs` or `codec/update_mask.rs`. T6 owns those.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway`,
`cargo test -p lyracore-module` clean. Integrated to the shared integration branch before any
parallel ticket starts.
