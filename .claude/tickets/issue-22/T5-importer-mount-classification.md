# T5 — mounts: importer classification for Mounted, `E_DISMOUNT`, and riding data

Parent: issue #22. **After T1. Parallel with T2, T3, T4.**
Model: sonnet. Estimated size: ~130k tokens.

## Problem

T1 defines `A_MOUNTED` and `E_DISMOUNT`, but no imported spell carries them. Real mount spells
arrive with a raw vanilla Mounted aura effect, and the real Dazed spell (1604) arrives as a
movement-speed aura plus a `DISPEL_MECHANIC` effect whose parameter is the mount mechanic. The
importer is the one place allowed to interpret that upstream data, and it already does this kind
of reclassification extensively (`importer/src/spell.rs` — Charge, Blink, stances, ground AoE, all
by name or by raw effect id).

Without this ticket, onboarding a real mount would need a spell-id branch at runtime, which the
issue forbids.

## Delivery

**1. Mounted aura → `A_MOUNTED`.** Map the vanilla Mounted aura effect. Freeze `p0` as the
resolved creature **display** id with `p0_kind = P_DISPLAY_ID`. Where the DBC names a mount
*creature template* rather than a display directly, resolve it through the imported
creature-template presentation data at import time. The frozen parameter is a display id in every
case, so runtime never resolves anything.

**2. `DISPEL_MECHANIC` → `E_DISMOUNT`.** Translate a raw `DISPEL_MECHANIC` effect whose parameter
is the mount mechanic (vanilla mechanic 21) into `E_DISMOUNT`. This interprets the upstream data
once.

- Do **not** implement a generic mechanic-dispel system.
- Do **not** branch on spell 1604 or on the name "Dazed".
- **Other raw mechanic-dispel parameters stay unsupported.** Leave them where they fall today and
  record them in the existing unmapped-effect coverage counters, so a future gameplay need shows
  up in the data rather than in a surprise.

Consequence, and the point of the design: Dazed imports as its existing `A_MOD_SPEED` slow plus
`E_DISMOUNT`. Any future spell with the same data shape gets mount removal for free.

**3. Mounted speed stays `A_MOD_SPEED`.** Continue importing mounted-speed aura variants as
`A_MOD_SPEED` with `SPEED_MOUNTED`. Normalize DBC base points so the vanilla stored 59 and 99
produce the nominal integer 60 and 100. T4 owns the fold that consumes them; agree on the stored
representation with T4 before diverging from it.

**4. Riding skill-line data.** Confirm `SkillLineAbility.dbc` import produces `SkillAbility` rows
pairing each mount spell with its riding skill line, with `min_skill` carrying the 75 and 150
tiers and the correct `race_mask` / `class_mask` for the race-specific riding lines. This is
existing plumbing (`importer/src/dbc.rs::skill_ability_sql`); the deliverable is verification plus
whatever mapping gap you find, not a rewrite.

**5. Verify the interrupt data.** The vanilla Brown Horse spell's DBC aura-interrupt value is the
underwater-cancel bit, not the damage bit. Confirm the import carries it through to the existing
`breaks_on_damage` machinery. This is what makes "ordinary damage does not dismount" a data fact
rather than a code rule. Report the finding to T3 and T6.

## Out of scope

The seeded fixture mount, riding rows, riding trainer and **synthetic Dazed spell** all belong to
T2, which owns `module/src/seed/`. Do not touch seed files. This ticket is `importer/` only.

## Acceptance criteria

Covers stories 26, 27, 32, and supports 25.

- [ ] A real mount spell imports with an `A_MOUNTED` effect whose `p0` is a resolved display id
      and whose `p0_kind` is `P_DISPLAY_ID`.
- [ ] A mount spell whose DBC names a creature template resolves to the template's display.
- [ ] A `DISPEL_MECHANIC` effect with mechanic 21 imports as `E_DISMOUNT`.
- [ ] A `DISPEL_MECHANIC` effect with any other mechanic does **not** import as `E_DISMOUNT` and is
      counted as unmapped.
- [ ] No importer output and no runtime path keys on spell id 1604 or on a mount spell name.
- [ ] Mounted-speed effects import as `A_MOD_SPEED` / `SPEED_MOUNTED` with 59 → 60 and 99 → 100.
- [ ] Riding `SkillAbility` rows carry the right `skill_line`, `min_skill` (75 / 150) and masks.
- [ ] The Brown Horse interrupt value imports as the underwater-cancel bit, and the report states
      whether any mount spell carries the damage bit.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-importer`, `cargo test -p lyracore-importer` clean. Push to
`feat/issue-22-mounts`. Report the exact raw effect and aura ids you mapped, the mounted-speed
storage representation, and the interrupt-flag finding.
