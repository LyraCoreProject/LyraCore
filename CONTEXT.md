# LyraCore

A World of Warcraft 1.12.1 server. Game state lives in SpacetimeDB; the gateway speaks the vanilla wire protocol to unmodified clients. Standard WoW vocabulary (guid, opcode, aura, gossip, master looter, round-robin, ...) keeps its client meaning and is listed here only where LyraCore narrows or changes it.

## Language

### Realm topology

**Realm**:
The set of shards behind one gateway tier that clients see as one server.
_Avoid_: server, cluster

**Shard**:
One SpacetimeDB database in a realm. The word for the boundary a character or mail crosses is "cross-shard".
_Avoid_: database (when meaning a shard), db, cross-database

**World Shard**:
A shard that owns a set of maps.

**Instance Pool**:
The shard that hosts instanced maps.

**Realm-core**:
The shard that holds realm-wide state: accounts, sessions, groups, whispers, loot rolls.

**Gateway**:
The trusted protocol tier between clients and shards. Holds no durable state.

**Module**:
The wasm that holds all durable state and all game logic. The same wasm runs on every shard.

**Operator**:
The identity that publishes and owns the shards, and the only caller of Gateway Verbs.

### Gateway and module

**Gateway Verb**:
A `gw_*` reducer the Operator calls on a character's behalf, with the Actor named by guid.

**Gate**:
A rule that refuses a request. Gates live in the Module, except the realm-wide reads only the Gateway can perform (presence, name resolution, loot-roll fan-out).
_Avoid_: validation, guard

**Coordinator**:
The Gateway's one connection per shard, authenticated with the Owner Token, serving every Durable Read and Durable Request.

**Owner Token**:
The credential that bypasses row-level security.

**Relay**:
Gateway code that turns a table change into a client message.
_Avoid_: forwarder, pusher

**AOI**:
The area of interest that decides which entities a World Session sees.
_Avoid_: visibility set, interest radius

**Durable Request**:
A reducer call the Gateway makes that changes Module state.
_Avoid_: mutation, write, reducer call (in gateway prose)

**Durable Read**:
A read of Module state through the Coordinator.

**Refusal**:
A Gate saying no to a Durable Request. An expected gameplay outcome, not a transport failure.
_Avoid_: reject, deny, error (for gameplay refusals)

### Accounts, characters and sessions

**Account**:
A login. Owns characters.

**Character**:
A guid-owned player entity.
_Avoid_: player (as a noun in code)

**Actor**:
The guid a Gateway Verb acts as.

**Session**:
The Module's record that an Account is logged in on a Character.

**World Session**:
The Gateway's per-connection loop for one client on the world port.
_Avoid_: session (unqualified, when meaning the connection)

### Sharding and transfer

**Transfer**:
Moving a Character's state from one shard to another. Uses Escrow.

**Home Shard**:
The shard that currently holds a Character's row.

**Shard Map**:
The rules that assign a map or instance to a shard.

**Shard Boundary**:
The edge between two shards.
_Avoid_: seam (see Working method)

**Escrow**:
Value or state held so it exists in exactly one shard while it moves. Used by Transfer and Mail; never by Trading.

### Trading

**Trade Session**:
The negotiation state between two players: offered items, offered gold, and each party's accept flag. Never uses Escrow.
_Avoid_: escrow, trade escrow

**Trade Commit**:
The single atomic swap of offered items and gold once both parties have accepted.
_Avoid_: escrow swap

**Will-Not-Be-Traded Slot**:
The 7th trade-window slot. Its item is shown to the other party but never included in the Trade Commit.
_Avoid_: enchant slot

### Loot

**Loot Window**:
A Character's open loot on one Loot Source.

**Loot Source**:
The corpse or GameObject a Loot Window is open on.
_Avoid_: loot target

**Loot Release**:
Closing a Loot Window. Makes the Loot Source available to the next eligible looter.

**Loot Roll**:
A group's roll on an item. Lives in Realm-core.

### Auctions

**Settlement**:
Resolving an auction at buyout or expiry: item and gold to their final owners, displaced bids refunded.
_Avoid_: resolve, close

### Working method

**Seam**:
The interface where session or protocol handling hands work to durable state, expressed as a trait so tests can substitute the far side. Not the Shard Boundary, not any arbitrary trait.

**Store**:
The durable side of a Seam: the trait a handler calls for Durable Reads and Durable Requests. The Coordinator implements it in production, a Fake in tests.
_Avoid_: repository, adapter, port, backend, service

**Fake**:
A working in-memory Store used by tests.
_Avoid_: harness, mock, stub

**Architecture Test**:
A test that fails the build when a structural rule is broken.
_Avoid_: tripwire, guard test

**Verification**:
A written record of a targeted check of server behaviour against the real client or game data.
_Avoid_: probe

**Prototype**:
Throwaway code built to answer a design question. Never shipped.
_Avoid_: spike, poc

**Headless Client**:
A client that speaks the real 1.12.1 protocol to the Gateway in tests, with no UI.
_Avoid_: wire harness, test harness

**Spec**:
The statement of what a feature must do. Lives on the GitHub issue unless a `docs/*.md` supersedes it.
_Avoid_: PRD, requirements doc, design doc

**Ticket**:
An agent-sized slice of an issue: one context window of work, kept local.

**Tracer**:
The first Ticket of an issue. Establishes the Seam or pattern the other Tickets copy, and blocks them.
