use super::*;

fn magic_miss_bp(caster_level: u32, target_level: u32) -> u32 {
    super::rolls::magic_miss_bp(caster_level, target_level, false, 0.0)
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
fn spell_miss_follows_the_level_hit_curve() {
    assert_eq!(magic_miss_bp(10, 10), 400);
    assert_eq!(magic_miss_bp(10, 5), 100);
    assert_eq!(magic_miss_bp(10, 11), 500);
    assert_eq!(magic_miss_bp(10, 12), 600);
    assert_eq!(magic_miss_bp(10, 13), 1700);
    assert!(magic_miss_bp(10, 14) > magic_miss_bp(10, 13));
    assert_eq!(magic_miss_bp(1, 60), 7800);
}

#[test]
fn magic_hit_uses_signed_levels_and_caps_base_full_resistance() {
    assert_eq!(magic_miss_bp(60, 1), 100);
    assert_eq!(magic_miss_bp(1, 60), 7800);
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
fn break_on_damage_flag_decode() {
    assert!(!breaks_on_damage(0));
    assert!(breaks_on_damage(0x1));
    assert!(breaks_on_damage(0x1 | 0x4));
    assert!(!breaks_on_damage(0x2));
}
