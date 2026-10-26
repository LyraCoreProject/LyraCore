# LyraCore architecture

This page explains what the runtime is and where each responsibility lives. Each claim names the
file that makes it true; where this page and the code disagree, the code wins. Traps and migration
rules are in [danger zones](./danger-zones.md).

## Native host migration

A Realm runs in one Shard, the SpacetimeDB database `lyracore`. Accounts, Characters, maps,
Instances and Realm-wide state share one Module. Map and Instance identifiers are gameplay
partitions inside that Shard. Request handlers and Relays run in the Module.

The [native Module](../module/src/native/mod.rs) owns SRP proof verification, logon limits,
Sessions, world authentication, connection state, request dispatch and response packets. The Host
supplies fresh cryptographic entropy for each SRP challenge. The [AOI tables](../module/src/aoi.rs)
keep AOI Grants stable through entity, GameObject, Corpse, DynamicObject and stealth changes.
Packet codecs live in the WASM-compatible [protocol crate](../crates/lyracore-protocol/src/lib.rs).

The [host extension](../native-host/src/mod.rs) adds logon and world TCP listeners to SpacetimeDB
standalone. It uses in-process Core connections and one private output subscription for all native
clients, with no SDK or WebSocket connection to the Module. It passes native reducer arguments as
BSATN. BSATN has no field names and the reducer call does not check for trailing bytes, so the
Host's arguments must match the reducer parameters' order and layout. The Host owns secure random
seeds, header ciphers, framing, socket deadlines and bounded output queues. It applies the grant and
revoke instructions the Module issues; it does not calculate spatial visibility or gameplay Gates.

If a Module replacement closes the selected Module before dispatch, the Host retries that
unexecuted request once against the current Module. It does not retry failed reducer outcomes or
requests whose outcome is unknown.

`game_native_packet.id` orders delivery instructions within a transaction. A grant's CREATE comes
before the packets delivered through that grant. A revoke removes its recipient before later
packets can use it. Logout clears every delivery permission of the connection before it returns to
Character selection. The Host applies these instructions in order. A failed output subscription
closes its native clients, because the Host cannot continue from a missed instruction.

World entry registers the Character as an AOI viewer after self CREATE and before grant replay.
Sessionless Characters stay AOI subjects but allocate no viewer Grants, so Companion populations
create no delivery subscriptions for Characters without native connections.

A request handler returns a Rollback when the request does not commit. `native_world_receive`
writes a Refusal rollback as a `native_reply:` envelope, a JSON list of Module-encoded packets. The
list is empty for a silent Refusal. The failed transaction rolls back. The Host parses the list and
submits its packets to `native_world_reply` on the same connection, which emits the Refusal through
the ordered output stream. The Host does not classify gameplay errors. A failure rollback returns
plain text, and the Host closes the connection.

### Host, listeners and admission

The host extension pins SpacetimeDB 2.13.0 at `08e1aba133671f9c631c6e70133a82faa0e7b283`
([`prepare-host.py`](../native-host/prepare-host.py)). The Module targets 2.7.1 and passes the
native compatibility check. The listeners read `LYRACORE_NATIVE_LOGON_ADDR`,
`LYRACORE_NATIVE_WORLD_ADDR` and `LYRACORE_NATIVE_DATABASE`; a listener whose address is absent is
off.

The Host counts completed reads and writes as socket activity, and Login Queue position updates
keep waiting clients connected. Idle sockets and incomplete world frames time out after 90 seconds.
The handshake has a 10-second deadline and each write a 2-second deadline. Accept errors retry with
backoff from 100 ms up to two seconds. Each listener holds at most 4,096 connections and closes
further sockets at once.

Admission policy runs in the Module. The Operator calls `native_set_admission_limits` with maximum
World Sessions, maximum concurrent entries and admissions per second. Zero turns a limit off.
Limits default to zero and persist across restarts. Lowering a limit keeps admitted World Sessions
connected. The Login Queue sends a position on arrival, updates waiting positions every second and
admits tickets in order. A finished world entry releases its entry slot; an unfinished entry
releases it after 20 seconds. The World Session keeps its capacity slot until disconnect. A new
authenticated connection replaces an older one for the same Account and joins behind existing
waiters.

Session revocation Gates run before admission and before every authenticated request, including
ignored controls. The one-hour Session expiry limits new world authentication only. An established
World Session continues past that deadline, and the reaper keeps its Session until the connection
closes. Account bans, deleted Sessions and replacement login proofs still revoke established
connections.

The local CLI starts this runtime. It builds the Host, publishes the `lyracore` Shard and
provisions Accounts through the Module. Native data lives in `.lyracore/native-data`. See
[local setup](./quickstart.md#3-start-the-realm) for process ownership and credential files, and
[the native operations guide](./native-operations.md) for deployment.

### Development reload

`./lyracore dev watch` runs a local Native Host with `LYRACORE_NATIVE_DEV=1`. It watches Module
source and builds the `dev-reload` WASM profile incrementally. Dependencies use optimization level
1, the Module level 0, and build scripts and procedural macros level 3. Watch asks Cargo only for
the WASM library, with debug information off. The Host uses Cranelift without optimization and
keeps fuel metering and epoch interruption. These settings shorten development builds; they are not
the release configuration. The profile keeps development assertions and integer overflow checks,
which can stop a reducer where a release build would continue.

The development engines share a bounded in-memory Cranelift function cache, so unchanged functions
can reuse machine code across WASM files. Keys and records use at most 256 MiB plus bookkeeping.
The oldest entries go first when it is full. The cache disappears when the Host stops, and normal
Hosts do not enable it. It uses Wasmtime 39's experimental incremental compiler cache. Local
measurements and their limits are in
[`development-reload-speed.json`](../native-host/results/development-reload-speed.json).

The [Host patch](../native-host/dev-reload.patch) checks the complete normalized Module definition
when it plans an update and again when it applies it. It compares the definition version and its
BSATN encoding, including reducer arguments and private tables, so it refuses additions that
SpacetimeDB's normal migration check permits. Type ordering changes can also need a planned update.
The normal check does not protect the Host's binary argument and delivery types, and nothing checks
contracts encoded in strings or packet bodies, such as the `native_reply:` envelope. Watch sends one
authenticated PUT to the loopback Host with compatible migration policy and `clear=false`. The Host
checks compatibility before replacement and confirms the checked program hash inside the
transaction.

During Development reload the Host keeps a recovery window through
[`retention.rs`](../native-host/src/retention.rs) and the
[persistence patch](../native-host/retention.patch). A log segment rotates after a write reaches
256 MiB, so a segment can exceed that by one transaction. Rotation requests a snapshot. Cleanup waits
until the snapshot's transaction is durable, then verifies the two newest completed snapshots
within that boundary: contents, identity, replica and offset. It persists their directory entries
before it removes older snapshots. It removes only a closed log prefix that ends before the first
transaction the older retained snapshot needs, and updates the open log's offset list under its
write lock. Each segment deletion reaches disk before the next one starts.

Development reload turns off background snapshot and log compression, so compression cannot
recreate a file after cleanup deletes it. The [snapshot patch](../native-host/independent-snapshots.patch)
gives each completed snapshot its own object files, copied after the committed-state lock is
released and before the snapshot is available for recovery. Damage to one snapshot cannot reach the
other through a hard link. Snapshot creation stays serial. Normal Hosts keep upstream compression
and history behaviour. A cleanup failure stops that pass and logs a warning. The window is not an
archive or a disk limit: recovery still needs one readable retained snapshot and the log after it.
On ext4 and similar filesystems, independent snapshots cost extra disk and writes; Linux
filesystems with copy-on-write cloning share storage until a file changes. On macOS the Host writes
full copies, even on APFS. A failed snapshot can leave an incomplete directory, and cleanup leaves
incomplete and invalid snapshots in place.

A compatible update replaces the Module in the running Host. Sockets, header ciphers and World
Session rows remain. A Rust build failure sends no update. A malformed WASM or a changed definition
fails before replacement. An HTTP failure can leave the outcome unknown, and the watcher reports
the failure instead of claiming a rollback. The watcher does not pause gameplay, restore durable
data or make changed gameplay semantics compatible with existing state.

Host changes still need a restart, and schema changes need a planned update. See
[the workflow](./quickstart.md#development-reload) for startup and failure handling.

### Native protocol coverage

The native protocol check covers SRP logon, world proof authentication, Character creation,
enumeration and deletion, world entry, player motion, entity CREATE/VALUES/DESTROY, stealth visibility,
melee stance and damage messages, spell casts and interruptions, delayed spell damage, combo points,
Aura timers,
pet commands, action bars, Hunter pet rename, care and progression updates,
inventory gains, swaps and equipment, permanent enchants on items and worn gear,
quest acceptance, credit, rewards, the next quest in a chain,
expiry and abandonment,
vendor purchases, partial sales and buyback, bank access and storage, training, spellbook and skill
notifications, talent learning and reset, gossip menus and home binding, taxi discovery, fares,
flight splines and landing, XP and level-up notifications, exploration and rest updates, reputation
and watched factions, Loot Tags, corpse loot, local Loot Rolls and group money shares,
object queries, control receipts, unavailable-service replies, same-map teleports and map changes,
release and reclaim, resurrection offers, Soulstone, spirit-healer interaction, corpse decay,
zone weather, breath and fatigue timers, drowning and exhaustion, Speech, whispers, away status,
Realm Chat Lines, flood limits, Chat Channel operations, friends and ignores,
friend presence notifications, `/who`,
party and raid requests, rosters, target icons, ready checks, minimap pings, random rolls and member stats,
guild creation, membership, settings, rosters, presence, charters, petitions and emblem purchases,
trade proposals, offers, acceptance and item and copper exchanges,
duel requests, countdowns, boundaries, completion and winner announcements,
Meeting Stone admission, queues, Stone Adds, reminders and party rosters,
mailbox access, delivery, COD payment, returns, private text and Letter Copies,
auction browsing, listings, bids, buyouts, cancellation, expiry and private notices,
AreaTrigger exploration and dungeon entry, creature movement and facing splines,
GameObject changes, encounter equipment, ground-effect visuals and Instance-removal notices,
Transport creation, template queries, boarding, Passenger carriage, late observers and leaving,
Transport map crossings with their Passengers' loading screens and arrival aboard,
Addon bridge requests and local Companion command admission,
logout and replacement of a World Session.

[The native check](../native-host/check.py) runs these paths against a disposable Shard and the
real Module. It also checks rollback after partial Character creation, stale Session keys, logon
limits, menu changes between display and selection, ordered taxi takeoff, stale teleport
acknowledgements, recovery after connection loss during loading, and failed crafts that send no
inventory notifications. Crafts plan reagent consumption and every product before they commit
inventory changes; a triggered inventory change invalidates that plan. The check does not
establish every gameplay path or real-client presentation. The
[verification ladder](./testing.md#verification-ladder) lists what each check proves.

### Handler rules

These are the rules each handler applies that its code does not make obvious at a glance.

**Corpses.** Corpse AOI uses map, Instance and cell indexes. Bodies and bones keep the same AOI
Grant until visibility ends or the Corpse expires. Reclaim selects the Actor's own Corpse and checks
distance, map, Instance, ghost state and the remaining delay. Resurrection offers and
self-resurrection fields go only to the owner. The spirit-healer handler checks the selected NPC
and interaction range. See the [Corpse migration](./danger-zones.md#corpse-aoi-migration) before
you publish to a Shard with existing rows.

**Environment.** Weather delivery uses the Module's terrain-derived zone and an index of native
connections in that zone. Entry sends the current weather; later changes reach only that zone.
Client zone claims do not change this state. Breath and fatigue notifications go only to the
owner. Death, godmode and world removal clear the breath timer through the WorldEntity lifecycle.
The 1-second environmental tick ends the fatigue timer for godmode and for a dead Character that
has not released.

**Chat.** The [chat handler](../module/src/native/chat.rs) resolves whisper targets, reads ignore
and away state, and selects Speech recipients. Speech queries indexed Character cells
independently of AOI Grants, so a yell can reach 300 yards. Zone-wide creature messages use the
zone index and check map and Instance. Realm Chat Lines use the group, guild and channel Gates,
with ignore checks at delivery. The [channel handler](../module/src/native/channel.rs) resolves
named targets and sends notices. Channel membership survives map changes and ends on logout or
connection loss; a departing owner hands ownership to a remaining member. The
[social handler](../module/src/native/social.rs) reads contacts and current presence, and friend
notifications use the contact target index. `/who` lists Characters in the world through the shared
protocol filters. Its one-second limit and the flood limits belong to the connection and survive
Character logout. Chat Refusals that occur before gameplay changes return normally and keep the
flood count; GM command Refusals still roll back. System messages emitted during world entry replay
after the connection joins.

**Groups.** The [party handler](../module/src/native/party.rs) resolves names, applies the Module's
group rules and sends group events to members. Invites check presence and faction. Companions
answer invites through their consent Gate. Roster updates include current presence, and world
entry restores the roster and target icons. Ready checks, minimap pings and random rolls each have
a one-second limit on the connection. The [member-stat handler](../module/src/native/member_stats.rs)
answers full requests only for group members. A five-second scheduled update per grouped connection
sends deltas for members outside AOI. Private snapshots keep the prior Aura and pet fields so
removals clear the client. Loading members keep their previous snapshot, and full requests report
that they are loading. Logout and group destruction delete the connection's scheduled updates.

**Guilds.** The [guild handler](../module/src/native/guild.rs) reads Character facts and applies
membership and rank permissions in the same transaction. Rosters show loading Characters as online
and filter officer notes by the recipient's rank. Membership changes update guild ID and rank
through AOI. Login sends the MOTD to the Character and sign-on to other members; map changes
announce no sign-off. [Charter and emblem purchases](../module/src/native/petition.rs) commit
copper, items, guild state and notifications in one transaction. Petition signatures use the actual
Account and Character facts. Turn-in creates the Guild and removes its charter and signatures
atomically. World entry restores charter petition fields and removes inert charters.

**Trade and duels.** The [trade handler](../module/src/native/trade.rs) applies requests to the
Trade Session. Offer and status events select the two recipients in the same transaction, and
exchanges commit items, copper and client notifications together. Death, logout, map changes and
Instance changes cancel trading; cancellation reaches the departing Character before map loading
clears delivery. The [duel handler](../module/src/native/duel.rs) sends the flag and duel state to
its participants, and scheduled Duel transitions emit packets in the same transaction. Completion
clears both participants' arbiter and team fields, then removes the flag. A winner announcement
also reaches Characters in nearby AOI cells on the same map and Instance.

**Meeting Stones.** The [meeting-stone handler](../module/src/native/meeting_stone.rs) reads race
and class from the Character rows and applies admission and queue changes in one transaction.
Seekers use their World Session for presence, and logout removes solo Seekers. Stone Adds send
party rosters directly.

**Mail.** The [mail handler](../module/src/native/mail.rs) checks the mailbox, resolves the
recipient and applies the send, take, return or Letter Copy in one transaction, without Escrow.
Delivery timers send arrival notices to the recipient's World Session. Mail bodies require the
recipient or possession of the matching Plain Letter. The shared codec bounds inbox replies by both
the 50-letter limit and the WoW frame size. The item-text query's 32-bit item hint matches owned
items and the requested text ID, so the hint is never compared with a full 64-bit item GUID. The
generated request and [CMaNGOS's reader](https://github.com/cmangos/mangos-classic/blob/master/src/game/Mails/MailHandler.cpp)
leave the following word unnamed; it grants no access to text.

**Pets.** The [pet handler](../module/src/native/pet.rs) applies Hunter pet rename Gates. Pet CREATE
sends the owner's summon link and action bar, and removing that AOI Grant clears both. Hunter pet
state changes send name-cache fields through AOI, and care and progression fields only to the owner
while its Grant holds. Grant replay restores the same recipient-specific fields.

**Auctions.** The [auction handler](../module/src/native/auction.rs) checks the actual auctioneer
and Character before browsing or changing a listing. Listing, bidding and cancellation commit purse
changes, items, Auction Mail and recipient notices in one transaction. A listing receipt supports a
repeated sell request and keeps the snapshot for expiry. Buyouts require the full offer to be
affordable but debit only the accepted price. Owner and bidder pages use their Character indexes.
Browse filters and usable-item checks run in the Module. Notices go only to their named recipient,
including scheduled expiry notices.

**Teleports and AreaTriggers.** The [teleport state](../module/src/native/teleport.rs) belongs to the
Module. Near teleports require the current counter and mover GUID before movement and gameplay
requests resume. Map changes clear delivery permissions before they remove the live Character. The
world-port acknowledgement rebuilds the Character and sends self CREATE before AOI replays.
Connection loss during loading marks the Character offline and keeps its destination for the next
login. The decoder uses the unpacked acknowledgement GUID in build 5875, as
[CMaNGOS](https://github.com/cmangos/mangos-classic/blob/master/src/game/MotionGenerators/MovementHandler.cpp#L240)
reads it. AreaTrigger requests check the Actor's map and position against the imported sphere or
rotated box before exploration credit or teleport. Dungeon entry resolves party ownership and
Character bindings. Separate Instances keep separate AOI Grants, even on the same map.

**Creatures and objects.** The [creature movement handler](../module/src/native/creature.rs) sends
spline changes through AOI. Curved movement stores its complete path before delivery. A late
viewer receives CREATE, then the remaining path and duration with the original spline ID. Completed
legs and stops do not replay; stationary facing does. Replay requires the creature's current map and
Instance to match the spline. A curve the client cannot represent sends a stop instead of malformed
path bytes. Run-speed changes send the forced-speed packet to the owner and the spline-speed form
to observers. Stand-state changes acknowledge the owner, including forced standing after damage.
GameObject CREATE is followed by all four rotation fields. Later changes send VALUES through the
object's AOI Grants without replacing the client object or its Grant. Encounter equipment changes
send virtual item displays through creature AOI, and late entry restores them after CREATE.
DynamicObjects use a separate AOI layer and a map, Instance and cell index. Ground-area expiry
removes their Grants and sends DESTROY. Existing rows need the
[DynamicObject cell backfill](./danger-zones.md#dynamicobject-aoi-migration).

**Transports.** The [Transport module](../module/src/transport/mod.rs) runs each imported route
from the Realm Clock. Every Character in the open world of a Transport's current map holds its AOI
Grant, and those Grants come before other CREATEs so a Passenger's CREATE can name its Transport.
The CREATE carries the Path Progress as the phase within the current cycle, not as an uptime, and
the client moves the Transport from it. The world database's period can differ from the client's
by milliseconds per cycle. A small Path Progress keeps that error from multiplying over thousands
of cycles, so the client drifts only from its latest CREATE. Movement that names a Transport on the
mover's map boards a Passenger when the mover stands within 30 yards of its claimed place on the
Transport; later movement updates the offset, and movement without a Transport ends the Passenger.
As in CMaNGOS, the Module refuses an offset beyond 50 yards across or 100 yards up. Passengers, and
Characters leaving a Transport, are exempt from movement-violation scoring. While Passengers
remain, a 500 ms schedule moves their world positions with the Transport for AOI and zone changes;
it sends no movement. Late observers receive the Passenger's CREATE with the Transport GUID and
offset. Teleports and leaving the world end the attachment.

A route that crosses maps has a one-shot schedule for its next map change. World entry arms it. When
it fires, Characters on the old map lose the Transport's Grant and Characters on the new map gain
it. Each Passenger on the old map teleports with it, as CMaNGOS `Transport::TeleportTransport`
does: `SMSG_TRANSFER_PENDING` names the Transport and the old map, and `SMSG_NEW_WORLD` carries the
offset instead of a world position. The attachment survives the loading screen. On arrival the
Transport's CREATE precedes the Character's own CREATE, which names the Transport and the offset.

**Instance Removal.** Inserts and cancellations send the countdown only to the affected Character.
World entry replays the remaining time, or clears a stale countdown. Scheduled removal clears the
countdown before it recalls the Character home.

**Addon bridge.** The bridge parses STC envelopes before ordinary chat. Its connection limit allows
a burst of twenty commands, then two per second, and survives Character logout. Replies use the
recipient Identity's live Character and its connection. Malformed STC envelopes cannot enter party
chat. Commands received at Character selection or during a teleport do not run.

**Companions.** Companion commands use the local Group roster, leader, Sessionless Action Consent
and current map and Instance. The Module checks the optional authority member and exact combat
target before it calls the Package command handler. Application, the result and the private reply
commit in one transaction. A Package result of `WaitingForCapacity` rolls back the request.
Companion invitations and departures run as local Group actions, with consent checked in the same
transaction; a sessionless invitee accepts only when its own consent permits it. Sessionless
AreaTrigger crossings admit the party Instance, move the live entity and persist the Character in
one transaction. They keep vitals and generate AOI departures and arrivals for observers. A
Character Relocation row records the latest completed crossing for a repeat call, and Controller
selection clears it.

## Durable state and extension points

The Module owns Characters, Accounts, inventory, groups, guilds, Mail and auction state in one
Shard. Spatial reads use the indexed partition helpers, and AOI uses map, Instance and cell indexes.
Whole-world scans are limited by the Architecture Tests in `module/src/tripwires.rs`.
[`schema.md`](./schema.md) maps the tables.

Native connection, cipher and delivery tables are private. The Host connects in process as the
Shard publisher, and WoW clients receive only the packets the Module selects. HTTP access still
requires the Operator Gates; public tables are not a substitute for them.

Packages compile into the same Module and have its privileges. `module/build.rs` discovers their
source and registry markers. The supported operations and compatibility rules are in
[the Package API](./package-api.md).
