//! The spell Import Family's half of a Package Delta apply: the setters for `game_spell` and
//! `game_spell_effect`, where a Package spell lives, and how to find one row.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of spells.
//!
//! The Claim schema and these setters are one contract; the tests below fail if a claimable column
//! has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_spell_id, packed_spell_effect_id, FieldValue, Operation, PrimaryKey,
    Table as ClaimTable, TracedRow,
};

use crate::spell::{game_spell, game_spell_effect, Spell, SpellEffect};

use super::{
    as_bool, as_f32, as_i32, as_str, as_u16, as_u32, as_u64, as_u8, check_insert_is_whole,
    UpdateTarget,
};

/// What this shard holds for the row an `update` claim names.
pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = match row.table() {
        ClaimTable::Spell => ctx
            .db
            .game_spell()
            .spell_id()
            .find(row.key().spell_id())
            .is_some(),
        ClaimTable::SpellEffect => ctx.db.game_spell_effect().id().find(row.row_id()).is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-spell row before the spell family's dispatch \
             runs, found {other}"
        ),
    };
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package spell range is cleared on every apply, so such a row is gone by the time the write
/// pass runs: the Package that owns it is not enabled, and the one tuning it is claiming a row that
/// does not exist. A base spell is different — the base import puts it there.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_spell_id(row.key().spell_id())
}

/// Removes every row a Package invented. A whole-table pass over both spell tables, once per
/// import, right after the base import rewrote them anyway.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
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
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::Spell, Operation::Insert) => {
            ctx.db
                .game_spell()
                .try_insert(built_spell(row)?)
                .map_err(|e| format!("`game_spell` row {} did not insert: {e}", row.key()))?;
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
            spells.spell_id().update(spell);
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
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-spell row before the spell family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

// ===========================================================================================
//  Row building — pure.
// ===========================================================================================

/// A Package spell before any claim has been applied to it. Every claimable column is overwritten
/// by the insert that follows, so these values never survive; they exist because the row's shape
/// has no `Default`.
fn blank_spell(spell_id: u32) -> Spell {
    Spell {
        spell_id,
        name: String::new(),
        power_type: 0,
        cost: 0,
        cast_time_ms: 0,
        gcd_ms: 0,
        cooldown_ms: 0,
        range_yd: 0,
        duration_ms: 0,
        school_mask: 0,
        dispel_type: 0,
        mechanic: 0,
        max_stacks: 0,
        aura_interrupt: 0,
        attributes: 0,
        spell_level: 0,
        max_level: 0,
        is_negative: false,
        cast_flags: 0,
        stances: 0,
        family_name: 0,
        family_flags: 0,
        proc_flags: 0,
        proc_chance: 0,
        proc_charges: 0,
    }
}

/// A Package spell effect before any claim has been applied to it. `id` is the derived packed key,
/// never authored.
fn blank_spell_effect(spell_id: u32, effect_index: u8) -> SpellEffect {
    SpellEffect {
        id: packed_spell_effect_id(spell_id, effect_index),
        spell_id,
        effect_index,
        kind: 0,
        base_points: 0,
        die_sides: 0,
        per_level: 0.0,
        period_ms: 0,
        target: 0,
        radius_yd: 0.0,
        chain_targets: 0,
        trigger_spell: 0,
        effect_mechanic: 0,
        p0: 0,
        p0_kind: 0,
        p1: 0,
        script_id: 0,
        enters_combat: false,
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
        "enters_combat" => effect.enters_combat = as_bool(field, value)?,
        other => {
            return Err(format!(
                "`game_spell_effect` has no claimable column `{other}`"
            ))
        }
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
    let mut effect = blank_spell_effect(spell_id, effect_index);
    for (field, claimed) in row.fields() {
        apply_effect_field(&mut effect, field, &claimed.value)?;
    }
    Ok(effect)
}

// ===========================================================================================
//  Tests — the pure half. Row WRITING needs a live ReducerContext, which a native test has no
//  way to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, effect_claim, plan, some_value, spell_claim, PACKAGE_SPELL, REAL_SPELL,
        WHOLE_EFFECT_ROW, WHOLE_SPELL_ROW,
    };
    use super::*;

    /// The Package spell range is cleared on every apply, so tuning a Package spell nobody enables
    /// is a plan that names a row which will not exist.
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

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
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

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
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

    /// The packed key is derived from the spell and the effect index, never authored, so the built
    /// row must carry the same value `game_aura.effect_id` is reproducible from.
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

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_spell_column_has_a_setter() {
        let mut spell = blank_spell(PACKAGE_SPELL);
        for column in ClaimTable::Spell.columns() {
            apply_spell_field(&mut spell, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_spell` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_effect_column_has_a_setter() {
        let mut effect = blank_spell_effect(PACKAGE_SPELL, 0);
        for column in ClaimTable::SpellEffect.columns() {
            apply_effect_field(&mut effect, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_spell_effect` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut spell = blank_spell(PACKAGE_SPELL);

        let refusal = apply_spell_field(&mut spell, "cooldown_ms", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("cooldown_ms"), "{refusal}");
    }
}
