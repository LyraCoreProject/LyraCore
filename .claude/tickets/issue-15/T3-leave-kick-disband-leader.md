# T3 — Leave, kick, disband and leadership transfer

Parent: issue #15. Depends on T1. **Runs in parallel with T2, T4, T5, T6.**
Model: Opus. Estimated size: ~170k tokens.

## Problem

A guild that can be joined but never left leaks membership, and the leak is worse than cosmetic:
`GuildMember.character_guid` is unique across the table, so a character stuck in a dead guild can
never join another one. Teardown is also where the only genuinely contested design question in this
issue lives — what happens to a guild when its master leaves.

You do not need T2 to build this. Seed multi-member guilds through `InMemoryGuildActions` canned
reads.

## Delivery

- `CMSG_GUILD_LEAVE` → the caller removes themselves.
- `CMSG_GUILD_REMOVE` → the guild master removes another member (kick).
- `CMSG_GUILD_DISBAND` → the guild master destroys the guild.
- `CMSG_GUILD_LEADER` → the guild master hands the guild to another member.

Append the LEAVE / REMOVE / DISBAND / LEADER op tags to the shared contract and the matching arms to
`realm_guild_op` and `dispatch_guild_action`.

Every removal path clears the departing character's `game_character.guild_id` / `guild_rank` on
whichever shard holds them, and broadcasts the matching `SMSG_GUILD_EVENT` (`Left`, `Removed`,
`Disbanded`, `LeaderChanged`).

**Guild-master succession is explicit, never automatic.** A master calling `CMSG_GUILD_LEAVE` while
other members remain is **rejected** with `GuildPermissionsOrLeader`. They must either transfer with
`CMSG_GUILD_LEADER` or disband. This is a decision, not a gap: silently promoting somebody is a
guild-politics call the server should not make, and vanilla does not make it. A master who is the
**last** member may leave, which disbands the guild.

Disband deletes the `Guild`, every `GuildMember`, all ten `GuildRank` rows and every pending
`GuildInvite`, and zeroes `guild_id` / `guild_rank` for every member across every shard that holds
one. Leaving residue here is the failure mode to test hardest: an orphaned `GuildMember` row is a
character permanently unable to join a guild.

The `character_owned!(delete, ...)` sweep on `GuildMember` from T1 routes a deleted character
through this same removal logic. Verify it does, rather than assuming: a bare row delete would strip
a guild of its master with no succession and no broadcast.

## Acceptance criteria

1. A non-master member leaving removes exactly their `GuildMember` row, clears their `guild_id` /
   `guild_rank`, and broadcasts `SMSG_GUILD_EVENT(Left)` to the remaining members.
2. A master leaving with other members remaining is rejected with `GuildPermissionsOrLeader` and
   changes nothing.
3. A master who is the last remaining member may leave, and the guild is fully disbanded by it.
4. A kick by the master removes the named member, clears their scalars, and broadcasts `Removed`.
5. A kick by a non-master is rejected with `GuildPermissionsOrLeader`.
6. A kick naming a character who is not in the guild is rejected with `GuildPlayerNotInGuildS`.
7. A master cannot kick themselves.
8. Disband by the master leaves **zero** `Guild`, `GuildMember`, `GuildRank` and `GuildInvite` rows
   for that guild, and zeroes every ex-member's `guild_id` / `guild_rank`, including members on
   other shards.
9. Disband by a non-master is rejected with `GuildPermissionsOrLeader` and changes nothing.
10. Leadership transfer to an existing member moves `Guild.master_guid`, sets the new master to rank
    0 and the old master to the second rank, and broadcasts `LeaderChanged`.
11. Leadership transfer to a non-member is rejected with `GuildPlayerNotInGuildS`.
12. Deleting a character routes through the removal logic, not a bare row delete: a deleted
    non-master leaves cleanly, and a deleted last-member master disbands the guild.
13. An unsharded gateway runs every op above on the player's own shard, byte-identically.

## Tests

- Seam tests in `handlers/guild.rs`, one per branch and per criterion, through
  `dispatch_guild_action` with `InMemoryGuildActions` seeded with multi-member guilds.
- Module tests for the disband cascade (criterion 8) and the character-delete sweep (criterion 12).
- Routing tests in `world/guild.rs` for the cross-shard scalar clearing in criterion 8 and for
  criterion 13.
- **One** encrypted-socket test in `world/tests.rs` for `CMSG_GUILD_LEAVE`.

## File ownership

Yours: the teardown region of `module/src/guild.rs`, `gateway/src/world/handlers/guild.rs`,
`gateway/src/world/guild.rs` and `crates/lyracore-shared/src/guild.rs`, plus your own tests in
`world/tests.rs`. Append **after T2's region**, in ticket order.

## Non-goals

- Do not implement invite, accept or decline. T2.
- Do not implement `CMSG_GUILD_ROSTER`. T4.
- Do not add rank permissions beyond the master-only check, and do not implement promote or demote.
  D3 — `CMSG_GUILD_PROMOTE` and `CMSG_GUILD_DEMOTE` are a separate future issue. `CMSG_GUILD_LEADER`
  is in scope only because succession needs it.
- Do not touch `codec/social.rs` or `codec/update_mask.rs`. T6.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway`,
`cargo test -p lyracore-module` clean. Rebased onto the integration branch that carries T1.
