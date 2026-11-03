# 0006. Relay player motion in the Native Host

Proposed, 2026-11-03.

A Shard runs its transactions one at a time on one writer thread. On 2026-11-03 the dense Argus
bench sent about 1,900 movement requests a second at 1,000 players, against about 35 scheduled
ticks. Batching those requests in the Host cut transactions 16 times but writer time only
6%: the cost is the Module's work for each heartbeat, not the transaction around it. Player motion
is the largest stream of requests and the one with the least gameplay in it, so it leaves the
writer. This follows the sidecar pattern SpacetimeDB describes for work that need not be a
transaction: keep the fast path out of the database and cross into it on purpose.

## Decisions

1. The Native Host relays a mover's movement messages to the mover's AOI Grant holders itself,
   from the Router it already keeps. The Module still owns AOI Grants, visibility, every gameplay
   rule and all durable state. The Host decides no Gate; it only skips work the Module permits it
   to skip.
2. A Motion Permit, one Module row per World Session, allows the relay. The Module holds it while
   the Character moves on foot and freely: alive or a ghost, not on a taxi, a Transport or a
   loading screen, and not held in place. It withdraws the permit in the transaction that ends any
   of those, and when the World Session leaves the world. Without a permit, movement takes the
   Module path unchanged. A permitted mover's Module relays go out in their own transaction, not
   at the motion tick. A Host relay can overtake one only while the Host has not yet applied that
   transaction, which affects at most the first movement after a permit is granted.
3. The Host checks what the Module's movement Gate checks before it relays: a client movement
   opcode, a complete MovementInfo with finite position, no Transport flag, and a client time newer
   than the last accepted one. A message that fails goes to the Module, which decides.
4. The Module still receives every movement except heartbeats, and one heartbeat sample a second,
   or sooner after 4 yards of drift, through `native_world_motion`. Facing and pitch reach it
   because melee and spell Gates read the Character's orientation. It applies them as it applies
   any movement and sends no relay of its own. Exploration, rest, breath, fatigue, AOI cell
   changes, aura and channel breaks, fall damage and movement scoring run on those messages.
5. The Module's position for a moving Character can trail the client by up to one sample. It
   already trailed by up to 4 yards before this change, because heartbeats within that drift did
   not write the entity. The sample drift matches it, so Range Gates such as the 9-yard melee
   leeway keep their existing tolerance. A Host crash loses at most the
   motion since the last sample, which the client corrects on its next movement.
6. Host relays use the delivery path of Module packets, including Coalescible Motion thinning for
   slow readers. The Host wakes writers for queued relays every 20 ms, so one write carries many
   movers instead of one socket write per heartbeat per recipient. A connection keeps at most one
   queued Coalescible Motion relay after its last discrete one, so a client that floods heartbeats
   adds no routing work. A transaction that changes a permit first sends the queued relays, so a
   relay accepted before a withdrawal reaches observers before that transaction's packets.
7. Movement opcode lists the Host and Module share live in `native-host/src/motion_opcodes.rs`.
   A Module test checks them against `lyracore-shared`.
8. A Module must be published before a Host that subscribes to `game_native_motion_permit`. An
   older Host ignores permits and leaves every movement on the Module path.
9. An observer more than 40 yards from a mover gets every second relayed heartbeat of that mover,
   and one more than 80 yards away every fourth. The Host measures from where each of them last
   relayed movement. An observer without a permit, or that has not moved, gets every heartbeat.
   Every other movement, and a heartbeat that replaced a queued facing or pitch change, reaches
   every observer. The 1.12 client moves peers on from their movement flags between heartbeats,
   so a distant heartbeat mostly corrects drift. A crowd sends N² heartbeats, and most of its
   pairs are far apart.

## Rejected

- **Batching movement requests without the Host relay.** Measured on Argus: 16 times fewer
  transactions, 6% less writer time, and 10 to 25% later movement for observers.
- **A separate database for movement or combat.** Combat, spells and aggro read positions in the
  same transaction as the rest of the game. Splitting them by function trades the writer limit for
  stale reads without cross-database rollback. Region Shards remain the way to add writers.
- **AOI derivation in the Host now.** AOI Grants grow with the square of a crowd and are durable
  rows. Moving their calculation out of the writer is the next lever, decided after this one is
  measured in production.

## Evidence

The A/B runs used a disposable Host on Argus seeded from the 2026-11-03 nightly backup, with the
Realm, World and Instance Shards of the live Realm. Main is `90ae629d`; the change also rebuilds a
moving entity's Grants only for viewers in cells that enter or leave its reach. Observer movement
lag is client-measured, p95; writer is the World Shard's.

| Run | Players | Main lag | Main writer | Relay lag | Relay writer |
| --- | --- | --- | --- | --- | --- |
| Dense, 15 yd, 500 ms heartbeat | 600 | 100 ms | 43% | 36 ms | 33% |
| | 800 | 156 ms | 62% | 70 ms | 47% |
| | 1,000 | 203 ms, 923 connected | 74% | 142 ms, 1,000 connected | 62% |
| Spread, 600 yd, 200 ms, combat | 600 | 216 ms | 97% | 24 ms | 87% |
| | 800 | 391 ms, 694 connected | 96% | 25 ms, 800 connected | 99% |

These runs predate two changes: facing and pitch now reach the Module, and samples follow 4 yards
of drift instead of 8. With those changes and Grants kept one cell beyond the AOI, dense 1,000
measured 72 ms at 43% writer, and spread 800 measured 25 ms at 86%.

In the spread runs creature AI keeps the writer busy, and movement lag no longer depends on it.
Raw reports are in `~/LyraCore-native/.lyracore/sidecar-20261103/` on Argus.
