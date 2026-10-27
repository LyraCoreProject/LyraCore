use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use wow_dbc::vanilla_tables::taxi_path::TaxiPath;
use wow_dbc::vanilla_tables::taxi_path_node::TaxiPathNode;
use wow_dbc::DbcTable;

use crate::dump::{got, push_insert, sql_text};

const MO_TRANSPORT: u32 = 15;

mod tr {
    pub const ENTRY: &str = "entry";
    pub const PERIOD: &str = "period";
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TransportCatalogueSql {
    pub(crate) statements: Vec<String>,
    pub(crate) count: usize,
    pub(crate) warnings: Vec<String>,
}

/// Joins the world database's `transports` and their type-15 templates to imported taxi paths.
pub(crate) fn catalogue_sql(
    paths: &TaxiPath,
    points: &TaxiPathNode,
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<TransportCatalogueSql> {
    let template_rows = crate::dump::parse_table(dump, "gameobject_template")?;
    let templates: HashMap<u32, &crate::world_source::Row> = template_rows
        .iter()
        .map(|row| Ok((row.number::<u32>(got::ENTRY)?, row)))
        .collect::<Result<_>>()?;
    let path_ids: HashSet<u32> = paths.rows().iter().map(|row| row.id.id).collect();
    let transports = crate::dump::parse_table(dump, "transports")?;
    if transports.is_empty() {
        bail!("transport dump has no transports rows");
    }
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for transport in transports.iter() {
        let entry: u32 = transport
            .number(tr::ENTRY)
            .context("transports has an invalid entry")?;
        let period_ms: u32 = transport
            .number(tr::PERIOD)
            .with_context(|| format!("transport {entry} has an invalid period"))?;
        let Some(template) = templates.get(&entry) else {
            warnings.push(format!(
                "transports: WARN transport {entry} has no gameobject_template row; omitted"
            ));
            continue;
        };
        let number = |column: &str| -> Result<u32> {
            template
                .number(column)
                .with_context(|| format!("transport {entry} template column {column} is invalid"))
        };
        if number(got::TYPE)? != MO_TRANSPORT {
            warnings.push(format!(
                "transports: WARN transport {entry} is not an MO_TRANSPORT; omitted"
            ));
            continue;
        }
        let (path_id, speed, accel) = (
            number(got::DATA0)?,
            number(got::DATA1)?,
            number(got::DATA2)?,
        );
        let mut nodes: Vec<_> = points
            .rows()
            .iter()
            .filter(|point| point.taxi_path.id == path_id)
            .collect();
        nodes.sort_by_key(|point| point.node_index);
        if !path_ids.contains(&path_id) || nodes.len() < 3 {
            warnings.push(format!(
                "transports: WARN transport {entry} path {path_id} has no imported route; omitted"
            ));
            continue;
        }
        if speed == 0 || accel == 0 || period_ms == 0 {
            warnings.push(format!(
                "transports: WARN transport {entry} has no speed, acceleration or period; omitted"
            ));
            continue;
        }
        let map_id = nodes[0].map.id;
        let size: f32 = template
            .number(got::SIZE)
            .with_context(|| format!("transport {entry} has an invalid size"))?;
        rows.push(format!(
            "({entry},{},{},{size},{},{path_id},{speed},{accel},{period_ms},{map_id})",
            sql_text(template.text(got::NAME)?.trim()),
            number(got::DISPLAY_ID)?,
            number(got::FLAGS)?,
        ));
    }
    let count = rows.len();
    let mut statements = vec!["DELETE FROM game_transport WHERE entry >= 0".to_string()];
    push_insert(
        &mut statements,
        "game_transport",
        "entry,name,display_id,size,flags,taxi_path_id,move_speed,accel_rate,period_ms,map_id",
        &rows,
    );
    Ok(TransportCatalogueSql {
        statements,
        count,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_dbc::vanilla_tables::map::MapKey;
    use wow_dbc::vanilla_tables::taxi_nodes::TaxiNodesKey;
    use wow_dbc::vanilla_tables::taxi_path::{TaxiPathKey, TaxiPathRow};
    use wow_dbc::vanilla_tables::taxi_path_node::{TaxiPathNodeKey, TaxiPathNodeRow};

    fn paths(ids: &[u32]) -> TaxiPath {
        TaxiPath {
            rows: ids
                .iter()
                .map(|&id| TaxiPathRow {
                    id: TaxiPathKey::new(id),
                    source_taxi_node: TaxiNodesKey::new(1),
                    destination_taxi_node: TaxiNodesKey::new(2),
                    cost: 0,
                })
                .collect(),
        }
    }

    fn points(path_id: u32, maps: &[u32]) -> Vec<TaxiPathNodeRow> {
        maps.iter()
            .enumerate()
            .map(|(index, &map)| TaxiPathNodeRow {
                id: TaxiPathNodeKey::new(path_id * 100 + index as u32),
                taxi_path: TaxiPathKey::new(path_id),
                node_index: index as i32,
                map: MapKey::new(map),
                location_x: index as f32,
                location_y: 0.0,
                location_z: 0.0,
                flags: 0,
                delay: 0,
            })
            .collect()
    }

    fn transport_source() -> crate::world_source::WorldSource {
        crate::world_source::fixture_sql("CREATE TABLE gameobject_template (
            entry INTEGER PRIMARY KEY, patch INTEGER DEFAULT 10, type INTEGER, displayId INTEGER,
            name TEXT, flags INTEGER, size REAL, data0 INTEGER, data1 INTEGER, data2 INTEGER);
            INSERT INTO gameobject_template (entry,type,displayId,name,flags,size,data0,data1,data2)
            VALUES (176244,15,3015,'Fixture boat A',40,1,293,30,1),
                   (176231,15,3015,'Fixture boat B',40,1,292,30,1),
                   (50100,3,259,'Chest',0,1,0,0,0);
            CREATE TABLE transports (entry INTEGER PRIMARY KEY, build INTEGER DEFAULT 5875, period INTEGER);
            INSERT INTO transports (entry,period) VALUES (176244,316251),(176231,329313),(50100,1000),(99999,1000);")
    }

    #[test]
    fn vmangos_transports_select_build_and_template_patch_by_name() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value::{Integer as N, Text};
        let template = |patch, display| {
            [
                ("entry", N(7)),
                ("patch", N(patch)),
                ("type", N(15)),
                ("displayId", N(display)),
                ("name", Text("Boat".into())),
                ("size", N(1)),
                ("flags", N(40)),
                ("data0", N(1)),
                ("data1", N(10)),
                ("data2", N(2)),
            ]
        };
        let transport =
            |build, period| [("entry", N(7)), ("build", N(build)), ("period", N(period))];
        let source = sqlite_fixture(&[
            FixtureTable {
                name: "gameobject_template",
                columns: &[
                    ("entry", "INTEGER"),
                    ("patch", "INTEGER"),
                    ("type", "INTEGER"),
                    ("displayId", "INTEGER"),
                    ("name", "TEXT"),
                    ("size", "REAL"),
                    ("flags", "INTEGER"),
                    ("data0", "INTEGER"),
                    ("data1", "INTEGER"),
                    ("data2", "INTEGER"),
                ],
                key: &["entry", "patch"],
                rows: &[&template(0, 1), &template(10, 2)],
            },
            FixtureTable {
                name: "transports",
                columns: &[
                    ("period", "INTEGER"),
                    ("build", "INTEGER"),
                    ("entry", "INTEGER"),
                ],
                key: &["entry", "build"],
                rows: &[
                    &transport(5860, 1000),
                    &transport(5875, 2000),
                    &transport(6000, 999),
                ],
            },
        ])
        .unwrap();
        let catalogue = catalogue_sql(
            &paths(&[1]),
            &TaxiPathNode {
                rows: points(1, &[0, 1, 1]),
            },
            &source,
        )
        .unwrap();
        assert_eq!(catalogue.count, 1);
        assert!(catalogue.statements[1].contains("(7,'Boat',2,1,40,1,10,2,2000,0)"));
    }

    #[test]
    fn a_route_imports_with_its_template_period_and_first_map() {
        let mut nodes = points(293, &[1, 1, 1, 1]);
        nodes.extend(points(292, &[0, 0, 1, 1]));
        let catalogue = catalogue_sql(
            &paths(&[292, 293]),
            &TaxiPathNode { rows: nodes },
            &transport_source(),
        )
        .unwrap();
        assert_eq!(catalogue.count, 2);
        assert_eq!(
            catalogue.statements[0],
            "DELETE FROM game_transport WHERE entry >= 0"
        );
        for row in [
            "(176244,'Fixture boat A',3015,1,40,293,30,1,316251,1)",
            "(176231,'Fixture boat B',3015,1,40,292,30,1,329313,0)",
        ] {
            assert!(
                catalogue.statements[1].contains(row),
                "{}",
                catalogue.statements[1]
            );
        }
    }

    #[test]
    fn routes_the_runtime_cannot_run_are_omitted_with_a_warning() {
        let mut nodes = points(293, &[1, 1, 1, 1]);
        nodes.extend(points(292, &[0, 0, 1, 1]));
        let catalogue = catalogue_sql(
            &paths(&[292, 293]),
            &TaxiPathNode { rows: nodes },
            &transport_source(),
        )
        .unwrap();
        let warnings = catalogue.warnings.join("\n");
        assert!(warnings.contains("transport 50100 is not an MO_TRANSPORT"));
        assert!(warnings.contains("transport 99999 has no gameobject_template row"));
    }

    #[test]
    fn a_dump_without_transports_fails_instead_of_clearing_them() {
        let error = catalogue_sql(
            &paths(&[293]),
            &TaxiPathNode { rows: vec![] },
            &crate::world_source::fixture_sql(
                "CREATE TABLE gameobject_template (entry INTEGER PRIMARY KEY, patch INTEGER);
             CREATE TABLE transports (entry INTEGER PRIMARY KEY, build INTEGER);",
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("no transports rows"), "{error}");
    }

    #[test]
    fn a_transport_without_imported_path_points_is_omitted() {
        let catalogue = catalogue_sql(
            &paths(&[293]),
            &TaxiPathNode { rows: vec![] },
            &transport_source(),
        )
        .unwrap();
        assert_eq!(catalogue.count, 0);
        assert!(catalogue
            .warnings
            .iter()
            .any(|warning| warning.contains("path 293 has no imported route")));
    }
}
