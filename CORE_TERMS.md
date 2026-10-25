# Core terms

These are the terms you meet in a first change. Code, comments, commits and pull requests use them,
and the words under _Avoid_ do not appear in new names or prose. The full glossary, with an
alphabetical index, is [`CONTEXT.md`](./CONTEXT.md). This page copies its entries, so when a term
changes in one file, change it in the other.

## Where things run

**Native Host**:
The SpacetimeDB process that accepts WoW TCP connections. It handles transport and cryptographic
scratch state, then applies the Module's ordered packet delivery instructions. The native runtime
uses one Shard for all Realm state.

**Realm**:
The game world and Accounts that clients see as one server. The native runtime stores a Realm in
one Shard.
_Avoid_: server, cluster

**Shard**:
One SpacetimeDB database containing a Realm. Maps and Instances partition gameplay within it.
_Avoid_: database (when meaning a shard), db, cross-database

**World Shard**:
A retired topology role that owned a set of maps. Native Realms keep all maps in one Shard.

**Instance Pool**:
A retired topology role that hosted instanced maps. Native Instances share the Realm's Shard.

**Realm-core**:
The retired Shard for realm-wide state. Native Realms store that state beside Characters in one Shard.

**Home Shard**:
The Shard that contains a Character. Every Character in a native Realm has the same Home Shard.

**Module**:
The WebAssembly program that owns durable state, gameplay, AOI and native protocol handling in a
Realm's Shard.

**Gateway**:
The removed protocol runtime between clients and Shards. Its request handling and AOI run in the
Module; the Native Host handles transport.

**Operator**:
The identity that publishes and owns the Realm's Shard, and the only caller of Gateway Verbs.

## Requests and delivery

**Gateway Verb**:
A `gw_*` reducer the Operator calls on a character's behalf, with the Actor named by guid.

**Actor**:
The Character a gameplay request acts as, identified by a nonzero guid. Native requests resolve it
from the authenticated World Session; Operator reducers accept a SessionActor.

**Gate**:
A Module rule that refuses a request. This includes presence, name resolution and recipient selection.
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
A request whose state changes commit in a Module transaction.

**Durable Read**:
A read of committed Module state.

**Coordinator**:
The removed Gateway's subscribed connection to a Shard. The Native Host uses in-process connections
and the Module reads its own state.

**Relay**:
Code that turns a state change into a client message. Native Relays run in the Module.
_Avoid_: forwarder, pusher

**Speech**:
A line a Character speaks to the Characters near it: a say, yell or `/e` line, or a text emote. Its
Durable Request goes to the speaker's Home Shard, and a Relay delivers it to listeners in range.
_Avoid_: local chat, proximity chat

**AOI**:
The area of interest that decides which entities a World Session sees.
_Avoid_: visibility set, interest radius

**AOI Grant**:
A private Module row that permits a Character to observe an entity. Its id stays unchanged while
that permission holds. WorldEntity, GameObject, Corpse and DynamicObject grants use separate layers.

## Characters and Sessions

**Account**:
A login. Owns characters.

**Character**:
A guid-owned player entity.
The public Runtime Script authoring API calls a Character `player`, as in `event.player` and
`PlayerLoginEvent`. Core code uses Character.
_Avoid_: player as a noun in Core code outside that authoring contract

**Session**:
The Module's record that an Account is logged in on a Character.

**World Session**:
One authenticated connection on the world port, with its request state in the Module.
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
named `package_api_lint` test checks every Package Rust file against it and reports paths outside
that contract. CI runs it with the Official Packages installed. It is a compatibility contract, never a
sandbox: compiled Package code is trusted either way.
_Avoid_: SDK, plugin API, public API, allowlist

## Tests

**Seam**:
An interface between caller behavior and a dependency that tests can substitute.

**Protocol Family**:
Related client opcodes handled by one Module component.
_Avoid_: family (unqualified), domain

**Store**:
The durable side of a Seam, represented by a trait when tests substitute a Fake.
_Avoid_: repository, adapter, port, backend, service

**Fake**:
A working in-memory Store used by tests.
_Avoid_: harness, mock, stub

**Architecture Test**:
A test that fails the build when a structural rule is broken.
_Avoid_: tripwire, guard test

**Headless Client**:
A client that speaks the real 1.12.1 protocol to the Native Host in tests, with no UI.
_Avoid_: wire harness, test harness

**Verification**:
A written record of a targeted check of server behaviour against the real client or game data.
_Avoid_: probe
