# T2 — mounts: riding gate, trainer wiring, item path, fixtures

Parent: issue #22. **After T1. Parallel with T3, T4, T5.**
Model: opus. Estimated size: ~170k tokens.

## Problem

After T1 a mount aura exists but anyone can apply it. Vanilla gates a mount cast on trained riding
skill at the right rank and line, on a living out-of-combat caster, on being outdoors, and on not
being in deep water. Mount items must also stop being consumed like potions, and nothing can be
tested headlessly without a reserved fixture mount.

## Delivery

**1. The gate.** Add a mount-eligibility check in `module/src/mount.rs` (gate region — T1 owns the
core region of the same file; append, do not interleave). Every check runs **before** item
consumption, aura creation, field writes, speed changes, cooldowns or combat changes. A refusal
leaves the world byte-identical.

- **Riding.** Join `module/src/skilldata.rs`'s `game_skill_ability` (`by_skill_line`) against
  `module/src/skill.rs`'s `game_player_skill` (`by_character`). The mount spell must have a
  `SkillAbility` row whose `skill_line` the player has learned, and the player's current rank must
  be at least that row's `min_skill`. `min_skill` is where the vanilla 75 and 150 tiers live. Race
  and class masks on the row still apply, so a player with the wrong race-specific riding line
  fails. **Never substitute character level for trained riding.**
- **Alive and out of combat.** Use the module's authoritative combat state, not gateway session
  state. A dead or ghost player is refused.
- **Liquid.** Reuse `crates/lyracore-shared/src/env.rs::is_submerged`, the same predicate player
  breath uses. Do not add a second water-position interpretation. Shallow water, or standing at
  the surface, is not a refusal.
- **Indoors.** Call the existing WMO area-info query; `indoor == true` refuses. Use the query as
  it stands today. **T3 fixes the underlying vmap indoor interpretation** — write your call so it
  reads the shared predicate rather than inlining a flag test, and do not touch
  `crates/lyracore-shared/src/vmap.rs`.

**2. Item path.** In `module/src/items/ops.rs`, mirror `spell_is_recall_home` (line ~391): a spell
carrying an `A_MOUNTED` effect is a data-driven non-consuming on-use spell. **No item-entry
allowlist** — unlike recall, the classification is purely by effect kind. A mount item stays in
the bag after use, and a refused mount does not consume it either.

**3. Riding trainer.** The Riding skill must be learnable from the riding-trainer NPC concept, not
only seeded. `crates/lyracore-shared/src/trainer.rs::trainer_type::MOUNTS` already exists and
serves every class. Extend `module/src/trainer.rs` (`trainer_buy_check`,
`validate_trainer_interaction`, `apply_trainer_buy`) and `gateway/src/world/handlers/trainer.rs`
so a MOUNTS trainer teaches the riding skill line and its rank. Follow the existing profession and
weapon trainer paths (`module/src/skill.rs::debug_learn_profession_from_trainer`,
`debug_learn_weapon_from_trainer`) rather than inventing a third shape.

**4. Fixtures** (this ticket owns everything under `module/src/seed/` and `module/src/seed.rs`
for the slice). Keep every entry collision-safe with imported worlds, following the existing
reserved-range convention:

- One reusable mount item with a self-cast mount spell.
- That spell's `A_MOUNTED` effect (frozen display id) plus a 60% `SPEED_MOUNTED` effect.
- A riding skill line, a `SkillAbility` row pairing the spell to it, and the player skill rows a
  fixture character needs.
- A riding-trainer creature, following `module/src/seed/fixtures.rs::profession_trainer_template`.
- A **synthetic Dazed spell** carrying `A_MOD_SPEED` plus `E_DISMOUNT`. It exists only to prove
  spell resolution and mount integration headlessly. It does not pretend to implement the absent
  mob-chase proc, and it must not be described as doing so. (T5 owns the *import* classification
  of the real Dazed spell; this is the seeded stand-in.)

## Acceptance criteria

Covers stories 1, 2, 12, 13, 14, 15, 16, 17, 18, 20, 31, 32, 33.

- [ ] An untrained player's mount cast fails, and the item, auras, display, speed, cooldowns and
      combat state are all unchanged.
- [ ] Riding rank 74 fails a rank-75 mount; rank 75 succeeds. Rank 149 fails a rank-150 mount;
      rank 150 succeeds.
- [ ] A player holding a different race's riding line fails a mount that requires the other line.
- [ ] In combat, dead, ghost, and submerged all refuse without mutation. Standing in shallow water
      does not refuse.
- [ ] The indoor refusal goes through the shared area query; no flag test is inlined at the call
      site, and `crates/lyracore-shared/src/vmap.rs` is untouched in this diff.
- [ ] Using the fixture mount item leaves its stack count unchanged, both on success and on every
      refusal.
- [ ] A player who buys riding from the MOUNTS trainer can then mount; the same player without the
      purchase cannot. This path has its own test, separate from the seeded-skill fixtures.
- [ ] The fixture entries do not collide with an imported world.
- [ ] No runtime code names a spell or branches on a spell id.

## Definition of done

`cargo fmt`, `cargo clippy`, and `cargo test` clean for `lyracore-module` and `lyracore-gateway`.
Push to `feat/issue-22-mounts`. Report the reserved fixture entry ids so T6's verification doc can
name them.
