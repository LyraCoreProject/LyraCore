use super::*;
use crate::Spell;

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

fn spell_with(attributes: u32, attributes_ex_2: u32) -> Spell {
    Spell {
        attributes,
        attributes_ex_2,
        ..Spell::blank(1, "Fixture")
    }
}

// The Form Set a DBC `Stances` and `StancesNot` pair imports as, seen from `form`.
fn requirement(required: u32, excluded: u32, form: Option<u32>) -> FormRequirement {
    let bit = |mask: u32| form.is_some_and(|form| mask & (1 << (form - 1)) != 0);
    FormRequirement {
        requires_any: required & !excluded != 0,
        requires_this: bit(required) && !bit(excluded),
        excludes_this: bit(excluded),
    }
}

fn form_gated(
    required: u32,
    excluded: u32,
    attributes: u32,
    attributes_ex_2: u32,
) -> impl Fn(Option<ShapeshiftState>) -> Option<FormRefusal> {
    let spell = spell_with(attributes, attributes_ex_2);
    move |form| {
        spell.form_refusal(
            requirement(required, excluded, form.map(|form| form.form_id)),
            form,
        )
    }
}

fn in_form(form_id: u32, acts_as_shapeshifted: bool) -> Option<ShapeshiftState> {
    Some(ShapeshiftState {
        form_id,
        acts_as_shapeshifted,
    })
}

#[test]
fn a_stance_ability_needs_one_of_its_forms() {
    let charge = form_gated(0x0001_0000, 0, 0x0005_0010, 0);
    assert_eq!(charge(in_form(17, false)), None);
    assert_eq!(
        charge(in_form(18, false)),
        Some(FormRefusal::OnlyShapeshift)
    );
    assert_eq!(charge(None), Some(FormRefusal::OnlyShapeshift));
    let excluded = form_gated(0, 0x0001_0000, 0, 0);
    assert_eq!(
        excluded(in_form(17, false)),
        Some(FormRefusal::NotShapeshift)
    );
    assert_eq!(excluded(in_form(18, false)), None);
}

#[test]
fn a_passive_allowed_unshifted_is_held_in_any_form_its_set_does_not_name() {
    let furor = form_gated(0x91, 0, 0x1D0, 0x80000);
    for form in [None, in_form(5, true), in_form(3, true), in_form(17, false)] {
        assert_eq!(furor(form), None, "{form:?}");
    }
    let active = form_gated(0x91, 0, 0, 0x80000);
    assert_eq!(
        active(in_form(3, true)),
        Some(FormRefusal::OnlyShapeshift),
        "a cast spell still needs another of its Forms"
    );
}

#[test]
fn a_form_above_the_dbc_mask_can_gate_a_spell() {
    let spell = spell_with(0, 0);
    let in_form_40 = in_form(40, false);
    let only_40 = FormRequirement {
        requires_any: true,
        requires_this: true,
        excludes_this: false,
    };
    assert_eq!(spell.form_refusal(only_40, in_form_40), None);
    assert_eq!(
        spell.form_refusal(
            FormRequirement {
                requires_this: false,
                ..only_40
            },
            in_form(17, false)
        ),
        Some(FormRefusal::OnlyShapeshift)
    );
}

#[test]
fn a_shapeshift_refuses_caster_spells_and_other_forms_abilities() {
    let caster_only = form_gated(0, 0, 0x0001_0000, 0);
    assert_eq!(
        caster_only(in_form(5, true)),
        Some(FormRefusal::NotShapeshift)
    );
    assert_eq!(caster_only(in_form(17, false)), None);
    assert_eq!(caster_only(None), None);
    let maul = form_gated(0x90, 0, 0, 0);
    assert_eq!(maul(in_form(8, true)), None);
    assert_eq!(maul(in_form(1, true)), Some(FormRefusal::OnlyShapeshift));
    let unshifted_too = form_gated(0x1, 0, 0, 0x0008_0000);
    assert_eq!(unshifted_too(None), None);
    assert_eq!(unshifted_too(in_form(17, false)), None);
    assert_eq!(
        unshifted_too(in_form(5, true)),
        Some(FormRefusal::OnlyShapeshift)
    );
}

#[test]
fn only_a_form_bound_spell_ends_with_its_form() {
    let ends = |required, attributes, attributes_ex_2| {
        spell_with(attributes, attributes_ex_2)
            .removed_on_shape_lost(requirement(required, 0, None))
    };
    assert!(ends(0x0001_0000, 0x0004_0010, 0));
    assert!(!ends(0x0001_0000, 0x0005_0010, 0));
    assert!(!ends(0x1, 0, 0x0008_0000));
    assert!(!ends(0, 0, 0));
}

#[test]
fn a_known_passive_comes_with_the_forms_its_form_set_names() {
    let defiance = spell_with(0x1D0, 0);
    let in_defensive = requirement(0x0002_0000, 0, Some(18));
    assert!(defiance.binds_to_forms(in_defensive));
    assert!(defiance.applies_on_form_entry(in_defensive));
    assert!(!defiance.applies_on_form_entry(requirement(0x0002_0000, 0, Some(17))));
    assert!(!spell_with(0x10, 0).applies_on_form_entry(in_defensive));
    assert!(!spell_with(0x1D0, 0x0008_0000).binds_to_forms(in_defensive));
    assert!(!defiance.binds_to_forms(requirement(0, 0, Some(18))));
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
fn damaged_value_subtracts_damage_down_to_zero() {
    assert_eq!(damaged_value(42, 20), 22);
    assert_eq!(damaged_value(10, 50), 0);
    assert_eq!(damaged_value(42, -5), 42);
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
fn rederive_pool_is_noop_for_an_unchanged_stat_and_follows_the_curve() {
    use crate::stats::{hp_from_stamina, mana_from_intellect};
    for base in [0u32, 5, 20, 22, 100] {
        let login = 60;
        assert_eq!(rederive_pool(login, base, base, hp_from_stamina), login);
    }
    assert_eq!(rederive_pool(60, 22, 27, hp_from_stamina), 110);
    assert_eq!(rederive_pool(60, 22, 27, hp_from_stamina) - 60, 5 * 10);
    assert_eq!(rederive_pool(60, 22, 17, hp_from_stamina), 37);
    assert_eq!(rederive_pool(100, 3, 0, hp_from_stamina), 97);
    assert_eq!(rederive_pool(120, 20, 30, mana_from_intellect), 270);
    assert_eq!(rederive_pool(120, 20, 20, mana_from_intellect), 120);
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
