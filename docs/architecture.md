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
standalone. It uses in-process Core connections and one private output subscription per Shard, shared by
that Shard's native clients, with no SDK or WebSocket connection to the Module. It passes native reducer arguments as
BSATN. BSATN has no field names and the reducer call does not check for trailing bytes, so the
Host's arguments must match the reducer parameters' order and layout. The Host owns secure random
seeds, header ciphers, framing, socket deadlines and bounded output queues. It applies the grant and
revoke instructions the Module issues; it does not calculate spatial visibility or gameplay Gates.

If a Module replacement closes the selected Module before dispatch, the Host retries that
unexecuted request once against the current Module. It does not retry failed reducer outcomes or
requests whose outcome is unknown.

A World Session with a Motion Permit has its movement relayed by the Host
([ADR 0006](./adr/0006-relay-motion-in-the-native-host.md)). The Module holds the permit while the
Character moves freely on foot and withdraws it in the transaction that roots, kills, seats on a
taxi or teleports the Character, or that sees a Transport flag. The Host checks that a movement
message is well formed, on foot and newer than the last one, then sends it straight to the mover's
AOI Grant holders. It forwards all movement except heartbeats, and one heartbeat sample a second or
after 4 yards, to `native_world_motion`, which applies it without a Module relay. Without a permit,
or when a check fails, movement goes to `native_world_receive` as before. The shared opcode lists
are in `native-host/src/motion_opcodes.rs`.

Output a connection has not taken yet waits in one queue, and the Host writes all of it at once, in
64 KiB chunks with a 2-second deadline each. Host relays wake writers every 20 ms. A newer
Coalescible Motion Relay for the same mover supersedes a queued one and returns its bytes to the
connection's 2 MiB output limit, so a slow reader falls behind on motion before it reaches the
limit. Discrete movement and every other packet always reach the client in order. Output after a
close is not sent.

`game_native_packet.id` orders delivery instructions within a transaction. A grant's CREATE comes
before the packets delivered through that grant. A revoke removes its recipient before later
packets can use it. Logout clears every delivery permission of the connection before it returns to
Character selection. The Host applies these instructions in order. A failed output subscription
closes its native clients, because the Host cannot continue from a missed instruction.

A Handoff marker moves a World Session to another Shard on the same Host. The Host writes the
connection's packets before the marker and drops the rest of the source Shard's output for it. It
attaches the connection to the target Shard under the same connection id and calls
`native_world_adopt`, which builds the World Session from the target's `game_native_handoff` row.
The source Shard sees an ordinary disconnect. A request still in flight to the source Shard may
fail without ending the World Session. Only a World Session at Character selection can move yet.

World entry registers the Character as an AOI viewer before self CREATE and replays its grants
after it. A Passenger's Transport is the one grant replayed before self CREATE.
Sessionless Characters stay AOI subjects but allocate no viewer Grants, so Companion populations
create no delivery subscriptions for Characters without native connections.

A request handler returns a Rollback when the request does not commit. `native_world_receive`
writes a Refusal rollback as a `native_reply:` envelope, a JSON list of Module-encoded packets. The
list is empty for a silent Refusal. The failed transaction rolls back. The Host parses the list and
submits its packets to `native_world_reply` on the same connection, which emits the Refusal through
the ordered output stream. The Host does not classify gameplay errors. A failure rollback returns
plain text, and the Host closes the connection.

### Host, listeners and admission

The host extension pins SpacetimeDB 2.14.0 at `7c3ee551755dc0152d9e0b0d5241fff939d3eda0`
([`prepare-host.py`](../native-host/prepare-host.py)). The Module uses the same version. The listeners read `LYRACORE_NATIVE_LOGON_ADDR`,
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

**Guilds.** The [guild handler](../module/src/native/guild.rs) takes Actor facts and an invitee
lookup, applies membership and rank permissions, and returns the direct reply. Its caller owns
authentication, invitee Gates and rollback on error. Guild events emit in the mutation transaction.
Rosters show loading Characters as online and filter officer notes by the recipient's rank. Membership changes update guild ID and rank
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

**Meeting Stones.** The [meeting-stone handler](../module/src/native/meeting_stone.rs) checks the
GameObject, partition, distance, taxi state and level on the Character’s World. On a single Shard,
admission and queue changes share one transaction. Across World Shards, the Realm owns the Queue
and Stone Adds as described below. Logout removes solo Seekers.

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
and Character before browsing or changing a Listing. On a single Shard, Listing, bidding and
Cancellation commit purse changes, items, Auction Mail and recipient notices in one transaction.
Across World Shards, the Realm owns Listings and decisions; World payments settle as described below. A listing receipt supports a
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
rotated box before exploration credit or teleport. Client requests allow five yards beyond the
volume because the request can precede the next movement update, as in
[VMaNGOS's range Gate](https://github.com/vmangos/core/blob/development/src/game/Handlers/MiscHandler.cpp#L585-L589).
The same margin applies when asynchronous Instance admission completes. Sessionless crossings and
content geometry use the exact volume. Dungeon entry resolves party ownership and
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
Shard, or in the World Shard of a Realm that has a Realm Shard. Spatial reads use the indexed partition helpers, and AOI uses map, Instance and cell indexes.
Whole-world scans are limited by the Architecture Tests in `module/src/tripwires.rs`.
[`schema.md`](./schema.md) maps the tables.

Native connection, cipher and delivery tables are private. The Host connects in process as the
Shard publisher, and WoW clients receive only the packets the Module selects. HTTP access still
requires the Operator Gates; public tables are not a substitute for them.

Shards of one Realm call each other through SpacetimeDB outbox tables, as calls between databases.
Every Shard runs the same Module, so the outbox bindings in `module/src/remote_bindings/peer` come
from the Module's own `idc_` reducers; `scripts/idc-bindings.py` regenerates them, and CI checks
that they are current. An `idc_` reducer takes plain arguments and refuses a caller that is not a
Peer Shard. A call arrives as a short connection from the calling Shard, so `client_disconnected`
skips Peer Shards. Delivery is exactly once, and each call waits for disk writes on both sides, so
calls carry rare events and batches, never per-packet traffic.

A Realm can split into a Realm Shard and World Shards on the same Native Host. The Native Host's
logon and world listeners point at the Realm Shard, which keeps logon, Accounts, Sessions and the
Login Queue. `ShardRole::Realm { world }` names its default World Shard. Without explicit map
assignments, the existing one-World setup keeps routing creation, login and deletion to it.
Without any role, a Shard still holds the whole Realm, including Argus.

For two continents, the Operator lists each World Shard and the Realm Shard as Peer Shards of each
other with `operator_add_peer_shard`, then sets their roles with `operator_set_shard_role`.
`operator_set_map_owner(map_id, world)` assigns map 0 to the Eastern Kingdoms World Shard and map 1
to the Kalimdor World Shard on the Realm Shard. The Operator repeats both assignments on each
World Shard and lists the World Shards as Peer Shards of each other. A map has one owner, and reassignment is refused.
Once a World Shard has explicit assignments, creation and world entry refuse maps it does not own.
Existing Characters must already be on their owning World Shard before enabling assignments.

The Realm Shard has no world import. The Operator calls
`operator_set_character_start_map(race, class, map_id)` there for each supported race/class pair,
using the World Shards' imported `game_start_position` rows. This small table contains only routing
facts and supports class-specific starts. Missing routes return a Character-create Refusal. The
World Shard still applies the gameplay rules and checks its actual start map before creation.

The Character directory (`game_character_directory`) maps a Character guid to its Account and World
Shard. Creation and deletion send IDC changes. Setting a World Shard role reports existing Characters
and can be repeated to refresh the directory. Assigning a map on a World Shard also reports its
Characters after the Realm Shard has its map assignments. Directory versions retain delete tombstones
so late answers cannot restore deleted entries. Successful selection replies also apply their directory
changes before the client receives the packet. The version and owner live apart from Character-owned
state. A Transfer changes the owner only through an update from the current World Shard with a
newer revision. World Shards carry a directory clock across each Transfer, so late updates cannot
undo a round trip.

A Character-list request asks each World Shard named by the Account's directory entries. With
explicit map assignments and no Characters, it sends an empty list directly. An existing one-World
setup asks its default World Shard even before its directory has been filled. Replies carry a request
id; the Realm Shard sums their count bytes and appends their entries into one `SMSG_CHAR_ENUM`. Selection requests are serialized
per connection. A refused or failed enumeration closes the World Session, and a ten-second deadline
also closes it if an answer never arrives. Closing a connection removes its pending selection.
Late replies can update the directory but cannot send a packet to the departed connection.
Creation and deletion Refusal packets return through the World Shard's error envelope.

Login and deletion route by Character guid and Account. Login sends the owning World Shard Account
and Session copies, then hands the World Session over by Handoff. Logout hands it back. The Realm
Shard records which World Shard holds the away World Session, so changed Accounts and Sessions go
to that Shard. The Handoff row retains the Character guid for arrival. The Login Queue counts only
World Sessions on the Realm Shard.

A cross-map teleport to a Peer World Shard starts a Character Transfer when the client acknowledges
its world port. The source leaves the world and sends one manifest batch over IDC while both copies
stay fenced by Escrow. A successful import permits the source finish cascade. The Realm Shard then
moves the Character directory entry and the away World Session together. The target releases its
arrival fence before the source writes the Handoff marker. Adoption enters with WorldPort and the
saved teleport counter, on the same connection. A refused import restores the source position and
closes the World Session. See [Character Transfer](character-transfer.md) for crash recovery.

Whispers and `/who` route through the Realm Shard. It asks each configured World Shard for the
current matches, then checks the replies against the Character directory and away World Session.
`/who` keeps the existing faction, name, level, guild, class, race and zone filters and 49-entry
limit. A refused query or a missing reply after 15 seconds sends an unavailable notification; it
does not report a partial list or an offline whisper target. Ambiguous online names also return
unavailable. Name reservation across World Shards remains separate work.

Replies carry the original Account, Character guid and connection id. Delivery uses the current
World Shard and repeats the recipient and ignore Gates there. A refused delivery retries every
250 ms for at most 15 seconds, so a resolved recipient can move through Transfer without receiving
duplicate packets. An in-flight IDC is never retried. A reply cannot reach a different Character
or a replacement connection, and delivery waits only while that Character remains away from the
Realm Shard. These are independent World Shard snapshots, not a simultaneous Realm snapshot: a
Character in the Transfer gap may be absent from a query, and away or zone facts can change after
the query.
No movement updates are sent to the Realm Shard for this feature.

Mailboxes stay with their Character on its World Shard; letters cross Shards through the Realm as
described below. The Realm owns Auction Listings and serializes bid decisions across World Shards.
Guild membership and communication use the Realm authority described below. Friends and ignores use the Character profiles and presence described below. A Transfer carries the existing Character-owned manifest, not
Shard-owned social or economic state. Casts in progress are cancelled; melee, threat, AOI Grants,
Transport attachment and other omitted transient state do not survive. World departure also applies
the existing logout cleanup, including buyback cleanup and pet despawn. A Shard without a role or
map assignments retains its local teleport behavior. Sessionless relocation to an unowned map is
still refused.

Packages compile into the same Module and have its privileges. `module/build.rs` discovers their
source and registry markers. The supported operations and compatibility rules are in
[the Package API](./package-api.md).

### Contacts across World Shards

Contact lists remain Character-owned and travel in the Transfer manifest. Each World Shard reports
Character names, race, class and gender through the Realm directory, including Characters that have
never logged in. The Realm checks each report against the current World owner, then distributes
private, versioned Character profiles to its World Shards. Profile revisions are ordered separately
from directory revisions: a newer Character-selection reply must not discard an unseen profile.
A delayed report cannot reverse a directory deletion or replace facts from the current World owner.
Deleted profiles remain as tombstones. Refused profile deliveries retry while the destination is still
a Peer Shard. The old directory reducer remains available for outboxes committed before the upgrade.

Friend and ignore additions resolve these profiles and apply the existing self, faction, duplicate
and list-cap Gates in the owning World Shard transaction. Ambiguous names are refused. Name queries
also use the profiles, so the client can display remote contacts. Friend lists and login/logout
notifications use the existing Group presence projections, even for Characters without a Group.
Zone, level and away-status changes refresh that presence. Repeated or older presence revisions do
not send duplicate login notifications. Presence and profiles are asynchronous: a recent change may
remain unseen until its projection arrives; a Transfer may briefly show a friend offline.

A deletion removes references from contact lists on each World Shard. World entry and list requests
also discard contacts whose deleted profile arrived before a Transfer imported the list. Local
contact rows, including ignores used by whisper delivery, retain their existing ownership and Gates.

For an existing Realm, publish the Module to all Shards with gameplay closed, then call
`operator_report_character_profiles` on every World Shard. Wait for the Realm profiles to match the
Character directory and for profile outboxes and retries to drain. Call the same reducer on the Realm
Shard to replay all retained profiles, including deletion tombstones, and wait for those deliveries
before reopening gameplay. Login repopulates online presence. Repeat the profile replay when adding
a World Shard. No contact rows move during this upgrade.

### Guilds across World Shards

`native::realm_guild` routes native guild requests and GM creation to the Realm Shard. It checks
Character ownership and the current World Session before applying membership, rank, invitation,
MOTD, information and note rules. Guild and officer chat use the same Realm delivery path as group
chat. Roster replies filter officer notes using authoritative rank rights.

World Shards report guild presence and ignore lists as private, versioned facts. The Realm refuses
reports from former Character owners and old connections cannot take a replacement connection
offline. Invite admission uses the latest accepted target facts, then delivery repeats the ignore
Gate on the recipient's World Shard. These are asynchronous facts, not a simultaneous Realm
snapshot. Login, logout, level, zone and ignore changes refresh them.

Guild snapshots and individual memberships have separate revision records. A guild snapshot never
implicitly removes its former members: each membership change carries its own revision and a
retained deletion record. This prevents a delayed roster from clearing a newer guild membership.
World projections update the existing guild tables and AOI fields. Character Transfer preserves
these projections; actual deletion reaches the Realm through the Character directory and applies
leader succession or disband there. Refused projections retry while the destination is a Peer Shard.
`operator_report_realm_guilds` replays current guilds and disband records.

For existing state, keep gameplay closed across all Shards and backfill Character profiles first.
Call `operator_report_guild_characters` on every World Shard and wait for the Realm's
`game_guild_character` rows to cover the resident guild members. This preserves offline level,
class, zone and last logout without making members log in; it refuses a World with active players.
Capture each World Shard's `game_guild`, `game_guild_rank`, `game_guild_member` and
`game_guild_invite` rows. Resolve conflicting guild ids, names, memberships and invitations before
importing. Call `operator_import_guild` on the Realm with one `Import` object per guild, containing
`guild`, `ranks`, `members` and `invites`. It preserves guild ids, rank numbers, notes, invitations
and join times; internal rank row ids are reassigned. It refuses occupied ids or names and
inconsistent Character facts. Imported ids advance the Realm's persistent guild sequence.

A World Shard with unmigrated guilds refuses native guild mutations. Its first projection must
match the captured local guild state before it can replace that state. Wait for every World
projection and retry to settle, then compare memberships, ranks and settings with the capture.
The import reducer refuses a Realm with Characters away in World Sessions. This Gate does not
replace closing the listeners and checking every Shard before migration.

Guild charters, signatures, renames, offers, declines, founding and emblem purchases also use the
Realm. The World checks NPC access and the purse, debits copper and creates a purchased charter
before queuing its durable request. `game_guild_transaction` records that request until its result
settles. Realm refusal returns the copper and removes the provisional charter; acceptance keeps the
charge. Founding consumes the charter only after Realm acceptance. A repeated result cannot charge,
refund or consume again. A missing transport response is not permission to refund.

Refunds update the live purse or the stored Character after logout. If the purse has filled while
the request was pending, settlement credits only the space available and retries the remaining
copper. Cross-map teleport, Transfer and Character deletion refuse while settlement is pending.
Ordinary movement, logout and other inventory actions can continue. Charter destruction during a
request is reconciled after its result. Login compares held charter item GUIDs with the Realm's
petition, closes a petition whose charter is missing, and removes inert items only after an explicit
Realm absence result. Pending reconciliation and closure retry refusals; they also prevent Transfer.

Before reopening an existing Realm, capture `game_guild_petition`,
`game_guild_petition_signature` and the matching owned charter items on every World Shard. Resolve
petition id conflicts and missing charter items. With gameplay closed and Character profiles
backfilled, call `operator_report_guild_petitions` on each World. It sends each petition and its
signatures to the Realm, preserving petition ids, item GUIDs, Account attribution and timestamps.
The Realm validates owners, signers and conflicts, then advances its petition id sequence. Only a
successful result removes the original World petition and signatures; charter items stay in their
inventory. Wait for source transactions to drain, require the source petition tables to be empty,
and compare Realm rows and World items with the capture. A refused import leaves its source rows
for correction and another report. Reopening gameplay before these checks is unsupported.

Publish the Realm Module before the World Modules and drain guild outboxes before changing their
private operation encoding. Sessionless guild mutations remain refused in the split topology;
single-Shard purchases and petitions retain their local atomic transaction.

### Mail across World Shards

The sending World checks the mailbox, resolves the recipient from Realm Character profiles, and
commits postage, attached copper and removal of the attached item with the Mail. A local recipient
receives it in that transaction. Otherwise `native::realm_mail` moves the complete letter into a
durable IDC outbox addressed to the Realm. It preserves the creation time, Delivery Delay, sender
kind, COD price, item state and any attached Letter Copy's text. This path also carries returns,
COD payments, Reward Letters and Auction Mail.

The Realm routes each letter through the Character directory. The receiving World requires a
resident Character with no Transfer fence, then commits the Mail, Item Text and Mail Timer together.
IDC executes each call once. A refused destination rolls back and retains the letter in a scheduled
retry; the Realm consults the directory again before every attempt. Transport delay does not refund
or duplicate value. An absent directory entry waits for a profile; a deletion tombstone returns a
Character's letter once, or discards a letter that cannot be returned.

Inbox reads, item pickup, COD debit, Letter Copy creation and money pickup remain local transactions.
An accepted COD pickup queues its payment in that same transaction. Mail transfers with its recipient;
Mail Timers wait behind the Transfer fence and resume after release or rollback. Transfer carries the
Item Text referenced by inventory, Mail and legacy value rows, allocating new local text ids and
rewriting references on arrival. Text ids share the Mail id space, so importing text consumes an id
before future Mail can use it. Older destinations refuse the additional text payload.

With gameplay closed on an existing split Realm, publish the Realm and every World, backfill Character
profiles and wait for the profile outboxes to drain. Capture Mail, attached items and Item Text on
each Shard, then call `operator_route_mail` on each World to move letters whose recipient is elsewhere.
Compare recipient totals and attachment contents after Mail outboxes and retries drain. Ordinary
resident inboxes need no migration. Do not transfer Characters carrying Letter Copies to an older
Module. A missing text record refuses the delivery or Transfer rather than dropping the text.

### Groups across World Shards

`native::realm_party` routes native party and raid requests to the Realm Shard. The Realm checks
the requesting Account, Character, connection and source against its directory and away record.
It applies the existing Group rules and sends versioned Group projections to its World Shards.
Per-Character membership revisions survive Transfer, including tombstones after departure, so a
late Group snapshot cannot undo a newer membership. Disband snapshots settle rolls on the World
Shard that holds the loot; each roll records its Group, so a late disband cannot settle a new
Group's rolls. A projection restores membership after
Character import; membership itself does not move in the Transfer manifest.

Presence carries Character names, faction, class and Member Stats. World entry and departure
publish presence; the existing five-second Member Stats schedule refreshes grouped members and
their roster projections. A World Shard change counts as new presence. Refused projection deliveries
retry after one second while their destination remains a Peer Shard. Applying a snapshot compares
the visible roster before and after the change, so a newer presence snapshot also repairs missed
client roster notifications. Transient Group events expire after fifteen seconds.
The Realm checks current connection ownership before accepting those facts. World Shards retain
remote Member Stats for their local party frames. Group chat selects the authoritative Realm
audience and uses the same connection-bound delivery and ignore Gates as whispers. Group events
are delivered only to their original connection; login restores the roster and pending invite.

Shard-local Group mutations are refused when a World Shard has a Realm Shard. Sessionless Group
actions are not supported in this topology; single-Shard behavior is unchanged. An existing multi-Shard Realm requires migration of its Group state before
this revision is deployed. This change does not supply an automatic Group migration.

Dungeon admission uses this Realm-owned membership and durable Instance Placements across the
configured Instance Shards. See
[Instance lifecycle](instance-lifecycle.md#dedicated-instance-shard) for admission, return and reaping.


### Meeting Stones across World Shards

The World admits a Meeting Stone use against its resident Character and GameObject. It sends the
admitted dungeon area through the same ordered command stream as Group requests. The internal JOIN
body contains that area as a four-byte integer; the client still sends a GameObject guid. The Realm
checks the source World and current World Session, reads authoritative Group membership and class
facts, and runs the existing Queue and Stone Add rules. Team and dungeon area separate the queues.
LEAVE and INFO use the same stream, so they cannot overtake that connection’s admitted JOIN.

The Realm owns `game_meeting_stone_seeker` and `game_meeting_stone_party`. Stone Adds use the Realm
Group join core. Group publication includes every roster affected by the resulting events, including
Parties formed or filled by another Character’s request. Queue status, completion, added-member
notices and five-minute reminders use Group delivery, including its retry, connection and expiry
Gates. INFO reads current Realm queue state. No World maintains a second Queue authority.

A solo Seeker remains queued through Transfer in the same World Session. A Seeker whose current
World presence is unavailable is skipped; fresh presence resumes matching after arrival.
Returning to Character selection, disconnecting or replacing the World Session removes a solo
Seeker. Queued Party membership follows the existing Group rules, including offline members,
leader-only withdrawal, kicks and disband. Late presence cannot recreate a removed Seeker.

Stored layouts, IDC signatures and client packet layouts are unchanged. Publish the Realm and every
World before reopening gameplay. Queues from a single-Shard topology are transient: drain them
before splitting that topology; World-local queue rows are not imported. The Group migration
requirements above still apply.

### Auctions across World Shards

`native::realm_auction` sends admitted requests to the Realm. Worlds check the resident Character,
auctioneer, distance and item or copper before committing a durable request. The Realm owns
Listings, standing bids, receipts and expiry. Alliance, Horde and neutral remain separate Auction
Markets. Browse, owner and bidder pages read that authority; usable-item filtering uses the World’s
admitted level, class and learned proficiencies. The Realm needs the same item catalogue as the
Worlds. No Listing projection can become a second bid authority.

A World removes the listed item and debits the deposit in the transaction that queues the request.
A bid reserves the full offer. Realm acceptance keeps only the accepted price and refunds any
buyout excess. A refused Listing returns its item through Auction Mail and refunds its deposit.
A refused bid refunds its whole offer. A transport result that confirms remote rollback is a
Refusal; an outstanding transport call is not permission to refund or repeat the operation.
Accepted Listing receipts also remain on their source World to recognize repeated sell packets.
The Realm checks that the original Listing is still active before answering a repeat.

Cancellation first asks the Realm for the Auction Cut. The World debits that quote and sends the
Cancellation. The Realm checks the current cut again before committing. If a new bid changed the
cut, the Cancellation refuses and refunds the quoted amount; the seller can retry. An unaffordable
quote produces no answer, as on a single Shard. Logout before the quote is paid leaves the Listing
active; a payment already reserved can finish after logout.

`game_auction_transaction` retains each World payment until the outcome and refund settle.
Refunds credit a live purse or its stored Character. If it fills meanwhile, remaining copper waits
for space without saturation loss. Transfer, cross-map teleport and Character deletion refuse while
that transaction remains. Ordinary movement and logout continue. A result is applied once and only
the originating connection receives its command reply. Realm-to-World results retry after a
Refusal. Auction Notices use the existing bounded Realm delivery path; Auction Mail remains durable
and follows its recipient through Transfer. The Realm refuses deletion of a seller or standing
bidder while it owns Auction value.

Before sending Character deletion to its World, the Realm records `game_auction_deletion` and
refuses later Auction requests for that Character. This closes the ordering gap between the
selection and Auction delivery streams. The guard remains after a client disconnect or selection
timeout. Only the matching World result or confirmed request rollback clears it; a successful
result updates the Character directory before clearing the guard.

With gameplay closed, publish every Shard and backfill Character profiles before migrating existing
Listings. Capture each World’s `game_auction`, `game_auction_operation_receipt` and referenced
`game_item_text` rows. Call `operator_route_auctions` on each World. The Realm imports each Listing
with a new local id, preserving the operation id, seller, bidder, bids, deposit, rates, item state
and dates. Item Text gets a fresh local id. Missing receipts or text and conflicting operation ids
refuse import. Resolve those conflicts against the capture before retrying.

World expiry pauses while Listings await migration; World auction requests refuse while any local
Listing remains. Only a successful result removes the source Listing, expiry and receipt and releases
its Item Text reference. Wait for transactions and result retries to drain, require source Listing
tables to be empty, and compare the Realm rows by operation id with the capture. An overdue Listing
can settle on the Realm during this wait, so include its resulting Auction Mail in the comparison.
Repeated migration calls skip owners with work already pending. The reducer’s active-session checks
do not replace closing listeners on every Shard before this procedure.
