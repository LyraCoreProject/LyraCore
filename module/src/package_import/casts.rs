//! The casts Import Family's half of a Package Delta apply: the setters for
//! `game_creature_cast`/`game_creature_spell`, and how to find one row of each.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of casts.
//!
//! `game_creature_cast` is update-only ([`lyracore_package_delta::DeltaError::InsertNotSupported`]):
//! its key names a creature template, out of this family's scope to invent. It carries no Package
//! band, so [`update_target`] and `write_row` never treat it as `Uninvented`, and
//! [`clear_package_range`] never touches it. `game_creature_spell` follows the loot shape instead:
//! its own surrogate `id` is the band, checked by `is_package_cast_id`.
//!
//! The Claim schema and the setters below are one contract; the tests at the bottom fail if a
//! claimable column has no setter.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_cast_id, FieldValue, Operation, PrimaryKey, Table as ClaimTable, TracedRow,
};

use crate::creatures::{game_creature_cast, game_creature_spell, CreatureCast, CreatureSpell};
use crate::{game_creature_template, game_spell};

use super::{as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// Refuses a cast claim whose final row would point at data this Shard does not hold.
pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        let spell_id = final_u32(ctx, row, "spell_id")?;
        if spell_id == 0 || ctx.db.game_spell().spell_id().find(spell_id).is_none() {
            return Err(missing_reference(row, "spell_id", spell_id));
        }

        if row.table() == ClaimTable::CreatureSpell {
            let creature_entry = final_u32(ctx, row, "creature_entry")?;
            if creature_entry == 0
                || ctx
                    .db
                    .game_creature_template()
                    .entry()
                    .find(creature_entry)
                    .is_none()
            {
                return Err(missing_reference(row, "creature_entry", creature_entry));
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

    let value = match row.table() {
        ClaimTable::CreatureCast => ctx
            .db
            .game_creature_cast()
            .creature_entry()
            .find(creature_entry_of(row))
            .map(|cast| match field {
                "spell_id" => cast.spell_id,
                _ => 0,
            }),
        ClaimTable::CreatureSpell => {
            ctx.db
                .game_creature_spell()
                .id()
                .find(row.row_id())
                .map(|spell| match field {
                    "creature_entry" => spell.creature_entry,
                    "spell_id" => spell.spell_id,
                    _ => 0,
                })
        }
        other => unreachable!("cast value lookup received {other}"),
    };

    value.ok_or_else(|| {
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
    // Only `game_creature_spell` has a Package band to reconcile against — `game_creature_cast`
    // permits no insert at all, so no row of it is ever "Package-invented, but no enabled Package
    // inserts it".
    if row.table() == ClaimTable::CreatureSpell && updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = match row.table() {
        ClaimTable::CreatureCast => ctx
            .db
            .game_creature_cast()
            .creature_entry()
            .find(creature_entry_of(row))
            .is_some(),
        ClaimTable::CreatureSpell => ctx
            .db
            .game_creature_spell()
            .id()
            .find(row.row_id())
            .is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-cast row before the casts family's dispatch \
             runs, found {other}"
        ),
    };
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

/// True when a traced update would land on a Package-range `game_creature_spell` row that no
/// enabled Package invents.
///
/// The Package cast range is cleared on every apply, so such a row is gone by the time the write
/// pass runs.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_cast_id(row.row_id())
}

/// Removes every `game_creature_spell` row a Package invented. `game_creature_cast` has no band —
/// nothing to clear there. Once per import, right after the base import rewrote
/// `game_creature_spell` anyway.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let spells = ctx.db.game_creature_spell();
    let stale: Vec<u64> = spells
        .iter()
        .filter(|s| is_package_cast_id(s.id))
        .map(|s| s.id)
        .collect();
    for id in stale {
        spells.id().delete(id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::CreatureCast, Operation::Update) => {
            let rows = ctx.db.game_creature_cast();
            let mut cast = rows
                .creature_entry()
                .find(creature_entry_of(row))
                .ok_or_else(|| {
                    format!("`game_creature_cast` row {} vanished mid-apply", row.key())
                })?;
            for (field, claimed) in row.fields() {
                apply_creature_cast_field(&mut cast, field, &claimed.value)?;
            }
            rows.creature_entry().update(cast);
        }
        (ClaimTable::CreatureCast, Operation::Insert) => unreachable!(
            "`check_inventable` refuses every insert on `game_creature_cast` before a `Claim` \
             can exist; see `DeltaError::InsertNotSupported`"
        ),
        (ClaimTable::CreatureSpell, Operation::Insert) => {
            ctx.db
                .game_creature_spell()
                .try_insert(built_creature_spell(row)?)
                .map_err(|e| {
                    format!(
                        "`game_creature_spell` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::CreatureSpell, Operation::Update) => {
            let rows = ctx.db.game_creature_spell();
            let mut spell = rows.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_creature_spell` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_creature_spell_field(&mut spell, field, &claimed.value)?;
            }
            rows.id().update(spell);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-cast row before the casts family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

// ===========================================================================================
//  Pure row building.
// ===========================================================================================

fn apply_creature_cast_field(
    cast: &mut CreatureCast,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "spell_id" => cast.spell_id = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_creature_cast` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

/// The `creature_entry` a `game_creature_cast` row names. Total, not a `Result`:
/// `check_claims_belong_to` has already refused a foreign-family row before this module's
/// dispatch runs, so `row.key()` is always `PrimaryKey::CreatureCast` here.
fn creature_entry_of(row: &TracedRow) -> u32 {
    let PrimaryKey::CreatureCast { creature_entry } = row.key() else {
        unreachable!(
            "`check_claims_belong_to` refuses a non-cast row before the casts family's dispatch runs"
        )
    };
    creature_entry
}

/// The `id` a `game_creature_spell` row names. Total, not a `Result`:
/// `check_claims_belong_to` has already refused a foreign-family row before this module's
/// dispatch runs.
fn id_of(row: &TracedRow) -> u64 {
    match row.key() {
        PrimaryKey::CreatureSpell { id } => id,
        _ => unreachable!(
            "`check_claims_belong_to` refuses a non-cast row before the casts family's dispatch runs"
        ),
    }
}

fn blank_creature_spell(id: u64) -> CreatureSpell {
    CreatureSpell {
        id,
        creature_entry: 0,
        spell_id: 0,
        priority: 0,
        condition: 0,
        condition_value: 0,
    }
}

fn apply_creature_spell_field(
    spell: &mut CreatureSpell,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "creature_entry" => spell.creature_entry = as_u32(field, value)?,
        "spell_id" => spell.spell_id = as_u32(field, value)?,
        "priority" => spell.priority = as_u8(field, value)?,
        "condition" => spell.condition = as_u8(field, value)?,
        "condition_value" => spell.condition_value = as_u8(field, value)?,
        other => {
            return Err(format!(
                "`game_creature_spell` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_creature_spell(row: &TracedRow) -> Result<CreatureSpell, String> {
    check_insert_is_whole(row)?;
    let mut spell = blank_creature_spell(id_of(row));
    for (field, claimed) in row.fields() {
        apply_creature_spell_field(&mut spell, field, &claimed.value)?;
    }
    Ok(spell)
}

// ===========================================================================================
//  Pure tests. Row WRITING needs a live ReducerContext, which a native test has no way
//  to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, creature_cast_claim, creature_spell_claim, plan, some_value,
        PACKAGE_CREATURE_SPELL, REAL_CREATURE_CAST, REAL_CREATURE_SPELL, WHOLE_CREATURE_SPELL_ROW,
    };
    use super::*;

    /// The Package cast range is cleared on every apply, so tuning a Package `game_creature_spell`
    /// row nobody enables is a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_creature_spell_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &creature_spell_claim(
                PACKAGE_CREATURE_SPELL,
                "update",
                r#"{"priority":{"type":"u8","value":5}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_creature_spell_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &creature_spell_claim(
                REAL_CREATURE_SPELL,
                "update",
                r#"{"priority":{"type":"u8","value":5}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_creature_spell_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &creature_spell_claim(PACKAGE_CREATURE_SPELL, "insert", WHOLE_CREATURE_SPELL_ROW),
        )])
        .expect("plan builds");

        let spell = built_creature_spell(&plan.rows[0]).expect("row builds");

        assert_eq!(spell.id, PACKAGE_CREATURE_SPELL);
        assert_eq!(spell.creature_entry, 6);
        assert_eq!(spell.spell_id, 133);
        assert_eq!(spell.priority, 10);
    }

    /// `game_creature_cast` is update-only: the row a claim names always already exists, so there
    /// is no `built_creature_cast` counterpart to `built_creature_spell` above. The setter is
    /// still exercised directly, on a real creature's row.
    #[test]
    fn an_update_on_creature_cast_carries_the_claimed_spell_onto_the_row() {
        let plan = plan(&[artifact(
            "example.retarget",
            &creature_cast_claim(
                REAL_CREATURE_CAST,
                "update",
                r#"{"spell_id":{"type":"u32","value":200}}"#,
            ),
        )])
        .expect("plan builds");

        let mut cast = CreatureCast {
            creature_entry: REAL_CREATURE_CAST,
            spell_id: 0,
        };
        for (field, claimed) in plan.rows[0].fields() {
            apply_creature_cast_field(&mut cast, field, &claimed.value).expect("setter applies");
        }

        assert_eq!(cast.spell_id, 200);
    }

    /// The Claim schema and the setters above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_creature_cast_column_has_a_setter() {
        let mut cast = CreatureCast {
            creature_entry: REAL_CREATURE_CAST,
            spell_id: 0,
        };
        for column in ClaimTable::CreatureCast.columns() {
            apply_creature_cast_field(&mut cast, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_creature_cast` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn every_claimable_creature_spell_column_has_a_setter() {
        let mut spell = blank_creature_spell(PACKAGE_CREATURE_SPELL);
        for column in ClaimTable::CreatureSpell.columns() {
            apply_creature_spell_field(&mut spell, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_creature_spell` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut spell = blank_creature_spell(PACKAGE_CREATURE_SPELL);

        let refusal = apply_creature_spell_field(&mut spell, "priority", &FieldValue::U32(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("priority"), "{refusal}");
    }
}
