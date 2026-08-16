# T1 — Add trainer rank-upgrade protocol coverage

Parent: issue #37. **Runs alone; blocks T2.**
Model: gpt-5.6-terra. Estimated size: ~80k tokens.

## Problem

The trainer handler already chooses `SMSG_SUPERCEDED_SPELL` for a non-stacking rank upgrade, but
the encrypted socket suite only proves the matching talent behavior. A regression in the trainer
path could therefore compile and ship without exercising the actual client opcode dispatch.

## Delivery

Extend the existing in-memory `WorldStore` fixture only as needed to return a configured resolved
rank and an optional known predecessor. Add a focused trainer-buy socket test for a non-mana or
passive rank upgrade. It must authenticate and enter the world, send the trainer-buy client
message, consume the trainer success reply, then assert that the next server message is
`SMSG_SUPERCEDED_SPELL` with old rank first and new rank second.

Keep the existing normal trainer-buy learned-spell behavior covered. Do not test private handler
helpers directly and do not change production rank-selection logic.

## Acceptance criteria

- The new test drives the public encrypted world session through a trainer purchase.
- It observes trainer-buy success followed by `SMSG_SUPERCEDED_SPELL`.
- It asserts cMaNGOS wire order: old rank in the first encoded field, new rank in the second.
- Existing first-rank behavior still emits `SMSG_LEARNED_SPELL`.
- `cargo fmt` and the focused test pass.
