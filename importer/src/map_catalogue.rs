use std::collections::BTreeMap;

use anyhow::{ensure, Context, Result};
use wow_dbc::vanilla_tables::map::Map;

use crate::dump::{push_insert, sql_text, TableRows};

pub(crate) fn archive_names(maps: &Map) -> Result<BTreeMap<u32, &str>> {
    let mut names = BTreeMap::new();
    for row in &maps.rows {
        let name = row.internal_name.as_str();
        ensure!(
            !name.is_empty()
                && name.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b' ' | b'\'')),
            "Map.dbc map {} has unsafe internal name {name:?}",
            row.id.id
        );
        ensure!(
            names.insert(row.id.id, name).is_none(),
            "Map.dbc repeats map {}",
            row.id.id
        );
    }
    Ok(names)
}

pub(crate) fn statements(
    source: &(impl TableRows + ?Sized),
    maps: &Map,
) -> Result<(Vec<String>, usize)> {
    let names = archive_names(maps)?;
    let mut values = Vec::new();
    for row in source.rows("map_template")?.iter() {
        let id = row.number::<u32>("entry")?;
        let name = names
            .get(&id)
            .with_context(|| format!("Map.dbc has no map {id}"))?;
        let x = row.number::<f32>("ghost_entrance_x")?;
        let y = row.number::<f32>("ghost_entrance_y")?;
        ensure!(
            x.is_finite() && y.is_finite(),
            "map_template {id}: non-finite ghost entrance"
        );
        values.push(format!(
            "({id},{},{},{},{},{},{},{},{},{x},{y})",
            sql_text(name),
            sql_text(row.text("map_name")?),
            row.number::<u32>("map_type")?,
            row.number::<u32>("linked_zone")?,
            row.number::<u32>("parent")?,
            row.number::<u32>("player_limit")?,
            row.number::<u32>("reset_delay")?,
            row.number::<i32>("ghost_entrance_map")?,
        ));
    }
    let mut statements = vec!["DELETE FROM game_map WHERE id >= 0".into()];
    push_insert(&mut statements, "game_map", "id,internal_name,name,map_type,linked_zone,parent,player_limit,reset_delay,ghost_entrance_map,ghost_entrance_x,ghost_entrance_y", &values);
    Ok((statements, values.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::WorldSource;
    use wow_dbc::vanilla_tables::map::{MapKey, MapRow};

    fn source() -> WorldSource {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE map_template (entry INTEGER, patch INTEGER, parent INTEGER, map_type INTEGER, linked_zone INTEGER, player_limit INTEGER, reset_delay INTEGER, ghost_entrance_map INTEGER, ghost_entrance_x REAL, ghost_entrance_y REAL, map_name TEXT, PRIMARY KEY(entry,patch)); INSERT INTO map_template VALUES (36,0,0,1,7,40,0,0,-10,20,'Old'),(36,10,1,2,8,10,7,-1,-11,21,'Source'),(36,11,2,3,9,5,8,1,-12,22,'Future');").unwrap();
        WorldSource::Vmangos(connection)
    }

    fn maps() -> Map {
        Map {
            rows: vec![MapRow {
                id: MapKey::new(36),
                internal_name: "DeadminesInstance".into(),
                instance_type: wow_world_base::vanilla::InstanceType::Normal,
                battleground: false,
                map_name: Default::default(),
                min_level: 0,
                max_level: 0,
                max_players: 99,
                unknown: [0; 3],
                area_table: Default::default(),
                map_description_horde: Default::default(),
                map_description_alliance: Default::default(),
                loading_screen: Default::default(),
                raid_offset: 0,
                unknown_2: [0; 2],
            }],
        }
    }

    #[test]
    fn gameplay_uses_selected_source_row_and_archive_name_uses_only_client_id() {
        let (sql, count) = statements(&source(), &maps()).unwrap();
        assert_eq!(count, 1);
        assert!(sql
            .join("\n")
            .contains("(36,'DeadminesInstance','Source',2,8,1,10,7,-1,-11,21)"));
    }

    #[test]
    fn missing_or_unsafe_archive_name_refuses_the_map_plan() {
        assert!(statements(&source(), &Map { rows: vec![] })
            .unwrap_err()
            .to_string()
            .contains("no map 36"));
        let mut maps = maps();
        maps.rows[0].internal_name = "../Other".into();
        assert!(statements(&source(), &maps)
            .unwrap_err()
            .to_string()
            .contains("unsafe internal name"));
    }

    #[test]
    #[ignore = "requires LYRACORE_VMANGOS_SQLITE and LYRACORE_CLIENT_DATA"]
    fn source_maps_keep_client_archive_lookup() {
        use std::path::Path;
        let client = std::env::var("LYRACORE_CLIENT_DATA").unwrap();
        let sqlite = std::env::var("LYRACORE_VMANGOS_SQLITE").unwrap();
        let source = WorldSource::open(Path::new(&sqlite)).unwrap();
        let mut dbc = crate::dbc::open_chain(Path::new(&client)).unwrap();
        let maps: Map = crate::dbc::read_table(&mut dbc).unwrap();
        let names = archive_names(&maps).unwrap();
        let (_, count) = statements(&source, &maps).unwrap();
        let mut geometry = crate::collision::open_geometry_chain(Path::new(&client)).unwrap();
        let mut checked = 0;
        for row in source.rows("map_template").unwrap().iter() {
            let id = row.number::<u32>("entry").unwrap();
            let name = names[&id];
            let wdt = format!("World\\Maps\\{name}\\{name}.wdt");
            geometry
                .read_file(&wdt)
                .unwrap_or_else(|error| panic!("map {id}: {wdt}: {error}"));
            let tiles = crate::terrain::map_tiles(&mut geometry, name)
                .unwrap()
                .unwrap();
            if let Some(tile) = tiles.iter().position(|present| *present) {
                let adt = format!(
                    "World\\Maps\\{name}\\{name}_{}_{}.adt",
                    tile % 64,
                    tile / 64
                );
                geometry
                    .read_file(&adt)
                    .unwrap_or_else(|error| panic!("map {id}: {adt}: {error}"));
            }
            checked += 1;
        }
        assert_eq!(checked, count);
    }
}
