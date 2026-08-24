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

**World Import Profile**:
A stable name for the import plan assigned to one World Shard or Instance Pool destination. The
canonical profiles are `alliance-eastern`, `alliance-kalimdor`, `alliance-single`, and `instances`.

**World Import Scope**:
The authoritative union of Bounded Map Slices, whole maps, and forced creature dependencies owned
by one World Import Profile. Accepted EventAI summons force their summoned templates into the
scope, to a fixpoint. It decides spatial import membership for dump, terrain, navigation,
and vmap modes.

**Bounded Map Slice**:
A named rectangular or circular part of one map, with the anchor used for terrain and navigation
selection. The anchor is a real ground point on the client heightmap inside the slice, not a WMO
floor; the terrain and navigation self-checks fail before `--apply` when it is not.

**Realm-core**:
The shard that holds realm-wide state, including accounts, sessions, groups, whispers, loot rolls, the character-to-shard index, and shard load samples.

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

**Proficiency**:
What a Character may wield and wear: its class weapon table plus the armor tiers it has reached. Armor tiers above the class base set are trained at a class trainer, so knowing the passive spell is the proficiency. Derived once in `lyracore-shared`; the Module equip Gate and the Gateway's `SMSG_SET_PROFICIENCY` mask both read that derivation.

**Coordinator**:
The Gateway's subscribed connection per shard, authenticated with the Owner Token. It serves every Durable Read; a small pool of call pipes carries Durable Requests.

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

**Alpha Test Tools**:
Account-owned authority for a limited set of alpha testing dot-commands. The Gateway reads its
current value from Realm-core for every command and conveys it to the Home Shard. The Module applies
the final Gate.

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

### Mounts

**Land Mount**:
A ground mount, held as an ordinary cancelable self aura. The aura row is the mounted state.
_Avoid_: mounted state (as a separate stored thing)

**Mount Projection**:
The `WorldEntity` columns re-derived from a Character's aura set for the client: the mount display and
the effective run speed. Never a second state machine.

**Dismount**:
The one shared operation that removes the active Land Mount's aura rows and re-derives the Mount
Projection. Idempotent, and a no-op for a rider who is not mounted.

### Procs

**Proc**:
An aura that fires off a combat event, with the event mask, chance, charges and internal cooldown its spell data gives it. One engine, one pass, run at the damage chokepoint.
_Avoid_: on-hit trigger, reactive aura

**Carrier**:
The unit wearing a Proc aura. A fired Proc casts from the Carrier, at the Carrier's frozen aura level.
_Avoid_: proc owner, wearer

**Counterparty**:
The unit on the other side of the hit that fired a Proc: the victim of a dealt event, the attacker of a taken one.
_Avoid_: other unit, opponent

**Triggered Cast**:
The cast a fired Proc starts. It runs the cast core's effect loop and nothing else: no Gate, no cost, no cooldown, no stealth break. Its own hits fire no further Procs.
_Avoid_: proc cast, internal cast, free cast

### Creature AI

**Creature-AI Family**:
The import family that loads the EventAI catalogue: event rows, broadcast texts, and summon
locations.

**Engagement**:
One creature's fight, from the aggro that starts it until the creature is freed, however that
happens. Numbered per creature; a new Engagement re-arms once-only rules and drops the phase and
the Ranged Posture.

**Rule State**:
The durable arming of one EventAI rule on one creature: its next eligible time and whether it is
consumed, keyed to the creature's lifecycle and Engagement.

**Flat Cast**:
The spell use a creature derives from its `game_creature_spell` rows alone: the rotation and the
lone spell, cast on cooldown with no authored condition. Off for an Authored Casting creature.

**Fixed Rout**:
The built-in break-off: a creature below the flee threshold and of a kind that runs opens one rout
window per Engagement. Off for an Authored Flee creature.

**Authored Combat**:
The halves of a creature's fight an imported EventAI script has taken over: Authored Casting and
Authored Flee. A property of the script's rows, not of the moment; eligibility and conditions do
not move it mid-fight.

**Authored Casting**:
An engaged EventAI cast rule exists for the creature, so its Flat Cast and caster hold range are
off and it closes to melee between authored casts unless a Ranged Posture holds it back.

**Authored Flee**:
An engaged EventAI flee rule exists for the creature, so the Fixed Rout is off and the rule's own
window runs the creature whatever its health or kind, as often as the rule fires.

**Ranged Posture**:
An authored stance holding a creature at a scripted distance and angle from its victim instead of
the melee approach. Set by the script, dropped with the Engagement.

### Runtime Scripts

**Runtime Script**:
Lua the Module runs on a core gameplay event, supplied from outside the core rather than compiled
into it. Named so a diagnostic can identify it.
_Avoid_: plugin, mod, addon, user script

**Runtime Script Host**:
The Module's embedded Lua interpreter and the boundary around it: one compiler cache, a fresh
environment per invocation, a Fuel Budget, and the failure containment. Only place a Runtime
Script executes.
_Avoid_: sandbox, VM, engine

**Invocation**:
One run of one Runtime Script for one event. Starts with an empty environment and ends by
committing its Staged Effects or by producing a Script Diagnostic. Nothing carries to the next one.

**Fuel Budget**:
The metered VM work one Invocation may spend before the Host cuts it off. The cut-off is a failure,
so a script that overruns changes nothing.
_Avoid_: quota, gas, instruction limit

**Staged Effect**:
A gameplay operation a Runtime Script asked for, recorded and not yet performed. A successful
Invocation commits its Staged Effects through core operations; any failure discards all of them.
_Avoid_: pending action, queued effect, side effect

**Script Diagnostic**:
The bounded record of a failed Invocation: the Runtime Script, the event, the failure kind
(syntax, runtime or fuel), and a truncated message. The only thing a failed Invocation produces.
_Avoid_: error log, stack trace

### Auctions

**Settlement**:
Resolving an auction at buyout or expiry: item and gold to their final owners, displaced bids refunded.
_Avoid_: resolve, close

### World clock and weather

**Realm Clock**:
The wall clock the realm runs on, always UTC. There is no realm-timezone setting, so no part of the realm reads host-local time. The Gateway packs it into `SMSG_LOGIN_SETTIMESPEED` once per world entry and the client advances it alone afterwards; the Module reads the same clock to pick the weather season.
_Avoid_: server time, game time, local time

**Zone Weather**:
One durable row per zone holding the sky that zone currently shows. The same row is the state a world entry reads and the source a live Relay fires from, so there is no second weather table and no replay ambiguity for a reconnecting client. A zone with no row has fine weather.
_Avoid_: weather event, weather state table

### Packages

**Package**:
A drop-in folder under `packages/<name>/` that adds content to the realm with no core-file edits. Its `src/` is compiled into the Module wasm by the build's own discovery; its `client/` half supplies addons and client overrides to the client packer. Either half alone is a valid Package.
Its data changes ship as Package Deltas rather than as edits to the base data.
_Avoid_: plugin, addon (when meaning the whole folder), mod, extension

**Reference Package**:
The maintained, minimal Package at `packages/example/`, committed to the LyraCore repo and present in every checkout. It doubles as living documentation for a Package's shape and is the template `lyracore packages new` copies and renames. It is deliberately inert: Rust-only, one commented hook pattern, no gameplay behavior.
_Avoid_: template package, sample package

**Package Source**:
Where an installed Package was copied from. Today only a local folder on the Operator's machine; git URLs and Official Packages are separate work. A scaffolded Package (`lyracore packages new`) has none. It was not copied from anywhere the Operator chose, so its Provenance Stamp records a scaffold origin instead.
_Avoid_: origin, upstream, repo

**Provenance Stamp**:
The record `lyracore packages add` writes inside an installed Package: its Package Source, its Content Identity at install time, and when it was installed. A Package without one is still a Package; only its history is unknown.
_Avoid_: manifest, lockfile, metadata

**Content Identity**:
A hash of an installed Package's files. Comparing the recorded one against the tree on disk is what "locally drifted" means.
_Avoid_: checksum, fingerprint, version

**Trust Review**:
The deterministic, read-only inventory `packages add` prints before it asks: what the candidate Package registers, counted the way the build counts it. It is an inventory, never a security verdict. A Package's unclassified Rust is trusted code.
_Avoid_: audit, scan, security check

**Datascript**:
Author-time TypeScript that describes game data, written against typings generated from the Module schema. It runs on the author's machine under Bun, never on the realm, and it is trusted code the author wrote — not sandboxed code. Distinct from a Runtime Script, which would execute inside the realm.
_Avoid_: data script, script (unqualified), seed script, sandbox

**Reference Datascript**:
The maintained, minimal Datascript at `datascripts/src/reference.ts`. It names real Module columns on purpose: it is both the worked example and the standing schema check that fails `tsc --noEmit` when the schema moves under it.
_Avoid_: sample script, test script

**Package Delta**:
The versioned artifact recording what one Package changes in the base data: the Package identity,
the source hash, and one Claim per row. Canonical JSON, so two artifacts that say the same thing are
byte-identical. A base import replaces whole data families, so deltas are a pipeline stage that
replays on every reload, never a one-shot edit.
_Avoid_: patch, override file, diff

**Claim**:
One Package's statement about one row: the table, the typed primary key, the operation, and the
columns it sets. An update names only the columns it changes; an insert carries the whole row. Row
deletion is not supported.
_Avoid_: edit, patch entry

**Claim Conflict**:
Two Packages claiming the same column of one row, or inserting the same primary key. Reported with
both Package identities and the exact claim. There are no priority numbers, so a human chooses.
_Avoid_: collision (for a claim), merge error

**Package Spell Range**:
The spell identifiers a Package may invent: 6,000,000 to 6,999,999. Two decimal orders above the
highest real client spell and above every reserved band, so an inserted spell can never collide with
imported or fixture data.
_Avoid_: custom id range, synthetic spell range

**Fixture-Reserved Identifier**:
An identifier the seeded fixtures own, which no Package may claim under any operation. For spells:
50,000 to 50,999, plus the project-wide 5,090,000 to 5,099,999 band.
_Avoid_: test id, reserved id (unqualified)

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
