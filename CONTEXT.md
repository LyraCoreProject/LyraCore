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
