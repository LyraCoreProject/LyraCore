# T1 — Establish the Duel tracer

Parent: issue #13. **Runs alone.**
Model: strongest tier. Estimated size: ~185k tokens.

## Problem

There is no authoritative Duel state and no path from spell 7266 through accept/cancel to the
vanilla client messages. Later combat and boundary slices need one coherent lifecycle and relay
pattern to extend.

## Delivery

Implement the thinnest complete request -> accept/countdown -> cancel tracer through module and
gateway. Add the authoritative Duel row and lifecycle/event representation, the effect-83 request
entry point, authenticated accept/cancel reducers, and the module clock transition that activates a
three-second countdown. Add generated bindings and a narrow gateway Duel handler for the two client
opcodes. Project request, countdown, active player fields, interruption, and duel-flag create/destroy
through typed vanilla packets and sparse updates. Centralize idempotent completion now, even though
winner/no-kill and boundary outcomes are filled in by later tickets.

## Acceptance criteria

- A valid spell-7266 Duel effect creates exactly one Duel and one stable duel-flag identity at the
  participants' midpoint.
- Invalid, dead, self, cross-map/instance, or already-participating requests create nothing.
- Both participant sessions receive one `SMSG_DUEL_REQUESTED` naming the flag arbiter and challenger.
- Only the challenged character can accept; forged GUIDs and initiator accepts are ignored/rejected.
- Accept emits a three-second countdown to both participants and does not authorize combat yet.
- The module clock, not a gateway timer, changes countdown to active and exposes opposing duel-team
  values.
- Decline before active emits interrupted completion and removes state, flag, arbiter, and team
  fields. Cancel after active is represented as surrender so T2 can attach the winner outcome.
- Gateway handler tests use an in-memory Duel store and assert reducer intents/outbound packets.
- Module tests cover request, duplicate membership, accept authorization, exact countdown boundary,
  and idempotent interruption.
- Existing module and gateway tests pass. The work is committed on the ticket branch.
