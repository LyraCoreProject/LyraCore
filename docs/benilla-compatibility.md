# Benilla protocol compatibility

`lyracore-protocol` pins the independent Benilla codec to
`c2fc3cf304075a0fa1c9accffaf5efb99be9edfe`. Its tests compare movement and other packet layouts with
that codec. The native TCP check runs separately against the real Module, using a Python client
that implements logon, header encryption and the checked packet layouts.

```sh
cargo test --locked -p lyracore-protocol
python3 native-host/check.py --host <path>/target/release/spacetimedb-standalone
```

The native check covers the gameplay exchanges listed in
[the architecture](./architecture.md#native-protocol-coverage), with committed-state and packet
assertions. These tests do not establish every gameplay path or real-client presentation.

## Generate the opcode inventory

```sh
mkdir -p "$HOME/.cache/lyracore-scratch/conformance"
cargo run --locked --quiet -p lyracore-protocol --example benilla_inventory \
  > "$HOME/.cache/lyracore-scratch/conformance/benilla-inventory.json"
```

The inventory reads the pinned Benilla writer, movement selectors and decoder. It matches their
numeric opcodes against Module dispatch. Shared protocol source contributes references,
but cannot supply dispatch evidence through an unused codec match. Every request emitted by
that Benilla revision has a typed dispatch pattern, a reviewed raw route or an explicit ignore.
The inventory test fails when none applies.

Each message records its direction, source references and handling classification:

- `dispatch_pattern_present`: a typed dispatch pattern exists in the Module.
- `explicitly_ignored`: the request is deliberately dropped.
- `raw_route_review_required`: a known numeric route needs manual inspection.
- `no_dispatch_pattern_found`: no typed pattern or reviewed route was found.
- `source_evidence_only`: an outgoing opcode appears in source.

Source presence does not certify behavior. The scanner excludes imports, comments, strings and
test code, handles `match`, destructuring and `matches!`, and does not expand other macros or trace
calls. A decoder in Benilla does not require every Realm to emit that message.

The `behaviors` list comes from the [Feature Ledger](../conformance/README.md), with one TOML file
per Behavior under `conformance/features/`. The loader checks the schema and Behavior references.
The inventory checks named Rust test declarations and records whether they are ignored. It does
not run them. Native Python scenarios remain separate until recorded conformance scenarios replace
them. An empty test list means no retained Rust evidence in this inventory. Real-client Verification
comes only from the ledger's human-maintained `real_client` field.

## Transport movement

The build-5875 movement codec uses transport flag `0x02000000`, a full 64-bit
transport guid, a relative position and orientation, with no transport timestamp.
The Module decodes relayed `MSG_MOVE_*` bodies before the general typed reader
and writes that same vanilla layout into the movement row. Observer replies
preserve the stored body. The shared fall-damage parser recognizes the same flag
and excludes transport movement from fall damage.

`wow_world_messages` 0.3.0 uses a later transport layout. Its typed movement values
remain the internal carrier, while the protocol crate owns the build-5875 movement bytes. Benilla
checks transport, jump and swim tails together. The transport regression test is
enabled in the normal suite. Benilla also reads a Passenger's map crossing: `SMSG_TRANSFER_PENDING` names
the Transport and the old map, and `SMSG_NEW_WORLD` carries the offset on it.

The real Module scenario also checks late-observer transport attachment. Speed,
root, hover, feather-fall, water-walk, knockback, teleport and spline completion
acknowledgements keep the World Session usable. The Module reads the speed,
root, knockback, hover, feather-fall and water-walk acknowledgements with the
build-5875 layout too, so a transport tail does not end the session. This does
not establish boat simulation, transport membership or acknowledgement counter
enforcement.

Malformed boolean fields, sized strings and counted arrays fail before generated decoding.
Compressed authentication and ticket requests use bounded raw handlers. Tickets reach the same
handler in the Login Queue, at Character selection and in the world. The Module ignores decoded
in-world requests without a handler, including Help UI bug reports.

Character CREATE and VALUES expose XP, coinage, inventory pointers, skills and Character-sheet
stats only to the owner. Observers receive public fields such as health, level and visible equipment.
Whispers remain deliverable while the recipient waits for a world-port acknowledgement.
Fall damage sends the owner an environmental damage log packet with the fall damage.

## Movement controls

Pitch changes (`MSG_MOVE_START_PITCH_UP`, `_DOWN`, `MSG_MOVE_STOP_PITCH` and
`MSG_MOVE_SET_PITCH`) relay to observers like other movement.

A stun, root or polymorph aura, and a dead body before release, root the
Character's own client with `SMSG_FORCE_MOVE_ROOT`. The last of them to end sends
`SMSG_FORCE_MOVE_UNROOT`. A ghost walks on water. World entry restores both
states. When the client acknowledges a root, unroot or water walk, observers
receive `MSG_MOVE_ROOT`, `MSG_MOVE_UNROOT` or `MSG_MOVE_WATER_WALK` with the
acknowledged pose. A dead body keeps its Module position when it acknowledges a control.
Observers see a creature's stun, root or polymorph as
`SMSG_SPLINE_MOVE_ROOT` and its end as `SMSG_SPLINE_MOVE_UNROOT`.

An acknowledgement that arrives after a same-map teleport and before the
client's `MSG_MOVE_TELEPORT_ACK` is not relayed. Its pose may be the old
position: a release acknowledges the unroot at the corpse.

Root, stun and polymorph follow `lyracore_shared::aura::holds_in_place`, the
rule the Module's movement gate also uses. The importer gives confuse, pacify
and charm the polymorph mechanic, so they root the client too. Telling them
apart needs a distinct mechanic in the imported spell data.

A slow also lowers backward running, through
`SMSG_FORCE_RUN_BACK_SPEED_CHANGE`, at world entry and in a peer's CREATE.
Observers receive another unit's run, backward run and swim speeds as
`SMSG_SPLINE_SET_*_SPEED`, players included. cmangos relays a player's
`MSG_MOVE_SET_*_SPEED` after the owner's acknowledgement instead. The motion
row holds only a MovementInfo body, so it cannot carry that relay's speed.

Cheat, Game Master and minigame movement requests and `CMSG_MOVE_FALL_RESET`
are ignored without decoding. They never move the Character.

### Root receipts

The client moves independently of the Module. The Module sends a root packet for a root, stun or
polymorph and sends an unroot packet only after the last hold ends. The library writes
`SMSG_FORCE_MOVE_ROOT` and `SMSG_SPLINE_MOVE_ROOT` with a full guid, which the client misreads, so
the Module writes the packed guid itself. The Module drops steering samples while held, but accepts
root receipts, stops, turns and landings so fall damage still applies.

`crates/lyracore-protocol/src/codec/movement.rs::acknowledged_move` turns the current Character's
root, unroot and water-walk receipts into `MSG_MOVE_ROOT`, `MSG_MOVE_UNROOT` and `MSG_MOVE_WATER_WALK` relays for
peers, as cmangos does. Peers otherwise retain the last run. A root receipt clears steering flags
so the Module accepts the reported position. The pinned library's `clear_*` methods mask with
`reverse_bits`, so the Module clears the bits in the encoded bytes instead. The Module reads the
receipts with the build-5875 layout, so receipts on a transport relay too. A receipt between a
same-map teleport and the client's `MSG_MOVE_TELEPORT_ACK` is not relayed.

Movement interruption uses client flags, including jumping, rather than a change in stored
position. A delayed stop can update the stored position after Shadowmeld begins without ending
the aura. Turning in place also preserves it.

### Resurrection packets

The shared [corpse codec](../crates/lyracore-protocol/src/codec/corpse.rs) writes reclaim delay in
milliseconds. The pinned library writes seconds. It also writes both resurrection-offer flags after
the sized caster name; the library omits the second flag. The Module uses these raw codecs. The
[CMaNGOS packet writers](https://github.com/cmangos/mangos-classic/blob/master/src/game/Entities/Player.cpp#L17442)
define the offer layout and reclaim-delay units.

## Build-5875 corrections

The protocol crate supplies raw codecs where the pinned generated library differs. These include
transport movement, teleport acknowledgements, packed movement-control GUIDs, reclaim-delay units,
resurrection-offer flags and XP layouts. Partial VALUES updates omit `OBJECT_FIELD_TYPE`.
See [the protocol traps](./danger-zones.md#1-do-not-touch-traps)
before changing those packets.

Native Character creation, inventory changes, quests, Mail and auctions return gameplay Refusals
without committing partial state. The Module constructs those replies. The Host delivers them
after the failed transaction has rolled back.

## Missing gameplay and deliberate responses

`module/src/native/unavailable.rs` defines responses for services that remain absent.
A Refusal or an empty reply does not establish a working gameplay feature.

| Service | Response |
| --- | --- |
| Battlegrounds | Empty instance lists, queue slots, scoreboards and positions; join requests report unavailability |
| GM tickets | Disabled service, absent ticket, refused creation and updates |
| Pet stables | Failure code 6 |
| Pet bar editing, autocast and pet spell requests | Specific System Messages, with a cast-failed packet for a pet spell |
| Item containers, wrapping and explicit vendor destinations | Inventory Refusal |
| Readable pages | A terminal page naming the unavailable text service |
| Tutorial resets and action-bar display preferences | One unavailable notice per feature per World Session |
| Tutorial mark-seen requests | No reply; login already reports every tutorial seen |
| Ammunition, skill unlearning, equipment visibility, PvP flags and honor queries | Specific System Messages |
| Summon acceptance, remote cameras, LFG preferences and quest-share confirmations | Specific System Messages |
| Instance resets | Specific System Message |
| Raid lockout queries | An empty lockout list; instance bindings end with their Instance |
| Taxi chains | Activation failure; single-hop taxis have their own implementation |

The Module handles current presence, name lookup, party authority and recipient Gates in one
Shard.
