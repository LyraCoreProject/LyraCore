# T1 — Add and import canonical item eligibility masks

Parent: issue #261. **Runs alone; blocks T2 and T3.**
Model: gpt-5.6-sol. Estimated size: ~190k tokens.

## Problem

The imported item template and the gateway query view omit `AllowedClass` and `AllowedRace`.
The stock 1.12 client treats the packet defaults as empty bitmasks, so all queried equipment is
shown as unusable.  Existing durable rows also need a safe value during an additive schema change.

## Delivery

- Establish named all-playable class/race constants at the smallest appropriate item-data boundary.
- Add END-appended `u32` mask columns to `ItemTemplate` with additive-migration defaults equal to
  the complete playable masks, then update every hand-authored template literal and generated
  Gateway binding required by a module publish/build.
- Establish the actual Classic dump column positions and source representation.  Import values
  preserving restrictive/unknown bits; normalize only the unrestricted source sentinel to the
  all-playable constants.  Update the item insert column list and importer coverage reporting if
  it can identify malformed effective masks without a speculative global content scan.
- Carry the mask fields through `ItemTemplateView` and encode them in
  `SMSG_ITEM_QUERY_SINGLE_RESPONSE` using the packet library's typed mask fields.
- Add focused tests: importer durable-row fixtures for unrestricted and restrictive masks, plus an
  item-query codec round trip checking exact non-zero class/race values alongside the existing
  requirement fields.

## Acceptance criteria

- A restrictive input mask reaches the durable row and query response unchanged.
- The Classic unrestricted input becomes explicit all-playable masks before it is durable.
- Existing rows migrated before a reimport have non-zero all-playable effective masks.
- No mask is reconstructed from known local class/race variants; `u32` high bits survive.
- Existing required level, skill/rank, and reputation query fields remain intact.
- Focused importer, module, and gateway codec checks pass; leave a concise handoff stating the
  constants, source sentinel, and changed test commands for T2/T3.
