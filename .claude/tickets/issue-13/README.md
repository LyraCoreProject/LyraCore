# Issue #13 — Duels, broken into tickets

Source: `gh issue view 13 --comments` — "Duels (CMSG_DUEL_* flow, duel flag, first-to-1-HP finish)".

Read the current GitHub issue body before working. It is the authoritative spec produced by the
`to-spec` pass. These tickets only slice that spec; they do not narrow it.

## State of the world

No duel implementation exists. The checked-in `wow_world_messages` vanilla API already provides
typed `CMSG_DUEL_ACCEPTED`, `CMSG_DUEL_CANCELLED`, `SMSG_DUEL_REQUESTED`,
`SMSG_DUEL_OUTOFBOUNDS`, `SMSG_DUEL_INBOUNDS`, `SMSG_DUEL_COMPLETE`, `SMSG_DUEL_WINNER`, and
`SMSG_DUEL_COUNTDOWN` messages. Duel initiation is not a client duel opcode: the normal spell-cast
path casts spell 7266, whose effect kind 83 is Duel.

All game state and rules belong in the SpacetimeDB module. The gateway is stateless and translates
packets to reducers and public/event-table changes back to packets. Do not make a gateway timer the
Duel clock.

The combat module has two critical existing seams:

- engagement authorization in the start-attack and spell target gates;
- `fold_incoming_damage` -> `apply_hit`, the shared damage application path used by melee, ranged,
  and spell damage. Player death is centralized behind that path.

The gateway has narrow handler seams shaped like `MeleeActionStore` + player facts + a dispatcher
that returns `Handled { outbound, transition }` or `PassThrough`. Copy that shape for accept/cancel.
Sparse player descriptor updates already live beside the other VALUES builders. AOI/event relays
already route public module changes to the relevant sessions.

## Shared decisions

- Domain terms are `Duel`, `duel flag`, `initiator`, `challenged character`, `participant`,
  `countdown`, `active`, `out of bounds`, `fled`, `interrupted`, `won`, `winner`, and `loser`.
- One authoritative Duel row represents both participants and is indexed from either participant.
- One participant belongs to at most one live Duel.
- Request comes from effect 83 of spell 7266. The challenged character alone may accept.
- Countdown is three seconds. Same-faction combat is legal only after it expires.
- Boundary is the original vanilla 50 yards from the duel flag, with a ten-second flee grace period.
- Opponent Duel damage floors the loser at 1 health and never enters the player death path.
- Completion is centralized and idempotent. It clears Duel state, duel fields, and Duel combat.
- Normal same-faction damage remains forbidden before countdown and after teardown.
- All schema additions follow non-destructive migration rules. Regenerate gateway bindings.
- Do not touch production or any live database. Tests and optional runtime checks use local fixtures.
- Keep comments concise. Do not add issue-number comments.

## Execution DAG

```text
T1 tracer: request, accept/cancel, typed wire projection
                    |
          +---------+---------+
          |                   |
T2 combat authorization   T3 boundary and cleanup
          |                   |
          +---------+---------+
                    |
          T4 integration and verification
```

T1 runs alone. T2 and T3 branch from the integrated T1 result and run in parallel. T4 runs alone on
their combined result. The integration ticket reconciles any overlap, regenerates/checks bindings,
runs the union of tests, records the manual local-client check, and removes this ticket directory
from the shipping diff.

## Verification discipline

Prefer the highest stable seams:

1. module lifecycle tests through request/accept/tick/damage operations;
2. gateway handler tests through an in-memory Duel action adapter;
3. packet/VALUES codec tests;
4. a headless two-session flow where the existing harness can support it.

Do not create a test per private helper. A focused matrix of observable transitions is better than
duplicating every damage source. Structural tests may prove that all damage resolvers still route
through the shared damage seam.
