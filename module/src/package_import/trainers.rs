//! The trainers Import Family's half of a Package Delta apply: the setters for
//! `game_trainer_spell`, and how to find one row of it.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of trainers.
//!
//! Trainers own exactly one table, the loot shape: `game_trainer_spell.id` is a plain surrogate,
//! its own Package band (`is_package_trainer_id`). Unlike every other reference this crate checks,
//! `spell_id` is NOT always a `game_spell` row: a profession offering (`learn_skill_line > 0`)
//! carries a synthetic marker `spell_id` (`module/src/skill.rs`'s `LEARN_COOKING_SPELL_ID` and
//! siblings) that the buy reducer routes around `game_spell` entirely
//! (`crate::skill::learn_profession`). [`check_references`] gates the `game_spell` check on
//! `learn_skill_line == 0` for exactly that reason.
//!
//! The Claim schema and the setter below are one contract; the tests at the bottom fail if a
//! claimable column has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{is_package_trainer_id, FieldValue, Operation, TracedRow};

use crate::trainer::{game_trainer_spell, TrainerSpell};
use crate::{game_creature_template, game_spell};

use super::{as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Refuses a trainer claim whose final row would point at data this Shard does not hold.
///
/// `spell_id` is checked against `game_spell` only when the row's final `learn_skill_line` is 0: a
/// profession offering's `spell_id` is a marker, never a real spell.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        let trainer_entry = final_u32(ctx, row, "trainer_entry")?;
        if trainer_entry == 0
            || ctx
                .db
                .game_creature_template()
                .entry()
                .find(trainer_entry)
                .is_none()
        {
            return Err(missing_reference(row, "trainer_entry", trainer_entry));
        }

        let learn_skill_line = final_u32(ctx, row, "learn_skill_line")?;
        if learn_skill_line == 0 {
            let spell_id = final_u32(ctx, row, "spell_id")?;
            if spell_id == 0 || ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                return Err(missing_reference(row, "spell_id", spell_id));
            }
        }
    }
    Ok(())
}

/// The value `field` will hold once this row's claim lands: the claimed value if the claim sets
/// it, otherwise what the Shard already holds. An update that changes only one column is judged on
/// what the row will hold after the apply, not on the column alone.
fn final_u32(ctx: &ReducerContext, row: &TracedRow, field: &str) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row.fields().get(field).map(|claimed| &claimed.value) {
        return Ok(*value);
    }

    ctx.db
        .game_trainer_spell()
        .id()
        .find(row.row_id())
        .map(|trainer| match field {
            "trainer_entry" => trainer.trainer_entry,
            "spell_id" => trainer.spell_id,
            "learn_skill_line" => trainer.learn_skill_line,
            _ => 0,
        })
        .ok_or_else(|| {
            format!(
                "`{}` row {} vanished during preflight",
                row.table(),
                row.key()
            )
        })
}

fn missing_reference(row: &TracedRow, field: &str, value: u32) -> String {
    format!(
        "`{}` row {} references missing {field} {value}",
        row.table(),
        row.key()
    )
}

/// What this shard holds for the row an `update` claim names.
pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = ctx
        .db
        .game_trainer_spell()
        .id()
        .find(row.row_id())
        .is_some();
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package trainer range is cleared on every apply, so such a row is gone by the time the
/// write pass runs.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_trainer_id(row.row_id())
}

/// Removes every row a Package invented. A whole-table pass over `game_trainer_spell`, once per
/// import, right after the base import rewrote it anyway.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let rows = ctx.db.game_trainer_spell();
    let stale: Vec<u64> = rows
        .iter()
        .filter(|row| is_package_trainer_id(row.id))
        .map(|row| row.id)
        .collect();
    for id in stale {
        rows.id().delete(id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match row.operation() {
        Operation::Insert => {
            ctx.db
                .game_trainer_spell()
                .try_insert(built_trainer_spell(row)?)
                .map_err(|e| {
                    format!("`game_trainer_spell` row {} did not insert: {e}", row.key())
                })?;
        }
        Operation::Update => {
            let rows = ctx.db.game_trainer_spell();
            let mut trainer = rows.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_trainer_spell` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_trainer_spell_field(&mut trainer, field, &claimed.value)?;
            }
            rows.id().update(trainer);
        }
    }
    Ok(())
}

// ===========================================================================================
//  Pure row building.
// ===========================================================================================

/// A Package trainer offering before any claim has been applied to it, defaulted the same way the
/// Module's own auto-migration defaults a class-spell row: `learn_skill_line` 0 (not a profession),
/// `learn_skill_cap` 75 (Apprentice, only meaningful once `learn_skill_line` is set).
fn blank_trainer_spell(id: u64) -> TrainerSpell {
    TrainerSpell {
        id,
        trainer_entry: 0,
        spell_id: 0,
        cost: 0,
        required_level: 0,
        learn_skill_line: 0,
        learn_skill_cap: 75,
    }
}

fn apply_trainer_spell_field(
    trainer: &mut TrainerSpell,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "trainer_entry" => trainer.trainer_entry = as_u32(field, value)?,
        "spell_id" => trainer.spell_id = as_u32(field, value)?,
        "cost" => trainer.cost = as_u32(field, value)?,
        "required_level" => trainer.required_level = as_u8(field, value)?,
        "learn_skill_line" => trainer.learn_skill_line = as_u32(field, value)?,
        "learn_skill_cap" => trainer.learn_skill_cap = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_trainer_spell` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_trainer_spell(row: &TracedRow) -> Result<TrainerSpell, String> {
    check_insert_is_whole(row)?;
    let mut trainer = blank_trainer_spell(row.row_id());
    for (field, claimed) in row.fields() {
        apply_trainer_spell_field(&mut trainer, field, &claimed.value)?;
    }
    Ok(trainer)
}

// ===========================================================================================
//  Pure tests. Row WRITING needs a live ReducerContext, which a native test has no way
//  to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, plan, some_value, trainer_spell_claim, PACKAGE_TRAINER_SPELL, REAL_TRAINER_SPELL,
        WHOLE_TRAINER_SPELL_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    /// The Package trainer range is cleared on every apply, so tuning a Package trainer row nobody
    /// enables is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_trainer_spell_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &trainer_spell_claim(
                PACKAGE_TRAINER_SPELL,
                "update",
                r#"{"cost":{"type":"u32","value":100}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_trainer_spell_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &trainer_spell_claim(
                REAL_TRAINER_SPELL,
                "update",
                r#"{"cost":{"type":"u32","value":100}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_trainer_spell_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &trainer_spell_claim(PACKAGE_TRAINER_SPELL, "insert", WHOLE_TRAINER_SPELL_ROW),
        )])
        .expect("plan builds");

        let trainer = built_trainer_spell(&plan.rows[0]).expect("row builds");

        assert_eq!(trainer.id, PACKAGE_TRAINER_SPELL);
        assert_eq!(trainer.trainer_entry, 100);
        assert_eq!(trainer.spell_id, 133);
        assert_eq!(trainer.cost, 500);
        assert_eq!(trainer.learn_skill_line, 0);
        assert_eq!(trainer.learn_skill_cap, 75);
    }

    /// The Claim schema and the setter above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_trainer_spell_column_has_a_setter() {
        let mut trainer = blank_trainer_spell(PACKAGE_TRAINER_SPELL);
        for column in ClaimTable::TrainerSpell.columns() {
            apply_trainer_spell_field(&mut trainer, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_trainer_spell` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut trainer = blank_trainer_spell(PACKAGE_TRAINER_SPELL);

        let refusal = apply_trainer_spell_field(&mut trainer, "cost", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("cost"), "{refusal}");
    }
}
