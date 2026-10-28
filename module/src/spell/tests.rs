use super::*;
use spacetimedb::Timestamp;

#[test]
fn can_afford_gate() {
    assert!(can_afford(10, 0));
    assert!(can_afford(10, 10));
    assert!(!can_afford(10, 11));
}

#[test]
fn creature_is_exempt_from_level_and_cost_gates() {
    assert!(cost_gate_blocks(true, 0, 90));
    assert!(!cost_gate_blocks(false, 0, 90));
    assert!(!cost_gate_blocks(true, 90, 90));
    assert!(!cost_gate_blocks(false, 0, 0));
    assert!(level_gate_blocks(true, 20, 9));
    assert!(!level_gate_blocks(false, 20, 9));
    assert!(!level_gate_blocks(true, 0, 1));
    assert!(!level_gate_blocks(true, 1, 1));
}

#[test]
fn spell_crit_is_one_and_a_half() {
    assert_eq!(apply_spell_crit(100, false), 100);
    assert_eq!(apply_spell_crit(100, true), 150);
    assert_eq!(apply_spell_crit(20, true), 30);
    assert_eq!(apply_spell_crit(7, true), 10);
    assert_eq!(apply_spell_crit(0, true), 0);
}

#[test]
fn damage_taken_modifier_scales_and_clamps() {
    assert_eq!(apply_damage_pct(100, 0), 100);
    assert_eq!(apply_damage_pct(100, -75), 25);
    assert_eq!(apply_damage_pct(100, 50), 150);
    assert_eq!(apply_damage_pct(100, -100), 0);
    assert_eq!(apply_damage_pct(100, -250), 0);
    assert_eq!(apply_damage_pct(7, -75), 1);
}

#[test]
fn stance_allows_gate() {
    assert!(stance_allows(0, STANCE_BATTLE));
    assert!(stance_allows(0, STANCE_DEFENSIVE));
    assert!(stance_allows(0, STANCE_BERSERKER));
    assert!(stance_allows(0x01, STANCE_BATTLE));
    assert!(!stance_allows(0x01, STANCE_DEFENSIVE));
    assert!(!stance_allows(0x01, STANCE_BERSERKER));
    assert!(stance_allows(0x03, STANCE_BATTLE));
    assert!(stance_allows(0x03, STANCE_DEFENSIVE));
    assert!(!stance_allows(0x03, STANCE_BERSERKER));
    assert!(stance_allows(0x05, STANCE_BATTLE));
    assert!(!stance_allows(0x05, STANCE_DEFENSIVE));
    assert!(stance_allows(0x05, STANCE_BERSERKER));
    assert!(!stance_allows(0x01, 9));
    assert!(stance_allows(0x28, STANCE_BEAR));
    assert!(stance_allows(0x28, STANCE_DIRE_BEAR));
    assert!(!stance_allows(0x28, STANCE_CAT));
    assert!(!stance_allows(0x28, STANCE_BATTLE));
    assert!(stance_allows(0x10, STANCE_CAT));
    assert!(!stance_allows(0x10, STANCE_BEAR));
    assert!(!stance_allows(0x10, STANCE_BATTLE));
    assert!(stance_allows(0, STANCE_BEAR));
}

#[test]
fn client_form_for_stance_matches_the_convention() {
    assert_eq!(client_form_for_stance(STANCE_BATTLE), 17);
    assert_eq!(client_form_for_stance(STANCE_DEFENSIVE), 18);
    assert_eq!(client_form_for_stance(STANCE_BERSERKER), 19);
    assert_eq!(client_form_for_stance(STANCE_BEAR), 5);
    assert_eq!(client_form_for_stance(STANCE_CAT), 1);
    assert_eq!(client_form_for_stance(STANCE_DIRE_BEAR), 8);
    assert_eq!(client_form_for_stance(6), 23);
    assert_eq!(client_form_for_stance(7), 24);
}

#[test]
fn stance_switch_clears_power_only_on_real_switch_for_rage() {
    use lyracore_shared::packing::power_type;
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
    assert!(!stance_switch_clears_power(
        STANCE_BATTLE,
        STANCE_BATTLE,
        power_type::RAGE
    ));
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

#[test]
fn form_recast_toggles_off_only_for_active_druid_forms() {
    assert!(form_recast_toggles_off(STANCE_BEAR, STANCE_BEAR));
    assert!(form_recast_toggles_off(STANCE_CAT, STANCE_CAT));
    assert!(form_recast_toggles_off(STANCE_DIRE_BEAR, STANCE_DIRE_BEAR));
    assert!(!form_recast_toggles_off(STANCE_BATTLE, STANCE_BATTLE));
    assert!(!form_recast_toggles_off(STANCE_DEFENSIVE, STANCE_DEFENSIVE));
    assert!(!form_recast_toggles_off(STANCE_BERSERKER, STANCE_BERSERKER));
    assert!(!form_recast_toggles_off(STANCE_BEAR, STANCE_CAT));
    assert!(!form_recast_toggles_off(0, STANCE_BEAR));
}

#[test]
fn stance_folds_only_defensive_contributes() {
    assert_eq!(stance_damage_taken_pct(STANCE_BATTLE), 0);
    assert_eq!(stance_damage_taken_pct(STANCE_DEFENSIVE), -10);
    assert_eq!(stance_damage_taken_pct(STANCE_BERSERKER), 0);
    assert_eq!(stance_damage_done_pct(STANCE_BATTLE), 0);
    assert_eq!(stance_damage_done_pct(STANCE_DEFENSIVE), -10);
    assert_eq!(stance_damage_done_pct(STANCE_BERSERKER), 0);
    assert_eq!(stance_threat_pct(STANCE_BATTLE), 0);
    assert_eq!(stance_threat_pct(STANCE_DEFENSIVE), 30);
    assert_eq!(stance_threat_pct(STANCE_BERSERKER), 0);
    assert_eq!(
        apply_damage_pct(100, stance_damage_taken_pct(STANCE_DEFENSIVE)),
        90
    );
    assert_eq!(
        apply_damage_pct(100, stance_damage_done_pct(STANCE_DEFENSIVE)),
        90
    );
    assert_eq!(
        apply_damage_pct(100, stance_damage_taken_pct(STANCE_BATTLE)),
        100
    );
}

#[test]
fn spell_crit_chance_folds_aura_and_clamps() {
    assert_eq!(spell_crit_bp_with_bonus(0), 0);
    assert_eq!(spell_crit_bp_with_bonus(1000), 1000);
    assert_eq!(spell_crit_bp_with_bonus(-5000), 0);
    assert_eq!(spell_crit_bp_with_bonus(100_000), 10_000);
}

#[test]
fn spell_crit_derives_from_intellect_and_level() {
    assert_eq!(intellect_crit_bp(23, 2), 328);
    assert_eq!(intellect_crit_bp(60, 2), 857);
    assert_eq!(intellect_crit_bp(23, 10), 153);
    assert_eq!(intellect_crit_bp(0, 2), 0);
    assert_eq!(intellect_crit_bp(150, 60), 230);
    assert!(intellect_crit_bp(60, 2) > intellect_crit_bp(23, 2));
    assert!(intellect_crit_bp(23, 10) < intellect_crit_bp(23, 2));
    assert!(intellect_crit_bp(23, 2) < 1000);
}

#[test]
fn spell_miss_follows_the_level_hit_curve() {
    assert_eq!(spell_miss_chance_bp(10, 10), 400);
    assert_eq!(spell_miss_chance_bp(10, 5), 400);
    assert_eq!(spell_miss_chance_bp(10, 11), 500);
    assert_eq!(spell_miss_chance_bp(10, 12), 600);
    assert_eq!(spell_miss_chance_bp(10, 13), 1700);
    assert!(spell_miss_chance_bp(10, 14) > spell_miss_chance_bp(10, 13));
    assert_eq!(spell_miss_chance_bp(1, 60), 9900);
}

#[test]
fn damaged_value_floors_at_one() {
    assert_eq!(damaged_value(42, 20), 22);
    assert_eq!(damaged_value(10, 50), 1);
    assert_eq!(damaged_value(42, -5), 42);
    assert_eq!(damaged_value(1, 100), 1);
}

#[test]
fn healed_value_clamps() {
    assert_eq!(healed_value(5, 42, 20), 25);
    assert_eq!(healed_value(30, 42, 20), 42);
    assert_eq!(healed_value(10, 42, -5), 10);
    assert_eq!(healed_value(0, 42, 50), 42);
}

#[test]
fn energized_value_clamps() {
    assert_eq!(energized_value(5, 100, 20), 25);
    assert_eq!(energized_value(90, 100, 20), 100);
    assert_eq!(energized_value(40, 100, -5), 40);
    assert_eq!(energized_value(0, 100, 100), 100);
}

#[test]
fn distance_3d_euclidean() {
    assert_eq!(distance_3d(0.0, 0.0, 0.0, 3.0, 4.0, 0.0), 5.0);
    assert_eq!(distance_3d(1.0, 2.0, 3.0, 1.0, 2.0, 3.0), 0.0);
    assert_eq!(distance_3d(0.0, 0.0, 0.0, 0.0, 0.0, 7.0), 7.0);
    assert_eq!(distance_3d(3.0, 4.0, 0.0, 0.0, 0.0, 0.0), 5.0);
}

#[test]
fn is_behind_rear_hemisphere() {
    use std::f32::consts::{FRAC_PI_2, PI};
    assert!(is_behind(-1.0, 0.0, 0.0, 0.0, 0.0));
    assert!(!is_behind(1.0, 0.0, 0.0, 0.0, 0.0));
    assert!(is_behind(0.0, 1.0, 0.0, 0.0, 0.0));
    assert!(is_behind(0.0, -1.0, 0.0, 0.0, 0.0));
    assert!(!is_behind(1.0, 0.5, 0.0, 0.0, 0.0));
    assert!(!is_behind(1.0, -0.5, 0.0, 0.0, 0.0));
    assert!(is_behind(1.0, 0.0, 0.0, 0.0, PI));
    assert!(!is_behind(-1.0, 0.0, 0.0, 0.0, PI));
    assert!(is_behind(1.0, 0.0, 0.0, 0.0, -PI));
    assert!(is_behind(0.0, -1.0, 0.0, 0.0, FRAC_PI_2));
    assert!(!is_behind(0.0, 1.0, 0.0, 0.0, FRAC_PI_2));
    assert!(!is_behind(0.0, 0.0, 0.0, 0.0, 0.0));
}

#[test]
fn dispel_category_match() {
    assert!(dispel_category_matches(0, 0));
    assert!(dispel_category_matches(0, 1));
    assert!(dispel_category_matches(0, 4));
    assert!(dispel_category_matches(1, 1));
    assert!(!dispel_category_matches(1, 2));
    assert!(!dispel_category_matches(1, 0));
    assert!(dispel_category_matches(2, 2));
    assert!(!dispel_category_matches(2, 1));
}

#[test]
fn is_on_cooldown_strict() {
    assert!(is_on_cooldown(1000, 2000));
    assert!(!is_on_cooldown(2000, 2000));
    assert!(!is_on_cooldown(3000, 2000));
}

#[test]
fn is_ready_at_is_inverse_of_on_cooldown() {
    assert!(!is_ready_at(1000, 2000));
    assert!(is_ready_at(2000, 2000));
    assert!(is_ready_at(3000, 2000));
    for (now, ready) in [(0, 0), (1000, 2000), (2000, 2000), (3000, 2000), (5, 4)] {
        assert_ne!(is_ready_at(now, ready), is_on_cooldown(now, ready));
    }
}

#[test]
fn school_locked_is_active_until_the_boundary() {
    assert!(school_locked(1000, 2000));
    assert!(!school_locked(2000, 2000));
    assert!(!school_locked(3000, 2000));
    assert!(!school_locked(0, 0));
    assert!(!school_locked(5000, 0));
}

#[test]
fn effect_amount_scales() {
    assert_eq!(effect_amount(50, 0, 0.0, 1, 1, 0, 0), 50);
    assert_eq!(effect_amount(5, 8, 0.0, 1, 1, 0, 0), 5);
    assert_eq!(effect_amount(5, 8, 0.0, 1, 1, 0, 3), 8);
    assert_eq!(effect_amount(100, 0, 10.0, 5, 1, 60, 0), 140);
    assert_eq!(effect_amount(100, 0, 10.0, 80, 1, 60, 0), 690);
    assert_eq!(effect_amount(100, 0, 10.0, 1, 5, 0, 0), 100);
}

#[test]
fn int_spell_power_gates_at_floor() {
    assert_eq!(int_spell_power(0), 0);
    assert_eq!(int_spell_power(INT_SPELL_POWER_BASE), 0);
    assert_eq!(int_spell_power(INT_SPELL_POWER_BASE - 1), 0);
    assert_eq!(int_spell_power(20 + 5), 1);
    assert_eq!(int_spell_power(20 + 9), 1);
    assert_eq!(int_spell_power(20 + 10), 2);
    assert_eq!(int_spell_power(150), 26);
}

#[test]
fn compose_magnitude_scales_with_spell_power() {
    assert_eq!(compose_magnitude(20, 0, SPELL_POWER_COEFF_PCT), 20);
    assert_eq!(compose_magnitude(50, 0, SPELL_POWER_COEFF_PCT), 50);
    assert_eq!(compose_magnitude(20, 26, 100), 46);
    assert_eq!(compose_magnitude(50, 26, 100), 76);
    assert_eq!(compose_magnitude(20, 26, 50), 33);
    assert_eq!(compose_magnitude(-5, 26, 100), 21);
    assert_eq!(compose_magnitude(20, 26, 0), 20);
    assert_eq!(compose_magnitude(20, -100, 100), 20);
}

#[test]
fn absorb_draw_soaks_then_overflows() {
    assert_eq!(absorb_draw(100, 20), (20, 80, 0));
    assert_eq!(absorb_draw(20, 20), (20, 0, 0));
    assert_eq!(absorb_draw(15, 20), (15, 0, 5));
    assert_eq!(absorb_draw(0, 20), (0, 0, 20));
    assert_eq!(absorb_draw(-5, 20), (0, 0, 20));
    assert_eq!(absorb_draw(100, 0), (0, 100, 0));

    let (a1, left1, spill1) = absorb_draw(30, 60);
    assert_eq!((a1, left1, spill1), (30, 0, 30));
    let (a2, left2, spill2) = absorb_draw(50, spill1);
    assert_eq!((a2, left2, spill2), (30, 20, 0));
    assert_eq!(a1 + a2, 60);
    assert_eq!(spill2, 0);
}

#[test]
fn passive_application_uses_only_aura_effects() {
    for kind in [A_MOD_STAT, A_STEALTH, A_PERIODIC_TRIGGER] {
        assert!(passive_applies_effect_kind(kind), "{kind:#x}");
    }
    for kind in [E_DAMAGE, E_HEAL, E_TRIGGER, E_SELF_RESURRECT] {
        assert!(!passive_applies_effect_kind(kind), "{kind:#x}");
    }
}

#[test]
fn rederive_pool_is_noop_at_zero_and_curve_exact() {
    use crate::stats::{hp_from_stamina, mana_from_intellect};
    for base in [0u32, 5, 20, 22, 100] {
        let login = 60;
        assert_eq!(rederive_pool(login, base, 0, hp_from_stamina), login);
    }
    assert_eq!(rederive_pool(60, 22, 5, hp_from_stamina), 110);
    assert_eq!(rederive_pool(60, 22, 5, hp_from_stamina) - 60, 5 * 10);
    assert_eq!(rederive_pool(60, 22, -5, hp_from_stamina), 37);
    assert_eq!(rederive_pool(100, 3, -10, hp_from_stamina), 97);
    assert_eq!(rederive_pool(120, 20, 10, mana_from_intellect), 270);
    assert_eq!(rederive_pool(120, 20, 0, mana_from_intellect), 120);
}

#[test]
fn direct_pool_bonus_clamps_at_imported_integer_boundaries() {
    assert_eq!(add_flat_pool_bonus(100, 25), 125);
    assert_eq!(add_flat_pool_bonus(100, -150), 0);
    assert_eq!(add_flat_pool_bonus(u32::MAX, i32::MAX), u32::MAX);
}

#[test]
fn aura_moves_vitals_gate() {
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_STA as i32));
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_INT as i32));
    assert!(aura_moves_vitals(A_MOD_STAT, STAT_ALL as i32));
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_STR as i32));
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_AGI as i32));
    assert!(!aura_moves_vitals(A_MOD_STAT, STAT_SPI as i32));
    assert!(!aura_moves_vitals(A_MOD_COMBAT, STAT_STA as i32));
    assert!(!aura_moves_vitals(A_PERIODIC_DAMAGE, STAT_INT as i32));
    assert!(!aura_moves_vitals(A_MOD_RESISTANCE, STAT_ALL as i32));
}

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
    assert!(!aura_moves_sheet(A_MOD_COMBAT, COMBAT_HIT as i32));
    assert!(!aura_moves_sheet(
        A_PERIODIC_DAMAGE,
        COMBAT_ATTACK_POWER as i32
    ));
}

#[test]
fn break_on_damage_flag_decode() {
    assert!(!breaks_on_damage(0));
    assert!(breaks_on_damage(0x1));
    assert!(breaks_on_damage(0x1 | 0x4));
    assert!(!breaks_on_damage(0x2));
}

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

#[test]
fn is_due_for_expiry_never_reaps_stealth() {
    let expires_at = Timestamp::from_micros_since_unix_epoch(1_000_000);
    let long_past = Timestamp::from_micros_since_unix_epoch(50_000_000);
    assert!(!is_due_for_expiry(A_STEALTH, expires_at, expires_at));
    assert!(!is_due_for_expiry(A_STEALTH, expires_at, long_past));
    assert!(is_due_for_expiry(A_MOD_STAT, expires_at, long_past));
}

#[test]
fn is_due_for_expiry_permanent_sentinel_never_reaped() {
    let permanent = Timestamp::from_micros_since_unix_epoch(i64::MAX);
    let far_future_now = Timestamp::from_micros_since_unix_epoch(9_999_999_999_999);
    assert!(!is_due_for_expiry(A_MOD_STAT, permanent, far_future_now));
    assert!(!is_due_for_expiry(A_STEALTH, permanent, far_future_now));
}
