# Core terms

These are the terms you meet in a first change. Code, comments, commits and pull requests use them,
and the words under _Avoid_ do not appear in new names or prose.

Each term lives in exactly one file. This page holds the terms a new contributor meets in a first
change, whatever part of the code it touches. [`CONTEXT.md`](./CONTEXT.md) holds every other term,
including retired ones, and its alphabetical index lists the terms of both files. A term for one
feature, one subsystem or one Operator task goes in `CONTEXT.md`. To move a term between the files,
move its entry and update the `CONTEXT.md` index. Never keep a copy in both.

## Where things run

<a id="native-host"></a>**Native Host**:
The SpacetimeDB process that accepts WoW TCP connections. It handles transport and cryptographic
scratch state, then applies the Module's ordered packet delivery instructions.

<a id="development-reload"></a>**Development reload**:
The local `dev watch` workflow that rebuilds Module code and publishes it while keeping the Native
Host and World Sessions running. The Module definition must stay unchanged.

<a id="realm"></a>**Realm**:
The game world and Accounts that clients see as one server. One Shard holds it.
_Avoid_: server, cluster

<a id="shard"></a>**Shard**:
The SpacetimeDB database that holds one Realm. Maps and Instances partition gameplay within it.
_Avoid_: database (when meaning a shard), db, cross-database

<a id="module"></a>**Module**:
The WebAssembly program that owns durable state, gameplay, AOI and native protocol handling in a
Realm's Shard.

<a id="operator"></a>**Operator**:
The identity that publishes and owns the Realm's Shard. The Native Host connects as the Operator,
and Operator reducers admit no other caller.

## Requests and delivery

<a id="actor"></a>**Actor**:
The Character a gameplay request acts as, identified by a nonzero guid. Native requests resolve it
from the authenticated World Session. Operator reducers take it as a SessionActor.

<a id="gate"></a>**Gate**:
A Module rule that refuses a request. This includes presence, name resolution and recipient selection.
_Avoid_: validation, guard

<a id="refusal"></a>**Refusal**:
A Gate saying no to a Durable Request. An expected gameplay outcome, not a Transport Loss.
_Avoid_: reject, deny, error (for gameplay refusals)

<a id="transport-loss"></a>**Transport Loss**:
A failed Durable Read or Durable Request that is not a Refusal, such as an unexpected reducer
error. The Native Host ends the World Session, and the client logs in again from durable state.
_Avoid_: transport failure, fatal error, disconnect

<a id="durable-request"></a>**Durable Request**:
A request whose state changes commit in a Module transaction.

<a id="durable-read"></a>**Durable Read**:
A read of committed Module state.

<a id="relay"></a>**Relay**:
Module code that turns a state change into a client message.
_Avoid_: forwarder, pusher

<a id="aoi"></a>**AOI**:
The area of interest that decides which entities a World Session sees.
_Avoid_: visibility set, interest radius

<a id="aoi-grant"></a>**AOI Grant**:
A private Module row that permits a Character to observe an entity. Its id stays unchanged while
that permission holds. WorldEntity, GameObject, Corpse and DynamicObject grants use separate layers.

## Characters and Sessions

<a id="account"></a>**Account**:
A login. Owns characters.

<a id="character"></a>**Character**:
A guid-owned player entity.
The public Runtime Script authoring API calls a Character `player`, as in `events.player`,
`event.player`, `PlayerEntity` and `PlayerLoginEvent`. The public event payload uses the same
word, for example `victimIsPlayer`. Core code uses Character.
_Avoid_: player as a noun in Core code outside that authoring contract

<a id="session"></a>**Session**:
The Module's record that an Account is logged in on a Character.

<a id="world-session"></a>**World Session**:
One authenticated connection on the world port, with its request state in the Module.
_Avoid_: session (unqualified, when meaning the connection)

## Extending the server

<a id="package"></a>**Package**:
A drop-in folder under `packages/<name>/` that adds content to the Realm with no core-file edits.
Any one of its parts is enough: Rust in `src/` that the build compiles into the Module, a `client/`
half for the client packer, Runtime Scripts in `scripts/` or Datascripts in `datascripts/`. Its data
changes ship as Package Deltas, and its own tests live with it.
_Avoid_: plugin, addon (when meaning the whole folder), mod, extension

<a id="package-api"></a>**Package API**:
The part of the Module a Package may name, versioned and written down in `docs/package-api.md`. The
`package_api_lint` test checks every Package Rust file against it. It is a compatibility contract,
never a sandbox. Compiled Package code is trusted either way.
_Avoid_: SDK, plugin API, public API, allowlist

<a id="runtime-script"></a>**Runtime Script**:
Lua the Module runs on a core gameplay event or a Package Event. It comes from outside the core
rather than being compiled into it, and it reaches a Shard only through a Package's Script Artifact.
_Avoid_: plugin, mod, addon, user script

## Tests

<a id="seam"></a>**Seam**:
An interface between caller behavior and a dependency that tests can substitute.

<a id="protocol-family"></a>**Protocol Family**:
Related client opcodes handled by one Module component.
_Avoid_: family (unqualified), domain

<a id="store"></a>**Store**:
The durable side of a Seam, represented by a trait when tests substitute a Fake.
_Avoid_: repository, adapter, port, backend, service

<a id="fake"></a>**Fake**:
A working in-memory Store used by tests.
_Avoid_: harness, mock, stub

<a id="architecture-test"></a>**Architecture Test**:
A test that fails the build when a structural rule is broken.
_Avoid_: tripwire, guard test

<a id="headless-client"></a>**Headless Client**:
A client that speaks the real 1.12.1 protocol to the Native Host in tests, with no UI.
_Avoid_: wire harness, test harness

<a id="verification"></a>**Verification**:
A written record of a targeted check of server behaviour against the real client or game data.
_Avoid_: probe
