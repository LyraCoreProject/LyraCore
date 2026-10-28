//! Package spells must be applied before their metadata references are checked.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_spell_id, is_package_spellmeta_id, FieldValue, Operation, Table as ClaimTable,
    TracedRow,
};

use crate::{
    game_spell, game_spell_chain, game_spell_learn, game_spell_proc_event, SpellChain, SpellLearn,
    SpellProcEvent,
};

use super::{as_f32, as_u32, as_u64, as_u8, check_insert_is_whole, UpdateTarget};

pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        match row.table() {
            ClaimTable::SpellChain => {
                check_spell(ctx, row, "spell_id", row.row_id() as u32)?;
                for field in ["prev_spell", "req_spell"] {
                    let spell_id = final_u32(ctx, row, field)?;
                    if spell_id != 0 {
                        check_spell(ctx, row, field, spell_id)?;
                    }
                }
                check_spell(ctx, row, "first_spell", final_u32(ctx, row, "first_spell")?)?;
            }
            ClaimTable::SpellLearn => {
                for field in ["parent_spell", "learn_spell"] {
                    check_spell(ctx, row, field, final_u32(ctx, row, field)?)?;
                }
            }
            ClaimTable::SpellProcEvent => {
                check_spell(ctx, row, "spell_id", row.row_id() as u32)?;
            }
            other => unreachable!("spell metadata reference check received {other}"),
        }
    }
    Ok(())
}

fn check_spell(
    ctx: &ReducerContext,
    row: &TracedRow,
    field: &str,
    spell_id: u32,
) -> Result<(), String> {
    if spell_id == 0 || ctx.db.game_spell().spell_id().find(spell_id).is_none() {
        return Err(format!(
            "`{}` row {} references missing {field} {spell_id}",
            row.table(),
            row.key()
        ));
    }
    Ok(())
}

fn final_u32(ctx: &ReducerContext, row: &TracedRow, field: &str) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row.fields().get(field).map(|claimed| &claimed.value) {
        return Ok(*value);
    }

    let id = row.row_id();
    let value = match row.table() {
        ClaimTable::SpellChain => {
            ctx.db
                .game_spell_chain()
                .spell_id()
                .find(id as u32)
                .map(|chain| match field {
                    "prev_spell" => chain.prev_spell,
                    "first_spell" => chain.first_spell,
                    "req_spell" => chain.req_spell,
                    _ => 0,
                })
        }
        ClaimTable::SpellLearn => {
            ctx.db
                .game_spell_learn()
                .id()
                .find(id)
                .map(|learn| match field {
                    "parent_spell" => learn.parent_spell,
                    "learn_spell" => learn.learn_spell,
                    _ => 0,
                })
        }
        other => unreachable!("spell metadata value lookup received {other}"),
    };

    value.ok_or_else(|| {
        format!(
            "`{}` row {} vanished during preflight",
            row.table(),
            row.key()
        )
    })
}

pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let id = row.row_id();
    let present = match row.table() {
        ClaimTable::SpellChain => ctx
            .db
            .game_spell_chain()
            .spell_id()
            .find(id as u32)
            .is_some(),
        ClaimTable::SpellLearn => ctx.db.game_spell_learn().id().find(id).is_some(),
        ClaimTable::SpellProcEvent => ctx
            .db
            .game_spell_proc_event()
            .spell_id()
            .find(id as u32)
            .is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-spellmeta row before the spellmeta family's \
             dispatch runs, found {other}"
        ),
    };
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_row(row.table(), row.row_id())
}

fn is_package_row(table: ClaimTable, id: u64) -> bool {
    match table {
        ClaimTable::SpellLearn => is_package_spellmeta_id(id),
        _ => u32::try_from(id).is_ok_and(is_package_spell_id),
    }
}

pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let chains = ctx.db.game_spell_chain();
    let stale: Vec<u32> = chains
        .iter()
        .map(|row| row.spell_id)
        .filter(|spell_id| is_package_spell_id(*spell_id))
        .collect();
    for spell_id in stale {
        chains.spell_id().delete(spell_id);
    }

    let dependents = ctx.db.game_spell_learn();
    let stale: Vec<u64> = dependents
        .iter()
        .map(|row| row.id)
        .filter(|id| is_package_spellmeta_id(*id))
        .collect();
    for id in stale {
        dependents.id().delete(id);
    }

    let overlays = ctx.db.game_spell_proc_event();
    let stale: Vec<u32> = overlays
        .iter()
        .map(|row| row.spell_id)
        .filter(|spell_id| is_package_spell_id(*spell_id))
        .collect();
    for spell_id in stale {
        overlays.spell_id().delete(spell_id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    let id = row.row_id();
    match (row.table(), row.operation()) {
        (ClaimTable::SpellChain, Operation::Insert) => {
            let mut chain = blank_spell_chain(id as u32);
            build(row, |field, value| {
                apply_spell_chain_field(&mut chain, field, value)
            })?;
            ctx.db
                .game_spell_chain()
                .try_insert(chain)
                .map_err(|e| failed_insert(row, &e))?;
        }
        (ClaimTable::SpellChain, Operation::Update) => {
            let chains = ctx.db.game_spell_chain();
            let mut chain = chains.spell_id().find(id as u32).ok_or_else(|| gone(row))?;
            apply_claimed(row, |field, value| {
                apply_spell_chain_field(&mut chain, field, value)
            })?;
            chains.spell_id().update(chain);
        }
        (ClaimTable::SpellLearn, Operation::Insert) => {
            let mut learn = blank_spell_learn(id);
            build(row, |field, value| {
                apply_spell_learn_field(&mut learn, field, value)
            })?;
            ctx.db
                .game_spell_learn()
                .try_insert(learn)
                .map_err(|e| failed_insert(row, &e))?;
        }
        (ClaimTable::SpellLearn, Operation::Update) => {
            let dependents = ctx.db.game_spell_learn();
            let mut learn = dependents.id().find(id).ok_or_else(|| gone(row))?;
            apply_claimed(row, |field, value| {
                apply_spell_learn_field(&mut learn, field, value)
            })?;
            dependents.id().update(learn);
        }
        (ClaimTable::SpellProcEvent, Operation::Insert) => {
            let mut overlay = blank_spell_proc_event(id as u32);
            build(row, |field, value| {
                apply_spell_proc_event_field(&mut overlay, field, value)
            })?;
            ctx.db
                .game_spell_proc_event()
                .try_insert(overlay)
                .map_err(|e| failed_insert(row, &e))?;
        }
        (ClaimTable::SpellProcEvent, Operation::Update) => {
            let overlays = ctx.db.game_spell_proc_event();
            let mut overlay = overlays
                .spell_id()
                .find(id as u32)
                .ok_or_else(|| gone(row))?;
            apply_claimed(row, |field, value| {
                apply_spell_proc_event_field(&mut overlay, field, value)
            })?;
            overlays.spell_id().update(overlay);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-spellmeta row before the spellmeta family's \
             dispatch runs, found {other}"
        ),
    }
    Ok(())
}

fn failed_insert(row: &TracedRow, e: &dyn std::fmt::Display) -> String {
    format!("`{}` row {} did not insert: {e}", row.table(), row.key())
}

fn gone(row: &TracedRow) -> String {
    format!("`{}` row {} vanished mid-apply", row.table(), row.key())
}

fn build(
    row: &TracedRow,
    set: impl FnMut(&str, &FieldValue) -> Result<(), String>,
) -> Result<(), String> {
    check_insert_is_whole(row)?;
    apply_claimed(row, set)
}

fn apply_claimed(
    row: &TracedRow,
    mut set: impl FnMut(&str, &FieldValue) -> Result<(), String>,
) -> Result<(), String> {
    for (field, claimed) in row.fields() {
        set(field, &claimed.value)?;
    }
    Ok(())
}

fn blank_spell_chain(spell_id: u32) -> SpellChain {
    SpellChain {
        spell_id,
        prev_spell: 0,
        first_spell: 0,
        rank: 0,
        req_spell: 0,
    }
}

fn apply_spell_chain_field(
    chain: &mut SpellChain,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "prev_spell" => chain.prev_spell = as_u32(field, value)?,
        "first_spell" => chain.first_spell = as_u32(field, value)?,
        "rank" => chain.rank = as_u8(field, value)?,
        "req_spell" => chain.req_spell = as_u32(field, value)?,
        other => return Err(no_such_column("game_spell_chain", other)),
    }
    Ok(())
}

fn blank_spell_learn(id: u64) -> SpellLearn {
    SpellLearn {
        id,
        parent_spell: 0,
        learn_spell: 0,
    }
}

fn apply_spell_learn_field(
    learn: &mut SpellLearn,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "parent_spell" => learn.parent_spell = as_u32(field, value)?,
        "learn_spell" => learn.learn_spell = as_u32(field, value)?,
        other => return Err(no_such_column("game_spell_learn", other)),
    }
    Ok(())
}

fn blank_spell_proc_event(spell_id: u32) -> SpellProcEvent {
    SpellProcEvent {
        spell_id,
        proc_flags: 0,
        proc_ex: 0,
        school_mask: 0,
        family_name: 0,
        family_flags: 0,
        ppm_rate: 0.0,
        custom_chance: 0,
        icd_ms: 0,
    }
}

fn apply_spell_proc_event_field(
    overlay: &mut SpellProcEvent,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "proc_flags" => overlay.proc_flags = as_u32(field, value)?,
        "proc_ex" => overlay.proc_ex = as_u32(field, value)?,
        "school_mask" => overlay.school_mask = as_u8(field, value)?,
        "family_name" => overlay.family_name = as_u8(field, value)?,
        "family_flags" => overlay.family_flags = as_u64(field, value)?,
        "ppm_rate" => overlay.ppm_rate = as_f32(field, value)?,
        "custom_chance" => overlay.custom_chance = as_u8(field, value)?,
        "icd_ms" => overlay.icd_ms = as_u32(field, value)?,
        other => return Err(no_such_column("game_spell_proc_event", other)),
    }
    Ok(())
}

fn no_such_column(table: &str, column: &str) -> String {
    format!("`{table}` has no claimable column `{column}`")
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, plan, spell_chain_claim,
        spell_learn_claim, PACKAGE_SPELL, PACKAGE_SPELL_LEARN, REAL_SPELL, REAL_SPELL_LEARN,
        WHOLE_SPELL_CHAIN_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    const A_RANK: &str = r#"{"rank":{"type":"u8","value":3}}"#;
    const A_LEARN: &str = r#"{"learn_spell":{"type":"u32","value":168}}"#;

    #[test]
    fn tuning_a_package_spells_rank_link_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_chain_claim(PACKAGE_SPELL, "update", A_RANK),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_spells_rank_link_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_chain_claim(REAL_SPELL, "update", A_RANK),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn the_dependent_table_reconciles_against_its_own_band_not_the_spell_band() {
        assert!(is_package_row(ClaimTable::SpellLearn, PACKAGE_SPELL_LEARN));
        assert!(!is_package_row(
            ClaimTable::SpellLearn,
            u64::from(PACKAGE_SPELL)
        ));
        assert!(is_package_row(
            ClaimTable::SpellChain,
            u64::from(PACKAGE_SPELL)
        ));
    }

    #[test]
    fn tuning_a_real_dependent_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &spell_learn_claim(REAL_SPELL_LEARN, "update", A_LEARN),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_rank_link_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &spell_chain_claim(PACKAGE_SPELL, "insert", WHOLE_SPELL_CHAIN_ROW),
        )])
        .expect("plan builds");

        let mut chain = blank_spell_chain(PACKAGE_SPELL);
        build(&plan.rows[0], |field, value| {
            apply_spell_chain_field(&mut chain, field, value)
        })
        .expect("row builds");

        assert_eq!(chain.spell_id, PACKAGE_SPELL);
        assert_eq!(chain.first_spell, 133);
        assert_eq!(chain.rank, 2);
    }

    #[test]
    fn every_claimable_spell_metadata_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::SpellChain,
            || blank_spell_chain(PACKAGE_SPELL),
            apply_spell_chain_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::SpellLearn,
            || blank_spell_learn(PACKAGE_SPELL_LEARN),
            apply_spell_learn_field,
        );
        assert_each_column_sets_its_own_field(
            ClaimTable::SpellProcEvent,
            || blank_spell_proc_event(PACKAGE_SPELL),
            apply_spell_proc_event_field,
        );
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut overlay = blank_spell_proc_event(PACKAGE_SPELL);

        let refusal =
            apply_spell_proc_event_field(&mut overlay, "family_flags", &FieldValue::U8(9))
                .expect_err("the setter refuses");

        assert!(refusal.contains("family_flags"), "{refusal}");
    }
}
