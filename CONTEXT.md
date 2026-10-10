# LyraCore glossary

The full glossary, for lookup. A contributor reads [`CORE_TERMS.md`](./CORE_TERMS.md) first: it holds
the terms met in a first change. When a term changes in one file, change it in the other.

A World of Warcraft 1.12.1 server. Game state lives in SpacetimeDB; the gateway speaks the vanilla wire protocol to unmodified clients. Standard WoW vocabulary (guid, opcode, aura, gossip, master looter, round-robin, ...) keeps its client meaning and is listed here only where LyraCore narrows or changes it.

## Index

**A** [Account](#accounts-characters-and-sessions) · [Account Character Owner](#accounts-characters-and-sessions) · [Account Claim](#accounts-characters-and-sessions) · [Account Fence](#accounts-characters-and-sessions) · [Action Outcome](#procs) · [Actor](#accounts-characters-and-sessions) · [Alpha Test Tools](#accounts-characters-and-sessions) · [AOI](#gateway-and-module) · [Architecture Test](#working-method) · [Assistant](#sharding-and-transfer) · [Auction Cut](#auctions) · [Auction Mail](#auctions) · [Auction Market](#auctions) · [Auction Notice](#auctions) · [Authored Casting](#creature-ai) · [Authored Combat](#creature-ai) · [Authored Flee](#creature-ai) · [Authoring Library](#packages) · [Auto-Reply](#chat) · [Away Status](#chat)

**B** [Base Snapshot](#packages) · [Baseline](#client-content) · [Bot Capacity Lease](#sharding-and-transfer) · [Bot Controller](#sharding-and-transfer) · [Bot Objective](#sharding-and-transfer) · [Bounded Map Slice](#realm-topology) · [Build Identity](#packages)

**C** [Cancellation](#auctions) · [Candidate](#sharding-and-transfer) · [Carrier](#procs) · [Cast Handle](#procs) · [Channel Membership](#chat) · [Channel Notice](#chat) · [Character](#accounts-characters-and-sessions) · [Chat Channel](#chat) · [Chat Flood Limiter](#chat) · [Chat Kind](#chat) · [Claim](#packages) · [Claim Conflict](#packages) · [Client Artifact](#client-content) · [Command Receipt](#sharding-and-transfer) · [Companion Order](#sharding-and-transfer) · [Compatibility Manifest](#realm-topology) · [Content Identity](#packages) · [Coordinator](#gateway-and-module) · [Counterparty](#procs) · [Creature-AI Family](#creature-ai)

**D** [Datascript](#packages) · [Delivery Delay](#mail) · [Dismount](#mounts) · [Distraction](#creature-ai) · [Dormant Character](#accounts-characters-and-sessions) · [Durable Read](#gateway-and-module) · [Durable Request](#gateway-and-module)

**E** [Encounter Binding](#realm-topology) · [Encounter Signal](#realm-topology) · [Engagement](#creature-ai) · [Entity Handle](#runtime-scripts) · [Escrow](#sharding-and-transfer) · [Event Binding](#packages) · [EventAI Source Profile](#realm-topology)

**F** [Fake](#working-method) · [Fee Hold](#guilds) · [Fixed Rout](#creature-ai) · [Fixture-Reserved Identifier](#packages) · [Flat Cast](#creature-ai) · [Forced Death](#creature-ai) · [Foreground Action](#sharding-and-transfer) · [Fuel Budget](#runtime-scripts)

**G** [GameObject Collider](#gameobject-collision) · [Gate](#gateway-and-module) · [Gateway](#realm-topology) · [Gateway Verb](#gateway-and-module) · [Git Package Source](#packages) · [Group](#sharding-and-transfer) · [Group Audience](#sharding-and-transfer) · [Group Broadcast](#sharding-and-transfer) · [Group Intent](#sharding-and-transfer) · [GUID Range](#sharding-and-transfer) · [Guild](#guilds) · [Guild Charter](#guilds) · [Guild Emblem](#guilds) · [Guild Event](#guilds) · [Guild Invite](#guilds) · [Guild Leader](#guilds) · [Guild Projection](#guilds) · [Guild Rank](#guilds)

**H** [Headless Client](#working-method) · [Hold](#auctions) · [Home Shard](#sharding-and-transfer) · [Host Operation](#runtime-scripts)

**I** [Idle Bot](#sharding-and-transfer) · [Import Family](#packages) · [Instance Pool](#realm-topology) · [Instance Removal](#sharding-and-transfer) · [Instance Vmap Slice](#realm-topology) · [Invocation](#runtime-scripts) · [Item Exchange](#procs) · [Item Text](#mail)

**L** [Land Mount](#mounts) · [Lethal Damage Floor](#creature-ai) · [Letter Copy](#mail) · [Logon Limiter](#accounts-characters-and-sessions) · [Loot Release](#loot) · [Loot Roll](#loot) · [Loot Roll Promotion](#loot) · [Loot Roll Promotion Receipt](#loot) · [Loot Source](#loot) · [Loot Tag](#loot) · [Loot Window](#loot)

**M** [Mail](#mail) · [Mail Arrival](#mail) · [Mail Expiry](#mail) · [Mail Sender](#mail) · [Mail Template](#mail) · [Mail Timer](#mail) · [Meeting Stone](#meeting-stones) · [Meeting Stone Queue](#meeting-stones) · [Member Stats](#sharding-and-transfer) · [Module](#realm-topology) · [Mount Projection](#mounts) · [Movement Intent](#creature-ai)

**N** [Navigation Inputs](#sharding-and-transfer)

**O** [Officer Note](#guilds) · [Official Package Collection](#packages) · [Official Package Source](#packages) · [Open Role](#meeting-stones) · [Operator](#realm-topology) · [Owner Token](#gateway-and-module)

**P** [Package](#packages) · [Package API](#packages) · [Package Cast Range](#packages) · [Package Config](#packages) · [Package Creature Range](#packages) · [Package Delta](#packages) · [Package Event](#runtime-scripts) · [Package EventAI Range](#packages) · [Package Fixture](#packages) · [Package Gameobject Range](#packages) · [Package Globals Range](#packages) · [Package Gossip Range](#packages) · [Package Identifier Range](#packages) · [Package Import](#packages) · [Package Inventory](#packages) · [Package Item Range](#packages) · [Package Loot Range](#packages) · [Package Quest Range](#packages) · [Package Script Range](#packages) · [Package Source](#packages) · [Package Spell](#packages) · [Package Spell Metadata Range](#packages) · [Package Spell Range](#packages) · [Package Teardown](#packages) · [Package Trainer Range](#packages) · [Package-owned Account](#accounts-characters-and-sessions) · [Party](#sharding-and-transfer) · [Party Partition](#sharding-and-transfer) · [Patrol Pause](#creature-ai) · [Petition](#guilds) · [Petitioner](#guilds) · [Pre-auth I/O Deadline](#accounts-characters-and-sessions) · [Proc](#procs) · [Proficiency](#gateway-and-module) · [Property Pool](#random-properties) · [Protocol Family](#working-method) · [Prototype](#working-method) · [Provenance Stamp](#packages) · [Provisioning Profile](#sharding-and-transfer) · [Public Note](#guilds)

**R** [Raid](#sharding-and-transfer) · [Raid Quest](#sharding-and-transfer) · [Raid Slot](#sharding-and-transfer) · [Random Property](#random-properties) · [Ranged Posture](#creature-ai) · [Rank Rights](#guilds) · [Ready Check](#sharding-and-transfer) · [Realm](#realm-topology) · [Realm Chat Line](#chat) · [Realm Clock](#world-clock-and-weather) · [Realm Presence](#chat) · [Realm-core](#realm-topology) · [Recorded Revision](#packages) · [Recovery Attempt](#sharding-and-transfer) · [Recovery Scan](#sharding-and-transfer) · [Reference Datascript](#packages) · [Reference Package](#packages) · [Refusal](#gateway-and-module) · [Relay](#gateway-and-module) · [Relay Definition](#realm-topology) · [Relay Run](#realm-topology) · [Returned Mail](#mail) · [Reward Letter](#mail) · [Roster Revision](#sharding-and-transfer) · [Roster Revision Relay](#sharding-and-transfer) · [Route Path](#procs) · [Route Step](#procs) · [Rule State](#creature-ai) · [Runtime Script](#runtime-scripts) · [Runtime Script Host](#runtime-scripts) · [Runtime Script Toolchain](#packages)

**S** [Script Answer](#runtime-scripts) · [Script Artifact](#packages) · [Script Diagnostic](#runtime-scripts) · [Script Directive](#packages) · [Seam](#working-method) · [Seeker](#meeting-stones) · [Self-Resurrection Option](#death-and-resurrection) · [Service Reconciliation](#realm-topology) · [Session](#accounts-characters-and-sessions) · [Session Expiry](#accounts-characters-and-sessions) · [SessionActor](#accounts-characters-and-sessions) · [Sessionless Action Consent](#sharding-and-transfer) · [Settlement](#auctions) · [Shard](#realm-topology) · [Shard Boundary](#sharding-and-transfer) · [Shard Map](#sharding-and-transfer) · [Signature](#guilds) · [Solo Target Claim](#sharding-and-transfer) · [Soulstone](#death-and-resurrection) · [Spatial Claim](#packages) · [Speaker Facts](#chat) · [Spec](#working-method) · [Speech](#chat) · [Spell Cast Event Kind](#procs) · [Staged Effect](#runtime-scripts) · [Standalone Supervisor](#realm-topology) · [Stat Kind](#random-properties) · [Stone Add](#meeting-stones) · [Store](#working-method) · [Strategy](#sharding-and-transfer) · [Subgroup](#sharding-and-transfer) · [Suffix](#random-properties)

**T** [Tabard Designer](#guilds) · [Target Icon](#sharding-and-transfer) · [Ticket](#working-method) · [Tracer](#working-method) · [Trade Commit](#trading) · [Trade Session](#trading) · [Transfer](#sharding-and-transfer) · [Transfer Intent](#sharding-and-transfer) · [Transport Loss](#gateway-and-module) · [Triggered Cast](#procs) · [Trust Review](#packages)

**U** [UI Transform](#client-content)

**V** [Verification](#working-method)

**W** [Whereabouts](#chat) · [Will-Not-Be-Traded Slot](#trading) · [World Import Profile](#realm-topology) · [World Import Scope](#realm-topology) · [World Session](#accounts-characters-and-sessions) · [World Session Token](#accounts-characters-and-sessions) · [World Shard](#realm-topology)

**Z** [Zone Weather](#world-clock-and-weather)

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
canonical profiles are `alliance-eastern`, `alliance-kalimdor`, `alliance-single`,
`starting-eastern`, `starting-kalimdor`, and `instances`. The starting profiles retain the Alliance
scope and add the Horde starting areas on the same continent.

**World Import Scope**:
The authoritative union of Bounded Map Slices, Instance Vmap Slices, whole maps, and forced
creature dependencies owned by one World Import Profile. Accepted EventAI summons force their
summoned templates into the scope, to a fixpoint. It decides spatial import membership for dump,
terrain, navigation, and vmap modes.

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

**Relay Arrival**:
The relay a `move-dynamic` step schedules for when its mover lands. Its arrival leg is the leg that
step started. While the mover's live leg is still the arrival leg, the arrival settles the mover on
the destination and runs. A newer leg drops it. A mover with no leg must stand on the destination.

**Bounded Map Slice**:
A named rectangular or circular part of one map, with the anchor used for terrain and navigation
selection. The anchor is a real ground point on the client heightmap inside the slice, not a WMO
floor; the terrain and navigation self-checks fail before `--apply` when it is not.

**Instance Vmap Slice**:
A named entry and exit route on an ADT-backed instance map, with an exit radius and a cell collar
for collision extraction. It owns only the selected vmap cells. It does not claim terrain,
Navigation Coverage, or the rest of the instance.

**Realm-core**:
The shard that holds realm-wide state: accounts, sessions, Account Claims, groups, guilds, Chat Channels, Realm Chat Lines, mail, auctions, loot rolls, the character-to-shard index and shard load samples. It holds no Characters.

**Gateway**:
The trusted protocol tier between clients and shards. Holds no durable state.

**Module**:
The wasm that holds all durable state and all game logic. The same wasm runs on every shard.

**Operator**:
The identity that publishes and owns the shards, and the only caller of Gateway Verbs.

**Standalone Supervisor**:
The systemd unit that runs a host's `spacetimedb-standalone` process. The tracked artifact is
`deploy/systemd/spacetimedb-standalone.service`. It waits thirty seconds before restarting an exit,
allows five starts in five minutes, gives each restart 524288 file descriptors, and appends
standalone stderr to a durable log. Exhausting the start limit requires Operator recovery.

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
A Gate saying no to a Durable Request. An expected gameplay outcome, not a Transport Loss.
_Avoid_: reject, deny, error (for gameplay refusals)

**Transport Loss**:
A failed Durable Read or Durable Request that is not a Refusal: the transport dropped, the call
timed out, or the send failed. Its durable outcome is unknown, so it ends the World Session and the
client logs in again from durable state.
_Avoid_: transport failure, fatal error, disconnect

### Accounts, characters and sessions

**Account**:
A login. Owns characters.

**Package-owned Account**:
An Account a Package created for its session-less Characters through the Package API. It has no
credentials, so no login reaches it. `game_package_account` records the owning Package in Core, so
the record outlives the Package when the Package is disabled.
_Avoid_: bot account, account block

**Dormant Character**:
A Character whose Package was torn down. It is offline and has no live entity, Sessionless Action
Consent or pending Intent. Its Account and Character rows stay, by maintainer decision: turning a
Package off never deletes its Characters. A Package enabled again does not adopt them.
_Avoid_: deleted bot, orphan bot, frozen bot

**Alpha Test Tools**:
Account-owned authority for a limited set of alpha testing dot-commands. The Gateway reads its
current value from Realm-core for every command and conveys it to the Home Shard. The Module applies
the final Gate.

**Character**:
A guid-owned player entity.
_Avoid_: player (as a noun in code)

**Actor**:
The Character a Gateway Verb acts as. In the Gateway it is the `Actor` type, a nonzero guid.
`Actor::new` returns `None` for guid 0, so no request acts as guid 0. The Coordinator signs an Actor
into a SessionActor.

**Session**:
The Module's record that an Account is logged in on a Character.

**World Session**:
The Gateway's per-connection loop for one client on the world port.
_Avoid_: session (unqualified, when meaning the connection)

**Account Claim**:
Realm-core's time-limited ownership of an Account by one World Session. A live claim refuses a
competing login. Each replacement advances the retained generation.

**Account Fence**:
A World Shard's retained Account Claim generation. Admission installs it on every configured World
Shard. The Module checks it in the transaction that acts on the Character.

**Account Character Owner**:
A World Shard's retained Realm Account ownership for one Character. It lets a transferred Character
use a local shadow Account without treating that shard-local Account id as authority. The row
survives Transfer, logout and Character deletion because Character guids are never reused.

**World Session Token**:
The Realm-core Account id, claim generation and request nonce carried by a bound Store and each
queued Durable Request. It preserves ownership across Transfer without changing bound identity.

**SessionActor**:
The Character guid and optional World Session Token carried by a reducer request. Bound World
Sessions retain their token across requests and Transfer. Tokenless requests are for trusted
Operator tools and bots; they refuse a Character with active Account ownership.

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
transaction; the Gateway claims the durable row and drives the same escrowed Transfer a World
Session would. The destination import binds the source Module identity, intent id and controller
generation to its arrival fence. Before release, the source intent records that the exact arrival
is ready. The Gateway deletes the exact claimed row only after that fence is released. A claim
lease lets a replacement Gateway resume after process restart.
_Avoid_: transfer request, move order

**Group Intent**:
A row a Package writes to ask the Gateway to run one party operation for a Character that has no
Session: invite that Character, or leave the party it leads. Party membership is authoritative on
realm-core, which a Package can never reach, so the Package decides and the Gateway executes against
the correct authority. Reaped on the shared event TTL, so it is a request, never a record. Held in
`game_bot_invite_intent`, which kept its name through the change that gave it a second operation.
_Avoid_: invite intent, group request, party order

**Bot Controller**:
The durable selector for Legacy, RecordOnly, Cohort, or Frozen behavior. RecordOnly records decisions
and authorizes no bot gameplay. Frozen cancels Foreground Actions and authorizes no new bot gameplay.

**Bot Capacity Lease**:
An optional host-issued expiry in the playerbots Package Config. A managed Realm renews it only
while its disk reserve is available. Expiry refuses spawning and controller activation and freezes
existing bots. Recovery never resumes frozen bots automatically.

**Provisioning Profile**:
A revisioned, bounded upkeep policy for a supported bot class and role. It selects free training and
owned supplies; Module Gates decide every operation. It is distinct from a World Import Profile,
which scopes imported world content.

**Bot Objective**:
A retained purpose with its destination, stage, deadline, catalog revision, and verified progress.
Tactical interruption does not replace the objective. A companion in a human-led party retains the
leader identity while refreshing the destination from current party facts; that refresh does not
replace a retained cast or its identity. Deferring Quest work can select another Bot Objective.
The accepted Quest and its bounded Recovery Attempt remain available for retry.
Selected return-home movement retains its Bot Objective while queued or travelling, including path
leg changes. Quest selection resumes after arrival, movement cancellation or the objective deadline.

**Companion Order**:
An authenticated human leader's retained Follow, Stay, Assist, or Target instruction for one bot.
Realm-core certifies party authority before the bot's World Shard applies it. The order directs the
existing Bot Controller and does not form a second runner. Each authenticated issuer carries a
monotonic order sequence across Shards, so a delayed older command cannot replace a newer one. In a
Raid, the Group leader may order a bot in any Subgroup and may name any Raid member.

**Solo Target Claim**:
The retained fight work of an ungrouped Cohort bot reserves one nearby creature while the bot
approaches or fights. Other ungrouped Cohort bots choose another eligible creature. The claim is
derived from the Recovery Attempt and holds only while the Bot Controller is Cohort. It expires
after thirty seconds without progress. It ends when the bot abandons the fight, fails its movement,
dies, joins a Party, switches to Legacy, RecordOnly or Frozen, or leaves the partition. It does not
grant loot rights. Party assistance and self-defense do not consult claims, but an owner that
defends itself against its claimed creature keeps that claim.

**Command Receipt**:
The target World Shard's durable result for one source Module Identity and intent id. It survives
the complete command retry window and travels with its Character through Escrow, so a Gateway retry
can finish the source response without applying the Companion Order twice.

**Foreground Action**:
The single movement or cast retained by one Bot Controller generation.
Selected movement continues between decision turns while its authority and destination remain valid.

**Candidate**:
A proposed action identified by its action, target, spell, reason, and Bot Objective identity.

**Strategy**:
A composition of typed triggers, defaults, and integer priorities used to select Candidates.

**Recovery Attempt**:
The bounded attempt to make useful progress on one destination, fight target, heal target, buff,
quest interaction, or companion leader in the current partition. Positioning and casting retain
their useful purpose. Only observed gameplay progress clears its elapsed failure time. A failed
attempt changes approach and then defers that work for a bounded period. Four attempts fit in the
retained memory, with at most three ordinary attempts so healing can still start.

**Navigation Inputs**:
The enabled navigation and collision modes, active static and derived generations, and the durable
revision advanced by terrain and navigation imports or changes to effective derived coverage. An
absent revision means those inputs predate revision tracking. These inputs identify retained
movement and failure evidence; only the route observation can state whether its consulted cells
had verified coverage.

**Group**:
The members led by one leader, a Party or a Raid. Stored in `game_group`.

**Party**:
A Group of up to 5 members that its leader has not converted to a Raid.

**Raid**:
A Group its leader converted. Up to 40 members in 8 Subgroups. It never converts back.
_Avoid_: raid group

**Raid Quest**:
A quest whose template type is 62. In a Raid, only Raid Quests take kill credit, quest-item drops
and quest-object use.
_Avoid_: raid-only quest

**Subgroup**:
One of a Raid's 8 divisions of up to 5 members, numbered 0 to 7. A member joining a Raid takes the
first Subgroup with room.
_Avoid_: party (for a subgroup), raid group

**Raid Slot**:
A member's Subgroup and Assistant flag, stored and sent as the vanilla group-list flags byte
`subgroup | 0x80 if assistant`. A Party member holds Subgroup 0 without the flag.

**Assistant**:
A Raid member the leader promoted. The Raid Slot carries the flag. An Assistant may invite, remove
any member but the leader, move members between Subgroups, set Target Icons, start a Ready Check
and send raid warnings. Only the leader converts, changes loot rules, passes the lead or promotes.
When the leader leaves a Raid, the first Assistant in join order leads.
_Avoid_: officer, raid officer, promoted member

**Group Audience**:
Who receives one Group Broadcast: everyone, everyone but the actor, the leader, or one Subgroup.

**Group Broadcast**:
One Realm-core `game_group_event` row per recipient in a Group Audience. Every Gateway relays the
rows addressed to its own sessions. A World Session may start a Ready Check, ping the minimap or
`/roll` once a second for each of the three. The Gateway drops a repeat inside that second.

**Target Icon**:
One of 8 marks a leader or Assistant puts on a unit, held per Group on the party authority. A unit
carries at most one.
_Avoid_: raid mark, marker, raid target (for the icon)

**Instance Removal**:
The 60-second countdown that moves a Character to its hearthstone home when it stands in a Group's
dungeon instance without being a member of that Group. Rejoining the Group or leaving the instance
cancels it. It starts only for a Character in the world; one removed while logged out starts it at
its next login. When the owning Group disbands, a Group that forms again inside the instance takes
it over, which also cancels it. A GM and a session-less Character with a live entity are exempt.
_Avoid_: homebind timer, instance kick, raid timer

**Ready Check**:
A leader or Assistant poll. Every member is asked; answers reach the leader only. The party
authority keeps no state for it.

**Party Partition**:
A Realm-core-ordered map and instance for one party member, confirmed by the Gateway against the
World Shard that holds the Character. It carries no position or Shard name. Pending Transfer,
transiently unknown location, and removed membership remain explicit states and cannot become a
companion destination.

**Roster Revision**:
Realm-core's monotonic order for one complete Group member list, leader, loot rules, Group kind,
and every Raid Slot. A World Shard keeps the last accepted value after disband. Older snapshots
cannot change its party mirror.

**Roster Revision Relay**:
The Gateway's push of a party's Realm-core roster to each World Shard whose mirror holds an older
Roster Revision or none, whatever changed it on Realm-core.
_Avoid_: mirror sync thread, roster watcher

**Member Stats**:
The status, health, power, level, zone, map position, auras, and live pet a group member's frame
shows for another member. The Gateway projects them from the Home Shard's cache and sends them
only while the member is outside the viewer's AOI.
_Avoid_: party stats, unit frame data

**Recovery Scan**:
A bounded scan of healing rotations that retains its last completed result while reading the next
batch. The scan can be pending or complete; its result distinguishes unread, missing, and a selected
rotation. It rechecks selections before use and restarts after Transfer using destination state.

**Sessionless Action Consent**:
A Character-owned core projection of a Package's controller selection. An absent row permits legacy
behavior. Each selection updates consent and clears unclaimed Group Intents in one transaction,
including a same-value selection. Automatic group answers and Group Intent claims require admission
on the owning World Shard. Suppression leaves a pending invitation unanswered. An action admitted
before a later selection may still finish on Realm-core. Admission and membership are separate
transactions across Shards. Consent travels with the Character on Transfer and is deleted with it.

**Idle Bot**:
A Package-controlled Character, one with a Sessionless Action Consent row, that is out of combat and
has not moved for thirty seconds. Its cells wake no creatures. A Character on a movement leg moves
every firing, so it is never an Idle Bot. Humans and test fixture Characters have no consent row and
always wake creatures.
_Avoid_: AFK bot, sleeping bot, inactive bot

**Home Shard**:
The shard that currently holds a Character's row.

**GUID Range**:
A disjoint interval assigned by Realm-core to one Shard. Character creation and item creation
consume the same durable high-water mark in that interval. Issued identities are never reused,
even after deletion or Transfer. Items add their wire type and format bits to the issued value.

**Shard Map**:
The rules that assign a map or instance to a shard.

**Shard Boundary**:
The edge between two shards.
_Avoid_: seam (see Working method)

**Escrow**:
Value or state held so it exists in exactly one shard while it moves. Used by Transfer and Mail; never by Trading.
Mail Escrow carries a Character's letter, a take from a Mail, and a Reward Letter.

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

### Chat

**Realm Chat Line**:
One non-proximity chat message committed on Realm-core with its complete recipient list. One Relay
delivers it to each recipient's World Session on any Shard.
_Avoid_: chat event (unqualified), broadcast

**Chat Kind**:
The 1.12 `ChatMsg` wire value of a player chat line. It names a Realm Chat Line's packet and
audience rule, and it picks the language rule for every player line, say and yell included.
_Avoid_: chat type (in new names)

**Speaker Facts**:
The speaker's race and chat tag, read by the Gateway on the Home Shard and conveyed in the Durable
Request. Realm-core holds no Characters.

**Speech**:
A line a Character speaks to the Characters near it: a say, yell or `/e` line, or a text emote. Its
Durable Request goes to the speaker's Home Shard, and a Relay delivers it to listeners in range.
_Avoid_: local chat, proximity chat

**Realm Presence**:
The Gateway's realm-wide read of one Character: Whereabouts, session online, race, class, level and
zone, from whichever World Shard holds it. Guild rosters, friends, `/who`, whisper and Member Stats
all read it.
_Avoid_: online status, presence cache

**Whereabouts**:
Realm Presence's own state for where a Character is: in world (with a live entity and an Away
Status), in transit between two places (a pending Transfer, or a Shard's own row reading online
with no entity there), or offline. A negative Whereabouts, offline or no Character found at all,
needs every configured World Shard to vouch that none of them is hiding the Character.
_Avoid_: presence state, location status

**Away Status**:
AFK or DND on a live Character: the `PLAYER_FLAGS` bit observers see, plus its Auto-Reply. Setting
one ends the other. It ends at login. A cross-map world-port and a Transfer keep it.
_Avoid_: presence, status (unqualified)

**Auto-Reply**:
The text a whisperer receives from a Character with an Away Status. It lives beside the live entity
on the Home Shard; with none stored, the whisperer gets "Away from Keyboard" or "Do not Disturb".
_Avoid_: away message, AFK message

**Chat Channel**:
A named, team-scoped conversation on Realm-core. Built-in (General, Trade, LocalDefense,
WorldDefense, LookingForGroup, GuildRecruitment) or custom. Its lowercase name and team are its
identity; the wire uses its creator's spelling.
_Avoid_: room, chat room

**Channel Membership**:
One Character's place in one Chat Channel. It lasts while the Account Claim generation that
admitted it lasts.
_Avoid_: channel subscription

**Channel Notice**:
One `SMSG_CHANNEL_NOTIFY`, committed on Realm-core with its explicit recipient list and delivered
like a Realm Chat Line.

**Chat Flood Limiter**:
The Gateway's in-memory count of fast chat lines per World Session. Eleven lines, each within one
second of the last, mute the session for ten seconds. `/afk`, `/dnd` and addon lines do not count,
and a Character with a GM level is never muted. Forgotten on disconnect.
_Avoid_: rate limiter, throttle, spam filter

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

**Loot Roll Promotion**:
Copying a Loot Roll from its source staging row to Realm-core. Its identity is the source Module
identity and the source row id. Retries preserve both values.

**Loot Roll Promotion Receipt**:
The greatest source row id accepted for one source Module identity, corpse guid and slot.
Realm-core retains it after resolution so an older Loot Roll Promotion cannot recreate the roll.

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

### Death and resurrection

**Self-Resurrection Option**:
The one spell a dead Character may cast on itself from the death dialog. Chosen at death from the
Character's Soulstone aura, kept through Release Spirit, and spent when the Character is resurrected
by any path or leaves the world. The client sees it as `PLAYER_SELF_RES_SPELL`.
_Avoid_: self-res offer, rez offer, resurrect offer (an offer is a resurrect request from another Character)

**Soulstone**:
The item a Warlock creates, and the Aura its use puts on a Character. A Character that dies while the
Aura is on it gets a Self-Resurrection Option. The Aura itself does not survive death.

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

**Spell Cast Event Kind**:
The append-only code naming the wire signal in one `game_spell_cast_event` row. Zero means a row
from the old schema. New rows use 1 START, 2 GO, 3 INTERRUPT, 4 PUSHBACK, or 5 PROC_LOG. A Gateway
does not guess how to decode an unknown nonzero code.
_Avoid_: cast flags, event flags, signal flags

**Item Exchange**:
A complete plan of carried-item consumption and item grants. Capacity, uniqueness, Random Property,
and identity allocation Gates pass before any inventory row changes. Quest rewards commit as one
exchange even when the caller retains a Refusal and continues.

**Action Outcome**:
An observation returned by the operation that owns an action. Acceptance records that work was
admitted; completion and progress require evidence from that work. A synchronous interaction ends
with success or Refusal. A timed cast also retains waiting and terminal outcomes.

**Route Step**:
A planned movement leg with its route status, first waypoint, collision clipping, and coverage
evidence. Its endpoint is proposed movement, not proof of advancement or arrival.

**Route Path**:
A bounded sequence of waypoints retained for one movement. The Module advances along the same
geometry and duration that the Gateway sends to the client. Crossing a waypoint does not require
another decision. A changed destination or a failed movement Gate can replace or stop the path. A
stop holds, and a replacement path starts from, the point the client renders at that moment, even
between two advance firings, unless the advance would halt the path short of it.

**Cast Handle**:
The scheduled identity, spell, target, and current due time of one timed cast. Repeated bot requests
retain this identity until the cast finishes or an explicit cancellation removes it.

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

**Distraction**:
The temporary state in which a Creature faces one ground point and holds its idle movement. It ends
at its expiry, at the Creature's next Engagement, or at its death or despawn. A second Distract
refreshes it. At the expiry the Creature turns back to its spawn orientation and a patrol continues
from the same waypoint.
_Avoid_: distract state, attention state, aggro redirect

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

### Runtime Scripts

**Runtime Script**:
Lua the Module runs on a core gameplay event or a Package Event, supplied from outside the core
rather than compiled into it. Named so a diagnostic can identify it. It reaches a Shard only through
a Package's Script Artifact; there is no upload path.
_Avoid_: plugin, mod, addon, user script

**Package Event**:
An event a Package fires itself, spelled `<package>.<name>`. It runs the same dispatch a core hook
event runs.
_Avoid_: custom event, user event, signal

**Script Answer**:
The number a Runtime Script returns, read back by the Package that asked. With no answer, the caller
keeps its own fallback.
_Avoid_: return value, script result, callback

**Runtime Script Host**:
The Module's embedded Lua interpreter and the boundary around it. It is the only place a Runtime
Script executes.
_Avoid_: sandbox, VM, engine

**Invocation**:
One run of one Runtime Script for one event. Nothing carries to the next one.

**Fuel Budget**:
The metered interpreter work one Invocation may spend before the Host cuts it off.
_Avoid_: quota, gas, instruction limit

**Entity Handle**:
The opaque reference a Runtime Script holds to one creature or player. It lasts exactly as long as
the Invocation that minted it.
_Avoid_: entity id, guid, pointer, reference

**Host Operation**:
One named gameplay call the Runtime Script Host offers a script.
_Avoid_: API function, binding, hook

**Staged Effect**:
A gameplay operation a Runtime Script asked for, recorded and not yet performed.
_Avoid_: pending action, queued effect, side effect

**Script Diagnostic**:
The bounded record of a failed Invocation, and the only thing a failed Invocation produces.
_Avoid_: error log, stack trace

### Mail

**Mail**:
One letter on Realm-core for one recipient. It carries a subject, an optional body, copper, one
item and a cash on delivery price. Its recipient cannot see, read, delete, take from or return it
before its delivery instant.
_Avoid_: message

**Delivery Delay**:
The hour a Mail with an item waits before it reaches a Character on another Realm Account, when it
is sent and when it is returned. A cash on delivery price needs an item, so a priced Mail waits too.
Copper, text, a COD payment, Auction Mail and a return at Mail Expiry arrive at once. The Gateway
reads both Realm Accounts, and the Module applies the rule.

**Mail Sender**:
Who a Mail is from, as the client names it in the inbox: a Character, an Auction House, a Creature
or a Gameobject.

**Returned Mail**:
A Character's Mail sent back to that Character once. It arrives unread, without its cash on delivery
price, and cannot be returned again. A Mail from any other Mail Sender, or from guid 0, cannot be
returned.

**Mail Template**:
An imported letter body from `MailTemplate.dbc`. The client shows it for a mail that names the
template id.

**Reward Letter**:
A letter a quest giver sends when a Character turns a quest in. It is from the quest ender, or from
the creature a quest script names, and it names a Mail Template, carries its text, and can carry
copper and one item. The World Session turn-in files it as Escrow in the same transaction, and the
Gateway delivers it, so it arrives once even when the Gateway stops between the two. A held letter
travels with its Character across a Transfer. A playerbot's turn-in sends none.

**Mail Timer**:
The one-shot schedule each Mail holds. It fires at the delivery instant of a Mail that is not
delivered yet, and at the end of the Mail's life for every Mail.
_Avoid_: mail reaper, expiry sweep

**Mail Expiry**:
What happens to a Mail at the end of its life: 3 days after it arrives with a cash on delivery
price, else 30 days. A Character's Mail that still carries an item goes back to its sender as a
Returned Mail. Every other Mail is deleted with its item and copper.

**Mail Arrival**:
The event that tells a recipient a Mail is now visible to them: on delivery, and when a Mail comes
back as a Returned Mail. The Gateway relays it as `SMSG_RECEIVED_MAIL` to a recipient in the world
on any Shard. An offline recipient learns of the Mail from the unread poll at the next login.
_Avoid_: new mail notification, mail push

**Letter Copy**:
A Plain Letter in the bags that carries a Mail's text. Made by `CMSG_MAIL_CREATE_TEXT_ITEM` from a
delivered Mail with a body, once. The Plain Letter sells for 0, so making one is not a value flow.
It keeps its text through a Mail, a Trade Commit, an auction and a Transfer.

**Item Text**:
Text readable from an item or a Mail, keyed by text id. A Letter Copy's text outlives the Mail that
made it, and is deleted with the last Letter Copy that carries it.

### Auctions

**Settlement**:
Resolving an auction at buyout or expiry: item and gold to their final owners, displaced bids refunded.
_Avoid_: resolve, close

**Auction Market**:
The listing pool shared by the houses of one team: Alliance (houses 1-3), Horde (houses 4-6) and
neutral (house 7). A bid reaches every listing in its bidder's market, whatever house placed it.
The house the seller stands at still sets that listing's deposit and cut.

**Auction Mail**:
A Mail from an Auction House whose subject and invoice body the client renders: item name, price,
and the counterparty's name where the invoice names one. Sent for an outbid refund, a won item, a
successful sale, an expired listing, a cancelled listing's item, and the bid a Cancellation refunds.

**Auction Notice**:
The live message an online seller or bidder gets the instant an auction is outbid, won, sold,
expired or cancelled. Rides the private per-recipient Relay; an offline recipient
gets the Auction Mail only.

**Hold**:
Value taken from a Character on its Home Shard for one auction operation, and held while Realm-core
decides. A listing Hold keeps the item and the deposit. A bid Hold keeps the full offer, and a
Cancellation's bid Hold keeps the Auction Cut. The decision spends the Hold or gives it back,
exactly once. Unlike Escrow, a Refusal refunds it. A bid Hold and a listing Hold both travel with
their Character on Transfer.

**Cancellation**:
The seller withdraws an active listing. The item goes back by Auction Mail, a displaced bidder gets
the bid back by Auction Mail, and the house keeps the deposit. A seller who cannot pay the Auction
Cut gets no answer.

**Auction Cut**:
The house's consignment share of a bid, truncated. The seller pays it out of the proceeds at
Settlement, or out of the purse at a Cancellation of a listing with a bid.

### Guilds

**Guild**:
A named, realm-wide set of Characters of one team with ranks, a leader, a message of the day and an
emblem. Lives on Realm-core.

**Guild Leader**:
The one member at rank 0. The client's default rank name is "Guild Master". Its Character cannot be
deleted. When a Guild Leader's Character is gone anyway, leadership passes to the member with the
highest Guild Rank, earliest join first; a Guild with nobody left disbands.
_Avoid_: guild master (it is also an NPC title), GM (means game master)

**Guild Rank**:
One of five to ten ordered ranks of a Guild; 0 is the highest. Each has a name and Rank Rights.

**Rank Rights**:
The permission bits a Guild Rank grants, such as invite, remove, promote, chat, notes and MOTD.

**Guild Event**:
A Realm-core row that tells every online member, or one addressed Character, that something happened
in a Guild. The Gateway renders it.

**Guild Projection**:
PLAYER_GUILDID and PLAYER_GUILDRANK as the Gateway derives them from Realm-core membership when it
encodes a player, and re-sends when membership changes. No Shard stores them.

**Fee Hold**:
Copper taken from a Character's purse on its Home Shard and held while Realm-core decides the guild
operation it pays for. The decision spends it or returns it, exactly once. A Character has at most
one; it travels with the Character on Transfer and blocks Character deletion.
_Avoid_: escrow (for a fee), reservation

**Tabard Designer**:
An NPC with the tabard-designer flag. Its window saves a Guild Emblem.

**Guild Emblem**:
The five tabard design values of a Guild. Only the Guild Leader saves one, for 10 gold.

**Guild Invite**:
A pending offer for one Character to join one Guild; expires after two minutes.

**Public Note**:
A short note a Guild Rank with EPNOTE sets on a member. Every member sees it on the roster.

**Officer Note**:
A short note a Guild Rank with EOFFNOTE sets on a member. It shows only to a viewer whose own Guild
Rank holds VIEWOFFNOTE; every other viewer's roster carries it blank.

**Guild Charter**:
The item (entry 5863) that stands for one Petition. It costs 10 silver through a Fee Hold. Its owner
offers it for Signatures and turns it in to found the Guild.

**Petition**:
Realm-core's record of a proposed Guild: its owner, its name, its Guild Charter and its Signatures.
A Character owns at most one.

**Signature**:
One Character's endorsement of a Petition. One per Realm Account; nine found the Guild.

**Petitioner**:
An NPC with the petitioner flag. One that is also a Tabard Designer sells Guild Charters.
_Avoid_: guild registrar

### Meeting stones

**Meeting Stone**:
A type-23 GameObject at a dungeon entrance. Using it puts the Character, or the Party it leads, in
the Meeting Stone Queue for the stone's dungeon area, inside the stone's level range.
_Avoid_: summoning stone, LFG tool

**Meeting Stone Queue**:
Realm-core's realm-wide list of Seekers and queued Parties, per dungeon area and team.
_Avoid_: LFG queue, matchmaking queue

**Seeker**:
One Character in the Meeting Stone Queue, alone or as a member of a queued Party. It carries its
class and its team, from the class and race the Gateway supplied. A solo Seeker lasts as long as
its Account Claim.
_Avoid_: queued player, LFG player

**Open Role**:
One of a queued Party's one tank, one healer and three damage roles that no member's class fills
yet.
_Avoid_: vacancy, role slot

**Stone Add**:
The Module adding a Seeker to a queued Party, or forming a Party from five waiting Seekers, through
the party authority's join core.
_Avoid_: matchmaking, auto-invite

### World clock and weather

**Realm Clock**:
The wall clock the realm runs on, always UTC. There is no realm-timezone setting, so no part of the realm reads host-local time. The Gateway packs it into `SMSG_LOGIN_SETTIMESPEED` once per world entry and the client advances it alone afterwards. It also answers `CMSG_QUERY_TIME` with Unix seconds in `SMSG_QUERY_TIME_RESPONSE`, so the client can interpret timed quest deadlines. The Module reads the same clock to pick the weather season.
_Avoid_: server time, game time, local time

**Zone Weather**:
One durable row per zone holding the sky that zone currently shows. The same row is the state a world entry reads and the source a live Relay fires from, so there is no second weather table and no replay ambiguity for a reconnecting client. A zone with no row has fine weather.
_Avoid_: weather event, weather state table

### Packages

**Package**:
A drop-in folder under `packages/<name>/` that adds content to the realm with no core-file edits. Its `src/` is compiled into the Module wasm by the build's own discovery; its `client/` half supplies addons, whole-file client overrides and UI Transforms to the client packer. A Package can also carry Runtime Scripts in `scripts/`, Datascripts in `datascripts/`, and generated artifacts in `data/.generated/`. Any one of these parts is enough.
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

**Package Fixture**:
Package code that stages or observes state for the Package's own durable tests. It compiles only
with `debug_reducers` and reaches Core through `crate::package_fixture`, a Package API root a
release build does not have.
_Avoid_: test harness, debug hook, test helper

**Package Config**:
A row of `game_package_config`, keyed by `(package_name, key)`: one durable value a Package reads
and the Operator edits. A Package seeds its own defaults idempotently, so the table always shows
real keys with live values.
_Avoid_: config file, setting (unqualified), package setting

**Package Inventory**:
The two directories that hold installed Packages. `packages/` holds the enabled ones, which the build compiles. `.lyracore/packages-disabled/` holds the disabled ones, which it cannot see. A Package's location IS its enabled state; no file records it, so nothing can disagree with the disk about what the next build compiles. `lyracore packages enable` and `lyracore packages disable` move one folder between the two.
`lyracore packages apply` applies the enabled set to the chosen Shards. See [Applying packages](packages/README.md#applying-packages) for artifact preparation and Module publishing.
_Avoid_: registry, package list, enabled flag, state file

**Package Teardown**:
The Operator step that stops a Package on every Shard before it leaves the Package Inventory. It
makes the Package's Characters Dormant Characters, empties the Package's tables, deletes its Package
Config, and stops its registered code. A publish that removes a table refuses while the table holds
rows, so the Package's tables must be empty first. `lyracore packages disable` runs it.
_Avoid_: uninstall, package wipe, cleanup

**Reference Package**:
One rung of the maintained example ladder in the [Official Package Collection](https://github.com/LyraCoreProject/packages#packages): `example-script`, `example-client`, `example-data`, `example-rust` or `example-all`. Each adds a small welcome at one level of the Package API. `lyracore packages new NAME [--from RUNG]` copies and renames one at the collection tag matching the checkout's Package API version. The default is `example-script`. Core ships no enabled Reference Package.
_Avoid_: template package, sample package

**Package Source**:
Where an installed Package was copied from: a local folder on the Operator's machine, a Git Package Source, or an Official Package Source. A scaffolded Package records a scaffold origin, its Reference Package rung and the collection revision. The author owns that copy; `packages update` does not replace it.
_Avoid_: origin, upstream, repo

**Git Package Source**:
A repository whose root is one Package, named by a URL. `lyracore packages add <git-url>` clones it and installs a copy of its tree, without the `.git`. An installed Package is never a working copy, so `lyracore packages update` re-clones rather than pulling. A Package installed this way is Git-backed, and `packages update` can advance it.
_Avoid_: git remote, upstream repo, checkout

**Official Package Collection**:
The one repository, `LyraCoreProject/packages`, that holds several first-party Packages side by side, one top-level directory each. `lyracore packages add <name>` resolves a bare Package name against it: a folder on the Operator's machine or a Git URL keeps its own rules, and only a name that matches neither falls back to the collection.
_Avoid_: registry, package repository, marketplace

**Official Package Source**:
The top-level directory of the Official Package Collection that a bare `lyracore packages add <name>` installed. Its Provenance Stamp records the collection's URL and the Recorded Revision the directory was resolved at, the same way a Git Package Source records its repository and commit. The checkout's Package API version selects a collection tag. `packages update` resolves that tag again and records the new commit after the update passes its checks.
_Avoid_: registry entry, published package

**Recorded Revision**:
The exact commit a Git Package Source, Official Package Source or Reference Package was copied from, held in its Provenance Stamp. `lyracore packages update` advances a Git Package Source from this rather than from whatever the repository's branch points at now, so it can name both commits when it reports or restores.
_Avoid_: version, tag, pin

**Provenance Stamp**:
The record `lyracore packages add` writes inside an installed Package: its Package Source, its Content Identity at install time, when it was installed, and, for a Git or Official Package Source or a scaffold, its Recorded Revision. A scaffold also records its Reference Package rung. A Package without one is still a Package; only its history is unknown.
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
The typed API at `datascripts/lib/` that a Datascript writes against: `data.spell(id)`, `.clone(newId)`, `.set(field, value)`, `.effect(0 | 1 | 2)`, and a `run(package, script)` that emits one Package Delta. It reads the Base Snapshot and refuses exactly what the artifact parser refuses, so a Datascript fails at author time rather than at import. The library lives in Core. A Package carries its Datascripts in `datascripts/`; Core's legacy `datascripts/src/<package>/` path also works.
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
band, and an apply clears the whole band before it writes, so a Package that leaves the enabled set
takes its invented rows with it. `docs/package-api.md` lists every band.
_Avoid_: custom id range, synthetic id range

**Package Spell Range**:
The spell family's Package Identifier Range: 6,000,000 to 6,999,999.
_Avoid_: custom id range, synthetic spell range

**Package Script Range**:
The script family's Package Identifier Range: 100,000 to 999,999.

**Script Artifact**:
The versioned artifact recording every Runtime Script one Package ships. Distinct from a Package
Delta, which states columns of rows a base import owns.
_Avoid_: script bundle, script manifest, script delta

**Event Binding**:
The event a Runtime Script runs for: a name from the Module's hook catalogue, or a Package Event of
the shipping Package.
_Avoid_: hook registration, subscription, listener

**Script Directive**:
A `@key value` comment line at the top of a Runtime Script source, declaring what the file cannot
say in its own code.
_Avoid_: annotation, frontmatter, pragma, metadata header

**Runtime Script Toolchain**:
The pinned author-time compiler that turns a Package's `scripts/` sources into its Script Artifact.
_Avoid_: transpiler, build pipeline, SDK

**Package Item Range**:
The items family's Package Identifier Range: 7,000,000 to 7,999,999.
_Avoid_: custom id range, synthetic item range

**Package Quest Range**:
The quest family's Package Identifier Range: 8,000,000 to 8,999,999.
_Avoid_: custom id range, synthetic quest range

**Package Loot Range**:
The loot family's Package Identifier Range: 9,000,000 to 9,999,999.
_Avoid_: custom id range, synthetic loot range

**Package Cast Range**:
The casts family's Package Identifier Range: 10,000,000 to 10,999,999.
_Avoid_: custom id range, synthetic cast range

**Package Trainer Range**:
The trainers family's Package Identifier Range: 11,000,000 to 11,999,999.
_Avoid_: custom id range, synthetic trainer range

**Package Gossip Range**:
The gossip family's Package Identifier Range: 12,000,000 to 12,999,999.
_Avoid_: custom id range, synthetic gossip range

**Package Globals Range**:
The globals family's Package Identifier Range: 13,000,000 to 13,999,999.
_Avoid_: custom id range, synthetic globals range

**Package Spell Metadata Range**:
The spellmeta family's Package Identifier Range: 14,000,000 to 14,999,999.
_Avoid_: custom id range, synthetic spellmeta range

**Package Creature Range**:
The creatures family's Package Identifier Range: 15,000,000 to 15,999,999.
_Avoid_: custom id range, synthetic creature range

**Package Gameobject Range**:
The gameobjects family's Package Identifier Range: 16,000,000 to 16,999,999.
_Avoid_: custom id range, synthetic gameobject range

**Package EventAI Range**:
The Creature-AI Family's Package Identifier Range: 17,000,000 to 17,999,999.
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

### Random properties

**Random Property**:
The suffix selected for one item instance at creation, held as `random_property_id`. Zero is a
plain item. Moving an existing item preserves this value, including zero.
_Avoid_: random enchant, item variant, roll

**Property Pool**:
The weighted set of Random Properties a template can produce, named by
`game_item_template.random_property`. Members sort by property ID. The importer stores ClassicDB
chance in exact hundredths and refuses more than two decimal places. Selection compares
`input * total_weight < cumulative_weight * 10000` using widened integer arithmetic, for inputs
0 through 9999. Thus each cumulative band endpoint rounds up. Both underweight and overweight
pools use their full total. Very small weights can receive no input at this resolution.
_Avoid_: random property group, suffix group

**Suffix**:
The name a Random Property adds to an item, such as "of the Bear". The Module keeps a copy for
diagnosis. The client renders its own.
_Avoid_: affix, postfix

**Stat Kind**:
An append-only code for an enchantment effect's contribution, independent of the client encoding.
`docs/schema.md` lists the codes and the effect key layout.
_Avoid_: stat type, mod type, effect type

### GameObject collision

**GameObject Collider**:
Derived registration of a DOOR or BUTTON mesh for one GameObject in one map and instance.
A closed, live GameObject blocks sight and collision rays. Its registration remains while open.
Static geometry retains its separate active-generation rule.
