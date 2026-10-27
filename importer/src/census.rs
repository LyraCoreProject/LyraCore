use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result};
use wow_dbc::DbcTable;

use crate::dump::TableRows;
use crate::world_source::{SourceKind, WorldSource};

mod eventai;
mod planned;

type Keys = BTreeSet<Vec<u32>>;

struct Count {
    source_rows: usize,
    selected_rows: usize,
    keys: Keys,
    compared: BTreeMap<Vec<u32>, Vec<(&'static str, String)>>,
}

const GLOBALS: &[(&str, &str, &[&str])] = &[
    (
        "player_classlevelstats",
        "game_class_level_stats",
        &["class", "level"],
    ),
    (
        "player_levelstats",
        "game_level_stats",
        &["race", "class", "level"],
    ),
    (
        "playercreateinfo",
        "game_start_position",
        &["race", "class"],
    ),
    (
        "game_graveyard_zone",
        "game_graveyard_zone",
        &["id", "ghost_zone"],
    ),
    (
        "playercreateinfo_spell",
        "game_createinfo_spell",
        &["race", "class", "spell"],
    ),
    (
        "playercreateinfo_action",
        "game_createinfo_action",
        &["race", "class", "button"],
    ),
    (
        "playercreateinfo_item",
        "game_start_item",
        &["race", "class", "itemid"],
    ),
    ("areatrigger_teleport", "game_areatrigger_teleport", &["id"]),
];

fn count(
    source: &WorldSource,
    kind: SourceKind,
    table: &str,
    key: &[&str],
    outfits: &BTreeSet<(u64, i32)>,
) -> Result<Count> {
    let rows = source.rows(table)?;
    let mut keys = Keys::new();
    let mut compared = BTreeMap::new();
    for row in rows.iter() {
        let numeric: &[&str] = match table {
            "player_classlevelstats" => &["basehp", "basemana"],
            "player_levelstats" => &["str", "agi", "sta", "inte", "spi"],
            "playercreateinfo" => &[
                "map",
                "zone",
                "position_x",
                "position_y",
                "position_z",
                "orientation",
            ],
            "game_graveyard_zone" => &["faction"],
            "playercreateinfo_action" => &["action", "type"],
            "areatrigger_teleport" => &[
                "target_map",
                "target_position_x",
                "target_position_y",
                "target_position_z",
                "target_orientation",
            ],
            _ => &[],
        };
        for column in numeric {
            row.number::<f64>(column)?;
        }
        if table == "areatrigger_teleport" {
            row.text("name")?;
        }
        if table == "playercreateinfo_item" && row.number::<i32>("itemid")? <= 0 {
            continue;
        }
        let values = key
            .iter()
            .map(|&column| {
                let column = match (kind, table, column) {
                    (SourceKind::ClassicDb, "game_graveyard_zone", "ghost_zone") => "ghost_loc",
                    (SourceKind::ClassicDb, "playercreateinfo_spell", "spell") => "Spell",
                    _ => column,
                };
                row.number::<u32>(column)
            })
            .collect::<Result<Vec<_>>>()?;
        if table == "playercreateinfo_item" {
            let item: i32 = row.number("itemid")?;
            let race_class = (u64::from(values[0]) << 8) | u64::from(values[1]);
            if item <= 0 || outfits.contains(&(race_class, item)) {
                continue;
            }
        }
        let fields = match table {
            "game_graveyard_zone" => vec![("faction", row.number::<u32>("faction")?.to_string())],
            "areatrigger_teleport" => {
                let mut fields = vec![
                    ("target_map", row.number::<u32>("target_map")?.to_string()),
                    ("name", row.text("name")?.trim().to_string()),
                ];
                for name in [
                    "target_position_x",
                    "target_position_y",
                    "target_position_z",
                    "target_orientation",
                ] {
                    fields.push((name, row.number::<f32>(name)?.to_string()));
                }
                fields
            }
            _ => Vec::new(),
        };
        compared.insert(values.clone(), fields);
        keys.insert(values);
    }
    Ok(Count {
        source_rows: source.row_count(table)?,
        selected_rows: rows.len(),
        keys,
        compared,
    })
}

fn ids(keys: impl Iterator<Item = Vec<u32>>) -> String {
    let ids = keys
        .map(|key| key.iter().map(u32::to_string).collect::<Vec<_>>().join(":"))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

fn floor(name: &str) -> Result<usize> {
    include_str!("../scripts/import-manifest.sh")
        .lines()
        .find_map(|line| line.strip_prefix(&format!("FLOOR_{name}=")))
        .with_context(|| format!("missing consumed floor {name}"))?
        .parse()
        .map_err(Into::into)
}

pub(crate) fn run(
    classic: &Path,
    vmangos: &Path,
    client: &str,
    commit: &str,
    args: &crate::world_import::WorldImportArgs,
) -> Result<String> {
    if args.family.as_deref() != Some("globals") {
        return planned::run(classic, vmangos, commit, args);
    }
    let sqlite_digest = crate::dump::sha256_hex(&std::fs::read(vmangos)?)?;
    let classic = WorldSource::open(classic, SourceKind::ClassicDb)?;
    let classic_digest = classic.source_hash()?;
    let vmangos = WorldSource::open(vmangos, SourceKind::Vmangos)?;
    let mut chain = crate::dbc::open_chain(Path::new(client))?;
    let outfits = crate::dbc::outfit_item_set(&mut chain)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let safe_locs: wow_dbc::vanilla_tables::world_safe_locs::WorldSafeLocs =
        crate::dbc::read_table(&mut chain)?;
    let areas: wow_dbc::vanilla_tables::area_table::AreaTable = crate::dbc::read_table(&mut chain)?;
    let parents = areas
        .rows()
        .iter()
        .map(|row| (row.id.id, row.parent_area_table.id))
        .collect::<BTreeMap<_, _>>();
    let classic_sql = crate::world_import::globals_statements(&classic, Some(client))?;
    let vmangos_sql = crate::world_import::globals_statements(&vmangos, Some(client))?;
    let emitted = |statements: &[String], table: &str| {
        let prefix = format!("INSERT INTO {table} (");
        statements
            .iter()
            .filter(|statement| statement.starts_with(&prefix))
            .map(|statement| statement.matches("),(").count() + 1)
            .sum::<usize>()
    };
    let safe_ids = safe_locs
        .rows()
        .iter()
        .map(|row| row.id.id)
        .collect::<BTreeSet<_>>();
    let mut output = String::from("# Globals census\n\nGenerated by `lyracore-importer --source vmangos --dump <sqlite> --census <classic.sql> --family globals --dbc <client/Data>`.\n\nPatch 10, client build 5875. Module row counts come from the production globals builders. These are planned rows, not counts read from a Realm.\n\n`game_start_item` counts only globals extras after CharStartOutfit and duplicate removal. The DBC outfit rows belong to the client catalogue. Keys below are source entry ids or natural composite keys, not generated Module row ids.\n\n| Module table | Classic source | vmangos source | vmangos selected | Classic Module rows | vmangos Module rows |\n| --- | ---: | ---: | ---: | ---: | ---: |\n");
    let provenance = format!("vmangos commit: `{}`. Classic SQL SHA-256: `{classic_digest}`. SQLite SHA-256: `{sqlite_digest}`.\n\n", if commit.is_empty() { "unspecified" } else { commit });
    output.insert_str(
        output.find("Patch 10").expect("report heading"),
        &provenance,
    );
    let mut counts = BTreeMap::new();
    for &(table, module, key) in GLOBALS {
        let before = count(&classic, SourceKind::ClassicDb, table, key, &outfits)?;
        let after = count(&vmangos, SourceKind::Vmangos, table, key, &outfits)?;
        writeln!(
            output,
            "| {module} | {} | {} | {} | {} | {} |",
            before.source_rows,
            after.source_rows,
            after.selected_rows,
            before.keys.len(),
            after.keys.len()
        )?;
        anyhow::ensure!(
            before.keys.len() == emitted(&classic_sql, module),
            "{module}: classic census does not match its plan"
        );
        anyhow::ensure!(
            after.keys.len() == emitted(&vmangos_sql, module),
            "{module}: vmangos census does not match its plan"
        );
        counts.insert(table, (before, after));
    }
    for &(table, module, key) in GLOBALS {
        let (before, after) = &counts[table];
        writeln!(
            output,
            "\n## {module}\n\nKey: `{}`.\n\nAdded: {}.\n\nRemoved: {}.",
            key.join(":"),
            ids(after.keys.difference(&before.keys).cloned()),
            ids(before.keys.difference(&after.keys).cloned())
        )?;
        writeln!(output, "\nSelection removes {} vmangos rows. After selection and import deduplication, the sources differ by {} added and {} removed keys.", after.source_rows - after.selected_rows, after.keys.difference(&before.keys).count(), before.keys.difference(&after.keys).count())?;
        if matches!(table, "game_graveyard_zone" | "areatrigger_teleport") {
            explain_differences(&mut output, table, before, after, &parents)?;
        }
        if table == "playercreateinfo_item" {
            writeln!(output, "\nCharStartOutfit overlap and duplicate extras remove {} classic-db rows and {} selected vmangos rows. The current import emits one extra per race, class and positive item id; it does not multiply rows by `amount`.", before.selected_rows - before.keys.len(), after.selected_rows - after.keys.len())?;
        }
    }
    writeln!(output, "\n## Consumed floors\n\nFloors come from `importer/scripts/import-manifest.sh`. Creation-item floors include the same client outfit keys in both sources. Graveyard resolution joins the source links to the supplied client's WorldSafeLocs. Zones 1, 38, 141 and 148 are the full-world corridor checks; zone 12 is the legacy map-0 check.\n\n| Floor | Minimum | Classic | vmangos |\n| --- | ---: | ---: | ---: |")?;
    for (race, map, name, item_name) in [
        (1, 0, "HUMAN_START_POSITIONS", "START_ITEMS_CLASSES"),
        (3, 0, "DWARF_START_POSITIONS", "DWARF_START_ITEMS_CLASSES"),
        (7, 0, "GNOME_START_POSITIONS", "GNOME_START_ITEMS_CLASSES"),
        (
            4,
            1,
            "NIGHT_ELF_START_POSITIONS",
            "NIGHT_ELF_START_ITEMS_CLASSES",
        ),
    ] {
        let positions = |source: &WorldSource| -> Result<usize> {
            source
                .rows("playercreateinfo")?
                .iter()
                .map(|row| {
                    Ok(usize::from(
                        row.number::<u32>("race")? == race && row.number::<u32>("map")? == map,
                    ))
                })
                .sum()
        };
        writeln!(
            output,
            "| {name} | {} | {} | {} |",
            floor(name)?,
            positions(&classic)?,
            positions(&vmangos)?
        )?;
        let item_classes = |count: &Count| {
            let mut classes = outfits
                .iter()
                .filter(|(race_class, _)| *race_class >> 8 == u64::from(race))
                .map(|(race_class, _)| (*race_class & 255) as u32)
                .collect::<BTreeSet<_>>();
            classes.extend(
                count
                    .keys
                    .iter()
                    .filter(|key| key[0] == race)
                    .map(|key| key[1]),
            );
            let allowed: &[u32] = match race {
                1 => &[1, 2, 4, 5, 8, 9],
                3 => &[1, 2, 3, 4, 5],
                7 => &[1, 4, 8, 9],
                4 => &[1, 3, 4, 5, 11],
                _ => &[],
            };
            classes
                .iter()
                .filter(|class| allowed.contains(class))
                .count()
        };
        let (before, after) = &counts["playercreateinfo_item"];
        writeln!(
            output,
            "| {item_name} | {} | {} | {} |",
            floor(item_name)?,
            item_classes(before),
            item_classes(after)
        )?;
    }
    let (before, after) = &counts["areatrigger_teleport"];
    writeln!(
        output,
        "| AREATRIGGER_TELEPORTS | {} | {} | {} |",
        floor("AREATRIGGER_TELEPORTS")?,
        before.keys.len(),
        after.keys.len()
    )?;
    let portals = |count: &Count| {
        count
            .keys
            .iter()
            .filter(|key| matches!(key[0], 78 | 119))
            .count()
    };
    writeln!(
        output,
        "| DEADMINES_PORTAL_ROUNDTRIP | {} | {} | {} |",
        floor("DEADMINES_PORTAL_ROUNDTRIP")?,
        portals(before),
        portals(after)
    )?;
    let (before, after) = &counts["game_graveyard_zone"];
    for zone in [1, 38, 141, 148, 12] {
        let resolved = |count: &Count| {
            count
                .keys
                .iter()
                .filter(|key| key[1] == zone && safe_ids.contains(&key[0]))
                .count()
        };
        writeln!(
            output,
            "| GRAVEYARD_ZONE_RESOLVE zone {zone} | {} | {} | {} |",
            floor("GRAVEYARD_ZONE_RESOLVE")?,
            resolved(before),
            resolved(after)
        )?;
    }
    let conditioned = vmangos
        .rows("areatrigger_teleport")?
        .iter()
        .map(|row| Ok(usize::from(row.number::<u32>("required_condition")? != 0)))
        .sum::<Result<usize>>()?;
    let patch_conditions = vmangos
        .rows("conditions")?
        .iter()
        .map(|row| Ok(usize::from(row.number::<i32>("type")? == 24)))
        .sum::<Result<usize>>()?;
    writeln!(output, "\n## Compatibility Manifest\n\nThe existing EventAI Compatibility Manifest is classic-db-only. A vmangos Source Profile and its approved value gate are due in S6c; this census does not claim a passed gate.\n\nThe globals builders use every column read by today's globals import, with `ghost_zone` and `spell` mapped to their classic-db counterparts. No missing consumed column was found.\n\nSelected vmangos portals have {conditioned} nonzero `required_condition` values. Today's Module portal catalogue has no condition field. The census counts these portals at current import parity and records the condition as unsupported. The source has {patch_conditions} WOW_PATCH conditions; their values pass the shared selection module's bounds checks. S3g must translate conditions before gameplay can use them.")?;
    writeln!(
        output,
        "\nThe production globals Compatibility Manifest reports:\n\n```json\n{}\n```",
        serde_json::to_string_pretty(&crate::world_import::globals_findings(&vmangos)?)?
    )?;
    Ok(output)
}

fn explain_differences(
    output: &mut String,
    table: &str,
    before: &Count,
    after: &Count,
    parents: &BTreeMap<u32, u32>,
) -> Result<()> {
    writeln!(output, "\nEach selected-row difference:")?;
    for (label, added, removed) in [("Added", after, before), ("Removed", before, after)] {
        for key in added.keys.difference(&removed.keys) {
            let id = ids(std::iter::once(key.clone()));
            if table == "game_graveyard_zone" {
                let location = match parents.get(&key[1]) {
                    Some(0) => format!("zone {}", key[1]),
                    Some(parent) => format!("area {} in zone {parent}", key[1]),
                    None => format!("location {} (absent from the client's AreaTable)", key[1]),
                };
                let counterpart = removed.keys.iter().any(|other| other[0] == key[0]);
                writeln!(output, "\n- {label} `{id}`: safe location {} links to {location}; the other source {}.", key[0], if counterpart { "links that safe location to different areas or zones" } else { "has no selected link for that safe location" })?;
            } else {
                let fields = &added.compared[key];
                let destination = fields
                    .iter()
                    .find(|(name, _)| *name == "target_map")
                    .context("portal destination")?
                    .1
                    .as_str();
                writeln!(output, "\n- {label} `{id}`: a selected portal to map {destination} exists only in this source; the other source has no selected row for this trigger.")?;
            }
        }
    }
    for key in before.keys.intersection(&after.keys) {
        let changed = before.compared[key]
            .iter()
            .zip(&after.compared[key])
            .filter(|(left, right)| left != right)
            .map(|((name, _), _)| *name)
            .collect::<Vec<_>>();
        if !changed.is_empty() {
            writeln!(output, "\n- Changed `{}`: selected source values differ in {}; the globals builder preserves those values.", ids(std::iter::once(key.clone())), changed.join(", "))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::{sqlite_fixture, FixtureTable};
    use rusqlite::types::Value;

    #[test]
    fn starter_extra_census_excludes_outfits_and_deduplicates_natural_keys() {
        let source = sqlite_fixture(&[FixtureTable {
            name: "playercreateinfo_item",
            columns: &[
                ("race", "INTEGER"),
                ("class", "INTEGER"),
                ("itemid", "INTEGER"),
            ],
            key: &[],
            rows: &[
                &[
                    ("race", Value::Integer(1)),
                    ("class", Value::Integer(1)),
                    ("itemid", Value::Integer(42)),
                ],
                &[
                    ("race", Value::Integer(1)),
                    ("class", Value::Integer(1)),
                    ("itemid", Value::Integer(43)),
                ],
                &[
                    ("race", Value::Integer(1)),
                    ("class", Value::Integer(1)),
                    ("itemid", Value::Integer(43)),
                ],
            ],
        }])
        .unwrap();
        let count = count(
            &source,
            SourceKind::Vmangos,
            "playercreateinfo_item",
            &["race", "class", "itemid"],
            &BTreeSet::from([(257, 42)]),
        )
        .unwrap();
        assert_eq!(count.source_rows, 3);
        assert_eq!(count.keys, BTreeSet::from([vec![1, 1, 43]]));
    }
}
