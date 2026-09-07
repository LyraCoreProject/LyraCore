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

**EventAI Source Profile**:
A named, pinned EventAI input contract. It binds exact decompressed SQL bytes, the source loader,
source censuses, and approved compatibility results.

**Compatibility Manifest**:
The complete EventAI import account for one World Import Scope. It records each source value and
dependency path as emitted, normalized, excluded, dropped, or unapproved. An unapproved result is a
Refusal for apply and remains visible in dry run.

**Encounter Binding**:
The map-scoped link from an imported EventAI action to the package that owns the encounter. It also
decides who may tune that creature's catalogue: a Claim on a broadcast text or a summon placement an
encounter-bound definition depends on is refused, and the refusal names both the claim and the
binding.

**Encounter Signal**:
A named Begin, Fail, Complete, or encounter-specific notification delivered through an Encounter
Binding. Source numeric states do not cross this boundary.

**Relay Definition**:
A typed, versioned sequence of EventAI steps imported as one validated catalogue. This is Module
gameplay data, not a Gateway Relay.

**Relay Run**:
A durable invocation of one Relay Definition. It pins its catalogue and definition versions,
participants, next step, due time, and saved random state.

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

**Standalone Supervisor**:
The systemd unit that runs a host's `spacetimedb-standalone` process. The tracked artifact is
`deploy/systemd/spacetimedb-standalone.service`. It restarts every standalone exit, gives each
restart 524288 file descriptors, and appends standalone stderr to a durable log.

**Service Reconciliation**:
Making a host's Standalone Supervisor match the unit tracked in the checkout. `lyracore service
reconcile` performs it, reading every expected value out of the tracked unit. A host that does not
match afterwards is reported as NOT reconciled, never as a success.

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

**Pre-auth I/O Deadline**:
The absolute budget from socket acceptance until the peer proves itself: 10 s on the logon port and
15 s on the world port. It bounds reads and writes, and an independent watchdog closes the socket
even when its blocking task has not started. Proof completion clears it before post-auth traffic.
_Avoid_: pre-auth read deadline, read timeout, idle timeout (for this limit)

**Logon Limiter**:
The Gateway's in-memory caps on the logon port: three attempts per connection, ten failed logons
per address per minute, eight open logon connections per address, and a 200 ms pause before a
failed proof is answered. A refusal closes the socket. Per gateway process; a restart forgets it.
_Avoid_: rate limiter, throttle, brute-force lockout

**Session Expiry**:
The one hour a `game_session` row stays valid after its logon. The world handshake refuses an
expired row like an absent one, and the Module's `reap_sessions` deletes it. Every logon rewrites
the row, so a returning Account always starts a fresh hour.
_Avoid_: session timeout, TTL (in prose)

### Sharding and transfer

**Transfer**:
Moving a Character's state from one shard to another. Uses Escrow.

**Transfer Intent**:
A row a Package writes to ask the Gateway to Transfer a Character that has no Session, naming the
destination map and instance. The Package places the Character and records the intent in one
transaction; the Gateway drives the same escrowed Transfer a World Session would. Reaped on the
shared event TTL, so it is a request, never a record.
_Avoid_: transfer request, move order

**Group Intent**:
A row a Package writes to ask the Gateway to run one party operation for a Character that has no
Session: invite that Character, or leave the party it leads. Party membership is authoritative on
realm-core, which a Package can never reach, so the Package decides and the Gateway executes against
the correct authority. Reaped on the shared event TTL, so it is a request, never a record. Held in
`game_bot_invite_intent`, which kept its name through the change that gave it a second operation.
_Avoid_: invite intent, group request, party order

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

**Loot Tag**:
The first positive player-controlled threat on a live creature. It fixes the eligible party at that
instant and owns kill rewards and corpse eligibility until the creature leaves combat. The existing
`game_creature_quest_tap` and `game_creature_quest_tap_member` names are retained schema artifacts.

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
locations. Its Package Delta stage is global: no table in it names a map, so a Claim reaches every
Shard, exactly as the family's own base import writes it.

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
Lua the Module runs on a core gameplay event or a Package Event, supplied from outside the core
rather than compiled into it. Named so a diagnostic can identify it. It reaches a Shard only through
a Package's Script Artifact; there is no upload path.
_Avoid_: plugin, mod, addon, user script

**Package Event**:
An event a Package fires itself, spelled `<package>.<name>`. It runs the same dispatch a core hook
event runs, so a Package exposes one of its own decisions to a Runtime Script without a new core
seam. A Package may only bind events it fires, which the artifact parser enforces against the
artifact's own Package identity.
_Avoid_: custom event, user event, signal

**Script Answer**:
The number a Runtime Script returns, read back by the Package that asked. The first number returned
in dispatch order is the answer; later scripts still run and still stage what they stage. No answer
— nothing bound, nothing returning a number, or every script failing — leaves the caller on its own
fallback, which is what makes a Runtime Script an override rather than a dependency.
_Avoid_: return value, script result, callback

**Runtime Script Host**:
The Module's embedded Lua interpreter and the boundary around it: one compiler cache, a fresh
environment per invocation, a Fuel Budget, and the failure containment. Only place a Runtime
Script executes.
_Avoid_: sandbox, VM, engine

**Invocation**:
One run of one Runtime Script for one event. Starts from an environment holding nothing but the
allowlisted standard library, the event with its Entity Handles, and the Host Operations; ends by
committing its Staged Effects or by producing a Script Diagnostic. Nothing carries to the next one.

**Fuel Budget**:
The metered interpreter work one Invocation may spend before the Host cuts it off. The cut-off is a failure,
so a script that overruns changes nothing.
_Avoid_: quota, gas, instruction limit

**Entity Handle**:
The opaque reference a Runtime Script holds to one creature or player. It carries the identity the
Host acts on and the curated fields the script may read, but no guid and no row, so a script can
neither forge one nor name an entity the Host did not resolve for that Invocation. It lasts exactly
as long as the Invocation that minted it.
_Avoid_: entity id, guid, pointer, reference

**Host Operation**:
One named gameplay call the Runtime Script Host offers a script — today `heal`, `send_chat` and
`grant_xp`. Each takes an Entity Handle, records a Staged Effect, and refuses a misuse with a
Script Diagnostic naming the call and the fault.
_Avoid_: API function, binding, hook

**Staged Effect**:
A gameplay operation a Runtime Script asked for, recorded and not yet performed. A successful
Invocation commits its Staged Effects through core operations; any failure discards all of them.
_Avoid_: pending action, queued effect, side effect

**Script Diagnostic**:
The bounded record of a failed Invocation: the Runtime Script, the event, the failure kind
(syntax, runtime or fuel), and a truncated message. The only thing a failed Invocation produces.
_Avoid_: error log, stack trace

**Lethal Damage Floor**:
Combat-owned protection that reduces a creature's final lethal damage so it remains at one health.
It is applied after mitigation and absorbs, persists across Engagements, and is cleared by its
definition revision or creature lifetime.

**Forced Death**:
An authored request that bypasses the Lethal Damage Floor and enters the canonical creature-death
operation.

**Movement Intent**:
Durable authored movement selected by EventAI. The creature behavior cycle consumes it, and the
creature-leg writer remains the only position writer.

**Patrol Pause**:
The durable pause on an active patrol. It keeps the current waypoint cursor so resuming continues
the same route.

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
A drop-in folder under `packages/<name>/` that adds content to the realm with no core-file edits. Its `src/` is compiled into the Module wasm by the build's own discovery; its `client/` half supplies addons, whole-file client overrides and UI Transforms to the client packer. Either half alone is a valid Package.
Its data changes ship as Package Deltas rather than as edits to the base data.
_Avoid_: plugin, addon (when meaning the whole folder), mod, extension

**Package API**:
The part of the Module a Package may name, versioned and written down at `docs/package-api.md`: the
marker macros, the hook catalogue, the encounter kernel, the actor verbs and helpers, the Package
Config seam, the Package Event seam, the table accessor conventions, and the list of module roots
everything else hangs under. The build lints every Package file against it and fails on a path
outside it, so a core refactor breaks a Package at compile time rather than on a live realm. It is a
compatibility contract, never a sandbox: compiled Package code is trusted either way.
_Avoid_: SDK, plugin API, public API, allowlist

**Package Config**:
A row of `game_package_config`, keyed by `(package_name, key)`: one durable value a Package reads
and the Operator edits. A Package seeds its own defaults idempotently, from its own ensure/init
path, so the table always shows real keys with live values. The `set_package_config` reducer is the
Operator's edit path today; a CLI verb for it is tracked separately.
_Avoid_: config file, setting (unqualified), package setting

**Package Inventory**:
The two directories that hold installed Packages. `packages/` holds the enabled ones, which the build compiles. `.lyracore/packages-disabled/` holds the disabled ones, which it cannot see. A Package's location IS its enabled state; no file records it, so nothing can disagree with the disk about what the next build compiles. `lyracore packages enable` and `lyracore packages disable` move one folder between the two.
_Avoid_: registry, package list, enabled flag, state file

**Reference Package**:
The maintained, minimal Package at `packages/example/`, committed to the LyraCore repo and present in every checkout. It doubles as living documentation for a Package's shape and is the template `lyracore packages new` copies and renames. It is deliberately inert: Rust-only, one commented hook pattern, no gameplay behavior.
_Avoid_: template package, sample package

**Package Source**:
Where an installed Package was copied from: a local folder on the Operator's machine, a Git Package Source, or an Official Package Source. A scaffolded Package (`lyracore packages new`) has none. It was not copied from anywhere the Operator chose, so its Provenance Stamp records a scaffold origin instead.
_Avoid_: origin, upstream, repo

**Git Package Source**:
A repository whose root is one Package, named by a URL. `lyracore packages add <git-url>` clones it and installs a copy of its tree, without the `.git`. An installed Package is never a working copy, so `lyracore packages update` re-clones rather than pulling. A Package installed this way is Git-backed, and it is the only kind `update` can advance.
_Avoid_: git remote, upstream repo, checkout

**Official Package Collection**:
The one repository, `LyraCoreProject/packages`, that holds several first-party Packages side by side, one top-level directory each. `lyracore packages add <name>` resolves a bare Package name against it: a folder on the Operator's machine or a Git URL keeps its own rules, and only a name that matches neither falls back to the collection.
_Avoid_: registry, package repository, marketplace

**Official Package Source**:
The top-level directory of the Official Package Collection that a bare `lyracore packages add <name>` installed. Its Provenance Stamp records the collection's URL and the Recorded Revision the directory was resolved at, the same way a Git Package Source records its repository and commit. The commit is pinned at install time: `lyracore packages update` refuses this kind by name, so a later commit to the collection cannot silently change what is installed.
_Avoid_: registry entry, published package

**Recorded Revision**:
The exact commit a Git Package Source or an Official Package Source was installed at, held in its Provenance Stamp. `lyracore packages update` advances a Git Package Source from this rather than from whatever the repository's branch points at now, so it can name both commits when it reports or restores.
_Avoid_: version, tag, pin

**Provenance Stamp**:
The record `lyracore packages add` writes inside an installed Package: its Package Source, its Content Identity at install time, when it was installed, and, for a Git or Official Package Source, its Recorded Revision. A Package without one is still a Package; only its history is unknown.
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

**Authoring Library**:
The typed API at `datascripts/lib/` that a Datascript writes against: `data.spell(id)`, `.clone(newId)`, `.set(field, value)`, `.effect(0 | 1 | 2)`, and a `run(package, script)` that emits one Package Delta. It reads the Base Snapshot and refuses exactly what the artifact parser refuses, so a Datascript fails at author time rather than at import. Its source lives outside the Package folder; only the generated artifact goes inside.
_Avoid_: SDK, framework, DSL, builder API

**Base Snapshot**:
The read-only file of derived base rows a Datascript reads to clone and tune, one file per Import
Family — today the spell family's, written by
`lyracore-importer --dbc <dir> --spell-snapshot <path>`. It carries the same `game_*` values the
import would load — never client bytes — and is git-ignored, because it is derived from the
Operator's own client data. It is the ONLY base data a Datascript sees, which is what makes one
Package unable to observe another's claims.
_Avoid_: dump, base data file, cache

**Package Spell**:
A spell a Package invents, at an identifier inside the Package Spell Range. It exists only in the realm's data: an unmodified client renders spells from its own `Spell.dbc` and shows no tooltip for one.
_Avoid_: custom spell, fake spell

**Import Family**:
One named slice of base data the importer loads as a unit, cleared and reloaded whole — "spell",
"creatures", "quests". It is the granularity of import provenance (`game_import_meta`, one row per
family) and of a Package Delta apply: a family's Package claims are reapplied as the last stage of
that family's import, in one transaction. Every table a Package may claim belongs to exactly one
family, so an apply for one family never reaches another's rows.
_Avoid_: data family, import group, dataset

**Package Delta**:
The versioned artifact recording what one Package changes in the base data: the Package identity,
the source hash, and one Claim per row. Canonical JSON, so two artifacts that say the same thing are
byte-identical. A base import replaces whole data families, so deltas are a pipeline stage that
replays on every reload, never a one-shot edit.
A Package's generated artifacts live at `packages/<name>/data/.generated/*.json`, inside the Package
folder, so enabling or disabling the Package moves them with it. What the importer can see IS the
enabled set; there is no second list to disagree with the Package Inventory.
_Avoid_: patch, override file, diff

**Package Import**:
The record of what one Package contributed to the last apply of one Import Family: the artifact
digest, the source hash, the row counts, and the base import generation the claims sit on. One row
per family and Package, rewritten wholesale on every apply, so it answers "what Packages is this
shard running now" rather than "what did it ever run". Distinct from a Provenance Stamp, which
records where a Package was installed from.
_Avoid_: apply log, history, audit trail

**Build Identity**:
The recorded inputs `packages build` writes next to a source-built artifact, in a sibling file
rather than inside it. A Package Delta records its Datascript source tree, generated Module
typings, Base Snapshot, authoring library, and pinned Datascript toolchain. A source-built Script
Artifact records its `scripts/` sources, Runtime Script Toolchain, Bun pin, and artifact hash. A
source-free prebuilt Script Artifact has no local author inputs or Build Identity; the authoritative
checker still parses and traces it. `lyracore packages check` and preflight recompute a present
identity against the checkout on disk and refuse, naming the input that changed.
_Avoid_: identity file, build fingerprint, artifact metadata

**Claim**:
One Package's statement about one row: the table, the typed primary key, the operation, and the
columns it sets. An update names only the columns it changes; an insert carries the whole row. Row
deletion is not supported.
_Avoid_: edit, patch entry

**Claim Conflict**:
Two Packages claiming the same column of one row, or inserting the same primary key. Reported with
both Package identities and the exact claim. There are no priority numbers, so a human chooses.
_Avoid_: collision (for a claim), merge error

**Package Identifier Range**:
The identifiers a Package may invent in one Import Family. Each family that allows inserts owns one
band, floored two decimal orders above the highest identifier a real client holds for its tables and
clear of every reserved band. An apply clears the whole band before it writes, so a Package that
leaves the enabled set takes its invented rows with it. The Package Spell Range is the worked
example; the Package Item Range is the second family to follow it, and the Package Script Range is
the case where a table has no real client identifiers to clear. A family whose tables have no
Package-inventable owning identifier of their own. The Package Loot Range checks the band
against a row's own identifier instead of an owning one; a family whose child tables share their
header's owning identifier. The Package Quest Range checks every child through that one band
rather than owning a second.
_Avoid_: custom id range, synthetic id range

**Package Spell Range**:
The spell family's Package Identifier Range: 6,000,000 to 6,999,999. Two decimal orders above the
highest real client spell and above every reserved band, so an inserted spell can never collide with
imported or fixture data.
_Avoid_: custom id range, synthetic spell range

**Package Script Range**:
The script family's Package Identifier Range: 100,000 to 999,999. No client and no import holds a
Runtime Script identifier, so the band has no real data to clear and sits below every reserved band
rather than above one. It is the whole of `game_script` by construction, which is what makes a
script apply a total reconciliation.

**Script Artifact**:
The versioned artifact recording every Runtime Script one Package ships: the Package identity, the
source revision, and one whole row per script — identifier, name, Event Binding, priority, enabled
state, and Lua. Distinct from a Package Delta, which states columns of rows a base import owns: a
Runtime Script has no base import, so the Package owns the whole row and two Packages meeting on one
is a collision rather than a merge. Both kinds live in `packages/<name>/data/.generated/` and are
told apart by a top-level kind.
_Avoid_: script bundle, script manifest, script delta

**Event Binding**:
The event a Runtime Script runs for: a name from the Module's hook catalogue, or a Package Event of
the shipping Package. Anything else is refused at author time. Several scripts may bind to one
event: lower priority runs first and the script identifier breaks a tie, so every Shard runs one
plan in one order.
_Avoid_: hook registration, subscription, listener

**Script Directive**:
A `@key value` comment line at the top of a Runtime Script source, declaring what the file cannot
say in its own code: `@event` and `@id` are required, `@priority` and `@enabled` have defaults. The
identifier is written down rather than derived, because it is durable — deriving it from a file
index would renumber a Package's scripts the moment an author added one.
_Avoid_: annotation, frontmatter, pragma, metadata header

**Runtime Script Toolchain**:
The pinned compiler that turns a Package's `scripts/` sources into its Script Artifact: Bun plus
`typescript-to-lua`, its config, the hand-maintained Host API typings, and the emitter that keeps
generated Lua off the interpreter's known call-shape fault. It lives in
`datascripts/runtime-scripts/` and runs at author time only; an Operator installs the prebuilt Lua.
_Avoid_: transpiler, build pipeline, SDK

**Package Item Range**:
The items family's Package Identifier Range: 7,000,000 to 7,999,999. Above every reserved band, and
one whole decade above the Package Spell Range so the millions column stays a family-at-a-glance
signal across tables, not only within one.
_Avoid_: custom id range, synthetic item range

**Package Quest Range**:
The quest family's Package Identifier Range: 8,000,000 to 8,999,999. One whole decade above the
Package Item Range. Checked against `quest_entry` alone: `game_quest_template` and every child table
(`game_quest_text` and the rest) are Package-owned exactly when the quest they belong to is, so one
band covers the whole family the same way the Package Spell Range covers both `game_spell` and
`game_spell_effect`.
_Avoid_: custom id range, synthetic quest range

**Package Loot Range**:
The loot family's Package Identifier Range: 9,000,000 to 9,999,999. One whole decade above the
Package Quest Range. No loot table's owning entity (a creature, a gameobject, or a zone) is ever
Package-invented, so this band is checked against a loot row's own identifier instead of an owning
one, the same shape the Package Item Range checks against `game_item_template.entry`. Shared across
all four loot tables (pickpocket, gameobject/chest, skinning, fishing), which cannot collide on it:
each is an independent SpacetimeDB table with its own primary-key space.
_Avoid_: custom id range, synthetic loot range

**Package Cast Range**:
The casts family's Package Identifier Range: 10,000,000 to 10,999,999. One whole decade above the
Package Loot Range. Checked against `game_creature_spell.id` alone, the loot shape: its owning
creature is never Package-invented. `game_creature_cast` carries no range of its own — its primary
key names a creature template, which no Package may invent, so every insert on it is refused
outright rather than banded.
_Avoid_: custom id range, synthetic cast range

**Package Trainer Range**:
The trainers family's Package Identifier Range: 11,000,000 to 11,999,999. One whole decade above
the Package Cast Range. Checked against `game_trainer_spell.id`, the same loot shape. Distinct from
the curated trainer overrides the importer hands out fixed identifiers for at 5,200,000
(`CURATED_TRAINER_ID_BASE`), which is a reserved band this range clears, not a Package range.
_Avoid_: custom id range, synthetic trainer range

**Package Gossip Range**:
The gossip family's Package Identifier Range: 12,000,000 to 12,999,999. One whole decade above the
Package Trainer Range. One range covers all five insertable gossip tables — `game_npc_text`,
`game_npc_text_slot`, `game_gossip_option`, `game_gossip_menu_profile` and
`game_gossip_menu_profile_option` — the loot shape: independent primary-key spaces cannot collide by
sharing a range. `game_gossip_menu` carries no range: its key names a creature template, so every
insert on it is refused outright.
_Avoid_: custom id range, synthetic gossip range

**Package Globals Range**:
The globals family's Package Identifier Range: 13,000,000 to 13,999,999. One whole decade above the
Package Gossip Range. Covers the three tables of the family whose key is a free surrogate:
`game_graveyard_zone`, `game_createinfo_spell` and `game_createinfo_action`. The family's other four
tables carry no range because no Package may invent their keys: `game_class_level_stats`,
`game_level_stats` and `game_start_position` key on a race, class and level the client fixes, and
`game_areatrigger_teleport` keys on an `AreaTrigger.dbc` trigger id.
_Avoid_: custom id range, synthetic globals range

**Package Spell Metadata Range**:
The spellmeta family's Package Identifier Range: 14,000,000 to 14,999,999. One whole decade above the
Package Globals Range. Covers `game_spell_learn.id` alone. `game_spell_chain` and
`game_spell_proc_event` key on a spell identifier rather than a surrogate, so an insert there takes
the Package Spell Range instead: a metadata row cannot outlive the `game_spell` row it describes.
_Avoid_: custom id range, synthetic spellmeta range

**Package Creature Range**:
The creatures family's Package Identifier Range: 15,000,000 to 15,999,999. One whole decade above
the Package Spell Metadata Range. One range covers both insertable tables: a creature template's own
`entry` and a creature spawn claim's own `spawn_id`, which are independent identifier spaces. Its
ceiling has a second constraint no earlier range has — a creature spawn's durable guid packs the
template entry and the spawn identifier into 24-bit fields, so the whole range has to fit inside
one. The seeded creature fixtures at 51,000 to 51,999 are Fixture-Reserved Identifiers no Package
may tune. `game_creature_waypoint` is not claimable at all: it names its creature by spawn guid and
carries no map, so a Spatial Claim on it could not be routed.
_Avoid_: custom id range, synthetic creature range

**Package Gameobject Range**:
The gameobjects family's Package Identifier Range: 16,000,000 to 16,999,999. One whole decade above
the Package Creature Range. Covers three tables: `game_gameobject_template.entry`,
`game_gameobject_trap.entry` and a `game_gameobject` claim's own `spawn_id`. The first two share one
identifier space on purpose — a trap row describes the template of the same entry, so a Package trap
is exactly as Package-owned as its template. The two gameobject pool tables are not claimable: no
base import writes either, so a claim on one would have no family reload to replay after.
_Avoid_: custom id range, synthetic gameobject range

**Package EventAI Range**:
The Creature-AI Family's Package Identifier Range: 17,000,000 to 17,999,999. One whole decade above
the Package Gameobject Range. Covers three tables that share nothing else:
`game_creature_ai_broadcast_text.id`, `game_creature_ai_summon.id` and
`game_quest_event_requirement.id`. The family's scripted definitions are not claimable at all: a
definition carries a creature's whole rule set as a nested payload, which no claimed column can
state, and a Claim is typed rows rather than a script blob. Reaching a creature's rules from a
Package remains a named gap.
_Avoid_: custom id range, synthetic eventai range

**Spatial Claim**:
A Claim on a row that sits on one map: a creature spawn or a gameobject spawn. Its primary key names
the map as well as the row, and it reaches only the Shards whose World Import Scope owns that map.
The importer applies that fence with the scope it already built for the base import, so a Spatial
Claim needs no routing concept of its own; a claim for another Shard's map is dropped from this
Shard's plan and reported, never refused. Every other claimed table is a global catalogue every
Shard loads whole. The map never reaches the derived durable guid, which is what stops a Package
from moving a placed row onto a map another Shard owns.
_Avoid_: map claim, zoned claim, sharded claim

**Fixture-Reserved Identifier**:
An identifier the seeded fixtures own, which no Package may claim under any operation, in any Import
Family. Two kinds: the project-wide 5,090,000 to 5,099,999 band, and a family's own fixture cluster —
for spells, 50,000 to 50,999, and for creature TEMPLATE entries, 51,000 to 51,999. Items have no
fixture cluster of their own; their seeded fixtures ride real client entries or the project-wide
band, so it is the whole check. A cluster covers one identifier space only: a creature spawn
identifier takes the project-wide band alone, because real imported spawn identifiers run through
51,000 to 51,999.
_Avoid_: test id, reserved id (unqualified)

### Client content

**UI Transform**:
An anchored edit one Package makes to a stock FrameXML or GlueXML file, declared in
`packages/<name>/client/ui-transforms.json`. Each entry names a path, one anchor (`before`, `after`
or `replace`) that must occur exactly once in the Baseline, and the text to insert. Several Packages
may edit one file as long as their anchors do not overlap. `lyracore client sync` composes the
result against the Operator's own client; a whole-file override of the same path refuses the pack.
_Avoid_: patch, override, hook, FrameXML edit

**Baseline**:
The stock bytes of one client file, read out of the Operator's own UI archives and never out of
`patch-3.MPQ`, the packer's own previous output. A UI Transform's output is the Baseline with edits
applied, which makes it baseline-derived: it reaches the Operator's client through `client sync` and
never enters the Client Artifact.
_Avoid_: original, stock file, source file, vanilla file

**Client Artifact**:
The directory tree `lyracore client pack` builds for a player to copy over a stock client:
`Data/patch-3.MPQ`, `Interface/AddOns/<Name>/`, and a `lyracore-client-pack.json` manifest written
last, with an optional zip beside it. It carries package-authored bytes only. A baseline-derived
file is refused by name before the first byte is written, which is how the licensing firewall holds
without anyone inspecting the output.
_Avoid_: client patch, distribution, release bundle, player package

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
