//! Pure predicate/curve unit tests for the spell engine. `use super::*` reaches every symbol via
//! `mod.rs`'s re-exports (a handful were widened to `pub(crate)` for cross-submodule reach — e.g.
//! `rederive_pool`).

use super::*;
use spacetimedb::Timestamp;

/// Cost gate is a pure `>=`: 0-cost always affords, exact-cost lands, one short rejects.
#[test]
fn can_afford_gate() {
    assert!(can_afford(10, 0));
    assert!(can_afford(10, 10));
    assert!(!can_afford(10, 11));
}

/// Creature-cast exemptions (caster mobs + the Imp): the level + cost gates apply to PLAYERS only, so a
/// 0-power mob can cast an above-level rank. Keyed on `is_player` — the player path stays byte-identical.
#[test]
fn creature_is_exempt_from_level_and_cost_gates() {
    // COST gate: a player short on power is blocked; a 0-power creature (Imp/Geomancer) is NOT.
    assert!(cost_gate_blocks(true, 0, 90)); // player, 0 power, Frostbolt cost 90 → blocked
    assert!(!cost_gate_blocks(false, 0, 90)); // creature, 0 power → exempt (casts)
    assert!(!cost_gate_blocks(true, 90, 90)); // player can afford → not blocked (baseline)
    assert!(!cost_gate_blocks(false, 0, 0)); // cost-0 (Firebolt 3110) → never blocked, either way
                                             // LEVEL gate: a L9 player can't cast a spell_level-20 rank; a L9 creature (Defias Rogue Wizard) can.
    assert!(level_gate_blocks(true, 20, 9)); // player below the rank's level → blocked
    assert!(!level_gate_blocks(false, 20, 9)); // creature → exempt (above-level rank by design)
    assert!(!level_gate_blocks(true, 0, 1)); // spell_level 0 → never gates (baseline seed kit)
    assert!(!level_gate_blocks(true, 1, 1)); // at-level player → not blocked (baseline)
}

/// Spell crit multiplier is ×1.5 (vanilla, NOT ×2 like melee); no crit leaves damage unchanged.
#[test]
fn spell_crit_is_one_and_a_half() {
    assert_eq!(apply_spell_crit(100, false), 100); // no crit → unchanged (baseline-safe)
    assert_eq!(apply_spell_crit(100, true), 150); // crit → ×1.5
    assert_eq!(apply_spell_crit(20, true), 30); // Fireball 20 → 30 on crit
    assert_eq!(apply_spell_crit(7, true), 10); // integer *3/2 truncates (7→10), never panics
    assert_eq!(apply_spell_crit(0, true), 0); // a 0-damage effect stays 0
}

/// Incoming-damage modifier (A_MOD_DAMAGE_TAKEN): signed percent, 0 = no-op, ≥100% reduction = immunity.
/// `apply_damage_pct` is direction-neutral (also folds the OUTGOING percent — see its doc comment); this
/// test exercises it through the incoming-damage framing its name used to (wrongly) assert alone.
#[test]
fn damage_taken_modifier_scales_and_clamps() {
    assert_eq!(apply_damage_pct(100, 0), 100); // no aura → unchanged (baseline-safe)
    assert_eq!(apply_damage_pct(100, -75), 25); // Shield Wall: 75% reduction → 25% taken
    assert_eq!(apply_damage_pct(100, 50), 150); // a +50% vulnerability debuff
    assert_eq!(apply_damage_pct(100, -100), 0); // 100% reduction → full immunity (0)
    assert_eq!(apply_damage_pct(100, -250), 0); // over-reduction clamps at 0, never negative
    assert_eq!(apply_damage_pct(7, -75), 1); // integer math on a small hit (7*25/100 = 1)
}

/// Stance usability gate: a 0 mask permits ANY stance (baseline-safe); a non-zero mask permits only the
/// stances whose 0-based bit is set. Battle=bit0, Defensive=bit1, Berserker=bit2 (vanilla form bits 16/17/18
/// translated to 0-based by the importer).
#[test]
fn stance_allows_gate() {
    // 0 mask → usable in every stance (every non-warrior spell, every unrestricted ability, the switches).
    assert!(stance_allows(0, STANCE_BATTLE));
    assert!(stance_allows(0, STANCE_DEFENSIVE));
    assert!(stance_allows(0, STANCE_BERSERKER));
    // Charge/Thunder Clap/Overpower: Battle-only (bit0 = 0x01).
    assert!(stance_allows(0x01, STANCE_BATTLE));
    assert!(!stance_allows(0x01, STANCE_DEFENSIVE));
    assert!(!stance_allows(0x01, STANCE_BERSERKER));
    // Rend: Battle OR Defensive (bits 0,1 = 0x03).
    assert!(stance_allows(0x03, STANCE_BATTLE));
    assert!(stance_allows(0x03, STANCE_DEFENSIVE));
    assert!(!stance_allows(0x03, STANCE_BERSERKER));
    // Hamstring: Battle OR Berserker (bits 0,2 = 0x05).
    assert!(stance_allows(0x05, STANCE_BATTLE));
    assert!(!stance_allows(0x05, STANCE_DEFENSIVE));
    assert!(stance_allows(0x05, STANCE_BERSERKER));
    // An out-of-range stance is disallowed under a non-zero mask (the shift is bounded → no overflow/panic).
    assert!(!stance_allows(0x01, 9));
    // Druid combat forms (work-item 156 — stance ids 3/4/5 flow through the SAME gate/mask convention):
    // a Maul/Growl-shaped Bear|DireBear mask (bits 3,5 = 0x28) admits Bear and Dire Bear only.
    assert!(stance_allows(0x28, STANCE_BEAR));
    assert!(stance_allows(0x28, STANCE_DIRE_BEAR));
    assert!(!stance_allows(0x28, STANCE_CAT));
    assert!(!stance_allows(0x28, STANCE_BATTLE)); // caster form (stance 0) is NOT bear — the gate holds
                                                  // A Cat-only ability (bit 4 = 0x10) is refused in Bear and in caster form.
    assert!(stance_allows(0x10, STANCE_CAT));
    assert!(!stance_allows(0x10, STANCE_BEAR));
    assert!(!stance_allows(0x10, STANCE_BATTLE));
    // A 0 mask admits the druid forms too (a druid in Bear can still cast an unrestricted spell as far
    // as THIS gate is concerned — vanilla's "can't cast while shapeshifted" is a separate rule, not ours).
    assert!(stance_allows(0, STANCE_BEAR));
}

/// The stance → client ShapeshiftForm byte map (UNIT_FIELD_BYTES_1[2], written by the E_SET_STANCE arm):
/// the inverse of the importer's form_to_stance per the taxonomy STANCE_* convention block. The warrior
/// trio MUST stay byte-identical to the pre-156 inline `stance + 17` (pinned against hardcoded values,
/// not the formula); unassigned ids keep the legacy fallback.
#[test]
fn client_form_for_stance_matches_the_convention() {
    assert_eq!(client_form_for_stance(STANCE_BATTLE), 17);
    assert_eq!(client_form_for_stance(STANCE_DEFENSIVE), 18);
    assert_eq!(client_form_for_stance(STANCE_BERSERKER), 19);
    assert_eq!(client_form_for_stance(STANCE_BEAR), 5);
    assert_eq!(client_form_for_stance(STANCE_CAT), 1);
    assert_eq!(client_form_for_stance(STANCE_DIRE_BEAR), 8);
    // Unassigned stance ids (6, 7) fall back to the legacy warrior formula — the pre-156 behavior.
    assert_eq!(client_form_for_stance(6), 23);
    assert_eq!(client_form_for_stance(7), 24);
}

/// Rage-clear gate for E_SET_STANCE: an ACTUAL stance change on a RAGE-power caster clears power;
/// re-casting the CURRENT stance (no-op switch) or a non-RAGE power type (mana/energy) never does. Mirrors
/// the exact guard in cast.rs's E_SET_STANCE arm — live/integration coverage can't exercise this arm (no
/// stance spells in the dev DBC subset), so it is covered here instead.
#[test]
fn stance_switch_clears_power_only_on_real_switch_for_rage() {
    use lyracore_shared::packing::power_type;
    // A genuine Battle -> Defensive switch on a rage caster (Warrior) zeroes power.
    assert!(stance_switch_clears_power(
        STANCE_BATTLE,
        STANCE_DEFENSIVE,
        power_type::RAGE
    ));
    assert!(stance_switch_clears_power(
        STANCE_DEFENSIVE,
        STANCE_BERSERKER,
        power_type::RAGE
    ));
    // Re-casting the SAME stance is not a switch — never clears, even for a rage caster.
    assert!(!stance_switch_clears_power(
        STANCE_BATTLE,
        STANCE_BATTLE,
        power_type::RAGE
    ));
    // A real switch on a non-rage power type (mana/energy) is untouched — spared by design.
    assert!(!stance_switch_clears_power(
        STANCE_BATTLE,
        STANCE_DEFENSIVE,
        power_type::MANA
    ));
    assert!(!stance_switch_clears_power(
        STANCE_BATTLE,
        STANCE_DEFENSIVE,
        power_type::ENERGY
    ));
}

/// Form-recast toggle-off gate (156 review): recasting the ACTIVE druid form (Bear/Cat/DireBear)
/// leaves it; a warrior recasting his active stance stays a no-op (warriors are never formless);
/// switching TO a different form/stance is never a toggle (the change arm handles it).
#[test]
fn form_recast_toggles_off_only_for_active_druid_forms() {
    // Druid recasting the form he is IN → leave it.
    assert!(form_recast_toggles_off(STANCE_BEAR, STANCE_BEAR));
    assert!(form_recast_toggles_off(STANCE_CAT, STANCE_CAT));
    assert!(form_recast_toggles_off(STANCE_DIRE_BEAR, STANCE_DIRE_BEAR));
    // Warrior recasting his current stance → no-op, never formless.
    assert!(!form_recast_toggles_off(STANCE_BATTLE, STANCE_BATTLE));
    assert!(!form_recast_toggles_off(STANCE_DEFENSIVE, STANCE_DEFENSIVE));
    assert!(!form_recast_toggles_off(STANCE_BERSERKER, STANCE_BERSERKER));
    // A genuine change is not a toggle in either direction (druid or warrior).
    assert!(!form_recast_toggles_off(STANCE_BEAR, STANCE_CAT));
    assert!(!form_recast_toggles_off(0, STANCE_BEAR));
}

/// Stance combat folds: only Defensive (stance 1) contributes; Battle/Berserker/non-warrior (0/2) are inert
/// (0), so every existing unit's combat is byte-identical (baseline-safe). The magnitudes come from the
/// single tuning constants.
#[test]
fn stance_folds_only_defensive_contributes() {
    // Damage taken: Defensive −10%, everything else 0.
    assert_eq!(stance_damage_taken_pct(STANCE_BATTLE), 0);
    assert_eq!(
        stance_damage_taken_pct(STANCE_DEFENSIVE),
        STANCE_DEFENSIVE_DR_PCT
    );
    assert_eq!(stance_damage_taken_pct(STANCE_BERSERKER), 0);
    // Damage dealt: Defensive −10%, everything else 0.
    assert_eq!(stance_damage_done_pct(STANCE_BATTLE), 0);
    assert_eq!(
        stance_damage_done_pct(STANCE_DEFENSIVE),
        STANCE_DEFENSIVE_DMG_DONE_PCT
    );
    assert_eq!(stance_damage_done_pct(STANCE_BERSERKER), 0);
    // Threat: Defensive +30%, everything else 0.
    assert_eq!(stance_threat_pct(STANCE_BATTLE), 0);
    assert_eq!(
        stance_threat_pct(STANCE_DEFENSIVE),
        STANCE_DEFENSIVE_THREAT_PCT
    );
    assert_eq!(stance_threat_pct(STANCE_BERSERKER), 0);
    // The folds compose through the existing signed-percent math: a 100-damage hit in Defensive takes/deals 90.
    assert_eq!(
        apply_damage_pct(100, stance_damage_taken_pct(STANCE_DEFENSIVE)),
        90
    );
    assert_eq!(
        apply_damage_pct(100, stance_damage_done_pct(STANCE_DEFENSIVE)),
        90
    );
    // In Battle/Berserker the fold is a no-op (byte-identical to today).
    assert_eq!(
        apply_damage_pct(100, stance_damage_taken_pct(STANCE_BATTLE)),
        100
    );
}

/// Spell crit CHANCE folds A_MOD_COMBAT(CRIT) auras onto the base and clamps to [0, 10000].
#[test]
fn spell_crit_chance_folds_aura_and_clamps() {
    assert_eq!(spell_crit_bp_with_bonus(0), SPELL_CRIT_BP); // no +crit aura → exactly the base
    assert_eq!(spell_crit_bp_with_bonus(1000), SPELL_CRIT_BP + 1000); // +10% crit aura raises it
    assert_eq!(spell_crit_bp_with_bonus(-5000), 0); // a huge debuff floors at 0 (no negative band)
    assert_eq!(spell_crit_bp_with_bonus(100_000), 10_000); // clamps at the 100% line
}

/// Intellect-derived spell crit: the exact `int * 100 / (level + 5)` curve, integer-floored — pinned so a
/// drift in the divisor/multiplier tuning knobs is caught, not just their monotonic shape.
#[test]
fn spell_crit_derives_from_intellect_and_level() {
    assert_eq!(intellect_crit_bp(23, 2), 328); // L2 starter INT: 23*100/7 = 2300/7 = 328 (floored)
    assert_eq!(intellect_crit_bp(60, 2), 857); // 60*100/7 = 6000/7 = 857 (floored)
    assert_eq!(intellect_crit_bp(23, 10), 153); // 23*100/15 = 2300/15 = 153 (floored)
    assert_eq!(intellect_crit_bp(0, 2), 0); // no INT → 0 (a creature/melee unit casting contributes none)
    assert_eq!(intellect_crit_bp(150, 60), 230); // a L60-ish ~150-INT caster: 150*100/65 = 15000/65 = 230
                                                 // Shape invariants the exact pins above also guarantee: more INT → more crit (level held); higher
                                                 // level → less per-INT; a low-INT L2 caster stays well under 10% crit (1000bp).
    assert!(intellect_crit_bp(60, 2) > intellect_crit_bp(23, 2));
    assert!(intellect_crit_bp(23, 10) < intellect_crit_bp(23, 2));
    assert!(intellect_crit_bp(23, 2) < 1000);
}

/// Level-based spell miss: ~4% vs equal/lower, climbing to 17% at +3, then +11%/level.
#[test]
fn spell_miss_follows_the_level_hit_curve() {
    assert_eq!(spell_miss_chance_bp(10, 10), 400); // equal level → 4% (≈96% hit)
    assert_eq!(spell_miss_chance_bp(10, 5), 400); // lower-level target → still the 4% floor
    assert_eq!(spell_miss_chance_bp(10, 11), 500); // +1 → 5%
    assert_eq!(spell_miss_chance_bp(10, 12), 600); // +2 → 6%
    assert_eq!(spell_miss_chance_bp(10, 13), 1700); // +3 → 17% (the done-when target)
    assert!(spell_miss_chance_bp(10, 14) > spell_miss_chance_bp(10, 13)); // keeps rising past +3
    assert_eq!(spell_miss_chance_bp(1, 60), 9900); // capped at an absurd delta (never 100%)
}

/// Damage subtracts, FLOORS at 1, negative basis = 0.
#[test]
fn damaged_value_floors_at_one() {
    assert_eq!(damaged_value(42, 20), 22);
    assert_eq!(damaged_value(10, 50), 1);
    assert_eq!(damaged_value(42, -5), 42);
    assert_eq!(damaged_value(1, 100), 1);
}

/// Heal clamps to max, saturates, negative basis = 0.
#[test]
fn healed_value_clamps() {
    assert_eq!(healed_value(5, 42, 20), 25);
    assert_eq!(healed_value(30, 42, 20), 42);
    assert_eq!(healed_value(10, 42, -5), 10);
    assert_eq!(healed_value(0, 42, 50), 42);
}

/// Energize restores power, clamps to max, saturates, negative basis = 0 (the power twin of heal).
#[test]
fn energized_value_clamps() {
    assert_eq!(energized_value(5, 100, 20), 25);
    assert_eq!(energized_value(90, 100, 20), 100); // clamp to max
    assert_eq!(energized_value(40, 100, -5), 40); // negative = no gain
    assert_eq!(energized_value(0, 100, 100), 100);
}

/// 3-D distance is the Euclidean norm: a 3-4-5 triangle in the XY plane is 5; same point is 0; a
/// pure-Z separation is its magnitude.
#[test]
fn distance_3d_euclidean() {
    assert_eq!(distance_3d(0.0, 0.0, 0.0, 3.0, 4.0, 0.0), 5.0);
    assert_eq!(distance_3d(1.0, 2.0, 3.0, 1.0, 2.0, 3.0), 0.0);
    assert_eq!(distance_3d(0.0, 0.0, 0.0, 0.0, 0.0, 7.0), 7.0);
    // sign-independent (order of the points doesn't matter).
    assert_eq!(distance_3d(3.0, 4.0, 0.0, 0.0, 0.0, 0.0), 5.0);
}

/// `is_behind`: the Backstab positional gate. Target sits at the origin; orientation = the facing dir
/// (0 = +X). "Behind" is the rear 180° hemisphere centered on `orientation + π`. Covers directly-behind,
/// directly-in-front, the two side quadrants, and the ±π wraparound seam.
#[test]
fn is_behind_rear_hemisphere() {
    use std::f32::consts::{FRAC_PI_2, PI};
    // Target at origin FACING +X (orientation 0). Its back points to -X.
    // Attacker directly BEHIND (on -X) → behind.
    assert!(is_behind(-1.0, 0.0, 0.0, 0.0, 0.0));
    // Attacker directly IN FRONT (on +X) → not behind.
    assert!(!is_behind(1.0, 0.0, 0.0, 0.0, 0.0));
    // Side quadrants (on ±Y, perpendicular to facing) — exactly 90° off the back → ON the boundary,
    // which we count as behind (`<=` inclusive), matching the rear-180° arc.
    assert!(is_behind(0.0, 1.0, 0.0, 0.0, 0.0));
    assert!(is_behind(0.0, -1.0, 0.0, 0.0, 0.0));
    // Just INSIDE the front quadrant (slightly off +X, still front hemisphere) → not behind.
    assert!(!is_behind(1.0, 0.5, 0.0, 0.0, 0.0));
    assert!(!is_behind(1.0, -0.5, 0.0, 0.0, 0.0));
    // ±π wraparound: target FACING -X (orientation π) → its back points to +X. An attacker on +X is now
    // behind. Without the (-π,π] normalization the raw diff near the seam would mis-classify this.
    assert!(is_behind(1.0, 0.0, 0.0, 0.0, PI));
    assert!(!is_behind(-1.0, 0.0, 0.0, 0.0, PI));
    // Same seam from the negative side: orientation -π is the same facing as +π.
    assert!(is_behind(1.0, 0.0, 0.0, 0.0, -PI));
    // A non-axis facing (orientation = π/2, faces +Y) → back points to -Y; attacker on -Y is behind.
    assert!(is_behind(0.0, -1.0, 0.0, 0.0, FRAC_PI_2));
    assert!(!is_behind(0.0, 1.0, 0.0, 0.0, FRAC_PI_2));
    // Coincident point has no bearing → not behind.
    assert!(!is_behind(0.0, 0.0, 0.0, 0.0, 0.0));
}

/// Dispel category match: category 0 strips EVERY foreign aura (the strip-all baseline); a non-zero
/// category strips only the matching one (magic dispel leaves a curse/poison/disease alone).
#[test]
fn dispel_category_match() {
    // category 0 = strip-all, regardless of the aura's own category.
    assert!(dispel_category_matches(0, 0));
    assert!(dispel_category_matches(0, 1));
    assert!(dispel_category_matches(0, 4));
    // a magic dispel (1) strips only magic (1).
    assert!(dispel_category_matches(1, 1));
    assert!(!dispel_category_matches(1, 2)); // not a curse
    assert!(!dispel_category_matches(1, 0)); // not an un-categorized aura
                                             // a curse dispel (2) strips only curse (2).
    assert!(dispel_category_matches(2, 2));
    assert!(!dispel_category_matches(2, 1));
}

/// GCD comparison is a strict `<`; the boundary is OFF.
#[test]
fn is_on_cooldown_strict() {
    assert!(is_on_cooldown(1000, 2000));
    assert!(!is_on_cooldown(2000, 2000));
    assert!(!is_on_cooldown(3000, 2000));
}

/// The per-spell cooldown READY check is the exact negation of `is_on_cooldown`: still-cooling before
/// `ready_at`, READY at and after the boundary tick. The two must never agree (one is `<`, the other
/// `>=`) — a drift would let a cast pass BOTH or fail BOTH gates at the boundary.
#[test]
fn is_ready_at_is_inverse_of_on_cooldown() {
    assert!(!is_ready_at(1000, 2000)); // before ready → not ready
    assert!(is_ready_at(2000, 2000)); // boundary tick → ready (the `>=`)
    assert!(is_ready_at(3000, 2000)); // past ready → ready
                                      // Exact negation across the boundary and around it.
    for (now, ready) in [(0, 0), (1000, 2000), (2000, 2000), (3000, 2000), (5, 4)] {
        assert_ne!(is_ready_at(now, ready), is_on_cooldown(now, ready));
    }
}

/// Kick school-lockout is active while `until` is STRICTLY in the future; the boundary tick is expired
/// (matching the cooldown gates' "ready at the boundary" `>=`/`<` convention, so a school unlocks exactly
/// when its lock elapses rather than one tick late).
#[test]
fn school_locked_is_active_until_the_boundary() {
    assert!(school_locked(1000, 2000)); // now before until → still locked
    assert!(!school_locked(2000, 2000)); // boundary tick → expired (unlocked)
    assert!(!school_locked(3000, 2000)); // past until → unlocked
                                         // A zero/absent until (an expired or never-set lock) never locks at any sane now.
    assert!(!school_locked(0, 0));
    assert!(!school_locked(5000, 0));
}

/// Cast-finish deadline is `now + cast_time_ms*1000`.
#[test]
fn cast_finish_micros_adds_cast_time() {
    assert_eq!(cast_finish_micros(1_000_000, 1500), 2_500_000);
    assert_eq!(cast_finish_micros(5, 0), 5);
    assert_eq!(cast_finish_micros(0, 1), 1_000);
}

/// effect_amount: no scaling (no die, no per-level) = base; die adds `roll%die`; per-level adds
/// `per_level·(clamped_level - spell_level)`; max_level==0 means only the floor clamp applies.
#[test]
fn effect_amount_scales() {
    assert_eq!(effect_amount(50, 0, 0.0, 1, 1, 0, 0), 50); // flat
    assert_eq!(effect_amount(5, 8, 0.0, 1, 1, 0, 0), 5); // die roll 0
    assert_eq!(effect_amount(5, 8, 0.0, 1, 1, 0, 3), 8); // die roll 3 → 5+3
    assert_eq!(effect_amount(100, 0, 10.0, 5, 1, 60, 0), 140); // +10·4
    assert_eq!(effect_amount(100, 0, 10.0, 80, 1, 60, 0), 690); // level capped at 60 → +10·59
    assert_eq!(effect_amount(100, 0, 10.0, 1, 5, 0, 0), 100); // level below floor → no negative
}

/// INT spell-power ramp: 0 at/below the starter floor (the L2 ~20-INT baseline gate), then floors
/// `(int - base)/per` above it. A high-INT caster ramps up; a creature/starter contributes nothing.
#[test]
fn int_spell_power_gates_at_floor() {
    // At or below the floor → ZERO (the baseline gate: the L2 player's ~20 INT adds nothing).
    assert_eq!(int_spell_power(0), 0);
    assert_eq!(int_spell_power(INT_SPELL_POWER_BASE), 0); // exactly at floor → 0
    assert_eq!(int_spell_power(INT_SPELL_POWER_BASE - 1), 0); // a hair under → still 0 (no underflow)
                                                              // Above the floor: integer-floored ramp at 1 SP per `INT_PER_SPELL_POWER` over the floor.
    assert_eq!(int_spell_power(20 + 5), 1); // exactly one step
    assert_eq!(int_spell_power(20 + 9), 1); // floored (9/5 = 1)
    assert_eq!(int_spell_power(20 + 10), 2);
    assert_eq!(int_spell_power(150), 26); // a L60-ish ~150-INT caster → (150-20)/5 = 26
}

/// Magnitude composition: 0 spell power returns base unchanged (the baseline byte-identical path);
/// positive spell power adds `sp·coeff_pct/100`; a negative base is preserved; negative SP clamps to 0.
#[test]
fn compose_magnitude_scales_with_spell_power() {
    // Baseline: no spell power → base unchanged (Fireball 20 → 20, Lesser Heal 50 → 50).
    assert_eq!(compose_magnitude(20, 0, SPELL_POWER_COEFF_PCT), 20);
    assert_eq!(compose_magnitude(50, 0, SPELL_POWER_COEFF_PCT), 50);
    // Full (100%) coefficient: base + spell power. 20 dmg + 26 SP → 46; 50 heal + 26 SP → 76.
    assert_eq!(compose_magnitude(20, 26, 100), 46);
    assert_eq!(compose_magnitude(50, 26, 100), 76);
    // A partial coefficient scales the spell-power contribution (50% of 26 = 13).
    assert_eq!(compose_magnitude(20, 26, 50), 33);
    // A negative base (a debuff magnitude) is preserved; a 0-coeff adds nothing.
    assert_eq!(compose_magnitude(-5, 26, 100), 21);
    assert_eq!(compose_magnitude(20, 26, 0), 20);
    // Negative spell power clamps to 0 (can't scale a hit/heal BELOW its base).
    assert_eq!(compose_magnitude(20, -100, 100), 20);
}

/// Absorb pool-draw arithmetic (the A_ABSORB shield core): a partial soak leaves pool, fully passes
/// nothing through; a soak-and-deplete-with-overflow spends the pool to 0 and spills the overflow;
/// an empty/negative pool absorbs nothing. The multi-shield drain is the same helper applied in
/// sequence (each shield's `leftover` is the next shield's `incoming`).
#[test]
fn absorb_draw_soaks_then_overflows() {
    // Partial soak: a 100-pool shield eats a 20-hit fully — 80 left, 0 leftover.
    assert_eq!(absorb_draw(100, 20), (20, 80, 0));
    // Exact deplete: a 20-pool eats a 20-hit — pool to 0 (shield breaks), nothing spills.
    assert_eq!(absorb_draw(20, 20), (20, 0, 0));
    // Soak-and-deplete-with-overflow: a 15-pool vs a 20-hit absorbs 15, breaks (0 left), spills 5.
    assert_eq!(absorb_draw(15, 20), (15, 0, 5));
    // A spent/negative pool absorbs nothing; the whole hit passes through.
    assert_eq!(absorb_draw(0, 20), (0, 0, 20));
    assert_eq!(absorb_draw(-5, 20), (0, 0, 20));
    // A zero hit draws nothing from a live pool (pool untouched).
    assert_eq!(absorb_draw(100, 0), (0, 100, 0));

    // Multi-shield: two stacked shields (30 then 50 pool) vs a 60-hit. The first soaks 30 (breaks),
    // spilling 30; the second soaks 30 of its 50 (20 left), spilling 0 → the hit is fully absorbed.
    let (a1, left1, spill1) = absorb_draw(30, 60);
    assert_eq!((a1, left1, spill1), (30, 0, 30)); // first shield spent, 30 spills
    let (a2, left2, spill2) = absorb_draw(50, spill1);
    assert_eq!((a2, left2, spill2), (30, 20, 0)); // second shield: 20 pool left, hit fully soaked
                                                  // Total absorbed across both equals the incoming; final leftover is 0.
    assert_eq!(a1 + a2, 60);
    assert_eq!(spill2, 0);
}

/// Pin the kind wire values: the seed/import data, the dispatch, and the SDK binding all key off
/// these — a drift would silently re-route an effect. KIND_AURA_BIT is the instant-vs-aura split.
#[test]
fn kind_wire_values() {
    assert_eq!(KIND_AURA_BIT, 0x80);
    assert_eq!(E_DAMAGE, 0x01);
    assert_eq!(E_HEAL, 0x02);
    assert_eq!(E_DISPEL, 0x04);
    assert_eq!(A_PERIODIC_DAMAGE, 0x90);
    assert_eq!(A_PERIODIC_HEAL, 0x91);
    assert_eq!(A_ABSORB, 0xA2);
    assert_eq!(A_MOD_COMBAT, 0xA3);
    assert_eq!(A_FLAG, 0xBE);
    // every aura kind has the high bit; every instant kind does not.
    for k in [
        E_DAMAGE, E_HEAL, E_ENERGIZE, E_DISPEL, E_TRIGGER, E_SCRIPTED,
    ] {
        assert_eq!(
            k & KIND_AURA_BIT,
            0,
            "instant kind 0x{k:02x} must not set the aura bit"
        );
    }
    for k in [
        A_PERIODIC_DAMAGE,
        A_PERIODIC_HEAL,
        A_MOD_COMBAT,
        A_FLAG,
        A_CONTROL,
    ] {
        assert_ne!(
            k & KIND_AURA_BIT,
            0,
            "aura kind 0x{k:02x} must set the aura bit"
        );
    }
}

/// Pin EVERY instant `E_*` kind wire value (the deduplicated effect taxonomy the importer maps mangos
/// `Effect` ids onto): a drift here would silently re-route the importer's dispatch and desync it from
/// the seed data / the SDK binding. Loops [`ALL_INSTANT_KINDS`] — taxonomy.rs's single canonical list —
/// rather than a second hand-copied array, so a kind is covered here purely by being IN that slice
/// (exhaustive by construction: the canonical list found this test's own hand-copied predecessor missing
/// E_BLINK/E_PERSISTENT_AREA/E_FISH/E_OPEN_LOCK, and a kind left off `ALL_INSTANT_KINDS` itself now
/// fails CI's `-D warnings` clippy gate via `dead_code` instead of silently passing every test). Also
/// doubles as the passive-filter regression pin: `passive_applies_effect_kind` is exactly `kind &
/// KIND_AURA_BIT != 0`, so asserting every instant kind clears the bit is asserting every instant kind
/// is rejected by the passive-apply path (the old, separately hand-copied
/// `apply_spell_auras_rejects_instant_effects` test — now redundant with this loop and retired).
#[test]
fn instant_kind_wire_values_exhaustive() {
    assert_eq!(
        ALL_INSTANT_KINDS.len(),
        34,
        "an instant kind was added to (or removed from) taxonomy.rs without a matching, deliberate \
         change to ALL_INSTANT_KINDS — bump this count only when the taxonomy really changed"
    );
    for (i, k) in ALL_INSTANT_KINDS.iter().enumerate() {
        assert_eq!(
            k & KIND_AURA_BIT,
            0,
            "instant kind 0x{k:02x} must not set the aura bit"
        );
        assert!(
            !passive_applies_effect_kind(*k),
            "instant kind 0x{k:02x} must not reach the passive-apply path (#90: Consecration's \
             E_PERSISTENT_AREA-only shape must never mint a spurious login/world-change buff)"
        );
        for other in &ALL_INSTANT_KINDS[i + 1..] {
            assert_ne!(
                k, other,
                "instant kinds must be distinct: 0x{k:02x} vs 0x{other:02x}"
            );
        }
    }
}

/// Pin EVERY `A_*` aura kind wire value: same rationale, same [`ALL_AURA_KINDS`]-loop construction, and
/// same-filter double duty as [`instant_kind_wire_values_exhaustive`] above (that test's old sibling
/// hand-copied list was missing A_SPELLMOD_FLAT/A_SPELLMOD_PCT). This is the control: a genuinely
/// passive talent/racial (an `A_*` aura-kind effect) must keep applying through `apply_spell_auras` — an
/// over-eager filter that rejected real auras too would silently disable every passive talent in the
/// game, worse than the cosmetic bug the passive filter fixes (the old, separately hand-copied
/// `apply_spell_auras_still_applies_aura_effects` test — now redundant with this loop and retired).
#[test]
fn aura_kind_wire_values_exhaustive() {
    assert_eq!(
        ALL_AURA_KINDS.len(),
        26,
        "an aura kind was added to (or removed from) taxonomy.rs without a matching, deliberate \
         change to ALL_AURA_KINDS — bump this count only when the taxonomy really changed"
    );
    for (i, k) in ALL_AURA_KINDS.iter().enumerate() {
        assert_ne!(
            k & KIND_AURA_BIT,
            0,
            "aura kind 0x{k:02x} must set the aura bit"
        );
        assert!(
            passive_applies_effect_kind(*k),
            "aura kind 0x{k:02x} must still reach the passive-apply path (#90 control)"
        );
        for other in &ALL_AURA_KINDS[i + 1..] {
            assert_ne!(
                k, other,
                "aura kinds must be distinct: 0x{k:02x} vs 0x{other:02x}"
            );
        }
    }
}

/// Review round 2's actual finding: the (now-retired) `apply_spell_auras_rejects_instant_effects` and
/// `apply_spell_auras_still_applies_aura_effects` tests both called `passive_applies_effect_kind`
/// DIRECTLY — they pinned the predicate's logic but never touched `apply_spell_auras` itself. A
/// reviewer deleted the `.filter(|e| passive_applies_effect_kind(e.kind))` call from
/// `apply_spell_auras`'s effect query (the fix's actual production wiring) and the full test suite
/// stayed green, because nothing exercised the call site. This is a source-scan tripwire on that WIRING:
/// it catches the filter call being DELETED (the shape this defect took, confirmed by mutating it and
/// watching this test go red) or the query losing the `.filter(` step entirely. It does NOT catch the
/// filter being wired to a DIFFERENT, wrong predicate at the same call site (`.filter(|e|
/// some_other_fn(e.kind))` still reads as "filtered" to a scan) — the two kind-exhaustive tests above
/// are what pin the LOGIC; this one only pins that SOME call to `passive_applies_effect_kind` still
/// gates the query. Uses the crate-shared scan primitives (`crate::test_scan`) rather than a local
/// copy — found this file's own copy was the seventh, and it carried the weaker (non-string-
/// literal-aware) trailing-comment stripper the canonical one was hardened against.
#[test]
fn apply_spell_auras_still_calls_the_passive_effect_filter() {
    let body = crate::test_scan::code_of(
        include_str!("cast/resolve.rs"),
        "pub(crate) fn apply_spell_auras(",
    );
    let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains(".filter(|e| passive_applies_effect_kind(e.kind))"),
        "`apply_spell_auras` no longer filters its effect query through `passive_applies_effect_kind` — \
         without that call, every instant-kind effect of a talent/racial/resurrection-sickness passive is \
         handed straight to `aura_apply` again (#90: Consecration's spurious login/world-change buff). \
         Body was:\n{body}"
    );
}

/// Pin EVERY `P_*` param-tag value (`eff_p0_kind` — what `p0` MEANS on a given effect/aura row): the
/// importer stamps these BY NAME and the readers (`resistance_bonus`, `is_immune_to_mechanic`, …) key off
/// them, so a drift here would silently reinterpret a frozen `p0`. All 15 are distinct.
#[test]
fn param_tag_wire_values_exhaustive() {
    assert_eq!(P_NONE, 0);
    assert_eq!(P_STAT_ID, 1);
    assert_eq!(P_SCHOOL_MASK, 2);
    assert_eq!(P_MECHANIC, 3);
    assert_eq!(P_POWER_TYPE, 4);
    assert_eq!(P_COMBAT_FIELD, 5);
    assert_eq!(P_SPEED_KIND, 6);
    assert_eq!(P_FLAG, 7);
    assert_eq!(P_ITEM_ENTRY, 8);
    assert_eq!(P_ENTRY, 9);
    assert_eq!(P_ENCHANT_ID, 10);
    assert_eq!(P_PCT_MAX_POWER, 12);
    assert_eq!(P_GAMEOBJECT_ENTRY, 13);
    assert_eq!(P_DISPLAY_ID, 14);
    assert_eq!(P_RAW, 255);
    let all = [
        P_NONE,
        P_STAT_ID,
        P_SCHOOL_MASK,
        P_MECHANIC,
        P_POWER_TYPE,
        P_COMBAT_FIELD,
        P_SPEED_KIND,
        P_FLAG,
        P_ITEM_ENTRY,
        P_ENTRY,
        P_ENCHANT_ID,
        P_PCT_MAX_POWER,
        P_GAMEOBJECT_ENTRY,
        P_DISPLAY_ID,
        P_RAW,
    ];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b, "param tags must be distinct: {a} vs {b}");
        }
    }
}

/// Pin the CC mechanic wire values: the seed (A_CONTROL eff_p0) and the gates key off these, so a
/// drift would silently un-gate a stun/root. They must be DISTINCT and all NON-ZERO (0 is the
/// reserved no-mechanic sentinel — a frozen A_CONTROL aura with eff_p0 == 0 matches no gate).
#[test]
fn mechanic_wire_values() {
    assert_eq!(M_STUN, 1);
    assert_eq!(M_ROOT, 2);
    assert_eq!(M_FEAR, 3);
    assert_eq!(M_POLY, 4);
    // All distinct + non-zero (0 stays the inert "no mechanic" value).
    let ms = [M_STUN, M_ROOT, M_FEAR, M_POLY];
    for (i, a) in ms.iter().enumerate() {
        assert_ne!(*a, 0, "mechanic must be non-zero (0 = inert sentinel)");
        for b in &ms[i + 1..] {
            assert_ne!(a, b, "mechanics must be distinct");
        }
    }
    // A_CONTROL is an aura kind (the gates read it off a frozen game_aura row).
    assert_eq!(A_CONTROL & KIND_AURA_BIT, KIND_AURA_BIT);
}

/// `rederive_pool` re-applies a stat-bonus delta through the SAME login curve. The two invariants
/// that guarantee baseline safety + correct buff/debuff direction:
///  - bonus 0 ⇒ exact no-op (returns the login max unchanged), for ANY base attribute.
///  - a +bonus adds exactly `curve(base+bonus) - curve(base)`; a -bonus subtracts the symmetric delta.
///
/// Uses the real `stats::hp_from_stamina` so the per-point yield (10 HP/pt above 20) is the live one.
#[test]
fn rederive_pool_is_noop_at_zero_and_curve_exact() {
    use crate::stats::{hp_from_stamina, mana_from_intellect};
    // Baseline (no aura): bonus 0 returns the login max byte-for-byte at every base.
    for base in [0u32, 5, 20, 22, 100] {
        let login = 60; // an arbitrary login max
        assert_eq!(rederive_pool(login, base, 0, hp_from_stamina), login);
    }
    // L1 Warrior: login max 60 (base_health 20 + hp_from_stamina(22)=40). +5 STA → stamina 22→27,
    // hp_from_stamina(27)-hp_from_stamina(22) = (20+70)-(20+20) = 50 more → 110.
    assert_eq!(rederive_pool(60, 22, 5, hp_from_stamina), 110);
    // The marginal HP equals 5 stamina × 10 HP/pt above the 20 breakpoint.
    assert_eq!(rederive_pool(60, 22, 5, hp_from_stamina) - 60, 5 * 10);
    // A -5 STA debuff is the symmetric drop: 22→17 stamina (now below the 20 breakpoint).
    // hp_from_stamina(17)-hp_from_stamina(22) = 17 - 40 = -23 → 60 - 23 = 37.
    assert_eq!(rederive_pool(60, 22, -5, hp_from_stamina), 37);
    // A debuff larger than the base attribute floors the effective at 0 (no underflow): base 3,
    // -10 bonus → effective 0; curve(0)=0, curve(3)=3 → login - 3.
    assert_eq!(rederive_pool(100, 3, -10, hp_from_stamina), 97);
    // Mana twin: 15 mana/int above 20. login 120, base int 20, +10 → mana_from_intellect(30)=170,
    // mana_from_intellect(20)=20 → +150 → 270.
    assert_eq!(rederive_pool(120, 20, 10, mana_from_intellect), 270);
    assert_eq!(rederive_pool(120, 20, 0, mana_from_intellect), 120); // no-op
}

/// The vitals-recompute gate: ONLY an A_MOD_STAT aura touching STA / INT / ALL moves a max pool.
/// A +STR/+AGI/+SPI A_MOD_STAT, or any non-stat aura kind, must NOT trip the recompute (no work →
/// the un-buffed unit keeps its login max). `STAT_ALL` (Mark of the Wild) counts (it includes STA+INT).
#[test]
fn aura_moves_vitals_gate() {
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_STA as i32));
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_INT as i32));
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_ALL as i32));
    // STR/AGI/SPI stat auras don't move HP/mana pools.
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_STR as i32));
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_AGI as i32));
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_SPI as i32));
    // A different aura kind with a STA-looking p0 still must NOT trip (the kind gate is first).
    assert!(!aura_moves_vitals(A_MOD_COMBAT, STAT_STA as i32));
    assert!(!aura_moves_vitals(A_PERIODIC_DAMAGE, STAT_INT as i32));
    assert!(!aura_moves_vitals(A_MOD_RESISTANCE, STAT_ALL as i32));
}

/// The sheet-recompute gate: wider than `aura_moves_vitals` — every `A_MOD_STAT` attribute (including
/// STR/AGI/SPI, which are inert for vitals) trips it, and so does an `A_MOD_COMBAT(COMBAT_ATTACK_POWER)`
/// aura (Battle Shout) or an `A_MOD_COMBAT(COMBAT_CRIT)` aura (a crit-rating buff like the test
/// "Combat Insight") even though neither kind moves a pool.
#[test]
fn aura_moves_sheet_gate() {
    assert!(aura_moves_sheet(A_MOD_STAT, STAT_STR as i32));
    assert!(aura_moves_sheet(A_MOD_STAT, STAT_AGI as i32));
    assert!(aura_moves_sheet(A_MOD_STAT, STAT_STA as i32));
    assert!(aura_moves_sheet(A_MOD_STAT, STAT_INT as i32));
    assert!(aura_moves_sheet(A_MOD_STAT, STAT_SPI as i32));
    assert!(aura_moves_sheet(A_MOD_STAT, STAT_ALL as i32));
    assert!(aura_moves_sheet(A_MOD_COMBAT, COMBAT_ATTACK_POWER as i32));
    assert!(aura_moves_sheet(A_MOD_COMBAT, COMBAT_CRIT as i32));
    // A combat aura on a field the sheet doesn't render (e.g. hit rating) must NOT trip it.
    assert!(!aura_moves_sheet(A_MOD_COMBAT, COMBAT_HIT as i32));
    // A different aura kind with an AP-looking p0 still must NOT trip (the kind gate is first).
    assert!(!aura_moves_sheet(A_PERIODIC_DAMAGE, COMBAT_ATTACK_POWER as i32));
}

/// Cast interrupt-on-damage / CC-break flag decode: `break_auras_on_damage` (which also folds in the
/// cast-interrupt) breaks an aura only when its header's aura_interrupt carries bit 0. An un-flagged CC (the
/// seed default, mask 0) is never broken; a flagged one (and any mask with bit 0 set) is. The runtime
/// interrupt-on-damage / no-op-without-pending-cast behavior is verified server-side (it needs a live ctx).
#[test]
fn break_on_damage_flag_decode() {
    assert!(!breaks_on_damage(0)); // every current seed spell → not broken (baseline-safe)
    assert!(breaks_on_damage(0x1)); // BREAK_ON_DAMAGE bit set → broken on a real hit (Polymorph/Sap)
    assert!(breaks_on_damage(0x1 | 0x4)); // bit 0 set alongside other interrupt bits → still breaks
    assert!(!breaks_on_damage(0x2)); // a different aura_interrupt bit (not BREAK_ON_DAMAGE) → not broken
}

// --- Aura-expiry reap gate (work-item 232) --------------------------------------------------------
// `tick_auras`'s expiry pass now range-scans `game_aura.by_expiry()` (a btree index on `expires_at`) to
// the horizon instead of `.iter()`ing the whole table, then applies `is_due_for_expiry` as the exact same
// combined predicate the old full scan used inline (`a.expires_at <= now && a.eff_kind != A_STEALTH`).
// These pin that predicate's semantics directly (no `ReducerContext` needed — pure fn, per the module's
// no-reducer-harness convention), so a regression in the boundary or the permanent/passive-aura carve-out
// fails a fast unit test instead of only surfacing on a live server.

/// Exactly-at boundary is INCLUSIVE: an aura whose `expires_at == now` reaps on THIS tick, mirroring the
/// periodic pass's own inclusive `>=` final-tick rule (a 3s/1s DoT ticks 3x, not 2x, then expires same
/// tick). One tick early must NOT reap; one tick late must.
#[test]
fn is_due_for_expiry_boundary_is_inclusive() {
    let expires_at = Timestamp::from_micros_since_unix_epoch(1_000_000);
    let one_tick_early = Timestamp::from_micros_since_unix_epoch(999_999);
    let exactly_at = Timestamp::from_micros_since_unix_epoch(1_000_000);
    let one_tick_late = Timestamp::from_micros_since_unix_epoch(1_000_001);
    assert!(!is_due_for_expiry(A_MOD_STAT, expires_at, one_tick_early));
    assert!(is_due_for_expiry(A_MOD_STAT, expires_at, exactly_at));
    assert!(is_due_for_expiry(A_MOD_STAT, expires_at, one_tick_late));
}

/// A_STEALTH is the one KIND carve-out: never timer-reaped no matter how far past its (nominal)
/// `expires_at` `now` has moved — only `break_stealth` removes it. Scoped to the kind, not to
/// `duration_ms == 0` (many non-stealth spells carry a 0 remap and must still reap normally — the
/// sibling assertion below).
#[test]
fn is_due_for_expiry_never_reaps_stealth() {
    let expires_at = Timestamp::from_micros_since_unix_epoch(1_000_000);
    let long_past = Timestamp::from_micros_since_unix_epoch(50_000_000);
    assert!(!is_due_for_expiry(A_STEALTH, expires_at, expires_at));
    assert!(!is_due_for_expiry(A_STEALTH, expires_at, long_past));
    // A non-stealth aura with the identical (already-past) expiry DOES reap — the carve-out is keyed on
    // kind, not on the timestamp being in the past.
    assert!(is_due_for_expiry(A_MOD_STAT, expires_at, long_past));
}

/// A permanent/passive aura (talent grants, toggle auras) freezes `expires_at` at the
/// `duration_ms == u32::MAX` sentinel's `Timestamp(i64::MAX)` (see `aura_apply`, cast.rs) — this must
/// never compare `<=` any realistic `now`, so it is never reaped by the timer regardless of `eff_kind`
/// (a non-stealth passive, e.g. a talent's `A_MOD_STAT`, relies on the timestamp alone here, not a kind
/// carve-out).
#[test]
fn is_due_for_expiry_permanent_sentinel_never_reaped() {
    let permanent = Timestamp::from_micros_since_unix_epoch(i64::MAX);
    let far_future_now = Timestamp::from_micros_since_unix_epoch(9_999_999_999_999);
    assert!(!is_due_for_expiry(A_MOD_STAT, permanent, far_future_now));
    assert!(!is_due_for_expiry(A_STEALTH, permanent, far_future_now));
}

// --- Consumable on-use magnitudes (moved from items::ops, "smalls": these exercise ONLY
// crate::spell functions against hand-copied seed magnitudes, not anything item-specific) --------------

/// Potion heal CLAMPS to max health — the potion routes through `E_HEAL`→`apply_heal`→`healed_value`,
/// so a heal that would overflow max is capped (no over-heal past the pool) and a heal below max lands
/// in full. Drives the SAME `healed_value` the live cast uses with the seeded potion magnitude (100),
/// so the clamp the verify recipe checks (health rises by ~100, never above max) is proven here.
#[test]
fn potion_heal_clamps_to_max_health() {
    const POTION_HEAL: i32 = 80; // game_spell_effect 200440 base_points (vanilla Minor Healing ~70-90, fixed 80)
                                 // From 1 HP with plenty of headroom: full +80 lands.
    assert_eq!(healed_value(1, 500, POTION_HEAL), 81);
    // Near max: the heal is CLAMPED to max (no overheal past the pool).
    assert_eq!(healed_value(450, 500, POTION_HEAL), 500);
    // Already at max: stays at max.
    assert_eq!(healed_value(500, 500, POTION_HEAL), 500);
}

/// CONSUMABLE BREADTH — the MANA POTION (2455→50113) ENERGIZES, clamped to max power. The potion routes
/// through `E_ENERGIZE`→`energized_value` with the seeded magnitude (160 = vanilla Restore Mana midpoint
/// 140-180). Drives the SAME `energized_value` the live cast uses, so the verify recipe's "power rises by
/// 160, never above max_power" is proven here: full restore with headroom, clamp at max, no overflow.
#[test]
fn mana_potion_energizes_and_clamps_to_max_power() {
    const POTION_MANA: i32 = 160; // game_spell_effect 200452 base_points (Restore Mana 140-180, midpoint 160)
                                  // From near-empty with plenty of headroom: the full +160 lands.
    assert_eq!(energized_value(40, 500, POTION_MANA), 200);
    // Near max: the restore is CLAMPED to max_power (no overflow past the pool).
    assert_eq!(energized_value(400, 500, POTION_MANA), 500);
    // Already at max: stays at max.
    assert_eq!(energized_value(500, 500, POTION_MANA), 500);
}

/// CONSUMABLE BREADTH — the WELL-FED buff (Spiced Wolf Meat 2680→50116) is an `A_MOD_STAT` whose +Stamina
/// effect MOVES the max-health pool: `aura_moves_vitals(A_MOD_STAT, STAT_STA)` is true, so applying the
/// buff re-derives max HP (the Cooking payoff is mechanically live). The +Spirit effect is summed by
/// `stat_bonus` but does NOT move vitals (no max-pool consumer for SPI) — staged/inert, like Mark of the
/// Wild's non-STA stats. This guards the kind/p0 the Well-Fed seed must carry to be a real HP buff.
#[test]
fn well_fed_stamina_moves_max_health_spirit_is_inert() {
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_STA as i32)); // +Sta grows max HP on apply
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_SPI as i32)); // +Spi is summed but moves no pool (inert)
}

/// CONSUMABLE BREADTH — the DRINK (water 159/5350→50114) SCHEDULES a periodic energize: an
/// `A_PERIODIC_ENERGIZE` aura that the scheduler folds `energized_value` into `power` each tick. Modeling
/// the 6 ticks (40 mana / 5s × 6 = 240 over 30s), power climbs +40 per tick and CLAMPS at max — exactly
/// the over-time restore the verify recipe polls. Proves the per-tick fold + the clamp without a ctx.
#[test]
fn drink_periodic_energize_climbs_per_tick_and_clamps() {
    const DRINK_PER_TICK: i32 = 41; // game_spell_effect 200456 base_points (Drink 430 base 41, mana / 5s)
                                    // Six ticks from empty with headroom → +246 total (41 × 6), each tick a clean +41.
    let mut power = 0u32;
    for _ in 0..6 {
        power = energized_value(power, 500, DRINK_PER_TICK);
    }
    assert_eq!(power, 246);
    // A tick near the cap clamps to max_power (no overflow on the last tick of a near-full drinker).
    assert_eq!(energized_value(480, 500, DRINK_PER_TICK), 500);
}
