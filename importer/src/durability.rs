use crate::dbc;
use crate::dump::push_insert;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use wow_dbc::vanilla_tables::{
    durability_costs::DurabilityCosts, durability_quality::DurabilityQuality,
};
use wow_dbc::DbcTable;
use wow_mpq::PatchChain;

pub(crate) fn catalogue_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize, usize)> {
    let costs: DurabilityCosts = dbc::read_table(chain)?;
    let qualities: DurabilityQuality = dbc::read_table(chain)?;
    let (mut statements, cost_rows) = cost_sql(&costs)?;
    let (quality_statements, quality_rows) = quality_sql(&qualities)?;
    statements.extend(quality_statements);
    Ok((statements, cost_rows, quality_rows))
}

fn cost_sql(table: &DurabilityCosts) -> Result<(Vec<String>, usize)> {
    if table.rows().is_empty() {
        bail!("DurabilityCosts.dbc contains no rows");
    }
    let mut seen_levels = HashSet::new();
    let mut rows = Vec::new();
    for row in table.rows() {
        let item_level = row.id.id;
        if !seen_levels.insert(item_level) {
            bail!("DurabilityCosts.dbc contains duplicate item level {item_level}");
        }
        let multipliers = row
            .weapon_subclass_cost
            .iter()
            .chain(&row.armour_subclass_cost)
            .map(|cost| {
                u32::try_from(*cost)
                    .map(|cost| cost.to_string())
                    .with_context(|| {
                        format!("DurabilityCosts.dbc item level {item_level} has a negative cost")
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        rows.push((
            item_level,
            format!("({item_level},[{}])", multipliers.join(",")),
        ));
    }
    rows.sort_unstable_by_key(|(item_level, _)| *item_level);
    let count = rows.len();
    let rows: Vec<String> = rows.into_iter().map(|(_, sql)| sql).collect();
    let mut statements = vec!["DELETE FROM game_durability_cost WHERE item_level >= 0".to_string()];
    push_insert(
        &mut statements,
        "game_durability_cost",
        "item_level,multipliers",
        &rows,
    );
    Ok((statements, count))
}

fn quality_sql(table: &DurabilityQuality) -> Result<(Vec<String>, usize)> {
    if table.rows().is_empty() {
        bail!("DurabilityQuality.dbc contains no rows");
    }
    let mut seen_ids = HashSet::new();
    let mut rows = Vec::new();
    for row in table.rows() {
        let id = row.id.id;
        if !seen_ids.insert(id) {
            bail!("DurabilityQuality.dbc contains duplicate id {id}");
        }
        if !row.data.is_finite() || row.data < 0.0 {
            bail!(
                "DurabilityQuality.dbc row {id} has an invalid modifier {}",
                row.data
            );
        }
        rows.push((id, format!("({id},{:?})", row.data)));
    }
    rows.sort_unstable_by_key(|(id, _)| *id);
    let count = rows.len();
    let rows: Vec<String> = rows.into_iter().map(|(_, sql)| sql).collect();
    let mut statements = vec!["DELETE FROM game_durability_quality WHERE id >= 0".to_string()];
    push_insert(
        &mut statements,
        "game_durability_quality",
        "id,quality_mod",
        &rows,
    );
    Ok((statements, count))
}

#[cfg(test)]
mod tests {
    use super::{cost_sql, quality_sql};
    use wow_dbc::vanilla_tables::durability_costs::{
        DurabilityCosts, DurabilityCostsKey, DurabilityCostsRow,
    };
    use wow_dbc::vanilla_tables::durability_quality::{
        DurabilityQuality, DurabilityQualityKey, DurabilityQualityRow,
    };

    fn cost_row(item_level: u32, weapon_axe: i32, armor_plate: i32) -> DurabilityCostsRow {
        let mut weapon_subclass_cost = [0; 21];
        weapon_subclass_cost[1] = weapon_axe;
        let mut armour_subclass_cost = [0; 8];
        armour_subclass_cost[4] = armor_plate;
        DurabilityCostsRow {
            id: DurabilityCostsKey::new(item_level),
            weapon_subclass_cost,
            armour_subclass_cost,
        }
    }

    fn quality_row(id: u32, data: f32) -> DurabilityQualityRow {
        DurabilityQualityRow {
            id: DurabilityQualityKey::new(id),
            data,
        }
    }

    #[test]
    fn cost_rows_list_weapon_subclasses_then_armor_subclasses_and_keep_zeros() {
        let (statements, count) = cost_sql(&DurabilityCosts {
            rows: vec![cost_row(2, 7, 11)],
        })
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            statements,
            [
                "DELETE FROM game_durability_cost WHERE item_level >= 0",
                "INSERT INTO game_durability_cost (item_level,multipliers) VALUES (2,[0,7,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,11,0,0,0])",
            ]
        );
    }

    #[test]
    fn cost_rows_come_out_in_item_level_order_and_reject_bad_tables() {
        let (ascending, _) = cost_sql(&DurabilityCosts {
            rows: vec![cost_row(1, 1, 1), cost_row(9, 1, 1)],
        })
        .unwrap();
        let (descending, _) = cost_sql(&DurabilityCosts {
            rows: vec![cost_row(9, 1, 1), cost_row(1, 1, 1)],
        })
        .unwrap();
        assert_eq!(ascending, descending);
        for (rows, reason) in [
            (vec![], "DurabilityCosts.dbc contains no rows"),
            (
                vec![cost_row(3, 1, 1), cost_row(3, 2, 2)],
                "DurabilityCosts.dbc contains duplicate item level 3",
            ),
            (
                vec![cost_row(4, -1, 1)],
                "DurabilityCosts.dbc item level 4 has a negative cost",
            ),
        ] {
            assert_eq!(
                cost_sql(&DurabilityCosts { rows }).unwrap_err().to_string(),
                reason
            );
        }
    }

    #[test]
    fn quality_rows_write_float_literals_in_id_order_and_reject_bad_tables() {
        let (statements, count) = quality_sql(&DurabilityQuality {
            rows: vec![quality_row(4, 1.0), quality_row(2, 0.5)],
        })
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            statements[1],
            "INSERT INTO game_durability_quality (id,quality_mod) VALUES (2,0.5),(4,1.0)"
        );
        for (rows, reason) in [
            (vec![], "DurabilityQuality.dbc contains no rows"),
            (
                vec![quality_row(2, 1.0), quality_row(2, 2.0)],
                "DurabilityQuality.dbc contains duplicate id 2",
            ),
            (
                vec![quality_row(2, -1.0)],
                "DurabilityQuality.dbc row 2 has an invalid modifier -1",
            ),
        ] {
            assert_eq!(
                quality_sql(&DurabilityQuality { rows })
                    .unwrap_err()
                    .to_string(),
                reason
            );
        }
    }
}
