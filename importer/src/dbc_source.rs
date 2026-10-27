use super::*;
use crate::dump::TableRows;
use crate::world_source::{Row, SourceKind};

enum Kind {
    Unsigned,
    Signed,
    Float,
    Text,
}

fn translated(
    source: &(impl TableRows + ?Sized),
    table: &str,
    destination: &str,
    key: &str,
    fields: &[(&str, &str, Kind)],
) -> Result<(Vec<String>, usize)> {
    let mut values = Vec::new();
    for row in source.rows(table)?.iter() {
        let values_for_row = fields
            .iter()
            .map(|(_, name, kind)| {
                Ok(match kind {
                    Kind::Unsigned => row.number::<u32>(name)?.to_string(),
                    Kind::Signed => row.number::<i32>(name)?.to_string(),
                    Kind::Float => finite(row, name)?.to_string(),
                    Kind::Text => sql_text(row.text(name)?),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        values.push(format!("({})", values_for_row.join(",")));
    }
    let mut statements = vec![format!("DELETE FROM {destination} WHERE {key} >= 0")];
    push_insert(
        &mut statements,
        destination,
        &fields
            .iter()
            .map(|(name, _, _)| *name)
            .collect::<Vec<_>>()
            .join(","),
        &values,
    );
    Ok((statements, values.len()))
}

fn finite(row: &Row, name: &str) -> Result<f32> {
    let value: f32 = row.number(name)?;
    anyhow::ensure!(value.is_finite(), "{name}: non-finite catalogue coordinate");
    Ok(value)
}

pub(super) fn faction(source: &(impl TableRows + ?Sized)) -> Result<(Vec<String>, usize)> {
    use Kind::*;
    translated(
        source,
        "faction",
        "game_faction",
        "faction_id",
        &[
            ("faction_id", "id", Unsigned),
            ("reputation_index", "reputation_list_id", Signed),
            ("base_standing", "base_rep_value1", Signed),
        ],
    )
}

pub(super) fn faction_template(source: &(impl TableRows + ?Sized)) -> Result<(Vec<String>, usize)> {
    use Kind::*;
    translated(
        source,
        "faction_template",
        "game_faction_template",
        "id",
        &[
            ("id", "id", Unsigned),
            ("faction", "faction_id", Unsigned),
            ("faction_group", "our_mask", Unsigned),
            ("friend_group", "friendly_mask", Unsigned),
            ("enemy_group", "hostile_mask", Unsigned),
            ("enemy_0", "enemy_faction1", Unsigned),
            ("enemy_1", "enemy_faction2", Unsigned),
            ("enemy_2", "enemy_faction3", Unsigned),
            ("enemy_3", "enemy_faction4", Unsigned),
            ("friend_0", "friend_faction1", Unsigned),
            ("friend_1", "friend_faction2", Unsigned),
            ("friend_2", "friend_faction3", Unsigned),
            ("friend_3", "friend_faction4", Unsigned),
        ],
    )
}

pub(super) fn area(source: &(impl TableRows + ?Sized)) -> Result<(Vec<String>, usize)> {
    use Kind::*;
    translated(
        source,
        "area_template",
        "game_area",
        "id",
        &[
            ("id", "entry", Unsigned),
            ("map_id", "map_id", Unsigned),
            ("parent_area_id", "zone_id", Unsigned),
            ("area_bit", "explore_flag", Unsigned),
            ("flags", "flags", Unsigned),
            ("exploration_level", "area_level", Signed),
            ("faction_group", "team", Unsigned),
            ("name", "name", Text),
        ],
    )
}

pub(super) fn area_trigger(source: &(impl TableRows + ?Sized)) -> Result<(Vec<String>, usize)> {
    use Kind::*;
    for row in source.rows("areatrigger_template")?.iter() {
        for field in ["radius", "box_x", "box_y", "box_z"] {
            anyhow::ensure!(
                finite(row, field)? >= 0.0,
                "areatrigger_template {}: negative {field}",
                row.number::<u32>("id")?
            );
        }
    }
    translated(
        source,
        "areatrigger_template",
        "game_area_trigger",
        "id",
        &[
            ("id", "id", Unsigned),
            ("map_id", "map_id", Unsigned),
            ("x", "x", Float),
            ("y", "y", Float),
            ("z", "z", Float),
            ("radius", "radius", Float),
            ("box_length", "box_x", Float),
            ("box_width", "box_y", Float),
            ("box_height", "box_z", Float),
            ("box_yaw", "box_orientation", Float),
        ],
    )
}

pub(super) fn skill_ability(source: &(impl TableRows + ?Sized)) -> Result<(Vec<String>, usize)> {
    use Kind::*;
    translated(
        source,
        "skill_line_ability",
        "game_skill_ability",
        "id",
        &[
            ("id", "id", Unsigned),
            ("spell_id", "spell_id", Unsigned),
            ("skill_line", "skill_id", Unsigned),
            ("race_mask", "race_mask", Unsigned),
            ("class_mask", "class_mask", Unsigned),
            ("min_skill", "req_skill_value", Signed),
            ("acquire_method", "learn_on_get_skill", Signed),
            ("gray", "max_value", Signed),
            ("green", "min_value", Signed),
        ],
    )
}

pub(super) fn taxi_nodes(source: &(impl TableRows + ?Sized)) -> Result<DbcTaxiNodes> {
    use wow_dbc::vanilla_tables::map::MapKey;
    use wow_dbc::vanilla_tables::taxi_nodes::{TaxiNodesKey, TaxiNodesRow};
    let mut rows = Vec::new();
    for row in source.rows("taxi_nodes")?.iter() {
        rows.push(TaxiNodesRow {
            id: TaxiNodesKey::new(row.number("id")?),
            map: MapKey::new(row.number("map_id")?),
            location_x: finite(row, "x")?,
            location_y: finite(row, "y")?,
            location_z: finite(row, "z")?,
            name: wow_dbc::LocalizedString {
                en_gb: row.text("name")?.into(),
                ..Default::default()
            },
            mount_creature_display_info: [
                row.number("mount_creature_id1")?,
                row.number("mount_creature_id2")?,
            ],
        });
    }
    Ok(DbcTaxiNodes { rows })
}

pub(crate) fn findings(source: &impl TableRows) -> Result<serde_json::Value> {
    let mut findings = Vec::new();
    for (column, code) in [
        ("cooldown", "dbc.trigger-cooldown"),
        ("condition_id", "dbc.trigger-condition"),
        ("script_id", "dbc.trigger-script-id"),
    ] {
        let ids = source
            .rows("areatrigger_template")?
            .iter()
            .filter_map(|row| match row.number::<u32>(column) {
                Ok(0) => None,
                Ok(_) => Some(row.number::<u32>("id")),
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        findings.push(serde_json::json!({"code":code,"classification":"unsupported","occurrences":ids.len(),"source_ids":ids}));
    }
    for (table, key, code) in [
        ("areatrigger_template", "id", "dbc.trigger-script-name"),
        ("map_template", "entry", "dbc.map-script-name"),
    ] {
        let ids = source
            .rows(table)?
            .iter()
            .filter_map(|row| match row.optional_text("script_name") {
                Ok(None | Some("")) => None,
                Ok(Some(_)) => Some(row.number::<u32>(key)),
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        findings.push(serde_json::json!({"code":code,"classification":"unsupported","occurrences":ids.len(),"source_ids":ids}));
    }
    for (column, code, sentinel) in [
        ("player_limit", "dbc.map-player-limit", 0),
        ("reset_delay", "dbc.map-reset-delay", 0),
        ("ghost_entrance_map", "dbc.map-ghost-entrance", -1),
    ] {
        let ids = source
            .rows("map_template")?
            .iter()
            .filter_map(|row| match row.number::<i32>(column) {
                Ok(value) if value == sentinel => None,
                Ok(_) => Some(row.number::<u32>("entry")),
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        findings.push(serde_json::json!({"code":code,"classification":"unsupported","occurrences":ids.len(),"source_ids":ids}));
    }
    Ok(
        serde_json::json!({"format_version":1,"source":"vmangos","family":"dbc","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":findings}),
    )
}

pub(crate) fn world_plan(
    source: &impl TableRows,
    data_dir: &str,
) -> Result<crate::world_import::DumpPlan> {
    let vmangos = source.kind() == SourceKind::Vmangos;
    let mut chain = open_chain(Path::new(data_dir))?;
    let mut statements = Vec::new();
    for (sql, _) in [
        if vmangos {
            faction(source)?
        } else {
            faction_sql(&mut chain)?
        },
        if vmangos {
            faction_template(source)?
        } else {
            faction_template_sql(&mut chain)?
        },
        if vmangos {
            area(source)?
        } else {
            area_sql(&read_table(&mut chain)?)
        },
        if vmangos {
            area_trigger(source)?
        } else {
            area_trigger_sql(&read_table(&mut chain)?)
        },
        if vmangos {
            skill_ability(source)?
        } else {
            skill_ability_sql(&read_table(&mut chain)?)
        },
    ] {
        statements.extend(sql);
    }
    if vmangos {
        statements.extend(crate::map_catalogue::statements(source, &read_table(&mut chain)?)?.0);
    }
    let nodes = if vmangos {
        taxi_nodes(source)?
    } else {
        read_table(&mut chain)?
    };
    let paths = read_table(&mut chain)?;
    let points = read_table(&mut chain)?;
    let displays: CreatureDisplayInfo = read_table(&mut chain)?;
    let taxi = taxi_catalogue_sql(
        &nodes,
        &paths,
        &points,
        source,
        &displays.rows().iter().map(|r| r.id.id).collect(),
    )?;
    for warning in taxi.warnings {
        eprintln!("{warning}");
    }
    statements.extend(taxi.statements.into_iter().filter(|s| {
        s.starts_with("DELETE FROM game_taxi_node ") || s.starts_with("INSERT INTO game_taxi_node ")
    }));
    Ok(crate::world_import::DumpPlan {
        stamps: vec![("dbc", crate::dump::insert_row_count(&statements))],
        stmts: statements,
        world_source_manifest: if vmangos {
            Some(serde_json::to_string_pretty(&findings(source)?)?)
        } else {
            None
        },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::WorldSource;

    fn fixture() -> WorldSource {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE map_template (entry INTEGER, patch INTEGER, player_limit INTEGER, reset_delay INTEGER, ghost_entrance_map INTEGER, script_name TEXT, PRIMARY KEY(entry,patch)); INSERT INTO map_template VALUES (1,10,10,7,0,'instance_example'); CREATE TABLE faction (id INTEGER, build INTEGER,reputation_list_id INTEGER,base_rep_value1 INTEGER,PRIMARY KEY(id,build)); INSERT INTO faction VALUES (1,5875,-1,-100),(1,6000,5,999);
            CREATE TABLE faction_template (id INTEGER, build INTEGER, faction_id INTEGER,our_mask INTEGER,friendly_mask INTEGER,hostile_mask INTEGER,enemy_faction1 INTEGER,enemy_faction2 INTEGER,enemy_faction3 INTEGER,enemy_faction4 INTEGER,friend_faction1 INTEGER,friend_faction2 INTEGER,friend_faction3 INTEGER,friend_faction4 INTEGER,PRIMARY KEY(id,build)); INSERT INTO faction_template VALUES(1,5875,1,2,3,4,5,6,7,8,9,10,11,12);
            CREATE TABLE area_template (entry INTEGER,map_id INTEGER,zone_id INTEGER,explore_flag INTEGER,flags INTEGER,area_level INTEGER,team INTEGER,name TEXT); INSERT INTO area_template VALUES (1,0,2,3,4,-1,6,'O''Brien');
            CREATE TABLE areatrigger_template (id INTEGER,build INTEGER,map_id INTEGER,x REAL,y REAL,z REAL,radius REAL,box_x REAL,box_y REAL,box_z REAL,box_orientation REAL,cooldown INTEGER,condition_id INTEGER,script_id INTEGER,script_name TEXT,PRIMARY KEY(id,build)); INSERT INTO areatrigger_template VALUES (1,5875,0,1,2,3,4,5,6,7,8,9,10,11,'example');
            CREATE TABLE skill_line_ability (id INTEGER,build INTEGER,spell_id INTEGER,skill_id INTEGER,race_mask INTEGER,class_mask INTEGER,req_skill_value INTEGER,learn_on_get_skill INTEGER,max_value INTEGER,min_value INTEGER,PRIMARY KEY(id,build)); INSERT INTO skill_line_ability VALUES(1,5875,10,20,3,4,5,2,100,75);
            CREATE TABLE taxi_nodes (id INTEGER,build INTEGER,map_id INTEGER,x REAL,y REAL,z REAL,name TEXT,mount_creature_id1 INTEGER,mount_creature_id2 INTEGER,PRIMARY KEY(id,build)); INSERT INTO taxi_nodes VALUES (1,5875,0,1,2,3,'Source',100,200),(1,6000,0,9,9,9,'Future',1,2);").unwrap();
        WorldSource::Vmangos(connection)
    }

    #[test]
    fn named_source_fields_keep_signed_reputation_geometry_and_skill_semantics() {
        let source = fixture();
        for (sql, expected) in [
            (faction(&source).unwrap(), "(1,-1,-100)"),
            (
                faction_template(&source).unwrap(),
                "(1,1,2,3,4,5,6,7,8,9,10,11,12)",
            ),
            (area(&source).unwrap(), "(1,0,2,3,4,-1,6,'O''Brien')"),
            (area_trigger(&source).unwrap(), "(1,0,1,2,3,4,5,6,7,8)"),
            (skill_ability(&source).unwrap(), "(1,10,20,3,4,5,2,100,75)"),
        ] {
            assert_eq!(sql.1, 1);
            assert!(sql.0.join("\n").contains(expected), "{:?}", sql.0);
        }
        let nodes = taxi_nodes(&source).unwrap();
        assert_eq!(nodes.rows()[0].name.en_gb, "Source");
        assert_eq!(nodes.rows()[0].mount_creature_display_info, [100, 200]);
        assert_eq!(nodes.rows()[0].location_x, 1.0);
        let report = findings(&source).unwrap();
        assert!(report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["occurrences"] == 1));
    }

    #[test]
    fn source_taxi_mounts_resolve_creature_entries_and_keep_source_coordinates() {
        let source = fixture();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection.execute_batch("CREATE TABLE creature_template (entry INTEGER,patch INTEGER,display_id1 INTEGER,display_id2 INTEGER,display_id3 INTEGER,display_id4 INTEGER,PRIMARY KEY(entry,patch)); INSERT INTO creature_template VALUES (100,10,0,295,0,0),(200,10,1147,0,0,0);").unwrap();
        let nodes = taxi_nodes(&source).unwrap();
        let mounts = taxi_mount_displays(&source, &HashSet::from([295, 1147])).unwrap();
        let node = &nodes.rows()[0];
        assert_eq!(node.location_x, 1.0);
        assert_eq!(mounts[&node.mount_creature_display_info[0]], 295);
        assert_eq!(mounts[&node.mount_creature_display_info[1]], 1147);
        let missing = taxi_mount_displays(&source, &HashSet::from([295])).unwrap();
        assert_eq!(missing[&200], 0);
    }

    #[test]
    fn invalid_geometry_and_missing_fields_fail_before_a_plan_is_returned() {
        let source = fixture();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection
            .execute("UPDATE areatrigger_template SET radius=-1", [])
            .unwrap();
        assert!(area_trigger(&source)
            .unwrap_err()
            .to_string()
            .contains("negative radius"));
        connection
            .execute("ALTER TABLE area_template DROP COLUMN name", [])
            .unwrap();
        assert!(area(&source).unwrap_err().to_string().contains("name"));
    }
}
