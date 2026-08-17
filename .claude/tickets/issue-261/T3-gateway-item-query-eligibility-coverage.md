# T3 — Cover the real Gateway item-query eligibility path

Parent: issue #261. **Blocks on T1; parallel with T2.**
Model: gpt-5.6-terra. Estimated size: ~150k tokens.

## Problem

Codec-only field tests could hide a lost value between the Coordinator durable read and the encrypted
world-session reply.  The defect is visible in quest and vendor screens only because both cause the
same client item query.

## Delivery

- Find the narrowest existing encrypted world-session item-query test and extend it to insert/read a
  real imported-equipment-shaped template with non-zero masks and existing requirements.
- Decode the reply and assert it carries the exact masks and requirement fields.  Express client
  eligibility in test-local terms from that one definition rather than inventing icon-colour code.
- Exercise the same entry when exposed through existing quest-reward and vendor fixtures, proving
  those paths lead the client to the one cached item-query definition and not screen-specific data.
- Add negative coverage for excluded class/race and a missing proficiency, using the T1 data model
  only.  If the proof identifies a missing Warrior starting skill, make the smallest authentic
  skill-seeding change and test the externally visible `PLAYER_SKILL_INFO` rows; otherwise do not
  alter skill seeding.
- Keep tests in owned gateway test files/helpers only.  Do not edit T1 production files or T2 Module
  equip files.

## Acceptance criteria

- The high seam proves a non-zero, restrictive imported template survives Coordinator read, world
  dispatch, encryption, decode, and the item-query response.
- Quest and vendor exposure use the same entry/query definition.
- Negative cases demonstrate that the test is not an all-bits workaround.
- Existing query and skill relay behavior remains covered; focused Gateway tests pass.
