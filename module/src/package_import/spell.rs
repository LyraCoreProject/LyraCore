use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_form_id, is_package_spell_id, FieldValue, Operation, PrimaryKey,
    Table as ClaimTable, TracedRow, SPELL_FAMILY,
};
use lyracore_shared::form::PowerOnEntry;
use lyracore_shared::spell::taxonomy::{A_SHAPESHIFT, SPELL_ATTR_PASSIVE};

use crate::form::tables::FormSpell;
use crate::form::{game_form, game_form_spell, Form};
use crate::spell::tables::{game_spell_form, SpellForm};
use crate::{game_spell, game_spell_effect, Spell, SpellEffect};

use super::{
    as_bool, as_f32, as_i32, as_str, as_u16, as_u32, as_u64, as_u8, check_insert_is_whole,
    missing_reference, no_such_column, ClaimOps,
};

pub(super) const OPS: ClaimOps = ClaimOps {
    name: SPELL_FAMILY,
    in_package_range,
    row_is_present,
    check_references: None,
    clear_package_range,
    write_row,
    after_write: Some(check_form_rows),
};

/// Refuses Form rows the written tables cannot honour; docs/package-api.md "Form" lists the rules.
fn check_form_rows(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    let spells = ctx.db.game_spell();
    let forms = ctx.db.game_form();
    let form_spells: Vec<(u32, u32)> = forms
        .iter()
        .map(|form| (form.spell_id, form.form_id))
        .collect();
    let shapeshifts = |spell_id: u32| {
        ctx.db
            .game_spell_effect()
            .by_spell()
            .filter(spell_id)
            .filter(|effect| effect.kind == A_SHAPESHIFT)
    };
    let enters_form = |spell_id: u32, form_id: u32| {
        shapeshifts(spell_id).any(|effect| u32::try_from(effect.p0) == Ok(form_id))
    };
    for row in rows {
        let (form_id, spell_id) = match row.key() {
            PrimaryKey::Form { form_id } => {
                let form = forms
                    .form_id()
                    .find(form_id)
                    .ok_or_else(|| format!("`game_form` row {} vanished mid-apply", row.key()))?;
                if PowerOnEntry::try_from(form.power_on_entry).is_err() {
                    return Err(format!(
                        "`game_form` row {} claims unknown power rule {}",
                        row.key(),
                        form.power_on_entry
                    ));
                }
                if !enters_form(form.spell_id, form_id) {
                    return Err(format!(
                        "`game_form` row {} names spell {}, which does not enter the Form",
                        row.key(),
                        form.spell_id
                    ));
                }
                continue;
            }
            PrimaryKey::FormSpell { form_id, spell_id } => {
                let spell = spells
                    .spell_id()
                    .find(spell_id)
                    .ok_or_else(|| missing_reference(row, "spell", spell_id))?;
                if spell.attributes & SPELL_ATTR_PASSIVE == 0
                    || shapeshifts(spell_id).next().is_some()
                {
                    return Err(format!(
                        "`game_form_spell` row {} names spell {spell_id}, which is not a passive \
                         spell or which shapeshifts",
                        row.key()
                    ));
                }
                (form_id, spell_id)
            }
            PrimaryKey::SpellForm { spell_id, form_id } => (form_id, spell_id),
            PrimaryKey::SpellEffect { spell_id, .. } => {
                let effect = ctx.db.game_spell_effect().id().find(row.row_id());
                if let Some(effect) = effect.filter(|effect| effect.kind == A_SHAPESHIFT) {
                    let named = u32::try_from(effect.p0).unwrap_or(0);
                    if forms.form_id().find(named).is_none() {
                        return Err(format!(
                            "`game_spell_effect` row {} shapeshifts into Form {}, which has no \
                             `game_form` row",
                            row.key(),
                            effect.p0
                        ));
                    }
                }
                let broken = form_spells.iter().find(|&&(form_spell, form_id)| {
                    form_spell == spell_id && !enters_form(spell_id, form_id)
                });
                if let Some(&(_, form_id)) = broken {
                    return Err(format!(
                        "`game_spell_effect` row {} leaves spell {spell_id} no longer entering Form {}",
                        row.key(),
                        form_id
                    ));
                }
                continue;
            }
            _ => continue,
        };
        if forms.form_id().find(form_id).is_none() {
            return Err(missing_reference(row, "Form", form_id));
        }
        if spells.spell_id().find(spell_id).is_none() {
            return Err(missing_reference(row, "spell", spell_id));
        }
    }
    Ok(())
}

fn row_is_present(ctx: &ReducerContext, row: &TracedRow) -> bool {
    match row.table() {
        ClaimTable::Spell => ctx
            .db
            .game_spell()
            .spell_id()
            .find(row.key().spell_id())
            .is_some(),
        ClaimTable::SpellEffect => ctx.db.game_spell_effect().id().find(row.row_id()).is_some(),
        ClaimTable::Form => ctx
            .db
            .game_form()
            .form_id()
            .find(row.key().form_id())
            .is_some(),
        ClaimTable::FormSpell => ctx.db.game_form_spell().id().find(row.row_id()).is_some(),
        ClaimTable::SpellForm => ctx.db.game_spell_form().id().find(row.row_id()).is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-spell row before the spell family's dispatch \
             runs, found {other}"
        ),
    }
}

fn in_package_range(row: &TracedRow) -> bool {
    match row.table() {
        ClaimTable::Form | ClaimTable::FormSpell | ClaimTable::SpellForm => {
            is_package_form_id(row.key().form_id())
        }
        _ => is_package_spell_id(row.key().spell_id()),
    }
}

fn clear_package_range(ctx: &ReducerContext) {
    let effects = ctx.db.game_spell_effect();
    let stale_effects: Vec<u64> = effects
        .iter()
        .filter(|effect| is_package_spell_id(effect.spell_id))
        .map(|effect| effect.id)
        .collect();
    for id in stale_effects {
        effects.id().delete(id);
    }

    let spells = ctx.db.game_spell();
    let stale_spells: Vec<u32> = spells
        .iter()
        .filter(|spell| is_package_spell_id(spell.spell_id))
        .map(|spell| spell.spell_id)
        .collect();
    for spell_id in stale_spells {
        spells.spell_id().delete(spell_id);
        crate::spell::delete_form_set(ctx, spell_id);
    }

    let form_sets = ctx.db.game_spell_form();
    let stale_form_sets: Vec<u64> = form_sets
        .iter()
        .filter(|row| is_package_form_id(row.form_id))
        .map(|row| row.id)
        .collect();
    for id in stale_form_sets {
        form_sets.id().delete(id);
    }

    let passives = ctx.db.game_form_spell();
    let stale_passives: Vec<u64> = passives
        .iter()
        .filter(|row| is_package_form_id(row.form_id))
        .map(|row| row.id)
        .collect();
    for id in stale_passives {
        passives.id().delete(id);
    }

    let forms = ctx.db.game_form();
    let stale_forms: Vec<u32> = forms
        .iter()
        .filter(|form| is_package_form_id(form.form_id))
        .map(|form| form.form_id)
        .collect();
    for form_id in stale_forms {
        forms.form_id().delete(form_id);
    }
}

fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::Spell, Operation::Insert) => {
            let spell = ctx
                .db
                .game_spell()
                .try_insert(built_spell(row)?)
                .map_err(|e| format!("`game_spell` row {} did not insert: {e}", row.key()))?;
            write_form_set(ctx, &spell);
        }
        (ClaimTable::Spell, Operation::Update) => {
            let spells = ctx.db.game_spell();
            let mut spell = spells
                .spell_id()
                .find(row.key().spell_id())
                .ok_or_else(|| format!("`game_spell` row {} vanished mid-apply", row.key()))?;
            for (field, claimed) in row.fields() {
                apply_spell_field(&mut spell, field, &claimed.value)?;
            }
            let spell = spells.spell_id().update(spell);
            write_form_set(ctx, &spell);
        }
        (ClaimTable::SpellEffect, Operation::Insert) => {
            ctx.db
                .game_spell_effect()
                .try_insert(built_spell_effect(row)?)
                .map_err(|e| {
                    format!("`game_spell_effect` row {} did not insert: {e}", row.key())
                })?;
        }
        (ClaimTable::SpellEffect, Operation::Update) => {
            let effects = ctx.db.game_spell_effect();
            let mut effect = effects.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_spell_effect` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_effect_field(&mut effect, field, &claimed.value)?;
            }
            effects.id().update(effect);
        }
        (ClaimTable::Form, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let mut form = blank_form(row.key().form_id());
            for (field, claimed) in row.fields() {
                apply_form_field(&mut form, field, &claimed.value)?;
            }
            ctx.db
                .game_form()
                .try_insert(form)
                .map_err(|e| format!("`game_form` row {} did not insert: {e}", row.key()))?;
        }
        (ClaimTable::Form, Operation::Update) => {
            let forms = ctx.db.game_form();
            let mut form = forms
                .form_id()
                .find(row.key().form_id())
                .ok_or_else(|| format!("`game_form` row {} vanished mid-apply", row.key()))?;
            for (field, claimed) in row.fields() {
                apply_form_field(&mut form, field, &claimed.value)?;
            }
            forms.form_id().update(form);
        }
        (ClaimTable::FormSpell, Operation::Insert) => {
            let PrimaryKey::FormSpell { form_id, spell_id } = row.key() else {
                unreachable!("a `game_form_spell` row carries a `game_form_spell` key")
            };
            ctx.db
                .game_form_spell()
                .try_insert(FormSpell {
                    id: row.row_id(),
                    form_id,
                    spell_id,
                })
                .map_err(|e| format!("`game_form_spell` row {} did not insert: {e}", row.key()))?;
        }
        (ClaimTable::FormSpell, Operation::Update) => unreachable!(
            "`Claim::new` refuses an update that claims no column, and `game_form_spell` has none"
        ),
        (ClaimTable::SpellForm, Operation::Insert) => {
            check_insert_is_whole(row)?;
            let PrimaryKey::SpellForm { spell_id, form_id } = row.key() else {
                unreachable!("a `game_spell_form` row carries a `game_spell_form` key")
            };
            let mut form_set = SpellForm {
                id: row.row_id(),
                spell_id,
                form_id,
                excluded: false,
            };
            for (field, claimed) in row.fields() {
                apply_spell_form_field(&mut form_set, field, &claimed.value)?;
            }
            ctx.db
                .game_spell_form()
                .try_insert(form_set)
                .map_err(|e| format!("`game_spell_form` row {} did not insert: {e}", row.key()))?;
        }
        (ClaimTable::SpellForm, Operation::Update) => {
            let form_sets = ctx.db.game_spell_form();
            let mut form_set = form_sets
                .id()
                .find(row.row_id())
                .ok_or_else(|| format!("`game_spell_form` row {} vanished mid-apply", row.key()))?;
            for (field, claimed) in row.fields() {
                apply_spell_form_field(&mut form_set, field, &claimed.value)?;
            }
            form_sets.id().update(form_set);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-spell row before the spell family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

fn write_form_set(ctx: &ReducerContext, spell: &Spell) {
    crate::spell::write_dbc_form_set(ctx, spell.spell_id, spell.form_mask, spell.form_exclude);
}

fn blank_spell(spell_id: u32) -> Spell {
    Spell {
        gcd_ms: 0,
        gcd_category: 0,
        ..Spell::blank(spell_id, "")
    }
}

fn apply_spell_field(spell: &mut Spell, field: &str, value: &FieldValue) -> Result<(), String> {
    match field {
        "name" => spell.name = as_str(field, value)?,
        "power_type" => spell.power_type = as_u8(field, value)?,
        "cost" => spell.cost = as_u32(field, value)?,
        "cast_time_ms" => spell.cast_time_ms = as_u32(field, value)?,
        "gcd_ms" => spell.gcd_ms = as_u32(field, value)?,
        "cooldown_ms" => spell.cooldown_ms = as_u32(field, value)?,
        "range_yd" => spell.range_yd = as_u32(field, value)?,
        "duration_ms" => spell.duration_ms = as_u32(field, value)?,
        "school_mask" => spell.school_mask = as_u8(field, value)?,
        "dispel_type" => spell.dispel_type = as_u8(field, value)?,
        "mechanic" => spell.mechanic = as_u8(field, value)?,
        "max_stacks" => spell.max_stacks = as_u8(field, value)?,
        "aura_interrupt" => spell.aura_interrupt = as_u16(field, value)?,
        "attributes" => spell.attributes = as_u32(field, value)?,
        "spell_level" => spell.spell_level = as_u8(field, value)?,
        "max_level" => spell.max_level = as_u8(field, value)?,
        "is_negative" => spell.is_negative = as_bool(field, value)?,
        "cast_flags" => spell.cast_flags = as_u32(field, value)?,
        "stances" => spell.stances = as_u8(field, value)?,
        "family_name" => spell.family_name = as_u8(field, value)?,
        "family_flags" => spell.family_flags = as_u64(field, value)?,
        "proc_flags" => spell.proc_flags = as_u32(field, value)?,
        "proc_chance" => spell.proc_chance = as_u8(field, value)?,
        "single_target_key" => spell.single_target_key = as_u64(field, value)?,
        "form_mask" => spell.form_mask = as_u32(field, value)?,
        "form_exclude" => spell.form_exclude = as_u32(field, value)?,
        "attributes_ex_2" => spell.attributes_ex_2 = as_u32(field, value)?,
        "visual_id" => spell.visual_id = as_u32(field, value)?,
        "duration_index" => spell.duration_index = as_u32(field, value)?,
        "category" => spell.category = as_u32(field, value)?,
        "category_cooldown_ms" => spell.category_cooldown_ms = as_u32(field, value)?,
        "equipped_item_class" => spell.equipped_item_class = as_i32(field, value)?,
        "equipped_item_subclass_mask" => spell.equipped_item_subclass_mask = as_u32(field, value)?,
        "equipped_item_inventory_mask" => {
            spell.equipped_item_inventory_mask = as_u32(field, value)?
        }
        "attributes_ex_3" => spell.attributes_ex_3 = as_u32(field, value)?,
        "attributes_ex_4" => spell.attributes_ex_4 = as_u32(field, value)?,
        "damage_class" => spell.damage_class = as_u8(field, value)?,
        "gcd_category" => spell.gcd_category = as_u32(field, value)?,

        "custom_flags" => spell.custom_flags = as_u32(field, value)?,
        "min_target_level" => spell.min_target_level = as_u32(field, value)?,
        "proc_charges" => spell.proc_charges = as_u8(field, value)?,
        other => return Err(format!("`game_spell` has no claimable column `{other}`")),
    }
    Ok(())
}

fn apply_effect_field(
    effect: &mut SpellEffect,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "kind" => effect.kind = as_u8(field, value)?,
        "base_points" => effect.base_points = as_i32(field, value)?,
        "die_sides" => effect.die_sides = as_i32(field, value)?,
        "per_level" => effect.per_level = as_f32(field, value)?,
        "period_ms" => effect.period_ms = as_u32(field, value)?,
        "target" => effect.target = as_u8(field, value)?,
        "radius_yd" => effect.radius_yd = as_f32(field, value)?,
        "chain_targets" => effect.chain_targets = as_u8(field, value)?,
        "trigger_spell" => effect.trigger_spell = as_u32(field, value)?,
        "effect_mechanic" => effect.effect_mechanic = as_u8(field, value)?,
        "p0" => effect.p0 = as_i32(field, value)?,
        "p0_kind" => effect.p0_kind = as_u8(field, value)?,
        "p1" => effect.p1 = as_i32(field, value)?,
        "script_id" => effect.script_id = as_u32(field, value)?,
        "bonus_coefficient" => effect.bonus_coefficient = as_f32(field, value)?,
        "equip_supported" => effect.equip_supported = as_bool(field, value)?,
        "enters_combat" => effect.enters_combat = as_bool(field, value)?,
        other => {
            return Err(format!(
                "`game_spell_effect` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn blank_form(form_id: u32) -> Form {
    Form {
        form_id,
        name: String::new(),
        spell_id: 0,
        acts_as_shapeshifted: false,
        power_type: 0,
        power_on_entry: 0,
        default_for_class: 0,
        client_form: 0,
    }
}

fn apply_form_field(form: &mut Form, field: &str, value: &FieldValue) -> Result<(), String> {
    match field {
        "name" => form.name = as_str(field, value)?,
        "spell_id" => form.spell_id = as_u32(field, value)?,
        "acts_as_shapeshifted" => form.acts_as_shapeshifted = as_bool(field, value)?,
        "power_type" => form.power_type = as_u8(field, value)?,
        "power_on_entry" => form.power_on_entry = as_u8(field, value)?,
        "default_for_class" => form.default_for_class = as_u8(field, value)?,
        "client_form" => form.client_form = as_u8(field, value)?,
        other => return Err(no_such_column("game_form", other)),
    }
    Ok(())
}

fn apply_spell_form_field(
    form_set: &mut SpellForm,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "excluded" => form_set.excluded = as_bool(field, value)?,
        other => return Err(no_such_column("game_spell_form", other)),
    }
    Ok(())
}

fn built_spell(row: &TracedRow) -> Result<Spell, String> {
    check_insert_is_whole(row)?;
    let mut spell = blank_spell(row.key().spell_id());
    for (field, claimed) in row.fields() {
        apply_spell_field(&mut spell, field, &claimed.value)?;
    }
    Ok(spell)
}

fn built_spell_effect(row: &TracedRow) -> Result<SpellEffect, String> {
    check_insert_is_whole(row)?;
    let PrimaryKey::SpellEffect {
        spell_id,
        effect_index,
    } = row.key()
    else {
        return Err(format!(
            "`game_spell_effect` row {} has no effect index",
            row.key()
        ));
    };
    let mut effect = SpellEffect::blank(spell_id, effect_index);
    for (field, claimed) in row.fields() {
        apply_effect_field(&mut effect, field, &claimed.value)?;
    }
    Ok(effect)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, effect_claim, form_claim,
        form_spell_claim, plan, spell_claim, PACKAGE_FORM, PACKAGE_SPELL, REAL_FORM, REAL_SPELL,
        WHOLE_EFFECT_ROW, WHOLE_SPELL_ROW,
    };
    use super::*;

    const A_POWER_RULE: &str = r#"{"power_on_entry":{"type":"u8","value":0}}"#;

    #[test]
    fn a_form_row_is_a_package_row_exactly_when_its_form_is() {
        let plan = plan(&[artifact(
            "example.aspect",
            &[
                form_claim(REAL_FORM, "update", A_POWER_RULE),
                form_claim(PACKAGE_FORM, "update", A_POWER_RULE),
                form_spell_claim(PACKAGE_FORM, REAL_SPELL),
            ]
            .join(","),
        )])
        .expect("plan builds");

        let in_range: Vec<bool> = plan.rows.iter().map(in_package_range).collect();

        assert_eq!(in_range, [false, true, true]);
    }

    #[test]
    fn every_claimable_form_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::Form,
            || blank_form(PACKAGE_FORM),
            apply_form_field,
        );
    }

    #[test]
    fn every_claimable_form_set_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::SpellForm,
            || SpellForm {
                id: lyracore_package_delta::packed_spell_form_id(REAL_SPELL, PACKAGE_FORM),
                spell_id: REAL_SPELL,
                form_id: PACKAGE_FORM,
                excluded: false,
            },
            apply_spell_form_field,
        );
    }

    #[test]
    fn tuning_a_package_spell_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_claim(
                PACKAGE_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(in_package_range(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_base_spell_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_claim(
                REAL_SPELL,
                "update",
                r#"{"cooldown_ms":{"type":"u32","value":1500}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!in_package_range(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_spell_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &spell_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_ROW),
        )])
        .expect("plan builds");

        let spell = built_spell(&plan.rows[0]).expect("row builds");

        assert_eq!(spell.spell_id, PACKAGE_SPELL);
        assert_eq!(spell.name, "Kindled Bolt");
        assert_eq!(spell.cooldown_ms, 8000);
        assert_eq!(spell.aura_interrupt, 3);
        assert_eq!(spell.family_flags, u64::MAX);
        assert!(spell.is_negative);
    }

    #[test]
    fn an_inserted_effect_derives_its_packed_key_from_the_spell_and_the_index() {
        let plan = plan(&[artifact(
            "example.bolt",
            &effect_claim(PACKAGE_SPELL, 2, "insert", WHOLE_EFFECT_ROW),
        )])
        .expect("plan builds");

        let effect = built_spell_effect(&plan.rows[0]).expect("row builds");

        assert_eq!(effect.id, (u64::from(PACKAGE_SPELL) << 2) | 2);
        assert_eq!(effect.spell_id, PACKAGE_SPELL);
        assert_eq!(effect.effect_index, 2);
        assert_eq!(effect.base_points, 120);
        assert!((effect.per_level - 1.5).abs() < f32::EPSILON);
        assert_eq!(effect.p0_kind, 255);
    }

    #[test]
    fn every_claimable_spell_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::Spell,
            || blank_spell(PACKAGE_SPELL),
            apply_spell_field,
        );
    }

    #[test]
    fn every_claimable_effect_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::SpellEffect,
            || SpellEffect::blank(PACKAGE_SPELL, 0),
            apply_effect_field,
        );
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut spell = blank_spell(PACKAGE_SPELL);

        let refusal = apply_spell_field(&mut spell, "cooldown_ms", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("cooldown_ms"), "{refusal}");
    }
}
