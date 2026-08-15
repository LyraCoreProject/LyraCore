# T5 — Guild chat

Parent: issue #15. Depends on T1. **Runs in parallel with T2, T3, T4, T6.**
Model: Sonnet. Estimated size: ~150k tokens.

## Problem

`/g` is dropped on the floor. `gateway/src/world/handlers/query.rs:361` has a catch-all arm
(`_ => {}`) that swallows `ChatType::Guild` with no store call and no reply, and
`world/tests.rs:7156` pins that behaviour as `messagechat_guild_is_dropped`. The player sees their
line vanish.

This is the last of the four silent holes the issue's parity audit found that is pure relay work.

## Delivery

Guild chat is the party-chat construction against guild rows. Party chat writes one `GroupEvent` per
recipient with `kind = PARTY_CHAT` and the message in `payload`, RLS-scoped so only that recipient's
connection sees it, plus a separate echo row for the sender. Guild chat does the same against
`GuildEvent`. Do not invent a new transport — D4.

- Add a `GUILD_CHAT` event kind and an `encode_guild_chat` / `decode_guild_chat` pair to
  `crates/lyracore-shared/src/guild.rs`, mirroring `encode_party_chat` / `decode_party_chat` in
  `group.rs`.
- Add a `guild_chat` reducer that writes one RLS-scoped `GuildEvent` per **online** member plus an
  echo to the sender. Vanilla server-echoes the speaker's own line back, exactly as party does.
- Add a pure `guild_chat_other_recipients(sender_guid, &member_guids)` helper, mirroring
  `party_chat_other_recipients`, so the recipient set is testable without a database.
- Delivery rides realm-core's coordinator connection, self-filtered per session — the
  coordinator-relay rule in `stdb::connection`. `world::whisper` is the closest precedent for a
  realm-core relay with no shard mirror behind it.
- Replace the catch-all drop in `handlers/query.rs` with a real `ChatType::Guild` arm.

**Online members only.** No offline queue, no history, no replay on login. That is the issue's
wording and it is also vanilla.

`/o` (officer chat) is **out of scope**: it is this same relay with a rank filter, and it lands with
rank enforcement in a later issue.

## Acceptance criteria

1. A `/g` line from a guild member delivers exactly one `SMSG_MESSAGECHAT` with `ChatType::Guild` to
   every other **online** member.
2. The sender receives an echo of their own line.
3. Offline members receive nothing, and nothing is queued for them: after an offline member logs in,
   they receive no backlog.
4. Two members on **different shards** exchange `/g` lines successfully. Assert this explicitly
   against the multi-database topology — it is why the relay runs on realm-core.
5. A `/g` line from a character not in a guild replies
   `SMSG_GUILD_COMMAND_RESULT { result: GuildPlayerNotInGuild }` and delivers nothing.
6. `guild_chat_other_recipients` excludes the sender, preserves member order, and returns empty when
   the sender is the guild's only member.
7. The existing `messagechat_guild_is_dropped` test in `world/tests.rs:7156` is **deleted** and
   replaced by a test asserting the line is now relayed. Leaving the old test passing means the
   feature did not ship.
8. An unsharded gateway relays `/g` on the player's own shard, byte-identically.

## Tests

- Pure-function tests for `guild_chat_other_recipients` (criterion 6), mirroring the
  `party_chat_other_recipients` tests in `module/src/chat.rs`.
- Seam tests in `handlers/guild.rs` for the dispatch branch and criteria 1, 2, 5.
- Routing tests in `world/guild.rs` for criteria 4 and 8.
- **One** encrypted-socket test in `world/tests.rs` for a `ChatType::Guild` line, replacing the
  deleted drop test.

## File ownership

Yours: the chat region of `module/src/guild.rs`, `gateway/src/world/handlers/guild.rs`,
`gateway/src/world/guild.rs` and `crates/lyracore-shared/src/guild.rs`; the `ChatType::Guild` arm in
`gateway/src/world/handlers/query.rs`; the guild-chat additions to `module/src/chat.rs`; and your own
tests in `world/tests.rs`. Append **after T4's region**, in ticket order.

You are the only ticket touching `handlers/query.rs` and `module/src/chat.rs`.

## Non-goals

- Do not implement `/o` officer chat.
- Do not implement invite, accept, decline, leave, kick, disband, roster or MOTD.
- Do not touch the `game_channel_*` system. A guild is not a chat channel.
- Do not add an offline message queue. Criterion 3 is deliberate.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway`,
`cargo test -p lyracore-module` clean. Rebased onto the integration branch that carries T1.
