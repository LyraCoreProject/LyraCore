# T4 — Publish Hunter identity and answer pet name queries

Parent: issue #21. Depends on T1. **Parallel with T2 and T3.**
Model: mid. Estimated size: ~160k tokens.

## Problem

The shared bar is sufficient for a summoned Imp but a Hunter pet also needs identity and advancement
fields plus a name-query response. These reads must cross the coordinator without moving gameplay
authority into the gateway or exposing private durable rows broadly.

## Delivery

- Add the narrow owner-scoped read model/subscription needed to associate a live Hunter pet with its
  durable identity. Avoid a realm-wide private-state broadcast.
- On tame/create and relevant identity changes, publish the owner summon field, pet bar, level,
  experience/next experience, happiness, loyalty and known action slots in the 1.12.1 representation
  supported by the current codec library. Hand-roll only packet shapes the library lacks.
- Handle the 1.12.1 pet-name query request and return the durable Hunter name. Resolve summoned pets
  from their authored creature name. Unknown or unauthorized queries are non-fatal and do not leak
  private state.
- Preserve the current `CMSG_PET_ACTION` forwarding and clear-bar behavior. Do not implement rename
  submission or autocast toggling.

## Acceptance criteria

1. A newly tamed Hunter pet produces a usable pet bar without relogging.
2. Owner-visible fields encode level, XP, next-level XP, happiness and loyalty at their expected
   descriptor positions or packet fields.
3. Identity changes produce bounded owner-only updates rather than recreating the pet for everyone.
4. A Hunter pet-name query returns its durable default name and identity timestamp/number fields
   expected by build 5875.
5. An Imp name query returns its creature-template name and no Hunter care data.
6. Foreign or unknown queries disclose nothing and never end the session.
7. Attack/Follow/Stay/react and bar-clearing tests remain byte-compatible.

## Tests

- Focused codec tests pin exact descriptor masks and packet bytes/typed values.
- Handler/store tests prove name resolution, owner scoping, passthrough and transient rejection.
- Relay tests prove initial publication, identity delta and clear behavior.

## File ownership

Own a focused gateway pet protocol module, codec and coordinator read surface. Keep generated binding
changes isolated. Do not change Hunter gameplay rules.

## Definition of done

`cargo fmt`, gateway tests and focused clippy are clean. Commit only this slice.

