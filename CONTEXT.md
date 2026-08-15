# LyraCore

A World of Warcraft server. Game state lives in SpacetimeDB; the gateway speaks the vanilla wire protocol to clients.

## Language

### Trading

**Trade Session**:
The negotiation state between two players: offered items, offered gold, and each party's accept flag. Stored as transient module rows — reaped after idle timeout, torn down on death, logout, or transfer, never recovered after a crash.
_Avoid_: escrow, trade escrow, in-memory session

**Trade Commit**:
The single atomic swap of offered items and gold, performed via SpacetimeDB when both parties have accepted. All validation re-runs at commit time.
_Avoid_: escrow swap

**Will-Not-Be-Traded Slot**:
The 7th trade-window slot. Its item is shown to the other party but never included in the Trade Commit.
_Avoid_: enchant slot

### Sharding

**Escrow**:
The hold placed on player state while it moves between shards. Reserved for cross-shard transfer only — trading does not use escrow.

### Guilds

**Guild**:
A realm-wide, persistent roster of characters with a name unique across the realm, a guild master, ten ranks, and a message of the day. Authoritative on realm-core, never mirrored to a world shard — a world shard holds only each character's own guild id and rank.
_Avoid_: clan, order

**Guild Master**:
The single member at rank 0. The only permission the server enforces: inviting, kicking, disbanding, setting the MOTD, setting the info text, setting another member's notes, and handing the guild on all require it. Leadership transfers explicitly, never automatically.
_Avoid_: GM (in this codebase a GM is a game master), leader, owner

**Guild Rank**:
One of ten ordered slots per guild, index 0 (the guild master) to 9. A rank has a name and a rights bitmask. The bitmask is written at creation from the vanilla defaults, carried on the wire, and never consulted server-side.
_Avoid_: role, permission level

**Guild Invite**:
A pending offer from the guild master to one character, at most one per target, TTL two minutes, consumed by accept or decline. Same lifecycle as a party invite.
_Avoid_: application, petition (a petition is the deferred charter item)

**Guild Chat**:
Non-proximity chat scoped to a guild's currently online members, relayed through realm-core. No offline delivery, no history.
_Avoid_: guild channel (a channel is the `game_channel_*` system)
