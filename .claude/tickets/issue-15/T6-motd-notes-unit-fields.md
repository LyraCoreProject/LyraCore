# T6 — MOTD, notes, unit fields and `/who`

Parent: issue #15. Depends on T1. **Runs in parallel with T2, T3, T4, T5.**
Model: Sonnet. Estimated size: ~150k tokens.

## Problem

Three things are missing that make a guild visible rather than merely stored.

The MOTD is named in the issue title and is the guild's one piece of writable shared text. T1 gives
the `Guild` row a `motd` column but nothing can set it and nothing shows it.

`SMSG_WHO` sends an empty guild name for every row (`gateway/src/codec/social.rs:37`, comment: "no
guild system yet"), so `/who` never shows guild membership.

`PLAYER_GUILDID` and `PLAYER_GUILDRANK` are absent from the object update fields, so a player's
guild is invisible on their unit — the client cannot render it under a nameplate or in the character
panel, regardless of what the roster says.

## Delivery

**Setters.** `CMSG_GUILD_MOTD`, `CMSG_GUILD_INFO_TEXT`, `CMSG_GUILD_SET_PUBLIC_NOTE`,
`CMSG_GUILD_SET_OFFICER_NOTE`. Append the op tags to the shared contract and the arms to
`realm_guild_op` and `dispatch_guild_action`.

Gating, per D3 (master-only is the only permission check that exists):

- MOTD and info text: guild master only, else `GuildPermissionsOrLeader`.
- Public note: a member may set their **own**; the master may set anyone's.
- Officer note: guild master only. Proper officer-rank gating arrives with rank enforcement.

Setting the MOTD broadcasts `SMSG_GUILD_EVENT(Motd)` to every online member. The MOTD is also
delivered to a member on login.

**Unit fields.** Add `PLAYER_GUILDID` (191) and `PLAYER_GUILDRANK` (192) to the `idx` module in
`gateway/src/codec/update_mask.rs`. They sit between the existing `PLAYER_FLAGS` (190) and
`PLAYER_BYTES_2` (194), which cross-checks the numbering against what is already in that file.
Populate both from the character's `game_character.guild_id` / `guild_rank` scalars T1 added, and
relay them live when membership changes.

**`/who`.** `gateway/src/codec/social.rs` sends the real guild name in `SMSG_WHO`. Remove the "no
guild system yet" comment rather than leaving it stale.

## Acceptance criteria

1. The guild master setting the MOTD updates `Guild.motd` and broadcasts `SMSG_GUILD_EVENT(Motd)` to
   every online member.
2. A non-master setting the MOTD is rejected with `GuildPermissionsOrLeader` and changes nothing.
3. A member logging in receives the current MOTD.
4. An empty MOTD is a valid value: it clears the MOTD rather than being rejected.
5. Info text behaves as the MOTD for gating and storage, without the broadcast.
6. A member setting their own public note updates only their own row. A member setting **another**
   member's public note is rejected unless they are the master.
7. The guild master setting an officer note updates the named member's row; a non-master is
   rejected.
8. `PLAYER_GUILDID` and `PLAYER_GUILDRANK` carry the character's real guild and rank in the object
   update, and `0` / `0` for an unguilded character.
9. A membership change relays the new `PLAYER_GUILDID` / `PLAYER_GUILDRANK` live, without requiring
   a relog.
10. `SMSG_WHO` shows the real guild name for a guilded character and an empty string for an
    unguilded one.
11. An unsharded gateway runs every setter on the player's own shard, byte-identically.

## Tests

- Seam tests in `handlers/guild.rs` per setter branch and per criterion.
- Codec tests for the two new update-mask indices, following the existing convention in
  `gateway/src/codec/tests.rs` (`idx::PLAYER_QUEST_LOG_1_1` has one — copy its shape).
- A codec test for the `/who` guild name in `codec/social.rs`.
- **One** encrypted-socket test in `world/tests.rs` for `CMSG_GUILD_MOTD`.

## File ownership

Yours: the motd/notes region of `module/src/guild.rs`, `gateway/src/world/handlers/guild.rs`,
`gateway/src/world/guild.rs` and `crates/lyracore-shared/src/guild.rs`; **all** of
`gateway/src/codec/update_mask.rs` and `gateway/src/codec/social.rs`; and your own tests in
`world/tests.rs` and `codec/tests.rs`. Append **after T5's region**, in ticket order.

You are the only ticket touching the two codec files.

## Non-goals

- Do not implement promote, demote, add rank or delete rank, and do not enforce the `rights`
  bitmask. D3.
- Do not implement tabards or emblems (`MSG_SAVE_GUILD_EMBLEM`). The emblem fields in
  `SMSG_GUILD_QUERY_RESPONSE` stay zero.
- Do not implement invite, accept, decline, leave, kick, disband, roster or chat.
- Do not change the roster packet. T4 reads `motd` and `guild_info`; you only make them writable.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway`,
`cargo test -p lyracore-module` clean. Rebased onto the integration branch that carries T1.
