# T2 — Enforce item eligibility in Module equip operations

Parent: issue #261. **Blocks on T1; parallel with T3.**
Model: gpt-5.6-terra. Estimated size: ~160k tokens.

## Problem

Even after the Gateway tells the client the correct eligibility masks, a forged equip request can
bypass explicit class/race and required-skill/reputation rules because `apply_item_move` currently
only checks slot, level, and the legacy derived proficiency taxonomy.

## Delivery

- Consume T1's durable `allowed_class` and `allowed_race` fields in the one existing equipment
  branch of `apply_item_move`; require the Character's race/class bit in the appropriate mask.
- Add a small, named pure mask-membership helper if it keeps the rules readable; zero masks fail
  closed and no helper treats zero as unrestricted.
- Gate a non-zero `required_skill` against the Character's durable `game_player_skill` row and
  require its current rank to meet `required_skill_rank`.  Reuse existing skill/reputation tables
  and `reputation_rank` rather than maintaining another derived record.
- Gate a non-zero required reputation faction/rank.  Establish whether absent player-reputation
  rows mean the faction's durable base standing or Neutral, and match current Module semantics.
- Retain the legacy `can_equip_proficiency` rule unless the evidence demonstrates it is exactly
  subsumed.  This ticket must not accidentally widen current weapon/armor gates.
- Add focused Module tests covering allowed and excluded class/race, missing/low skill, rank-qualified
  skill, low/qualified reputation, and unchanged placement after every Refusal.

## Acceptance criteria

- The actual actor/equip Durable Request refuses a forged request that fails any explicit mask,
  required skill/rank, level, reputation, or existing proficiency condition.
- A Character satisfying all gates still equips normally.
- Refusal leaves source and destination item rows unchanged and preserves BoE semantics.
- No Gateway-only enforcement or individual item/vendor/quest exception is added.
- Relevant Module tests and formatting pass; hand off any testing fixture API T4 needs.
