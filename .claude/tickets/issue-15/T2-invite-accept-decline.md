# T2 — Invite, accept and decline

Parent: issue #15. Depends on T1. **Runs in parallel with T3, T4, T5, T6.**
Model: Opus. Estimated size: ~180k tokens.

## Problem

T1 leaves a guild that can only ever have one member. Joining is the handshake that makes it a
guild, and it is the one guild path that must work across a shard boundary from day one: the whole
reason guild state sits on realm-core is that a master standing in Elwynn must be able to invite a
character inside Deadmines. Resolving the target inside the calling database, which is what the
pre-realm-core party code did, produces "no player named X" for a character that plainly exists.

## Delivery

Add the invite handshake to the seam T1 established.

- `CMSG_GUILD_INVITE` → resolve the target realm-wide, gate, write one `GuildInvite`, deliver
  `SMSG_GUILD_INVITE` to the target.
- `CMSG_GUILD_ACCEPT` → consume the invite, insert the `GuildMember` at the lowest rank, set the new
  member's `game_character.guild_id` / `guild_rank` on whichever shard holds them, broadcast
  `SMSG_GUILD_EVENT(Joined)` to the guild.
- `CMSG_GUILD_DECLINE` → consume the invite and notify the inviter. No membership change.

Append the INVITE / ACCEPT / DECLINE op tags to the shared contract and the matching arms to
`realm_guild_op` and `dispatch_guild_action`.

Name resolution uses `party::resolve_all_by_name`, the realm-wide name union. Every candidate, not
the first: character names are unique per **database**, not per realm, so the online gate doubles as
the disambiguator. Do not write a new resolver.

Gates, all replying through `SMSG_GUILD_COMMAND_RESULT`:

- caller is not in a guild → `GuildPlayerNotInGuild`
- caller is not the guild master → `GuildPermissionsOrLeader` (D3: master-only is the *only*
  permission check that exists)
- target does not exist anywhere on the realm → `GuildPlayerNotFoundS`
- target is already in a guild → `AlreadyInGuildS`
- target already has a pending invite → `AlreadyInvitedToGuildS`

`SMSG_GUILD_EVENT(SignedOn)` / `(SignedOff)` on a member's login and logout belong here too — they
are the same broadcast helper and no other ticket has a reason to build it.

## Acceptance criteria

1. An invite by the guild master to an existing, unguilded, uninvited character writes exactly one
   `GuildInvite` and delivers exactly one `SMSG_GUILD_INVITE` to the target.
2. An invite whose target is on a **different shard** from the inviter succeeds, and the target
   receives it. This is the criterion the whole realm-core decision exists for; assert it explicitly
   against the multi-database topology.
3. Each of the five gates above produces its listed `GuildCommandResult` and writes no rows.
4. Accept consumes the invite, inserts exactly one `GuildMember` at the lowest rank index, and sets
   the accepting character's `guild_id` / `guild_rank`.
5. Accept broadcasts `SMSG_GUILD_EVENT(Joined)` to every online member, including the new one.
6. Decline consumes the invite, writes no `GuildMember`, and notifies the inviter.
7. An accept with no pending invite is a no-op that writes nothing and is not session-fatal.
8. An invite is not answerable after its two-minute TTL: once the GC has reaped it, accept behaves
   as criterion 7.
9. A member signing on broadcasts `SignedOn` to the rest of the guild; signing off broadcasts
   `SignedOff`.
10. An unsharded gateway runs invite, accept and decline on the player's own shard, byte-identically.

## Tests

- Seam tests in `handlers/guild.rs`, one per branch and per criterion, through `dispatch_guild_action`
  with `InMemoryGuildActions`.
- Routing tests in `world/guild.rs` for criteria 2 and 10 against the in-memory multi-database
  topology.
- **One** encrypted-socket test in `world/tests.rs` for `CMSG_GUILD_INVITE`, proving dispatch and the
  typed reply over the cipher, not the gates.

## File ownership

Yours: the invite region of `module/src/guild.rs`, `gateway/src/world/handlers/guild.rs`,
`gateway/src/world/guild.rs` and `crates/lyracore-shared/src/guild.rs`, plus your own tests in
`world/tests.rs`. Nothing else. Append your tables' columns, trait methods, match arms and tests
**after T1's region and before T3's**, in ticket order.

## Non-goals

- Do not implement leave, kick, disband or leadership transfer. T3.
- Do not implement `CMSG_GUILD_ROSTER`. T4 owns the roster packet; your `Joined` broadcast is an
  event, not a roster.
- Do not add rank permissions beyond the master-only check. D3.
- Do not touch `codec/social.rs` or `codec/update_mask.rs`. T6.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway`,
`cargo test -p lyracore-module` clean. Rebased onto the integration branch that carries T1.
