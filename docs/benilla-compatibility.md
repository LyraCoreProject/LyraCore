# Benilla protocol compatibility

Benilla is an independent client codec for testing LyraCore's 1.12.1 protocol.
The Gateway's development dependencies pin `benilla-protocol` and `benilla-srp`
to `c2fc3cf304075a0fa1c9accffaf5efb99be9edfe`. Tests use that revision from Cargo,
so changes in a neighboring Benilla checkout cannot change the results.

All 277 requests in the pinned Benilla writer now have a typed dispatch pattern
or a reviewed raw route. The inventory test flags requests with neither source
classification. This accounts for requests; it does not certify every gameplay path or every outgoing
packet. Unsupported operations return a Refusal where the protocol defines one,
or a System Message naming the unavailable feature.

The first Headless Client scenario covers SRP logon, realm discovery, encrypted
world authentication, Character selection, world entry, the Realm Clock, ping,
logout and another world entry. It repeats on a new connection with the saved
logon key. It uses the real Gateway dispatch with a Fake at the LogonStore and
WorldStore Seams. It needs no running realm, client assets or graphics runtime.

Run the scenario and movement checks with:

```sh
cargo test --locked -p lyracore-gateway --bin lyracore-gateway benilla_
```

## What the tests establish

The Headless Client uses Benilla for SRP proofs, header encryption, request bodies
and reply decoding. Local framing keeps the existing socket deadlines in force.
World entry checks the Character guid, name, map, position, health and level.
Clock replies must contain current Unix seconds, and pong must echo the sequence
number. Logout must complete before the next Character selection.

Every received world packet must decode without unread bytes. Two world-entry
messages have no Benilla decoder at this pin. The test checks their lengths
explicitly: `SMSG_ACCOUNT_DATA_TIMES` at `0x0209` is 128 bytes, and
`SMSG_SET_REST_START` at `0x021E` is four bytes. Their contents are not verified.
Any other unmodelled packet fails the test.

Two Headless Clients also move through real dispatch and a shared Fake WorldStore.
Each receives the other's ground, jump, swim and transport movement through the
production motion callback, AOI audience, known-object Gate and encrypted writer.
The Fake supplies the initial peer snapshot and applies movement to its stored
positions. The scenario checks those positions, suppresses echoes to the mover,
and excludes an observer outside the AOI. It does not run the Module's movement
reducer or verify initial peer discovery.

Separate codec checks cover movement tails and malformed bodies. Every truncated
prefix and a body with trailing bytes must fail decoding.

## Generate the opcode inventory

```sh
cargo fetch --locked
cargo run --locked --quiet -p lyracore-gateway --example benilla_inventory \
  > /tmp/benilla-inventory.json
```

The inventory groups world messages by numeric opcode and direction. It reads
the pinned Benilla writer, movement selectors and decoder, then matches those
numbers against LyraCore's codec names and Gateway source. For example,
Benilla's `CMSG_MEETINGSTONE_STATUS_QUERY` and LyraCore's
`CMSG_MEETINGSTONE_INFO` both mean `0x0296`.

Each row contains source locations and one of these observations:

- `dispatch_pattern_present`: Gateway source contains a typed dispatch pattern.
- `explicitly_ignored`: a matching dispatch arm has an empty body.
- `raw_route_review_required`: the opcode has a known raw routing path.
- `no_dispatch_pattern_found`: the scanner found no typed dispatch pattern.
- `source_evidence_only`: an outgoing message appears in source. Its decoder
  presence and Gateway references are separate fields.

These are source observations, not claims that a feature works. The scanner
excludes imports, comments, strings, test code and generated bindings. It handles
`match`, destructuring patterns and `matches!`; it does not expand other macros,
resolve aliases or trace calls. Raw numeric sends need manual inspection.
Outgoing rows include Benilla decoder references and LyraCore's named `SMSG_`
references. A Benilla decoder does not make a message mandatory for every realm.
Logon packets use a separate protocol and are covered by the scenario instead.

The inventory scanner tests run in `cargo test -p lyracore-gateway`.

## Transport movement

The build-5875 movement codec uses transport flag `0x02000000`, a full 64-bit
transport guid, a relative position and orientation, with no transport timestamp.
The Gateway decodes relayed `MSG_MOVE_*` bodies before the general typed reader
and writes that same vanilla layout into the movement row. Observer replies
preserve the stored body. The shared fall-damage parser recognizes the same flag
and excludes transport movement from fall damage.

`wow_world_messages` 0.3.0 uses a later transport layout. Its typed movement values
remain the internal carrier, while the Gateway owns the transport bytes. Benilla
checks transport, jump and swim tails together. The transport regression test is
enabled in the normal suite.

The real Module scenario also checks late-observer transport attachment. Speed,
root, hover, feather-fall, water-walk, knockback, teleport and spline completion
acknowledgements keep the World Session usable. This does not establish boat
simulation, transport membership or acknowledgement counter enforcement.

## Gameplay scenarios and remaining gaps

The Headless Client scenarios cover these exchanges through the same WorldStore Seam:

- Select a target, start melee and stop melee. Check the selected target and
  engagement in the Fake, plus the attack-start and attack-stop replies.
- Open a corpse, take copper and an item stack, close loot and move the item to
  another inventory slot. Check the Fake's balances and inventory, then log in
  again and decode the acquired item's entry, owner and stack count before the
  Character's inventory pointers arrive.
- Read and accept a quest without objectives, request its reward screen, then
  collect the reward. Check the quest log and copper balance, plus the completion
  packet's quest id, money and XP. An incomplete quest must stay on the progress
  screen and grant nothing when its reward screen is requested.

The reward scenario exposed `CMSG_QUESTGIVER_REQUEST_REWARD`, which now uses the
same durable completion evaluation as `CMSG_QUESTGIVER_COMPLETE_QUEST`. Choosing
a reward still crosses the Store Seam and leaves the grant rules in the Module.

## Durable scenarios

The ignored `benilla_durable_` tests start private SpacetimeDB instances on loopback
ports. Each publishes the Module with `debug_reducers`, creates a Coordinator and
runs the real Gateway dispatch. Fixture changes apply only to these disposable
instances. No existing realm is needed or changed.

```sh
cargo test --locked -p lyracore-gateway --bin lyracore-gateway benilla_durable_ \
  -- --ignored --test-threads=1
```

They require SpacetimeDB 2.7.1 and the Wasm toolchain. The Module durable workflow
runs them in CI. They cover:

- Two Characters discovering one another, ground and transport movement, late
  observer attachment, leaving the AOI and returning.
- Accepting a two-kill quest, melee damage, kill credit, corpse money and item
  loot, reward collection and persistence after reconnect.
- Splitting stacks, refusing invalid counts and equipment destinations, then
  reconnecting with both stacks intact.
- Partial and full item destruction, indestructible items and persistence.
- Destroying an equipped weapon and updating the Character sheet to unarmed damage.
- Splitting into an equipped bag, capacity checks, reconnecting with its contents,
  moving, equipping, using and destroying contained items. Nonempty bag moves and
  destruction are refused, as are reverse swaps that would equip food.
- Starting a spell channel and cancelling it with `CMSG_CANCEL_CHANNELLING`.
- Refusing a distant, dead or wrong-kind innkeeper, then binding at the selected
  innkeeper and receiving the new hearth point during the same World Session.

Module inventory regressions also check that split, destroy, use, move, merge,
vendor sale, disenchanting and reagent consumption cannot change offered trade
items. Quest item exchanges use the same inventory Gate. Scheduled crafting checks
all reagents before consuming any or charging power. Automatic grants that grow
an offered stack clear both acceptances and refresh both trade windows. Failed
crafts also refresh the restored quantities. A completed trade delivers the newly
accepted stack count and copper. Equipment moves refuse a two-handed weapon
while an offhand item is equipped, and refuse the reverse combination. The
Character must remove the conflicting item first.

The XP scenario found a codec mismatch. Vanilla kill XP includes base XP and the
group multiplier; other XP awards have no extra fields. The Gateway now writes
that layout directly. `XpEvent.rested_bonus` is an appended field with a zero
default so the kill packet can distinguish base XP from rested XP.

The bag scenario found missing container pointers at world entry. Those pointers
now follow the item and Character creation packets, using the same slot mapping
as incremental inventory updates.

## Additional protocol corrections

The Gateway owns these build-5875 layouts where the protocol library differs:

| Request | Benilla layout |
| --- | --- |
| `CMSG_BATTLEFIELD_JOIN` | Map, instance and group flag, nine bytes |
| `CMSG_PAGE_TEXT_QUERY` | Page id and object guid, twelve bytes |
| `CMSG_SET_FACTION_ATWAR` | Reputation-list index and boolean, five bytes |
| `CMSG_SET_FACTION_INACTIVE` | Reputation-list index and boolean, five bytes |
| `CMSG_SET_WATCHED_FACTION` | Signed reputation-list index, four bytes |
| `CMSG_ACTIVATETAXIEXPRESS` | Flightmaster, total cost, node count and node ids |
| `CMSG_OPENING_CINEMATIC` | Empty body, absent from the library's client enum |
| `CMSG_SET_LOOKING_FOR_GROUP` | Three slots and a terminated comment, absent from the library's client enum |
| `CMSG_GMTICKET_CREATE` | Category, map, position and two terminated strings; Benilla omits the optional harassment transcript |
| `CMSG_GMTICKET_UPDATETEXT` | Category and terminated text, including the Help window's 500-character limit |

Ticket requests receive Refusals after framing checks. The unavailable service
does not decompress the optional transcript or allocate from its size headers.

At War changes reach the existing Module operation. Bind and talent-reset
confirmations also reach existing Module operations. Selecting the gossip option
opens the confirmation dialog first. Talent quotes use the same price calculation
as the Module. Changed bind points reach the client from the Character row update.
The selected innkeeper guid reaches the Module's interaction Gate.
`CMSG_PLAYER_LOGOUT` uses the existing logout Gate, and
`CMSG_LOGOUT_CANCEL` returns its acknowledgement. Logout remains immediate when
allowed; there is no twenty-second pending logout to cancel.

## Missing gameplay and deliberate responses

These features remain unavailable. The responses below apply to valid requests
while in world. A decoded request or a refusal is not a working gameplay system.
Automatic stand-state, action-bar and tutorial requests announce each missing
feature once per World Session. Repeated requests do not add chat lines.

| Feature | Protocol behavior | Missing work |
| --- | --- | --- |
| Battlegrounds | Empty instance lists, three clear queue slots, empty scoreboards and positions. Join and port requests report unavailability. | Queues, matchmaking, instances, matches, rewards and battlemaster map selection. |
| Battleground spirit healers | System Message reporting unavailability. | Resurrection queues and timers. |
| GM tickets | System disabled, no current ticket, creation and update refused. Deletion reports the already-absent ticket. | Durable tickets and GM workflow. |
| Pet stables | Stable failure code 6 for listing, storing, retrieving, swapping and slot purchase. | Stable storage and purchases. |
| Pet controls | Specific System Messages for bar changes, autocast, aura cancellation, stop-attack, naming, abandonment and unlearning. | These operations and their durable state. Existing pet action commands and progression remain separate. |
| Item containers and gifts | Inventory Refusal 39. | Item-container loot, wrapping and unwrapping. |
| Readable pages | A terminal page names the unavailable text service. | Page-text catalog import and page chains. |
| Explicit vendor destination | Inventory Refusal 39. | An atomic purchase into the requested bag and slot. Ordinary vendor purchase uses the existing path. |
| Automatic bag placement | Equipment-to-backpack uses the existing path; other requests receive Inventory Refusal 39. | Automatic placement into a chosen bag and movement of nonempty equipped bags. Explicit supported slot moves work. |
| Character presentation | Specific System Messages. | Stand state, mount flourish, helm and cloak visibility preferences. |
| Tutorials and display preferences | Specific System Messages. | Durable tutorial flags, action bar visibility, inactive and watched factions. Action buttons and At War use existing operations. |
| Ammunition and skill unlearning | Specific System Messages. | Ammunition selection and removal of learned skills. |
| Taxi chains | Activation failure code 1. | Chained flight state and charging. Single-hop taxis use the existing path. |
| Quest-share confirmations and results | System Messages. | Recipient acceptance and result relay. The existing share-offer path alone does not complete the exchange. |
| Instance resets and raid lockout queries | System Messages. | Client routing and projection across Instance Pools. Existing instance bindings are not reported as an empty list. |
| Honor and PvP flag toggles | System Messages. | Honor accounting, inspection and PvP flag timers. |
| Summon acceptance, remote cameras and opening cinematics | System Messages. | Summon offers, camera control and cinematic start state. |
| LFG preferences | System Message. | Durable LFG slots and comments. Meeting Stone requests use their existing path. |

Movement-control acknowledgements, skipped time, inactive-mover movement,
cinematic camera advances and cinematic completion are receipt-only. They do
not move the Character or establish acknowledgement counter enforcement. Setting
the active mover to the current Character is accepted; another mover receives a
System Message. Possession and remote movement authority remain unavailable.

The scenarios establish the behavior listed above, not every branch of the older
handlers. Further gameplay verification includes targeted item spells, channel
refresh and reconnect, pet spell execution, multi-shard instance flows and the
complete outgoing opcode inventory. The source report contains 346 outgoing
opcode rows; decoder presence alone does not require a realm to emit each one.

## Verification record

The workspace run passed 4,398 tests with `lyracore-package-delta` excluded,
using the Module's `debug_reducers` feature. All eight durable Benilla scenarios
passed against disposable SpacetimeDB instances. Formatting, workspace Clippy with
all targets and features, and the check of all 1,182 Gateway binding files passed.
Plain publish from `main` at `f09dafbd` to this Module migrated a private instance
without clearing it. Its Character rows survived, and `XpEvent.rested_bonus` was
readable after the update.

An earlier full workspace run found two unchanged package-delta fixture failures:
`a_package_may_insert_a_quest_inside_the_package_range` and
`an_incomplete_quest_insert_is_refused`. Their shared `WHOLE_QUEST_ROW` lacks the
`quest_type` field required by the existing package-delta schema. Those files are
outside this change.
