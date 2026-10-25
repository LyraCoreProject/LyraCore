# LyraCore architecture

Every claim below cites the file that makes it true. Where this page and the code disagree, the code
wins.

**Authority note:** [`danger-zones.md`](./danger-zones.md) is authoritative over this document and
over every other document in this directory for anything about migrations, publishing, or the
deploy/verify procedure. This page explains *what the system is*; `danger-zones.md` tells you *what
will bite you*.

---

## Native host migration

The replacement runtime uses one Shard for the Realm. Accounts, Characters, maps, Instances and
Realm-wide state share one Module. Map and Instance identifiers remain gameplay partitions. The
runtime removes Gateway routing, cross-Shard Transfer and cross-Shard coordination. Its handlers
and Relays run in the Module; the Gateway crate and process have been removed.

The [native Module](../module/src/native/mod.rs) owns SRP proof verification, logon limits, Sessions,
world authentication, connection state, request dispatch and response packets. The host supplies
fresh cryptographic entropy for each SRP challenge. The [AOI tables](../module/src/aoi.rs) maintain
stable AOI Grants through entity, GameObject, Corpse, DynamicObject and stealth changes. Packet codecs live in the WASM-compatible
[protocol crate](../crates/lyracore-protocol/src/lib.rs).

The [host extension](../native-host/src/mod.rs) adds logon and world TCP listeners to SpacetimeDB
standalone. It uses in-process Core connections and one private output subscription for all native clients. It has no
SDK or WebSocket connection to the Module. It passes native reducer arguments as BSATN. BSATN has
no field names and the reducer call does not check for trailing bytes, so the host's arguments must
match the reducer parameters' order and BSATN layout. The host owns secure random seeds, header ciphers,
framing, socket deadlines and bounded output queues. It applies Module-issued grant and revoke
instructions; it does not calculate spatial visibility or gameplay Gates.

`game_native_packet.id` orders delivery instructions within a transaction. A grant's CREATE precedes
the packets delivered through that grant. A revoke removes its recipient before later packets can
use it. Logout clears all delivery permissions for that connection before it returns to Character
selection. The host applies these instructions in order. A failed output subscription closes its
native clients; the host cannot continue from a missed delivery instruction.

World entry registers its Character as an AOI viewer after self CREATE and before grant replay.
Sessionless Characters remain AOI subjects but do not allocate viewer Grants. Companion populations
therefore do not create packet-delivery subscriptions for Characters without native connections.

An expected request failure returns a `native_reply:` envelope, a JSON list of Module-encoded
packets. The list is empty for a silent Refusal. The failed transaction rolls back. The host parses
that list and submits its packets to `native_world_reply` on the same connection, which emits the
refusal through the ordered output stream. The host does not classify gameplay errors. Other
failures close the connection.

### Development reload

`./lyracore dev watch` runs a local Native Host with `LYRACORE_NATIVE_DEV=1`. It watches Module
source and builds the `dev-reload` WASM profile incrementally. Dependencies use optimization level
1; the Module uses level 0, and build scripts and procedural macros use level 3. Watch requests only
the WASM library from Cargo. Debug information is off. The Host uses Cranelift without optimization
and retains fuel metering and epoch interruption. These settings shorten development builds and
WASM compilation; they are not the release performance configuration.
The development engines share a bounded in-memory Cranelift function cache. Unchanged functions
can reuse machine code across different WASM files. Cached keys and records occupy at most 256 MiB,
plus collection bookkeeping. It evicts the oldest entries when full, which can make later edits
recompile functions. The cache disappears when the Host stops. Normal Hosts do not enable
it. This uses Wasmtime 39's experimental incremental compiler cache.
Local measurements and their limits are in
[`development-reload-speed.json`](../native-host/results/development-reload-speed.json).
The profile also keeps Rust's development assertions and integer overflow checks. These can stop
a reducer where a release build would continue, so a development build can expose additional bugs.

The [Host patch](../native-host/dev-reload.patch) checks the complete normalized Module definition
both when planning an update and when applying it. It compares the definition version and its
BSATN encoding, including reducer arguments and private tables. This deliberately refuses additions
that SpacetimeDB's normal migration check permits. Type ordering changes can also require a planned
update. SpacetimeDB's normal compatibility check alone does not protect the Native Host's binary
argument and delivery BSATN types. It cannot check semantic contracts encoded in strings or packet
bodies, such as the `native_reply:` envelope.
Watch sends a single authenticated PUT to the loopback Host with compatible migration policy and
`clear=false`. It skips the CLI's separate pre-publish request. The Host checks compatibility before
replacement and confirms the checked program hash inside the transaction.

A compatible update replaces the Module in the running Host. The sockets, header ciphers and
World Session rows remain. A Rust build failure sends no update. A malformed WASM or changed
definition fails before replacement. An HTTP failure can leave the update outcome unknown; the
watcher reports the failure instead of claiming a rollback. The watcher does not pause gameplay,
restore durable data or make changed gameplay semantics compatible with existing state.

This workflow covers Module code changes in a local development Realm. Host changes still require
a restart, and schema changes need a planned update. It does not implement paused production
updates. See [the workflow](./quickstart.md#development-reload) for startup and failure handling.

The [recorded Benilla check](../native-host/results/development-reload.json) kept one World Session
through two source edits, a compile failure and Ctrl+C. Build plus publish took 37.85 and 31.62
seconds while recording. This is a development workflow with minimal fixture content, not a
release-build comparison or a populated-Realm performance result.

### Native protocol coverage

The native protocol check covers SRP logon, world proof authentication, Character creation,
enumeration and deletion, world entry, player motion, entity CREATE/VALUES/DESTROY, stealth visibility,
melee stance and damage messages, spell casts and interruptions, delayed spell damage, Aura timers,
pet commands, action bars, Hunter pet rename, care and progression updates,
inventory gains, swaps and equipment, quest acceptance, credit, rewards, expiry and abandonment,
vendor purchases, partial sales and buyback, bank access and storage, training, spellbook and skill
notifications, talent learning and reset, gossip menus and home binding, taxi discovery, fares,
flight splines and landing, XP and level-up notifications, exploration and rest updates, reputation
and watched factions, Loot Tags, corpse loot, local Loot Rolls and group money shares,
object queries, control receipts, unavailable-service replies, same-map teleports and map changes,
release and reclaim, resurrection offers, Soulstone, spirit-healer interaction, corpse decay,
zone weather, breath timers and drowning, Speech, whispers, away status, Realm Chat Lines,
flood limits, Chat Channel operations, friends and ignores, friend presence notifications, `/who`,
party and raid requests, rosters, target icons, ready checks, minimap pings, random rolls and member stats,
guild creation, membership, settings, rosters, presence, charters, petitions and emblem purchases,
trade proposals, offers, acceptance and item and copper exchanges,
duel requests, countdowns, boundaries, completion and winner announcements,
Meeting Stone admission, queues, Stone Adds, reminders and party rosters,
mailbox access, delivery, COD payment, returns, private text and Letter Copies,
auction browsing, listings, bids, buyouts, cancellation, expiry and private notices,
AreaTrigger exploration and dungeon entry, creature movement and facing splines,
GameObject changes, encounter equipment, ground-effect visuals and Instance-removal notices,
Addon bridge requests and local Companion command admission,
logout and replacement of a World Session.
[The native check](../native-host/check.py) runs these paths
against a disposable Shard and the real Module. It checks rollback after partial Character creation,
stale Session keys, logon limits, menu changes between display and selection, ordered taxi takeoff,
stale teleport acknowledgements, recovery after connection loss during loading,
and failed crafts that send no inventory notifications. Crafts plan reagent consumption and every
product before committing inventory changes. A triggered inventory change invalidates that plan.
The check does not establish every gameplay path or real-client presentation. The Gateway crate
has been removed. Existing multi-Shard deployments still need a separate data migration. Cutover must
retire old Gateway leases and Account Claims before opening native listeners on an existing Realm.
Their expiry paths can remove Characters or channel memberships.
Pending guild Fee Holds must settle before native purchases can proceed. Old Mail Escrows must
settle before cutover; native mail does not resume the Gateway's cross-Shard delivery steps.
Old auction Holds must also settle before cutover. Native auctions do not resume those operations.
The disposable checks do not yet establish safety for that migration.

Corpse AOI uses map, Instance and cell indexes. Bodies and bones keep the same AOI Grant until
visibility ends or the Corpse expires. Reclaim selects the Actor's own Corpse and checks distance,
map, Instance, ghost state and the remaining delay. Resurrection offers and self-resurrection fields
go only to the owner. The native spirit-healer handler checks the selected NPC and interaction range.
See the [Corpse migration](./danger-zones.md#corpse-aoi-migration) before publishing existing rows.

Weather delivery uses the Module's terrain-derived zone and an index of native connections in that
zone. Entry sends the current weather; later changes reach only that zone. Client zone claims do not
change this state. Breath notifications go only to the owner. Death, godmode and world removal clear
the active breath timer through the WorldEntity lifecycle.

The [chat handler](../module/src/native/chat.rs) resolves whisper targets, reads ignore and away
state, and selects Speech recipients. Speech queries indexed Character cells independently of AOI
Grants, so a yell can reach 300 yards. Zone-wide creature messages use the native zone index and
check map and Instance. Realm Chat Lines use the existing group, guild and channel Gates, with
ignore checks at delivery. The [channel handler](../module/src/native/channel.rs) resolves named
targets and sends notices through the Module. Native channel membership survives map changes and
ends on logout or connection loss. A departing owner hands ownership to a remaining member.
The [social handler](../module/src/native/social.rs) reads contacts and current native presence.
Friend notifications use the contact target index. `/who` lists native Characters in the world and
uses the shared protocol filters; its one-second limit survives Character logout.
Flood limits belong to the native connection and survive Character logout. Chat Refusals that occur
before gameplay changes return normally, retaining the flood count. GM command Refusals still use
transaction rollback. System messages emitted during world entry replay after the connection joins.

The [party handler](../module/src/native/party.rs) resolves names, applies the Module's group rules
and sends group events to native members. Invites check presence and faction in the same Shard.
Companions answer invites through their consent Gate. Roster updates include current presence;
world entry restores the roster and target icons. Ready checks, minimap pings and random rolls have
independent one-second limits on the native connection.
The [member-stat handler](../module/src/native/member_stats.rs) answers full requests only for group
members. A five-second scheduled update per grouped native connection sends deltas for members
outside AOI. Private snapshots retain the prior Aura and pet fields so removals clear the client.
Loading members keep their previous snapshot; full requests report that they are loading. Logout
and group destruction delete the connection's scheduled updates.

The [guild handler](../module/src/native/guild.rs) reads Character facts and applies membership and
rank permissions in the same transaction. Native rosters include loading Characters as online and
filter officer notes by the recipient's rank. Guild events select native recipients in the Module;
membership changes update guild ID and rank through AOI. Login sends the MOTD to the Character and
sign-on to other members. Map changes do not announce a guild sign-off.
[Charter and emblem purchases](../module/src/native/petition.rs) commit copper, items, guild state
and notifications in one transaction. Native purchases create no
Fee Hold or Fee Decision. Petition signatures use the actual Account and Character facts in the
Shard. Turn-in creates the Guild and removes its charter and signatures atomically. World entry
restores charter petition fields and removes inert charters when no legacy Fee Hold is pending.

The [trade handler](../module/src/native/trade.rs) applies requests to the Module's Trade Session.
Offer and status events select the two native recipients in the same transaction. Exchanges commit
items, copper and client notifications together. Death, logout, map changes and Instance changes
cancel trading. Cancellation reaches the departing Character before map loading clears delivery.

The [duel handler](../module/src/native/duel.rs) sends the flag and duel state to its participants.
The Module's scheduled Duel transitions emit native packets in the same transaction. Completion
clears both participants' arbiter and team fields, then removes the flag. A winner announcement
also reaches other native Characters in nearby AOI cells on the same map and Instance.

The [meeting-stone handler](../module/src/native/meeting_stone.rs) reads race and class from the
Character rows and applies stone admission and queue changes in one transaction. Native Seekers
use their World Session for presence. Logout removes solo Seekers. Stone Adds send party rosters
directly, and queue events, reminders and completion select native recipients in the Module.

The [mail handler](../module/src/native/mail.rs) checks the mailbox, resolves the recipient and
applies the send, take, return or Letter Copy in one transaction. Native mail uses no Escrow.
Delivery timers send arrival notices to the recipient's World Session. Mail bodies require the
recipient or possession of the matching Plain Letter. The shared codec bounds inbox replies by
both the 50-letter limit and the WoW frame size. The item-text query's 32-bit item hint matches
against owned items and the requested text ID. This avoids comparing the hint with a full 64-bit
item GUID. The generated request and [CMaNGOS's reader](https://github.com/cmangos/mangos-classic/blob/master/src/game/Mails/MailHandler.cpp)
leave the following word unnamed; it grants no access to text.

The [pet handler](../module/src/native/pet.rs) applies Hunter pet rename Gates in the Module.
Pet CREATE sends the owner's summon link and action bar. Removing that AOI Grant clears both.
Hunter pet state changes send name-cache fields through AOI and care and progression fields only
to the owner while its AOI Grant holds. Grant replay restores the same recipient-specific fields.

The [auction handler](../module/src/native/auction.rs) checks the actual auctioneer and Character
before browsing or changing a listing. Listing, bidding and cancellation commit purse changes,
items, Auction Mail and recipient notices in one transaction. Native requests create no Hold or
Decision rows. A listing receipt supports a repeated sell request and retains the snapshot for
expiry. Buyouts require the full offer to be affordable but debit only the accepted price.
Owner and bidder pages use their Character indexes. Browse filters and usable-item checks run in
the Module. Native notices go only to their named recipient, including scheduled expiry notices.

The [teleport state](../module/src/native/teleport.rs) belongs to the Module. Near teleports require
the current counter and mover GUID before movement and gameplay requests resume. Map changes clear
delivery permissions before removing the live Character. The world-port acknowledgement rebuilds
the Character in the same Shard and sends self CREATE before AOI replays. Connection loss during
loading marks the Character offline and retains its destination for the next login. The decoder
uses the unpacked acknowledgement GUID in build 5875, as read by
[CMaNGOS](https://github.com/cmangos/mangos-classic/blob/master/src/game/MotionGenerators/MovementHandler.cpp#L240).

AreaTrigger requests check the Actor's map and position against the imported sphere or rotated box
before exploration credit or teleport. Dungeon entry resolves party ownership and Character
bindings in the same Shard. Separate Instances retain separate AOI Grants even on the same map.

The [creature movement handler](../module/src/native/creature.rs) sends spline changes through AOI.
Curved movement stores its complete path before delivery. A late viewer receives CREATE followed
by the remaining path and duration, with the original spline ID. Completed legs and stops do not
replay; stationary facing does. Replay requires the creature's current map and Instance to match
the spline. Unrepresentable curves send a stop instead of malformed path bytes.
Run-speed changes send the forced-speed packet to the owner and the spline-speed form to
observers. Stand-state changes acknowledge the owner, including forced standing after damage.

GameObject CREATE is followed by all four rotation fields. Later changes send VALUES through the
object's AOI Grants, without replacing the client object or its Grant. Encounter equipment changes
send virtual item displays through creature AOI; late entry restores those displays after CREATE.
DynamicObjects use a separate AOI layer and a map, Instance and cell index. Ground-area expiry
removes their Grants and sends DESTROY. Existing rows require the
[DynamicObject cell backfill](./danger-zones.md#dynamicobject-aoi-migration) before native cutover.

Instance Removal inserts and cancellations send the countdown only to the affected Character.
World entry replays the remaining time, or clears a stale countdown when none remains. Scheduled
removal clears the countdown before recalling the Character home.

The native Addon bridge parses STC envelopes before ordinary chat. Its connection limit allows a
burst of twenty commands, then two per second, and survives Character logout. Replies use the
recipient Identity's live Character and its native connection. Malformed STC envelopes cannot enter
ordinary party chat. Commands received at Character selection or during teleport do not run.

Native Companion commands use the local Group roster, leader, Sessionless Action Consent and current
map and Instance. The Module checks the optional authority member and exact combat target before
calling the Package command handler. Application, the finished result and the private reply commit
in one transaction. They need no dispatch lane, claim lease or cross-Shard Command Receipt. The
Character's issuer sequence and the Package command contract remain in use. A Package result of
`WaitingForCapacity` rolls back the request.

Companion-initiated invitations and departures run as local Group actions. Consent is checked in
the same transaction, and a sessionless invitee accepts only when its own consent permits it. The
existing Package operation names remain, but they create no pending Group Intent.

Sessionless AreaTrigger crossings admit the existing party Instance, move the live entity and
persist the Character in one transaction. They retain vitals and generate AOI departures and arrivals
for native observers. A Character Relocation row records the latest completed crossing for a repeat
call. Controller selection clears it. Party facts prefer live entities over old Realm-core mirrors.
These crossings complete locally. Published coordination tables and their debug fixtures remain
for schema compatibility. The native runtime does not coordinate cross-Shard Transfers.

The local CLI starts this native runtime by default. It builds one host, publishes the single
`lyracore` Shard, and provisions Accounts directly through the Module. Native data lives in
`.lyracore/native-data`. Startup does not consolidate old Shards or change an existing deployment.
See [local setup](./quickstart.md#3-start-the-realm) for process ownership and credential files.

The host extension pins SpacetimeDB 2.13.0 at `08e1aba133671f9c631c6e70133a82faa0e7b283`.
The Module still targets 2.7.1 and passes this native-host compatibility check. Prepare the host with
`python3 native-host/prepare-host.py --checkout <path> --build`. Build the Module with debug reducers,
then run `python3 native-host/check.py --host <path>/target/release/spacetimedb-standalone`.
The listeners read `LYRACORE_NATIVE_LOGON_ADDR`, `LYRACORE_NATIVE_WORLD_ADDR` and
`LYRACORE_NATIVE_DATABASE`. Each listener is disabled when its address is absent. These local checks
do not change a deployed Realm.

Admission policy also runs in the Module. The Operator calls `native_set_admission_limits` with
maximum World Sessions, maximum concurrent entries and admissions per second. Each zero disables
that limit. Limits default to zero and persist across restarts. Lowering a limit keeps admitted
World Sessions connected. The Login Queue sends a position on arrival, updates waiting positions
every second, and admits tickets in order. A finished world entry releases its entry slot; an
unfinished entry releases that slot after 20 seconds. The World Session retains its capacity slot
until disconnect. A new authenticated connection replaces an older connection for the same Account
and joins behind existing waiters.
Session revocation Gates run before admission and every authenticated request, including ignored
controls. The one-hour Session expiry limits new world authentication. An established World Session
continues past that deadline, and the reaper retains its Session until the connection closes.
Account bans, deleted Sessions and replacement login proofs still revoke established connections.

The Host treats completed reads and writes as socket activity. Queue position updates keep waiting
clients connected. Idle sockets and incomplete world frames time out after 90 seconds. The handshake
still has a 10-second deadline, and writes retain their 2-second deadline. Listener accept errors
retry with backoff capped at two seconds. The `--check-accept-recovery` check exhausts a disposable
Host's descriptors and requires both game listeners to recover after sockets close.

## Durable state and extension points

The Module owns Characters, Accounts, inventory, groups, guilds, Mail and auction state in one
Shard. Maps and Instances remain independent gameplay partitions. Spatial reads use the indexed
partition helpers, and AOI uses map, Instance and cell indexes. Whole-world scans remain subject to
the Architecture Tests in `module/src/tripwires.rs`.

Native connection, cipher and delivery tables are private. The Host connects in process as the
Shard publisher. WoW clients receive only the packets selected by the Module. HTTP access still
requires the existing Operator Gates. Public tables are not a substitute for those Gates.

Packages compile into the same Module and have its privileges. `module/build.rs` discovers their
source and registry markers. The supported operations and compatibility rules are in
[the Package API](./package-api.md). Existing Package gameplay code uses the same named operations;
external test programs that launch the removed Gateway must change to the native Host. Core no
longer exports `lyracore_test_support::gateway_binary`.

## Verification

Each check establishes a different property. Passing a codec test does not establish native TCP
behavior, and a protocol check does not establish real-client presentation or world capacity.

| Check | Command or source | Establishes |
| --- | --- | --- |
| Module and protocol tests | `cargo test -p lyracore-module --lib --features=debug_reducers`, `cargo test -p lyracore-protocol` | Gameplay calculations, packet layouts and Architecture Tests |
| Durable Module tests | `module/tests/` with `--ignored` | Transactions in disposable Shards |
| Build and schema | `./lyracore preflight` | The publish feature set and offline schema extraction |
| Native protocol | `python3 native-host/check.py --host <binary>` | TCP authentication, requests, committed state and emitted packets |
| Login Queue | Add `--check-admission` | Admission Gates and a waiting client kept connected past 90 seconds |
| Crash recovery | Add `--check-recovery` | Restart with durable Character and inventory state, connection cleanup and fresh AOI |
| Slow readers | Add `--check-slow-readers` | A blocked output connection closes while another client remains responsive |
| Movement load | Add `--load-clients 1000 --load-seconds 30 --load-group-size 16` | Throughput, latency and final observer positions for the stated fixture |
| Real client | WoW build 5875 | Presentation and client interaction |

The native checks create temporary keys, ports and data directories. The load check reads Linux
`/proc` for Host CPU and sampled RSS. It uses the full Module with seed fixtures, pre-established
world-session keys and synthetic Characters. It does not import a world or run Package AI.
Heartbeats can coalesce before the 50 ms movement publication tick. The final stop check verifies
that every expected observer receives the final position. Driver CPU and loop delay are reported
so a saturated driver cannot be mistaken for the Host's capacity limit.

On an Intel Core i7-7820HQ with four cores and eight threads, a 30-second run with 1,000 Characters
in groups of 16 achieved 9,615 movements per second. Clients received 4,313,418 movement deliveries
and all 14,936 final observer positions, with zero closed connections. Host CPU averaged 1.81 cores,
sampled RSS peaked at 711 MiB, and sampled movement latency was 150 ms at p95. Driver CPU averaged
0.79 cores. The [recorded metrics](../native-host/results/full-module-1000.json) include the Module
and Host binary hashes. These measurements cover synthetic movement traffic. Populated-world
capacity and a controlled comparison with the removed Gateway remain unmeasured.

The crash check kills its own disposable Host with a Character online. Restart clears stale
native connections, channel membership and AOI, preserves committed position and inventory, and
permits world entry with the saved Session key. The slow-reader check fills one client's output
while another sends requests. With bounded NPC text replies, the repaired check closed the blocked
client in 2.06 seconds; the healthy client's slowest request took 1.68 ms.
