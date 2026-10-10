# Core terms

These are the terms you meet in a first change. Code, comments, commits and pull requests use them,
and the words under _Avoid_ do not appear in new names or prose. The full glossary, with an
alphabetical index, is [`CONTEXT.md`](./CONTEXT.md). This page copies its entries, so when a term
changes in one file, change it in the other.

## Where things run

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
The shard that holds realm-wide state: accounts, sessions, Account Claims, groups, guilds, Chat Channels, Realm Chat Lines, mail, auctions, loot rolls, the character-to-shard index and shard load samples. It holds no Characters.

**Home Shard**:
The shard that currently holds a Character's row.

**Module**:
The wasm that holds all durable state and all game logic. The same wasm runs on every shard.

**Gateway**:
The trusted protocol tier between clients and shards. Holds no durable state.

**Operator**:
The identity that publishes and owns the shards, and the only caller of Gateway Verbs.

## How the Gateway talks to the Module

**Gateway Verb**:
A `gw_*` reducer the Operator calls on a character's behalf, with the Actor named by guid.

**Actor**:
The guid a Gateway Verb acts as.

**Gate**:
A rule that refuses a request. Gates live in the Module, except the realm-wide reads only the Gateway can perform (presence, name resolution, loot-roll fan-out).
_Avoid_: validation, guard

**Refusal**:
A Gate saying no to a Durable Request. An expected gameplay outcome, not a Transport Loss.
_Avoid_: reject, deny, error (for gameplay refusals)

**Transport Loss**:
A failed Durable Read or Durable Request that is not a Refusal: the transport dropped, the call
timed out, or the send failed. Its durable outcome is unknown, so it ends the World Session and the
client logs in again from durable state.
_Avoid_: transport failure, fatal error, disconnect

**Durable Request**:
A reducer call the Gateway makes that changes Module state.
_Avoid_: mutation, write, reducer call (in gateway prose)

**Durable Read**:
A read of Module state through the Coordinator.

**Coordinator**:
The Gateway's subscribed connection per shard, authenticated with the Owner Token. It serves every Durable Read; a small pool of call pipes carries Durable Requests.

**Relay**:
Gateway code that turns a table change into a client message.
_Avoid_: forwarder, pusher

**Speech**:
A line a Character speaks to the Characters near it: a say, yell or `/e` line, or a text emote. Its
Durable Request goes to the speaker's Home Shard, and a Relay delivers it to listeners in range.
_Avoid_: local chat, proximity chat

**AOI**:
The area of interest that decides which entities a World Session sees.
_Avoid_: visibility set, interest radius

## Players and moving between Shards

**Account**:
A login. Owns characters.

**Character**:
A guid-owned player entity.
_Avoid_: player (as a noun in code)

**Session**:
The Module's record that an Account is logged in on a Character.

**World Session**:
The Gateway's per-connection loop for one client on the world port.
_Avoid_: session (unqualified, when meaning the connection)

**Transfer**:
Moving a Character's state from one shard to another. Uses Escrow.

**Escrow**:
Value or state held so it exists in exactly one shard while it moves. Used by Transfer and Mail; never by Trading.
Mail Escrow carries a Character's letter, a take from a Mail, and a Reward Letter.

## Extending the server

**Package**:
A drop-in folder under `packages/<name>/` that adds content to the realm with no core-file edits. Its `src/` is compiled into the Module wasm by the build's own discovery; its `client/` half supplies addons, whole-file client overrides and UI Transforms to the client packer. Either half alone is a valid Package.
Its data changes ship as Package Deltas rather than as edits to the base data.
Package-specific tests live with the Package. Core supplies reusable private integration fixtures;
the Package's test runner selects the Core checkout and owns its compatibility checks.
_Avoid_: plugin, addon (when meaning the whole folder), mod, extension

**Package API**:
The part of the Module a Package may name, versioned and written down at `docs/package-api.md`. The
build lints every Package file against it and fails on a path outside it, so a core refactor breaks a
Package at compile time rather than on a live realm. It is a compatibility contract, never a
sandbox: compiled Package code is trusted either way.
_Avoid_: SDK, plugin API, public API, allowlist

## Tests

**Seam**:
The interface where session or protocol handling hands work to durable state, expressed as a trait so tests can substitute the far side. Not the Shard Boundary, not any arbitrary trait.

**Protocol Family**:
A group of client opcodes one Gateway handler owns, with the Store trait that handler calls (for example the vendor family and `VendorActionStore`). `WorldStore` is the umbrella over every family; a handler takes only its family's Store.
_Avoid_: family (unqualified), domain

**Store**:
The durable side of a Seam: the trait a handler calls for Durable Reads and Durable Requests. The Coordinator implements it in production, a Fake in tests.
_Avoid_: repository, adapter, port, backend, service

**Fake**:
A working in-memory Store used by tests.
_Avoid_: harness, mock, stub

**Architecture Test**:
A test that fails the build when a structural rule is broken.
_Avoid_: tripwire, guard test

**Headless Client**:
A client that speaks the real 1.12.1 protocol to the Gateway in tests, with no UI.
_Avoid_: wire harness, test harness

**Verification**:
A written record of a targeted check of server behaviour against the real client or game data.
_Avoid_: probe
