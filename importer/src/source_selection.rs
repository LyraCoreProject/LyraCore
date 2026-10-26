// See docs/data-ingestion.md for patch/build selection and the pinned reference rules.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};

use crate::world_source::Row;

pub(crate) const PATCH: u32 = 10;
pub(crate) const CLIENT_BUILD: u32 = 5875;

pub(crate) fn required_columns(table: &str) -> &'static [&'static str] {
    match table {
        "creature_template"
        | "item_template"
        | "quest_template"
        | "gameobject_template"
        | "creature_onkill_reputation"
        | "areatrigger_teleport"
        | "game_event_creature_data"
        | "creature_addon"
        | "map_template"
        | "battleground_template" => &["patch"],
        "creature"
        | "gameobject"
        | "creature_questrelation"
        | "creature_involvedrelation"
        | "gameobject_questrelation"
        | "gameobject_involvedrelation"
        | "pool_template"
        | "pool_creature"
        | "pool_creature_template"
        | "pool_gameobject"
        | "pool_gameobject_template"
        | "item_enchantment_template"
        | "creature_equip_template"
        | "game_event"
        | "game_graveyard_zone"
        | "creature_charm_spells"
        | "petcreateinfo_spell" => &["patch_min", "patch_max"],
        "areatrigger_tavern" | "game_event_quest" => &["patch_min"],
        "spell_template"
        | "faction"
        | "faction_template"
        | "areatrigger_template"
        | "taxi_nodes"
        | "spell_group_stack_rules"
        | "transports"
        | "skill_line_ability" => &["build"],
        "npc_trainer"
        | "npc_trainer_template"
        | "spell_chain"
        | "spell_proc_event"
        | "spell_learn_spell"
        | "spell_script_target"
        | "spell_target_position"
        | "spell_threat"
        | "spell_group"
        | "spell_elixir"
        | "playercreateinfo_spell" => &["build_min", "build_max"],
        "taxi_path_transitions" => &["build_min"],
        "conditions" => &["type", "value1", "value2"],
        "forbidden_items" => &["patch", "after_or_before"],
        _ if table.ends_with("_loot_template") => &["patch_min", "patch_max"],
        _ => &[],
    }
}

pub(crate) fn select(
    table: &str,
    columns: &BTreeSet<String>,
    primary_key: &[String],
    rows: Vec<Row>,
) -> Result<Vec<Row>> {
    for &column in required_columns(table) {
        if !columns.contains(column) {
            bail!("{table}.{column}: missing selection column");
        }
    }
    if table == "forbidden_items" {
        return rows
            .into_iter()
            .filter_map(|row| match forbidden_item(&row) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .collect();
    }
    if table == "conditions" {
        for row in &rows {
            if row.number::<i32>("type")? == 24 {
                wow_patch(row.number("value1")?, row.number("value2")?)?;
            }
        }
    }
    let revision = if columns.contains("patch") {
        Some(("patch", PATCH))
    } else if columns.contains("build") && table != "skill_line_ability" {
        Some(("build", CLIENT_BUILD))
    } else {
        None
    };
    let mut selected = Vec::new();
    let mut versions = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        if columns.contains("patch_min")
            && !in_range(
                &row,
                "patch_min",
                "patch_max",
                PATCH,
                columns.contains("patch_max"),
            )?
        {
            continue;
        }
        if columns.contains("build_min")
            && !in_range(
                &row,
                "build_min",
                "build_max",
                CLIENT_BUILD,
                columns.contains("build_max"),
            )?
        {
            continue;
        }
        if table == "skill_line_ability" && row.number::<u32>("build")? != CLIENT_BUILD {
            continue;
        }
        if let Some((column, maximum)) = revision {
            let version: u32 = row.number(column)?;
            if version > maximum {
                continue;
            }
            let keys: Vec<_> = primary_key
                .iter()
                .filter(|name| name.as_str() != column)
                .collect();
            if keys.is_empty() {
                bail!("{table}: versioned rows need a primary key besides {column}");
            }
            let key = keys
                .iter()
                .map(|name| row.number::<i64>(name))
                .collect::<Result<Vec<_>>>()?;
            if !seen.insert((key.clone(), version)) {
                bail!("{table}: duplicate {column} revision {version}");
            }
            match versions.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((version, row));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().0 < version {
                        entry.insert((version, row));
                    }
                }
            }
        } else {
            selected.push(row);
        }
    }
    if revision.is_some() {
        selected.extend(versions.into_values().map(|(_, row)| row));
    }
    Ok(selected)
}

fn in_range(
    row: &Row,
    minimum: &str,
    maximum: &str,
    selected: u32,
    has_maximum: bool,
) -> Result<bool> {
    let lower = row.number::<u32>(minimum)?;
    let upper = if has_maximum {
        row.number::<u32>(maximum)?
    } else {
        u32::MAX
    };
    if lower > upper {
        bail!("invalid {minimum}..{maximum} range {lower}..{upper}");
    }
    Ok(lower <= selected && selected <= upper)
}

fn forbidden_item(row: &Row) -> Result<bool> {
    let patch = row.number::<u32>("patch")?;
    match row.number::<u32>("after_or_before")? {
        0 => Ok(PATCH >= patch),
        1 => Ok(PATCH <= patch),
        value => bail!("forbidden_items.after_or_before: unknown value {value}"),
    }
}

pub(crate) fn wow_patch(value: u32, comparison: u32) -> Result<bool> {
    if value > PATCH {
        bail!("WOW_PATCH value {value} is outside 0..=10");
    }
    match comparison {
        0 => Ok(PATCH == value),
        1 => Ok(PATCH >= value),
        2 => Ok(PATCH <= value),
        _ => bail!("WOW_PATCH comparison {comparison} is outside 0..=2"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::{Cell, TableBuilder};

    fn selected(table: &str, key: &[&str], rows: &[&[(&str, i64)]]) -> Result<Vec<Row>> {
        let columns: BTreeSet<String> = rows
            .iter()
            .flat_map(|row| row.iter())
            .map(|(name, _)| (*name).to_owned())
            .collect();
        let mut output = TableBuilder::new(table, columns.iter().cloned());
        for values in rows {
            output.push(
                columns
                    .iter()
                    .map(|name| {
                        let value = values.iter().find(|(column, _)| column == name).unwrap().1;
                        Cell::Number(value.to_string().into())
                    })
                    .collect(),
            )?;
        }
        select(
            table,
            &columns,
            &key.iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
            output.finish(),
        )
    }

    #[test]
    fn a_missing_version_or_range_column_cannot_disable_selection() {
        assert!(selected("item_template", &["entry"], &[&[("entry", 1)]])
            .unwrap_err()
            .to_string()
            .contains("item_template.patch"));
        assert!(selected("creature", &[], &[&[("patch_min", 0)]]).is_err());
        assert!(selected("npc_trainer", &[], &[&[("build_max", 5875)]]).is_err());
    }

    #[test]
    fn versioned_patch_keeps_the_last_eligible_revision_per_composite_key() {
        let rows = selected(
            "game_event_creature_data",
            &["guid", "id", "patch"],
            &[
                &[("guid", 2), ("id", 4), ("patch", 11)],
                &[("guid", 2), ("id", 3), ("patch", 10)],
                &[("guid", 1), ("id", 3), ("patch", 8)],
                &[("guid", 2), ("id", 3), ("patch", 0)],
                &[("guid", 1), ("id", 3), ("patch", 9)],
            ],
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].number::<u32>("patch").unwrap(), 9);
        assert_eq!(rows[1].number::<u32>("patch").unwrap(), 10);
    }

    #[test]
    fn versioned_keys_sort_numerically_including_composite_components() {
        let rows = selected(
            "game_event_creature_data",
            &["guid", "id", "patch"],
            &[
                &[("guid", 10), ("id", 1), ("patch", 10)],
                &[("guid", 2), ("id", 10), ("patch", 10)],
                &[("guid", 100), ("id", 1), ("patch", 10)],
                &[("guid", 1), ("id", 1), ("patch", 10)],
                &[("guid", 2), ("id", 2), ("patch", 9)],
                &[("guid", 2), ("id", 2), ("patch", 10)],
            ],
        )
        .unwrap();
        let keys: Vec<(u32, u32)> = rows
            .iter()
            .map(|row| (row.number("guid").unwrap(), row.number("id").unwrap()))
            .collect();
        assert_eq!(keys, [(1, 1), (2, 2), (2, 10), (10, 1), (100, 1)]);
    }

    #[test]
    fn taxi_transitions_include_the_minimum_build_without_an_upper_bound() {
        let rows = selected(
            "taxi_path_transitions",
            &[],
            &[
                &[("in_path", 1), ("build_min", 5874)],
                &[("in_path", 2), ("build_min", 5875)],
                &[("in_path", 3), ("build_min", 5876)],
            ],
        )
        .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.number::<u32>("in_path").unwrap())
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(selected("taxi_path_transitions", &[], &[&[("in_path", 1)]]).is_err());
    }

    #[test]
    fn spell_builds_choose_5875_or_the_nearest_earlier_build() {
        let rows = selected(
            "spell_template",
            &["entry", "build"],
            &[
                &[("entry", 1), ("build", 5875)],
                &[("entry", 1), ("build", 6000)],
                &[("entry", 2), ("build", 5000)],
                &[("entry", 2), ("build", 4000)],
            ],
        )
        .unwrap();
        assert_eq!(rows[0].number::<u32>("build").unwrap(), 5875);
        assert_eq!(rows[1].number::<u32>("build").unwrap(), 5000);
    }

    #[test]
    fn skill_line_abilities_require_the_exact_client_build() {
        let rows = selected(
            "skill_line_ability",
            &["entry", "build"],
            &[
                &[("entry", 1), ("build", 5875)],
                &[("entry", 2), ("build", 5874)],
                &[("entry", 3), ("build", 5876)],
            ],
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].number::<u32>("entry").unwrap(), 1);
    }

    #[test]
    fn ranges_include_both_endpoints_and_support_a_patch_minimum_without_a_maximum() {
        for (table, lower, upper, value) in [
            ("creature", "patch_min", "patch_max", 10),
            ("playercreateinfo_spell", "build_min", "build_max", 5875),
        ] {
            let rows = selected(
                table,
                &[],
                &[
                    &[(lower, 0), (upper, value)],
                    &[(lower, value), (upper, value + 1)],
                    &[(lower, 0), (upper, value - 1)],
                    &[(lower, value + 1), (upper, value + 2)],
                ],
            )
            .unwrap();
            assert_eq!(rows.len(), 2);
            assert!(selected(table, &[], &[&[(lower, value + 1), (upper, value)]]).is_err());
        }
        assert_eq!(
            selected(
                "areatrigger_tavern",
                &[],
                &[
                    &[("patch_min", 10)],
                    &[("patch_min", 11)],
                    &[("patch_min", 0)],
                ]
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn forbidden_items_include_the_boundary_for_both_directions() {
        let rows = selected(
            "forbidden_items",
            &[],
            &[
                &[("patch", 9), ("after_or_before", 0)],
                &[("patch", 10), ("after_or_before", 0)],
                &[("patch", 11), ("after_or_before", 0)],
                &[("patch", 9), ("after_or_before", 1)],
                &[("patch", 10), ("after_or_before", 1)],
                &[("patch", 11), ("after_or_before", 1)],
            ],
        )
        .unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter()
                .map(|r| r.number::<u32>("patch").unwrap())
                .collect::<Vec<_>>(),
            [9, 10, 10, 11]
        );
        assert!(selected(
            "forbidden_items",
            &[],
            &[&[("patch", 10), ("after_or_before", 2)]]
        )
        .is_err());
    }

    #[test]
    fn wow_patch_condition_checks_each_comparison_and_refuses_unknown_values() {
        for (value, comparison, result) in [
            (10, 0, true),
            (9, 0, false),
            (9, 1, true),
            (10, 1, true),
            (9, 2, false),
            (10, 2, true),
        ] {
            assert_eq!(wow_patch(value, comparison).unwrap(), result);
        }
        assert!(wow_patch(11, 0).is_err());
        assert!(wow_patch(10, 3).is_err());
        assert!(selected(
            "conditions",
            &[],
            &[&[("type", 24), ("value1", 10), ("value2", 3)]]
        )
        .is_err());
    }

    #[test]
    fn ambiguous_or_unkeyed_versions_fail_instead_of_picking_input_order() {
        let rows: &[&[(&str, i64)]] = &[
            &[("entry", 1), ("patch", 10)],
            &[("entry", 1), ("patch", 10)],
        ];
        assert!(selected("item_template", &["entry", "patch"], rows).is_err());
        assert!(selected("item_template", &["patch"], &rows[..1]).is_err());
    }
}
