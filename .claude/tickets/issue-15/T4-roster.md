# T4 — Roster, rendered from the shards

Parent: issue #15. Depends on T1. **Runs in parallel with T2, T3, T5, T6.**
Model: Opus. Estimated size: ~180k tokens.

## Problem

The roster is the guild panel. It is also the one packet the realm-core decision makes genuinely
hard, and the place a naive implementation will quietly produce garbage rather than fail.

`SMSG_GUILD_ROSTER` needs each member's name, level, class, area and online status.
**Realm-core holds none of those.** It has no `game_character` rows at all — it knows guids, ranks
and notes. Every human-readable field has to be filled at render time by fanning out to the shards.
An implementation that skips the fan-out still compiles, still sends a well-formed packet, and
renders a guild full of level-1 unknown-class members standing in `Area::None`. That is the failure
mode this ticket exists to prevent.

`world::party::render_list` solved exactly this for the party roster. Copy it.

You do not need T2 or T3. Seed multi-member guilds through `InMemoryGuildActions` canned reads.

## Delivery

`CMSG_GUILD_ROSTER` → `SMSG_GUILD_ROSTER`, carrying:

- `motd` and `guild_info` from the `Guild` row (T6 makes them settable; read whatever is there).
- `rights: Vec<u32>`, one per rank, in rank order, from the `GuildRank` rows. Ships as the vanilla
  per-rank defaults and is **not consulted server-side** — D3.
- `members: Vec<GuildMember>`, each carrying `guid`, `status` (online/offline), `name`, `rank`,
  `level`, `class`, `area`, `public_note`, `officer_note`.

Guids, ranks and notes come from realm-core. Name, level, class, area and status come from the
shards, unioned across every shard in the topology. Reuse `party::presence` and
`party::character_anywhere` rather than writing new lookups.

Offline members appear in the roster with `status` offline and their last-known level, class and
area. They are not omitted — an offline roster entry is the main thing a guild panel is for.

Append the ROSTER op tag to the shared contract and the matching arms to `realm_guild_op` and
`dispatch_guild_action`. Also append the roster payload grammar to the shared contract if T1's
`encode_roster` / `decode_roster` needs widening for the guild-specific fields; keep the change
compile-visible to both crates.

## Acceptance criteria

1. A roster request from a member returns exactly one `SMSG_GUILD_ROSTER` listing every member of
   that guild, online and offline.
2. Each member entry carries the **real** name, level, class and area, resolved from the shard that
   holds that character. Assert against a topology where members sit on **different** shards, with
   distinct levels, classes and areas, so a hard-coded default cannot pass.
3. Online members report online status; offline members report offline status and still appear.
4. `rights` has exactly one entry per rank, in rank order.
5. `motd` and `guild_info` match the `Guild` row.
6. Notes round-trip: a member's `public_note` and `officer_note` appear on their entry.
7. A roster request from a character not in a guild is rejected with `GuildPlayerNotInGuild`.
8. A member whose character row cannot be found on any shard still appears, with the guid and rank
   realm-core knows and safe defaults for the rest. It is never dropped from the list and never
   panics.
9. An unsharded gateway renders the roster from the player's own shard, byte-identically.

## Tests

- Seam tests in `handlers/guild.rs` per criterion, through `dispatch_guild_action` with
  `InMemoryGuildActions`.
- Routing tests in `world/guild.rs` for criteria 2, 3, 8 and 9 against the in-memory multi-database
  topology. Criterion 2 is the important one: give the shards different data and assert the packet
  carries it.
- **One** encrypted-socket test in `world/tests.rs` for `CMSG_GUILD_ROSTER`.

## File ownership

Yours: the roster region of `module/src/guild.rs`, `gateway/src/world/handlers/guild.rs`,
`gateway/src/world/guild.rs` and `crates/lyracore-shared/src/guild.rs`, plus your own tests in
`world/tests.rs`. Append **after T3's region**, in ticket order.

## Non-goals

- Do not implement invite, accept, decline, leave, kick, disband or leadership transfer. T2 and T3.
- Do not make `motd`, `guild_info` or the notes **settable**. T6 owns the setters; you only read.
- Do not enforce the `rights` bitmask. D3.
- Do not build a world-shard roster mirror. D1 — the fan-out is the design, not a workaround.
- Do not touch `codec/social.rs` or `codec/update_mask.rs`. T6.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-gateway`, `cargo test -p lyracore-gateway`,
`cargo test -p lyracore-module` clean. Rebased onto the integration branch that carries T1.
