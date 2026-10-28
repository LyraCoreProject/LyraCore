use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{is_package_trainer_id, FieldValue, Operation, TracedRow};

use crate::trainer::{game_trainer_spell, TrainerSpell};
use crate::{game_creature_template, game_spell};

use super::{as_u32, as_u8, check_insert_is_whole, UpdateTarget};

// Profession offerings carry synthetic spell ids, so only class offerings join game_spell.
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

fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_trainer_id(row.row_id())
}

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

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, plan, trainer_spell_claim,
        PACKAGE_TRAINER_SPELL, REAL_TRAINER_SPELL, WHOLE_TRAINER_SPELL_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

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

    #[test]
    fn every_claimable_trainer_spell_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::TrainerSpell,
            || blank_trainer_spell(PACKAGE_TRAINER_SPELL),
            apply_trainer_spell_field,
        );
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut trainer = blank_trainer_spell(PACKAGE_TRAINER_SPELL);

        let refusal = apply_trainer_spell_field(&mut trainer, "cost", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("cost"), "{refusal}");
    }
}
