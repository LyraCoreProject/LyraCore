# LyraCore glossary

LyraCore is a World of Warcraft 1.12.1 server. Game state lives in SpacetimeDB. The Native Host runs
inside the SpacetimeDB process and speaks the vanilla wire protocol to unmodified clients. Standard
WoW words such as guid, opcode, aura, gossip, master looter and round-robin keep their client
meaning. This glossary lists one only where LyraCore narrows or changes it.

Each term lives in exactly one file. [`CORE_TERMS.md`](./CORE_TERMS.md) holds the terms a new
contributor meets in a first change. This file holds every other term, and the index below lists
the terms of both. A term for one feature, one subsystem or one Operator task goes here. A term
that goes out of use moves to [Retired terms](#retired-terms) as one line. Never keep a copy of an
entry in both files.

## Index

**A** [Account](CORE_TERMS.md#account) · [Account Character Owner](#retired-terms) · [Account Claim](#retired-terms) · [Account Fence](#retired-terms) · [Action Outcome](#procs) · [Actor](CORE_TERMS.md#actor) · [Alpha Test Tools](#accounts-characters-and-sessions) · [AOI](CORE_TERMS.md#aoi) · [AOI Grant](CORE_TERMS.md#aoi-grant) · [Architecture Test](CORE_TERMS.md#architecture-test) · [Assistant](#groups) · [Auction Cut](#auctions) · [Auction House](#auctions) · [Auction Mail](#auctions) · [Auction Market](#auctions) · [Auction Notice](#auctions) · [Authored Casting](#creature-ai) · [Authored Combat](#creature-ai) · [Authored Flee](#creature-ai) · [Authored Spawn](#requests-and-delivery) · [Authoring Library](#packages) · [Auto-Reply](#chat) · [Away Status](#chat)

**B** [Base Snapshot](#packages) · [Baseline](#client-content) · [Behavior](#conformance) · [Bot Capacity Lease](#bots) · [Bot Controller](#bots) · [Bounded Map Slice](#realm-topology) · [Build Identity](#packages)

**C** [Cancellation](#auctions) · [Carrier](#procs) · [Cast Handle](#procs) · [Cast Miss](#procs) · [Channel Membership](#chat) · [Channel Notice](#chat) · [Character](CORE_TERMS.md#character) · [Character Relocation](#bots) · [Chat Channel](#chat) · [Chat Flood Limiter](#chat) · [Chat Kind](#chat) · [Claim](#packages) · [Claim Conflict](#packages) · [Client Artifact](#client-content) · [Command Receipt](#retired-terms) · [Compatibility Manifest](#realm-topology) · [Conformance Audit](#conformance) · [Content Identity](#packages) · [Counterparty](#procs) · [Creature-AI Family](#creature-ai)

**D** [Data-dependent field](#conformance) · [Datascript](#packages) · [Deliberate Deviation](#conformance) · [Delivery Delay](#mail) · [Development reload](CORE_TERMS.md#development-reload) · [Dismount](#mounts) · [Disposable Host](#working-method) · [Distraction](#creature-ai) · [Dormant Character](#accounts-characters-and-sessions) · [Durable Read](CORE_TERMS.md#durable-read) · [Durable Request](CORE_TERMS.md#durable-request) · [DynamicObject](#requests-and-delivery)

**E** [Effect Kind](#requests-and-delivery) · [Encounter Binding](#realm-topology) · [Encounter Signal](#realm-topology) · [Engagement](#creature-ai) · [Entity Handle](#runtime-scripts) · [Equivalence Rule](#conformance) · [Escrow](#retired-terms) · [Event Binding](#packages)

**F** [Fake](CORE_TERMS.md#fake) · [Feature Ledger](#conformance) · [Fee Hold](#retired-terms) · [Fixed Rout](#creature-ai) · [Fixture Verb](#conformance) · [Fixture-Reserved Identifier](#packages) · [Flat Cast](#creature-ai) · [Forced Death](#creature-ai) · [Fuel Budget](#runtime-scripts)

**G** [GameObject Collider](#gameobject-collision) · [Gate](CORE_TERMS.md#gate) · [Gateway](#retired-terms) · [Gateway Verb](#retired-terms) · [Git Package Source](#packages) · [GM level](#accounts-characters-and-sessions) · [Golden Trace](#conformance) · [Group](#groups) · [Group Audience](#groups) · [Group Broadcast](#groups) · [Group Intent](#retired-terms) · [Guardian](#creature-ai) · [GUID Range](#accounts-characters-and-sessions) · [Guild](#guilds) · [Guild Charter](#guilds) · [Guild Emblem](#guilds) · [Guild Event](#guilds) · [Guild Invite](#guilds) · [Guild Leader](#guilds) · [Guild Projection](#guilds) · [Guild Rank](#guilds)

**H** [Headless Client](CORE_TERMS.md#headless-client) · [Hold](#retired-terms) · [Host Operation](#runtime-scripts)

**I** [Idle Bot](#bots) · [Import Family](#packages) · [Instance Lease](#groups) · [Instance Pool](#retired-terms) · [Instance Removal](#groups) · [Instance Vmap Slice](#realm-topology) · [Invocation](#runtime-scripts) · [Item Exchange](#procs) · [Item Text](#mail)

**L** [Land Mount](#mounts) · [Lethal Damage Floor](#creature-ai) · [Letter Copy](#mail) · [Listing](#auctions) · [Login Queue](#accounts-characters-and-sessions) · [Logon Limiter](#accounts-characters-and-sessions) · [Loot Release](#loot) · [Loot Roll](#loot) · [Loot Roll Promotion](#retired-terms) · [Loot Roll Promotion Receipt](#retired-terms) · [Loot Source](#loot) · [Loot Tag](#loot) · [Loot Window](#loot)

**M** [Mail](#mail) · [Mail Arrival](#mail) · [Mail Expiry](#mail) · [Mail Sender](#mail) · [Mail Template](#mail) · [Mail Timer](#mail) · [Meeting Stone](#meeting-stones) · [Meeting Stone Queue](#meeting-stones) · [Member Stats](#groups) · [Module](CORE_TERMS.md#module) · [Mount Projection](#mounts) · [Movement Intent](#creature-ai)

**N** [Native Host](CORE_TERMS.md#native-host) · [Navigation Inputs](#bots)

**O** [Officer Note](#guilds) · [Official Package Collection](#packages) · [Official Package Source](#packages) · [Open Role](#meeting-stones) · [Operator](CORE_TERMS.md#operator) · [Operator Content API](#requests-and-delivery) · [Operator Spawn Range](#requests-and-delivery) · [Operator Template Range](#requests-and-delivery) · [Owner Token](#requests-and-delivery)

**P** [Package](CORE_TERMS.md#package) · [Package API](CORE_TERMS.md#package-api) · [Package Cast Range](#packages) · [Package Config](#packages) · [Package Creature Range](#packages) · [Package Delta](#packages) · [Package Event](#runtime-scripts) · [Package EventAI Range](#packages) · [Package Fixture](#packages) · [Package Gameobject Range](#packages) · [Package Globals Range](#packages) · [Package Gossip Range](#packages) · [Package Identifier Range](#packages) · [Package Import](#packages) · [Package Inventory](#packages) · [Package Item Range](#packages) · [Package Loot Range](#packages) · [Package Quest Range](#packages) · [Package Script Range](#packages) · [Package Source](#packages) · [Package Spell](#packages) · [Package Spell Metadata Range](#packages) · [Package Spell Range](#packages) · [Package Teardown](#packages) · [Package Trainer Range](#packages) · [Package-owned Account](#accounts-characters-and-sessions) · [Party](#groups) · [Party Partition](#groups) · [Passenger](#transports) · [Path Progress](#transports) · [Patrol Pause](#creature-ai) · [Petition](#guilds) · [Petitioner](#guilds) · [Pre-auth I/O Deadline](#accounts-characters-and-sessions) · [Proc](#procs) · [Proficiency](#requests-and-delivery) · [Property Pool](#random-properties) · [Protocol Family](CORE_TERMS.md#protocol-family) · [Prototype](#working-method) · [Provenance Stamp](#packages) · [Public Note](#guilds)

**R** [Raid](#groups) · [Raid Quest](#groups) · [Raid Slot](#groups) · [Random Property](#random-properties) · [Ranged Posture](#creature-ai) · [Rank Rights](#guilds) · [Ready Check](#groups) · [Realm](CORE_TERMS.md#realm) · [Realm Chat Line](#chat) · [Realm Clock](#world-clock-and-weather) · [Realm-core](#retired-terms) · [Recorded Revision](#packages) · [Reference Datascript](#packages) · [Reference Package](#packages) · [Reference Server](#conformance) · [Refusal](CORE_TERMS.md#refusal) · [Relay](CORE_TERMS.md#relay) · [Relay Arrival](#realm-topology) · [Relay Definition](#realm-topology) · [Relay Run](#realm-topology) · [Returned Mail](#mail) · [Reward Letter](#mail) · [Rollback](#requests-and-delivery) · [Roster Revision](#groups) · [Roster Revision Relay](#retired-terms) · [Route Path](#procs) · [Route Step](#procs) · [Rule State](#creature-ai) · [Runtime Script](CORE_TERMS.md#runtime-script) · [Runtime Script Host](#runtime-scripts) · [Runtime Script Toolchain](#packages)

**S** [Script Answer](#runtime-scripts) · [Script Artifact](#packages) · [Script Diagnostic](#runtime-scripts) · [Script Directive](#packages) · [Script Identity](#packages) · [Seam](CORE_TERMS.md#seam) · [Seeker](#meeting-stones) · [Self-Resurrection Option](#death-and-resurrection) · [Service Reconciliation](#realm-topology) · [Session](CORE_TERMS.md#session) · [Session Expiry](#accounts-characters-and-sessions) · [SessionActor](#accounts-characters-and-sessions) · [Sessionless Action Consent](#bots) · [Settlement](#auctions) · [Shard](CORE_TERMS.md#shard) · [Signature](#guilds) · [Source Profile](#realm-topology) · [Soulstone](#death-and-resurrection) · [Spatial Claim](#packages) · [Speaker Facts](#chat) · [Spec](#working-method) · [Speech](#chat) · [Spell Cast Event Kind](#procs) · [Staged Effect](#runtime-scripts) · [Standalone Supervisor](#realm-topology) · [Stat Kind](#random-properties) · [Stone Add](#meeting-stones) · [Store](CORE_TERMS.md#store) · [Subgroup](#groups) · [Suffix](#random-properties) · [Swing Error](#procs)

**T** [Tabard Designer](#guilds) · [Target Icon](#groups) · [Tier 1 baseline](#conformance) · [Trade Commit](#trading) · [Trade Session](#trading) · [Transfer](#retired-terms) · [Transfer Intent](#retired-terms) · [Transport](#transports) · [Transport Loss](CORE_TERMS.md#transport-loss) · [Triggered Cast](#procs) · [Trust Review](#packages)

**U** [UI Transform](#client-content)

**V** [Verification](CORE_TERMS.md#verification)

**W** [Weapon Strike](#procs) · [Will-Not-Be-Traded Slot](#trading) · [World Import Completion](#realm-topology) · [World Import Profile](#realm-topology) · [World Import Scope](#realm-topology) · [World Session](CORE_TERMS.md#world-session) · [World Session Token](#retired-terms) · [World Shard](#retired-terms) · [World Source](#realm-topology)

**Z** [Zone Weather](#world-clock-and-weather)

## Language

### Realm topology

Core terms used here: [Native Host](CORE_TERMS.md#native-host),
[Development reload](CORE_TERMS.md#development-reload), [Realm](CORE_TERMS.md#realm),
[Shard](CORE_TERMS.md#shard), [Module](CORE_TERMS.md#module), [Operator](CORE_TERMS.md#operator).

**World Source**:
The external world database an Operator supplies to an import. vmangos owns the source identifiers,
content revisions and server spell catalogue. The Operator's client supplies the remaining client
data and archive names for Map lookup.

**World Import Profile**:
A named import plan that selects the maps and content one world import loads into a Shard.
`full-world` imports every source map. The other profiles import part of the world. See
`importer/src/world_import_scope.rs`.

**World Import Completion**:
The record that a World Import loaded one map and passed its content checks. An import clears these
records before it loads. The development CLI refuses play checks for a map without one.
_Avoid_: import done flag, content ready flag

**World Import Scope**:
The authoritative union of Bounded Map Slices, Instance Vmap Slices, whole maps, and forced
creature dependencies owned by one World Import Profile. Accepted EventAI summons force their
summoned templates into the scope, to a fixpoint. It decides spatial import membership for
World Source, terrain, navigation, and vmap modes.

**Source Profile**:
A named input contract for a World Source. It defines the columns, types and behavior values an
import accepts. Count changes are reported without deciding admission. The accepted snapshot
provides the comparison baseline; an admitted import records the actual source commit.
_Avoid_: EventAI Source Profile

**Compatibility Manifest**:
The import account for one World Import Scope. It records source values, dependencies and omissions
as emitted, normalized, excluded, dropped, unsupported or unapproved. An unapproved result is a
Refusal for apply and remains visible in dry run.

**Encounter Binding**:
The map-scoped link from an imported EventAI action to the package that owns the encounter. It also
decides who may tune that creature's catalogue. A Claim on a broadcast text or summon placement that
an encounter-bound definition depends on is refused, and the refusal names the claim and the
binding.

**Encounter Signal**:
A named Reset, Begin, Fail, Complete, or encounter-specific notification delivered through an Encounter
Binding. Source numeric states do not cross this boundary.

**Relay Definition**:
A typed, versioned sequence of EventAI steps imported as one validated catalogue. This is Module
gameplay data, not a Relay.

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
floor. The terrain and navigation self-checks fail before `--apply` when it is not.

**Instance Vmap Slice**:
A named entry and exit route on an ADT-backed instance map, with an exit radius and a cell collar
for collision extraction. It owns only the selected vmap cells. It does not claim terrain,
Navigation Coverage, or the rest of the instance.

**Standalone Supervisor**:
The systemd unit that runs a host's `spacetimedb-standalone` process, tracked at
`deploy/systemd/spacetimedb-standalone.service`. That unit sets its restart limits. Exhausting the
start limit requires Operator recovery.

**Service Reconciliation**:
Making a host's Standalone Supervisor match the unit tracked in the checkout. `lyracore-ops service
reconcile` from the `lyracore-cli` repository performs it, reading every expected value out of the
tracked unit. A host that does not match afterwards is reported as NOT reconciled, never as a
success.

### Requests and delivery

Core terms used here: [Gate](CORE_TERMS.md#gate), [Durable Request](CORE_TERMS.md#durable-request),
[Durable Read](CORE_TERMS.md#durable-read), [Refusal](CORE_TERMS.md#refusal),
[Transport Loss](CORE_TERMS.md#transport-loss), [Relay](CORE_TERMS.md#relay),
[AOI](CORE_TERMS.md#aoi), [AOI Grant](CORE_TERMS.md#aoi-grant).

**Rollback**:
How a native request ends without committing. A Refusal rollback carries the packets that the
Native Host sends after the transaction rolls back; the list can be empty. A failure rollback ends
the World Session. Only `native_world_receive` writes a Refusal as the `native_reply:` envelope.
_Avoid_: native error, reply string

**Proficiency**:
What a Character may wield and wear. A weapon type needs its weapon proficiency spell, and misc
weapons and fishing poles need none. Armor is the class base set plus the tiers trained at a class
trainer. The equip Gate, the client's proficiency masks and the auction usable filter all read one
derivation in `crates/lyracore-shared/src/item.rs`.

**Owner Token**:
The SpacetimeDB token of the Shard's publisher. It bypasses row-level security.

**Effect Kind**:
A known durable spell effect code. The shared `EffectKind` enum defines its byte value and whether
it applies an aura. Imported and authored table rows keep their byte encoding; unknown codes
remain possible and consumers handle them explicitly.

**DynamicObject**:
The client object that draws a ground spell effect. It has its own GUID, map, Instance and AOI Grant.
The ground area's timer removes the DynamicObject when the effect expires.

**Operator Content API**:
The `operator_*` reducers that edit creature and gameobject spawns, item templates and vendor offers
on a running Realm. Only the Operator can call them. Edits apply in one transaction and refresh
connected clients where needed. A later import of the same Import Family replaces the rows.
`docs/operator-content-api.md` gives the contracts.
_Avoid_: editor API, world editor reducers

**Authored Spawn**:
An instance-zero creature or gameobject spawn row that no Package, population copy, Instance or
pool owns. Spawn edits through the Operator Content API change only an Authored Spawn.

**Operator Spawn Range**:
The creature and gameobject spawn identifiers the Operator Content API allocates. No import and no
Package spawn uses it. `docs/operator-content-api.md` gives the band.

**Operator Template Range**:
Template entries 5,200,500 through 5,999,999, reserved for allocation by the Operator Content API.
Imports and Packages must not insert templates here. Item creation uses this range; creature,
gameobject and quest template creation can use it later. It fits the creature-entry 24-bit limit.

### Accounts, characters and sessions

Core terms used here: [Account](CORE_TERMS.md#account), [Character](CORE_TERMS.md#character),
[Actor](CORE_TERMS.md#actor), [Session](CORE_TERMS.md#session),
[World Session](CORE_TERMS.md#world-session).

**Package-owned Account**:
An Account a Package created for its session-less Characters through the Package API. It has no
credentials, so no login reaches it. `game_package_account` records the owning Package in Core, so
the record outlives the Package when the Package is disabled.
_Avoid_: bot account, account block

**Dormant Character**:
A Character whose Package was torn down. It is offline and has no live entity or Sessionless Action
Consent. Its Account and Character rows stay, by maintainer decision: turning a Package off never
deletes its Characters. A Package enabled again does not adopt them.
_Avoid_: deleted bot, orphan bot, frozen bot

**Alpha Test Tools**:
Account-owned authority for a limited set of alpha testing dot-commands. The Module reads it from
the Account for every command and applies the final Gate.

**GM level**:
Account-owned authority for every dot-command. Any nonzero level grants all of them. It also lets
each Character on the Account create a Guild with `.guild create`, stay in another party's
Instance, chat past the Chat Flood Limiter and move without logging movement violations. The
Operator sets it with `set_gm_level`, which takes a Character name and writes that Character's
Account. Godmode (`.god`), not the GM level, keeps creatures from choosing a Character as a target.
_Avoid_: GM character

**Login Queue**:
The Module's line of authenticated World Sessions waiting for admission in ticket order, held in
`game_native_admission`. The vanilla client shows its position. Capacity, entry limits and
admission rate can keep new arrivals in it.
_Avoid_: wait list, login line

**SessionActor**:
The Actor argument an Operator reducer takes: a Character guid and an optional legacy ownership
token. Native requests resolve their Actor from the World Session instead.

**Pre-auth I/O Deadline**:
The time a peer has from socket acceptance to prove itself before the Native Host closes the
socket. Proof completion clears it. See `native-host/src/mod.rs`.
_Avoid_: pre-auth read deadline, read timeout, idle timeout (for this limit)

**Logon Limiter**:
The Module's caps on logon attempts per connection, and on failed logons and open logon connections
per address. A refusal closes the socket. See `module/src/native/logon.rs`.
_Avoid_: rate limiter, throttle, brute-force lockout

**Session Expiry**:
The hour a Session stays valid for new world authentication after its logon. The world handshake
refuses an expired Session like an absent one. Every logon starts a fresh hour, and an established
World Session continues past it. See `module/src/auth.rs`.
_Avoid_: session timeout, TTL (in prose)

**GUID Range**:
The interval a Shard issues Character and item guids from. Character creation and item creation
consume one durable high-water mark in it, and issued guids are never reused, even after deletion.
See `module/src/auth.rs`.

### Groups

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
One group event per recipient in a Group Audience. The Module sends it to native members and
records it as a `game_group_event` row in the same transaction.

**Target Icon**:
One of 8 marks a leader or Assistant puts on a unit, held per Group. A unit carries at most one.
_Avoid_: raid mark, marker, raid target (for the icon)

**Instance Removal**:
The countdown that sends a Character to its hearthstone home when it stands in a Group's dungeon
Instance without being a member of that Group. Rejoining the Group or leaving the Instance cancels
it. A GM and a session-less Character with a live entity are exempt. See `module/src/instance.rs`.
_Avoid_: homebind timer, instance kick, raid timer

**Instance Lease**:
The `game_instance` row that gives one dungeon Instance its id. Removing an Instance's population
keeps it, and reaping removes it last. See `docs/instance-lifecycle.md`.

**Ready Check**:
A leader or Assistant poll. Every member is asked; answers reach the leader only. The Group keeps
no state for it.

**Party Partition**:
The map and Instance that party facts report for one member, read from its live entity. For a
member without one, only the legacy mirror table `game_group_member_partition` can supply it. An
unknown location or removed membership cannot become a companion destination.

**Roster Revision**:
A Group's membership counter in `game_group_roster_revision`. Every roster change advances it. The
row outlives disband, so a delayed roster cannot recreate the Group.

**Member Stats**:
The status, health, power, level, zone, map position, auras, and live pet a group member's frame
shows for another member. The Module sends them only while the member is outside the viewer's AOI.
_Avoid_: party stats, unit frame data

### Bots

The playerbots Package lives in the Official Package Collection and defines its own terms. This
section holds the bot terms core code and docs use.

**Character Relocation**:
The latest completed sessionless AreaTrigger crossing of one Character. A repeat call with the same
AreaTrigger, destination and controller generation returns its id. Position, Instance admission and
this record commit in one Module transaction. Controller selection and Character deletion remove it.

**Bot Controller**:
The playerbots Package's durable selector for Legacy, RecordOnly, Cohort, or Frozen behavior.
RecordOnly records decisions and authorizes no bot gameplay. Frozen authorizes no new bot gameplay.
Each selection advances a controller generation that Character Relocation records.

**Bot Capacity Lease**:
An optional host-issued expiry in the playerbots Package Config. A managed Realm renews it only
while its disk reserve is available. Expiry refuses spawning and controller activation and freezes
existing bots. Recovery never resumes frozen bots automatically.

**Navigation Inputs**:
The enabled navigation and collision modes, their active generations, and the durable revision that
terrain and navigation imports advance. They identify retained movement and failure evidence. Only
the route observation can state whether its consulted cells had verified coverage.

**Sessionless Action Consent**:
A Character-owned core projection of a Package's controller selection. An absent row permits
sessionless actions. Each selection updates consent and clears the latest Character Relocation.
Invite, leave and automatic group acceptance check current consent in the same transaction as the
action. Consent is deleted with its Character.

**Idle Bot**:
A Package-controlled Character, one with a Sessionless Action Consent row, that is out of combat and
has stood still for a set time. Its cells wake no creatures. Humans and test fixture Characters have
no consent row, so they always wake creatures. See `module/src/creatures/tick/mod.rs`.
_Avoid_: AFK bot, sleeping bot, inactive bot

### Trading

**Trade Session**:
The negotiation state between two Characters: offered items, offered gold, and each side's accept
flag.
_Avoid_: escrow, trade escrow

**Trade Commit**:
The single atomic swap of offered items and gold once both parties have accepted.
_Avoid_: escrow swap

**Will-Not-Be-Traded Slot**:
The 7th trade-window slot. Its item is shown to the other party but never included in the Trade Commit.
_Avoid_: enchant slot

### Chat

**Speech**:
A line a Character speaks to the Characters near it: a say, yell or `/e` line, or a text emote. A
Relay delivers it to listeners in range.
_Avoid_: local chat, proximity chat

**Realm Chat Line**:
One non-proximity chat message committed with its complete recipient list. One Relay delivers it to
each recipient's World Session.
_Avoid_: chat event (unqualified), broadcast

**Chat Kind**:
The 1.12 `ChatMsg` wire value of a player chat line. It names a Realm Chat Line's packet and
audience rule, and it picks the language rule for every player line, say and yell included.
_Avoid_: chat type (in new names)

**Speaker Facts**:
The speaker's race and chat tag that a Realm Chat Line request carries. The Module reads them from
the speaking Character.

**Away Status**:
AFK or DND on a live Character: the `PLAYER_FLAGS` bit observers see, plus its Auto-Reply. Setting
one ends the other. It ends at login. A world-port keeps it.
_Avoid_: presence, status (unqualified)

**Auto-Reply**:
The text a whisperer receives from a Character with an Away Status. It lives beside the live entity.
With none stored, the whisperer gets "Away from Keyboard" or "Do not Disturb".
_Avoid_: away message, AFK message

**Chat Channel**:
A named, team-scoped conversation, built in like General and Trade or made by a Character. Its
lowercase name and team are its identity; the wire uses its creator's spelling.
_Avoid_: room, chat room

**Channel Membership**:
One Character's place in one Chat Channel. It survives map changes and ends on logout or connection
loss.
_Avoid_: channel subscription

**Channel Notice**:
One `SMSG_CHANNEL_NOTIFY`, committed with its explicit recipient list and delivered like a Realm
Chat Line.

**Chat Flood Limiter**:
The native connection's count of fast chat lines. A burst mutes the World Session for a short time.
`/afk`, `/dnd` and addon lines do not count, a Character whose Account has a GM level is never muted, and the
count survives Character logout. See `module/src/native/chat_flood.rs`.
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
A group's roll on one item from a Loot Source.

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

### Transports

**Transport**:
A boat or zeppelin: a type-15 `MO_TRANSPORT` GameObject that travels one TaxiPathNode path on a
fixed cycle. A path can cross maps, and every Character on the Transport's current map sees it. The client moves it from its Path Progress, and the
Module computes the same timetable to carry Passengers. Elevators and trams are not Transports, and
neither is the network transport of a Transport Loss.
_Avoid_: ship, vehicle

**Passenger**:
A Character standing on a Transport. Its movement names the Transport and its offset on it, and the
Module moves its world position with the Transport between movement updates, and to the next map
when the Transport crosses maps. Movement without the Transport, any other teleport or leaving the
world ends it. The taxi code's `passenger` names a Character in
flight, not a Passenger.
_Avoid_: rider (a rider is on a mount)

**Path Progress**:
The milliseconds a Transport's CREATE carries. The client places the Transport at Path Progress plus
elapsed time, modulo its cycle period. LyraCore sends the phase within the current cycle.

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
A cast started by a Proc, a periodic aura or another triggered effect. It checks faction targeting
and target life, then runs the cast core's effect loop without cast-start position, range, line of
sight, power cost, cooldown or stealth-break checks. Its own hits fire no further Procs.
_Avoid_: proc cast, internal cast, free cast

**Spell Cast Event Kind**:
The append-only code naming the wire signal in one `game_spell_cast_event` row, such as START, GO
or PERIODIC_LOG. Zero marks a row from the old schema. See `module/src/spell/tables.rs`.
_Avoid_: cast flags, event flags, signal flags

**Cast Miss**:
The primary target's outcome when a damage spell's hit roll fails, recorded on the GO row. The
roll runs once per target, and a missed target takes none of the spell's effects. A physical spell
misses; a magic spell is resisted outright. A Weapon Strike rolls the physical attack table, so its
target can also dodge or parry it.
_Avoid_: spell miss flag, resist flag

**Weapon Strike**:
A spell effect that deals the caster's weapon damage plus the spell's flat bonus, scaled by its
weapon damage percent. A ranged-slot spell such as Throw strikes with the ranged weapon and spends
one thrown weapon or one ammo. A queued strike instead waits for the next melee swing.
_Avoid_: weapon damage spell, special attack

**Swing Error**:
Why a player's due melee swing could not land: the target is out of reach or behind the player.
Stored on the engagement row and reported to the player once per change.
_Avoid_: swing failure, attack error

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
geometry and duration it sends to the client, so crossing a waypoint needs no new decision. A
changed destination or a failed movement Gate can replace or stop the path from the point the
client renders at that moment.

**Cast Handle**:
The scheduled identity, spell, target, and current due time of one timed cast. Repeated bot requests
retain this identity until the cast finishes or an explicit cancellation removes it.

### Creature AI

**Creature-AI Family**:
The import family that loads the EventAI catalogue: event rows, broadcast texts, and summon
locations. No table in it names a map, so its Claims apply whatever the World Import Scope, exactly
as the family's own base import writes it.

**Guardian**:
A spell-summoned creature owned by another unit. It uses its template level and its owner's
faction, attacks hostile units, and has its own lifetime. Its timer continues during combat.
An NPC-owned Guardian can keep fighting after its owner dies.

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

Core terms used here: [Runtime Script](CORE_TERMS.md#runtime-script).

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
One letter for one recipient. It carries a subject, an optional body, copper, one item and a cash
on delivery price. Its recipient cannot see, read, delete, take from or return it before its
delivery instant.
_Avoid_: message

**Delivery Delay**:
The hour a Mail with an item waits before it reaches a Character on another Realm Account, when it
is sent and when it is returned. A cash on delivery price needs an item, so a priced Mail waits too.
Copper, text, a COD payment, Auction Mail and a return at Mail Expiry arrive at once.

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
the creature a quest script names. It names a Mail Template, carries its text, and can carry copper
and one item. The native turn-in and `gw_turn_in_quest` insert the Mail in the same transaction.
A Package turn-in through `actor::turn_in_quest` sends none.

**Mail Timer**:
The one-shot schedule each Mail holds. It fires at the delivery instant of a Mail that is not
delivered yet, and at the end of the Mail's life for every Mail.
_Avoid_: mail reaper, expiry sweep

**Mail Expiry**:
What happens to a Mail at the end of its life. A Character's Mail that still carries an item goes
back to its sender as a Returned Mail. Every other Mail is deleted with its item and copper. See
`crates/lyracore-shared/src/mail.rs` for the lifetimes.

**Mail Arrival**:
The event that tells a recipient a Mail is now visible to them: on delivery, and when a Mail comes
back as a Returned Mail. The Module sends `SMSG_RECEIVED_MAIL` to the recipient's World Session. An
offline recipient learns of the Mail from the unread poll at the next login.
_Avoid_: new mail notification, mail push

**Letter Copy**:
A Plain Letter in the bags that carries a Mail's text. Made by `CMSG_MAIL_CREATE_TEXT_ITEM` from a
delivered Mail with a body, once. The Plain Letter sells for 0, so making one is not a value flow.
It keeps its text through a Mail, a Trade Commit and an auction.

**Item Text**:
Text readable from an item or a Mail, keyed by text id. A Letter Copy's text outlives the Mail that
made it, and is deleted with the last Letter Copy that carries it.

### Auctions

**Auction House**:
One house from `AuctionHouse.dbc`, a `game_auction_house` row. Its faction picks its Auction Market
and sets the deposit and Auction Cut of the listings placed there.

**Listing**:
One item a seller puts up at an Auction House, a `game_auction` row, until Settlement or
Cancellation.
_Avoid_: auction (for the row), lot

**Settlement**:
Resolving an auction at buyout or expiry: item and gold to their final owners, displaced bids refunded.
_Avoid_: resolve, close

**Auction Market**:
The listing pool shared by the houses of one team: Alliance, Horde or neutral. A bid reaches every
listing in its bidder's market, whatever house placed it. The house the seller stands at still sets
that listing's deposit and cut. See `crates/lyracore-shared/src/auction.rs`.

**Auction Mail**:
A Mail from an Auction House whose subject and invoice body the client renders: item name, price,
and the counterparty's name where the invoice names one. Sent for an outbid refund, a won item, a
successful sale, an expired listing, a cancelled listing's item, and the bid a Cancellation refunds.

**Auction Notice**:
The live message an online seller or bidder gets the instant an auction is outbid, won, sold,
expired or cancelled. The Module selects its recipient in the same transaction. An offline
recipient gets the Auction Mail only.

**Cancellation**:
The seller withdraws an active listing. The item goes back by Auction Mail, a displaced bidder gets
the bid back by Auction Mail, and the house keeps the deposit. A seller who cannot pay the Auction
Cut gets no answer.

**Auction Cut**:
The house's consignment share of a bid, truncated. The seller pays it out of the proceeds at
Settlement, or out of the purse at a Cancellation of a listing with a bid.

### Guilds

**Guild**:
A named, Realm-wide set of Characters of one team with ranks, a leader, a message of the day and an
emblem.

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
A row that tells every online member, or one addressed Character, that something happened in a
Guild. The Module sends it to native recipients when it inserts the row.

**Guild Projection**:
PLAYER_GUILDID and PLAYER_GUILDRANK as the Module derives them from Guild membership when it
encodes a Character. Membership changes update them through AOI. No Character row stores them.

**Tabard Designer**:
An NPC with the tabard-designer flag. Its window saves a Guild Emblem.

**Guild Emblem**:
The five tabard design values of a Guild. Only the Guild Leader saves one, for a fee. See
`crates/lyracore-shared/src/guild.rs`.

**Guild Invite**:
A pending offer for one Character to join one Guild. It expires after a fixed time. See
`module/src/gc.rs`.

**Public Note**:
A short note a Guild Rank with EPNOTE sets on a member. Every member sees it on the roster.

**Officer Note**:
A short note a Guild Rank with EOFFNOTE sets on a member. It shows only to a viewer whose own Guild
Rank holds VIEWOFFNOTE; every other viewer's roster carries it blank.

**Guild Charter**:
The item that stands for one Petition. Its owner buys it, offers it for Signatures and turns it in
to found the Guild. See `crates/lyracore-shared/src/guild.rs` for its entry and price.

**Petition**:
The record of a proposed Guild: its owner, its name, its Guild Charter and its Signatures. A
Character owns at most one.

**Signature**:
One Character's endorsement of a Petition, at most one per Realm Account. A full set founds the
Guild. See `crates/lyracore-shared/src/guild.rs` for the count.

**Petitioner**:
An NPC with the petitioner flag. One that is also a Tabard Designer sells Guild Charters.
_Avoid_: guild registrar

### Meeting stones

**Meeting Stone**:
A type-23 GameObject at a dungeon entrance. Using it puts the Character, or the Party it leads, in
the Meeting Stone Queue for the stone's dungeon area, inside the stone's level range.
_Avoid_: summoning stone, LFG tool

**Meeting Stone Queue**:
The Realm's list of Seekers and queued Parties, per dungeon area and team.
_Avoid_: LFG queue, matchmaking queue

**Seeker**:
One Character in the Meeting Stone Queue, alone or as a member of a queued Party. It carries its
class and team from Character facts. A solo Seeker lasts as long as its World Session.
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
The wall clock the Realm runs on, always UTC. No part of the Realm reads host-local time. The Module
sends it to the client at world entry, answers time queries from it, and picks the weather season
from it.
_Avoid_: server time, game time, local time

**Zone Weather**:
One durable row per zone holding the sky that zone shows now. World entry reads it and a live Relay
fires from it, so a reconnecting client sees the same weather. A zone with no row has fine weather.
_Avoid_: weather event, weather state table

### Packages

Core terms used here: [Package](CORE_TERMS.md#package), [Package API](CORE_TERMS.md#package-api).

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
The two directories that hold installed Packages: `packages/` for the enabled ones the build
compiles, and `.lyracore/packages-disabled/` for the disabled ones. A Package's location is its
enabled state, and no file records it. `lyracore packages enable` and `lyracore packages disable`
move one folder between the two.
_Avoid_: registry, package list, enabled flag, state file

**Package Teardown**:
The Operator step that stops a Package before it leaves the Package Inventory. It makes the
Package's Characters Dormant Characters, empties the Package's tables, deletes its Package Config,
and stops its registered code. `lyracore packages disable` runs it.
_Avoid_: uninstall, package wipe, cleanup

**Reference Package**:
One rung of the maintained example ladder in the [Official Package Collection](https://github.com/LyraCoreProject/packages#packages):
`example-script`, `example-client`, `example-data`, `example-rust` or `example-all`. Each adds a
small welcome at one level of the Package API. `lyracore packages new` copies and renames one,
`example-script` by default. Core ships no enabled Reference Package.
_Avoid_: template package, sample package

**Package Source**:
Where an installed Package was copied from: a local folder on the Operator's machine, a Git Package
Source, or an Official Package Source. A scaffolded Package records its Reference Package rung and
the collection revision. The author owns that copy, and `packages update` does not replace it.
_Avoid_: origin, upstream, repo

**Git Package Source**:
A repository whose root is one Package, named by a URL. `lyracore packages add <git-url>` installs a
copy of its tree without the `.git`. An installed Package is never a working copy, so `lyracore
packages update` clones again rather than pulling.
_Avoid_: git remote, upstream repo, checkout

**Official Package Collection**:
The one repository, `LyraCoreProject/packages`, that holds several first-party Packages side by
side, one top-level directory each. `lyracore packages add <name>` resolves a bare Package name
against it only when the name matches no local folder and no Git URL.
_Avoid_: registry, package repository, marketplace

**Official Package Source**:
The top-level directory of the Official Package Collection that a bare `lyracore packages add
<name>` installed. Its Provenance Stamp records the collection's URL and the Recorded Revision. The
checkout's Package API version selects the collection tag that `packages update` resolves.
_Avoid_: registry entry, published package

**Recorded Revision**:
The exact commit a Git Package Source, Official Package Source or Reference Package was copied
from, held in its Provenance Stamp. `lyracore packages update` advances from this commit rather
than from the repository's current branch, so it can name both commits when it reports or restores.
_Avoid_: version, tag, pin

**Provenance Stamp**:
The record `lyracore packages add` writes inside an installed Package: its Package Source, its
Content Identity at install time, when it was installed, and any Recorded Revision. A scaffold also
records its Reference Package rung. A Package without one is still a Package; only its history is
unknown.
_Avoid_: manifest, lockfile, metadata

**Content Identity**:
A hash of an installed Package's files. Comparing the recorded one against the tree on disk is what "locally drifted" means.
_Avoid_: checksum, fingerprint, version

**Trust Review**:
The deterministic, read-only inventory `packages add` prints before it asks: what the candidate
Package registers, counted the way the build counts it. It is an inventory, never a security
verdict. A Package's unclassified Rust is trusted code.
_Avoid_: audit, scan, security check

**Datascript**:
Author-time TypeScript that describes game data, written against typings generated from the Module
schema. It runs on the author's machine under Bun, never on the Realm. It is trusted code the author
wrote, not sandboxed code. A Runtime Script is the kind that runs inside the Realm.
_Avoid_: data script, script (unqualified), seed script, sandbox

**Reference Datascript**:
The maintained, minimal Datascript at `datascripts/src/reference.ts`. It names real Module columns
on purpose. It is the worked example and also the schema check that fails `tsc --noEmit` when the
schema moves under it.
_Avoid_: sample script, test script

**Authoring Library**:
The typed API at `datascripts/lib/` that a Datascript writes against. Its `run(package, script)`
emits one Package Delta. It reads the Base Snapshot and refuses exactly what the artifact parser
refuses, so a Datascript fails at author time rather than at import.
_Avoid_: SDK, framework, DSL, builder API

**Base Snapshot**:
The read-only file of derived base rows a Datascript reads to clone and tune, one per Import Family.
Only the spell family has one today. It holds the `game_*` values the import would load, never
client bytes, and stays out of git. It is the only base data a Datascript sees, so one Package
cannot observe another's claims.
_Avoid_: dump, base data file, cache

**Package Spell**:
A spell a Package invents, at an identifier inside the Package Spell Range. It exists only in the
Realm's data. An unmodified client renders spells from its own `Spell.dbc` and shows no tooltip for
one.
_Avoid_: custom spell, fake spell

**Import Family**:
One named slice of base data the importer loads as a unit and reloads whole, such as "spell",
"creatures" or "quests". Import provenance and Package Delta apply both work per family: a family's
Package claims are reapplied as the last stage of its import, in one transaction. Every claimable
table belongs to exactly one family.
_Avoid_: data family, import group, dataset

**Package Delta**:
The versioned artifact recording what one Package changes in the base data: the Package identity,
the source hash, and one Claim per row. It is canonical JSON, so two artifacts that say the same
thing are byte-identical. A base import replaces whole Import Families, so the importer replays
deltas on every reload. They live in the Package's `data/.generated/`.
_Avoid_: patch, override file, diff

**Package Import**:
The record of what one Package contributed to the last apply of one Import Family: the artifact
digest, the source hash, the row counts and the base import generation. Every apply rewrites it, so
it says what Packages the Shard runs now. A Provenance Stamp records where a Package came from.
_Avoid_: apply log, history, audit trail

**Build Identity**:
The recorded inputs `packages build` writes in a sibling file next to a source-built Package Delta
or Script Artifact. `lyracore packages check` and preflight recompute it against the checkout and
refuse, naming the input that changed. A prebuilt Script Artifact without sources has none. See
`src/cmd/packages/identity.rs` in lyracore-cli.
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
takes its invented rows with it. Each band takes its family's name: Package Spell, Script, Item,
Quest, Loot, Cast, Trainer, Gossip, Globals, Spell Metadata, Creature, Gameobject and EventAI Range.
`docs/package-api.md` lists every band.
_Avoid_: custom id range, synthetic id range, synthetic spell range and its siblings

**Script Artifact**:
The versioned artifact recording every Runtime Script one Package ships. Distinct from a Package
Delta, which states columns of rows a base import owns.
_Avoid_: script bundle, script manifest, script delta

**Event Binding**:
The event a Runtime Script runs for: a name from the Module's hook catalogue, or a Package Event of
the shipping Package. A source file declares one binding, such as `events.player.onLogin(welcome)`,
and the toolchain emits an Invocation of that function with the typed event payload.
_Avoid_: hook registration, subscription, listener

**Script Identity**:
A Runtime Script's Package and source-file stem, paired with its durable numeric ID in
`script-ids.json`. The Runtime Script Toolchain allocates new IDs and keeps recorded ones, even for
removed sources. Renaming a handler keeps its identity; renaming its source file creates a new one.

**Script Directive**:
A legacy `@key value` comment at the top of a Runtime Script source. It selects the legacy source
contract for the whole file, still builds and keeps its IDs. New source declares its Event Binding
in code. Removing directives requires a Package API migration.
_Avoid_: annotation, frontmatter, pragma, metadata header

**Runtime Script Toolchain**:
The pinned author-time compiler that turns a Package's `scripts/` sources into its Script Artifact.
_Avoid_: transpiler, build pipeline, SDK

**Spatial Claim**:
A Claim on a creature spawn or gameobject spawn row, whose primary key names its map. An import
applies it only when its World Import Scope owns that map, and reports a claim for another map as
routed away, never refused. The map never reaches the derived durable guid, so a Package cannot
move a placed row to another map.
_Avoid_: map claim, zoned claim, sharded claim

**Fixture-Reserved Identifier**:
An identifier the seeded fixtures own, which no Package may claim under any operation in any Import
Family. It lies in the project-wide fixture band or in one family's own fixture cluster. See
`crates/lyracore-package-delta/src/ids.rs`.
_Avoid_: test id, reserved id (unqualified)

### Client content

**UI Transform**:
An anchored edit one Package makes to a stock FrameXML or GlueXML file, declared in the Package's
`client/ui-transforms.json`. Each entry names a path, an anchor that occurs exactly once in the
Baseline, and text to insert before or after the anchor or to replace it. Packages may edit one
file while their anchors do not overlap. `lyracore client sync` composes the result against the
Operator's own client.
_Avoid_: patch, override, hook, FrameXML edit

**Baseline**:
The stock bytes of one client file, read out of the Operator's own UI archives and never out of
`patch-3.MPQ`, the packer's own previous output. A UI Transform's output is the Baseline with edits
applied, which makes it baseline-derived: it reaches the Operator's client through `client sync` and
never enters the Client Artifact.
_Avoid_: original, stock file, source file, vanilla file

**Client Artifact**:
The directory tree `lyracore client pack` builds for a player to copy over a stock client. It
carries package-authored bytes only. The packer refuses a baseline-derived file by name before it
writes the first byte.
_Avoid_: client patch, distribution, release bundle, player package

### Conformance

**Behavior**:
One interaction a player would recognize, with a defined 1.12.1 result and verification needs.
_Avoid_: feature (when meaning one interaction)

**Conformance Audit**:
A dated comparison of LyraCore and the Reference Server through the same wire scenarios. It
separates rule differences from data differences using the comparator's classification tags.
_Avoid_: compatibility audit

**Data-dependent field**:
A packet field declared in a harness scenario with its opcode, field path and reason. The comparator
classifies changed data values separately while preserving presence, shape and protocol checks.

**Reference Server**:
The vmangos server whose observable results define 1.12.1 behavior for conformance checks.
_Avoid_: reference core

**Golden Trace**:
A recording of a scenario on the Reference Server, with text replaced by SHA-256 digests, used as
expected results for conformance checks.
_Avoid_: golden file, snapshot (for recorded packets)

**Equivalence Rule**:
A named rule with a reason that permits a specified difference between decoded results, such as
consistent GUID remapping or a time tolerance.
_Avoid_: normalization rule

**Deliberate Deviation**:
A client-visible result LyraCore intentionally keeps different from the Reference Server, with a
reason and a source. A different implementation with the same result is not a deviation.
_Avoid_: intentional divergence

**Fixture Verb**:
A scenario's request for setup that both the LyraCore and Reference Server adapters implement.
_Avoid_: setup command (in a scenario)

**Feature Ledger**:
The collection of Behaviors, their scope, evidence requirements and Deliberate Deviations from
which tools derive conformance progress.
_Avoid_: feature matrix, support matrix

**Tier 1 baseline**:
The committed set of rule and data signatures reported by tier 1 for each scenario. CI compares
both sets exactly. It records improvements for review but does not change the comparator's result.

### Working method

Core terms used here: [Seam](CORE_TERMS.md#seam), [Protocol Family](CORE_TERMS.md#protocol-family),
[Store](CORE_TERMS.md#store), [Fake](CORE_TERMS.md#fake),
[Architecture Test](CORE_TERMS.md#architecture-test), [Headless Client](CORE_TERMS.md#headless-client),
[Verification](CORE_TERMS.md#verification).

**Prototype**:
Throwaway code built to answer a design question. Never shipped.
_Avoid_: spike, poc

**Disposable Host**:
A Native Host that the native check starts on free local ports, with its own data directory, the
Module published and the Operator claimed. It stops and deletes its data when the check ends.
_Avoid_: test server, harness

**Spec**:
The statement of what a feature must do. Lives on the GitHub issue unless a `docs/*.md` supersedes it.
_Avoid_: PRD, requirements doc, design doc

### Random properties

**Random Property**:
The suffix selected for one item instance at creation, held as `random_property_id`. Zero is a
plain item. Moving an existing item preserves this value, including zero.
_Avoid_: random enchant, item variant, roll

**Property Pool**:
The weighted set of Random Properties a template can produce, named by
`game_item_template.random_property`. Selection uses exact integer arithmetic over the pool's full
total weight, so a very small weight can receive no roll. See `module/src/items/properties.rs` and
`importer/src/item_property.rs`.
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

## Retired terms

These name concepts of the multi-Shard Gateway runtime. Some legacy tables and reducers still use
the names. [Migrating to the Native Host](docs/migrating-to-native-host.md) covers that code.

**Account Character Owner**: retired. A World Shard's record of the Realm Account that owned a transferred Character. _Avoid_ in new text.

**Account Claim**: retired. Realm-core's time-limited ownership of an Account by one World Session. Module tests and the native check still open one through `claim_account`, and meeting stone and Operator reducers still read `game_account_claim`. _Avoid_ in new text.

**Account Fence**: retired. A World Shard's copy of an Account Claim generation. _Avoid_ in new text.

**Command Receipt**: retired. A World Shard's record that one playerbots companion order applied once. _Avoid_ in new text.

**Escrow**: retired. Value held so it existed in exactly one Shard while it moved between Shards. `game_mail_escrow` and the `game_transfer_*` ledger keep the legacy rows. _Avoid_ in new text.

**Fee Hold**: retired. Copper held while Realm-core decided a guild fee. Native purchases create none. _Avoid_ in new text.

**Gateway**: retired. The protocol runtime before the Native Host. _Avoid_ in new text.

**Gateway Verb**: retired. A `gw_*` reducer the Gateway called. `module/src/gw.rs` keeps them, and Module tests, the native check and Operator tools still call them. _Avoid_ in new text.

**Group Intent**: retired. A row that asked the Gateway to run a party operation for a sessionless Character. _Avoid_ in new text.

**Hold**: retired. Value held while Realm-core decided an auction operation. Native auctions create none. _Avoid_ in new text.

**Instance Pool**: retired. A Shard that hosted instanced maps. _Avoid_ in new text.

**Loot Roll Promotion**: retired. Copying a Loot Roll from a World Shard to Realm-core. _Avoid_ in new text.

**Loot Roll Promotion Receipt**: retired. Realm-core's record that kept an old Loot Roll Promotion from recreating a roll. _Avoid_ in new text.

**Realm-core**: retired. The Shard for Realm-wide state. A native Realm keeps that state in its one Shard. `module/src/realm_core.rs` keeps its tables and reducers. _Avoid_ in new text.

**Roster Revision Relay**: retired. The Gateway's push of a Realm-core roster to World Shard mirrors. `sync_group_mirror` remains. _Avoid_ in new text.

**Transfer**: retired. Moving a Character's state from one Shard to another. `module/src/transfer/` remains, and gameplay still asks `transfer::is_in_transit` before it acts on a Character. _Avoid_ outside that module and `docs/character-transfer.md`.

**Transfer Intent**: retired. A row a Package wrote to ask the Gateway to Transfer a sessionless Character. Character Relocation replaced it. _Avoid_ in new text.

**World Session Token**: retired. The Realm-core ownership token a request carried across Shards. _Avoid_ in new text.

**World Shard**: retired. A Shard that owned a set of maps. _Avoid_ in new text.
