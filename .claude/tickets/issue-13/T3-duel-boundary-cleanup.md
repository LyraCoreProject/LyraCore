# T3 — Enforce the duel flag boundary and lifecycle cleanup

Parent: issue #13. **Parallel with T2 after T1.**
Model: mid tier. Estimated size: ~165k tokens.

## Problem

An active Duel can currently persist forever and across invalid world state. It needs the vanilla
out-of-bounds warning/grace flow and teardown on participant removal without putting authority in a
gateway session.

## Delivery

Extend the module-owned Duel lifecycle pass with the 50-yard flag boundary, one ten-second deadline
per participant, transition events for leaving and returning, and a fled completion after expiry.
Route the resulting typed in/out-of-bounds messages to the affected participant. Connect logout,
character removal, transfer/map-instance departure, and invalid participant reaping to centralized
Duel teardown. Keep all checks shard-local and bounded by the live Duel set.

## Acceptance criteria

- An active in-bounds participant has no flee deadline and receives no repeated message.
- Crossing beyond 50 yards starts one ten-second deadline and emits one
  `SMSG_DUEL_OUTOFBOUNDS` to that participant.
- Returning within the boundary clears the deadline and emits one `SMSG_DUEL_INBOUNDS`.
- Staying outside through the exact deadline completes the Duel as fled with the other participant
  as winner and uses centralized teardown.
- Pending/countdown Duels do not accidentally grant combat authorization while boundary state changes.
- Missing/dead participants, logout/removal, and map/instance transfer cannot leave a live Duel or
  stale duel fields behind. The outcome matches the spec's interrupted/fled distinction.
- Cleanup is idempotent under repeated lifecycle ticks and duplicated session teardown.
- The lifecycle pass scans live Duels and directly fetches their two entities; it does not scan all
  world entities.
- Gateway tests assert recipient routing and exact in/out-of-bounds opcode transitions.
- Module tests cover exact distance/deadline boundaries, leave-return-leave, timeout, missing entity,
  logout, and transfer cleanup.
- Existing world/transfer/logout tests pass. The work is committed on the ticket branch.
